use std::ffi::{c_void, CString};
use std::fmt;
use std::ptr;

use crate::{check_ret, sys, Result};

pub use sys::MppCfgStrFmt as CfgStrFmt;
pub use sys::MppEncHeaderMode as EncHeaderMode;

#[inline]
fn ckey(key: &str) -> CString {
    CString::new(key).expect("MPP cfg key must not contain interior NUL bytes")
}

/// Owned handle to an MPP encoder configuration object.
pub struct EncCfg {
    raw: sys::MppEncCfg,
}

// MppEncCfg is internally thread-safe.
unsafe impl Send for EncCfg {}
unsafe impl Sync for EncCfg {}

impl EncCfg {
    /// Create a new userspace encoder configuration.
    pub fn new() -> Result<Self> {
        let mut cfg: sys::MppEncCfg = ptr::null_mut();
        check_ret(unsafe { sys::mpp_enc_cfg_init(&mut cfg) })?;
        Ok(Self { raw: cfg })
    }

    /// Create a new kernel encoder configuration.
    pub fn new_kernel() -> Result<Self> {
        let mut cfg: sys::MppEncCfg = ptr::null_mut();
        check_ret(unsafe { sys::mpp_enc_cfg_init_k(&mut cfg) })?;
        Ok(Self { raw: cfg })
    }

    pub fn set_s8(&self, key: &str, val: i8) -> Result<()> {
        let k = ckey(key);
        check_ret(unsafe { sys::mpp_enc_cfg_set_s8(self.raw, k.as_ptr(), val) })
    }

    pub fn set_u8(&self, key: &str, val: u8) -> Result<()> {
        let k = ckey(key);
        check_ret(unsafe { sys::mpp_enc_cfg_set_u8(self.raw, k.as_ptr(), val) })
    }

    pub fn set_s16(&self, key: &str, val: i16) -> Result<()> {
        let k = ckey(key);
        check_ret(unsafe { sys::mpp_enc_cfg_set_s16(self.raw, k.as_ptr(), val) })
    }

    pub fn set_u16(&self, key: &str, val: u16) -> Result<()> {
        let k = ckey(key);
        check_ret(unsafe { sys::mpp_enc_cfg_set_u16(self.raw, k.as_ptr(), val) })
    }

    pub fn set_s32(&self, key: &str, val: i32) -> Result<()> {
        let k = ckey(key);
        check_ret(unsafe { sys::mpp_enc_cfg_set_s32(self.raw, k.as_ptr(), val) })
    }

    pub fn set_u32(&self, key: &str, val: u32) -> Result<()> {
        let k = ckey(key);
        check_ret(unsafe { sys::mpp_enc_cfg_set_u32(self.raw, k.as_ptr(), val) })
    }

    pub fn set_s64(&self, key: &str, val: i64) -> Result<()> {
        let k = ckey(key);
        check_ret(unsafe { sys::mpp_enc_cfg_set_s64(self.raw, k.as_ptr(), val) })
    }

    pub fn set_u64(&self, key: &str, val: u64) -> Result<()> {
        let k = ckey(key);
        check_ret(unsafe { sys::mpp_enc_cfg_set_u64(self.raw, k.as_ptr(), val) })
    }

    /// Set a raw pointer config value.
    ///
    /// # Safety
    ///
    /// The pointed-to data must remain valid until the config is applied
    /// to the encoder via `enc_set_cfg`.
    pub unsafe fn set_ptr(&self, key: &str, val: *mut c_void) -> Result<()> {
        let k = ckey(key);
        check_ret(sys::mpp_enc_cfg_set_ptr(self.raw, k.as_ptr(), val))
    }

    /// Set a struct config value.
    ///
    /// # Safety
    ///
    /// `val` must point to a valid struct of the type expected by `key`.
    pub unsafe fn set_st(&self, key: &str, val: *mut c_void) -> Result<()> {
        let k = ckey(key);
        check_ret(sys::mpp_enc_cfg_set_st(self.raw, k.as_ptr(), val))
    }

