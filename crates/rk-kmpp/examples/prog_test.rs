//! Board test for progressive encoder input (`include/rk-vcodec-prog.h` in
//! rk-kmpp-kmod), through `/dev/vcodec` like the server.
//!
//! ```text
//! prog_test <gate|control|abort|stall|close|stop|jpeg> [h264|h265] [nv12|bgr24|yuv444] [frames] [outdir]
//! ```
//!
//! The "capture" is simulated: before a progressive frame is submitted, rows
//! at and below the ready count hold random poison; rows are written correctly
//! just before each advance. A shadow channel encodes the same frames whole.
//!
//! * gate: every gated packet must be byte-identical to the shadow packet.
//! * abort: queued abort, running abort (P and IDR), request_idr during a
//!   hold, an encoder drop (Tracked frame smaller than the config). The shadow
//!   skips the frames the gated channel lost; decode both streams on the host.
//! * stall: a frame that never advances must end STALLED after ~200 ms.
//! * close: closing the channel during a hold must not hang or reset.
//! * stop: STOP during a hold; the held frame and the queued one still end.
//! * jpeg: a JPEG channel next to a hold (tracked JPEG rejected, JPEG output
//!   unchanged, wait time reported).

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

use rk_kmpp::types::{Coding, FrameFormat, HeaderMode, RcMode};
use rk_kmpp::venc::{FrameDesc, InputMode, InputStatus, Venc, VencInit};
use rk_kmpp::Kmpp;

const W: u32 = 1920;
const H: u32 = 1080;
const R0: u32 = 16;
const STEP: u32 = 64;

// ---- dma-heap buffers -------------------------------------------------------

#[repr(C)]
struct HeapAlloc {
    len: u64,
    fd: u32,
    fd_flags: u32,
    heap_flags: u64,
}
const DMA_HEAP_IOCTL_ALLOC: libc::c_ulong =
    (3 << 30) | ((core::mem::size_of::<HeapAlloc>() as libc::c_ulong) << 16) | ((b'H' as libc::c_ulong) << 8);
const DMA_BUF_IOCTL_SYNC: libc::c_ulong = (1 << 30) | (8 << 16) | ((b'b' as libc::c_ulong) << 8);
const SYNC_WRITE_START: u64 = 2;
const SYNC_WRITE_END: u64 = 2 | 4;

struct DmaBuf {
    fd: OwnedFd,
    ptr: *mut u8,
    size: usize,
}

impl DmaBuf {
    fn new(size: usize) -> DmaBuf {
        let heap = unsafe { libc::open(c"/dev/dma_heap/system".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        assert!(heap >= 0, "open dma heap: {}", std::io::Error::last_os_error());
        let mut a = HeapAlloc {
            len: size as u64,
            fd: 0,
            fd_flags: (libc::O_RDWR | libc::O_CLOEXEC) as u32,
            heap_flags: 0,
        };
        let r = unsafe { libc::ioctl(heap, DMA_HEAP_IOCTL_ALLOC as _, &mut a) };
        assert!(r >= 0, "dma heap alloc: {}", std::io::Error::last_os_error());
        unsafe { libc::close(heap) };
        let fd = unsafe { OwnedFd::from_raw_fd(a.fd as RawFd) };
        let ptr = unsafe {
            libc::mmap(core::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.as_raw_fd(), 0)
        };
        assert!(ptr != libc::MAP_FAILED, "mmap dmabuf");
        DmaBuf { fd, ptr: ptr as *mut u8, size }
    }

    fn sync(&self, flags: u64) {
        let r = unsafe { libc::ioctl(self.fd.as_raw_fd(), DMA_BUF_IOCTL_SYNC as _, &flags) };
        assert!(r >= 0, "dmabuf sync");
    }

    fn bytes(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.size) }
    }
}

// ---- frame layout and content ------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Fmt {
    Nv12,
    Bgr24,
    Yuv444,
}

impl Fmt {
    fn stride(self) -> u32 {
        match self {
            Fmt::Nv12 => W,
            _ => W * 3,
        }
    }
    fn size(self) -> usize {
        match self {
            Fmt::Nv12 => (W * H * 3 / 2) as usize,
            _ => (W * 3 * H) as usize,
        }
    }
    fn mpp(self) -> FrameFormat {
        match self {
            Fmt::Nv12 => FrameFormat::Yuv420Sp,
            Fmt::Bgr24 => FrameFormat::Bgr888,
            Fmt::Yuv444 => FrameFormat::Yuv444Packed,
        }
    }
}

