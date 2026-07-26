//! Capture from a V4L2 device and encode through KMPP.
//!
//! Captured buffers are exported as dma-buf fds (`VIDIOC_EXPBUF`) and handed
//! straight to the in-kernel encoder — the pixel data never touches
//! userspace. The whole loop is driven by a single `poll(2)` over the V4L2
//! fd (frame ready) and the KMPP channel fd (packet ready), demonstrating
//! the crate's non-blocking API.
//!
//! The capture format is user-supplied rather than negotiated, to keep the
//! example focused on the encoder. Run with `--help` for the full argument
//! list; the short version is:
//!
//! ```text
//! enc_v4l2 <device> <WxH> <pixfmt> <codec> <output> [options]
//!
//! e.g. enc_v4l2 /dev/video0 1920x1080 nv16 h264 out.h264 --bitrate 8000
//! ```
//!
//! For h264/h265 the output is a raw Annex-B elementary stream (play with
//! `ffplay -f h264 out.h264`); for jpeg it is a series of concatenated JPEG
//! images (`ffplay -f mjpeg`).

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::str::FromStr;

use clap::{Parser, ValueEnum};
use rk_kmpp::types::{
    color_primaries, color_space, color_trc, Coding, ColorRange, FrameFormat, HeaderMode, RcMode,
};
use rk_kmpp::venc::{FrameDesc, Venc, VencInit};
use rk_kmpp::Kmpp;

// ---------------------------------------------------------------------------
// Minimal raw V4L2 (multi-planar capture, MMAP + EXPBUF)
// ---------------------------------------------------------------------------

const V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE: u32 = 9;
const V4L2_MEMORY_MMAP: u32 = 1;
const NUM_BUFFERS: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy)]
struct V4l2PlanePixFormat {
    sizeimage: u32,
    bytesperline: u32,
    reserved: [u16; 6],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct V4l2PixFormatMplane {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    colorspace: u32,
    plane_fmt: [V4l2PlanePixFormat; 8],
    num_planes: u8,
    flags: u8,
    ycbcr_enc: u8,
    quantization: u8,
    xfer_func: u8,
    reserved: [u8; 7],
}

/// `struct v4l2_format`: u32 type + 200-byte union aligned to 8.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct V4l2FormatUnion {
    pix_mp: V4l2PixFormatMplane,
    _pad: [u8; 8],
}

#[repr(C)]
struct V4l2Format {
    type_: u32,
    fmt: V4l2FormatUnion,
}

