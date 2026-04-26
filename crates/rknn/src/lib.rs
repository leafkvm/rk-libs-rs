pub(crate) use rkblob_sys::rknn_sys as sys;

use std::{
    borrow::Cow,
    ffi::{c_char, c_void, CStr},
    mem::{align_of, size_of, MaybeUninit},
    os::{fd::RawFd, raw::c_int},
    ptr::NonNull,
    sync::Arc,
};

use bitflags::bitflags;
use thiserror::Error;

pub use sys::{
    rknn_context, rknn_core_mask as CoreMask, rknn_custom_string as CustomString,
    rknn_init_extend as InitExtend, rknn_input as Input, rknn_input_output_num as InputOutputNum,
    rknn_input_range as InputRange, rknn_mem_size as MemSize, rknn_mem_sync_mode as MemorySyncMode,
    rknn_output as Output, rknn_output_extend as OutputExtend, rknn_perf_detail as PerfDetail,
    rknn_perf_run as PerfRun, rknn_query_cmd as QueryCommand, rknn_run_extend as RunExtend,
    rknn_sdk_version as SdkVersion, rknn_tensor_attr as TensorAttr,
    rknn_tensor_format as TensorFormat, rknn_tensor_mem as RawTensorMemory,
    rknn_tensor_mem_flags as TensorMemoryFlags, rknn_tensor_qnt_type as QuantizationType,
    rknn_tensor_type as TensorType,
};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct InitFlags: u32 {
        const PRIOR_MEDIUM = sys::RKNN_FLAG_PRIOR_MEDIUM;
        const PRIOR_LOW = sys::RKNN_FLAG_PRIOR_LOW;
        const ASYNC = sys::RKNN_FLAG_ASYNC_MASK;
        const COLLECT_PERF = sys::RKNN_FLAG_COLLECT_PERF_MASK;
        const MEM_ALLOC_OUTSIDE = sys::RKNN_FLAG_MEM_ALLOC_OUTSIDE;
        const SHARE_WEIGHT_MEM = sys::RKNN_FLAG_SHARE_WEIGHT_MEM;
        const FENCE_IN_OUTSIDE = sys::RKNN_FLAG_FENCE_IN_OUTSIDE;
        const FENCE_OUT_OUTSIDE = sys::RKNN_FLAG_FENCE_OUT_OUTSIDE;
        const COLLECT_MODEL_INFO_ONLY = sys::RKNN_FLAG_COLLECT_MODEL_INFO_ONLY;
        const INTERNAL_ALLOC_OUTSIDE = sys::RKNN_FLAG_INTERNAL_ALLOC_OUTSIDE;
        const EXECUTE_FALLBACK_PRIOR_DEVICE_GPU =
            sys::RKNN_FLAG_EXECUTE_FALLBACK_PRIOR_DEVICE_GPU;
        const ENABLE_SRAM = sys::RKNN_FLAG_ENABLE_SRAM;
        const SHARE_SRAM = sys::RKNN_FLAG_SHARE_SRAM;
        const DISABLE_PROC_HIGH_PRIORITY = sys::RKNN_FLAG_DISABLE_PROC_HIGH_PRIORITY;
        const DISABLE_FLUSH_INPUT_MEM_CACHE = sys::RKNN_FLAG_DISABLE_FLUSH_INPUT_MEM_CACHE;
        const DISABLE_FLUSH_OUTPUT_MEM_CACHE = sys::RKNN_FLAG_DISABLE_FLUSH_OUTPUT_MEM_CACHE;
        const MODEL_BUFFER_ZERO_COPY = sys::RKNN_FLAG_MODEL_BUFFER_ZERO_COPY;
    }
}

impl InitFlags {
    pub const PRIOR_HIGH: Self = Self::empty();
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct MemoryAllocFlags: u64 {
        const CACHEABLE = sys::_rknn_mem_alloc_flags::RKNN_FLAG_MEMORY_CACHEABLE as u64;
        const NON_CACHEABLE = sys::_rknn_mem_alloc_flags::RKNN_FLAG_MEMORY_NON_CACHEABLE as u64;
        const TRY_ALLOC_SRAM = sys::_rknn_mem_alloc_flags::RKNN_FLAG_MEMORY_TRY_ALLOC_SRAM as u64;
        const ALLOC_NO_CONTEXT = sys::RKNN_MEM_FLAG_ALLOC_NO_CONTEXT as u64;
    }
}

