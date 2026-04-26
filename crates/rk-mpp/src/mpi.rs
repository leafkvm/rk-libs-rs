use std::ffi::c_void;
use std::fmt;
use std::ptr;

use crate::frame::Frame;
use crate::packet::Packet;
use crate::{check_ret, sys, Error, Result};

use rkblob_sys::mpp_sys::MPP_RET;
pub use sys::MppCodingType as CodingType;
pub use sys::MppCtxType as CtxType;
pub use sys::MppPortType as PortType;

// ============================================================
// Timeout helpers
// ============================================================

/// Timeout value for MPI poll / input / output operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Timeout {
    /// Non-blocking: return immediately.
    NonBlock,
    /// Block indefinitely until data is available.
    #[default]
    Block,
    /// Block for at most `ms` milliseconds.
    Millis(u32),
}

impl Timeout {
    fn to_poll(self) -> sys::MppPollType {
        match self {
            Timeout::NonBlock => sys::MppPollType_MPP_POLL_NON_BLOCK,
            Timeout::Block => sys::MppPollType_MPP_POLL_BLOCK,
            Timeout::Millis(ms) => ms as sys::MppPollType,
        }
    }
}

// ============================================================
// MppContext — The main MPP encoder/decoder context
// ============================================================

/// The main MPP encoder/decoder context.
pub struct Context {
    ctx: sys::MppCtx,
    mpi: *mut sys::MppApi,
}

// MppContext is internally thread-safe; the C library serializes operations.
unsafe impl Send for Context {}
// Multiple threads may read through the MPI vtable concurrently, but
// callers should coordinate mutations (e.g., encode/decode calls) externally.
// Sync is provided because the underlying C implementation allows it.
unsafe impl Sync for Context {}

impl Context {
    // ================================================================
    // Convenience timeout setters (these can only be initialized before mpp_init)
    // ================================================================

    /// Set the io timeout
    fn set_io_timeout(&self, cmd: sys::MpiCmd, timeout: Timeout) -> Result<()> {
        let mut val = timeout.to_poll();
        unsafe { self.control(cmd, &mut val as *mut _ as sys::MppParam) }
    }

    /// Create and initialize an MPP context.
    pub fn new(
        ctx_type: CtxType,
        coding: CodingType,
        input_timeout: Timeout,
        output_timeout: Timeout,
    ) -> Result<Self> {
        let mut ctx: sys::MppCtx = ptr::null_mut();
        let mut mpi: *mut sys::MppApi = ptr::null_mut();

        if ctx_type == CtxType::MPP_CTX_DEC {
            /*
            For the decoder, these fields has to be initialized before mpp_init:
            Command	Field	Impact
            MPP_SET_DISABLE_THREAD	mDisableThread	Disables internal thread
            MPP_DEC_SET_PARSER_SPLIT_MODE etc.	mDecCfg	Various decoder behavior flags
            */
            todo!()
        }

        check_ret(unsafe { sys::mpp_create(&mut ctx, &mut mpi) })?;

        let new = Self { ctx, mpi };
        new.set_io_timeout(sys::MpiCmd::MPP_SET_INPUT_TIMEOUT, input_timeout)?;
        new.set_io_timeout(sys::MpiCmd::MPP_SET_OUTPUT_TIMEOUT, output_timeout)?;

        let ret = unsafe { sys::mpp_init(new.ctx, ctx_type, coding) };
        if ret != MPP_RET::MPP_SUCCESS {
            return Err(Error(ret));
        }

        Ok(new)
    }

    /// Get the raw `MppApi` function-pointer table.
    ///
    /// # Safety
    ///
    /// The returned pointer is valid for the lifetime of this context.
    pub fn api(&self) -> &sys::MppApi {
        // SAFETY: mpp_create guarantees mpi is valid while ctx is alive.
        unsafe { &*self.mpi }
    }

    /// Get the raw `MppCtx` handle.
    pub fn raw_ctx(&self) -> sys::MppCtx {
        self.ctx
    }

    // ================================================================
    // Simple decode API
    // ================================================================

    /// Synchronous decode: send a packet and receive a frame in one call.
    ///
    /// Returns `Ok(None)` if no frame is available yet.
    pub fn decode(&self, packet: &Packet) -> Result<Option<Frame>> {
        let mut frame: sys::MppFrame = ptr::null_mut();
        let f = self.api().decode.expect("MppApi::decode is NULL");
        check_ret(unsafe { f(self.ctx, packet.as_raw(), &mut frame) })?;
        if frame.is_null() {
            Ok(None)
        } else {
            Ok(Some(unsafe { Frame::from_raw(frame) }))
        }
    }

