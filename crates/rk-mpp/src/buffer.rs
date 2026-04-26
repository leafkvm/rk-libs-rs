use std::ffi::c_void;
use std::fmt;
use std::mem::ManuallyDrop;
use std::os::fd::RawFd;
use std::ptr;

use crate::{check_ret, sys, Result, CALLER, TAG};

// ============================================================
// Buffer type and flags
// ============================================================

/// Combined buffer allocator type and flags, used when creating a buffer group.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct BufferType(pub u32);

impl BufferType {
    pub const NORMAL: Self = Self(sys::MppBufferType_MPP_BUFFER_TYPE_NORMAL);
    pub const ION: Self = Self(sys::MppBufferType_MPP_BUFFER_TYPE_ION);
    pub const EXT_DMA: Self = Self(sys::MppBufferType_MPP_BUFFER_TYPE_EXT_DMA);
    pub const DRM: Self = Self(sys::MppBufferType_MPP_BUFFER_TYPE_DRM);
    pub const DMA_HEAP: Self = Self(sys::MppBufferType_MPP_BUFFER_TYPE_DMA_HEAP);

    pub const FLAG_CONTIG: u32 = 0x0001_0000;
    pub const FLAG_CACHABLE: u32 = 0x0002_0000;
    pub const FLAG_WC: u32 = 0x0004_0000;
    pub const FLAG_SECURE: u32 = 0x0008_0000;
    pub const FLAG_ALLOC_KMAP: u32 = 0x0010_0000;
    pub const FLAG_DMA32: u32 = 0x0020_0000;

    const TYPE_MASK: u32 = 0x0000_FFFF;
    const FLAGS_MASK: u32 = 0x003F_0000;

    /// Add a flag to this buffer type.
    pub const fn with_flag(self, flag: u32) -> Self {
        Self(self.0 | flag)
    }

    pub const fn cachable(self) -> Self {
        self.with_flag(Self::FLAG_CACHABLE)
    }

    pub const fn contig(self) -> Self {
        self.with_flag(Self::FLAG_CONTIG)
    }

    pub const fn secure(self) -> Self {
        self.with_flag(Self::FLAG_SECURE)
    }

    pub const fn dma32(self) -> Self {
        self.with_flag(Self::FLAG_DMA32)
    }

    /// Extract the allocator type (lower 16 bits).
    pub const fn base_type(self) -> u32 {
        self.0 & Self::TYPE_MASK
    }

    /// Extract the flags (upper 16 bits).
    pub const fn flags(self) -> u32 {
        self.0 & Self::FLAGS_MASK
    }

    /// Get the raw combined value.
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Debug for BufferType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.base_type() {
            sys::MppBufferType_MPP_BUFFER_TYPE_NORMAL => "NORMAL",
            sys::MppBufferType_MPP_BUFFER_TYPE_ION => "ION",
            sys::MppBufferType_MPP_BUFFER_TYPE_EXT_DMA => "EXT_DMA",
            sys::MppBufferType_MPP_BUFFER_TYPE_DRM => "DRM",
            sys::MppBufferType_MPP_BUFFER_TYPE_DMA_HEAP => "DMA_HEAP",
            _ => "UNKNOWN",
        };
        let flags = self.flags();
        if flags != 0 {
            write!(f, "BufferType({name}|{flags:#x})")
        } else {
            write!(f, "BufferType({name})")
        }
    }
}

// ============================================================
// BufferInfo
// ============================================================

/// Buffer metadata used for import and commit operations.
///
/// This mirrors the C `MppBufferInfo` struct. Fields:
/// - `buf_type`: allocator type (ION, DRM, EXT_DMA, etc.)
/// - `size`: buffer size in bytes
/// - `ptr`: CPU virtual address (may be null)
/// - `fd`: DMA-BUF file descriptor (-1 if not applicable)
/// - `index`: buffer index within a group
pub struct BufferInfo {
    pub buf_type: BufferType,
    pub size: usize,
    pub ptr: *mut c_void,
    pub fd: RawFd,
    pub index: i32,
}

// BufferInfo is a pure data struct describing a buffer; the raw pointer
// is just a value passed through to the C library.
unsafe impl Send for BufferInfo {}
unsafe impl Sync for BufferInfo {}

impl BufferInfo {
    /// Create a `BufferInfo` for importing an external DMA-BUF.
    ///
    /// `fd` is the DMA-BUF file descriptor to import.
    pub fn for_ext_dma(fd: RawFd, size: usize) -> Self {
        Self {
            buf_type: BufferType::EXT_DMA,
            size,
            ptr: ptr::null_mut(),
            fd,
            index: 0,
        }
    }

