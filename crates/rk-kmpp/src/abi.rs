//! Raw kernel ABI shared with `kmpp.ko`.
//!
//! Sources of truth (kernel tree `ipc_drv_ko/kmpp/kmpp`):
//! * `include/rk-mpp-kobj.h` — `KmppObjShm`, `KmppShmReq`, `KmppObjIocArg`
//! * `include/kmpi_defs.h` — `KmppShmPtr`
//! * `sys/kmpp_shm.c` — `/dev/kmpp_objs` ioctl numbers
//! * `sys/inc/kmpp_sys_defs.h` — `KmppEntry` (the 64-bit field descriptor)
//! * `vcodec/mpp_vcodec_driver.c` — `/dev/vcodec` ioctl + `struct vcodec_msg`
//! * `osal/driver/inc/mpp_vcodec_client.h` — `vcodec_event_id`
//! * `inc/rk_mpi_cmd.h` (rockchip-mpp) — `MpiCmd` control command ids

/// Linux `_IOW(magic, nr, unsigned int)`.
const fn iow_uint(magic: u8, nr: u8) -> libc::c_ulong {
    // dir = _IOC_WRITE (1) << 30 | size << 16 | magic << 8 | nr
    (1 << 30) | ((core::mem::size_of::<libc::c_uint>() as libc::c_ulong) << 16)
        | ((magic as libc::c_ulong) << 8)
        | (nr as libc::c_ulong)
}

// /dev/kmpp_objs ('m')
pub const KMPP_SHM_IOC_QUERY_INFO: libc::c_ulong = iow_uint(b'm', 1);
pub const KMPP_SHM_IOC_RELEASE_INFO: libc::c_ulong = iow_uint(b'm', 2);
pub const KMPP_SHM_IOC_GET_SHM: libc::c_ulong = iow_uint(b'm', 3);
pub const KMPP_SHM_IOC_PUT_SHM: libc::c_ulong = iow_uint(b'm', 4);
pub const KMPP_SHM_IOC_DUMP: libc::c_ulong = iow_uint(b'm', 5);

// /dev/vcodec ('V')
pub const VOCDEC_IOC_CFG: libc::c_ulong = iow_uint(b'V', 1);

/// `{uaddr, kaddr}` handle of one kernel shared-memory object. This 16-byte
/// pair is the *only* thing that crosses the ioctl boundary for objects.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KmppShmPtr {
    /// Userspace base address of the shm block (`KmppShmImpl` head).
    pub uaddr: u64,
    /// Kernel address of the same block. Opaque cookie — never dereference.
    pub kaddr: u64,
}

impl KmppShmPtr {
    pub fn is_null(&self) -> bool {
        self.uaddr == 0
    }
}

/// Ioctl argument for `KMPP_SHM_IOC_GET_SHM` / `PUT_SHM` with one slot.
///
/// The kernel reads `{count, flag}` then walks 16-byte slots. On GET the
/// slot holds a userspace pointer to a NUL-terminated object type name and
/// is overwritten with the resulting [`KmppShmPtr`]; on PUT it holds the
/// [`KmppShmPtr`] to release.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ShmIoc1 {
    pub count: u32,
    pub flag: u32,
    pub slot: KmppShmPtr,
}

/// `struct vcodec_msg` — the single `/dev/vcodec` ioctl payload.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VcodecMsg {
    pub cmd: u32,
    pub ctrl_cmd: u32,
    pub size: u32,
    // 4 bytes implicit padding before the u64 in the C struct as well
    pub data_ptr: u64,
}

// vcodec_event_id — /dev/vcodec `cmd` values
pub const VCODEC_CHAN_CREATE: u32 = 0x0000_0000;
pub const VCODEC_CHAN_DESTROY: u32 = 0x0000_0001;
pub const VCODEC_CHAN_RESET: u32 = 0x0000_0002;
pub const VCODEC_CHAN_CONTROL: u32 = 0x0000_0003;
pub const VCODEC_CHAN_START: u32 = 0x0000_0100;
pub const VCODEC_CHAN_STOP: u32 = 0x0000_0101;
pub const VCODEC_CHAN_PAUSE: u32 = 0x0000_0102;
pub const VCODEC_CHAN_RESUME: u32 = 0x0000_0103;
pub const VCODEC_CHAN_IN_FRM_RDY: u32 = 0x0000_0400;
pub const VCODEC_CHAN_OUT_PKT_RDY: u32 = 0x0000_0603;

// MpiCmd control command ids (inc/rk_mpi_cmd.h) used with VCODEC_CHAN_CONTROL.
pub const MPP_ENC_SET_CFG: u32 = 0x0032_0001;
pub const MPP_ENC_GET_CFG: u32 = 0x0032_0002;
pub const MPP_ENC_SET_IDR_FRAME: u32 = 0x0032_0009;
pub const MPP_ENC_SET_SEI_CFG: u32 = 0x0032_000f;
pub const MPP_ENC_SET_ROI_CFG: u32 = 0x0032_0013;
pub const MPP_ENC_SET_HEADER_MODE: u32 = 0x0032_8001;
pub const MPP_ENC_SET_REF_CFG: u32 = 0x0032_8201;
pub const MPP_ENC_SET_OSD_DATA_CFG: u32 = 0x0032_8403;
pub const MPP_ENC_SET_USERDATA: u32 = 0x0032_8501;

/// `KmppEntry` — the kernel's 64-bit field descriptor stored as trie payload
/// for every named object field. Decoded from the `tbl` union variant:
///
/// ```text
/// bits  0..4   EntryType (3 = location table)
/// bits  4..8   EntryLocTblFlag
/// bits  8..16  ElemType
/// bits 16..32  elem_size    (bytes)
/// bits 32..48  elem_offset  (bytes, from entry base)
/// bits 48..64  flag_offset  (BIT position of the update flag; 0 = none)
/// ```
#[derive(Debug, Clone, Copy)]
pub struct KmppEntry(pub u64);

impl KmppEntry {
    pub fn entry_type(&self) -> u8 {
        (self.0 & 0xf) as u8
    }
    pub fn elem_type(&self) -> u8 {
        ((self.0 >> 8) & 0xff) as u8
    }
    pub fn elem_size(&self) -> usize {
        ((self.0 >> 16) & 0xffff) as usize
    }
    pub fn elem_offset(&self) -> usize {
        ((self.0 >> 32) & 0xffff) as usize
    }
    /// Bit position of the "field updated" flag, 0 when the field has none.
    pub fn flag_offset(&self) -> usize {
        ((self.0 >> 48) & 0xffff) as usize
    }
}

/// `EntryType::ENTRY_TYPE_LOC_TBL` — the only entry type that describes an
/// accessible field.
pub const ENTRY_TYPE_LOC_TBL: u8 = 0x3;
