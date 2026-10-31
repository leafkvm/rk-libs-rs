use nix::fcntl::{open, OFlag};
use nix::sys::mman::{mmap, munmap, MapFlags, ProtFlags};
use nix::sys::stat::Mode;
use std::ffi::c_void;
use std::num::NonZeroUsize;
use std::ops::{Deref, DerefMut};
use std::os::fd::{FromRawFd as _, OwnedFd};
use std::os::unix::io::{AsFd, AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::slice;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Standard cached, scatter-gather DMA heap for devices behind an IOMMU.
pub const DEFAULT_HEAP_PATH: &str = "/dev/dma_heap/system";

/// Process-wide allocation policy, read once on first allocation.
/// Set `RK_DMA_HEAP` before starting the process to select a board's contiguous
/// heap when its devices have no IOMMU. There is deliberately no fallback: a
/// fallback can hide an invalid DMA policy or an exhausted heap.
pub fn default_heap_path() -> &'static Path {
    static HEAP_PATH: OnceLock<PathBuf> = OnceLock::new();
    HEAP_PATH.get_or_init(|| {
        let path = std::env::var_os("RK_DMA_HEAP")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_HEAP_PATH));
        log::info!("DMA buffer heap: {}", path.display());
        path
    })
}

// Define the necessary constants from the C header.
const DMA_HEAP_IOC_MAGIC: u8 = b'H';
const DMA_BUF_BASE: u8 = b'b';

const DMA_BUF_SYNC_READ: u64 = 1 << 0;
const DMA_BUF_SYNC_WRITE: u64 = 2;
const DMA_BUF_SYNC_RW: u64 = DMA_BUF_SYNC_READ | DMA_BUF_SYNC_WRITE;
const DMA_BUF_SYNC_START: u64 = 0 << 2;
const DMA_BUF_SYNC_END: u64 = 1 << 2;

// Define the ioctl request codes using the nix macro.
nix::ioctl_readwrite!(
    dma_heap_alloc,
    DMA_HEAP_IOC_MAGIC,
    0x0,
    DmaHeapAllocationData
);
nix::ioctl_write_ptr!(dma_buf_sync, DMA_BUF_BASE, 0, DmaBufSync);

#[repr(C)]
#[derive(Debug, Default)]
struct DmaHeapAllocationData {
    len: u64,
    fd: i32,
    fd_flags: u32,
    heap_flags: u64,
}

#[repr(C)]
#[derive(Debug, Default)]
struct DmaBufSync {
    flags: u64,
}

/// A DMA-BUF mapping which owns its FD. Imported mappings are read-only.
///
/// CPU slices are available only through access guards. Callers must arrange
/// device completion before CPU access and avoid submitting or recycling the
/// buffer until all guards have dropped. SYNC handles cache maintenance and
/// registered implicit fences, not arbitrary device ownership or buffer reuse.
#[derive(Debug)]
pub struct MappedDmaBuffer {
    fd: OwnedFd,
    ptr: NonNull<c_void>,
    size: usize,
    // Serialize read scopes too: one reader's END must not run while another
    // reader is still using the mapping. Separate mappings/FD aliases remain
    // the caller's responsibility.
    readers: Mutex<()>,
}

impl MappedDmaBuffer {
    /// Duplicates an imported FD and maps `size` bytes read-only.
    pub fn map_read_only(fd: std::os::fd::BorrowedFd<'_>, size: usize) -> std::io::Result<Self> {
        let fd = fd.try_clone_to_owned()?;
        Self::map_owned(fd, size, ProtFlags::PROT_READ).map_err(std::io::Error::from)
    }

    fn map_owned(fd: OwnedFd, size: usize, prot: ProtFlags) -> Result<Self, nix::Error> {
        let length = NonZeroUsize::new(size).ok_or(nix::errno::Errno::EINVAL)?;
        // SAFETY: the mapping owns the FD and unmaps exactly this region on drop.
        let ptr = unsafe { mmap(None, length, prot, MapFlags::MAP_SHARED, &fd, 0)? };
        Ok(Self {
            fd,
            ptr,
            size,
            readers: Mutex::new(()),
        })
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Begins a read scope. END runs on drop, including during unwinding.
    /// Do not nest read scopes on the same mapping: they are serialized.
    /// Persistent sync errors abort the process; EINTR/EAGAIN are retried.
    pub fn read(&self) -> CpuReadAccess<'_> {
        // This lock protects access scope ordering, not data that could become
        // inconsistent after a panic. The previous guard still issued END.
        let lock = self.readers.lock().unwrap_or_else(|e| e.into_inner());
        self.sync(DMA_BUF_SYNC_START | DMA_BUF_SYNC_READ);
        CpuReadAccess {
            mapping: self,
            _lock: lock,
        }
    }

    fn sync(&self, flags: u64) {
        let sync_data = DmaBufSync { flags };
        loop {
            // SAFETY: the FD is owned and the ioctl argument has the UAPI layout.
            match unsafe { dma_buf_sync(self.fd.as_raw_fd(), &sync_data) } {
                Ok(_) => return,
                Err(nix::errno::Errno::EINTR | nix::errno::Errno::EAGAIN) => continue,
                Err(err) => {
                    // A panic could kill only a worker, or panic again during
                    // unwinding. Never continue with uncertain cache ownership.
                    // Ignore diagnostic I/O errors so they cannot prevent abort.
                    use std::io::Write;
                    let operation = if flags & DMA_BUF_SYNC_END != 0 {
                        "END"
                    } else {
                        "START"
                    };
                    let _ = writeln!(
                        std::io::stderr().lock(),
                        "fatal DMA_BUF_IOCTL_SYNC {operation}: fd={}, flags={flags:#x}: {err}",
                        self.fd.as_raw_fd()
                    );
                    std::process::abort();
                }
            }
        }
    }
}

