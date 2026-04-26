use rkblob_sys::rga_sys as ffi;

/// RGB→YUV conversion standard (occupies bits 2–3 of `IM_COLOR_SPACE_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Bt601Full,
    Bt601Limit,
    Bt709Limit,
}

/// RGB→Y4/Y1 grayscale conversion mode (occupies bits 4–5 of `IM_COLOR_SPACE_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RgbToY4 {
    Raw,
    Dither,
    Y1Dither,
}

/// Per-buffer full-CSC color space descriptor (occupies bits 8–11 of `IM_COLOR_SPACE_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullCscSpace {
    RgbFull,
    RgbClip,
    YuvBt601Limit,
    YuvBt601Full,
    YuvBt709Limit,
    YuvBt709Full,
}

/// Structured replacement for the raw `IM_COLOR_SPACE_MODE` bitmask.
///
/// The four independent bit-field dimensions of `IM_COLOR_SPACE_MODE` are each
/// represented as a separate `Option` field:
///
/// | Field        | Bit range | Mask constant         |
/// |------------- |-----------|-----------------------|
/// | `yuv_to_rgb` | 0–1       | `IM_YUV_TO_RGB_MASK`  |
/// | `rgb_to_yuv` | 2–3       | `IM_RGB_TO_YUV_MASK`  |
/// | `rgb_to_y4`  | 4–5       | `IM_Y4_MASK`          |
/// | `full_csc`   | 8–11      | `IM_FULL_CSC_MASK`    |
///
/// `None` in any field leaves the corresponding bits at 0 (default/disabled).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorSpaceMode {
    pub yuv_to_rgb: Option<Range>,
    pub rgb_to_yuv: Option<Range>,
    pub rgb_to_y4: Option<RgbToY4>,
    pub full_csc: Option<FullCscSpace>,
}

impl From<ColorSpaceMode> for ffi::IM_COLOR_SPACE_MODE {
    fn from(mode: ColorSpaceMode) -> Self {
        let mut bits: ffi::IM_COLOR_SPACE_MODE = 0;

        if let Some(v) = mode.yuv_to_rgb {
            bits |= match v {
                Range::Bt601Limit => ffi::IM_COLOR_SPACE_MODE_IM_YUV_TO_RGB_BT601_LIMIT,
                Range::Bt601Full => ffi::IM_COLOR_SPACE_MODE_IM_YUV_TO_RGB_BT601_FULL,
                Range::Bt709Limit => ffi::IM_COLOR_SPACE_MODE_IM_YUV_TO_RGB_BT709_LIMIT,
            };
        }

        if let Some(v) = mode.rgb_to_yuv {
            bits |= match v {
                Range::Bt601Full => ffi::IM_COLOR_SPACE_MODE_IM_RGB_TO_YUV_BT601_FULL,
                Range::Bt601Limit => ffi::IM_COLOR_SPACE_MODE_IM_RGB_TO_YUV_BT601_LIMIT,
                Range::Bt709Limit => ffi::IM_COLOR_SPACE_MODE_IM_RGB_TO_YUV_BT709_LIMIT,
            };
        }

        if let Some(v) = mode.rgb_to_y4 {
            bits |= match v {
                RgbToY4::Raw => ffi::IM_COLOR_SPACE_MODE_IM_RGB_TO_Y4,
                RgbToY4::Dither => ffi::IM_COLOR_SPACE_MODE_IM_RGB_TO_Y4_DITHER,
                RgbToY4::Y1Dither => ffi::IM_COLOR_SPACE_MODE_IM_RGB_TO_Y1_DITHER,
            };
        }

        if let Some(v) = mode.full_csc {
            bits |= match v {
                FullCscSpace::RgbFull => ffi::IM_COLOR_SPACE_MODE_IM_RGB_FULL,
                FullCscSpace::RgbClip => ffi::IM_COLOR_SPACE_MODE_IM_RGB_CLIP,
                FullCscSpace::YuvBt601Limit => ffi::IM_COLOR_SPACE_MODE_IM_YUV_BT601_LIMIT_RANGE,
                FullCscSpace::YuvBt601Full => ffi::IM_COLOR_SPACE_MODE_IM_YUV_BT601_FULL_RANGE,
                FullCscSpace::YuvBt709Limit => ffi::IM_COLOR_SPACE_MODE_IM_YUV_BT709_LIMIT_RANGE,
                FullCscSpace::YuvBt709Full => ffi::IM_COLOR_SPACE_MODE_IM_YUV_BT709_FULL_RANGE,
            };
        }

        bits
    }
}
