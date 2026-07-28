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
//! # Running it
//!
//! `annex` with no arguments starts the real application: it creates a virtual
//! display, streams it, and puts a menu bar icon up. Everything else is a
//! milestone harness kept around because each one isolates a different layer,
//! which is exactly what you want when something breaks.
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
//! cargo run -p annex-host -- --input   # allow clients to control this Mac
//! ```

mod app;
mod displays;
mod icon;
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
    // Flags are recognised wherever they appear; the first non-flag argument
    // selects the mode. Treating argv[1] as the mode unconditionally meant
    // `annex --input` silently ran the M0 harness instead.
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| argv.iter().any(|a| a == name);
    let positional: Vec<&String> = argv.iter().filter(|a| !a.starts_with("--")).collect();
    let arg1 = positional
        .first()
        .map(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let arg_at = |i: usize| positional.get(i).map(|s| s.as_str()).unwrap_or("");
    let value_of = |name: &str| {
        argv.iter()
            .position(|a| a == name)
            .and_then(|i| argv.get(i + 1))
            .cloned()
    };

    // Build-time helper, used by scripts/bundle.sh to produce the app icon.
    // Kept in the binary rather than a separate tool so the icon is generated
    // by the same code that documents it.
    if flag("--emit-iconset") {
        // The directory follows the flag, so it is read by position relative to
        // it rather than from `positional`, which strips flags out.
        let dir = argv
            .iter()
            .position(|a| a == "--emit-iconset")
            .and_then(|i| argv.get(i + 1))
            .cloned()
            .unwrap_or_else(|| "Annex.iconset".into());
        match icon::write_iconset(std::path::Path::new(&dir)) {
            Ok(()) => println!("wrote iconset to {dir}"),
            Err(e) => {
                eprintln!("could not write iconset: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // No arguments, or `mirror`: the actual application.
    if flag("--help") || flag("-h") || arg1 == "help" {
        print_help();
        return;
    }

    if arg1.is_empty() || arg1 == "mirror" || arg1 == "extend" {
        let opts = app::Options {
            extend: arg1 != "mirror",
            // Off unless asked for, explicitly, by name.
            allow_input: flag("--input"),
            port: value_of("--port")
                .and_then(|v| v.parse().ok())
                .unwrap_or(8787),
            ..Default::default()
        };
        if let Err(e) = app::run(opts) {
            eprintln!("\n  Annex failed to start: {e}");
            std::process::exit(1);
        }
        return;
    }

    if arg1 == "m1" {
        let frames = arg_at(1).parse().unwrap_or(10);
        let scale = arg_at(2).parse().unwrap_or(1);
        let out = std::path::PathBuf::from(if scale > 1 { "captures-2x" } else { "captures" });
        if let Err(e) = m1::run(frames, out, scale) {
            eprintln!("\n  M1 failed: {e}");
            std::process::exit(1);
        }
        return;
    }

    if arg1 == "m2" {
        let frames = arg_at(1).parse().unwrap_or(60);
        let out = std::path::PathBuf::from("out.h264");
        if let Err(e) = m2::run(frames, out) {
            eprintln!("\n  M2 failed: {e}");
            std::process::exit(1);
        }
        return;
    }

    if arg1 == "m3" {
        let arg2 = arg_at(1);
        let use_virtual = arg2 == "virtual";
        let port = arg2.parse().unwrap_or(8787);
        if let Err(e) = m3::run(use_virtual, port) {
            eprintln!("\n  M3 failed: {e}");
            std::process::exit(1);
        }
        return;
    }

    // Everything past here is a milestone harness, and each one is named. An
    // unrecognised argument is a mistake, not an invitation to guess: a bare
    // number used to mean "M0 hold seconds", so `annex 8788` quietly ran the
    // spike for two hours instead of serving on port 8788.
    if arg1 != "m0" {
        eprintln!("annex: unknown argument `{arg1}`\n");
        print_help();
        std::process::exit(2);
    }

    let hold = arg_at(1)
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

fn print_help() {
    println!(
        "\
Annex: turn any laptop on your network into a second monitor for this Mac.

USAGE
  annex                    create a new desktop and stream it (the normal use)
  annex mirror             stream this screen instead of adding a desktop
  annex help               show this

OPTIONS
  --port <n>               listen on this port (default 8787)
  --input                  let clients control this Mac's cursor and keyboard.
                           Off by default. Needs the Accessibility permission,
                           and the menu bar shows when it is on.

DIAGNOSTICS
  Each isolates one layer, which is what you want when something breaks.

  annex m0 [seconds]       create a virtual display, hold, remove it
  annex m1 [frames] [x]    capture the virtual display to PNG
  annex m2 [frames]        encode to H.264, write out.h264
  annex m3 [virtual|port]  stream over WebRTC without the menu bar app

The URL printed at startup carries a one-time access token. Anyone on this
network who has it can see this screen."
    );
}