impl MemoryAllocFlags {
    pub const DEFAULT: Self = Self::empty();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[repr(i32)]
pub enum Error {
    #[error("rknn failure")]
    Fail = sys::RKNN_ERR_FAIL,
    #[error("rknn timeout")]
    Timeout = sys::RKNN_ERR_TIMEOUT,
    #[error("rknn device unavailable")]
    DeviceUnavailable = sys::RKNN_ERR_DEVICE_UNAVAILABLE,
    #[error("rknn allocation failed")]
    MallocFail = sys::RKNN_ERR_MALLOC_FAIL,
    #[error("rknn invalid parameter")]
    ParamInvalid = sys::RKNN_ERR_PARAM_INVALID,
    #[error("rknn invalid model")]
    ModelInvalid = sys::RKNN_ERR_MODEL_INVALID,
    #[error("rknn invalid context")]
    ContextInvalid = sys::RKNN_ERR_CTX_INVALID,
    #[error("rknn invalid input")]
    InputInvalid = sys::RKNN_ERR_INPUT_INVALID,
    #[error("rknn invalid output")]
    OutputInvalid = sys::RKNN_ERR_OUTPUT_INVALID,
    #[error("rknn device mismatch")]
    DeviceUnmatch = sys::RKNN_ERR_DEVICE_UNMATCH,
    #[error("rknn incompatible pre-compiled model")]
    IncompatiblePreCompileModel = sys::RKNN_ERR_INCOMPATILE_PRE_COMPILE_MODEL,
    #[error("rknn incompatible optimization level version")]
    IncompatibleOptimizationLevelVersion = sys::RKNN_ERR_INCOMPATILE_OPTIMIZATION_LEVEL_VERSION,
    #[error("rknn target platform mismatch")]
    TargetPlatformUnmatch = sys::RKNN_ERR_TARGET_PLATFORM_UNMATCH,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub const fn code(self) -> i32 {
        self as i32
    }
}

impl TryFrom<c_int> for Error {
    type Error = c_int;

    fn try_from(value: c_int) -> std::result::Result<Self, Self::Error> {
        match value {
            sys::RKNN_ERR_FAIL => Ok(Self::Fail),
            sys::RKNN_ERR_TIMEOUT => Ok(Self::Timeout),
            sys::RKNN_ERR_DEVICE_UNAVAILABLE => Ok(Self::DeviceUnavailable),
            sys::RKNN_ERR_MALLOC_FAIL => Ok(Self::MallocFail),
            sys::RKNN_ERR_PARAM_INVALID => Ok(Self::ParamInvalid),
            sys::RKNN_ERR_MODEL_INVALID => Ok(Self::ModelInvalid),
            sys::RKNN_ERR_CTX_INVALID => Ok(Self::ContextInvalid),
            sys::RKNN_ERR_INPUT_INVALID => Ok(Self::InputInvalid),
            sys::RKNN_ERR_OUTPUT_INVALID => Ok(Self::OutputInvalid),
            sys::RKNN_ERR_DEVICE_UNMATCH => Ok(Self::DeviceUnmatch),
            sys::RKNN_ERR_INCOMPATILE_PRE_COMPILE_MODEL => Ok(Self::IncompatiblePreCompileModel),
            sys::RKNN_ERR_INCOMPATILE_OPTIMIZATION_LEVEL_VERSION => {
                Ok(Self::IncompatibleOptimizationLevelVersion)
            }
            sys::RKNN_ERR_TARGET_PLATFORM_UNMATCH => Ok(Self::TargetPlatformUnmatch),
            other => Err(other),
        }
    }
}

#[derive(Debug)]
struct ContextInner {
    raw: sys::rknn_context,
}

impl Drop for ContextInner {
    fn drop(&mut self) {
        unsafe {
            sys::rknn_destroy(self.raw);
        }
    }
}

#[derive(Debug)]
pub struct Context {
    inner: Arc<ContextInner>,
}

impl Context {
    pub fn new(model_data: &[u8], flags: InitFlags) -> Result<Self> {
        Self::new_with_extend(model_data, flags, None)
    }

    pub fn new_with_extend(
        model_data: &[u8],
        flags: InitFlags,
        extend: Option<&mut InitExtend>,
    ) -> Result<Self> {
        let mut raw = 0;
        let size = u32_size("model data", model_data.len());

        check_ret(unsafe {
            sys::rknn_init(
                &mut raw,
                model_data.as_ptr() as *mut c_void,
                size,
                flags.bits(),
                extend.map_or(std::ptr::null_mut(), |value| value),
            )
        })?;

        Ok(Self {
            inner: Arc::new(ContextInner { raw }),
        })
    }

