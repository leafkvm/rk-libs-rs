//! `/dev/kmpp_objs` client: kernel object definitions and shared objects.

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;

use crate::abi::{
    KmppEntry, KmppShmPtr, ShmIoc1, ENTRY_TYPE_LOC_TBL, KMPP_SHM_IOC_GET_SHM,
    KMPP_SHM_IOC_PUT_SHM, KMPP_SHM_IOC_QUERY_INFO,
};
use crate::trie::Trie;
use crate::{Error, Result};

/// One kernel object type: its name and field-lookup trie.
#[derive(Debug)]
pub struct ObjDef {
    name: String,
    /// `None` for object types whose kernel sub-trie cannot be walked — see
    /// [`Trie::try_from_root`]. Such a type has no accessible fields.
    trie: Option<Trie>,
    entry_size: usize,
}

impl ObjDef {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Kernel-side payload size of one object of this type.
    pub fn entry_size(&self) -> usize {
        self.entry_size
    }

    /// Look up one field's `{offset, size, flag}` descriptor.
    pub fn field(&self, name: &str) -> Option<KmppEntry> {
        let info = self.trie?.lookup(name)?;
        let e = KmppEntry(info.ctx_u64()?);
        (e.entry_type() == ENTRY_TYPE_LOC_TBL).then_some(e)
    }

    /// Iterate all field names (skipping `__`-reserved records).
    pub fn field_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Some(trie) = self.trie else {
            return out;
        };
        let mut info = trie.info_first();
        while let Some(i) = info {
            let name = i.name();
            if !name.contains("__") {
                out.push(name.to_string());
            }
            info = trie.info_next(i);
        }
        out
    }
}

#[derive(Debug)]
struct KmppInner {
    fd: OwnedFd,
    root: Trie,
    defs: Vec<ObjDef>,
    /// Payload offset from the shm block base (`sizeof(KmppShmImpl)`).
    entry_offset: usize,
    /// Offset of the objdef-name offset word inside the shm block header.
    name_offset: usize,
}

/// Connection to `/dev/kmpp_objs`.
///
/// Owns the device fd and the kernel objdef trie mapping; cheap to clone
/// (shared handle). All [`Obj`]s allocated from it keep it alive, and the
/// kernel frees any leaked shared objects when the last handle (fd) closes.
#[derive(Debug, Clone)]
pub struct Kmpp {
    inner: Arc<KmppInner>,
}

// `KmppInner` is immutable after `open()`: the trie mapping is written only
// by the kernel at setup and read-only from here on, and the fd is used for
// thread-safe ioctls. The raw trie pointers are what suppress the auto impls.
unsafe impl Send for Kmpp {}
unsafe impl Sync for Kmpp {}

