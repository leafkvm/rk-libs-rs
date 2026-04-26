use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::ptr;

use crate::buffer::Buffer;
use crate::{check_ret, sys, Result};

/// Frame mode flag constants (interlace, field order, etc.)
pub mod flags {
    pub const FRAME: u32 = 0x0000_0000;
    pub const TOP_FIELD: u32 = 0x0000_0001;
    pub const BOT_FIELD: u32 = 0x0000_0002;
    pub const PAIRED_FIELD: u32 = TOP_FIELD | BOT_FIELD;
    pub const TOP_FIRST: u32 = 0x0000_0004;
    pub const BOT_FIRST: u32 = 0x0000_0008;
    pub const DEINTERLACED: u32 = TOP_FIRST | BOT_FIRST;
    pub const FIELD_ORDER_MASK: u32 = 0x0000_000C;
    pub const VIEW_ID_MASK: u32 = 0x0000_00F0;
    pub const IEP_DEI_MASK: u32 = 0x0000_0F00;
    pub const IEP_DEI_I2O1: u32 = 0x0000_0100;
    pub const IEP_DEI_I4O2: u32 = 0x0000_0200;
    pub const IEP_DEI_I4O1: u32 = 0x0000_0300;
}

/// Frame format mask/flag constants for composing [`MppFrameFormat`](sys::MppFrameFormat) values.
pub mod frame_fmt {
    pub const FMT_MASK: u32 = 0x000F_FFFF;
    pub const FMT_COLOR_MASK: u32 = 0x000F_0000;
    pub const FMT_YUV: u32 = 0x0000_0000;
    pub const FMT_RGB: u32 = 0x0001_0000;
    pub const FBC_MASK: u32 = 0x00F0_0000;
    pub const FBC_AFBC_V1: u32 = 0x0010_0000;
    pub const FBC_AFBC_V2: u32 = 0x0020_0000;
    pub const FBC_RKFBC: u32 = 0x0040_0000;
    pub const HDR_MASK: u32 = 0x0C00_0000;
    pub const HDR: u32 = 0x0400_0000;
    pub const TILE_FLAG: u32 = 0x0200_0000;
    pub const LE_MASK: u32 = 0x0100_0000;

    pub fn is_yuv(fmt: u32) -> bool {
        (fmt & FMT_COLOR_MASK) == FMT_YUV
            && (fmt & FMT_MASK) < super::sys::MppFrameFormat::MPP_FMT_YUV_BUTT as u32
    }

    pub fn is_rgb(fmt: u32) -> bool {
        (fmt & FMT_COLOR_MASK) == FMT_RGB
            && (fmt & FMT_MASK) < super::sys::MppFrameFormat::MPP_FMT_RGB_BUTT as u32
    }

    pub fn is_fbc(fmt: u32) -> bool {
        (fmt & FBC_MASK) != 0
    }

    pub fn is_tile(fmt: u32) -> bool {
        (fmt & TILE_FLAG) != 0
    }

    pub fn is_hdr(fmt: u32) -> bool {
        (fmt & HDR_MASK) != 0
    }

    pub fn is_le(fmt: u32) -> bool {
        (fmt & LE_MASK) == LE_MASK
    }
}

/// An owned handle to an MPP frame.
pub struct Frame {
    raw: sys::MppFrame,
}

// MppFrame internal operations are thread-safe; the opaque pointer is self-contained.
unsafe impl Send for Frame {}
unsafe impl Sync for Frame {}

impl Frame {
    /// Allocate a new, empty frame.
    pub fn new() -> Result<Self> {
        let mut frame: sys::MppFrame = ptr::null_mut();
        check_ret(unsafe { sys::mpp_frame_init(&mut frame) })?;
        Ok(Self { raw: frame })
    }

    // ---- Geometry ----

    pub fn width(&self) -> u32 {
        unsafe { sys::mpp_frame_get_width(self.raw) }
    }

    pub fn set_width(&self, v: u32) {
        unsafe { sys::mpp_frame_set_width(self.raw, v) }
    }

    pub fn height(&self) -> u32 {
        unsafe { sys::mpp_frame_get_height(self.raw) }
    }

    pub fn set_height(&self, v: u32) {
        unsafe { sys::mpp_frame_set_height(self.raw, v) }
    }