    pub fn raw(&self) -> sys::rknn_context {
        self.inner.raw
    }

    pub fn dup(&self) -> Result<Self> {
        let mut src = self.raw();
        let mut dst = 0;
        check_ret(unsafe { sys::rknn_dup_context(&mut src, &mut dst) })?;
        Ok(Self {
            inner: Arc::new(ContextInner { raw: dst }),
        })
    }

    pub fn sdk_version(&self) -> Result<SdkVersion> {
        self.query_struct(QueryCommand::RKNN_QUERY_SDK_VERSION)
    }

    pub fn custom_string_raw(&self) -> Result<CustomString> {
        self.query_struct(QueryCommand::RKNN_QUERY_CUSTOM_STRING)
    }

    pub fn custom_string(&self) -> Result<String> {
        let custom = self.custom_string_raw()?;
        Ok(cstr_from_char_array(&custom.string)
            .to_string_lossy()
            .into_owned())
    }

    pub fn input_output_num(&self) -> Result<InputOutputNum> {
        self.query_struct(QueryCommand::RKNN_QUERY_IN_OUT_NUM)
    }

    pub fn mem_size(&self) -> Result<MemSize> {
        self.query_struct(QueryCommand::RKNN_QUERY_MEM_SIZE)
    }

    pub fn tensor_attr(&self, cmd: QueryCommand, index: u32) -> Result<TensorAttr> {
        let count = self.tensor_attr_count(cmd)?;
        assert!(
            index < count,
            "tensor index {} out of range for {:?}; available tensors: {}",
            index,
            cmd,
            count
        );

        let mut attr = unsafe { std::mem::zeroed::<TensorAttr>() };
        attr.index = index;
        self.query_into(cmd, &mut attr)?;
        Ok(attr)
    }

    pub fn tensor_attrs(&self, cmd: QueryCommand) -> Result<Vec<TensorAttr>> {
        let count = self.tensor_attr_count(cmd)?;
        let mut attrs = Vec::with_capacity(count as usize);
        for index in 0..count {
            attrs.push(self.tensor_attr(cmd, index)?);
        }
        Ok(attrs)
    }

    pub fn inputs_set(&self, inputs: &mut [Input]) -> Result<()> {
        let count = u32_size("input count", inputs.len());
        check_ret(unsafe { sys::rknn_inputs_set(self.raw(), count, inputs.as_mut_ptr()) })
    }

    pub fn set_batch_core_num(&self, core_num: i32) -> Result<()> {
        check_ret(unsafe { sys::rknn_set_batch_core_num(self.raw(), core_num) })
    }

    pub fn set_core_mask(&self, core_mask: CoreMask) -> Result<()> {
        check_ret(unsafe { sys::rknn_set_core_mask(self.raw(), core_mask) })
    }

    pub fn run(&self) -> Result<()> {
        check_ret(unsafe { sys::rknn_run(self.raw(), std::ptr::null_mut()) })
    }

    pub fn run_with(&self, extend: &mut RunExtend) -> Result<()> {
        check_ret(unsafe { sys::rknn_run(self.raw(), extend) })
    }

    pub fn wait_with(&self, extend: &mut RunExtend) -> Result<()> {
        check_ret(unsafe { sys::rknn_wait(self.raw(), extend) })
    }

    pub fn set_io_mem(&self, mem: &TensorMemory, attr: &mut TensorAttr) -> Result<()> {
        check_ret(unsafe { sys::rknn_set_io_mem(self.raw(), mem.as_ptr(), attr) })
    }

    pub fn set_input_shape(&self, attr: &mut TensorAttr) -> Result<()> {
        check_ret(unsafe { sys::rknn_set_input_shape(self.raw(), attr) })
    }

    pub fn set_input_shapes(&self, attrs: &mut [TensorAttr]) -> Result<()> {
        let count = u32_size("input shape count", attrs.len());
        check_ret(unsafe { sys::rknn_set_input_shapes(self.raw(), count, attrs.as_mut_ptr()) })
    }

    pub fn create_mem(&self, size: usize) -> TensorMemory {
        let size = u32_size("tensor memory", size);
        let ptr = unsafe { sys::rknn_create_mem(self.raw(), size) };
        TensorMemory::from_raw(self.inner.clone(), ptr)
    }