impl Kmpp {
    /// Open `/dev/kmpp_objs` and import the kernel's object definitions.
    pub fn open() -> Result<Kmpp> {
        let fd = unsafe {
            let raw = libc::open(
                c"/dev/kmpp_objs".as_ptr(),
                libc::O_RDWR | libc::O_CLOEXEC,
            );
            if raw < 0 {
                return Err(Error::sys("open /dev/kmpp_objs"));
            }
            OwnedFd::from_raw_fd(raw)
        };

        // The kernel mmaps its objdef trie into this process and hands back
        // the userspace address. The mapping lives until the fd closes.
        let mut root_uaddr: u64 = 0;
        let ret = unsafe {
            libc::ioctl(
                fd.as_raw_fd(),
                KMPP_SHM_IOC_QUERY_INFO as _,
                &mut root_uaddr,
            )
        };
        if ret < 0 || root_uaddr == 0 {
            return Err(Error::sys("ioctl KMPP_SHM_IOC_QUERY_INFO"));
        }

        let root = unsafe { Trie::from_root(root_uaddr as usize as *const u8)? };

        let get_u32 = |name: &str| -> Result<u32> {
            root.lookup(name)
                .and_then(|i| i.ctx_u32())
                .ok_or(Error::BadKernelData("objdef trie missing self info"))
        };
        let count = get_u32("__count")? as usize;
        let entry_offset = get_u32("__offset")? as usize;
        let name_offset = get_u32("__name_offset")? as usize;

        // The first `count` info records are the object definitions, each
        // holding the byte offset of that type's own sub-trie.
        let mut defs = Vec::with_capacity(count);
        let mut info = root.info_first();
        for _ in 0..count {
            let i = info.ok_or(Error::BadKernelData("objdef trie truncated"))?;
            let name = i.name().to_string();
            let sub_off = i
                .ctx_u32()
                .ok_or(Error::BadKernelData("objdef record has no offset"))? as usize;
            // A sub-trie the shared walk cannot enter yields a field-less
            // objdef, exactly as it does in Rockchip's own client.
            let trie =
                unsafe { Trie::try_from_root((root_uaddr as usize + sub_off) as *const u8) };
            let entry_size = trie
                .and_then(|t| t.lookup("__size"))
                .and_then(|s| s.ctx_u32())
                .unwrap_or(0) as usize;

            log::debug!("kmpp objdef {name:16} entry_size {entry_size}");
            defs.push(ObjDef {
                name,
                trie,
                entry_size,
            });
            info = root.info_next(i);
        }

        Ok(Kmpp {
            inner: Arc::new(KmppInner {
                fd,
                root,
                defs,
                entry_offset,
                name_offset,
            }),
        })
    }

    /// All object types the kernel exposes.
    pub fn defs(&self) -> impl Iterator<Item = &ObjDef> {
        self.inner.defs.iter()
    }

    /// Find an object type by its trie key (the interface typedef name, e.g.
    /// `"KmppFrame"`, `"KmppPacket"`, `"KmppVencInitCfg"`, `"KmppVencStCfg"`).
    pub fn def(&self, name: &str) -> Option<&ObjDef> {
        // Definitions are indexed by the root trie's info index.
        let idx = self.inner.root.lookup(name)?.index() as usize;
        self.inner.defs.get(idx)
    }

    fn def_index(&self, name: &str) -> Result<usize> {
        let idx = self
            .inner
            .root
            .lookup(name)
            .map(|i| i.index() as usize)
            .filter(|&i| i < self.inner.defs.len())
            .ok_or_else(|| Error::UnknownObjType(name.to_string()))?;
        Ok(idx)
    }

    /// Allocate one kernel shared object of the named type.
    pub fn alloc(&self, type_name: &str) -> Result<Obj> {
        let def_idx = self.def_index(type_name)?;
        let cname = CString::new(type_name).expect("object type name with NUL");

        let mut ioc = ShmIoc1 {
            count: 1,
            flag: 0,
            slot: KmppShmPtr::default(),
        };
        // On GET the slot initially carries the pointer to the name string.
        ioc.slot.uaddr = cname.as_ptr() as u64;

        let ret = unsafe {
            libc::ioctl(
                self.inner.fd.as_raw_fd(),
                KMPP_SHM_IOC_GET_SHM as _,
                &mut ioc,
            )
        };
        if ret < 0 || ioc.slot.is_null() {
            return Err(Error::sys("ioctl KMPP_SHM_IOC_GET_SHM"));
        }

        Ok(Obj {
            kmpp: self.clone(),
            def_idx,
            sptr: ioc.slot,
        })
    }

    /// Wrap a [`KmppShmPtr`] handed back by the kernel (e.g. an encoded
    /// packet from `VCODEC_CHAN_OUT_PKT_RDY`) into an owned [`Obj`].
    ///
    /// The object's type is recovered from the shm block header, which
    /// records the offset of the type name inside the objdef trie buffer.
    /// Ownership transfers: dropping the `Obj` releases the kernel object.
    pub fn adopt(&self, sptr: KmppShmPtr) -> Result<Obj> {
        if sptr.is_null() {
            return Err(Error::BadKernelData("NULL shm pointer"));
        }
        let name_off = unsafe {
            ((sptr.uaddr as usize + self.inner.name_offset) as *const u32).read_volatile()
        } as usize;
        if name_off == 0 {
            return Err(Error::BadKernelData("shm block has no type name"));
        }
        let name = unsafe {
            let p = self.inner.root.root().add(name_off);
            let mut len = 0usize;
            while len < 64 && *p.add(len) != 0 {
                len += 1;
            }
            core::str::from_utf8(core::slice::from_raw_parts(p, len))
                .map_err(|_| Error::BadKernelData("shm type name not UTF-8"))?
        };
        let def_idx = self.def_index(name)?;
        Ok(Obj {
            kmpp: self.clone(),
            def_idx,
            sptr,
        })
    }

