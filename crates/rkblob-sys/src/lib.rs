#![allow(nonstandard_style, dead_code)]
#![allow(unused_imports)]
#![allow(unnecessary_transmutes)]

#[cfg(feature = "mpp")]
pub mod mpp_sys;
#[cfg(feature = "rga")]
pub mod rga_sys;
#[cfg(feature = "rkcrypto")]
pub mod rkcrypto_sys;
#[cfg(feature = "rknn")]
pub mod rknn_sys;