#[repr(C)]
#[derive(Default)]
struct V4l2RequestBuffers {
    count: u32,
    type_: u32,
    memory: u32,
    capabilities: u32,
    flags: u8,
    reserved: [u8; 3],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct V4l2Plane {
    bytesused: u32,
    length: u32,
    m: u64, // union { mem_offset, userptr, fd }
    data_offset: u32,
    reserved: [u32; 11],
}

#[repr(C)]
struct V4l2Buffer {
    index: u32,
    type_: u32,
    bytesused: u32,
    flags: u32,
    field: u32,
    _pad: u32,
    timestamp: libc::timeval,
    timecode: [u32; 4],
    sequence: u32,
    memory: u32,
    m: u64, // union; for mplane: pointer to planes array
    length: u32,
    reserved2: u32,
    request_fd: u32,
}

#[repr(C)]
#[derive(Default)]
struct V4l2ExportBuffer {
    type_: u32,
    index: u32,
    plane: u32,
    flags: u32,
    fd: i32,
    reserved: [u32; 11],
}

const fn iowr<T>(nr: u8) -> libc::c_ulong {
    (3 << 30) | ((core::mem::size_of::<T>() as libc::c_ulong) << 16) | ((b'V' as libc::c_ulong) << 8) | nr as libc::c_ulong
}
const fn iow<T>(nr: u8) -> libc::c_ulong {
    (1 << 30) | ((core::mem::size_of::<T>() as libc::c_ulong) << 16) | ((b'V' as libc::c_ulong) << 8) | nr as libc::c_ulong
}

const VIDIOC_S_FMT: libc::c_ulong = iowr::<V4l2Format>(5);
const VIDIOC_REQBUFS: libc::c_ulong = iowr::<V4l2RequestBuffers>(8);
const VIDIOC_QBUF: libc::c_ulong = iowr::<V4l2Buffer>(15);
const VIDIOC_EXPBUF: libc::c_ulong = iowr::<V4l2ExportBuffer>(16);
const VIDIOC_DQBUF: libc::c_ulong = iowr::<V4l2Buffer>(17);
const VIDIOC_STREAMON: libc::c_ulong = iow::<i32>(18);
const VIDIOC_STREAMOFF: libc::c_ulong = iow::<i32>(19);

// The ioctl size field encodes sizeof; a mismatched struct silently becomes
// a different ioctl number, so pin the ABI-critical sizes.
const _: () = assert!(core::mem::size_of::<V4l2Format>() == 208);
const _: () = assert!(core::mem::size_of::<V4l2Buffer>() == 88);
const _: () = assert!(core::mem::size_of::<V4l2Plane>() == 64);
const _: () = assert!(core::mem::size_of::<V4l2ExportBuffer>() == 64);

fn xioctl<T>(fd: RawFd, req: libc::c_ulong, arg: &mut T, what: &str) -> std::io::Result<()> {
    let ret = unsafe { libc::ioctl(fd, req as _, arg as *mut T) };
    if ret < 0 {
        let e = std::io::Error::last_os_error();
        eprintln!("{what} failed: {e}");
        return Err(e);
    }
    Ok(())
}

struct Capture {
    fd: OwnedFd,
    /// Exported dma-buf fd of each buffer index.
    dmabuf_fds: Vec<OwnedFd>,
    bytesperline: u32,
}

impl Capture {
    fn open(device: &str, width: u32, height: u32, fourcc: u32) -> std::io::Result<Capture> {
        let cdev = std::ffi::CString::new(device).unwrap();
        let fd = unsafe {
            let raw = libc::open(cdev.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC | libc::O_NONBLOCK);
            if raw < 0 {
                return Err(std::io::Error::last_os_error());
            }
            OwnedFd::from_raw_fd(raw)
        };
        let raw = fd.as_raw_fd();

        let mut fmt: V4l2Format = unsafe { core::mem::zeroed() };
        fmt.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
        fmt.fmt.pix_mp.width = width;
        fmt.fmt.pix_mp.height = height;
        fmt.fmt.pix_mp.pixelformat = fourcc;
        fmt.fmt.pix_mp.field = 1; // V4L2_FIELD_NONE
        fmt.fmt.pix_mp.num_planes = 1;
        xioctl(raw, VIDIOC_S_FMT, &mut fmt, "VIDIOC_S_FMT")?;
        let got = &fmt.fmt.pix_mp;
        if got.width != width || got.height != height || got.pixelformat != fourcc {
            eprintln!(
                "warning: driver adjusted format to {}x{} fourcc {:#x}",
                got.width, got.height, got.pixelformat
            );
        }
        let bytesperline = got.plane_fmt[0].bytesperline;
        println!(
            "v4l2: {}x{} bytesperline={bytesperline} sizeimage={}",
            got.width, got.height, got.plane_fmt[0].sizeimage
        );

        let mut req = V4l2RequestBuffers {
            count: NUM_BUFFERS,
            type_: V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
            memory: V4L2_MEMORY_MMAP,
            ..Default::default()
        };
        xioctl(raw, VIDIOC_REQBUFS, &mut req, "VIDIOC_REQBUFS")?;
        if req.count == 0 {
            return Err(std::io::Error::other("no buffers granted"));
        }

        let mut dmabuf_fds = Vec::new();
        for i in 0..req.count {
            let mut exp = V4l2ExportBuffer {
                type_: V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
                index: i,
                plane: 0,
                flags: (libc::O_RDWR | libc::O_CLOEXEC) as u32,
                ..Default::default()
            };
            xioctl(raw, VIDIOC_EXPBUF, &mut exp, "VIDIOC_EXPBUF")?;
            dmabuf_fds.push(unsafe { OwnedFd::from_raw_fd(exp.fd) });
        }

        let cap = Capture {
            fd,
            dmabuf_fds,
            bytesperline,
        };
        for i in 0..req.count {
            cap.queue(i)?;
        }

        let mut on: i32 = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE as i32;
        xioctl(cap.fd.as_raw_fd(), VIDIOC_STREAMON, &mut on, "VIDIOC_STREAMON")?;
        Ok(cap)
    }

    fn queue(&self, index: u32) -> std::io::Result<()> {
        let mut planes = [V4l2Plane::default()];
        let mut buf: V4l2Buffer = unsafe { core::mem::zeroed() };
        buf.index = index;
        buf.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.m = planes.as_mut_ptr() as u64;
        buf.length = 1;
        xioctl(self.fd.as_raw_fd(), VIDIOC_QBUF, &mut buf, "VIDIOC_QBUF")
    }

