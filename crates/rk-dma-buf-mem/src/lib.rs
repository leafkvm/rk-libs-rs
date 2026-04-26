use nix::fcntl::{open, OFlag};
use nix::sys::mman::{mmap, munmap, MapFlags, ProtFlags};
use nix::sys::stat::Mode;
use nix::unistd::close;
use std::ffi::c_void;
use std::num::NonZeroUsize;
use std::os::fd::{FromRawFd as _, OwnedFd};
use std::os::unix::io::{AsFd, AsRawFd, RawFd};
use std::ptr::NonNull;
use std::slice;

const HEAP_PATH: &str = "/dev/rk_dma_heap/rk-dma-heap-cma";

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

/// A struct that represents a DMA buffer, holding ownership of the file
/// descriptor and the memory-mapped region.
pub struct DmaBuffer {
    fd: OwnedFd,
    ptr: NonNull<c_void>,
    size: usize,
}

impl std::fmt::Debug for DmaBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DmaBuffer")
            .field("fd", &self.fd)
            .field("size", &self.size)
            .finish()
    }
}

impl DmaBuffer {
    /// Allocates a new DMA buffer.
    ///
    /// # Arguments
    ///
    /// * `heap_path` - The path to the DMA heap device.
    /// * `size` - The desired size of the buffer in bytes.
    ///
    /// # Returns
    ///
    /// A `Result` containing the new `DmaBuffer` on success, or a `nix::Error` on failure.
    pub fn new(size: usize) -> Result<Self, nix::Error> {
        let heap_fd = open(HEAP_PATH, OFlag::O_RDWR, Mode::empty())?;

        let mut alloc_data = DmaHeapAllocationData {
            len: size as u64,
            fd_flags: (OFlag::O_CLOEXEC | OFlag::O_RDWR).bits() as u32,
            ..Default::default()
        };

        // The allocation happens here.
        unsafe {
            dma_heap_alloc(heap_fd.as_raw_fd(), &mut alloc_data)?;
        }

        close(heap_fd)?;

        let fd = unsafe { OwnedFd::from_raw_fd(alloc_data.fd) };

        let prot = ProtFlags::PROT_READ | ProtFlags::PROT_WRITE;
        let map_flags = MapFlags::MAP_SHARED;
        let ptr = unsafe {
            mmap(
                None,
                NonZeroUsize::try_from(size).unwrap(),
                prot,
                map_flags,
                &fd,
                0,
            )?
        };

        Ok(Self { fd, ptr, size })
    }

    /// Synchronizes the buffer from the device to the CPU.
    pub fn sync_device_to_cpu(&self) -> Result<(), nix::Error> {
        let mut sync_data = DmaBufSync {
            flags: DMA_BUF_SYNC_START | DMA_BUF_SYNC_RW,
        };
        unsafe {
            dma_buf_sync(self.fd.as_raw_fd(), &mut sync_data)?;
        }
        Ok(())
    }

    /// Synchronizes the buffer from the CPU to the device.
    pub fn sync_cpu_to_device(&self) -> Result<(), nix::Error> {
        let mut sync_data = DmaBufSync {
            flags: DMA_BUF_SYNC_END | DMA_BUF_SYNC_RW,
        };
        unsafe {
            dma_buf_sync(self.fd.as_raw_fd(), &mut sync_data)?;
        }
        Ok(())
    }

    /// Returns the size of the buffer.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns a shared slice over the buffer contents.
    pub fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr() as *const u8, self.size) }
    }

    /// Returns a mutable slice over the buffer contents.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr() as *mut u8, self.size) }
    }
}

impl AsRawFd for DmaBuffer {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        unsafe {
            if let Err(e) = munmap(self.ptr, self.size) {
                // In a real application, you might want to log this error.
                eprintln!("Failed to unmap memory: {}", e);
            }
        }
    }
}

impl AsFd for DmaBuffer {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

// Ensure the DmaBuffer is Send + Sync as it holds a raw pointer,
// but the underlying memory and fd are safe to be sent across threads.
unsafe impl Send for DmaBuffer {}
unsafe impl Sync for DmaBuffer {}
