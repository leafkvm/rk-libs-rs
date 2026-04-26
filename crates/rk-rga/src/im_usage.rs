use rkblob_sys::rga_sys as ffi;

/// Rotation angle (mutually exclusive, bits 0–2 of `IM_USAGE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum Rotation {
    #[default]
    Rot0 = 0,
    Rot90 = ffi::IM_USAGE_IM_HAL_TRANSFORM_ROT_90,
    Rot180 = ffi::IM_USAGE_IM_HAL_TRANSFORM_ROT_180,
    Rot270 = ffi::IM_USAGE_IM_HAL_TRANSFORM_ROT_270,
}

/// Porter-Duff alpha blending mode (mutually exclusive, bits 6–16 of `IM_USAGE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum AlphaBlendMode {
    SrcOver = ffi::IM_USAGE_IM_ALPHA_BLEND_SRC_OVER,
    Src = ffi::IM_USAGE_IM_ALPHA_BLEND_SRC,
    Dst = ffi::IM_USAGE_IM_ALPHA_BLEND_DST,
    SrcIn = ffi::IM_USAGE_IM_ALPHA_BLEND_SRC_IN,
    DstIn = ffi::IM_USAGE_IM_ALPHA_BLEND_DST_IN,
    SrcOut = ffi::IM_USAGE_IM_ALPHA_BLEND_SRC_OUT,
    DstOut = ffi::IM_USAGE_IM_ALPHA_BLEND_DST_OUT,
    DstOver = ffi::IM_USAGE_IM_ALPHA_BLEND_DST_OVER,
    SrcAtop = ffi::IM_USAGE_IM_ALPHA_BLEND_SRC_ATOP,
    DstAtop = ffi::IM_USAGE_IM_ALPHA_BLEND_DST_ATOP,
    Xor = ffi::IM_USAGE_IM_ALPHA_BLEND_XOR,
}

/// Color key mode (mutually exclusive, bits 17–18 of `IM_USAGE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ColorKeyMode {
    Normal = ffi::IM_USAGE_IM_ALPHA_COLORKEY_NORMAL,
    Inverted = ffi::IM_USAGE_IM_ALPHA_COLORKEY_INVERTED,
}

/// Structured representation of the `IM_USAGE` bitmask for `improcess`.
///
/// Each group of related flags is represented by an `Option<Enum>` or `bool`.
/// `Default` yields an empty usage (no operations, synchronous).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImUsage {
    pub rotation: Rotation,
    pub flip_h: bool,
    pub flip_v: bool,
    pub alpha_blend: Option<AlphaBlendMode>,
    pub alpha_blend_pre_mul: bool,
    pub colorkey: Option<ColorKeyMode>,
    pub color_fill: bool,
    pub color_palette: bool,
    pub nn_quantize: bool,
    pub rop: bool,
    pub mosaic: bool,
    pub osd: bool,
    pub pre_intr: bool,
    pub alpha_bit_map: bool,
    pub gauss: bool,
}

impl ImUsage {
    pub fn to_raw(self) -> i32 {
        let mut bits: i32 = 0;

        bits |= self.rotation as i32;

        // bits |= self.flip as i32;
        bits |= match (self.flip_h, self.flip_v) {
            (false, false) => 0,
            (true, false) => ffi::IM_USAGE_IM_HAL_TRANSFORM_FLIP_H,
            (false, true) => ffi::IM_USAGE_IM_HAL_TRANSFORM_FLIP_V,
            (true, true) => ffi::IM_USAGE_IM_HAL_TRANSFORM_FLIP_H_V,
        };

        if let Some(mode) = self.alpha_blend {
            bits |= mode as i32;
        }

        if self.alpha_blend_pre_mul {
            bits |= ffi::IM_USAGE_IM_ALPHA_BLEND_PRE_MUL;
        }

        if let Some(ck) = self.colorkey {
            bits |= ck as i32;
        }

        if self.color_fill {
            bits |= ffi::IM_USAGE_IM_COLOR_FILL;
        }
        if self.color_palette {
            bits |= ffi::IM_USAGE_IM_COLOR_PALETTE;
        }
        if self.nn_quantize {
            bits |= ffi::IM_USAGE_IM_NN_QUANTIZE;
        }
        if self.rop {
            bits |= ffi::IM_USAGE_IM_ROP;
        }
        if self.mosaic {
            bits |= ffi::IM_USAGE_IM_MOSAIC;
        }
        if self.osd {
            bits |= ffi::IM_USAGE_IM_OSD;
        }
        if self.pre_intr {
            bits |= ffi::IM_USAGE_IM_PRE_INTR;
        }
        if self.alpha_bit_map {
            bits |= ffi::IM_USAGE_IM_ALPHA_BIT_MAP;
        }
        if self.gauss {
            bits |= ffi::IM_USAGE_IM_GAUSS;
        }

        bits
    }
}
