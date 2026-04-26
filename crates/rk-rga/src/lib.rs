pub use ffi::_Rga_SURF_FORMAT as Format;
use rkblob_sys::rga_sys::{self as ffi, rga_buffer_t};
use std::{
    ops::{Deref, DerefMut},
    os::fd::{AsRawFd as _, BorrowedFd, RawFd},
};

mod color_space;
mod error;
mod im_usage;

pub use color_space::{ColorSpaceMode, FullCscSpace, Range, RgbToY4};
pub use error::{RgaError, RgaResult};
pub use im_usage::{AlphaBlendMode, ColorKeyMode, ImUsage, Rotation};
pub use rk_dma_buf_mem::DmaBuffer;

use crate::error::rga_failed;

pub fn get_size_from_w_h_f(w: u32, h: u32, format: Format) -> usize {
    let bpp = unsafe { ffi::get_bpp_from_format(format as _) };
    (w as f64 * h as f64 * bpp as f64).round() as usize
}

fn import_handle_fd(raw_fd: RawFd, size: usize) -> Result<ffi::rga_buffer_handle_t, RgaError> {
    let handle = unsafe { ffi::_Z15importbuffer_fdii(raw_fd, size as _) };
    if handle == 0 {
        Err(rga_failed())
    } else {
        Ok(handle)
    }
}

fn import_handle_fd_format(
    raw_fd: i32,
    width: u32,
    height: u32,
    format: Format,
) -> Result<ffi::rga_buffer_handle_t, RgaError> {
    let handle =
        unsafe { ffi::_Z15importbuffer_fdiiii(raw_fd, width as i32, height as i32, format as _) };
    if handle == 0 {
        Err(rga_failed())
    } else {
        Ok(handle)
    }
}

fn release_handle(handle: ffi::rga_buffer_handle_t) {
    unsafe {
        let ret = ffi::releasebuffer_handle(handle);
        if ret != ffi::IM_STATUS::IM_STATUS_SUCCESS {
            log::error!("releasebuffer_handle failed: {:?}", ret);
        }
    }
}

/// An RGA buffer that wraps an imported handle and its associated metadata.
///
/// When dropped, the imported handle is released.
pub struct RgaBuffer {
    handle: ffi::rga_buffer_handle_t,
    rga_buffer: ffi::rga_buffer_t,
}

impl RgaBuffer {
    fn from_handle(
        handle: ffi::rga_buffer_handle_t,
        width: u32,
        height: u32,
        format: Format,
        wstride: u32,
        hstride: u32,
    ) -> Self {
        let rga_format = format;
        let rga_buffer = unsafe {
            ffi::wrapbuffer_handle_t(
                handle,
                width as _,
                height as _,
                wstride as _,
                hstride as _,
                rga_format as u32 as _,
            )
        };
        Self { handle, rga_buffer }
    }

    pub fn width(&self) -> u32 {
        self.rga_buffer.width as u32
    }

    pub fn height(&self) -> u32 {
        self.rga_buffer.height as u32
    }

    pub fn format(&self) -> Format {
        unsafe { std::mem::transmute(self.rga_buffer.format) }
    }

    pub fn wstride(&self) -> u32 {
        self.rga_buffer.wstride as u32
    }

    pub fn set_color_space(&mut self, cs: ColorSpaceMode) {
        self.rga_buffer.color_space_mode = ffi::IM_COLOR_SPACE_MODE::from(cs) as i32;
    }
}

impl Drop for RgaBuffer {
    fn drop(&mut self) {
        release_handle(self.handle);
    }
}

// Safety: `RgaBuffer` is constructed through `wrapbuffer_handle_t()`, which zeroes the
// descriptor and stores only the imported handle plus scalar image metadata for this
// handle-backed path. Upstream librga imports and releases that handle via process-global
// ioctls on a shared session rather than thread-local state, so moving or sharing this
// wrapper across threads does not create additional aliasing beyond the external dma-buf
// contract already represented by the underlying buffer handle.
unsafe impl Send for RgaBuffer {}
unsafe impl Sync for RgaBuffer {}

