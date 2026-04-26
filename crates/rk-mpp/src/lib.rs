pub mod buffer;
pub mod cfg;
pub mod frame;
pub mod mpi;
pub mod packet;

pub(crate) use rkblob_sys::mpp_sys as sys;

use std::fmt;

// ============================================================
// Caller/tag constants for MPP internal logging
// ============================================================

pub(crate) const CALLER: *const std::ffi::c_char = c"rk_mpp_rs".as_ptr();
pub(crate) const TAG: *const std::ffi::c_char = std::ptr::null();

// ============================================================
// Error type
// ============================================================

/// Error type wrapping an MPP return code.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Error(pub sys::MPP_RET);

impl Error {
    /// Get the raw MPP return code.
    pub fn code(self) -> i32 {
        self.0 as i32
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MppError({:?})", self.0)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for Error {}

/// Result type alias for MPP operations.
pub type Result<T> = std::result::Result<T, Error>;

#[inline]
pub(crate) fn check_ret(ret: sys::MPP_RET) -> Result<()> {
    if ret == sys::MPP_RET::MPP_OK {
        Ok(())
    } else {
        Err(Error(ret))
    }
}

pub use sys::MppFrameFormat as FrameFormat;
