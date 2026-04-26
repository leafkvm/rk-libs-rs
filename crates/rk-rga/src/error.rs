#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgaError {
    pub inner: crate::ffi::IM_STATUS,
}

impl std::fmt::Display for RgaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = format!("{:?}", self.inner);
        let str = str.strip_prefix("IM_STATUS_").unwrap_or(&str);
        write!(f, "RgaError: {}", str)
    }
}

impl std::error::Error for RgaError {}

pub type RgaResult = Result<(), RgaError>;

pub fn rga_failed() -> RgaError {
    RgaError {
        inner: crate::ffi::IM_STATUS::IM_STATUS_FAILED,
    }
}
