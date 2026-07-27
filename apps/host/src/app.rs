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
use annex_virtual_display::mode::DisplayMode;
use annex_virtual_display::VirtualDisplay;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Capture and encode, which are the two stages tied to a specific resolution.
///
/// Bundled together because they cannot be changed independently: a
/// `VTCompressionSession` is created at a fixed size, and `SCStream` is
/// configured with one. Changing resolution means replacing both, while the
/// display, the server and every connected client survive untouched.
struct Pipeline {
    capturer: Capturer,
    encoder: Arc<Encoder>,
}

impl Pipeline {
    fn start(
        display_id: u32,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_kbps: u32,
        server: Arc<annex_transport::Server>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let server_enc = Arc::clone(&server);
        let encoder = Arc::new(Encoder::new(
            EncoderConfig {
                width,
                height,
                fps,
                bitrate_kbps,
                codec: Codec::H264,
                // Long on purpose. Keyframes are requested on demand when a
                // client connects or reports picture loss; unforced ones only
                // waste bitrate.
                keyframe_interval: 240,
            },
            Box::new(move |sample| server_enc.broadcast(sample)),
        )?);

        let enc_sink = Arc::clone(&encoder);
        let server_cap = Arc::clone(&server);
        let mut first = true;

        let capturer = Capturer::start(
            CaptureConfig {
                display_id,
                fps,
                pixel_format: PixFmt::Nv12,
                size: None,
                scale: 1,
                show_cursor: true,
            },
            Box::new(move |frame| {
                // A client that just connected has no reference frame, and one
                // that sent a PLI lost sync. Either way the next thing it needs
                // is an IDR, and `take_keyframe_request` is a one-shot so
                // exactly one is forced per request.
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

        Ok(Self { capturer, encoder })
    }

    fn stop(self) {
        self.capturer.stop();
        self.encoder.flush();
    }
}

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
    /// Let clients drive this Mac's cursor and keyboard.
    ///
    /// Off by default, and deliberately a startup choice rather than a menu
    /// toggle: the DataChannel is negotiated when a client connects, so
    /// flipping it later would apply to some sessions and not others, which is
    /// exactly the kind of half-state a capability like this must never have.
    pub allow_input: bool,
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
            allow_input: false,
        }
    }
}