    /// Create a `BufferInfo` for committing a buffer to an external group.
    ///
    /// Both `fd` and `ptr` should describe the same underlying buffer.
    pub fn for_commit(
        buf_type: BufferType,
        fd: RawFd,
        ptr: *mut c_void,
        size: usize,
        index: i32,
    ) -> Self {
        Self {
            buf_type,
            size,
            ptr,
            fd,
            index,
        }
    }

    pub(crate) fn to_sys(&self) -> sys::MppBufferInfo_t {
        sys::MppBufferInfo_t {
            type_: self.buf_type.0,
            size: self.size,
            ptr: self.ptr,
            hnd: ptr::null_mut(),
            fd: self.fd,
            index: self.index,
        }
    }

    pub(crate) fn from_sys(info: &sys::MppBufferInfo_t) -> Self {
        Self {
            buf_type: BufferType(info.type_),
            size: info.size,
            ptr: info.ptr,
            fd: info.fd,
            index: info.index,
        }
    }
}

impl Default for BufferInfo {
    fn default() -> Self {
        Self {
            buf_type: BufferType::NORMAL,
            size: 0,
            ptr: ptr::null_mut(),
            fd: -1,
            index: 0,
        }
    }
}

impl fmt::Debug for BufferInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufferInfo")
            .field("buf_type", &self.buf_type)
            .field("size", &self.size)
            .field("ptr", &self.ptr)
            .field("fd", &self.fd)
            .field("index", &self.index)
            .finish()
    }
}

// ============================================================
// MppBuffer
// ============================================================

/// An owned handle to an MPP buffer.
pub struct Buffer {
    raw: sys::MppBuffer,
}

// MPP's internal reference counting is thread-safe.
unsafe impl Send for Buffer {}
unsafe impl Sync for Buffer {}

impl Buffer {
    /// Allocate a new buffer from the given group.
    ///
    /// Equivalent to the C macro `mpp_buffer_get(group, &buffer, size)`.
    pub fn get(group: &BufferGroup, size: usize) -> Result<Self> {
        let mut buffer: sys::MppBuffer = ptr::null_mut();
        check_ret(unsafe {
            sys::mpp_buffer_get_with_tag(group.raw, &mut buffer, size, TAG, CALLER)
        })?;
        Ok(Self { raw: buffer })
    }

    /// Allocate a new buffer from the default (misc) allocator.
    ///
    /// Equivalent to `mpp_buffer_get(NULL, &buffer, size)` in C.
    /// The buffer is tracked by MPP's internal misc group.
    pub fn get_default(size: usize) -> Result<Self> {
        let mut buffer: sys::MppBuffer = ptr::null_mut();
        check_ret(unsafe {
            sys::mpp_buffer_get_with_tag(ptr::null_mut(), &mut buffer, size, TAG, CALLER)
        })?;
        Ok(Self { raw: buffer })
    }

    /// Import an external buffer described by `info`.
    ///
    /// The buffer is attached to MPP's internal misc group for leak tracking.
    ///
    /// Equivalent to the C macro `mpp_buffer_import(&buffer, &info)`.
    ///
    /// # File descriptor ownership
    ///
    /// For [`BufferType::EXT_DMA`], MPP internally duplicates the file descriptor.
    /// The caller retains ownership of the original fd and must close it separately
    /// when appropriate.
    pub fn import(info: &BufferInfo) -> Result<Self> {
        let mut sys_info = info.to_sys();
        let mut buffer: sys::MppBuffer = ptr::null_mut();
        check_ret(unsafe {
            sys::mpp_buffer_import_with_tag(
                ptr::null_mut(), // NULL group → default misc group
                &mut sys_info,
                &mut buffer,
                TAG,
                CALLER,
            )
        })?;
        Ok(Self { raw: buffer })
    }

    // ---- Accessors ----

    /// Get the CPU virtual address of this buffer.
    ///
    /// Returns null for buffers without a CPU mapping.
    pub fn ptr(&self) -> *mut c_void {
        unsafe { sys::mpp_buffer_get_ptr_with_caller(self.raw, CALLER) }
    }

    /// Get the DMA-BUF file descriptor of this buffer.
    ///
    /// Returns `-1` for `NORMAL` (malloc-backed) buffers.
    ///
    /// The returned fd is **borrowed** from the buffer — do not close it.
    /// It remains valid as long as this `MppBuffer` (or a clone) is alive.
    pub fn fd(&self) -> RawFd {
        unsafe { sys::mpp_buffer_get_fd_with_caller(self.raw, CALLER) }
    }