    fn put(&self, sptr: KmppShmPtr) {
        let mut ioc = ShmIoc1 {
            count: 1,
            flag: 0,
            slot: sptr,
        };
        let ret = unsafe {
            libc::ioctl(
                self.inner.fd.as_raw_fd(),
                KMPP_SHM_IOC_PUT_SHM as _,
                &mut ioc,
            )
        };
        if ret < 0 {
            log::warn!(
                "KMPP_SHM_IOC_PUT_SHM failed for {:#x}: {}",
                sptr.uaddr,
                std::io::Error::last_os_error()
            );
        }
    }
}

/// One kernel-allocated shared-memory object, mapped into this process.
///
/// Fields are accessed by name; offsets and sizes come from the kernel's own
/// tables, so a name the kernel doesn't know fails cleanly instead of
/// scribbling at a stale offset. Dropping the `Obj` returns it to the kernel.
///
/// Field writes mark the kernel-side "updated" flag bit when the value
/// changes, which is what makes partial `MPP_ENC_SET_CFG` updates work.
#[derive(Debug)]
pub struct Obj {
    kmpp: Kmpp,
    def_idx: usize,
    sptr: KmppShmPtr,
}

// The shared mapping is plain memory owned by this process + kernel; the Obj
// is the unique handle to it.
unsafe impl Send for Obj {}

impl Obj {
    pub fn def(&self) -> &ObjDef {
        &self.kmpp.inner.defs[self.def_idx]
    }

    pub fn type_name(&self) -> &str {
        self.def().name()
    }

    /// The `{uaddr, kaddr}` pair to pass across `/dev/vcodec` ioctls.
    pub fn shm_ptr(&self) -> &KmppShmPtr {
        &self.sptr
    }

    fn entry_base(&self) -> *mut u8 {
        (self.sptr.uaddr as usize + self.kmpp.inner.entry_offset) as *mut u8
    }

    fn field(&self, name: &str) -> Result<KmppEntry> {
        self.def().field(name).ok_or_else(|| Error::UnknownField {
            obj: "kmpp",
            field: name.to_string(),
        })
    }

    fn check_size(&self, name: &str, e: KmppEntry, wanted: usize) -> Result<()> {
        if e.elem_size() != wanted {
            return Err(Error::SizeMismatch {
                obj: "kmpp",
                field: name.to_string(),
                actual: e.elem_size(),
                wanted,
            });
        }
        Ok(())
    }

    /// Set the per-field update flag. `flag_offset` is a bit position within
    /// the flags area that follows the entry payload.
    fn raise_flag(&self, e: KmppEntry) {
        let bit = e.flag_offset();
        if bit == 0 {
            return;
        }
        unsafe {
            let word = self.entry_base().add((bit & !31) / 8) as *mut u32;
            word.write_volatile(word.read_volatile() | 1u32 << (bit & 31));
        }
    }