    pub fn get_s8(&self, key: &str) -> Result<i8> {
        let k = ckey(key);
        let mut val: i8 = 0;
        check_ret(unsafe { sys::mpp_enc_cfg_get_s8(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    pub fn get_u8(&self, key: &str) -> Result<u8> {
        let k = ckey(key);
        let mut val: u8 = 0;
        check_ret(unsafe { sys::mpp_enc_cfg_get_u8(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    pub fn get_s16(&self, key: &str) -> Result<i16> {
        let k = ckey(key);
        let mut val: i16 = 0;
        check_ret(unsafe { sys::mpp_enc_cfg_get_s16(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    pub fn get_u16(&self, key: &str) -> Result<u16> {
        let k = ckey(key);
        let mut val: u16 = 0;
        check_ret(unsafe { sys::mpp_enc_cfg_get_u16(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    pub fn get_s32(&self, key: &str) -> Result<i32> {
        let k = ckey(key);
        let mut val: i32 = 0;
        check_ret(unsafe { sys::mpp_enc_cfg_get_s32(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    pub fn get_u32(&self, key: &str) -> Result<u32> {
        let k = ckey(key);
        let mut val: u32 = 0;
        check_ret(unsafe { sys::mpp_enc_cfg_get_u32(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    pub fn get_s64(&self, key: &str) -> Result<i64> {
        let k = ckey(key);
        let mut val: i64 = 0;
        check_ret(unsafe { sys::mpp_enc_cfg_get_s64(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    pub fn get_u64(&self, key: &str) -> Result<u64> {
        let k = ckey(key);
        let mut val: u64 = 0;
        check_ret(unsafe { sys::mpp_enc_cfg_get_u64(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    /// Get a raw pointer config value.
    pub fn get_ptr(&self, key: &str) -> Result<*mut c_void> {
        let k = ckey(key);
        let mut val: *mut c_void = ptr::null_mut();
        check_ret(unsafe { sys::mpp_enc_cfg_get_ptr(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    /// Get a struct config value.
    ///
    /// # Safety
    ///
    /// `out` must point to a buffer of the correct type and size for `key`.
    pub unsafe fn get_st(&self, key: &str, out: *mut c_void) -> Result<()> {
        let k = ckey(key);
        check_ret(sys::mpp_enc_cfg_get_st(self.raw, k.as_ptr(), out))
    }

    // ---- Serialization ----

    /// Extract the config to a formatted string buffer.
    ///
    /// # Safety
    ///
    /// The returned pointer is allocated by MPP's internal allocator.
    /// The caller is responsible for freeing it (e.g., via `libc::free`).
    /// Returns `null` if extraction produces no output.
    pub unsafe fn extract_raw(&self, fmt: CfgStrFmt) -> Result<*mut std::os::raw::c_char> {
        let mut buf: *mut std::os::raw::c_char = ptr::null_mut();
        check_ret(sys::mpp_enc_cfg_extract(self.raw, fmt, &mut buf))?;
        Ok(buf)
    }

    /// Apply config from a formatted string.
    pub fn apply(&self, fmt: CfgStrFmt, data: &str) -> Result<()> {
        let cdata = CString::new(data).expect("config data must not contain interior NUL bytes");
        check_ret(unsafe { sys::mpp_enc_cfg_apply(self.raw, fmt, cdata.as_ptr() as *mut _) })
    }

    /// Dump all registered encoder config entries to the MPP log.
    pub fn show() {
        unsafe { sys::mpp_enc_cfg_show() }
    }

    /// Get the underlying raw `MppEncCfg` handle.
    pub fn as_raw(&self) -> sys::MppEncCfg {
        self.raw
    }
}

impl Drop for EncCfg {
    fn drop(&mut self) {
        unsafe {
            sys::mpp_enc_cfg_deinit(self.raw);
        }
    }
}

impl fmt::Debug for EncCfg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncCfg").field("raw", &self.raw).finish()
    }
}

// ============================================================
// DecCfg — Decoder configuration
// ============================================================

/// Owned handle to an MPP decoder configuration object.
pub struct DecCfg {
    raw: sys::MppDecCfg,
}

unsafe impl Send for DecCfg {}
unsafe impl Sync for DecCfg {}

impl DecCfg {
    /// Create a new decoder configuration.
    pub fn new() -> Result<Self> {
        let mut cfg: sys::MppDecCfg = ptr::null_mut();
        check_ret(unsafe { sys::mpp_dec_cfg_init(&mut cfg) })?;
        Ok(Self { raw: cfg })
    }

    // ---- Setters ----

    pub fn set_s32(&self, key: &str, val: i32) -> Result<()> {
        let k = ckey(key);
        check_ret(unsafe { sys::mpp_dec_cfg_set_s32(self.raw, k.as_ptr(), val) })
    }

    pub fn set_u32(&self, key: &str, val: u32) -> Result<()> {
        let k = ckey(key);
        check_ret(unsafe { sys::mpp_dec_cfg_set_u32(self.raw, k.as_ptr(), val) })
    }

    pub fn set_s64(&self, key: &str, val: i64) -> Result<()> {
        let k = ckey(key);
        check_ret(unsafe { sys::mpp_dec_cfg_set_s64(self.raw, k.as_ptr(), val) })
    }

    pub fn set_u64(&self, key: &str, val: u64) -> Result<()> {
        let k = ckey(key);
        check_ret(unsafe { sys::mpp_dec_cfg_set_u64(self.raw, k.as_ptr(), val) })
    }

    /// Set a raw pointer config value.
    ///
    /// # Safety
    ///
    /// The pointed-to data must remain valid until the config is applied.
    pub unsafe fn set_ptr(&self, key: &str, val: *mut c_void) -> Result<()> {
        let k = ckey(key);
        check_ret(sys::mpp_dec_cfg_set_ptr(self.raw, k.as_ptr(), val))
    }

    // ---- Getters ----

    pub fn get_s32(&self, key: &str) -> Result<i32> {
        let k = ckey(key);
        let mut val: i32 = 0;
        check_ret(unsafe { sys::mpp_dec_cfg_get_s32(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    pub fn get_u32(&self, key: &str) -> Result<u32> {
        let k = ckey(key);
        let mut val: u32 = 0;
        check_ret(unsafe { sys::mpp_dec_cfg_get_u32(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    pub fn get_s64(&self, key: &str) -> Result<i64> {
        let k = ckey(key);
        let mut val: i64 = 0;
        check_ret(unsafe { sys::mpp_dec_cfg_get_s64(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    pub fn get_u64(&self, key: &str) -> Result<u64> {
        let k = ckey(key);
        let mut val: u64 = 0;
        check_ret(unsafe { sys::mpp_dec_cfg_get_u64(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    /// Get a raw pointer config value.
    pub fn get_ptr(&self, key: &str) -> Result<*mut c_void> {
        let k = ckey(key);
        let mut val: *mut c_void = ptr::null_mut();
        check_ret(unsafe { sys::mpp_dec_cfg_get_ptr(self.raw, k.as_ptr(), &mut val) })?;
        Ok(val)
    }

    /// Dump all registered decoder config entries to the MPP log.
    pub fn show() {
        unsafe { sys::mpp_dec_cfg_show() }
    }

    /// Get the underlying raw `MppDecCfg` handle.
    pub fn as_raw(&self) -> sys::MppDecCfg {
        self.raw
    }
}

impl Drop for DecCfg {
    fn drop(&mut self) {
        unsafe {
            sys::mpp_dec_cfg_deinit(self.raw);
        }
    }
}

impl fmt::Debug for DecCfg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecCfg").field("raw", &self.raw).finish()
    }
}