    pub fn create_mem2(&self, size: usize, flags: MemoryAllocFlags) -> TensorMemory {
        let ptr = unsafe { sys::rknn_create_mem2(self.raw(), size as u64, flags.bits()) };
        TensorMemory::from_raw(self.inner.clone(), ptr)
    }

    pub unsafe fn create_mem_from_fd(
        &self,
        fd: RawFd,
        virt_addr: *mut c_void,
        size: usize,
        offset: i32,
    ) -> TensorMemory {
        let size = u32_size("tensor memory", size);
        let ptr = unsafe { sys::rknn_create_mem_from_fd(self.raw(), fd, virt_addr, size, offset) };
        TensorMemory::from_raw(self.inner.clone(), ptr)
    }

    pub unsafe fn create_mem_from_phys(
        &self,
        phys_addr: u64,
        virt_addr: *mut c_void,
        size: usize,
    ) -> TensorMemory {
        let size = u32_size("tensor memory", size);
        let ptr = unsafe { sys::rknn_create_mem_from_phys(self.raw(), phys_addr, virt_addr, size) };
        TensorMemory::from_raw(self.inner.clone(), ptr)
    }

    fn query_into<T>(&self, cmd: QueryCommand, value: &mut T) -> Result<()> {
        let size = u32_size("query buffer", size_of::<T>());
        check_ret(unsafe { sys::rknn_query(self.raw(), cmd, value as *mut T as *mut c_void, size) })
    }

    fn query_struct<T>(&self, cmd: QueryCommand) -> Result<T> {
        let mut value = unsafe { std::mem::zeroed::<T>() };
        self.query_into(cmd, &mut value)?;
        Ok(value)
    }

    fn tensor_attr_count(&self, cmd: QueryCommand) -> Result<u32> {
        let io_num = self.input_output_num()?;
        match cmd {
            QueryCommand::RKNN_QUERY_INPUT_ATTR
            | QueryCommand::RKNN_QUERY_NATIVE_INPUT_ATTR
            | QueryCommand::RKNN_QUERY_NATIVE_NHWC_INPUT_ATTR
            | QueryCommand::RKNN_QUERY_CURRENT_INPUT_ATTR
            | QueryCommand::RKNN_QUERY_CURRENT_NATIVE_INPUT_ATTR => Ok(io_num.n_input),
            QueryCommand::RKNN_QUERY_OUTPUT_ATTR
            | QueryCommand::RKNN_QUERY_NATIVE_OUTPUT_ATTR
            | QueryCommand::RKNN_QUERY_NATIVE_NHWC_OUTPUT_ATTR
            | QueryCommand::RKNN_QUERY_CURRENT_OUTPUT_ATTR
            | QueryCommand::RKNN_QUERY_CURRENT_NATIVE_OUTPUT_ATTR => Ok(io_num.n_output),
            _ => panic!("query command {:?} does not return tensor attributes", cmd),
        }
    }
}

#[derive(Debug)]
pub struct TensorMemory {
    ctx: Arc<ContextInner>,
    raw: NonNull<RawTensorMemory>,
}

impl TensorMemory {
    fn from_raw(ctx: Arc<ContextInner>, ptr: *mut RawTensorMemory) -> Self {
        let raw = NonNull::new(ptr).expect("rknn tensor memory allocation returned a null pointer");
        Self { ctx, raw }
    }

    pub fn as_ptr(&self) -> *mut RawTensorMemory {
        self.raw.as_ptr()
    }

    pub fn raw(&self) -> &RawTensorMemory {
        unsafe { self.raw.as_ref() }
    }

    pub fn raw_mut(&mut self) -> &mut RawTensorMemory {
        unsafe { self.raw.as_mut() }
    }

    pub fn virt_addr(&self) -> *mut c_void {
        self.raw().virt_addr
    }

    pub fn phys_addr(&self) -> u64 {
        self.raw().phys_addr
    }

    pub fn fd(&self) -> i32 {
        self.raw().fd
    }

    pub fn offset(&self) -> i32 {
        self.raw().offset
    }

    pub fn size(&self) -> usize {
        self.raw().size as usize
    }

    pub fn flags(&self) -> u32 {
        self.raw().flags
    }

    pub fn sync(&self, mode: MemorySyncMode) -> Result<()> {
        check_ret(unsafe { sys::rknn_mem_sync(self.ctx.raw, self.as_ptr(), mode) })
    }