    /// Dequeue one filled buffer: `(index, timestamp_us)`.
    fn dequeue(&self) -> std::io::Result<Option<(u32, i64)>> {
        let mut planes = [V4l2Plane::default()];
        let mut buf: V4l2Buffer = unsafe { core::mem::zeroed() };
        buf.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.m = planes.as_mut_ptr() as u64;
        buf.length = 1;
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), VIDIOC_DQBUF as _, &mut buf) };
        if ret < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EAGAIN) {
                return Ok(None);
            }
            return Err(e);
        }
        let ts = buf.timestamp.tv_sec as i64 * 1_000_000 + buf.timestamp.tv_usec as i64;
        Ok(Some((buf.index, ts)))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let mut off: i32 = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE as i32;
        let _ = unsafe { libc::ioctl(self.fd.as_raw_fd(), VIDIOC_STREAMOFF as _, &mut off) };
    }
}

// ---------------------------------------------------------------------------
// Encoder configuration
// ---------------------------------------------------------------------------

// Hardware AQ/QBIAS tables from rockchip-mpp's mpi_enc_utils.c. On VEPU511
// the HEVC encoder times out without the qbias table programmed.
static QBIAS_ARR_HEVC: [i32; 18] = [
    3, 6, 13, 171, 171, 171, 171, 3, 6, 13, 171, 171, 220, 171, 85, 85, 85, 85,
];
static QBIAS_ARR_AVC: [i32; 18] = [
    3, 6, 13, 683, 683, 683, 683, 3, 6, 13, 683, 683, 683, 683, 341, 341, 341, 341,
];
static AQ_THD: [i32; 16] = [0, 0, 0, 0, 3, 3, 5, 5, 8, 8, 8, 15, 15, 20, 25, 25];
static AQ_STEP_I: [i32; 16] = [-8, -7, -6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 5, 7, 7, 8];
static AQ_STEP_P: [i32; 16] = [-8, -7, -6, -5, -4, -2, -1, -1, 0, 2, 3, 4, 6, 8, 9, 10];

struct ColorArgs {
    range: ColorRange,
    /// (matrix, primaries, transfer)
    space: (i32, i32, i32),
}

/// Intra-refresh (GDR) settings: spread the intra content of a "keyframe"
/// over a run of frames instead of spending it all in one IDR.
///
/// The kernel derives the cycle length itself as
/// `refresh_length = ceil(mb_rows / num)`, clamped to the GOP
/// (`mpp_enc_impl.c:930-941`), and drives it off `seq_idx % gop`. So the GOP
/// has to be at least as long as the sweep or the band never reaches the
/// bottom of the frame — [`RefreshArgs::check`] enforces that.
#[derive(Clone, Copy)]
struct RefreshArgs {
    /// Macroblock rows (or columns) made intra per frame; 0 = feature off.
    num: u32,
    mode: RefreshMode,
    /// `constrained_intra_pred_flag`. Without it the intra blocks in the
    /// refresh band predict from their not-yet-refreshed neighbours, so the
    /// band is never actually clean and the recovery point does not recover.
    const_intra: bool,
}

impl RefreshArgs {
    fn enabled(&self) -> bool {
        self.num > 0
    }

    /// Macroblock size the encoder counts in: H.264 works in 16x16 MBs,
    /// H.265 in 32x32 CTUs (`mpp_enc_impl.c:934-937` / `:990-993`).
    fn mb_size(coding: Coding) -> u32 {
        match coding {
            Coding::Hevc => 32,
            _ => 16,
        }
    }

    /// Frames one full sweep takes, i.e. the kernel's `refresh_length`.
    fn cycle(&self, coding: Coding, width: u32, height: u32) -> u32 {
        let mb = Self::mb_size(coding);
        let span = match self.mode {
            RefreshMode::Row => height,
            RefreshMode::Col => width,
        };
        let mbs = span.div_ceil(mb);
        mbs.div_ceil(self.num)
    }

