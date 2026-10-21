//! Enum values shared with the MPP/KMPP config tables.
//!
//! These mirror `inc/rk_type.h`, `inc/mpp_frame.h`, `inc/rk_venc_rc.h` and
//! `inc/rk_venc_cmd.h` from rockchip-mpp. Only encoder-relevant values are
//! defined; the raw `set_s32`/`set_u32` escape hatch on [`crate::Obj`]
//! accepts anything else.

/// `MppCtxType`.
pub const MPP_CTX_ENC: u32 = 1;

/// `MppCodingType` — codecs the VEPU511 encoder supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Coding {
    /// H.264 / AVC
    Avc = 7,
    /// Motion JPEG
    Mjpeg = 8,
    /// H.265 / HEVC
    Hevc = 0x0100_0004,
}

/// `MppFrameFormat` (YUV group is 0x0_0000-based, RGB group 0x1_0000-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum FrameFormat {
    /// YYYY... UVUV (NV12)
    Yuv420Sp = 0,
    Yuv420Sp10Bit = 1,
    /// YYYY... UVUV full-height chroma (NV16)
    Yuv422Sp = 2,
    Yuv420P = 4,
    /// NV21
    Yuv420SpVu = 5,
    Yuv422P = 6,
    /// NV61
    Yuv422SpVu = 7,
    /// YUYV (YUY2)
    Yuv422Yuyv = 8,
    Yuv422Yvyu = 9,
    Yuv422Uyvy = 10,
    Yuv422Vyuy = 11,
    Yuv400 = 12,
    /// Packed Y,U,V bytes; requires the LeafKVM VEPU511 driver extension.
    /// No DDR conversion; full-range zero is rounded to one by the Q8 stage.
    Yuv444Packed = 20,
    Rgb565 = 0x1_0000,
    Bgr565 = 0x1_0001,
    Rgb888 = 0x1_0006,
    Bgr888 = 0x1_0007,
    Argb8888 = 0x1_000a,
    Abgr8888 = 0x1_000b,
    Bgra8888 = 0x1_000c,
    Rgba8888 = 0x1_000d,
}

/// `MppEncRcMode` (`rc:mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum RcMode {
    Vbr = 0,
    Cbr = 1,
    FixQp = 2,
    Avbr = 3,
    /// Smart rate control; silently degrades to CBR without `kmpp_smart.ko`.
    SmtRc = 4,
}

/// `MppEncHeaderMode` (`MPP_ENC_SET_HEADER_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum HeaderMode {
    /// Headers only in the first packet.
    Default = 0,
    /// Prepend SPS/PPS(/VPS) to every IDR packet.
    EachIdr = 1,
}

/// `MppFrameColorRange` (`prep:colorrange`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ColorRange {
    Unspecified = 0,
    /// Limited / MPEG range.
    Limited = 1,
    /// Full / JPEG range.
    Full = 2,
}

/// `MppFrameColorSpace` (matrix coefficients, `prep:colorspace`).
pub mod color_space {
    pub const BT709: i32 = 1;
    pub const UNSPECIFIED: i32 = 2;
    pub const BT470BG: i32 = 5;
    pub const SMPTE170M: i32 = 6;
    pub const BT2020_NCL: i32 = 9;
}

/// `MppFrameColorPrimaries` (`prep:colorprim`).
pub mod color_primaries {
    pub const BT709: i32 = 1;
    pub const UNSPECIFIED: i32 = 2;
    pub const BT470BG: i32 = 5;
    pub const SMPTE170M: i32 = 6;
    pub const BT2020: i32 = 9;
}

/// `MppFrameColorTransferCharacteristic` (`prep:colortrc`).
pub mod color_trc {
    pub const BT709: i32 = 1;
    pub const UNSPECIFIED: i32 = 2;
    pub const SMPTE170M: i32 = 6;
    pub const BT2020_10: i32 = 14;
}

/// `KMPP_PACKET_FLAG_*` bits in a packet's `flag` field.
pub mod packet_flag {
    pub const EOS: u32 = 0x01;
    pub const EXTRA_DATA: u32 = 0x02;
    pub const INTERNAL: u32 = 0x04;
    pub const EXTERNAL: u32 = 0x08;
    pub const INTRA: u32 = 0x10;
    pub const PARTITION: u32 = 0x20;
    pub const EOI: u32 = 0x40;
}

/// `MppPollType` values for `input_timeout` / `output_timeout`.
pub mod poll_timeout {
    pub const BLOCK: i32 = -1;
    pub const NON_BLOCK: i32 = 0;
}