impl std::fmt::Debug for RgaBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RgaBuffer")
            .field("handle", &self.handle)
            .field("width", &self.width())
            .field("height", &self.height())
            .field("format", &self.format())
            .field("wstride", &self.wstride())
            .field("hstride", &(self.rga_buffer.hstride as u32))
            .finish()
    }
}

/// An RGA buffer backed by an owned DMA memory allocation.
///
/// Derefs to [`RgaBuffer`] for use in RGA operations. Access the underlying
/// DMA memory via [`dma_buffer`](Self::dma_buffer) / [`dma_buffer_mut`](Self::dma_buffer_mut).
pub struct OwnedRgaBuffer {
    rga: RgaBuffer,
    dma: DmaBuffer,
}

impl OwnedRgaBuffer {
    pub fn new(width: u32, height: u32, format: Format) -> Result<Self, RgaError> {
        let size = get_size_from_w_h_f(width, height, format);
        let dma = DmaBuffer::new(size).map_err(|_| rga_failed())?;
        let handle = import_handle_fd(dma.as_raw_fd(), size)?;
        let rga = RgaBuffer::from_handle(handle, width, height, format, width, height);
        Ok(Self { rga, dma })
    }

    pub fn dma_buffer(&self) -> &DmaBuffer {
        &self.dma
    }

    pub fn dma_buffer_mut(&mut self) -> &mut DmaBuffer {
        &mut self.dma
    }
}

impl Deref for OwnedRgaBuffer {
    type Target = RgaBuffer;
    fn deref(&self) -> &RgaBuffer {
        &self.rga
    }
}

impl DerefMut for OwnedRgaBuffer {
    fn deref_mut(&mut self) -> &mut RgaBuffer {
        &mut self.rga
    }
}

impl std::fmt::Debug for OwnedRgaBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedRgaBuffer")
            .field("rga", &self.rga)
            .field("dma", &self.dma)
            .finish()
    }
}

/// An RGA buffer imported from a borrowed file descriptor.
///
/// The lifetime parameter ensures the fd source outlives this buffer.
/// Derefs to [`RgaBuffer`] for use in RGA operations.
pub struct BorrowedRgaBuffer<'fd> {
    _fd: BorrowedFd<'fd>,
    rga: RgaBuffer,
}

impl<'fd> BorrowedRgaBuffer<'fd> {
    /// Import a buffer from a borrowed fd with format information.
    pub fn new(
        fd: BorrowedFd<'fd>,
        width: u32,
        height: u32,
        format: Format,
    ) -> Result<Self, RgaError> {
        let handle = import_handle_fd_format(fd.as_raw_fd(), width, height, format)?;
        let rga = RgaBuffer::from_handle(handle, width, height, format, width, height);
        Ok(Self { _fd: fd, rga })
    }

    /// Import a buffer from a borrowed fd with explicit size and stride.
    pub fn new_stride(
        fd: BorrowedFd<'fd>,
        size: usize,
        width: u32,
        height: u32,
        format: Format,
        wstride: u32,
        hstride: u32,
    ) -> Result<Self, RgaError> {
        let handle = import_handle_fd(fd.as_raw_fd(), size)?;
        let rga = RgaBuffer::from_handle(handle, width, height, format, wstride, hstride);
        Ok(Self { _fd: fd, rga })
    }
}

impl<'fd> Deref for BorrowedRgaBuffer<'fd> {
    type Target = RgaBuffer;
    fn deref(&self) -> &RgaBuffer {
        &self.rga
    }
}

impl<'fd> DerefMut for BorrowedRgaBuffer<'fd> {
    fn deref_mut(&mut self) -> &mut RgaBuffer {
        &mut self.rga
    }
}

