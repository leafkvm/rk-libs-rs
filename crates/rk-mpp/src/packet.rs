use std::ffi::c_void;
use std::fmt;
use std::mem::ManuallyDrop;
use std::ptr;

use crate::buffer::Buffer;
use crate::{check_ret, sys, Result};

// ============================================================
// Packet
// ============================================================

/// An owned handle to an MPP packet.
///
/// Packets carry compressed bitstream data for decoding or encoder output.
/// Dropping this handle calls `mpp_packet_deinit`.
///
/// # Data model
///
/// A packet has four core pointers/values:
/// - **data** / **size**: the base address and total allocation size
/// - **pos** / **length**: current read position and remaining bytes from pos
///
/// When feeding data to the decoder, set `data`/`size` to describe the entire
/// buffer and `pos`/`length` to the region containing valid bitstream data.
///
/// # Buffer-backed packets
///
/// For zero-copy flows, a packet can be backed by an [`MppBuffer`](crate::buffer::Buffer)
/// via [`new_with_buffer`](Self::new_with_buffer). The buffer's DMA-BUF fd allows
/// hardware access. The C library holds a reference to the buffer internally.
pub struct Packet {
    raw: sys::MppPacket,
}

// MppPacket internal operations are thread-safe.
unsafe impl Send for Packet {}
unsafe impl Sync for Packet {}

impl Packet {
    /// Allocate a new empty packet (no data pointer set).
    ///
    /// Equivalent to `mpp_packet_new`.
    pub fn new_empty() -> Result<Self> {
        let mut pkt: sys::MppPacket = ptr::null_mut();
        check_ret(unsafe { sys::mpp_packet_new(&mut pkt) })?;
        Ok(Self { raw: pkt })
    }

    /// Allocate a packet and set its data pointer and size.
    ///
    /// # Safety
    ///
    /// The caller must ensure `data` points to a valid allocation of at least `size` bytes
    /// that remains valid for the lifetime of this packet (or until `set_data` is called
    /// with a different pointer).
    pub unsafe fn new_with_data(data: *mut c_void, size: usize) -> Result<Self> {
        let mut pkt: sys::MppPacket = ptr::null_mut();
        check_ret(sys::mpp_packet_init(&mut pkt, data, size))?;
        Ok(Self { raw: pkt })
    }

    /// Allocate a packet backed by an MPP buffer.
    ///
    /// The packet's data pointer and size are derived from the buffer.
    /// The C library takes a reference on the buffer.
    pub fn new_with_buffer(buffer: &Buffer) -> Result<Self> {
        let mut pkt: sys::MppPacket = ptr::null_mut();
        check_ret(unsafe { sys::mpp_packet_init_with_buffer(&mut pkt, buffer.as_raw()) })?;
        Ok(Self { raw: pkt })
    }

    /// Create a deep copy of another packet.
    ///
    /// Equivalent to `mpp_packet_copy_init` — the new packet gets its own
    /// data allocation with a copy of `src`'s data.
    pub fn copy_from(src: &Packet) -> Result<Self> {
        let mut pkt: sys::MppPacket = ptr::null_mut();
        check_ret(unsafe { sys::mpp_packet_copy_init(&mut pkt, src.raw) })?;
        Ok(Self { raw: pkt })
    }

    // ---- Data pointers ----

    /// Set the data pointer and total size of the packet buffer.
    ///
    /// # Safety
    ///
    /// The caller must ensure the pointed-to memory remains valid for the
    /// packet's lifetime (or until overwritten by another `set_data` call).
    pub unsafe fn set_data(&self, data: *mut c_void, size: usize) {
        sys::mpp_packet_set_data(self.raw, data);
        sys::mpp_packet_set_size(self.raw, size);
    }

    /// Get the base data pointer.
    pub fn data(&self) -> *mut c_void {
        unsafe { sys::mpp_packet_get_data(self.raw) }
    }

    /// Get the total buffer size.
    pub fn size(&self) -> usize {
        unsafe { sys::mpp_packet_get_size(self.raw) }
    }

    /// Set the current read/write position.
    pub fn set_pos(&self, pos: *mut c_void) {
        unsafe { sys::mpp_packet_set_pos(self.raw, pos) }
    }

    /// Get the current read/write position.
    pub fn pos(&self) -> *mut c_void {
        unsafe { sys::mpp_packet_get_pos(self.raw) }
    }

    /// Set the remaining data length from the current position.
    ///
    /// **Important**: for encoder output packets, clear this to 0 before encoding.
    pub fn set_length(&self, len: usize) {
        unsafe { sys::mpp_packet_set_length(self.raw, len) }
    }

