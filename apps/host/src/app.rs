//! The Annex application: the whole pipeline behind a menu bar UI.
//!
//! This is what M0 through M3 were building toward, wired together and given a
//! face. `annex` with no arguments runs this.
//!
//! # Startup order, which is not negotiable
//!
//! 1. **tokio first**, on its own threads, because the run loop will take the
//!    main thread and never give it back.
//! 2. Virtual display, capture, encoder, server.
//! 3. Only then hand the main thread to AppKit.
//!
//! Doing these in any other order deadlocks or crashes. Once
//! `NSApplication` owns the main thread, nothing else can run there.
//!
//! # Shutdown has to be clean
//!
//! The `VirtualDisplay` removes the monitor in its `Drop`. Quitting via
//! `exit()` would skip that and leave a ghost display the user cannot clear
//! without logging out, so Quit sets a flag, the run loop returns, and
//! everything unwinds in order.

use crate::tray;
use annex_capture::{CaptureConfig, Capturer};
use annex_core::{Codec, EncoderConfig, PixFmt, VirtualDisplayConfig};
use annex_encoder::Encoder;
use annex_transport::{RtcConfig, Server};
use annex_virtual_display::VirtualDisplay;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Options {
    /// Extend the desktop with a virtual display, rather than mirroring an
    /// existing screen. This is the actual product; mirroring is for testing.
    pub extend: bool,
    pub port: u16,
    pub fps: u32,
    pub bitrate_kbps: u32,
    /// Print the QR code to the terminal too, for when the menu bar is not
    /// convenient.
    pub print_qr: bool,
    /// Ceiling on simultaneous viewers.
    pub max_clients: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            extend: true,
            port: 8787,
            fps: 60,
            bitrate_kbps: 12_000,
            print_qr: true,
            max_clients: 4,
        }
    }
}

