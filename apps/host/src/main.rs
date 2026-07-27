//! The Annex host binary.
//!
//! # Thread layout, which is the whole reason this file is fiddly
//!
//! Apple's frameworks and Rust's async runtime both want to own a thread, and
//! they cannot share one.
//!
//! ScreenCaptureKit, AppKit and the virtual display callbacks all need a thread
//! running a Core Foundation run loop, which in practice means the main thread.
//! Once `NSApplication` takes it over, that thread blocks in the run loop and
//! never returns.
//!
//! So the ordering, once there is more here than M0, is not stylistic. Start
//! tokio on background threads *first*, keep a handle to it, then hand the main
//! thread to the run loop and let it block. The two halves talk over bounded
//! channels: encoded samples flow out from the capture queue to tokio, and
//! phase-2 input events flow back from tokio to the main thread for injection.
//!
//! The one rule that prevents most of the crashes here: never touch AppKit off
//! the main thread.
//!
//! # What this currently does
//!
//! Two milestones, selected by the first argument.
//!
//! - `m0` (default): the virtual-display spike. Probes for the private classes,
//!   takes a census of active displays, creates the virtual display, takes
//!   another census to prove it appeared, waits so you can look at it, then
//!   drops it and takes a third census to prove it went away.
//! - `m1`: creates the virtual display, points ScreenCaptureKit at it, and
//!   writes captured frames to PNG.
//! - `m2`: the same, but encodes to H.264 with VideoToolbox and writes an
//!   Annex-B elementary stream. This is the first zero-copy path: the pixel
//!   buffer goes capture to encoder without ever reaching the CPU.
//! - `m3`: serves the whole thing over WebRTC to a browser. Uses the main
//!   display by default so the network path is proven independently of the
//!   private API; `m3 virtual` uses the virtual display, which is M4.
//!
//! ```text
//! cargo run -p annex-host              # M0, 20 second hold
//! cargo run -p annex-host -- 60        # M0, 60 second hold
//! cargo run -p annex-host -- m1        # M1, 10 frames to ./captures
//! cargo run -p annex-host -- m1 30     # M1, 30 frames
//! cargo run -p annex-host -- m1 4 2    # M1, 4 frames at 2x (HiDPI probe)
//! cargo run -p annex-host -- m2 60     # M2, 60 frames to out.h264
//! cargo run -p annex-host -- m3        # M3, stream the main display
//! cargo run -p annex-host -- m3 virtual # M4, stream the virtual display
//! ```

mod displays;
mod m1;
mod m2;
mod m3;
mod pngout;
mod tray;

use annex_core::VirtualDisplayConfig;
use annex_virtual_display::VirtualDisplay;
use std::time::Duration;

/// How long to hold the display up so it can be inspected. Override with the
/// first CLI argument.
const DEFAULT_HOLD_SECS: u64 = 20;

fn main() {
    let arg1 = std::env::args().nth(1).unwrap_or_default();

    if arg1 == "m1" {
        let frames = std::env::args()
            .nth(2)
            .and_then(|a| a.parse().ok())
            .unwrap_or(10);
        let scale = std::env::args()
            .nth(3)
            .and_then(|a| a.parse().ok())
            .unwrap_or(1);
        let out = std::path::PathBuf::from(if scale > 1 { "captures-2x" } else { "captures" });
        if let Err(e) = m1::run(frames, out, scale) {
            eprintln!("\n  M1 failed: {e}");
            std::process::exit(1);
        }
        return;
    }

    if arg1 == "m2" {
        let frames = std::env::args()
            .nth(2)
            .and_then(|a| a.parse().ok())
            .unwrap_or(60);
        let out = std::path::PathBuf::from("out.h264");
        if let Err(e) = m2::run(frames, out) {
            eprintln!("\n  M2 failed: {e}");
            std::process::exit(1);
        }
        return;
    }

    if arg1 == "m3" {
        let arg2 = std::env::args().nth(2).unwrap_or_default();
        let use_virtual = arg2 == "virtual";
        let port = arg2.parse().unwrap_or(8787);
        if let Err(e) = m3::run(use_virtual, port) {
            eprintln!("\n  M3 failed: {e}");
            std::process::exit(1);
        }
        return;
    }

    let hold = arg1
        .parse()
        .ok()
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_HOLD_SECS));

    println!("Annex M0: virtual-display spike\n");

    // ---- step 1: is the private API even here? --------------------------
    //
    // The cheapest possible answer to the project's largest risk. Doing this
    // first means a macOS version that moved these classes fails in one line
    // with a clear message, rather than somewhere deep in the creation sequence.
    println!("  probing private CoreGraphics classes");
    for (name, found) in annex_virtual_display::availability() {
        println!("      {} {}", if found { "ok  " } else { "MISS" }, name);
    }
    if !annex_virtual_display::is_available() {
        eprintln!("\n  the private virtual-display API is not present on this macOS build.");
        eprintln!("  M0 cannot proceed. The dummy-plug fallback in section 13 applies.");
        std::process::exit(1);
    }

    // ---- step 2: census before ------------------------------------------
    let before = displays::active();
    println!("\n  active displays before ({}):", before.len());
    println!("{}", displays::render(&before, None));

    // ---- step 3: create --------------------------------------------------
    let cfg = VirtualDisplayConfig {
        name: "Annex Display".to_string(),
        width: 1920,
        height: 1080,
        refresh_hz: 60.0,
        hidpi: true,
        size_mm: (600.0, 340.0),
    };
    println!(
        "\n  creating \"{}\" at {}x{} @ {}Hz, hidpi={}",
        cfg.name, cfg.width, cfg.height, cfg.refresh_hz, cfg.hidpi
    );

    let vd = match VirtualDisplay::create(&cfg) {
        Ok(vd) => vd,
        Err(e) => {
            eprintln!("\n  create failed: {e}");
            std::process::exit(1);
        }
    };
    println!("      displayID = {}", vd.display_id());

    // ---- step 4: census after, which is the actual proof -----------------
    let during = displays::active();
    println!("\n  active displays after ({}):", during.len());
    println!("{}", displays::render(&during, Some(vd.display_id())));

    if !during.iter().any(|d| d.id == vd.display_id()) {
        eprintln!("\n  the object was created but no new display is active.");
        eprintln!("  applySettings: succeeded yet macOS did not attach the monitor.");
        std::process::exit(1);
    }
    println!("\n  the virtual display is live and visible to public CoreGraphics.");
    println!("  check System Settings > Displays, and try dragging a window onto it.");

    // ---- step 5: hold ----------------------------------------------------
    println!("\n  holding for {}s, then dropping it", hold.as_secs());
    std::thread::sleep(hold);

    // ---- step 6: drop, then prove it went away ---------------------------
    let id = vd.display_id();
    drop(vd);
    // Removal goes through WindowServer, so it is not instantaneous.
    std::thread::sleep(Duration::from_millis(600));

    let after = displays::active();
    println!("\n  active displays after drop ({}):", after.len());
    println!("{}", displays::render(&after, None));

    if after.iter().any(|d| d.id == id) {
        eprintln!("\n  GHOST DISPLAY: {id} survived the drop. This is the failure mode");
        eprintln!("  that section 13 warns about. Log out to clear it.");
        std::process::exit(1);
    }

    println!("\n  M0 passed: created, visible, and removed cleanly. No ghost display.");
}