    /// Get the size of this buffer in bytes.
    pub fn size(&self) -> usize {
        unsafe { sys::mpp_buffer_get_size_with_caller(self.raw, CALLER) }
    }

    /// Get the buffer index within its group.
    pub fn index(&self) -> i32 {
        unsafe { sys::mpp_buffer_get_index_with_caller(self.raw, CALLER) }
    }

    /// Set the buffer index.
    pub fn set_index(&self, index: i32) -> Result<()> {
        check_ret(unsafe { sys::mpp_buffer_set_index_with_caller(self.raw, index, CALLER) })
    }

    /// Get the data offset within this buffer.
    pub fn offset(&self) -> usize {
        unsafe { sys::mpp_buffer_get_offset_with_caller(self.raw, CALLER) }
    }

    /// Set the data offset within this buffer.
    pub fn set_offset(&self, offset: usize) -> Result<()> {
        check_ret(unsafe { sys::mpp_buffer_set_offset_with_caller(self.raw, offset, CALLER) })
    }

    /// Retrieve the full [`BufferInfo`] for this buffer.
    pub fn info(&self) -> Result<BufferInfo> {
        let mut sys_info = unsafe { std::mem::zeroed::<sys::MppBufferInfo_t>() };
        check_ret(unsafe {
            sys::mpp_buffer_info_get_with_caller(self.raw, &mut sys_info, CALLER)
        })?;
        Ok(BufferInfo::from_sys(&sys_info))
    }

    // ---- Read / Write ----

    /// Read `data.len()` bytes from the buffer starting at `offset`.
    pub fn read(&self, offset: usize, data: &mut [u8]) -> Result<()> {
        check_ret(unsafe {
            sys::mpp_buffer_read_with_caller(
                self.raw,
                offset,
                data.as_mut_ptr() as *mut c_void,
                data.len(),
                CALLER,
            )
        })
    }

    /// Write `data.len()` bytes to the buffer starting at `offset`.
    pub fn write(&self, offset: usize, data: &[u8]) -> Result<()> {
        check_ret(unsafe {
            sys::mpp_buffer_write_with_caller(
                self.raw,
                offset,
                // C API declares `void *data` (not const); cast is safe since it only reads.
                data.as_ptr() as *mut c_void,
                data.len(),
                CALLER,
            )
        })
    }

    /// Mark this buffer as discarded.
    ///
    /// When the reference count reaches zero, the underlying memory will be freed
    /// immediately rather than being recycled back to the group's pool.
    pub fn discard(&self) -> Result<()> {
        check_ret(unsafe { sys::mpp_buffer_discard_with_caller(self.raw, CALLER) })
    }

    // ---- Cache sync operations ----

    /// Begin a read-write cache sync.
    ///
    /// Call before CPU writes to a cachable DRM/ION buffer.
    /// Pair with [`sync_end`](Self::sync_end) after the write.
    pub fn sync_begin(&self) -> Result<()> {
        check_ret(unsafe { sys::mpp_buffer_sync_begin_f(self.raw, 0, CALLER) })
    }

    /// End a read-write cache sync.
    pub fn sync_end(&self) -> Result<()> {
        check_ret(unsafe { sys::mpp_buffer_sync_end_f(self.raw, 0, CALLER) })
    }

    /// Begin a read-only cache sync.
    pub fn sync_ro_begin(&self) -> Result<()> {
        check_ret(unsafe { sys::mpp_buffer_sync_begin_f(self.raw, 1, CALLER) })
    }

    /// End a read-only cache sync.
    pub fn sync_ro_end(&self) -> Result<()> {
        check_ret(unsafe { sys::mpp_buffer_sync_end_f(self.raw, 1, CALLER) })
    }

    /// Begin a partial read-write cache sync over `[offset, offset+length)`.
    pub fn sync_partial_begin(&self, offset: u32, length: u32) -> Result<()> {
        check_ret(unsafe {
            sys::mpp_buffer_sync_partial_begin_f(self.raw, 0, offset, length, CALLER)
        })
    }

    /// End a partial read-write cache sync.
    pub fn sync_partial_end(&self, offset: u32, length: u32) -> Result<()> {
        check_ret(unsafe {
            sys::mpp_buffer_sync_partial_end_f(self.raw, 0, offset, length, CALLER)
        })
    }

    /// Begin a partial read-only cache sync over `[offset, offset+length)`.
    pub fn sync_ro_partial_begin(&self, offset: u32, length: u32) -> Result<()> {
        check_ret(unsafe {
            sys::mpp_buffer_sync_partial_begin_f(self.raw, 1, offset, length, CALLER)
        })
    }

