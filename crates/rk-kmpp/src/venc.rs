//! Non-blocking encoder channel on `/dev/vcodec`.
//!
//! One [`Venc`] is one kernel encoder channel (H.264 / H.265 / JPEG on
//! VEPU511). The data path:
//!
//! * input — [`Venc::put_frame`] hands the kernel a dma-buf fd plus frame
//!   geometry through a shared `KmppFrame` object. The ioctl queues the
//!   frame for the kernel's SCHED_FIFO encode worker and returns
//!   immediately; the kernel bounds the queue at 2 frames and rejects
//!   ([`Error::QueueFull`]) beyond that.
//! * readiness — the channel fd polls readable (`POLLIN`) whenever at least
//!   one encoded packet is queued. Use [`Venc::as_fd`] with `poll(2)` /
//!   `epoll` / tokio `AsyncFd`.
//! * output — [`Venc::get_packet`] dequeues one `KmppPacket`. The bitstream
//!   already sits in a kernel-allocated ring that the kernel double-maps
//!   into this process, so [`Packet::data`] is zero-copy. Dropping the
//!   [`Packet`] recycles its ring space — hold it only as long as needed.
//!
//! This crate expects this project's patched `kmpp.ko`, whose vcodec driver
//! returns distinct errnos: `EAGAIN` for backpressure (input queue full /
//! output queue empty), `EFAULT` for bad user pointers, `EINVAL` for bad
//! state or rejected controls, `ENODEV` for channel creation failure. The
//! stock Rockchip driver collapses every failure to `EFAULT`, which this
//! crate would report as [`Error::Sys`].

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Mutex;

use crate::abi::{
    KmppShmPtr, VcodecMsg, MPP_ENC_GET_CFG, MPP_ENC_SET_CFG, MPP_ENC_SET_HEADER_MODE,
    MPP_ENC_SET_IDR_FRAME, VCODEC_CHAN_CONTROL, VCODEC_CHAN_CREATE, VCODEC_CHAN_DESTROY,
    VCODEC_CHAN_IN_FRM_RDY, VCODEC_CHAN_OUT_PKT_RDY, VCODEC_CHAN_PAUSE, VCODEC_CHAN_RESUME,
    VCODEC_CHAN_START, VCODEC_CHAN_STOP, VOCDEC_IOC_CFG,
};
use crate::types::{packet_flag, Coding, FrameFormat, HeaderMode, MPP_CTX_ENC};
use crate::{Error, Kmpp, Obj, Result};

/// Number of `KmppFrame` objects kept in rotation. The kernel queues at most
/// 2 frames (`MAX_FRAME_NUM`) and reads the object's scalar fields shortly
/// after dequeuing, so 4 objects guarantee a slot is never rewritten while
/// the kernel may still read it. (The vendor client reuses a *single*
/// object and relies on timing; the pool removes that race.)
const FRAME_POOL: usize = 4;

/// Channel creation parameters (`KmppVencInitCfg`). These are fixed for the
/// channel's lifetime; everything else is runtime config via [`EncCfg`].
#[derive(Debug, Clone)]
pub struct VencInit {
    pub coding: Coding,
    /// Maximum input dimensions; sizes kernel-side buffers.
    pub max_width: u32,
    pub max_height: u32,
    /// Requested channel id (0..31). If busy, the kernel picks another free
    /// one; the actual id is available as [`Venc::chan_id`] after creation.
    pub chan_id: i32,
    /// Output stream ring size in bytes; 0 = kernel default.
    pub buf_size: u32,
    /// Max packets buffered kernel-side; 0 = kernel default.
    pub max_strm_cnt: u32,
    /// Use the "smart" rate control from `kmpp_smart.ko` (falls back to the
    /// built-in RC with a kernel log line when the module is absent).
    pub smart_en: bool,
    pub max_lt_cnt: u32,
    pub qpmap_en: bool,
    pub deblur_en: bool,
    /// `tune:scene_mode` seed; 0 = default.
    pub scene_mode: i32,
}

impl VencInit {
    pub fn new(coding: Coding, max_width: u32, max_height: u32) -> Self {
        VencInit {
            coding,
            max_width,
            max_height,
            chan_id: 0,
            buf_size: 0,
            max_strm_cnt: 0,
            smart_en: false,
            max_lt_cnt: 0,
            qpmap_en: false,
            deblur_en: false,
            scene_mode: 0,
        }
    }
}

