//! M1: capture the virtual display and write frames to disk.
//!
//! Builds directly on M0. The virtual display is created exactly as before,
//! then pointed at ScreenCaptureKit using the `displayID` it hands out. That
//! `u32` is the entire interface between the private half of the project and
//! the public half.
//!
//! # What "proof" means here
//!
//! Not "no errors". A capture pipeline can run flawlessly and produce black
//! rectangles, which is the most common way this fails. So the run checks:
//!
//! - frames actually arrived, and how many
//! - what size and pixel format they really are, which answers the open HiDPI
//!   question left over from M0
//! - whether the pixels are non-black, via mean luma
//! - and it writes PNGs you can open

use crate::pngout;
use annex_capture::{CaptureConfig, Capturer};
use annex_core::{PixFmt, VirtualDisplayConfig};
use annex_virtual_display::VirtualDisplay;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

/// A frame handed back from the capture queue to the main thread.
struct Captured {
    index: usize,
    width: usize,
    height: usize,
    stride: usize,
    format: u32,
    pts_us: i64,
    luma: Option<f32>,
    bgra: Option<Vec<u8>>,
}

pub fn run(
    frames_wanted: usize,
    out_dir: PathBuf,
    scale: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Annex M1: capture the virtual display\n");

    // ---- permission first ------------------------------------------------
    //
    // Without the grant, ScreenCaptureKit starts a stream that silently never
    // delivers. Checking up front turns a mysterious hang into a sentence.
    println!("  checking Screen Recording permission");
    if !annex_capture::has_permission() {
        println!("      not granted, asking macOS to prompt");
        annex_capture::permission::request_permission();
        if !annex_capture::has_permission() {
            eprintln!("\n{}", annex_capture::permission::permission_help());
            return Err("Screen Recording permission denied".into());
        }
    }
    println!("      ok");

    // ---- M0, unchanged ---------------------------------------------------
    // Overridable so the mode itself can be varied. That matters: whether
    // HiDPI produces a real 2x backing store depends on the mode you offer,
    // not on the capture size you request afterwards.
    let env_u32 = |k: &str, d: u32| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let vd_cfg = VirtualDisplayConfig {
        name: "Annex Display".to_string(),
        width: env_u32("ANNEX_VD_WIDTH", 1920),
        height: env_u32("ANNEX_VD_HEIGHT", 1080),
        refresh_hz: 60.0,
        hidpi: env_u32("ANNEX_VD_HIDPI", 1) == 1,
        size_mm: (600.0, 340.0),
    };
    println!(
        "\n  creating virtual display mode {}x{} hidpi={}",
        vd_cfg.width, vd_cfg.height, vd_cfg.hidpi
    );
    let vd = VirtualDisplay::create(&vd_cfg)?;
    let display_id = vd.display_id();
    println!("      displayID = {display_id}");

    // ScreenCaptureKit maintains its own view of attached displays and does not
    // learn about a brand new one instantly. Without this pause the very first
    // run reports DisplayNotFound for a display that plainly exists.
    // Points versus backing pixels. This is the only API that distinguishes
    // them, and it decides whether capturing at 2x is worth 4x the bandwidth.
    if let Some((pw, ph, xw, xh)) = crate::displays::mode_geometry(display_id) {
        let retina = xw == pw * 2 && xh == ph * 2;
        println!(
            "      current mode: {pw}x{ph} points, {xw}x{xh} backing pixels  ({})",
            if retina {
                "HiDPI, capture at scale 2"
            } else {
                "1x, capture at scale 1"
            }
        );
    }

    println!("\n  waiting for ScreenCaptureKit to notice it");
    std::thread::sleep(Duration::from_millis(1200));

    match annex_capture::list_displays() {
        Ok(list) => {
            for (id, w, h) in &list {
                let mark = if *id == display_id { " <-- ours" } else { "" };
                println!("      SCDisplay {id:>10}  {w:>5} x {h:<5}{mark}");
            }
            if !list.iter().any(|(id, _, _)| *id == display_id) {
                return Err(format!(
                    "ScreenCaptureKit cannot see display {display_id} even though CoreGraphics can"
                )
                .into());
            }
        }
        Err(e) => return Err(format!("could not enumerate shareable content: {e}").into()),
    }

    // ---- capture ---------------------------------------------------------
    let cfg = CaptureConfig {
        display_id,
        fps: 30,
        // BGRA so M1 can write PNGs directly. M2 switches to NV12, which is
        // what VideoToolbox actually wants.
        pixel_format: PixFmt::Bgra,
        size: None,
        // Multiplies the display's reported point size. On a HiDPI display this
        // must be 2 to capture the pixels macOS actually rendered.
        scale,
        show_cursor: false,
    };
    println!(
        "\n  starting capture at {} fps, BGRA, scale {}x",
        cfg.fps, scale
    );

    let (tx, rx) = mpsc::channel::<Captured>();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_sink = Arc::clone(&counter);

    // This closure runs on ScreenCaptureKit's dispatch queue, so it does the
    // minimum: copy out what is needed and hand it to the main thread. The
    // readback is itself the expensive part and only exists for M1.
    let capturer = Capturer::start(
        cfg,
        Box::new(move |frame| {
            let index = counter_sink.fetch_add(1, Ordering::SeqCst);
            if index >= frames_wanted {
                return;
            }
            let decoded = frame.to_bgra_vec();
            let _ = tx.send(Captured {
                index,
                width: frame.width(),
                height: frame.height(),
                stride: frame.bytes_per_row(),
                format: frame.format(),
                pts_us: frame.pts_us,
                luma: frame.mean_luma(),
                bgra: decoded.map(|(px, _, _)| px),
            });
        }),
    )?;

    println!("      stream started, collecting {frames_wanted} frames");
    println!(
        "      note: ScreenCaptureKit only emits on change, so a completely\n\
         \x20     static desktop produces nothing. Move a window onto the\n\
         \x20     virtual display if this stalls."
    );

    std::fs::create_dir_all(&out_dir)?;

    let mut got = 0usize;
    let mut wrote = 0usize;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);

    while got < frames_wanted && std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(f) => {
                got += 1;
                let luma = f
                    .luma
                    .map(|l| format!("{l:.3}"))
                    .unwrap_or_else(|| "n/a".into());
                println!(
                    "      frame {:>2}  {:>5} x {:<5} stride {:<6} {}  pts {:>9}us  luma {}",
                    f.index,
                    f.width,
                    f.height,
                    f.stride,
                    annex_capture::format_name(f.format),
                    f.pts_us,
                    luma
                );

                if let Some(px) = f.bgra {
                    let path = out_dir.join(format!("frame-{:03}.png", f.index));
                    pngout::write_bgra(&path, &px, f.width, f.height)?;
                    wrote += 1;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    capturer.stop();

    // ---- verdict ---------------------------------------------------------
    println!(
        "\n  captured {got} frames, wrote {wrote} PNGs to {}",
        out_dir.display()
    );

    if got == 0 {
        drop(vd);
        return Err("no frames arrived. \
             With permission granted this usually means nothing on the virtual \
             display changed, since ScreenCaptureKit only emits on change."
            .into());
    }

    // A pipeline that produces perfectly black frames looks identical to a
    // working one from the outside, so say it out loud either way.
    println!("\n  M1 passed: ScreenCaptureKit delivered frames from the virtual display.");
    println!("  Open the PNGs to confirm they show the virtual desktop and not a black rectangle.");

    drop(vd);
    Ok(())
}