    /// Get the remaining data length from the current position.
    pub fn length(&self) -> usize {
        unsafe { sys::mpp_packet_get_length(self.raw) }
    }

    // ---- Timestamps ----

    pub fn pts(&self) -> i64 {
        unsafe { sys::mpp_packet_get_pts(self.raw) }
    }

    pub fn set_pts(&self, v: i64) {
        unsafe { sys::mpp_packet_set_pts(self.raw, v) }
    }

    pub fn dts(&self) -> i64 {
        unsafe { sys::mpp_packet_get_dts(self.raw) }
    }

    pub fn set_dts(&self, v: i64) {
        unsafe { sys::mpp_packet_set_dts(self.raw, v) }
    }

    // ---- Flags ----

    pub fn flag(&self) -> u32 {
        unsafe { sys::mpp_packet_get_flag(self.raw) }
    }

    pub fn set_flag(&self, v: u32) {
        unsafe { sys::mpp_packet_set_flag(self.raw, v) }
    }

    /// Set the End-Of-Stream flag.
    pub fn set_eos(&self) -> Result<()> {
        check_ret(unsafe { sys::mpp_packet_set_eos(self.raw) })
    }

    /// Clear the End-Of-Stream flag.
    pub fn clr_eos(&self) -> Result<()> {
        check_ret(unsafe { sys::mpp_packet_clr_eos(self.raw) })
    }

    /// Check for End-Of-Stream.
    pub fn eos(&self) -> bool {
        unsafe { sys::mpp_packet_get_eos(self.raw) != 0 }
    }

    /// Mark this packet as containing extra data (e.g., SPS/PPS).
    pub fn set_extra_data(&self) -> Result<()> {
        check_ret(unsafe { sys::mpp_packet_set_extra_data(self.raw) })
    }

    // ---- Buffer ----

    /// Get the raw MppBuffer handle attached to this packet.
    ///
    /// Returns `None` if no buffer is attached. The handle is **borrowed**;
    /// its lifetime is tied to the packet.
    pub fn buffer_raw(&self) -> Option<sys::MppBuffer> {
        let buf = unsafe { sys::mpp_packet_get_buffer(self.raw) };
        if buf.is_null() {
            None
        } else {
            Some(buf)
        }
    }

    /// Get an owned [`Buffer`] handle by incrementing the buffer's reference count.
    ///
    /// Returns `None` if no buffer is attached.
    pub fn buffer_cloned(&self) -> Option<Result<Buffer>> {
        self.buffer_raw()
            .map(|raw| unsafe { Buffer::clone_ref(raw) })
    }

    /// Attach a buffer to this packet.
    ///
    /// The C library takes a reference on the buffer.
    pub fn set_buffer(&self, buffer: &Buffer) {
        unsafe { sys::mpp_packet_set_buffer(self.raw, buffer.as_raw()) }
    }

    // ---- Read / Write ----

    /// Read bytes from the packet's data at `offset`.
    pub fn read(&self, offset: usize, data: &mut [u8]) -> Result<()> {
        check_ret(unsafe {
            sys::mpp_packet_read(
                self.raw,
                offset,
                data.as_mut_ptr() as *mut c_void,
                data.len(),
            )
        })
    }

    /// Write bytes to the packet's data at `offset`.
    pub fn write(&self, offset: usize, data: &[u8]) -> Result<()> {
        check_ret(unsafe {
            sys::mpp_packet_write(self.raw, offset, data.as_ptr() as *mut c_void, data.len())
        })
    }

    // ---- Meta ----

    /// Check if the packet carries metadata.
    pub fn has_meta(&self) -> bool {
        unsafe { sys::mpp_packet_has_meta(self.raw) != 0 }
    }

    /// Get the raw `MppMeta` handle from this packet.
    ///
    /// The metadata lifetime is tied to the packet.
    /// Returns `None` if no meta is present.
    pub fn meta(&self) -> Option<sys::MppMeta> {
        let m = unsafe { sys::mpp_packet_get_meta(self.raw) };
        if m.is_null() {
            None
        } else {
            Some(m)
        }
    }

    /// Returns `true` if the encoder tagged this output packet as an intra
    /// (IDR/keyframe) via the `KEY_OUTPUT_INTRA` meta. The `MPP_PACKET_FLAG_INTRA`
    /// bit on `flag()` is not set by the encoder output path, so reading the
    /// meta is the only reliable way.
    pub fn is_intra(&self) -> bool {
        let meta = unsafe { sys::mpp_packet_get_meta(self.raw) };
        if meta.is_null() {
            return false;
        }
        let mut val: sys::RK_S32 = 0;
        let ret =
            unsafe { sys::mpp_meta_get_s32(meta, sys::MppMetaKey::KEY_OUTPUT_INTRA, &mut val) };
        ret == sys::MPP_RET::MPP_OK && val != 0
    }