impl AsRawFd for MappedDmaBuffer {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl AsFd for MappedDmaBuffer {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl Drop for MappedDmaBuffer {
    fn drop(&mut self) {
        // SAFETY: guards borrow this mapping and cannot outlive it.
        if let Err(e) = unsafe { munmap(self.ptr, self.size) } {
            log::error!("Failed to unmap DMA buffer: {e}");
        }
    }
}

// SAFETY: the owned mapping can move between threads. Shared CPU access is
// read-only and serialized; writable access requires an exclusive borrow.
// Device access and other aliases must obey the ownership contract above.
unsafe impl Send for MappedDmaBuffer {}
unsafe impl Sync for MappedDmaBuffer {}

/// Read-only CPU access, ending before the borrowed mapping can be destroyed.
#[must_use = "keep the guard alive for the entire CPU access"]
pub struct CpuReadAccess<'a> {
    mapping: &'a MappedDmaBuffer,
    _lock: MutexGuard<'a, ()>,
}

impl Deref for CpuReadAccess<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: START completed; the slice is bounded by this guard's borrow.
        unsafe { slice::from_raw_parts(self.mapping.ptr.as_ptr().cast(), self.mapping.size) }
    }
}

impl Drop for CpuReadAccess<'_> {
    fn drop(&mut self) {
        // The lock is released after END, when fields are dropped.
        self.mapping.sync(DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ);
    }
}

/// Writable CPU access. A write-only scope must not read previous contents;
/// use `read_write()` for read-modify-write operations or partial rendering.
#[must_use = "keep the guard alive for the entire CPU access"]
pub struct CpuWriteAccess<'a> {
    mapping: &'a mut MappedDmaBuffer,
    direction: u64,
}

impl Deref for CpuWriteAccess<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: START completed and this guard exclusively borrows the mapping.
        unsafe { slice::from_raw_parts(self.mapping.ptr.as_ptr().cast(), self.mapping.size) }
    }
}

impl DerefMut for CpuWriteAccess<'_> {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: the mutable slice cannot outlive the exclusive guard borrow.
        unsafe { slice::from_raw_parts_mut(self.mapping.ptr.as_ptr().cast(), self.mapping.size) }
    }
}

impl Drop for CpuWriteAccess<'_> {
    fn drop(&mut self) {
        self.mapping.sync(DMA_BUF_SYNC_END | self.direction);
    }
}

/// Heap-allocated DMA-BUF with a writable mapping and scoped CPU access.
///
/// Every access guard retries EINTR/EAGAIN on START and END. Other sync errors
/// abort the process, including on drop; allocation and mapping remain fallible.
#[derive(Debug)]
pub struct DmaBuffer {
    mapping: MappedDmaBuffer,
}

impl DmaBuffer {
    /// Allocates from the process-wide heap selected by [`default_heap_path`].
    pub fn new(size: usize) -> Result<Self, nix::Error> {
        Self::new_in_heap(default_heap_path(), size)
    }

    /// Allocates from an explicit DMA heap using the standard Linux heap ABI.
    pub fn new_in_heap(heap_path: &Path, size: usize) -> Result<Self, nix::Error> {
        Self::allocate(heap_path, size).map_err(|err| {
            log::error!(
                "DMA allocation: heap={}, size={size}: {err}",
                heap_path.display()
            );
            err
        })
    }

    fn allocate(heap_path: &Path, size: usize) -> Result<Self, nix::Error> {
        NonZeroUsize::new(size).ok_or(nix::errno::Errno::EINVAL)?;
        let heap_fd = open(heap_path, OFlag::O_RDWR | OFlag::O_CLOEXEC, Mode::empty())?;
        let mut alloc_data = DmaHeapAllocationData {
            len: size as u64,
            fd_flags: (OFlag::O_CLOEXEC | OFlag::O_RDWR).bits() as u32,
            ..Default::default()
        };
        // SAFETY: valid heap FD and allocation ioctl argument.
        unsafe {
            dma_heap_alloc(heap_fd.as_raw_fd(), &mut alloc_data)?;
        }
        // SAFETY: successful allocation transfers ownership of this new FD.
        let fd = unsafe { OwnedFd::from_raw_fd(alloc_data.fd) };
        let mapping =
            MappedDmaBuffer::map_owned(fd, size, ProtFlags::PROT_READ | ProtFlags::PROT_WRITE)?;
        Ok(Self { mapping })
    }

    pub fn size(&self) -> usize {
        self.mapping.size()
    }

    pub fn read(&self) -> CpuReadAccess<'_> {
        self.mapping.read()
    }

    /// Begins write-only CPU access. Do not read previous contents through the
    /// returned slice. Device submission must follow the guard's drop.
    pub fn write(&mut self) -> CpuWriteAccess<'_> {
        self.access_mut(DMA_BUF_SYNC_WRITE)
    }

    /// Begins CPU access which may read and modify existing contents.
    pub fn read_write(&mut self) -> CpuWriteAccess<'_> {
        self.access_mut(DMA_BUF_SYNC_RW)
    }

    fn access_mut(&mut self, direction: u64) -> CpuWriteAccess<'_> {
        self.mapping.sync(DMA_BUF_SYNC_START | direction);
        CpuWriteAccess {
            mapping: &mut self.mapping,
            direction,
        }
    }
}

impl AsRawFd for DmaBuffer {
    fn as_raw_fd(&self) -> RawFd {
        self.mapping.as_raw_fd()
    }
}

impl AsFd for DmaBuffer {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.mapping.as_fd()
    }
}