    /// End a partial read-only cache sync.
    pub fn sync_ro_partial_end(&self, offset: u32, length: u32) -> Result<()> {
        check_ret(unsafe {
            sys::mpp_buffer_sync_partial_end_f(self.raw, 1, offset, length, CALLER)
        })
    }

    // ---- Raw pointer interop ----

    /// Get the underlying raw `MppBuffer` handle.
    ///
    /// The handle is valid as long as this `MppBuffer` (or a clone) is alive.
    /// Needed when passing to frame/packet/meta APIs in the C library.
    pub fn as_raw(&self) -> sys::MppBuffer {
        self.raw
    }

    /// Consume this handle without decrementing the reference count.
    ///
    /// The caller takes responsibility for eventually calling `mpp_buffer_put`
    /// or re-wrapping with [`from_raw`](Self::from_raw).
    pub fn into_raw(self) -> sys::MppBuffer {
        let me = ManuallyDrop::new(self);
        me.raw
    }

    /// Take ownership of a raw `MppBuffer` handle.
    ///
    /// # Safety
    ///
    /// - `raw` must be a valid, non-null `MppBuffer` with at least one owned reference.
    /// - The caller transfers that reference to this wrapper; `mpp_buffer_put` will
    ///   be called on drop.
    pub unsafe fn from_raw(raw: sys::MppBuffer) -> Self {
        debug_assert!(!raw.is_null());
        Self { raw }
    }

    /// Create an owned handle by incrementing the reference count of a raw handle.
    ///
    /// Use this when you receive a buffer reference from a frame or packet and want
    /// to keep it alive independently (e.g., `mpp_frame_get_buffer` returns a borrowed
    /// reference — this method converts it to an owned one).
    ///
    /// # Safety
    ///
    /// - `raw` must be a valid, non-null `MppBuffer`.
    pub unsafe fn clone_ref(raw: sys::MppBuffer) -> Result<Self> {
        debug_assert!(!raw.is_null());
        check_ret(sys::mpp_buffer_inc_ref_with_caller(raw, CALLER))?;
        Ok(Self { raw })
    }
}

impl Clone for Buffer {
    /// Increment the reference count and return a new handle.
    fn clone(&self) -> Self {
        let ret = unsafe { sys::mpp_buffer_inc_ref_with_caller(self.raw, CALLER) };
        assert_eq!(
            ret,
            sys::MPP_RET::MPP_SUCCESS,
            "mpp_buffer_inc_ref failed with code {ret:?}"
        );
        Self { raw: self.raw }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            sys::mpp_buffer_put_with_caller(self.raw, CALLER);
        }
    }
}

impl fmt::Debug for Buffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MppBuffer")
            .field("raw", &self.raw)
            .field("size", &self.size())
            .field("fd", &self.fd())
            .field("index", &self.index())
            .finish()
    }
}

// ============================================================
// MppBufferGroup
// ============================================================

/// An owned handle to an MPP buffer group.
///
/// Buffer groups manage a pool of [`MppBuffer`]s. Two modes are available:
///
/// - **Internal** ([`new_internal`](Self::new_internal)): MPP allocates buffers on demand.
///   Typical for encoder frame/packet buffers and decoder with internal allocation.
///
/// - **External** ([`new_external`](Self::new_external)): The application commits
///   pre-allocated buffers via [`commit`](Self::commit). Used when buffer memory
///   comes from an external source (V4L2, gralloc, etc.).
///
/// Dropping this handle releases the group via `mpp_buffer_group_put`.
/// All buffers in the group should have been released (put) before the group is dropped.
pub struct BufferGroup {
    raw: sys::MppBufferGroup,
}

// MPP buffer groups are internally thread-safe.
unsafe impl Send for BufferGroup {}
unsafe impl Sync for BufferGroup {}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferMode {
    Internal = sys::MppBufferMode_MPP_BUFFER_INTERNAL as u32,
    External = sys::MppBufferMode_MPP_BUFFER_EXTERNAL as u32,
}

impl BufferGroup {
    /// Create a buffer group.
    pub fn new(buf_type: BufferType, mode: BufferMode) -> Result<Self> {
        let mut group: sys::MppBufferGroup = ptr::null_mut();
        check_ret(unsafe {
            sys::mpp_buffer_group_get(
                &mut group,
                buf_type.0 as sys::MppBufferType,
                mode as sys::MppBufferMode,
                TAG,
                CALLER,
            )
        })?;
        Ok(Self { raw: group })
    }