/// Content of row `y` (plane 0) of frame `f`: a moving ramp and a moving square.
fn fill_row(row: &mut [u8], y: u32, f: u32) {
    let sq = (f * 24) % (W - 200);
    for (x, b) in row.iter_mut().enumerate() {
        let x = x as u32;
        let px = x / 3;
        // smooth ramp: encodes at high quality, so a wrong reference shows
        let v = ((x / 2 + y / 3 + f * 3) & 0xff) / 2;
        let in_sq = px >= sq && px < sq + 200 && y >= 300 && y < 500;
        *b = if in_sq { 230 } else { (v + 40) as u8 };
    }
}

fn fill_chroma_row(row: &mut [u8], c: u32, f: u32) {
    for (x, b) in row.iter_mut().enumerate() {
        *b = (100 + ((x as u32 / 16 + c / 8 + f) & 31)) as u8;
    }
}

struct Rng(u64);
impl Rng {
    fn fill(&mut self, b: &mut [u8]) {
        for v in b.iter_mut() {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            *v = self.0 as u8;
        }
    }
}

/// Write luma rows `[r0, r1)` (and the matching chroma rows) of frame `f`,
/// correctly or as poison.
fn write_rows(buf: &mut DmaBuf, fmt: Fmt, f: u32, r0: u32, r1: u32, poison: Option<&mut Rng>) {
    let s = fmt.stride() as usize;
    buf.sync(SYNC_WRITE_START);
    let mem = buf.bytes();
    let (y_rows, c_rows) = (r0 as usize..r1 as usize, (r0 as usize / 2)..(r1 as usize).div_ceil(2));
    match poison {
        Some(rng) => {
            rng.fill(&mut mem[y_rows.start * s..y_rows.end * s]);
            if fmt == Fmt::Nv12 {
                let base = s * H as usize;
                rng.fill(&mut mem[base + c_rows.start * s..base + c_rows.end * s]);
            }
        }
        None => {
            for y in y_rows {
                fill_row(&mut mem[y * s..(y + 1) * s], y as u32, f);
            }
            if fmt == Fmt::Nv12 {
                let base = s * H as usize;
                for c in c_rows {
                    fill_chroma_row(&mut mem[base + c * s..base + (c + 1) * s], c as u32, f);
                }
            }
        }
    }
    buf.sync(SYNC_WRITE_END);
}

// ---- encoder -----------------------------------------------------------------

fn create(kmpp: &Kmpp, coding: Coding, fmt: Fmt) -> Venc {
    let init = VencInit {
        buf_size: W * H * 3 / 2,
        ..VencInit::new(coding, W, H)
    };
    let venc = Venc::create(kmpp, &init).expect("create channel");
    let cfg = venc.new_cfg().unwrap();
    let c = || -> rk_kmpp::Result<()> {
        cfg.set_s32("codec:type", coding as i32)?;
        cfg.set_s32("prep:width", W as i32)?;
        cfg.set_s32("prep:height", H as i32)?;
        cfg.set_s32("prep:hor_stride", fmt.stride() as i32)?;
        cfg.set_s32("prep:ver_stride", H as i32)?;
        cfg.set_s32("prep:format", fmt.mpp() as i32)?;
        cfg.set_s32("rc:fps_in_flex", 0)?;
        cfg.set_s32("rc:fps_in_num", 60)?;
        cfg.set_s32("rc:fps_in_denom", 1)?;
        cfg.set_s32("rc:fps_out_flex", 0)?;
        cfg.set_s32("rc:fps_out_num", 60)?;
        cfg.set_s32("rc:fps_out_denom", 1)?;
        cfg.set_s32("rc:gop", 0)?;
        cfg.set_s32("rc:mode", RcMode::Cbr as i32)?;
        cfg.set_s32("rc:bps_target", 8_000_000)?;
        cfg.set_s32("rc:bps_max", 8_500_000)?;
        cfg.set_s32("rc:bps_min", 7_500_000)?;
        cfg.set_s32("rc:qp_init", -1)?;
        cfg.set_u32("rc:max_reenc_times", 0)?;
        cfg.set_u32("rc:drop_mode", 0)?;
        if coding == Coding::Avc {
            cfg.set_s32("h264:profile", 100)?;
            cfg.set_s32("h264:level", 40)?;
            cfg.set_s32("h264:cabac_en", 1)?;
            cfg.set_s32("h264:trans8x8", 1)?;
        }
        venc.set_cfg(&cfg)?;
        venc.set_header_mode(HeaderMode::EachIdr)
    };
    c().expect("configure");
    assert!(venc.supports_progressive(), "kmpp.ko without progressive input");
    venc
}

fn desc(fmt: Fmt, buf: &DmaBuf, dts: i64, input: InputMode) -> FrameDesc {
    let mut d = FrameDesc::new(W, H, fmt.stride(), H, fmt.mpp(), buf.fd.as_raw_fd());
    d.dts = dts;
    d.pts = dts * 16_667;
    d.input = input;
    d
}

struct Rec {
    dts: i64,
    data: Vec<u8>,
    intra: bool,
    discard: bool,
    status: InputStatus,
    rows_start: u32,
    tail_us: u32,
    at: Instant,
}