    /// Send a packet to the decoder (async).
    /*
    TODO: Suggested by AI, investigate whether this is true!!!!
    #### `decode_put_packet` ownership semantics
    The C implementation of `mpp_put_packet` has two code paths:
    - **No buffer attached:** Deep-copies the packet data and sets the original packet's length to 0. The caller retains their packet.
    - **Buffer attached:** Stores the raw packet pointer in the task. MPP takes ownership and will `mpp_packet_deinit` later.

    The Rust `decode_put_packet` always takes `&Packet`, which is correct for the no-buffer path but wrong for the buffer-attached path (similar to the `encode_put_frame` async issue). Consider providing a consuming variant for buffer-backed packets.
    */
    pub fn decode_put_packet(&self, packet: &Packet) -> Result<()> {
        let f = self
            .api()
            .decode_put_packet
            .expect("MppApi::decode_put_packet is NULL");
        check_ret(unsafe { f(self.ctx, packet.as_raw()) })
    }

    /// Get a decoded frame from the decoder (async).
    ///
    /// Returns `Ok(None)` if no frame is ready.
    /// Returns `Err(Timeout)` if the output timeout expired.
    pub fn decode_get_frame(&self) -> Result<Option<Frame>> {
        let mut frame: sys::MppFrame = ptr::null_mut();
        let f = self
            .api()
            .decode_get_frame
            .expect("MppApi::decode_get_frame is NULL");
        let ret = unsafe { f(self.ctx, &mut frame) };
        if ret != MPP_RET::MPP_SUCCESS {
            return Err(Error(ret));
        }
        if frame.is_null() {
            Ok(None)
        } else {
            Ok(Some(unsafe { Frame::from_raw(frame) }))
        }
    }

    // ================================================================
    // Simple encode API
    // ================================================================

    /// Synchronous encode: send a frame and receive a packet in one call.
    ///
    /// Returns `Ok(None)` if no packet is produced.
    pub fn encode(&self, frame: &Frame) -> Result<Option<Packet>> {
        let mut pkt: sys::MppPacket = ptr::null_mut();
        let f = self.api().encode.expect("MppApi::encode is NULL");
        check_ret(unsafe { f(self.ctx, frame.as_raw(), &mut pkt) })?;
        if pkt.is_null() {
            Ok(None)
        } else {
            Ok(Some(unsafe { Packet::from_raw(pkt) }))
        }
    }

    /// Send a frame to the encoder (async).
    /*
    TODO: Suggested by AI, investigate whether this is true!!!!

    #### `encode_put_frame` ownership in async mode
    When the MPP context is configured with `Timeout::NonBlock` for input (as in video-cap-test), `encode_put_frame` uses the async code path internally. In this path, the C library stores the raw `MppFrame` pointer in an internal list (`mFrmIn`) and returns immediately. The Rust binding takes `&Frame`, so the `Frame` drops normally after the call, invoking `mpp_frame_deinit` — which frees the MppFrame struct. The encoder thread later accesses this freed memory.

    **Why it works in practice:** The encoder thread tends to consume the frame quickly, and the pool allocator may not immediately reuse the memory. But this is technically undefined behavior.

    **Recommended fix:** Split into two methods or store the input timeout mode:
    ```rust
    /// For blocking mode — frame is borrowed and returned before the call completes.
    pub fn encode_put_frame(&self, frame: &Frame) -> Result<()>;

    /// For async mode — frame ownership is transferred to the encoder pipeline.
    /// Retrieve the frame later from the output packet's meta (KEY_INPUT_FRAME).
    pub fn encode_put_frame_async(&self, frame: Frame) -> Result<()>;
    ```
    The async variant should call `frame.into_raw()` (or `std::mem::forget(frame)`) on success so `mpp_frame_deinit` is not called. On failure, reclaim the frame for proper cleanup.
    */
    pub fn encode_put_frame(&self, frame: &Frame) -> Result<()> {
        let f = self
            .api()
            .encode_put_frame
            .expect("MppApi::encode_put_frame is NULL");
        check_ret(unsafe { f(self.ctx, frame.as_raw()) })
    }

    /// Get an encoded packet from the encoder (async).
    ///
    /// Returns `Ok(None)` if no packet is ready.
    pub fn encode_get_packet(&self) -> Result<Option<Packet>> {
        let mut pkt: sys::MppPacket = ptr::null_mut();
        let f = self
            .api()
            .encode_get_packet
            .expect("MppApi::encode_get_packet is NULL");
        let ret = unsafe { f(self.ctx, &mut pkt) };
        if ret != MPP_RET::MPP_SUCCESS {
            return Err(Error(ret));
        }
        if pkt.is_null() {
            Ok(None)
        } else {
            Ok(Some(unsafe { Packet::from_raw(pkt) }))
        }
    }