    /// Commit an external buffer to this group.
    ///
    /// The buffer described by `info` is registered in unused state.
    /// Internal consumers can then obtain it via [`MppBuffer::get`].
    ///
    /// Equivalent to the C macro `mpp_buffer_commit(group, &info)`.
    ///
    /// # File descriptor ownership
    ///
    /// The `fd` in `info` is **not** consumed — MPP stores or duplicates it internally.
    /// The caller must keep the underlying allocation alive until the group is released.
    pub fn commit(&self, info: &BufferInfo) -> Result<()> {
        let mut sys_info = info.to_sys();
        check_ret(unsafe {
            sys::mpp_buffer_import_with_tag(
                self.raw,
                &mut sys_info,
                ptr::null_mut(), // NULL buffer → commit mode
                TAG,
                CALLER,
            )
        })
    }

    /// Import an external buffer into this group and immediately get an owned handle.
    ///
    /// Combines commit + get in one operation.
    ///
    /// Equivalent to calling `mpp_buffer_import_with_tag(group, &info, &buffer, ...)` in C.
    pub fn import(&self, info: &BufferInfo) -> Result<Buffer> {
        let mut sys_info = info.to_sys();
        let mut buffer: sys::MppBuffer = ptr::null_mut();
        check_ret(unsafe {
            sys::mpp_buffer_import_with_tag(self.raw, &mut sys_info, &mut buffer, TAG, CALLER)
        })?;
        Ok(Buffer { raw: buffer })
    }

    /// Remove all buffers from this group.
    ///
    /// The group remains valid and can accept new buffers.
    pub fn clear(&self) -> Result<()> {
        check_ret(unsafe { sys::mpp_buffer_group_clear(self.raw) })
    }

    /// Configure buffer size and count limits.
    ///
    /// - `size`: maximum buffer size in bytes (`0` = no limit)
    /// - `count`: maximum number of buffers (`0` = no limit)
    ///
    /// Typically used with internal groups managing decoder frame buffers.
    pub fn set_limit(&self, size: usize, count: i32) -> Result<()> {
        check_ret(unsafe { sys::mpp_buffer_group_limit_config(self.raw, size, count) })
    }

    /// Get the number of unused (available) buffers in this group.
    pub fn unused_count(&self) -> i32 {
        unsafe { sys::mpp_buffer_group_unused(self.raw) }
    }

    /// Get the total memory usage of this group in bytes.
    pub fn usage(&self) -> usize {
        unsafe { sys::mpp_buffer_group_usage(self.raw) }
    }

    /// Get the buffer mode (internal or external).
    pub fn mode(&self) -> sys::MppBufferMode {
        unsafe { sys::mpp_buffer_group_mode(self.raw) }
    }

    /// Get the buffer allocator type of this group.
    pub fn buf_type(&self) -> BufferType {
        BufferType(unsafe { sys::mpp_buffer_group_type(self.raw) })
    }

    // ---- Raw pointer interop ----

    /// Get the underlying raw `MppBufferGroup` handle.
    ///
    /// Needed when passing to MPI control commands
    /// (e.g., `MPP_DEC_SET_EXT_BUF_GROUP`).
    pub fn as_raw(&self) -> sys::MppBufferGroup {
        self.raw
    }

    /// Consume this handle without calling `mpp_buffer_group_put`.
    ///
    /// The caller takes responsibility for releasing the group.
    pub fn into_raw(self) -> sys::MppBufferGroup {
        let me = ManuallyDrop::new(self);
        me.raw
    }

    /// Take ownership of a raw `MppBufferGroup` handle.
    ///
    /// # Safety
    ///
    /// - `raw` must be a valid, non-null `MppBufferGroup`.
    /// - `mpp_buffer_group_put` will be called on drop.
    pub unsafe fn from_raw(raw: sys::MppBufferGroup) -> Self {
        debug_assert!(!raw.is_null());
        Self { raw }
    }
}

impl Drop for BufferGroup {
    fn drop(&mut self) {
        unsafe {
            sys::mpp_buffer_group_put(self.raw);
        }
    }
}

impl fmt::Debug for BufferGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufferGroup")
            .field("raw", &self.raw)
            .field("usage", &self.usage())
            .field("unused", &self.unused_count())
            .finish()
    }
}

// ============================================================
// Global functions
// ============================================================

/// Get the total buffer memory currently allocated by MPP (across all groups).
pub fn buffer_total_now() -> u32 {
    unsafe { sys::mpp_buffer_total_now() }
}

/// Get the peak total buffer memory ever allocated by MPP.
pub fn buffer_total_max() -> u32 {
    unsafe { sys::mpp_buffer_total_max() }
}