pub fn run(opts: Options) -> Result<(), Box<dyn std::error::Error>> {
    println!("Annex\n");

    if !annex_capture::has_permission() {
        annex_capture::permission::request_permission();
        if !annex_capture::has_permission() {
            eprintln!("{}", annex_capture::permission::permission_help());
            return Err("Screen Recording permission denied".into());
        }
    }

    // ---- the display ------------------------------------------------------
    let mut vd: Option<VirtualDisplay> = None;
    let (display_id, source) = if opts.extend {
        let cfg = VirtualDisplayConfig::default();
        let d = VirtualDisplay::create(&cfg)?;
        let id = d.display_id();
        vd = Some(d);
        // ScreenCaptureKit keeps its own view of attached displays and does not
        // learn about a brand new one instantly.
        std::thread::sleep(Duration::from_millis(1200));
        (id, "a new desktop".to_string())
    } else {
        let displays = annex_capture::list_displays()?;
        let (id, _, _) = *displays.first().ok_or("no displays visible to capture")?;
        (id, "this screen".to_string())
    };

    // Ask ScreenCaptureKit what the display actually is, rather than assuming.
    // For the virtual display these currently differ from what we requested,
    // because the mode we apply is not honoured yet.
    let (width, height) = annex_capture::list_displays()?
        .into_iter()
        .find(|(id, _, _)| *id == display_id)
        .map(|(_, w, h)| (w, h))
        .ok_or("the display vanished before capture could start")?;

    println!("  source      {source} at {width}x{height}");

    // ---- tokio, before anything takes the main thread ---------------------
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    // A fresh secret every launch. Without it, anyone who can reach the port
    // watches your screen, and the port is open to the whole local network.
    // The token only ever reaches the person at the keyboard, through the
    // printed URL, the QR code, or the menu bar.
    let token = annex_transport::auth::generate_token();

    let server = Arc::new(rt.block_on(Server::bind(RtcConfig {
        bind_addr: format!("0.0.0.0:{}", opts.port).parse()?,
        auth_token: Some(token),
        allow_input: false,
        max_clients: opts.max_clients,
        width,
        height,
        fps: opts.fps,
    }))?);

    let url = server.connect_url();
    println!("  open on the other machine:\n");
    println!("      {url}\n");
    println!("  That URL carries a one-time access token. Anyone on this network");
    println!("  who has it can see this screen, so share it deliberately.\n");

    if opts.print_qr {
        if let Some(qr) = crate::icon::qr_text(&url) {
            println!("{qr}");
        }
    }

    // ---- encoder ----------------------------------------------------------
    let server_enc = Arc::clone(&server);
    let encoder = Arc::new(Encoder::new(
        EncoderConfig {
            width,
            height,
            fps: opts.fps,
            bitrate_kbps: opts.bitrate_kbps,
            codec: Codec::H264,
            // Long on purpose. Keyframes are requested on demand when a client
            // connects or reports picture loss; unforced ones only waste
            // bitrate.
            keyframe_interval: 240,
        },
        Box::new(move |sample| server_enc.broadcast(sample)),
    )?);

    // ---- capture ----------------------------------------------------------
    let enc_sink = Arc::clone(&encoder);
    let server_cap = Arc::clone(&server);
    let mut first = true;

    let capturer = Capturer::start(
        CaptureConfig {
            display_id,
            fps: opts.fps,
            pixel_format: PixFmt::Nv12,
            size: None,
            scale: 1,
            show_cursor: true,
        },
        Box::new(move |frame| {
            // A client that just connected has no reference frame, and one that
            // sent a PLI lost sync. Either way the next thing it needs is an
            // IDR, and `take_keyframe_request` is a one-shot so exactly one is
            // forced per request.
            let force = first || server_cap.take_keyframe_request();
            first = false;
            let r = if force {
                enc_sink.encode_keyframe(&frame, frame.pts_us)
            } else {
                enc_sink.encode(&frame, frame.pts_us)
            };
            if let Err(e) = r {
                eprintln!("  encode error: {e}");
            }
        }),
    )?;

    println!("  running. Use the menu bar icon to quit.\n");
    if opts.extend {
        println!("  Drag a window off the edge of your screen to move it across.\n");
    }

    // ---- the UI, which now owns the main thread ---------------------------
    let running = Arc::new(AtomicBool::new(true));
    {
        // Ctrl-C has to route through the same clean shutdown as Quit, or the
        // virtual display leaks.
        let r = Arc::clone(&running);
        let _ = ctrlc::set_handler(move || r.store(false, Ordering::SeqCst));
    }

    let status_server = Arc::clone(&server);
    let status_encoder = Arc::clone(&encoder);
    let mut last_frames = 0u64;
    let mut last_at = Instant::now();
    let mut fps_out = 0u64;
    let mut bitrate = opts.bitrate_kbps;
    let ceiling = opts.bitrate_kbps;

    let initial = tray::Status {
        url: url.clone(),
        clients: 0,
        fps_out: 0,
        source: source.clone(),
        resolution: (width, height),
    };

    tray::run(
        initial,
        move || {
            let frames = status_encoder.stats().frames_out;
            let dt = last_at.elapsed();
            // Only recompute once a second, otherwise the reading is noise.
            if dt >= Duration::from_secs(1) {
                fps_out = ((frames - last_frames) as f64 / dt.as_secs_f64()).round() as u64;
                last_frames = frames;
                last_at = Instant::now();

                // Adapt to what the clients are actually receiving. Ignoring
                // their reported loss and continuing to push the configured
                // bitrate keeps the network queue full, and a full queue is
                // latency, which is the one thing this cannot spend.
                let want = status_server.recommended_bitrate_kbps(bitrate, ceiling);
                // Only act on a meaningful change, so the encoder is not
                // reconfigured every second for a rounding difference.
                if want.abs_diff(bitrate) * 20 > ceiling && status_encoder.set_bitrate(want).is_ok()
                {
                    bitrate = want;
                }
            }
            tray::Status {
                url: status_server.connect_url(),
                clients: status_server.stats().clients_now.load(Ordering::Relaxed),
                fps_out,
                source: source.clone(),
                resolution: (width, height),
            }
        },
        Arc::clone(&running),
    );

    // ---- unwind in order --------------------------------------------------
    println!("  stopping");
    capturer.stop();
    encoder.flush();
    rt.block_on(async {
        if let Ok(s) = Arc::try_unwrap(server) {
            s.shutdown().await;
        }
    });
    // Explicit for emphasis: this is what removes the monitor.
    drop(vd);
    println!("  done");
    Ok(())
}