/// Geometry and content of one input frame. The pixel data lives in a
/// dma-buf; the kernel imports the fd on submit.
#[derive(Debug, Clone)]
pub struct FrameDesc {
    pub width: u32,
    pub height: u32,
    /// Line stride in **bytes**.
    pub hor_stride: u32,
    /// Vertical stride in lines (chroma plane offset = `hor_stride * ver_stride`).
    pub ver_stride: u32,
    /// Line stride in pixels; 0 = unset (derived from `hor_stride`).
    pub hor_stride_pixel: u32,
    pub offset_x: u32,
    pub offset_y: u32,
    pub fmt: FrameFormat,
    /// dma-buf fd of the pixel data. Must stay valid until the frame has
    /// been encoded (the kernel imports the buffer, so closing the fd after
    /// a successful `put_frame` is allowed; *reusing* the buffer memory is
    /// only safe once the corresponding packet came back).
    pub buf_fd: RawFd,
    pub pts: i64,
    pub dts: i64,
}

impl FrameDesc {
    pub fn new(width: u32, height: u32, hor_stride: u32, ver_stride: u32, fmt: FrameFormat, buf_fd: RawFd) -> Self {
        FrameDesc {
            width,
            height,
            hor_stride,
            ver_stride,
            hor_stride_pixel: 0,
            offset_x: 0,
            offset_y: 0,
            fmt,
            buf_fd,
            pts: 0,
            dts: 0,
        }
    }
}

/// Encoder static config — a shared `KmppVencStCfg` object addressed with
/// the same hierarchical string keys as MPP's `mpp_enc_cfg` API
/// (`"rc:mode"`, `"prep:width"`, `"h264:profile"`, `"jpeg:q_factor"`, ...).
///
/// Set only what you want to change: every write flags the field, and
/// [`Venc::set_cfg`] applies exactly the flagged subset — partial updates
/// (e.g. only `rc:bps_*` on a bitrate change) are the supported mechanism.
/// Writing a field's existing value still counts as setting it, so an
/// explicit `0` on a fresh (all-zero) config reaches the encoder rather than
/// leaving its default in place; `rc:gop = 0` ("infinite gop", against a
/// default of 60) depends on that.
#[derive(Debug)]
pub struct EncCfg(Obj);

impl std::ops::Deref for EncCfg {
    type Target = Obj;
    fn deref(&self) -> &Obj {
        &self.0
    }
}

impl EncCfg {
    /// Allocate a fresh (all-unset) config object.
    pub fn new(kmpp: &Kmpp) -> Result<EncCfg> {
        Ok(EncCfg(kmpp.alloc("KmppVencStCfg")?))
    }
}

struct FramePool {
    frames: Vec<Obj>,
    next: usize,
}

/// One non-blocking encoder channel. See the module docs for the model.
pub struct Venc {
    kmpp: Kmpp,
    fd: OwnedFd,
    chan_id: i32,
    /// Kept alive for the channel's lifetime — the kernel holds a pointer
    /// to this object from `mpp_enc_init`.
    _init_cfg: Obj,
    pool: Mutex<FramePool>,
}