    /// Reject the configurations the kernel would silently mangle.
    fn check(&self, coding: Coding, width: u32, height: u32, gop: i32) -> Result<u32, String> {
        let cycle = self.cycle(coding, width, height);
        if matches!(coding, Coding::Mjpeg) {
            return Err("intra refresh is meaningless for jpeg (every frame is intra)".into());
        }
        if gop <= 0 {
            // refresh_length is clamped to the GOP, and a 0 length reads as
            // "feature off" in set_frm_refresh_flag.
            return Err(format!(
                "intra refresh needs --gop >= {cycle} (the sweep length); \
                 --gop 0 clamps the sweep to 0 and silently disables it"
            ));
        }
        if (gop as u32) < cycle {
            return Err(format!(
                "--gop {gop} is shorter than the {cycle}-frame sweep; the kernel clamps the \
                 sweep to the GOP, so the band would restart before reaching the end of the frame"
            ));
        }
        Ok(cycle)
    }
}

#[allow(clippy::too_many_arguments)]
fn configure(
    venc: &Venc,
    coding: Coding,
    width: u32,
    height: u32,
    hor_stride: u32,
    fmt: FrameFormat,
    bitrate_kbps: u32,
    fps: u32,
    gop: i32,
    refresh: &RefreshArgs,
    color: &ColorArgs,
) -> rk_kmpp::Result<()> {
    let cfg = venc.new_cfg()?;

    cfg.set_s32("codec:type", coding as i32)?;

    cfg.set_s32("prep:width", width as i32)?;
    cfg.set_s32("prep:height", height as i32)?;
    cfg.set_s32("prep:hor_stride", hor_stride as i32)?;
    cfg.set_s32("prep:ver_stride", height as i32)?;
    cfg.set_s32("prep:format", fmt as i32)?;
    cfg.set_s32("prep:colorrange", color.range as i32)?;
    cfg.set_s32("prep:colorspace", color.space.0)?;
    cfg.set_s32("prep:colorprim", color.space.1)?;
    cfg.set_s32("prep:colortrc", color.space.2)?;

    cfg.set_s32("rc:fps_in_flex", 0)?;
    cfg.set_s32("rc:fps_in_num", fps as i32)?;
    cfg.set_s32("rc:fps_in_denom", 1)?;
    cfg.set_s32("rc:fps_out_flex", 0)?;
    cfg.set_s32("rc:fps_out_num", fps as i32)?;
    cfg.set_s32("rc:fps_out_denom", 1)?;
    cfg.set_s32("rc:gop", gop)?;

    if refresh.enabled() {
        cfg.set_u32("rc:refresh_en", 1)?;
        cfg.set_u32("rc:refresh_mode", refresh.mode as u32)?;
        cfg.set_u32("rc:refresh_num", refresh.num)?;
    }

    let bps = (bitrate_kbps * 1000) as i32;
    match coding {
        Coding::Mjpeg => {
            cfg.set_s32("rc:mode", RcMode::FixQp as i32)?;
            cfg.set_s32("jpeg:q_factor", 80)?;
            cfg.set_s32("jpeg:qf_min", 1)?;
            cfg.set_s32("jpeg:qf_max", 99)?;
        }
        _ => {
            cfg.set_s32("rc:mode", RcMode::Cbr as i32)?;
            cfg.set_s32("rc:bps_target", bps)?;
            cfg.set_s32("rc:bps_max", bps * 17 / 16)?;
            cfg.set_s32("rc:bps_min", bps * 15 / 16)?;
            cfg.set_s32("rc:qp_init", -1)?;
            cfg.set_s32("rc:qp_min", 10)?;
            cfg.set_s32("rc:qp_max", 51)?;
            cfg.set_s32("rc:qp_min_i", 10)?;
            cfg.set_s32("rc:qp_max_i", 51)?;
            cfg.set_s32("rc:qp_ip", 2)?;
        }
    }

    match coding {
        Coding::Avc => {
            cfg.set_s32("h264:profile", 100)?;
            cfg.set_s32("h264:level", 40)?;
            cfg.set_s32("h264:cabac_en", 1)?;
            cfg.set_s32("h264:cabac_idc", 0)?;
            cfg.set_s32("h264:trans8x8", 1)?;
            cfg.set_s32("h264:const_intra", refresh.const_intra as i32)?;
        }
        Coding::Hevc => {
            cfg.set_s32("h265:diff_cu_qp_delta_depth", 0)?;
            cfg.set_s32("h265:const_intra", refresh.const_intra as i32)?;
        }
        Coding::Mjpeg => {}
    }

    // AQ / qbias hardware tables. Sizes are kernel-defined; warn instead of
    // failing so a size change in a future SDK degrades gracefully.
    if !matches!(coding, Coding::Mjpeg) {
        let qbias: &[i32] = match coding {
            Coding::Avc => &QBIAS_ARR_AVC,
            _ => &QBIAS_ARR_HEVC,
        };
        for (name, arr) in [
            ("hw:aq_thrd_i", &AQ_THD[..]),
            ("hw:aq_thrd_p", &AQ_THD[..]),
            ("hw:aq_step_i", &AQ_STEP_I[..]),
            ("hw:aq_step_p", &AQ_STEP_P[..]),
            ("hw:qbias_arr", qbias),
        ] {
            if let Err(e) = cfg.set_struct_s32s(name, arr) {
                eprintln!("warning: {name}: {e}");
            }
        }
    }

    venc.set_cfg(&cfg)?;

    if !matches!(coding, Coding::Mjpeg) {
        venc.set_header_mode(HeaderMode::EachIdr)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Forced-IDR test
// ---------------------------------------------------------------------------

/// Packet gap between forced-IDR requests: a fixed `N`, or `MIN..MAX` to draw
/// a fresh gap at random before each request.
#[derive(Clone, Copy)]
struct Interval {
    min: u64,
    max: u64,
}

impl Interval {
    /// Uniform in `min..=max`.
    fn pick(&self, rng: &mut Rng) -> u64 {
        if self.max <= self.min {
            return self.max.max(1);
        }
        (self.min + rng.next() % (self.max - self.min + 1)).max(1)
    }
}

impl FromStr for Interval {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parse = |v: &str| v.parse::<u64>().map_err(|e| format!("bad packet count `{v}`: {e}"));
        let (min, max) = match s.split_once("..") {
            Some((a, b)) => (parse(a)?, parse(b)?),
            None => {
                let n = parse(s)?;
                (n, n)
            }
        };
        if min > max {
            return Err(format!("empty range `{s}`: {min} > {max}"));
        }
        Ok(Interval { min, max })
    }
}

/// xorshift64*; the test only needs spread, not statistical quality.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

/// Exercises [`Venc::request_idr`]: asks for an intra frame at irregular
/// intervals and checks that one actually comes back.
///
/// `request_idr` is a channel control, not a per-frame flag, so it applies to
/// the next frame the *encoder* picks up — not necessarily the next frame
/// this process submits. With up to 2 frames already queued kernel-side the
/// forced packet can therefore lag the request by a few packets, which is why
/// the check is "an intra packet within `deadline` packets" rather than
/// "the very next packet".
///
/// Run with `--gop 0` so the only intra frames in the stream are the opening
/// one and the ones this test asks for; any other intra packet is then a
/// genuine surprise and gets reported as unsolicited.
struct IdrTest {
    /// Packet gap between requests.
    interval: Interval,
    rng: Rng,
    /// Output packet index at which to make the next request.
    next_at: u64,
    /// How many packets a request may lag before it counts as missed.
    deadline: u64,
    /// Packet index of the outstanding request, if any.
    pending: Option<u64>,
    /// `(request packet index, latency in packets)` for answered requests.
    satisfied: Vec<(u64, u64)>,
    /// Requests that never produced an intra packet in time.
    missed: Vec<u64>,
    /// Intra packets nobody asked for (index).
    unsolicited: Vec<u64>,
}

impl IdrTest {
    fn new(interval: Option<Interval>, deadline: u64, seed: u64) -> Self {
        let interval = interval.unwrap_or(Interval { min: 0, max: 0 });
        let mut rng = Rng::new(seed);
        let next_at = interval.pick(&mut rng);
        IdrTest {
            interval,
            rng,
            next_at,
            deadline,
            pending: None,
            satisfied: Vec::new(),
            missed: Vec::new(),
            unsolicited: Vec::new(),
        }
    }

    fn enabled(&self) -> bool {
        self.interval.max > 0
    }

    /// Account for output packet `n` (1-based) and issue the next request.
    fn on_packet(&mut self, venc: &Venc, n: u64, intra: bool) {
        if !self.enabled() {
            return;
        }
        if intra {
            match self.pending.take() {
                Some(at) => {
                    println!("pkt {n}: [IDR] answers request from pkt {at} (+{})", n - at);
                    self.satisfied.push((at, n - at));
                    self.next_at = n + self.interval.pick(&mut self.rng);
                }
                // Packet 1 is the stream-opening IDR, not a forced one.
                None if n == 1 => {}
                None => {
                    eprintln!("pkt {n}: unsolicited [IDR]");
                    self.unsolicited.push(n);
                }
            }
        } else if let Some(at) = self.pending {
            if n - at >= self.deadline {
                eprintln!("pkt {n}: no IDR within {} packets of request at pkt {at}", self.deadline);
                self.missed.push(at);
                self.pending = None;
                self.next_at = n + self.interval.pick(&mut self.rng);
            }
        }

        if self.pending.is_none() && n >= self.next_at {
            match venc.request_idr() {
                Ok(()) => {
                    println!("pkt {n}: requested IDR");
                    self.pending = Some(n);
                }
                Err(e) => {
                    eprintln!("pkt {n}: request_idr failed: {e}");
                    self.missed.push(n);
                    self.next_at = n + self.interval.pick(&mut self.rng);
                }
            }
        }
    }

    /// Print the verdict; `false` if the encoder did not honour the requests.
    fn report(&self) -> bool {
        if !self.enabled() {
            return true;
        }
        let asked = self.satisfied.len() + self.missed.len();
        let lat: Vec<u64> = self.satisfied.iter().map(|&(_, l)| l).collect();
        print!("idr test: {}/{asked} forced IDRs delivered", self.satisfied.len());
        match (lat.iter().min(), lat.iter().max()) {
            (Some(min), Some(max)) => println!(" (latency {min}..{max} packets)"),
            _ => println!(),
        }
        if !self.missed.is_empty() {
            println!("  missed requests at packets: {:?}", self.missed);
        }
        if !self.unsolicited.is_empty() {
            println!("  unsolicited intra packets: {:?}", self.unsolicited);
        }
        // A request still in flight when the run ends is not a failure.
        let ok = asked > 0 && self.missed.is_empty() && self.unsolicited.is_empty();
        println!("idr test: {}", if ok { "PASS" } else { "FAIL" });
        ok
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*s)
}

/// `WxH`, e.g. `1920x1080`.
#[derive(Clone, Copy)]
struct Resolution {
    width: u32,
    height: u32,
}

impl FromStr for Resolution {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (w, h) = s
            .split_once(['x', 'X'])
            .ok_or_else(|| format!("expected WxH, got `{s}`"))?;
        let parse = |v: &str, what: &str| {
            v.parse::<u32>()
                .map_err(|e| format!("bad {what} `{v}`: {e}"))
                .and_then(|n| (n > 0).then_some(n).ok_or_else(|| format!("{what} must be > 0")))
        };
        Ok(Resolution {
            width: parse(w, "width")?,
            height: parse(h, "height")?,
        })
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum PixFmt {
    Nv12,
    Nv16,
    Nv21,
    Yuyv,
    Uyvy,
}

impl PixFmt {
    /// `(V4L2 fourcc, MPP frame format)`
    fn formats(self) -> (u32, FrameFormat) {
        match self {
            PixFmt::Nv12 => (fourcc(b"NV12"), FrameFormat::Yuv420Sp),
            PixFmt::Nv16 => (fourcc(b"NV16"), FrameFormat::Yuv422Sp),
            PixFmt::Nv21 => (fourcc(b"NV21"), FrameFormat::Yuv420SpVu),
            PixFmt::Yuyv => (fourcc(b"YUYV"), FrameFormat::Yuv422Yuyv),
            PixFmt::Uyvy => (fourcc(b"UYVY"), FrameFormat::Yuv422Uyvy),
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Codec {
    H264,
    H265,
    Jpeg,
}

impl Codec {
    fn coding(self) -> Coding {
        match self {
            Codec::H264 => Coding::Avc,
            Codec::H265 => Coding::Hevc,
            Codec::Jpeg => Coding::Mjpeg,
        }
    }
}

/// Direction the intra-refresh band sweeps. Values match
/// `MppEncRcRefreshMode` (`rk_venc_rc.h:57-61`).
#[derive(Clone, Copy, ValueEnum)]
enum RefreshMode {
    /// Top to bottom, refreshing whole macroblock rows.
    Row = 0,
    /// Left to right, refreshing whole macroblock columns.
    Col = 1,
}

#[derive(Clone, Copy, ValueEnum)]
enum Range {
    Limited,
    Full,
}

impl Range {
    fn color_range(self) -> ColorRange {
        match self {
            Range::Limited => ColorRange::Limited,
            Range::Full => ColorRange::Full,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ColorSpace {
    Bt709,
    Bt601,
}

impl ColorSpace {
    /// `(matrix, primaries, transfer)`
    fn triplet(self) -> (i32, i32, i32) {
        match self {
            ColorSpace::Bt709 => (color_space::BT709, color_primaries::BT709, color_trc::BT709),
            ColorSpace::Bt601 => (
                color_space::BT470BG,
                color_primaries::BT470BG,
                color_trc::SMPTE170M,
            ),
        }
    }
}

/// Capture from a V4L2 device and encode through KMPP.
#[derive(Parser)]
#[command(
    about,
    long_about = None,
    after_help = "For h264/h265 the output is a raw Annex-B elementary stream (play with\n\
                  `ffplay -f h264 out.h264`); for jpeg it is a series of concatenated JPEG\n\
                  images (`ffplay -f mjpeg`).\n\n\
                  e.g. enc_v4l2 /dev/video0 1920x1080 nv16 h264 out.h264 --bitrate 8000\n\n\
                  To test forced IDR, disable the periodic one so only forced intra\n\
                  frames remain:\n\
                  \x20 enc_v4l2 /dev/video0 1920x1080 yuyv h264 out.h264 --gop 0 --idr-every 20..80"
)]
struct Args {
    /// V4L2 capture device, e.g. /dev/video0
    device: String,

    /// Capture resolution as WxH, e.g. 1920x1080
    resolution: Resolution,

    /// Capture pixel format (not negotiated — must be supported by the device)
    pixfmt: PixFmt,

    /// Output codec
    codec: Codec,

    /// Output file
    output: PathBuf,

    /// Stop after N encoded packets
    #[arg(long, value_name = "N", default_value_t = 300)]
    frames: u64,

    /// Target bitrate in kbps (ignored for jpeg)
    #[arg(long, value_name = "KBPS", default_value_t = 4000)]
    bitrate: u32,

    /// Nominal input fps for rate control
    #[arg(long, value_name = "N", default_value_t = 60)]
    fps: u32,

    /// GOP length in frames; 0 = only the first frame is intra.
    /// Defaults to 2 seconds' worth.
    #[arg(long, value_name = "N")]
    gop: Option<i32>,

    /// Test forced IDR: ask for an intra frame every N output packets, or at
    /// a random gap drawn from MIN..MAX, and check one arrives. Pair with
    /// `--gop 0` so forced IDRs are the only ones in the stream.
    #[arg(long, value_name = "N|MIN..MAX")]
    idr_every: Option<Interval>,

    /// How many packets a forced IDR may lag its request before the test
    /// calls it missed
    #[arg(long, value_name = "N", default_value_t = 4)]
    idr_deadline: u64,

    /// Seed for the random `--idr-every` gaps; 0 = derive from the clock
    #[arg(long, value_name = "N", default_value_t = 0)]
    idr_seed: u64,

    /// Intra refresh (GDR): make N macroblock rows/columns intra per frame
    /// and drop the periodic IDR, spreading keyframe cost over the sweep.
    /// Needs `--gop` >= the sweep length. 0 = off.
    #[arg(long, value_name = "N", default_value_t = 0)]
    intra_refresh: u32,

    /// Direction the intra-refresh band sweeps
    #[arg(long, value_enum, default_value_t = RefreshMode::Row)]
    refresh_mode: RefreshMode,

    /// Set constrained_intra_pred_flag, so intra blocks predict only from
    /// other intra blocks. Needed for an intra-refresh sweep to leave a
    /// genuinely clean picture behind it.
    #[arg(long)]
    const_intra: bool,

    /// Color range signalled to the encoder
    #[arg(long, value_enum, default_value_t = Range::Limited)]
    range: Range,

    /// Color space signalled to the encoder
    #[arg(long, value_enum, default_value_t = ColorSpace::Bt709)]
    colorspace: ColorSpace,
}

fn main() -> std::process::ExitCode {
    env_logger::init();
    let args = Args::parse();

    let Resolution { width, height } = args.resolution;
    let (v4l2_fourcc, mpp_fmt) = args.pixfmt.formats();
    let coding = args.codec.coding();
    let device = &args.device;
    let out_path = &args.output;
    let max_frames = args.frames;
    let bitrate_kbps = args.bitrate;
    let fps = args.fps;
    let gop = args.gop.unwrap_or((fps * 2) as i32);
    let refresh = RefreshArgs {
        num: args.intra_refresh,
        mode: args.refresh_mode,
        const_intra: args.const_intra,
    };
    if refresh.enabled() {
        match refresh.check(coding, width, height, gop) {
            Ok(cycle) => println!(
                "intra refresh: {} {} per frame, {cycle}-frame sweep, gop {gop}",
                refresh.num,
                match args.refresh_mode {
                    RefreshMode::Row => "rows",
                    RefreshMode::Col => "columns",
                }
            ),
            Err(e) => {
                eprintln!("error: {e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    let color = ColorArgs {
        range: args.range.color_range(),
        space: args.colorspace.triplet(),
    };

    // --- encoder channel ---
    let kmpp = Kmpp::open().expect("open /dev/kmpp_objs (is kmpp.ko loaded?)");
    let venc = Venc::create(&kmpp, &VencInit::new(coding, width, height)).expect("create channel");

    // --- capture ---
    let cap = Capture::open(device, width, height, v4l2_fourcc).expect("v4l2 setup");

    configure(
        &venc,
        coding,
        width,
        height,
        cap.bytesperline,
        mpp_fmt,
        bitrate_kbps,
        fps,
        gop,
        &refresh,
        &color,
    )
    .expect("encoder config");

    let mut out = File::create(out_path).expect("create output");
    let idr_seed = match args.idr_seed {
        0 => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.subsec_nanos() as u64 | (d.as_secs() << 32)),
        n => n,
    };
    if args.idr_every.is_some() {
        println!("idr test: seed {idr_seed} (replay with --idr-seed {idr_seed})");
    }
    let mut idr = IdrTest::new(args.idr_every, args.idr_deadline, idr_seed);
    let mut n_in: u64 = 0;
    let mut n_out: u64 = 0;
    let mut bytes: u64 = 0;
    let mut n_intra: u64 = 0;
    let mut peak: usize = 0;
    // The stream-opening IDR is unavoidable in every mode, so it tells you
    // nothing about how peaky the steady state is; track that separately.
    let mut peak_steady: usize = 0;

    let mut fds = [
        libc::pollfd {
            fd: cap.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: venc.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];

    while n_out < max_frames {
        let ret = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, 2000) };
        if ret < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            panic!("poll: {e}");
        }
        if ret == 0 {
            eprintln!("timeout waiting for frames/packets (in={n_in} out={n_out})");
            continue;
        }

        if fds[0].revents & libc::POLLIN != 0 {
            while let Some((index, ts)) = cap.dequeue().expect("DQBUF") {
                let mut desc = FrameDesc::new(
                    width,
                    height,
                    cap.bytesperline,
                    height,
                    mpp_fmt,
                    cap.dmabuf_fds[index as usize].as_raw_fd(),
                );
                desc.pts = ts;
                desc.dts = ts;
                match venc.put_frame(&desc) {
                    Ok(()) => n_in += 1,
                    Err(rk_kmpp::Error::QueueFull) => {
                        eprintln!("input queue full, dropping frame {n_in}")
                    }
                    Err(e) => panic!("put_frame: {e}"),
                }
                // NOTE: requeueing immediately means capture may overwrite a
                // buffer the encoder is still reading. With 4 buffers in
                // rotation the encoder (one frame-time latency) is done long
                // before the buffer comes around again.
                cap.queue(index).expect("QBUF");
            }
        }

        if fds[1].revents & libc::POLLIN != 0 {
            while let Some(pkt) = venc.get_packet().expect("get_packet") {
                out.write_all(pkt.data()).expect("write output");
                bytes += pkt.len() as u64;
                n_out += 1;
                let intra = pkt.is_intra();
                n_intra += intra as u64;
                peak = peak.max(pkt.len());
                if n_out > 1 {
                    peak_steady = peak_steady.max(pkt.len());
                }
                if intra || n_out.is_multiple_of(60) {
                    println!(
                        "pkt {n_out}: {} bytes pts={} {}",
                        pkt.len(),
                        pkt.pts(),
                        if intra { "[IDR]" } else { "" }
                    );
                }
                drop(pkt);
                idr.on_packet(&venc, n_out, intra);
                if n_out >= max_frames {
                    break;
                }
            }
        }
    }

    println!(
        "done: {n_in} frames in, {n_out} packets out, {bytes} bytes -> {}",
        out_path.display()
    );
    if let Some(mean) = bytes.checked_div(n_out) {
        let mean = mean.max(1);
        println!(
            "packets: {n_intra} intra, mean {mean} bytes, peak {peak} ({:.1}x mean), \
             peak after first {peak_steady} ({:.1}x mean)",
            peak as f64 / mean as f64,
            peak_steady as f64 / mean as f64
        );
    }

    if idr.report() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