    // ---- Passthrough data ----

    /// Retrieve passthrough data that was attached to the input frame.
    ///
    /// Returns `Some(Vec<u8>)` with the byte payload, or `None` if no
    /// passthrough data is present.
    ///
    /// This is a **consuming read** — calling it a second time on the same
    /// packet will return `None`.
    pub fn get_passthrough_data(&self) -> Option<Vec<u8>> {
        let meta = unsafe { sys::mpp_packet_get_meta(self.raw) };
        if meta.is_null() {
            return None;
        }
        let mut pt_ptr: *mut c_void = ptr::null_mut();
        let ret = unsafe {
            sys::mpp_meta_get_ptr(meta, sys::MppMetaKey::KEY_PASSTHROUGH_DATA, &mut pt_ptr)
        };
        if ret != sys::MPP_RET::MPP_OK || pt_ptr.is_null() {
            return None;
        }
        // pt_ptr points to the meta-internal MppPassthroughData struct.
        let pt = unsafe { &*(pt_ptr as *const sys::MppPassthroughData) };
        if pt.pdata.is_null() || pt.len == 0 {
            return None;
        }
        let data = unsafe { std::slice::from_raw_parts(pt.pdata as *const u8, pt.len as usize) };
        Some(data.to_vec())
    }

    // ---- Partition / segment info ----

    /// Check if the packet is a partition (part of a larger image).
    pub fn is_partition(&self) -> bool {
        unsafe { sys::mpp_packet_is_partition(self.raw) != 0 }
    }

    /// Check for Start Of Image in a partitioned stream.
    pub fn is_soi(&self) -> bool {
        unsafe { sys::mpp_packet_is_soi(self.raw) != 0 }
    }

    /// Check for End Of Image in a partitioned stream.
    pub fn is_eoi(&self) -> bool {
        unsafe { sys::mpp_packet_is_eoi(self.raw) != 0 }
    }

    /// Get the number of segments in this packet.
    pub fn segment_count(&self) -> u32 {
        unsafe { sys::mpp_packet_get_segment_nb(self.raw) }
    }

    /// Get a pointer to the first segment info entry.
    ///
    /// Segments form a linked list via `MppPktSeg::next`.
    /// Returns `None` if there are no segments.
    pub fn segment_info(&self) -> Option<&sys::MppPktSeg> {
        let ptr = unsafe { sys::mpp_packet_get_segment_info(self.raw) };
        if ptr.is_null() {
            None
        } else {
            // SAFETY: the pointer is valid for the lifetime of the packet,
            // and MppPktSeg is a plain data struct.
            Some(unsafe { &*ptr })
        }
    }

    // ---- Convenient data access ----

    /// Get the remaining data from `pos` to `pos + length` as a byte slice.
    ///
    /// Returns `None` if `pos` is null or `length` is 0.
    ///
    /// # Safety
    ///
    /// The underlying data must be valid and not concurrently mutated.
    pub unsafe fn as_slice(&self) -> Option<&[u8]> {
        let p = self.pos();
        let len = self.length();
        if p.is_null() || len == 0 {
            None
        } else {
            Some(std::slice::from_raw_parts(p as *const u8, len))
        }
    }

    // ---- Raw pointer interop ----

    /// Get the underlying raw `MppPacket` handle.
    pub fn as_raw(&self) -> sys::MppPacket {
        self.raw
    }

    /// Consume this handle without calling `mpp_packet_deinit`.
    pub fn into_raw(self) -> sys::MppPacket {
        let me = ManuallyDrop::new(self);
        me.raw
    }

    /// Take ownership of a raw `MppPacket` handle.
    ///
    /// # Safety
    ///
    /// - `raw` must be a valid, non-null `MppPacket`.
    /// - `mpp_packet_deinit` will be called on drop.
    pub unsafe fn from_raw(raw: sys::MppPacket) -> Self {
        debug_assert!(!raw.is_null());
        Self { raw }
    }
}

impl Drop for Packet {
    fn drop(&mut self) {
        unsafe {
            sys::mpp_packet_deinit(&mut self.raw);
        }
    }
}

impl fmt::Debug for Packet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Packet")
            .field("raw", &self.raw)
            .field("size", &self.size())
            .field("length", &self.length())
            .field("pts", &self.pts())
            .field("eos", &self.eos())
            .finish()
    }
}