impl Venc {
    /// Create and start an encoder channel.
    pub fn create(kmpp: &Kmpp, init: &VencInit) -> Result<Venc> {
        let fd = unsafe {
            let raw = libc::open(c"/dev/vcodec".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
            if raw < 0 {
                return Err(Error::sys("open /dev/vcodec"));
            }
            OwnedFd::from_raw_fd(raw)
        };

        let cfg = kmpp.alloc("KmppVencInitCfg")?;
        cfg.set_u32("type", MPP_CTX_ENC)?;
        cfg.set_u32("coding", init.coding as u32)?;
        cfg.set_s32("chan_id", init.chan_id)?;
        cfg.set_s32("chan_fd", fd.as_raw_fd())?;
        cfg.set_s32("online", 0)?;
        // Non-blocking submit; the queue-full case surfaces as an ioctl
        // error instead of sleeping in the kernel.
        cfg.set_s32("input_timeout", 0)?;
        cfg.set_s32("output_timeout", 0)?;
        cfg.set_u32("buf_size", init.buf_size)?;
        cfg.set_u32("max_strm_cnt", init.max_strm_cnt)?;
        cfg.set_u32("shared_buf_en", 0)?;
        cfg.set_u32("smart_en", init.smart_en as u32)?;
        cfg.set_u32("max_width", init.max_width)?;
        cfg.set_u32("max_height", init.max_height)?;
        cfg.set_u32("max_lt_cnt", init.max_lt_cnt)?;
        cfg.set_u32("deblur_en", init.deblur_en as u32)?;
        cfg.set_u32("qpmap_en", init.qpmap_en as u32)?;
        cfg.set_u32("chan_dup", 0)?;
        cfg.set_u32("tmvp_enable", 0)?;
        cfg.set_u32("only_smartp", 0)?;
        cfg.set_s32("scene_mode", init.scene_mode)?;

        vcodec_ioctl(&fd, VCODEC_CHAN_CREATE, 0, cfg.shm_ptr())
            .map_err(|e| Error::Sys { op: "VCODEC_CHAN_CREATE", source: e })?;

        // The kernel writes the actually-assigned channel id back into the
        // shared init config.
        let chan_id = cfg.get_s32("chan_id")?;

        vcodec_ioctl_nodata(&fd, VCODEC_CHAN_START, 0)
            .map_err(|e| Error::Sys { op: "VCODEC_CHAN_START", source: e })?;

        let mut frames = Vec::with_capacity(FRAME_POOL);
        for _ in 0..FRAME_POOL {
            frames.push(kmpp.alloc("KmppFrame")?);
        }

        log::info!("kmpp venc chan {chan_id} created ({:?})", init.coding);

        Ok(Venc {
            kmpp: kmpp.clone(),
            fd,
            chan_id,
            _init_cfg: cfg,
            pool: Mutex::new(FramePool { frames, next: 0 }),
        })
    }

    pub fn kmpp(&self) -> &Kmpp {
        &self.kmpp
    }

    /// The channel id the kernel actually assigned.
    pub fn chan_id(&self) -> i32 {
        self.chan_id
    }

    /// Allocate a fresh config object for this channel's [`Venc::set_cfg`].
    pub fn new_cfg(&self) -> Result<EncCfg> {
        EncCfg::new(&self.kmpp)
    }

    /// Apply the flagged fields of `cfg` (`MPP_ENC_SET_CFG`).
    pub fn set_cfg(&self, cfg: &EncCfg) -> Result<()> {
        self.control_obj(MPP_ENC_SET_CFG, &cfg.0)
    }

    /// Fill `cfg` with the channel's current config (`MPP_ENC_GET_CFG`).
    pub fn get_cfg(&self, cfg: &EncCfg) -> Result<()> {
        self.control_obj(MPP_ENC_GET_CFG, &cfg.0)
    }

    /// Encode the next frame as an IDR frame.
    pub fn request_idr(&self) -> Result<()> {
        vcodec_ioctl_nodata(&self.fd, VCODEC_CHAN_CONTROL, MPP_ENC_SET_IDR_FRAME)
            .map_err(|e| Error::Sys { op: "MPP_ENC_SET_IDR_FRAME", source: e })
    }

    /// Choose whether SPS/PPS headers are prepended to every IDR packet.
    pub fn set_header_mode(&self, mode: HeaderMode) -> Result<()> {
        let val: u32 = mode as u32;
        self.control_raw(
            MPP_ENC_SET_HEADER_MODE,
            &val as *const u32 as *const libc::c_void,
            4,
        )
    }

    pub fn pause(&self) -> Result<()> {
        vcodec_ioctl_nodata(&self.fd, VCODEC_CHAN_PAUSE, 0).map_err(|e| Error::Sys { op: "VCODEC_CHAN_PAUSE", source: e })
    }

    pub fn resume(&self) -> Result<()> {
        vcodec_ioctl_nodata(&self.fd, VCODEC_CHAN_RESUME, 0).map_err(|e| Error::Sys { op: "VCODEC_CHAN_RESUME", source: e })
    }

    pub fn stop(&self) -> Result<()> {
        vcodec_ioctl_nodata(&self.fd, VCODEC_CHAN_STOP, 0).map_err(|e| Error::Sys { op: "VCODEC_CHAN_STOP", source: e })
    }

    pub fn start(&self) -> Result<()> {
        vcodec_ioctl_nodata(&self.fd, VCODEC_CHAN_START, 0).map_err(|e| Error::Sys { op: "VCODEC_CHAN_START", source: e })
    }

    /// Submit one frame for encoding. Never blocks; returns
    /// [`Error::QueueFull`] when the kernel's 2-deep input queue is full
    /// (`EAGAIN` — drain packets, or drop the frame and move on). Any other
    /// errno is a genuine fault and surfaces as [`Error::Sys`].
    pub fn put_frame(&self, desc: &FrameDesc) -> Result<()> {
        let mut pool = self.pool.lock().unwrap();
        let idx = pool.next;
        let frame = &pool.frames[idx];

        frame.set_u32("width", desc.width)?;
        frame.set_u32("height", desc.height)?;
        frame.set_u32("hor_stride", desc.hor_stride)?;
        frame.set_u32("ver_stride", desc.ver_stride)?;
        frame.set_u32("hor_stride_pixel", desc.hor_stride_pixel)?;
        frame.set_u32("offset_x", desc.offset_x)?;
        frame.set_u32("offset_y", desc.offset_y)?;
        frame.set_u32("fmt", desc.fmt as u32)?;
        frame.set_u32("eos", 0)?;
        frame.set_s64("pts", desc.pts)?;
        frame.set_s64("dts", desc.dts)?;
        frame.set_u32("buf_fd", desc.buf_fd as u32)?;

        vcodec_ioctl(&self.fd, VCODEC_CHAN_IN_FRM_RDY, 0, frame.shm_ptr())
            .map_err(frm_rdy_error)?;

        pool.next = (idx + 1) % pool.frames.len();
        Ok(())
    }

    /// Signal end of stream. The kernel emits a final empty packet with the
    /// EOS flag set.
    pub fn put_eos(&self, pts: i64) -> Result<()> {
        // A fresh object: reusing a pooled frame here would hand the kernel
        // a stale `buffer` shm pointer from an earlier submit (the no-fd
        // path dereferences it).
        let frame = self.kmpp.alloc("KmppFrame")?;
        frame.set_u32("eos", 1)?;
        frame.set_s64("pts", pts)?;
        frame.set_s64("dts", pts)?;
        vcodec_ioctl(&self.fd, VCODEC_CHAN_IN_FRM_RDY, 0, frame.shm_ptr())
            .map_err(frm_rdy_error)
    }

    /// Fetch one encoded packet without blocking. `Ok(None)` when nothing
    /// is queued — poll the fd for readability to know when to call again.
    pub fn get_packet(&self) -> Result<Option<Packet>> {
        let mut sptr = KmppShmPtr::default();
        let msg = VcodecMsg {
            cmd: VCODEC_CHAN_OUT_PKT_RDY,
            ctrl_cmd: 0,
            size: core::mem::size_of::<KmppShmPtr>() as u32,
            data_ptr: &mut sptr as *mut KmppShmPtr as u64,
        };
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), VOCDEC_IOC_CFG as _, &msg) };
        if ret < 0 {
            // Our patched vcodec driver reports "queue empty" as EAGAIN;
            // any other errno is a genuine fault.
            let errno = std::io::Error::last_os_error();
            return match errno.raw_os_error() {
                Some(libc::EAGAIN) => Ok(None),
                _ => Err(Error::Sys {
                    op: "VCODEC_CHAN_OUT_PKT_RDY",
                    source: errno,
                }),
            };
        }
        if sptr.is_null() {
            return Ok(None);
        }

        let obj = self.kmpp.adopt(sptr)?;
        if obj.type_name() != "KmppPacket" {
            let actual = obj.type_name().to_string();
            return Err(Error::TypeMismatch {
                expected: "KmppPacket",
                actual,
            });
        }

        let len = obj.get_s32("length")?.max(0) as usize;
        let pos = obj.get_shm_ptr("pos")?;
        let pts = obj.get_s64("pts")?;
        let dts = obj.get_s64("dts")?;
        let flag = obj.get_u32("flag")?;
        let temporal_id = obj.get_u32("temporal_id")?;

        Ok(Some(Packet {
            obj,
            data: pos.uaddr as usize as *const u8,
            len,
            pts,
            dts,
            flag,
            temporal_id,
        }))
    }

    /// Send a `VCODEC_CHAN_CONTROL` whose payload is a shared object.
    fn control_obj(&self, ctrl_cmd: u32, obj: &Obj) -> Result<()> {
        vcodec_ioctl(&self.fd, VCODEC_CHAN_CONTROL, ctrl_cmd, obj.shm_ptr())
            .map_err(|e| Error::Sys { op: "VCODEC_CHAN_CONTROL", source: e })
    }

    /// Escape hatch: send a raw control command with an arbitrary payload
    /// (`MPP_ENC_SET_ROI_CFG`, OSD, userdata, ...). The kernel copies
    /// `size` bytes from `data`.
    pub fn control_raw(&self, ctrl_cmd: u32, data: *const libc::c_void, size: u32) -> Result<()> {
        let msg = VcodecMsg {
            cmd: VCODEC_CHAN_CONTROL,
            ctrl_cmd,
            size,
            data_ptr: data as u64,
        };
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), VOCDEC_IOC_CFG as _, &msg) };
        if ret < 0 {
            return Err(Error::sys("VCODEC_CHAN_CONTROL"));
        }
        Ok(())
    }
}