impl std::fmt::Debug for BorrowedRgaBuffer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BorrowedRgaBuffer")
            .field("rga", &self.rga)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ImRect {
    pub fn xx(&self) -> u32 {
        self.x + self.width
    }

    pub fn yy(&self) -> u32 {
        self.y + self.height
    }

    pub fn within_rect(&self, rhs: &ImRect) -> bool {
        self.x <= rhs.x && self.y <= rhs.y && self.xx() <= rhs.xx() && self.yy() <= rhs.yy()
    }
}

impl std::fmt::Display for ImRect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "[({}, {}), ({}, {})]",
            self.x,
            self.y,
            self.xx(),
            self.yy()
        ))
    }
}

impl From<ImRect> for ffi::im_rect {
    fn from(value: ImRect) -> Self {
        ffi::im_rect {
            x: value.x as _,
            y: value.y as _,
            width: value.width as _,
            height: value.height as _,
        }
    }
}

fn check_rga(ret: ffi::IM_STATUS) -> RgaResult {
    if ret == ffi::IM_STATUS::IM_STATUS_SUCCESS {
        Ok(())
    } else {
        Err(RgaError { inner: ret })
    }
}

pub fn imcopy(src: &RgaBuffer, dst: &mut RgaBuffer) -> RgaResult {
    log::trace!("imcopy: {:?} => {:?}", src, dst);
    check_rga(unsafe { ffi::imcopy_t(src.rga_buffer, dst.rga_buffer, 1) })
}

pub fn imcvtcolor(
    src: &RgaBuffer,
    dst: &mut RgaBuffer,
    mode: ffi::IM_COLOR_SPACE_MODE,
) -> RgaResult {
    log::trace!("imcvtcolor: {:?} => {:?}, mode = {}", src, dst, mode);
    check_rga(unsafe {
        ffi::imcvtcolor_t(
            src.rga_buffer,
            dst.rga_buffer,
            src.format() as i32,
            dst.format() as i32,
            mode as i32,
            1,
        )
    })
}

#[repr(u32)]
pub enum ScaleMode {
    Nearest = ffi::IM_SCALE_MODE_INTER_NEAREST,
    Linear = ffi::IM_SCALE_MODE_INTER_LINEAR,
    Cubic = ffi::IM_SCALE_MODE_INTER_CUBIC,
}

pub fn imresize(
    src: &RgaBuffer,
    dst: &mut RgaBuffer,
    fx: f64,
    fy: f64,
    interpolation: ScaleMode,
) -> RgaResult {
    log::trace!("imresize: {:?} => {:?}, fx = {}, fy = {}", src, dst, fx, fy);
    check_rga(unsafe {
        ffi::imresize_t(
            src.rga_buffer,
            dst.rga_buffer,
            fx,
            fy,
            interpolation as _,
            1,
        )
    })
}

pub fn imcrop(src: &RgaBuffer, dst: &mut RgaBuffer, rect: ImRect) -> RgaResult {
    log::trace!("imcrop: {:?} => {:?}, rect = {}", src, dst, &rect);

    check_rga(unsafe { ffi::imcrop_t(src.rga_buffer, dst.rga_buffer, ffi::im_rect::from(rect), 1) })
}

pub fn imrotate(src: &RgaBuffer, dst: &mut RgaBuffer, rotation: Rotation) -> RgaResult {
    log::trace!(
        "imrotate: {:?} => {:?}, rotation = {:?}",
        src,
        dst,
        rotation
    );

    check_rga(unsafe { ffi::imrotate_t(src.rga_buffer, dst.rga_buffer, rotation as _, 1) })
}

pub fn imblend(
    fg: &RgaBuffer,
    bg: &mut RgaBuffer,
    mode: AlphaBlendMode,
    pre_mul: bool,
) -> RgaResult {
    log::trace!(
        "imblend: {:?} => {:?}, mode = {:?}, premul = {}",
        fg,
        bg,
        mode,
        pre_mul
    );
    let usage = ImUsage {
        alpha_blend: Some(mode),
        alpha_blend_pre_mul: pre_mul,
        ..Default::default()
    };
    let src_b: rga_buffer_t = unsafe { std::mem::zeroed() };
    check_rga(unsafe {
        ffi::imblend_t(fg.rga_buffer, src_b, bg.rga_buffer, usage.to_raw() as _, 1)
    })
}