fn wait_rec(venc: &Venc, timeout_ms: i32) -> Option<Rec> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
    loop {
        if let Some(p) = venc.get_packet().expect("get_packet") {
            let t = p.input_timing().unwrap();
            return Some(Rec {
                dts: p.dts(),
                data: p.data().to_vec(),
                intra: p.is_intra(),
                discard: p.is_discarded(),
                status: p.input_status().unwrap(),
                rows_start: t.rows_start,
                tail_us: t.tail_us,
                at: Instant::now(),
            });
        }
        let left = deadline.saturating_duration_since(Instant::now()).as_millis() as i32;
        if left <= 0 {
            return None;
        }
        let mut pfd = libc::pollfd { fd: venc.as_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        unsafe { libc::poll(&mut pfd, 1, left) };
    }
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Simulated capture of frame `f` into `buf` as a progressive frame: poison
/// below R0, submit, then write and advance STEP rows per millisecond.
/// `hook(rows)` runs after each advance and may stop the loop (returns false).
fn gated_frame(venc: &Venc, buf: &mut DmaBuf, fmt: Fmt, f: u32, dts: i64, rng: &mut Rng, hook: impl FnMut(u32) -> bool) {
    capture_frame(venc, buf, fmt, f, dts, rng, true, hook)
}

/// `gate = false` is the positive control: the same simulated capture, but
/// the frame is submitted as an ordinary (tracked) one, so the encoder reads
/// the poison.
#[allow(clippy::too_many_arguments)]
fn capture_frame(venc: &Venc, buf: &mut DmaBuf, fmt: Fmt, f: u32, dts: i64, rng: &mut Rng, gate: bool, mut hook: impl FnMut(u32) -> bool) {
    write_rows(buf, fmt, f, 0, R0, None);
    write_rows(buf, fmt, f, R0, H, Some(rng));
    let input = if gate { InputMode::Progressive { ready_rows: R0 } } else { InputMode::Tracked };
    venc.put_frame(&desc(fmt, buf, dts, input)).expect("put_frame progressive");
    sleep_ms(2);
    let mut r = R0;
    while r < H {
        let r2 = (r + STEP).min(H);
        write_rows(buf, fmt, f, r, r2, None);
        if gate {
            let ok = if r2 == H { venc.complete(dts) } else { venc.progress(dts, r2) }.expect("progress");
            assert!(ok, "progress for dts {dts}: frame gone");
        }
        r = r2;
        if !hook(r) {
            return;
        }
        sleep_ms(1);
    }
}

fn shadow_frame(venc: &Venc, buf: &mut DmaBuf, fmt: Fmt, f: u32, dts: i64) -> Rec {
    write_rows(buf, fmt, f, 0, H, None);
    venc.put_frame(&desc(fmt, buf, dts, InputMode::Tracked)).expect("put_frame shadow");
    wait_rec(venc, 1000).expect("shadow packet")
}

struct Out {
    gated: File,
    shadow: File,
}

fn report(tag: &str, r: &Rec) {
    println!(
        "{tag} dts {:3} {:?}{}{} len {:7} rows_start {:4} tail_us {:5}",
        r.dts,
        r.status,
        if r.discard { " DISCARD" } else { "" },
        if r.intra { " INTRA" } else { "" },
        r.data.len(),
        r.rows_start,
        r.tail_us
    );
}

// ---- tests ---------------------------------------------------------------------

fn test_gate(kmpp: &Kmpp, coding: Coding, fmt: Fmt, n: u32, out: &mut Out, gate: bool) -> bool {
    let g = create(kmpp, coding, fmt);
    let s = create(kmpp, coding, fmt);
    let mut gbuf = DmaBuf::new(fmt.size());
    let mut sbuf = DmaBuf::new(fmt.size());
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let mut ok = true;
    let (mut same, mut tails) = (0, Vec::new());
    for f in 0..n {
        let sr = shadow_frame(&s, &mut sbuf, fmt, f, f as i64);
        capture_frame(&g, &mut gbuf, fmt, f, f as i64, &mut rng, gate, |_| true);
        let gr = wait_rec(&g, 1000).expect("gated packet");
        let eq = gr.data == sr.data;
        let gated_start = gr.rows_start < H || !gate;
        report(if eq { "SAME" } else { "DIFF" }, &gr);
        if gr.dts != f as i64 || gr.discard || gr.status != InputStatus::Encoded || !gated_start {
            println!("  FAIL: record dts/status/gating unexpected");
            ok = false;
        }
        if eq {
            same += 1;
        } else {
            ok = false;
        }
        tails.push(gr.tail_us);
        out.gated.write_all(&gr.data).unwrap();
        out.shadow.write_all(&sr.data).unwrap();
    }
    if !gate {
        // positive control: the poison must have reached the encoder
        println!("control: {same}/{n} frames identical to the shadow (expected: few or none)");
        return same < n / 2;
    }
    tails.sort();
    println!(
        "gate: {same}/{n} frames identical to the shadow; tail_us median {} max {}",
        tails[tails.len() / 2],
        tails[tails.len() - 1]
    );
    ok
}

fn test_abort(kmpp: &Kmpp, coding: Coding, fmt: Fmt, n: u32, out: &mut Out) -> bool {
    let g = create(kmpp, coding, fmt);
    let s = create(kmpp, coding, fmt);
    let mut gbuf = [DmaBuf::new(fmt.size()), DmaBuf::new(fmt.size())];
    let mut sbuf = DmaBuf::new(fmt.size());
    let mut rng = Rng(0x2545f4914f6cdd1d);
    let mut ok = true;
    let mut check = |cond: bool, what: &str| {
        if !cond {
            println!("  FAIL: {what}");
            ok = false;
        }
    };
    let n = n.max(30);
    let mut f = 0;
    while f < n {
        let dts = f as i64;
        match f {
            5 => {
                // frame 5 holds the encoder; frame 6 queued behind it is aborted
                let [b5, b6] = &mut gbuf;
                write_rows(b5, fmt, 5, 0, R0, None);
                write_rows(b5, fmt, 5, R0, H, Some(&mut rng));
                g.put_frame(&desc(fmt, b5, 5, InputMode::Progressive { ready_rows: R0 })).unwrap();
                sleep_ms(3);
                write_rows(b6, fmt, 6, 0, R0, None);
                g.put_frame(&desc(fmt, b6, 6, InputMode::Progressive { ready_rows: R0 })).unwrap();
                let t = Instant::now();
                check(g.abort(6).unwrap(), "abort of queued frame 6 returned false");
                let r6 = wait_rec(&g, 100).expect("record 6");
                report("ABRT", &r6);
                println!("  queued abort record after {} us", t.elapsed().as_micros());
                check(r6.dts == 6 && r6.discard && r6.status == InputStatus::Aborted, "frame 6 record");
                check(!g.abort(6).unwrap(), "second abort of 6 should be ENOENT");
                let mut r = R0;
                while r < H {
                    let r2 = (r + STEP).min(H);
                    write_rows(b5, fmt, 5, r, r2, None);
                    if r2 == H { g.complete(5) } else { g.progress(5, r2) }.unwrap();
                    r = r2;
                }
                let r5 = wait_rec(&g, 1000).expect("record 5");
                report("GATE", &r5);
                check(r5.dts == 5 && !r5.discard && r5.status == InputStatus::Encoded, "frame 5 record");
                out.gated.write_all(&r5.data).unwrap();
                out.shadow.write_all(&shadow_frame(&s, &mut sbuf, fmt, 5, 5).data).unwrap();
                f = 7;
                continue;
            }
            10 | 15 => {
                // running abort: a P frame (10) and an IDR (15)
                if f == 15 {
                    g.request_idr().unwrap();
                    s.request_idr().unwrap();
                }
                let b = &mut gbuf[0];
                write_rows(b, fmt, f, 0, R0, None);
                write_rows(b, fmt, f, R0, H, Some(&mut rng));
                g.put_frame(&desc(fmt, b, dts, InputMode::Progressive { ready_rows: R0 })).unwrap();
                sleep_ms(5);
                check(g.abort(dts).unwrap(), "running abort returned false");
                let r = wait_rec(&g, 200).expect("aborted record");
                report("ABRT", &r);
                check(r.dts == dts && r.discard && r.status == InputStatus::Aborted, "running abort record");
                // the next frame: P after an aborted P (rollback), IDR after an aborted IDR
                f += 1;
                gated_frame(&g, &mut gbuf[1], fmt, f, f as i64, &mut rng, |_| true);
                let rn = wait_rec(&g, 1000).expect("frame after abort");
                report("GATE", &rn);
                let want_intra = dts == 15;
                check(rn.intra == want_intra, if want_intra { "frame after aborted IDR is not intra" } else { "frame after aborted P is intra (no rollback)" });
                out.gated.write_all(&rn.data).unwrap();
                out.shadow.write_all(&shadow_frame(&s, &mut sbuf, fmt, f, f as i64).data).unwrap();
            }
            20 => {
                // control calls during a hold must not wait for the frame
                let mut lat = Vec::new();
                gated_frame(&g, &mut gbuf[0], fmt, f, dts, &mut rng, |rows| {
                    if rows == R0 + STEP * 4 {
                        let t = Instant::now();
                        g.request_idr().unwrap();
                        let cfg = g.new_cfg().unwrap();
                        cfg.set_s32("rc:bps_target", 8_000_000).unwrap();
                        cfg.set_s32("rc:bps_max", 8_500_000).unwrap();
                        cfg.set_s32("rc:bps_min", 7_500_000).unwrap();
                        g.set_cfg(&cfg).unwrap();
                        lat.push(t.elapsed().as_micros());
                    }
                    true
                });
                s.request_idr().unwrap();
                let r = wait_rec(&g, 1000).expect("record 20");
                report("GATE", &r);
                println!("  request_idr + set_cfg during the hold took {:?} us", lat);
                check(lat.iter().all(|&us| us < 5000), "control call during a hold took >= 5 ms");
                check(r.status == InputStatus::Encoded && !r.discard, "frame 20 record");
                out.gated.write_all(&r.data).unwrap();
                out.shadow.write_all(&shadow_frame(&s, &mut sbuf, fmt, f, dts).data).unwrap();
            }
            25 => {
                // an encoder-side drop of a tracked frame (smaller than the config)
                let b = &mut gbuf[0];
                write_rows(b, fmt, f, 0, H, None);
                let mut d = desc(fmt, b, dts, InputMode::Tracked);
                d.height = H - 16;
                g.put_frame(&d).unwrap();
                let r = wait_rec(&g, 500).expect("record 25");
                report("DROP", &r);
                check(r.dts == dts && r.discard && r.status == InputStatus::Dropped, "dropped frame record");
            }
            _ => {
                gated_frame(&g, &mut gbuf[0], fmt, f, dts, &mut rng, |_| true);
                let r = wait_rec(&g, 1000).expect("gated packet");
                report("GATE", &r);
                check(r.dts == dts && !r.discard && r.status == InputStatus::Encoded, "normal record");
                out.gated.write_all(&r.data).unwrap();
                out.shadow.write_all(&shadow_frame(&s, &mut sbuf, fmt, f, dts).data).unwrap();
            }
        }
        f += 1;
    }
    // nothing left over: every tracked frame had exactly one record
    check(wait_rec(&g, 50).is_none(), "extra record");
    println!("abort: done (decode gated.bin and shadow.bin on the host)");
    ok
}

fn test_stall(kmpp: &Kmpp, coding: Coding, fmt: Fmt, out: &mut Out) -> bool {
    let g = create(kmpp, coding, fmt);
    let mut buf = DmaBuf::new(fmt.size());
    let mut rng = Rng(7);
    let mut ok = true;
    for f in 0..3 {
        gated_frame(&g, &mut buf, fmt, f, f as i64, &mut rng, |_| true);
        let r = wait_rec(&g, 1000).unwrap();
        out.gated.write_all(&r.data).unwrap();
    }
    write_rows(&mut buf, fmt, 3, 0, R0, None);
    write_rows(&mut buf, fmt, 3, R0, H, Some(&mut rng));
    let t = Instant::now();
    g.put_frame(&desc(fmt, &buf, 3, InputMode::Progressive { ready_rows: R0 })).unwrap();
    let r = wait_rec(&g, 2000).expect("stalled record");
    let ms = r.at.duration_since(t).as_millis();
    report("STAL", &r);
    println!("stall: record after {ms} ms");
    if !(r.discard && r.status == InputStatus::Stalled && (150..400).contains(&ms)) {
        println!("  FAIL: expected a STALLED discard after ~200 ms");
        ok = false;
    }
    check_late_update(&g, 3, &mut ok);
    for f in 4..7 {
        gated_frame(&g, &mut buf, fmt, f, f as i64, &mut rng, |_| true);
        let r = wait_rec(&g, 1000).unwrap();
        report("GATE", &r);
        if r.status != InputStatus::Encoded || (f == 4 && r.intra) {
            println!("  FAIL: frame after the stall (intra means no rollback)");
            ok = false;
        }
        out.gated.write_all(&r.data).unwrap();
    }
    ok
}

fn check_late_update(g: &Venc, dts: i64, ok: &mut bool) {
    if g.progress(dts, H).unwrap() {
        println!("  FAIL: update after the record was accepted");
        *ok = false;
    }
}

fn test_close(kmpp: &Kmpp, coding: Coding, fmt: Fmt, out: &mut Out) -> bool {
    let mut buf = DmaBuf::new(fmt.size());
    let mut rng = Rng(11);
    let mut ok = true;
    {
        let g = create(kmpp, coding, fmt);
        write_rows(&mut buf, fmt, 0, 0, R0, None);
        write_rows(&mut buf, fmt, 0, R0, H, Some(&mut rng));
        g.put_frame(&desc(fmt, &buf, 0, InputMode::Progressive { ready_rows: R0 })).unwrap();
        sleep_ms(20);
        let t = Instant::now();
        drop(g);
        let ms = t.elapsed().as_millis();
        println!("close: channel closed during a hold in {ms} ms");
        if ms > 100 {
            println!("  FAIL: close waited for the hold");
            ok = false;
        }
    }
    let g = create(kmpp, coding, fmt);
    for f in 0..3 {
        gated_frame(&g, &mut buf, fmt, f, f as i64, &mut rng, |_| true);
        let r = wait_rec(&g, 1000).expect("packet after reopen");
        report("GATE", &r);
        if r.status != InputStatus::Encoded {
            ok = false;
        }
        out.gated.write_all(&r.data).unwrap();
    }
    ok
}

/// Advance a submitted progressive frame from R0 to complete, STEP rows per ms.
fn advance(venc: &Venc, buf: &mut DmaBuf, fmt: Fmt, f: u32, dts: i64) {
    let mut r = R0;
    while r < H {
        let r2 = (r + STEP).min(H);
        write_rows(buf, fmt, f, r, r2, None);
        let ok = if r2 == H { venc.complete(dts) } else { venc.progress(dts, r2) }.expect("progress");
        assert!(ok, "progress for dts {dts}: frame gone");
        r = r2;
        sleep_ms(1);
    }
}

/// Submit frame `f` progressively (poison below R0), without advancing.
fn submit_held(venc: &Venc, buf: &mut DmaBuf, fmt: Fmt, f: u32, rng: &mut Rng) {
    write_rows(buf, fmt, f, 0, R0, None);
    write_rows(buf, fmt, f, R0, H, Some(rng));
    venc.put_frame(&desc(fmt, buf, f as i64, InputMode::Progressive { ready_rows: R0 })).expect("put_frame progressive");
}

/// One gated frame and its shadow; the packets must match.
#[allow(clippy::too_many_arguments)]
fn normal_frame(g: &Venc, s: &Venc, gbuf: &mut DmaBuf, sbuf: &mut DmaBuf, fmt: Fmt, f: u32, rng: &mut Rng, out: &mut Out, check: &mut dyn FnMut(bool, &str)) {
    gated_frame(g, gbuf, fmt, f, f as i64, rng, |_| true);
    let r = wait_rec(g, 1000).expect("gated packet");
    report("GATE", &r);
    let sr = shadow_frame(s, sbuf, fmt, f, f as i64);
    check(r.dts == f as i64 && r.status == InputStatus::Encoded && !r.discard, "normal record");
    check(r.data == sr.data, "differs from the shadow");
    out.gated.write_all(&r.data).unwrap();
    out.shadow.write_all(&sr.data).unwrap();
}

/// STOP during a hold. STOP does not wait for the hardware: the running
/// frame keeps its gate and ends normally when advanced; a frame queued
/// behind it stays queued (and keeps taking updates) until START. A hold
/// nobody advances while stopped still ends STALLED.
fn test_stop(kmpp: &Kmpp, coding: Coding, fmt: Fmt, out: &mut Out) -> bool {
    let g = create(kmpp, coding, fmt);
    let s = create(kmpp, coding, fmt);
    let mut gbuf = [DmaBuf::new(fmt.size()), DmaBuf::new(fmt.size())];
    let mut sbuf = DmaBuf::new(fmt.size());
    let mut rng = Rng(0x5eed);
    let mut ok = true;
    let mut check = |cond: bool, what: &str| {
        if !cond {
            println!("  FAIL: {what}");
            ok = false;
        }
    };
    for f in 0..3 {
        normal_frame(&g, &s, &mut gbuf[0], &mut sbuf, fmt, f, &mut rng, out, &mut check);
    }

    // 1: frame 3 running and held, frame 4 queued; STOP; advance both; START
    {
        let [b3, b4] = &mut gbuf;
        submit_held(&g, b3, fmt, 3, &mut rng);
        sleep_ms(3);
        submit_held(&g, b4, fmt, 4, &mut rng);
        let t = Instant::now();
        g.stop().expect("stop");
        println!("stop: STOP during a hold returned in {} us", t.elapsed().as_micros());
        check(t.elapsed() < Duration::from_millis(5), "STOP waited for the hold");
        advance(&g, b3, fmt, 3, 3);
        let r3 = wait_rec(&g, 500);
        match &r3 {
            Some(r) => {
                report("STOP", r);
                check(r.dts == 3 && r.status == InputStatus::Encoded && !r.discard, "held frame record after STOP");
                let sr = shadow_frame(&s, &mut sbuf, fmt, 3, 3);
                check(r.data == sr.data, "held frame differs from the shadow");
                out.gated.write_all(&r.data).unwrap();
                out.shadow.write_all(&sr.data).unwrap();
            }
            None => check(false, "no record for the held frame after STOP"),
        }
        advance(&g, b4, fmt, 4, 4);
        let early = wait_rec(&g, 50);
        if let Some(r) = &early {
            report("EARLY", r);
        }
        check(early.is_none(), "queued frame encoded while stopped");
        let t = Instant::now();
        g.start().expect("start");
        let r4 = wait_rec(&g, 500);
        match &r4 {
            Some(r) => {
                report("STRT", r);
                println!("  queued frame record {} us after START", r.at.duration_since(t).as_micros());
                check(r.dts == 4 && r.status == InputStatus::Encoded && !r.discard, "queued frame record after START");
                check(r.rows_start >= H, "queued frame was complete at START but rows_start < height");
                let sr = shadow_frame(&s, &mut sbuf, fmt, 4, 4);
                check(r.data == sr.data, "queued frame differs from the shadow");
                out.gated.write_all(&r.data).unwrap();
                out.shadow.write_all(&sr.data).unwrap();
            }
            None => check(false, "no record for the queued frame after START"),
        }
    }
    for f in 5..8 {
        normal_frame(&g, &s, &mut gbuf[0], &mut sbuf, fmt, f, &mut rng, out, &mut check);
    }

    // 2: frame 8 held, STOP, nobody advances: STALLED, then START and go on
    submit_held(&g, &mut gbuf[0], fmt, 8, &mut rng);
    sleep_ms(3);
    let t = Instant::now();
    g.stop().expect("stop");
    let r = wait_rec(&g, 1000);
    match &r {
        Some(r) => {
            report("STAL", r);
            let ms = r.at.duration_since(t).as_millis();
            println!("  stopped hold ended after {ms} ms");
            check(r.dts == 8 && r.discard && r.status == InputStatus::Stalled, "stopped hold record");
            check((150..400).contains(&ms), "stopped hold record not delivered ~200 ms after STOP");
        }
        None => check(false, "no record for the stopped hold"),
    }
    g.start().expect("start");
    // after a discarded P frame the references roll back but rate control
    // does not, so the bytes differ from the shadow (drift is checked by
    // PSNR in the abort test); here: encoded, P (rollback, no forced IDR)
    for f in 9..12 {
        gated_frame(&g, &mut gbuf[0], fmt, f, f as i64, &mut rng, |_| true);
        let r = wait_rec(&g, 1000).expect("gated packet");
        report("GATE", &r);
        check(r.dts == f as i64 && r.status == InputStatus::Encoded && !r.discard && !r.intra, "frame after the stopped stall");
        out.gated.write_all(&r.data).unwrap();
    }
    check(wait_rec(&g, 50).is_none(), "extra record");
    ok
}

fn jpeg_create(kmpp: &Kmpp, fmt: Fmt) -> Venc {
    let venc = Venc::create(kmpp, &VencInit::new(Coding::Mjpeg, W, H)).expect("create jpeg");
    let cfg = venc.new_cfg().unwrap();
    let c = || -> rk_kmpp::Result<()> {
        cfg.set_s32("codec:type", Coding::Mjpeg as i32)?;
        cfg.set_s32("prep:width", W as i32)?;
        cfg.set_s32("prep:height", H as i32)?;
        cfg.set_s32("prep:hor_stride", fmt.stride() as i32)?;
        cfg.set_s32("prep:ver_stride", H as i32)?;
        cfg.set_s32("prep:format", fmt.mpp() as i32)?;
        cfg.set_s32("rc:mode", RcMode::FixQp as i32)?;
        cfg.set_s32("jpeg:q_factor", 90)?;
        venc.set_cfg(&cfg)
    };
    c().expect("configure jpeg");
    venc
}

/// A JPEG channel (the server's screenshot path) next to a hold: tracked JPEG
/// input is rejected, and a JPEG frame submitted while an H.26x frame holds
/// the encoder is encoded correctly once the hold ends (same output as
/// without a hold). Reports how long the JPEG waited.
fn test_jpeg(kmpp: &Kmpp, coding: Coding, fmt: Fmt, out: &mut Out) -> bool {
    let g = create(kmpp, coding, fmt);
    let j = jpeg_create(kmpp, fmt);
    let mut gbuf = DmaBuf::new(fmt.size());
    let mut jbuf = DmaBuf::new(fmt.size());
    let mut rng = Rng(0x1bad);
    let mut ok = true;
    let mut check = |cond: bool, what: &str| {
        if !cond {
            println!("  FAIL: {what}");
            ok = false;
        }
    };
    write_rows(&mut jbuf, fmt, 99, 0, H, None);
    let jdesc = desc(fmt, &jbuf, 0, InputMode::Complete);
    let mut td = desc(fmt, &jbuf, 0, InputMode::Tracked);
    td.dts = 1000;
    let rej = j.put_frame(&td);
    println!("jpeg: tracked JPEG frame -> {:?}", rej.as_ref().err());
    check(matches!(rej, Err(rk_kmpp::Error::InputRejected)), "tracked JPEG frame not rejected");

    let jpeg_once = |j: &Venc| -> (Vec<u8>, u128) {
        let t = Instant::now();
        j.put_frame(&jdesc).expect("put jpeg");
        let r = wait_rec(j, 1000).expect("jpeg packet");
        (r.data, r.at.duration_since(t).as_micros())
    };
    let (ref_jpeg, us) = jpeg_once(&j);
    println!("jpeg: alone {} bytes in {us} us", ref_jpeg.len());
    out.shadow.write_all(&ref_jpeg).unwrap();
    let mut waits = Vec::new();
    for f in 0..6 {
        // H.26x frame f held at R0; the JPEG goes in 3 ms into the hold
        submit_held(&g, &mut gbuf, fmt, f, &mut rng);
        sleep_ms(3);
        let tj = Instant::now();
        j.put_frame(&jdesc).expect("put jpeg during hold");
        let mut jrec = None;
        let mut r = R0;
        while r < H {
            let r2 = (r + STEP).min(H);
            write_rows(&mut gbuf, fmt, f, r, r2, None);
            let ok = if r2 == H { g.complete(f as i64) } else { g.progress(f as i64, r2) }.unwrap();
            assert!(ok);
            r = r2;
            if jrec.is_none() {
                jrec = wait_rec(&j, 0);
            }
            sleep_ms(1);
        }
        let t_complete = Instant::now();
        let gr = wait_rec(&g, 1000).expect("gated packet");
        let jr = jrec.or_else(|| wait_rec(&j, 1000)).expect("jpeg packet");
        report("GATE", &gr);
        let jw = jr.at.duration_since(tj).as_micros();
        let rel = jr.at.duration_since(t_complete.min(jr.at)).as_micros() as i128
            - t_complete.duration_since(t_complete.min(jr.at)).as_micros() as i128;
        println!(
            "  jpeg: {} bytes {}, waited {jw} us, {} us {} the H.26x complete",
            jr.data.len(),
            if jr.data == ref_jpeg { "SAME" } else { "DIFF" },
            rel.abs(),
            if rel >= 0 { "after" } else { "before" }
        );
        check(gr.dts == f as i64 && gr.status == InputStatus::Encoded && !gr.discard, "gated record");
        check(jr.data == ref_jpeg, "JPEG differs from the one encoded alone");
        waits.push(jw);
        out.gated.write_all(&gr.data).unwrap();
    }
    // and the other order: JPEG first, then a progressive frame right behind it
    for f in 6..9 {
        j.put_frame(&jdesc).expect("put jpeg");
        gated_frame(&g, &mut gbuf, fmt, f, f as i64, &mut rng, |_| true);
        let jr = wait_rec(&j, 1000).expect("jpeg packet");
        let gr = wait_rec(&g, 1000).expect("gated packet");
        report("GATE", &gr);
        check(gr.status == InputStatus::Encoded && !gr.discard, "gated record after a JPEG");
        check(jr.data == ref_jpeg, "JPEG differs (JPEG first)");
        out.gated.write_all(&gr.data).unwrap();
    }
    waits.sort();
    println!("jpeg: wait during a hold median {} us max {} us", waits[waits.len() / 2], waits[waits.len() - 1]);
    ok
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let test = a.get(1).map(String::as_str).unwrap_or("gate");
    let coding = match a.get(2).map(String::as_str).unwrap_or("h264") {
        "h265" => Coding::Hevc,
        _ => Coding::Avc,
    };
    let fmt = match a.get(3).map(String::as_str).unwrap_or("nv12") {
        "bgr24" => Fmt::Bgr24,
        "yuv444" => Fmt::Yuv444,
        _ => Fmt::Nv12,
    };
    let n: u32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(30);
    let dir = a.get(5).cloned().unwrap_or_else(|| ".".into());
    let kmpp = Kmpp::open().expect("open kmpp");
    let mut out = Out {
        gated: File::create(format!("{dir}/gated.bin")).unwrap(),
        shadow: File::create(format!("{dir}/shadow.bin")).unwrap(),
    };
    let ok = match test {
        "gate" => test_gate(&kmpp, coding, fmt, n, &mut out, true),
        "control" => test_gate(&kmpp, coding, fmt, n, &mut out, false),
        "abort" => test_abort(&kmpp, coding, fmt, n, &mut out),
        "stall" => test_stall(&kmpp, coding, fmt, &mut out),
        "close" => test_close(&kmpp, coding, fmt, &mut out),
        "stop" => test_stop(&kmpp, coding, fmt, &mut out),
        "jpeg" => test_jpeg(&kmpp, coding, fmt, &mut out),
        _ => {
            eprintln!("usage: prog_test <gate|control|abort|stall|close|stop|jpeg> [h264|h265] [nv12|bgr24|yuv444] [frames] [outdir]");
            std::process::exit(2);
        }
    };
    println!("RESULT {test} {:?} {}: {}", coding, a.get(3).map(String::as_str).unwrap_or("nv12"), if ok { "PASS" } else { "FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}
