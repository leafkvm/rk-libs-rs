use std::{
    ops::{Deref, DerefMut},
    pin::Pin,
    ptr::NonNull,
};

use int_enum::IntEnum;

pub use rkblob_sys::rkcrypto_sys as ffi;

fn ensure_init() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        ffi::rk_crypto_init();
    });
}

pub struct CryptoMem {
    mem: NonNull<ffi::rk_crypto_mem>,
}

impl CryptoMem {
    pub fn new(size: usize) -> Self {
        ensure_init();
        unsafe {
            let mem = ffi::rk_crypto_mem_alloc(size);
            Self {
                mem: NonNull::new(mem).unwrap(),
            }
        }
    }

    pub fn size(&self) -> usize {
        unsafe { self.mem.as_ref().size }
    }
}

impl Deref for CryptoMem {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.mem.as_ref().vaddr as *const u8, self.size()) }
    }
}

impl DerefMut for CryptoMem {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { std::slice::from_raw_parts_mut(self.mem.as_ref().vaddr as *mut u8, self.size()) }
    }
}

impl Drop for CryptoMem {
    fn drop(&mut self) {
        unsafe {
            ffi::rk_crypto_mem_free(self.mem.as_ptr());
        }
    }
}

#[repr(u32)]
#[derive(IntEnum, Debug, Clone, Copy, thiserror::Error)]
pub enum RkCryptoErr {
    #[error("Generic Error")]
    Generic = ffi::RK_CRYPTO_ERR_GENERIC,
    #[error("Parameter Error")]
    Parameter = ffi::RK_CRYPTO_ERR_PARAMETER,
    #[error("State Error")]
    State = ffi::RK_CRYPTO_ERR_STATE,
    #[error("Not Supported Error")]
    NotSupported = ffi::RK_CRYPTO_ERR_NOT_SUPPORTED,
    #[error("OOM Error")]
    OutOfMemory = ffi::RK_CRYPTO_ERR_OUT_OF_MEMORY,
    #[error("Access Denied Error")]
    AccessDenied = ffi::RK_CRYPTO_ERR_ACCESS_DENIED,
    #[error("Busy Error")]
    Busy = ffi::RK_CRYPTO_ERR_BUSY,
    #[error("Timeout Error")]
    Timeout = ffi::RK_CRYPTO_ERR_TIMEOUT,
    #[error("Uninited Error")]
    Uninited = ffi::RK_CRYPTO_ERR_UNINITED,
    #[error("Key Error")]
    Key = ffi::RK_CRYPTO_ERR_KEY,
    #[error("Verify Error")]
    Verify = ffi::RK_CRYPTO_ERR_VERIFY,
    #[error("Padding Error")]
    Padding = ffi::RK_CRYPTO_ERR_PADDING,
    #[error("Padding Overflow Error")]
    PaddingOverflow = ffi::RK_CRYPTO_ERR_PADDING_OVERFLOW,
    #[error("Mac Invalid Error")]
    MacInvalid = ffi::RK_CRYPTO_ERR_MAC_INVALID,
}

pub fn rk_err_chk(err: ffi::RK_RES) -> Result<(), RkCryptoErr> {
    if err == ffi::RK_CRYPTO_SUCCESS {
        Ok(())
    } else {
        Err(RkCryptoErr::try_from(err).unwrap_or(RkCryptoErr::Generic))
    }
}

#[repr(u32)]
#[derive(IntEnum, Debug, Clone, Copy)]
pub enum OtpKeyId {
    Key0 = ffi::RK_OEM_OTP_KEYID_RK_OEM_OTP_KEY0,
    Key1 = ffi::RK_OEM_OTP_KEYID_RK_OEM_OTP_KEY1,
    Key2 = ffi::RK_OEM_OTP_KEYID_RK_OEM_OTP_KEY2,
    Key3 = ffi::RK_OEM_OTP_KEYID_RK_OEM_OTP_KEY3,
    KeyFw = ffi::RK_OEM_OTP_KEYID_RK_OEM_OTP_KEY_FW,
}