pub fn imcomposite(
    fg: &RgaBuffer,
    bg: &RgaBuffer,
    out: &mut RgaBuffer,
    mode: AlphaBlendMode,
    pre_mul: bool,
) -> RgaResult {
    log::trace!(
        "imcomposite: {:?} + {:?} => {:?}, mode = {:?}, premul = {}",
        fg,
        bg,
        out,
        mode,
        pre_mul
    );
    let usage = ImUsage {
        alpha_blend: Some(mode),
        alpha_blend_pre_mul: pre_mul,
        ..Default::default()
    };
    check_rga(unsafe {
        ffi::imblend_t(
            fg.rga_buffer,
            bg.rga_buffer,
            out.rga_buffer,
            usage.to_raw() as _,
            1,
        )
    })
}

pub fn imfill(dst: &mut RgaBuffer, rect: ImRect, color: u32) -> RgaResult {
    log::trace!(
        "imfill: {:?}, rect = {:?}, color = {:?}",
        dst,
        &rect,
        &color
    );
    let im_rect = rect.into();
    check_rga(unsafe { ffi::imfill_t(dst.rga_buffer, im_rect, color as i32, 1) })
}

pub fn imfill_background(dst: &mut RgaBuffer, color: u32) -> RgaResult {
    let im_rect = ImRect {
        x: 0,
        y: 0,
        width: dst.width(),
        height: dst.height(),
    };
    imfill(dst, im_rect, color)
}

pub fn imcolorkey(
    src: &RgaBuffer,
    dst: &mut RgaBuffer,
    range: (u32, u32),
    colorkey_mode: ColorKeyMode,
) -> RgaResult {
    let range = ffi::im_colorkey_range {
        min: range.0 as i32,
        max: range.1 as i32,
    };
    let usage = ImUsage {
        colorkey: Some(colorkey_mode),
        ..Default::default()
    };

    check_rga(unsafe {
        ffi::imcolorkey_t(
            src.rga_buffer,
            dst.rga_buffer,
            range,
            usage.to_raw() as i32,
            1,
        )
    })
}

pub fn move_image(src: (&RgaBuffer, ImRect), dst: (&mut RgaBuffer, ImRect)) -> RgaResult {
    improcess(
        src.0,
        dst.0,
        None,
        Some(src.1),
        Some(dst.1),
        None,
        ImUsage::default(),
    )
}

/// Call the RGA `improcess` C entry-point.
///
/// Pass `None` for `pat` / any rect to leave them zeroed.
/// The call is always synchronous (`IM_SYNC` is OR-ed in automatically).
pub fn improcess(
    src: &RgaBuffer,
    dst: &mut RgaBuffer,
    pat: Option<&RgaBuffer>,
    src_rect: Option<ImRect>,
    dst_rect: Option<ImRect>,
    pat_rect: Option<ImRect>,
    usage: ImUsage,
) -> RgaResult {
    use std::mem::zeroed;

    let pat_buf = pat
        .map(|p| p.rga_buffer)
        .unwrap_or_else(|| unsafe { zeroed() });
    let srect = src_rect
        .map(Into::into)
        .unwrap_or_else(|| unsafe { zeroed() });
    let drect = dst_rect
        .map(Into::into)
        .unwrap_or_else(|| unsafe { zeroed() });
    let prect = pat_rect
        .map(Into::into)
        .unwrap_or_else(|| unsafe { zeroed() });

    let raw_usage = usage.to_raw() | ffi::IM_USAGE_IM_SYNC;

    check_rga(unsafe {
        ffi::improcess(
            src.rga_buffer,
            dst.rga_buffer,
            pat_buf,
            srect,
            drect,
            prect,
            raw_usage,
        )
    })
}