    // ================================================================
    // Advanced task API
    // ================================================================

    /// Poll an input or output port for readiness.
    pub fn poll(&self, port: PortType, timeout: Timeout) -> Result<()> {
        let f = self.api().poll.expect("MppApi::poll is NULL");
        check_ret(unsafe { f(self.ctx, port as sys::MppPortType, timeout.to_poll()) })
    }

    /// Dequeue a task from the given port.
    ///
    /// Returns `None` if no task is available.
    pub fn dequeue(&self, port: PortType) -> Result<Option<sys::MppTask>> {
        let mut task: sys::MppTask = ptr::null_mut();
        let f = self.api().dequeue.expect("MppApi::dequeue is NULL");
        check_ret(unsafe { f(self.ctx, port as sys::MppPortType, &mut task) })?;
        if task.is_null() {
            Ok(None)
        } else {
            Ok(Some(task))
        }
    }

    /// Enqueue a task to the given port.
    pub fn enqueue(&self, port: PortType, task: sys::MppTask) -> Result<()> {
        let f = self.api().enqueue.expect("MppApi::enqueue is NULL");
        check_ret(unsafe { f(self.ctx, port as sys::MppPortType, task) })
    }

    // ================================================================
    // Control API
    // ================================================================

    /// Generic control command — the Swiss-army knife of MPP configuration.
    ///
    /// # Safety
    ///
    /// `param` must be a valid pointer to the data type expected by `cmd`,
    /// or null if the command takes no parameter. See `rk_mpi_cmd.h` for
    /// the expected parameter type of each command.
    pub unsafe fn control(&self, cmd: sys::MpiCmd, param: sys::MppParam) -> Result<()> {
        let f = self.api().control.expect("MppApi::control is NULL");
        check_ret(f(self.ctx, cmd, param))
    }

    /// Reset the context — discard all pending packets and frames.
    pub fn reset(&self) -> Result<()> {
        let f = self.api().reset.expect("MppApi::reset is NULL");
        check_ret(unsafe { f(self.ctx) })
    }

    // ================================================================
    // Convenience decoder helpers
    // ================================================================

    /// Set external buffer group for decoder frame output.
    ///
    /// Used after receiving an info-change frame to provide the decoder with
    /// pre-allocated frame buffers.
    pub fn dec_set_ext_buf_group(&self, group: &crate::buffer::BufferGroup) -> Result<()> {
        unsafe {
            self.control(
                sys::MpiCmd::MPP_DEC_SET_EXT_BUF_GROUP,
                group.as_raw() as sys::MppParam,
            )
        }
    }

    /// Signal that the application has handled the info-change and is ready
    /// to continue decoding.
    pub fn dec_set_info_change_ready(&self) -> Result<()> {
        unsafe { self.control(sys::MpiCmd::MPP_DEC_SET_INFO_CHANGE_READY, ptr::null_mut()) }
    }

    /// Set decoder output format (for JPEG or format conversion).
    pub fn dec_set_output_format(&self, fmt: sys::MppFrameFormat) -> Result<()> {
        let mut f = fmt;
        unsafe {
            self.control(
                sys::MpiCmd::MPP_DEC_SET_OUTPUT_FORMAT,
                &mut f as *mut sys::MppFrameFormat as sys::MppParam,
            )
        }
    }

    /// Set decoder frame info (used for FBC/tile format).
    pub fn dec_set_frame_info(&self, frame: &Frame) -> Result<()> {
        // The C API takes an MppFrame (non-const), consumed as read-only.
        unsafe {
            self.control(
                sys::MpiCmd::MPP_DEC_SET_FRAME_INFO,
                frame.as_raw() as sys::MppParam,
            )
        }
    }

    /// Enable parser split mode (must be called before `mpp_init` via raw control).
    ///
    /// For the typical post-init path, use direct control with `MPP_DEC_SET_CFG`.
    pub fn dec_set_parser_split_mode(&self, enable: bool) -> Result<()> {
        let mut val: u32 = enable as u32;
        unsafe {
            self.control(
                sys::MpiCmd::MPP_DEC_SET_PARSER_SPLIT_MODE,
                &mut val as *mut u32 as sys::MppParam,
            )
        }
    }

    // ================================================================
    // Convenience encoder helpers
    // ================================================================

