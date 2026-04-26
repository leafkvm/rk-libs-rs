include!(concat!(env!("OUT_DIR"), "/ffi_librga.rs"));

use std::ffi::c_int;

unsafe extern "C" {
    pub fn _Z15importbuffer_fdii(fd: c_int, size: c_int) -> rga_buffer_handle_t;
    pub fn _Z15importbuffer_fdiiii(
        fd: c_int,
        width: c_int,
        height: c_int,
        format: c_int,
    ) -> rga_buffer_handle_t;
}