impl AsFd for Venc {
    /// The pollable channel fd — readable when a packet is ready.
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsRawFd for Venc {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl Drop for Venc {
    fn drop(&mut self) {
        // Explicit destroy; the kernel would also do this on fd close.
        let _ = vcodec_ioctl_nodata(&self.fd, VCODEC_CHAN_DESTROY, 0);
    }
}

/// One encoded packet. Holds kernel ring-buffer space until dropped.
pub struct Packet {
    obj: Obj,
    data: *const u8,
    len: usize,
    pts: i64,
    dts: i64,
    flag: u32,
    temporal_id: u32,
}

unsafe impl Send for Packet {}

impl Packet {
    /// The encoded bitstream, zero-copy from the kernel output ring. The
    /// ring is double-mapped, so a wrapping packet is still contiguous.
    pub fn data(&self) -> &[u8] {
        if self.data.is_null() || self.len == 0 {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(self.data, self.len) }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn pts(&self) -> i64 {
        self.pts
    }

    pub fn dts(&self) -> i64 {
        self.dts
    }

    pub fn flag(&self) -> u32 {
        self.flag
    }

    pub fn temporal_id(&self) -> u32 {
        self.temporal_id
    }

    pub fn is_intra(&self) -> bool {
        self.flag & packet_flag::INTRA != 0
    }

    pub fn is_eos(&self) -> bool {
        self.flag & packet_flag::EOS != 0
    }

    /// The underlying `KmppPacket` object, for fields not surfaced here.
    pub fn obj(&self) -> &Obj {
        &self.obj
    }
}

impl std::fmt::Debug for Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Packet")
            .field("len", &self.len)
            .field("pts", &self.pts)
            .field("flag", &format_args!("{:#x}", self.flag))
            .finish()
    }
}

/// Map a `VCODEC_CHAN_IN_FRM_RDY` errno: `EAGAIN` = input queue full
/// (backpressure), anything else a genuine fault.
fn frm_rdy_error(e: std::io::Error) -> Error {
    match e.raw_os_error() {
        Some(libc::EAGAIN) => Error::QueueFull,
        _ => Error::Sys {
            op: "VCODEC_CHAN_IN_FRM_RDY",
            source: e,
        },
    }
}

fn vcodec_ioctl(fd: &OwnedFd, cmd: u32, ctrl_cmd: u32, sptr: &KmppShmPtr) -> std::io::Result<()> {
    let msg = VcodecMsg {
        cmd,
        ctrl_cmd,
        size: core::mem::size_of::<KmppShmPtr>() as u32,
        data_ptr: sptr as *const KmppShmPtr as u64,
    };
    let ret = unsafe { libc::ioctl(fd.as_raw_fd(), VOCDEC_IOC_CFG as _, &msg) };
    if ret < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn vcodec_ioctl_nodata(fd: &OwnedFd, cmd: u32, ctrl_cmd: u32) -> std::io::Result<()> {
    let msg = VcodecMsg {
        cmd,
        ctrl_cmd,
        size: 0,
        data_ptr: 0,
    };
    let ret = unsafe { libc::ioctl(fd.as_raw_fd(), VOCDEC_IOC_CFG as _, &msg) };
    if ret < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
