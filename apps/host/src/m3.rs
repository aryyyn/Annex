//! M3: stream a display to a browser over WebRTC.
//!
//! # Why this uses the *main* display
//!
//! Deliberate sequencing from section 14. M3 proves the entire network path,
//! signalling, ICE, DTLS, RTP, browser decode, using a display that definitely
//! exists and definitely renders. If the picture is wrong, the fault is in the
//! streaming half and nowhere else.
//!
//! Only at M4 does capture repoint at the virtual display, and by then the
//! network path is known good, so any new failure is unambiguously the private
//! API's fault. Debugging the two halves at once would be far harder than
//! debugging them in sequence.
//!
//! Pass `--virtual` to jump ahead and use the virtual display instead, which is
//! effectively M4.
//!
//! # Threading
//!
//! This is the first milestone where both worlds are live at once. tokio owns
//! the HTTP server and the peer connections on its own threads; ScreenCaptureKit
//! delivers frames on a dispatch queue; VideoToolbox calls back on a third. They
//! meet at exactly one place: the encoder's sink hands finished samples to
//! `Server::broadcast`, which is non-blocking.

use annex_capture::{CaptureConfig, Capturer};
use annex_core::{Codec, EncoderConfig, PixFmt, VirtualDisplayConfig};
use annex_encoder::Encoder;
use annex_transport::{RtcConfig, Server};
use annex_virtual_display::VirtualDisplay;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

pub fn run(use_virtual: bool, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "Annex M3: stream the {} display to a browser\n",
        if use_virtual { "virtual" } else { "main" }
    );

    // webrtc-rs logs through the `log` crate. Without a subscriber its errors
    // vanish, and a failed DTLS handshake looks like silence.
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("annex=debug,webrtc=debug"),
    )
    .try_init();

    if !annex_capture::has_permission() {
        annex_capture::permission::request_permission();
        if !annex_capture::has_permission() {
            eprintln!("\n{}", annex_capture::permission::permission_help());
            return Err("Screen Recording permission denied".into());
        }
    }

    // Kept alive for the whole session when in virtual mode. Dropping it would
    // remove the monitor out from under the capture.
    let mut _vd: Option<VirtualDisplay> = None;

    let (display_id, width, height) = if use_virtual {
        let cfg = VirtualDisplayConfig::default();
        let vd = VirtualDisplay::create(&cfg)?;
        let id = vd.display_id();
        _vd = Some(vd);
        std::thread::sleep(Duration::from_millis(1200));
        println!("  virtual display {id} at {}x{}", cfg.width, cfg.height);
        (id, cfg.width, cfg.height)
    } else {
        // The main display, as ScreenCaptureKit sees it. Its own reported size
        // is authoritative here: unlike the virtual display, a real screen may
        // genuinely be HiDPI.
        let displays = annex_capture::list_displays()?;
        let (id, w, h) = *displays
            .first()
            .ok_or("ScreenCaptureKit reports no displays")?;
        println!("  main display {id} at {w}x{h}");
        (id, w, h)
    };

    // ---- tokio, started before anything blocks ---------------------------
    //
    // The runtime has to exist before the server, and the capture callbacks
    // need a handle to reach it from their own threads.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    let fps = 30u32;
    let rtc_cfg = RtcConfig {
        bind_addr: format!("0.0.0.0:{port}").parse()?,
        // The milestone harnesses are local diagnostics, so they stay open on
        // purpose. The application itself always requires a token.
        auth_token: None,
        allow_input: false,
        max_clients: 4,
        width,
        height,
        fps,
    };

    let server = Arc::new(rt.block_on(Server::bind(rtc_cfg))?);
    println!("\n  serving on {}", server.local_addr());
    println!("  open this on the other machine:\n");
    println!("      {}\n", server.connect_url());

    // ---- encoder ---------------------------------------------------------
    let enc_cfg = EncoderConfig {
        width,
        height,
        fps,
        bitrate_kbps: 8_000,
        codec: Codec::H264,
        // Long, because keyframes are requested on demand when a client
        // connects or loses sync. Unforced ones just waste bitrate.
        keyframe_interval: 120,
    };

    let server_enc = Arc::clone(&server);
    let encoder = Arc::new(Encoder::new(
        enc_cfg.clone(),
        Box::new(move |sample| {
            // Non-blocking. With no clients connected the sample is simply
            // dropped, which is the right thing when nobody is watching.
            server_enc.broadcast(sample);
        }),
    )?);
    println!(
        "  encoder: H.264 {width}x{height} @ {fps} fps, {} kbps",
        enc_cfg.bitrate_kbps
    );

    // ---- capture ---------------------------------------------------------
    let cap_cfg = CaptureConfig {
        display_id,
        fps,
        pixel_format: PixFmt::Nv12,
        size: None,
        scale: 1,
        show_cursor: true,
    };

    let enc_sink = Arc::clone(&encoder);
    let server_cap = Arc::clone(&server);
    let mut first = true;

    let capturer = Capturer::start(
        cap_cfg,
        Box::new(move |frame| {
            // A newly connected client has no reference frame, so the first
            // thing it must receive is an IDR. The transport sets this flag
            // when a peer reaches Connected, and takes it as a one-shot so we
            // force exactly one keyframe per request.
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

    println!("  capturing and encoding. Ctrl-C to stop.\n");

    // ---- report ----------------------------------------------------------
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let r = Arc::clone(&running);
        // Ctrl-C has to unwind through a clean shutdown rather than exiting the
        // process, or a virtual display created in `--virtual` mode is leaked
        // as a ghost monitor.
        ctrlc::set_handler(move || r.store(false, Ordering::SeqCst))?;
    }

    let mut last_sent = 0u64;
    while running.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_secs(2));
        let s = server.stats();
        let sent = s.samples_sent.load(Ordering::Relaxed);
        let clients = s.clients_now.load(Ordering::Relaxed);
        let est = encoder.stats();
        println!(
            "  clients {clients}  |  encoded {} frames, {} keyframes  |  sent {} (+{})  dropped {}  no-receiver {}",
            est.frames_out,
            est.keyframes,
            sent,
            sent - last_sent,
            s.samples_dropped.load(Ordering::Relaxed),
            s.broadcast_no_receiver.load(Ordering::Relaxed)
        );
        last_sent = sent;
    }

    println!("\n  stopping");
    capturer.stop();
    encoder.flush();
    rt.block_on(async {
        // `server` is behind an Arc because the capture thread holds one too.
        // By here that thread is stopped, so unwrapping is safe.
        if let Ok(s) = Arc::try_unwrap(server) {
            s.shutdown().await;
        }
    });
    drop(_vd);
    println!("  done");
    Ok(())
}