    pub fn as_uninit_slice<T>(&self) -> &[MaybeUninit<T>] {
        let (ptr, len) = self.typed_view_parts::<T>();
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }

    pub fn as_uninit_slice_mut<T>(&mut self) -> &mut [MaybeUninit<T>] {
        let (ptr, len) = self.typed_view_parts::<T>();
        unsafe { std::slice::from_raw_parts_mut(ptr, len) }
    }

    /// # Safety
    ///
    /// The caller must ensure the tensor memory is initialized as `T` values
    /// for the entire returned slice.
    pub unsafe fn as_slice<T>(&self) -> &[T] {
        let (ptr, len) = self.typed_view_parts::<T>();
        unsafe { std::slice::from_raw_parts(ptr.cast::<T>(), len) }
    }

    /// # Safety
    ///
    /// The caller must ensure the tensor memory can be treated as initialized
    /// `T` values for the entire returned slice.
    pub unsafe fn as_slice_mut<T>(&mut self) -> &mut [T] {
        let (ptr, len) = self.typed_view_parts::<T>();
        unsafe { std::slice::from_raw_parts_mut(ptr.cast::<T>(), len) }
    }

    fn typed_view_parts<T>(&self) -> (*mut MaybeUninit<T>, usize) {
        let element_size = size_of::<T>();
        assert_ne!(
            element_size, 0,
            "cannot create a typed slice for a zero-sized type"
        );

        let size = self.size();
        assert_eq!(
            size % element_size,
            0,
            "tensor memory size {} is not a multiple of element size {}",
            size,
            element_size
        );

        let ptr = self.virt_addr().cast::<MaybeUninit<T>>();
        assert!(!ptr.is_null(), "tensor memory has a null virtual address");

        let alignment = align_of::<T>();
        let address = ptr as usize;
        assert_eq!(
            address % alignment,
            0,
            "tensor memory address 0x{:x} is not aligned to {}",
            address,
            alignment
        );

        (ptr, size / element_size)
    }
}

impl Drop for TensorMemory {
    fn drop(&mut self) {
        unsafe {
            sys::rknn_destroy_mem(self.ctx.raw, self.as_ptr());
        }
    }
}

pub trait TensorAttrExt {
    fn dims(&self) -> &[u32];
    fn name_cstr(&self) -> &CStr;
    fn name_lossy(&self) -> Cow<'_, str>;
}

impl TensorAttrExt for TensorAttr {
    fn dims(&self) -> &[u32] {
        &self.dims[..self.n_dims as usize]
    }

    fn name_cstr(&self) -> &CStr {
        cstr_from_char_array(&self.name)
    }

    fn name_lossy(&self) -> Cow<'_, str> {
        self.name_cstr().to_string_lossy()
    }
}

pub trait SdkVersionExt {
    fn api_version_cstr(&self) -> &CStr;
    fn api_version_lossy(&self) -> Cow<'_, str>;
    fn driver_version_cstr(&self) -> &CStr;
    fn driver_version_lossy(&self) -> Cow<'_, str>;
}

impl SdkVersionExt for SdkVersion {
    fn api_version_cstr(&self) -> &CStr {
        cstr_from_char_array(&self.api_version)
    }

    fn api_version_lossy(&self) -> Cow<'_, str> {
        self.api_version_cstr().to_string_lossy()
    }

    fn driver_version_cstr(&self) -> &CStr {
        cstr_from_char_array(&self.drv_version)
    }

    fn driver_version_lossy(&self) -> Cow<'_, str> {
        self.driver_version_cstr().to_string_lossy()
    }
}

fn check_ret(ret: c_int) -> Result<()> {
    if ret < 0 {
        Err(Error::try_from(ret)
            .unwrap_or_else(|code| panic!("unexpected rknn api error code {}", code)))
    } else {
        Ok(())
    }
}

fn u32_size(what: &'static str, size: usize) -> u32 {
    u32::try_from(size)
        .unwrap_or_else(|_| panic!("size {} exceeds the RKNN API limit for {}", size, what))
}

fn cstr_from_char_array(chars: &[c_char]) -> &CStr {
    let bytes = unsafe { std::slice::from_raw_parts(chars.as_ptr().cast::<u8>(), chars.len()) };
    CStr::from_bytes_until_nul(bytes)
        .unwrap_or_else(|_| CStr::from_bytes_with_nul(b"\0").expect("valid empty c string"))
}