    pub fn hor_stride(&self) -> u32 {
        unsafe { sys::mpp_frame_get_hor_stride(self.raw) }
    }

    pub fn set_hor_stride(&self, v: u32) {
        unsafe { sys::mpp_frame_set_hor_stride(self.raw, v) }
    }

    pub fn ver_stride(&self) -> u32 {
        unsafe { sys::mpp_frame_get_ver_stride(self.raw) }
    }

    pub fn set_ver_stride(&self, v: u32) {
        unsafe { sys::mpp_frame_set_ver_stride(self.raw, v) }
    }

    pub fn hor_stride_pixel(&self) -> u32 {
        unsafe { sys::mpp_frame_get_hor_stride_pixel(self.raw) }
    }

    pub fn set_hor_stride_pixel(&self, v: u32) {
        unsafe { sys::mpp_frame_set_hor_stride_pixel(self.raw, v) }
    }

    pub fn fbc_hdr_stride(&self) -> u32 {
        unsafe { sys::mpp_frame_get_fbc_hdr_stride(self.raw) }
    }

    pub fn set_fbc_hdr_stride(&self, v: u32) {
        unsafe { sys::mpp_frame_set_fbc_hdr_stride(self.raw, v) }
    }

    pub fn offset_x(&self) -> u32 {
        unsafe { sys::mpp_frame_get_offset_x(self.raw) }
    }

    pub fn set_offset_x(&self, v: u32) {
        unsafe { sys::mpp_frame_set_offset_x(self.raw, v) }
    }

    pub fn offset_y(&self) -> u32 {
        unsafe { sys::mpp_frame_get_offset_y(self.raw) }
    }

    pub fn set_offset_y(&self, v: u32) {
        unsafe { sys::mpp_frame_set_offset_y(self.raw, v) }
    }

    // ---- Mode / status flags ----

    pub fn mode(&self) -> u32 {
        unsafe { sys::mpp_frame_get_mode(self.raw) }
    }

    pub fn set_mode(&self, v: u32) {
        unsafe { sys::mpp_frame_set_mode(self.raw, v) }
    }

    pub fn discard(&self) -> u32 {
        unsafe { sys::mpp_frame_get_discard(self.raw) }
    }

    pub fn set_discard(&self, v: u32) {
        unsafe { sys::mpp_frame_set_discard(self.raw, v) }
    }

    pub fn viewid(&self) -> u32 {
        unsafe { sys::mpp_frame_get_viewid(self.raw) }
    }

    pub fn set_viewid(&self, v: u32) {
        unsafe { sys::mpp_frame_set_viewid(self.raw, v) }
    }

    pub fn poc(&self) -> u32 {
        unsafe { sys::mpp_frame_get_poc(self.raw) }
    }

    pub fn set_poc(&self, v: u32) {
        unsafe { sys::mpp_frame_set_poc(self.raw, v) }
    }

    // ---- Timestamps ----

    pub fn pts(&self) -> i64 {
        unsafe { sys::mpp_frame_get_pts(self.raw) }
    }

    pub fn set_pts(&self, v: i64) {
        unsafe { sys::mpp_frame_set_pts(self.raw, v) }
    }

    pub fn dts(&self) -> i64 {
        unsafe { sys::mpp_frame_get_dts(self.raw) }
    }

    pub fn set_dts(&self, v: i64) {
        unsafe { sys::mpp_frame_set_dts(self.raw, v) }
    }

    // ---- Error / buf_size / thumbnail ----

    pub fn errinfo(&self) -> u32 {
        unsafe { sys::mpp_frame_get_errinfo(self.raw) }
    }

    pub fn set_errinfo(&self, v: u32) {
        unsafe { sys::mpp_frame_set_errinfo(self.raw, v) }
    }

    pub fn buf_size(&self) -> usize {
        unsafe { sys::mpp_frame_get_buf_size(self.raw) }
    }

    pub fn set_buf_size(&self, v: usize) {
        unsafe { sys::mpp_frame_set_buf_size(self.raw, v) }
    }

    pub fn thumbnail_en(&self) -> u32 {
        unsafe { sys::mpp_frame_get_thumbnail_en(self.raw) }
    }

    pub fn set_thumbnail_en(&self, v: u32) {
        unsafe { sys::mpp_frame_set_thumbnail_en(self.raw, v) }
    }