pub fn run(opts: Options) -> Result<(), Box<dyn std::error::Error>> {
    println!("Annex\n");

    if !annex_capture::has_permission() {
        annex_capture::permission::request_permission();
        if !annex_capture::has_permission() {
            // A bundled app has no terminal, so printing here reaches nobody:
            // the user double-clicks, a system prompt appears, and the app
            // seems to vanish. Say what happened, and open the right settings
            // pane so the fix is one click rather than a hunt.
            eprintln!("{}", annex_capture::permission::permission_help());
            explain_permission_and_exit();
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

    // Ask ScreenCaptureKit what the display actually is rather than assuming,
    // since the mode finally selected may not be the one requested verbatim.
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

    // Bind to the LAN interface rather than every interface.
    //
    // `0.0.0.0` also listens on a VPN adapter, a hotspot, and anything else
    // that appears later, which quietly widens who can reach the port well
    // beyond the local network the design scopes itself to. Falling back to
    // `0.0.0.0` when the LAN address cannot be determined keeps a machine with
    // unusual networking working, at the cost of the narrower exposure.
    let bind_ip = match annex_transport::lan_ip() {
        Some(ip) => ip.to_string(),
        None => {
            println!("  note        could not determine the LAN address, binding all interfaces");
            "0.0.0.0".to_string()
        }
    };

    // ---- input, when enabled --------------------------------------------
    //
    // Events arrive on a webrtc-rs task but `CGEventPost` has to run on the
    // main thread, so they cross a channel and are drained by the run loop.
    let (input_tx, input_rx) = std::sync::mpsc::channel::<annex_core::InputEvent>();
    // Counted separately from injection so delivery can be observed even when
    // the Accessibility grant is missing and nothing is actually posted.
    let input_seen = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let input_seen_report = Arc::clone(&input_seen);
    let input_sink: Option<annex_transport::session::InputSink> = if opts.allow_input {
        if !annex_input::has_permission() {
            println!("\n{}\n", annex_input::permission_help());
            request_accessibility();
        }
        let tx = input_tx.clone();
        let seen = Arc::clone(&input_seen);
        Some(Arc::new(move |ev| {
            seen.fetch_add(1, Ordering::Relaxed);
            let _ = tx.send(ev);
        }))
    } else {
        None
    };

    let server = Arc::new(rt.block_on(Server::bind_with_input(
        RtcConfig {
            bind_addr: format!("{bind_ip}:{}", opts.port).parse()?,
            auth_token: Some(token),
            allow_input: opts.allow_input,
            max_clients: opts.max_clients,
            width,
            height,
            fps: opts.fps,
        },
        input_sink,
    ))?);

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

    // ---- capture and encode ----------------------------------------------
    //
    // Creating the display only establishes what it *can* do: macOS then picks
    // its own default mode, which is 1920x1080 whatever ceiling was set. The
    // requested size has to be selected explicitly.
    if let Some(vd) = &vd {
        if let Some(m) = vd.set_mode(width, height) {
            println!("  mode        {}", m.label());
        }
    }
    let (width, height) = current_size(display_id).unwrap_or((width, height));

    let mut pipeline = Some(Pipeline::start(
        display_id,
        width,
        height,
        opts.fps,
        opts.bitrate_kbps,
        Arc::clone(&server),
    )?);

    if opts.allow_input {
        println!("  INPUT       enabled: clients can control this Mac");
    }
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
    let mut last_frames = 0u64;
    let mut last_at = Instant::now();
    let mut fps_out = 0u64;
    let mut bitrate = opts.bitrate_kbps;
    let ceiling = opts.bitrate_kbps;
    let mut size = (width, height);
    let mut last_input_seen = 0u64;

    // Only a virtual display can be resized: changing a real monitor's mode
    // out from under its owner would be rude and surprising.
    let offered: Vec<DisplayMode> = match &vd {
        Some(v) => v
            .modes()
            .into_iter()
            // Below this the second screen stops being useful for anything.
            .filter(|m| m.width >= 1024)
            .collect(),
        None => Vec::new(),
    };

    // Built here, on the main thread, and never sent anywhere else.
    let mut injector = if opts.allow_input {
        match annex_input::Injector::new(display_id) {
            Ok(i) => Some(i),
            Err(e) => {
                eprintln!("  input disabled: {e}");
                None
            }
        }
    } else {
        None
    };

    let initial = tray::Status {
        url: url.clone(),
        clients: 0,
        fps_out: 0,
        source: source.clone(),
        resolution: size,
        modes: offered.iter().map(|m| m.label()).collect(),
        current_mode: current_size(display_id)
            .map(|(w, h)| format!("{w} x {h}"))
            .unwrap_or_default(),
        input_enabled: opts.allow_input && injector.is_some(),
    };

    let requested_mode = Arc::new(std::sync::Mutex::new(None::<usize>));
    let requested_for_ui = Arc::clone(&requested_mode);

    tray::run(
        initial,
        move || {
            // ---- client input, applied here because this is the main thread -
            //
            // `CGEventPost` requires it. Posting from the webrtc task instead
            // produces events macOS silently discards, which looks like the
            // client being ignored rather than an error.
            if let Some(inj) = injector.as_mut() {
                // Bounded per tick so a flood of events cannot starve the menu.
                for _ in 0..256 {
                    match input_rx.try_recv() {
                        Ok(ev) => {
                            if let Err(e) = inj.inject(ev) {
                                eprintln!("  input: {e}");
                            }
                        }
                        Err(_) => break,
                    }
                }
            }

            // ---- a resolution change, if the user picked one ---------------
            //
            // The display, the server and every connected peer survive this.
            // Only capture and encode are rebuilt, because both are created at
            // a fixed size. Clients see new SPS/PPS followed by an IDR, which
            // browsers handle as an in-stream resolution change.
            if let Some(idx) = requested_for_ui.lock().ok().and_then(|mut r| r.take()) {
                if let (Some(m), Some(_)) = (offered.get(idx), vd.as_ref()) {
                    if let Some(p) = pipeline.take() {
                        p.stop();
                    }
                    if let Some(v) = vd.as_ref() {
                        v.set_mode(m.width, m.height);
                    }
                    // Ask the display what it settled on rather than assuming
                    // it honoured the request exactly.
                    size = current_size(display_id).unwrap_or((m.width, m.height));
                    match Pipeline::start(
                        display_id,
                        size.0,
                        size.1,
                        opts.fps,
                        bitrate,
                        Arc::clone(&status_server),
                    ) {
                        Ok(p) => {
                            println!("  resolution changed to {}x{}", size.0, size.1);
                            pipeline = Some(p);
                        }
                        Err(e) => eprintln!("  could not restart capture: {e}"),
                    }
                }
            }

            // ---- status and adaptive bitrate ------------------------------
            let frames = pipeline
                .as_ref()
                .map(|p| p.encoder.stats().frames_out)
                .unwrap_or(0);
            let dt = last_at.elapsed();
            // Only recompute once a second, otherwise the reading is noise.
            if dt >= Duration::from_secs(1) {
                fps_out =
                    ((frames.saturating_sub(last_frames)) as f64 / dt.as_secs_f64()).round() as u64;
                last_frames = frames;
                last_at = Instant::now();

                // Adapt to what the clients are actually receiving. Ignoring
                // their reported loss and continuing to push the configured
                // bitrate keeps the network queue full, and a full queue is
                // latency, which is the one thing this cannot spend.
                let want = status_server.recommended_bitrate_kbps(bitrate, ceiling);
                // Only act on a meaningful change, so the encoder is not
                // reconfigured every second for a rounding difference.
                if want.abs_diff(bitrate) * 20 > ceiling {
                    if let Some(p) = pipeline.as_ref() {
                        if p.encoder.set_bitrate(want).is_ok() {
                            bitrate = want;
                        }
                    }
                }
            }

            let n = input_seen_report.load(Ordering::Relaxed);
            if n != last_input_seen {
                last_input_seen = n;
                println!("  input events received: {n}");
            }
            tray::Status {
                url: status_server.connect_url(),
                clients: status_server.stats().clients_now.load(Ordering::Relaxed),
                fps_out,
                source: source.clone(),
                resolution: size,
                modes: offered.iter().map(|m| m.label()).collect(),
                current_mode: format!("{} x {}", size.0, size.1),
                input_enabled: injector.is_some(),
            }
        },
        Arc::clone(&running),
        requested_mode,
    );

    // ---- unwind in order --------------------------------------------------
    println!("  stopping");
    rt.block_on(async {
        if let Ok(s) = Arc::try_unwrap(server) {
            s.shutdown().await;
        }
    });
    println!("  done");
    Ok(())
}

/// The display's current logical size.
fn current_size(id: u32) -> Option<(u32, u32)> {
    annex_virtual_display::mode::current(id).map(|m| (m.width, m.height))
}

/// Tells the user what to do when the Screen Recording grant is missing.
///
/// macOS reads TCC decisions at process start, so approving the prompt does not
/// help the *running* process. It has to be relaunched, and saying so plainly
/// avoids the obvious conclusion that the app is broken.
fn explain_permission_and_exit() {
    let msg = "Annex needs Screen Recording permission.\n\n\
               Enable Annex under Privacy & Security > Screen & System Audio \
               Recording, then open Annex again.\n\n\
               macOS only applies this permission when an app starts, so \
               Annex has to be relaunched after you allow it.";
    // AppleScript rather than an NSAlert: this runs before the run loop owns
    // the main thread, and putting up AppKit UI here would mean standing up a
    // second application surface for a single message.
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(format!(
            r#"display dialog "{}" with title "Annex" buttons {{"Open Settings", "Quit"}} default button "Open Settings" with icon caution"#,
            msg.replace('"', "'")
        ))
        .output()
        .map(|out| {
            if String::from_utf8_lossy(&out.stdout).contains("Open Settings") {
                let _ = std::process::Command::new("open")
                    .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
                    .status();
            }
        });
}

/// Nudges macOS into showing the Accessibility prompt.
///
/// There is no request call for this the way there is for Screen Recording:
/// macOS shows its dialogue when an app first tries to post an event, and the
/// grant applies only from the next launch. Saying so avoids the reasonable
/// conclusion that input is broken.
fn request_accessibility() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .status();
}