    /// Write a field and mark it present in this update.
    ///
    /// The flag is raised on every write, including one that stores the value
    /// the field already held. The kernel's own setters
    /// (`KMPP_OBJ_ACCESS_IMPL` in `sys/kmpp_obj.c`) skip the flag when
    /// `old == val`, which makes a value indistinguishable from "untouched"
    /// whenever it matches what is already there — and a freshly allocated
    /// object is all-zero, so writing an explicit 0 to one would never reach
    /// the encoder. `rc:gop` is the case that bites: 0 means "infinite gop,
    /// no periodic intra frame", but the encoder's own default is 60, so a
    /// skipped flag silently leaves periodic IDRs switched on.
    ///
    /// The flags live in the shared entry that userspace maps writable, so
    /// raising them unconditionally needs no kernel change; the consumer
    /// (`kmpp_obj_update`) only ever tests the bit.
    fn set_bytes_inner(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let e = self.field(name)?;
        self.check_size(name, e, bytes.len())?;
        unsafe {
            let dst = self.entry_base().add(e.elem_offset());
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        }
        self.raise_flag(e);
        Ok(())
    }

    fn get_bytes_inner(&self, name: &str, bytes: &mut [u8]) -> Result<()> {
        let e = self.field(name)?;
        self.check_size(name, e, bytes.len())?;
        unsafe {
            let src = self.entry_base().add(e.elem_offset());
            core::ptr::copy_nonoverlapping(src as *const u8, bytes.as_mut_ptr(), bytes.len());
        }
        Ok(())
    }

    pub fn set_u32(&self, name: &str, v: u32) -> Result<()> {
        self.set_bytes_inner(name, &v.to_ne_bytes())
    }
    pub fn set_s32(&self, name: &str, v: i32) -> Result<()> {
        self.set_bytes_inner(name, &v.to_ne_bytes())
    }
    pub fn set_u64(&self, name: &str, v: u64) -> Result<()> {
        self.set_bytes_inner(name, &v.to_ne_bytes())
    }
    pub fn set_s64(&self, name: &str, v: i64) -> Result<()> {
        self.set_bytes_inner(name, &v.to_ne_bytes())
    }

    pub fn get_u32(&self, name: &str) -> Result<u32> {
        let mut b = [0u8; 4];
        self.get_bytes_inner(name, &mut b)?;
        Ok(u32::from_ne_bytes(b))
    }
    pub fn get_s32(&self, name: &str) -> Result<i32> {
        let mut b = [0u8; 4];
        self.get_bytes_inner(name, &mut b)?;
        Ok(i32::from_ne_bytes(b))
    }
    pub fn get_u64(&self, name: &str) -> Result<u64> {
        let mut b = [0u8; 8];
        self.get_bytes_inner(name, &mut b)?;
        Ok(u64::from_ne_bytes(b))
    }
    pub fn get_s64(&self, name: &str) -> Result<i64> {
        let mut b = [0u8; 8];
        self.get_bytes_inner(name, &mut b)?;
        Ok(i64::from_ne_bytes(b))
    }

    /// Set a struct/array field (`st` entries such as `hw:qbias_arr`). The
    /// slice must exactly match the kernel-side field size.
    pub fn set_struct_bytes(&self, name: &str, bytes: &[u8]) -> Result<()> {
        self.set_bytes_inner(name, bytes)
    }

    /// Convenience for `i32`-array struct fields (AQ / qbias tables).
    pub fn set_struct_s32s(&self, name: &str, vals: &[i32]) -> Result<()> {
        let mut bytes = Vec::with_capacity(vals.len() * 4);
        for v in vals {
            bytes.extend_from_slice(&v.to_ne_bytes());
        }
        self.set_bytes_inner(name, &bytes)
    }

    /// Size of a field in bytes, for sizing struct writes.
    pub fn field_size(&self, name: &str) -> Result<usize> {
        Ok(self.field(name)?.elem_size())
    }

    /// Read an embedded [`KmppShmPtr`] field (e.g. a packet's `pos`).
    pub fn get_shm_ptr(&self, name: &str) -> Result<KmppShmPtr> {
        let mut b = [0u8; 16];
        self.get_bytes_inner(name, &mut b)?;
        Ok(KmppShmPtr {
            uaddr: u64::from_ne_bytes(b[0..8].try_into().unwrap()),
            kaddr: u64::from_ne_bytes(b[8..16].try_into().unwrap()),
        })
    }
}

impl Drop for Obj {
    fn drop(&mut self) {
        self.kmpp.put(self.sptr);
    }
}