    // ---- Flow control ----

    /// Returns non-zero if the End-Of-Stream flag is set.
    pub fn eos(&self) -> bool {
        unsafe { sys::mpp_frame_get_eos(self.raw) != 0 }
    }

    pub fn set_eos(&self, eos: bool) {
        unsafe { sys::mpp_frame_set_eos(self.raw, eos as u32) }
    }

    /// Returns `true` if this frame signals a resolution / format change
    /// from the decoder.
    pub fn info_change(&self) -> bool {
        unsafe { sys::mpp_frame_get_info_change(self.raw) != 0 }
    }

    pub fn set_info_change(&self, v: bool) {
        unsafe { sys::mpp_frame_set_info_change(self.raw, v as u32) }
    }

    // ---- Buffer ----

    /// Get the raw MppBuffer handle attached to this frame.
    ///
    /// Returns `None` if no buffer is set.
    ///
    /// The returned raw handle is **borrowed** from the frame — its lifetime
    /// is tied to the frame. If you need an independent owned reference, use
    /// [`buffer_cloned()`](Self::buffer_cloned).
    pub fn buffer_raw(&self) -> Option<sys::MppBuffer> {
        let buf = unsafe { sys::mpp_frame_get_buffer(self.raw) };
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

    /// Attach a buffer to this frame.
    ///
    /// The C library increments the buffer's ref count internally.
    /// Pass `None` to clear the buffer attachment.
    pub fn set_buffer(&self, buf: Option<&Buffer>) {
        let raw = buf.map_or(ptr::null_mut(), |b| b.as_raw());
        unsafe { sys::mpp_frame_set_buffer(self.raw, raw) }
    }

    // ---- Meta ----

    /// Check if the frame carries metadata.
    pub fn has_meta(&self) -> bool {
        unsafe { sys::mpp_frame_has_meta(self.raw) != 0 }
    }

    /// Get the raw `MppMeta` handle from this frame.
    ///
    /// The metadata lifetime is tied to the frame. Returns a raw handle
    /// that can be used with `mpp_meta_*` functions via the `sys` module.
    ///
    /// Returns `None` if no meta is present. Note that the decoder always
    /// attaches metadata to output frames, but user-created frames may not
    /// have it until explicitly set.
    pub fn meta(&self) -> Option<sys::MppMeta> {
        let m = unsafe { sys::mpp_frame_get_meta(self.raw) };
        if m.is_null() {
            None
        } else {
            Some(m)
        }
    }

    /// Set meta on this frame.
    ///
    /// # Safety
    ///
    /// `meta` must be a valid `MppMeta` handle whose lifetime outlasts
    /// the frame, or is managed by the frame itself (typical for encode flows).
    pub unsafe fn set_meta(&self, meta: sys::MppMeta) {
        sys::mpp_frame_set_meta(self.raw, meta)
    }

    // ---- Color ----

    pub fn color_range(&self) -> sys::MppFrameColorRange {
        unsafe { sys::mpp_frame_get_color_range(self.raw) }
    }

    pub fn set_color_range(&self, v: sys::MppFrameColorRange) {
        unsafe { sys::mpp_frame_set_color_range(self.raw, v) }
    }

    pub fn color_primaries(&self) -> sys::MppFrameColorPrimaries {
        unsafe { sys::mpp_frame_get_color_primaries(self.raw) }
    }

    pub fn set_color_primaries(&self, v: sys::MppFrameColorPrimaries) {
        unsafe { sys::mpp_frame_set_color_primaries(self.raw, v) }
    }

    pub fn color_trc(&self) -> sys::MppFrameColorTransferCharacteristic {
        unsafe { sys::mpp_frame_get_color_trc(self.raw) }
    }

    pub fn set_color_trc(&self, v: sys::MppFrameColorTransferCharacteristic) {
        unsafe { sys::mpp_frame_set_color_trc(self.raw, v) }
    }

    pub fn colorspace(&self) -> sys::MppFrameColorSpace {
        unsafe { sys::mpp_frame_get_colorspace(self.raw) }
    }

    pub fn set_colorspace(&self, v: sys::MppFrameColorSpace) {
        unsafe { sys::mpp_frame_set_colorspace(self.raw, v) }
    }

    pub fn chroma_location(&self) -> sys::MppFrameChromaLocation {
        unsafe { sys::mpp_frame_get_chroma_location(self.raw) }
    }

    pub fn set_chroma_location(&self, v: sys::MppFrameChromaLocation) {
        unsafe { sys::mpp_frame_set_chroma_location(self.raw, v) }
    }

    pub fn format(&self) -> sys::MppFrameFormat {
        unsafe { sys::mpp_frame_get_fmt(self.raw) }
    }

    pub fn set_fmt(&self, v: sys::MppFrameFormat) {
        unsafe { sys::mpp_frame_set_fmt(self.raw, v) }
    }

    pub fn sar(&self) -> sys::MppFrameRational {
        unsafe { sys::mpp_frame_get_sar(self.raw) }
    }

    pub fn set_sar(&self, v: sys::MppFrameRational) {
        unsafe { sys::mpp_frame_set_sar(self.raw, v) }
    }

    pub fn mastering_display(&self) -> sys::MppFrameMasteringDisplayMetadata {
        unsafe { sys::mpp_frame_get_mastering_display(self.raw) }
    }

    pub fn set_mastering_display(&self, v: sys::MppFrameMasteringDisplayMetadata) {
        unsafe { sys::mpp_frame_set_mastering_display(self.raw, v) }
    }

    pub fn content_light(&self) -> sys::MppFrameContentLightMetadata {
        unsafe { sys::mpp_frame_get_content_light(self.raw) }
    }

    pub fn set_content_light(&self, v: sys::MppFrameContentLightMetadata) {
        unsafe { sys::mpp_frame_set_content_light(self.raw, v) }
    }

    // ---- Passthrough data ----

    /// Attach opaque passthrough data to this frame.
    ///
    /// The data is deep-copied into the frame's meta storage. The MPP encoder
    /// will transfer it to the output packet meta without embedding it in the
    /// bitstream.
    ///
    /// Passing an empty slice clears any previously set passthrough data.
    pub fn set_passthrough_data(&self, data: &[u8]) -> Result<()> {
        let meta = unsafe { sys::mpp_frame_get_meta(self.raw) };
        if meta.is_null() {
            return Err(crate::Error(sys::MPP_RET::MPP_ERR_NULL_PTR));
        }
        if data.is_empty() {
            // Clear passthrough data by setting NULL
            check_ret(unsafe {
                sys::mpp_meta_set_ptr(meta, sys::MppMetaKey::KEY_PASSTHROUGH_DATA, ptr::null_mut())
            })
        } else {
            // Build a MppPassthroughData on the stack; set_ptr deep-copies the payload.
            let pt = sys::MppPassthroughData {
                len: data.len() as u32,
                pdata: data.as_ptr() as *mut c_void,
            };
            check_ret(unsafe {
                sys::mpp_meta_set_ptr(
                    meta,
                    sys::MppMetaKey::KEY_PASSTHROUGH_DATA,
                    &pt as *const _ as *mut c_void,
                )
            })
        }
    }

    // ---- Raw pointer interop ----

    /// Get the underlying raw `MppFrame` handle.
    pub fn as_raw(&self) -> sys::MppFrame {
        self.raw
    }

    /// Consume this handle without calling `mpp_frame_deinit`.
    ///
    /// The caller takes responsibility for eventually calling `mpp_frame_deinit`.
    pub fn into_raw(self) -> sys::MppFrame {
        let me = ManuallyDrop::new(self);
        me.raw
    }

    /// Take ownership of a raw `MppFrame` handle.
    ///
    /// # Safety
    ///
    /// - `raw` must be a valid, non-null `MppFrame` allocated by `mpp_frame_init`
    ///   or obtained from decoder output.
    /// - `mpp_frame_deinit` will be called on drop.
    pub unsafe fn from_raw(raw: sys::MppFrame) -> Self {
        debug_assert!(!raw.is_null());
        Self { raw }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        unsafe {
            sys::mpp_frame_deinit(&mut self.raw);
        }
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("raw", &self.raw)
            .field("width", &self.width())
            .field("height", &self.height())
            .field("hor_stride", &self.hor_stride())
            .field("ver_stride", &self.ver_stride())
            .field("fmt", &self.format())
            .field("eos", &self.eos())
            .field("info_change", &self.info_change())
            .field("errinfo", &self.errinfo())
            .field("pts", &self.pts())
            .finish()
    }
}