/// Write an OTP key to the specified OEM OTP area.
pub fn write_otp_key(key_id: OtpKeyId, key: &[u8]) -> Result<(), RkCryptoErr> {
    ensure_init();
    unsafe {
        rk_err_chk(ffi::rk_write_oem_otp_key(
            ffi::RK_OEM_OTP_KEYID::from(key_id),
            key.as_ptr() as *mut u8,
            key.len() as u32,
        ))
    }
}

/// Check if an OTP key is already written in the specified OEM OTP area.
pub fn otp_key_is_written(key_id: OtpKeyId) -> Result<bool, RkCryptoErr> {
    ensure_init();
    let mut is_written: u8 = 0;
    unsafe {
        rk_err_chk(ffi::rk_oem_otp_key_is_written(
            ffi::RK_OEM_OTP_KEYID::from(key_id),
            &mut is_written as *mut u8,
        ))?;
    }
    Ok(is_written != 0)
}

pub fn set_otp_read_lock(key_id: OtpKeyId) -> Result<(), RkCryptoErr> {
    ensure_init();
    unsafe {
        rk_err_chk(ffi::rk_set_oem_hr_otp_read_lock(
            ffi::RK_OEM_OTP_KEYID::from(key_id),
        ))
    }
}

pub struct RkCipherConfig {
    config: ffi::rk_cipher_config,
}

impl RkCipherConfig {
    pub fn new_aes_256_ecb(encrypt: bool, key: [u8; 32]) -> Self {
        let mut key_s = [0u8; 64];
        key_s[..32].copy_from_slice(&key);
        Self {
            config: ffi::rk_cipher_config {
                algo: ffi::RK_CRYPTO_ALGO_RK_ALGO_AES,
                mode: ffi::RK_CIPIHER_MODE_RK_CIPHER_MODE_ECB,
                operation: if encrypt {
                    ffi::RK_OP_CIPHER_ENC
                } else {
                    ffi::RK_OP_CIPHER_DEC
                },
                key: key_s,
                key_len: key.len() as u32,
                iv: [0u8; 16],
                reserved: std::ptr::null_mut(),
            },
        }
    }
}

pub fn otp_key_cipher_virt(
    key_id: OtpKeyId,
    config: &mut RkCipherConfig,
    src: &[u8],
    dst: &mut [u8],
) -> Result<(), RkCryptoErr> {
    if src.len() != dst.len() {
        return Err(RkCryptoErr::Parameter);
    }
    ensure_init();
    unsafe {
        rk_err_chk(ffi::rk_oem_otp_key_cipher_virt(
            ffi::RK_OEM_OTP_KEYID::from(key_id),
            &mut config.config as *mut ffi::rk_cipher_config,
            src.as_ptr() as *mut u8,
            dst.as_mut_ptr() as *mut u8,
            src.len() as u32,
        ))
    }
}

pub struct CipherHandle {
    config: Pin<Box<RkCipherConfig>>,
    handle: ffi::rk_handle,
}

impl CipherHandle {
    pub fn new(config: RkCipherConfig) -> Result<Self, RkCryptoErr> {
        ensure_init();
        let mut res = Self {
            config: Box::pin(config),
            handle: 0,
        };
        unsafe {
            rk_err_chk(ffi::rk_cipher_init(
                &res.config.as_ref().config,
                &mut res.handle,
            ))?
        };
        Ok(res)
    }
}

impl Drop for CipherHandle {
    fn drop(&mut self) {
        if self.handle != 0 {
            unsafe {
                ffi::rk_cipher_final(self.handle);
            }
        }
    }
}

pub fn key_cipher_virt(
    handle: &mut CipherHandle,
    src: &[u8],
    dst: &mut [u8],
) -> Result<(), RkCryptoErr> {
    if src.len() != dst.len() {
        return Err(RkCryptoErr::Parameter);
    }

    ensure_init();
    unsafe {
        rk_err_chk(ffi::rk_cipher_crypt_virt(
            handle.handle,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as u32,
        ))?
    };

    Ok(())
}