    /// Get encoder header (SPS/PPS/VPS) synchronously into the given packet.
    ///
    /// The packet should be pre-allocated with a buffer, and its length
    /// should be set to 0 before calling.
    pub fn enc_get_hdr_sync(&self, packet: &Packet) -> Result<()> {
        unsafe {
            self.control(
                sys::MpiCmd::MPP_ENC_GET_HDR_SYNC,
                packet.as_raw() as sys::MppParam,
            )
        }
    }

    /// Set encoder configuration.
    pub fn enc_set_cfg(&self, cfg: &crate::cfg::EncCfg) -> Result<()> {
        unsafe { self.control(sys::MpiCmd::MPP_ENC_SET_CFG, cfg.as_raw()) }
    }

    /// Get (read back) the current encoder configuration.
    pub fn enc_get_cfg(&self, cfg: &crate::cfg::EncCfg) -> Result<()> {
        unsafe { self.control(sys::MpiCmd::MPP_ENC_GET_CFG, cfg.as_raw()) }
    }

    /// Set encoder header mode (when SPS/PPS headers are emitted).
    pub fn enc_set_header_mode(&self, mode: crate::cfg::EncHeaderMode) -> Result<()> {
        let mut raw = mode;
        unsafe {
            self.control(
                sys::MpiCmd::MPP_ENC_SET_HEADER_MODE,
                &mut raw as *mut _ as sys::MppParam,
            )
        }
    }

    /// Request the next encoded frame to be an IDR (key) frame.
    pub fn enc_set_idr_frame(&self) -> Result<()> {
        unsafe { self.control(sys::MpiCmd::MPP_ENC_SET_IDR_FRAME, ptr::null_mut()) }
    }

    // ================================================================
    // Convenience decoder config helpers
    // ================================================================

    /// Set decoder configuration.
    pub fn dec_set_cfg(&self, cfg: &crate::cfg::DecCfg) -> Result<()> {
        unsafe { self.control(sys::MpiCmd::MPP_DEC_SET_CFG, cfg.as_raw()) }
    }

    /// Get (read back) the current decoder configuration.
    pub fn dec_get_cfg(&self, cfg: &crate::cfg::DecCfg) -> Result<()> {
        unsafe { self.control(sys::MpiCmd::MPP_DEC_GET_CFG, cfg.as_raw()) }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            sys::mpp_destroy(self.ctx);
        }
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MppContext")
            .field("ctx", &self.ctx)
            .field("api_version", &self.api().version)
            .finish()
    }
}

// ============================================================
// Meta helpers — convenience functions on raw MppMeta
// ============================================================

/// Helper functions that operate on a raw `MppMeta` handle.
///
/// These are intentionally free functions rather than a wrapper struct,
/// because `MppMeta` lifetime is always tied to a parent Frame or Packet.
pub mod meta {
    use super::*;

    pub fn set_s32(meta: sys::MppMeta, key: sys::MppMetaKey, val: i32) -> Result<()> {
        check_ret(unsafe { sys::mpp_meta_set_s32(meta, key, val) })
    }

    pub fn get_s32(meta: sys::MppMeta, key: sys::MppMetaKey) -> Result<i32> {
        let mut val: i32 = 0;
        check_ret(unsafe { sys::mpp_meta_get_s32(meta, key, &mut val) })?;
        Ok(val)
    }

    pub fn set_s64(meta: sys::MppMeta, key: sys::MppMetaKey, val: i64) -> Result<()> {
        check_ret(unsafe { sys::mpp_meta_set_s64(meta, key, val) })
    }

    pub fn get_s64(meta: sys::MppMeta, key: sys::MppMetaKey) -> Result<i64> {
        let mut val: i64 = 0;
        check_ret(unsafe { sys::mpp_meta_get_s64(meta, key, &mut val) })?;
        Ok(val)
    }

    /// Set a raw pointer in metadata.
    ///
    /// # Safety
    ///
    /// The pointed-to data must remain valid for the lifetime of the meta's
    /// parent frame/packet (or until the meta key is overwritten).
    pub unsafe fn set_ptr(
        meta: sys::MppMeta,
        key: sys::MppMetaKey,
        val: *mut c_void,
    ) -> Result<()> {
        check_ret(sys::mpp_meta_set_ptr(meta, key, val))
    }

    pub fn get_ptr(meta: sys::MppMeta, key: sys::MppMetaKey) -> Result<*mut c_void> {
        let mut val: *mut c_void = ptr::null_mut();
        check_ret(unsafe { sys::mpp_meta_get_ptr(meta, key, &mut val) })?;
        Ok(val)
    }

    /// Transfer a frame into metadata (e.g., `KEY_OUTPUT_FRAME` for advanced decode).
    ///
    /// The frame is **consumed** — the raw `MppFrame` pointer is stored in the
    /// meta slot. Retrieve it later with [`get_frame`]. If the meta slot is
    /// overwritten or never read, the frame will leak (matching C semantics).
    pub fn set_frame(meta: sys::MppMeta, key: sys::MppMetaKey, frame: Frame) -> Result<()> {
        let raw = frame.into_raw();
        let ret = check_ret(unsafe { sys::mpp_meta_set_frame(meta, key, raw) });
        if ret.is_err() {
            // Reclaim the frame so it is properly freed on failure.
            drop(unsafe { Frame::from_raw(raw) });
        }
        ret
    }

    /// Retrieve a frame from metadata.
    ///
    /// This is a **consuming read** — the meta slot is cleared and the raw
    /// `MppFrame` pointer is transferred into the returned [`Frame`].
    /// Calling again for the same key returns `None`.
    pub fn get_frame(meta: sys::MppMeta, key: sys::MppMetaKey) -> Result<Option<Frame>> {
        let mut raw: sys::MppFrame = ptr::null_mut();
        check_ret(unsafe { sys::mpp_meta_get_frame(meta, key, &mut raw) })?;
        if raw.is_null() {
            Ok(None)
        } else {
            Ok(Some(unsafe { Frame::from_raw(raw) }))
        }
    }

    /// Transfer a packet into metadata (e.g., `KEY_OUTPUT_PACKET` for encode).
    ///
    /// The packet is **consumed**. Retrieve it later with [`get_packet`].
    pub fn set_packet(meta: sys::MppMeta, key: sys::MppMetaKey, packet: Packet) -> Result<()> {
        let raw = packet.into_raw();
        let ret = check_ret(unsafe { sys::mpp_meta_set_packet(meta, key, raw) });
        if ret.is_err() {
            drop(unsafe { Packet::from_raw(raw) });
        }
        ret
    }

    /// Get a packet from metadata.
    pub fn get_packet(meta: sys::MppMeta, key: sys::MppMetaKey) -> Result<Option<Packet>> {
        let mut raw: sys::MppPacket = ptr::null_mut();
        check_ret(unsafe { sys::mpp_meta_get_packet(meta, key, &mut raw) })?;
        if raw.is_null() {
            Ok(None)
        } else {
            Ok(Some(unsafe { Packet::from_raw(raw) }))
        }
    }

    /// Attach a buffer to metadata (e.g., `KEY_MOTION_INFO`).
    ///
    /// The buffer's reference count is incremented so the meta holds an
    /// independent reference. [`get_buffer`] returns an owned [`Buffer`]
    /// that inherits this reference. If the meta slot is overwritten or
    /// never read, the extra reference leaks (matching C semantics where
    /// the meta does not clean up stored buffers).
    pub fn set_buffer(
        meta: sys::MppMeta,
        key: sys::MppMetaKey,
        buffer: &crate::buffer::Buffer,
    ) -> Result<()> {
        // Increment refcount so the meta holds a valid, independent reference.
        check_ret(unsafe { sys::mpp_buffer_inc_ref_with_caller(buffer.as_raw(), crate::CALLER) })?;
        let ret = check_ret(unsafe { sys::mpp_meta_set_buffer(meta, key, buffer.as_raw()) });
        if ret.is_err() {
            // Rollback: release the extra reference on failure.
            unsafe { sys::mpp_buffer_put_with_caller(buffer.as_raw(), crate::CALLER) };
        }
        ret
    }

    /// Retrieve a buffer from metadata.
    ///
    /// This is a **consuming read** — the meta slot is cleared. The returned
    /// [`Buffer`] inherits the reference that was stored by [`set_buffer`].
    pub fn get_buffer(
        meta: sys::MppMeta,
        key: sys::MppMetaKey,
    ) -> Result<Option<crate::buffer::Buffer>> {
        let mut raw: sys::MppBuffer = ptr::null_mut();
        check_ret(unsafe { sys::mpp_meta_get_buffer(meta, key, &mut raw) })?;
        if raw.is_null() {
            Ok(None)
        } else {
            Ok(Some(unsafe { crate::buffer::Buffer::from_raw(raw) }))
        }
    }
}

// ============================================================
// Global queries
// ============================================================

/// Check if the given context type and coding are supported by MPP.
pub fn check_support_format(ctx_type: CtxType, coding: CodingType) -> bool {
    unsafe { sys::mpp_check_support_format(ctx_type, coding) == MPP_RET::MPP_SUCCESS }
}
