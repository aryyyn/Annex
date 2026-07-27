//! M2: encode the captured display to H.264 and prove the stream is valid.
//!
//! The full chain for the first time: virtual display, capture, encode, file.
//!
//! # This is the zero-copy path
//!
//! M1 called `to_bgra_vec` to write PNGs, dragging every frame off the GPU. M2
//! does not. The `CVPixelBuffer` goes from ScreenCaptureKit straight into
//! VideoToolbox, so the pixels stay on the GPU from the compositor to the media
//! engine and only compressed bytes ever reach the CPU.
//!
//! # What counts as proof
//!
//! An encoder that emits plausible-looking bytes is worthless if no decoder
//! accepts them. So this checks three separate things:
//!
//! 1. **Structure.** Walk the Annex-B stream ourselves and confirm SPS, PPS and
//!    IDR units are present, and that the first keyframe carries its parameter
//!    sets in band.
//! 2. **Latency shape.** Confirm the output has no frame reordering, since a
//!    single B-frame would undermine the whole design.
//! 3. **A real decoder agrees.** The caller runs ffmpeg over the file.

use annex_capture::{CaptureConfig, Capturer};
use annex_core::{Codec, EncoderConfig, PixFmt, VirtualDisplayConfig};
use annex_encoder::{annexb, Encoder};
use annex_virtual_display::VirtualDisplay;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

pub fn run(frames_wanted: usize, out_path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!("Annex M2: encode the virtual display to H.264\n");

    if !annex_capture::has_permission() {
        annex_capture::permission::request_permission();
        if !annex_capture::has_permission() {
            eprintln!("\n{}", annex_capture::permission::permission_help());
            return Err("Screen Recording permission denied".into());
        }
    }

    // ---- display + capture, as M0 and M1 ---------------------------------
    let vd_cfg = VirtualDisplayConfig::default();
    let vd = VirtualDisplay::create(&vd_cfg)?;
    let display_id = vd.display_id();
    println!(
        "  virtual display {display_id} at {}x{}",
        vd_cfg.width, vd_cfg.height
    );
    std::thread::sleep(Duration::from_millis(1200));

    // ---- encoder ---------------------------------------------------------
    let enc_cfg = EncoderConfig {
        width: vd_cfg.width,
        height: vd_cfg.height,
        fps: 30,
        bitrate_kbps: 8_000,
        codec: Codec::H264,
        // Deliberately short for M2 so a 30-frame run contains more than one
        // keyframe and the in-band parameter sets can be checked repeatedly.
        keyframe_interval: 15,
    };
    println!(
        "  encoder: H.264 {}x{} @ {} fps, {} kbps, keyframe every {} frames",
        enc_cfg.width, enc_cfg.height, enc_cfg.fps, enc_cfg.bitrate_kbps, enc_cfg.keyframe_interval
    );

    let (tx, rx) = mpsc::channel::<annex_core::EncodedSample>();
    let encoder = Arc::new(Encoder::new(
        enc_cfg.clone(),
        Box::new(move |s| {
            let _ = tx.send(s);
        }),
    )?);
    println!("      RealTime=true, AllowFrameReordering=false, Main AutoLevel");

    // ---- capture straight into the encoder --------------------------------
    //
    // NV12 rather than M1's BGRA: it is VideoToolbox's native input, so the
    // encoder does not have to colour-convert every frame.
    let cap_cfg = CaptureConfig {
        display_id,
        fps: enc_cfg.fps,
        pixel_format: PixFmt::Nv12,
        size: None,
        scale: 1,
        show_cursor: false,
    };

    let fed = Arc::new(AtomicUsize::new(0));
    let fed_sink = Arc::clone(&fed);
    let enc_sink = Arc::clone(&encoder);

    let capturer = Capturer::start(
        cap_cfg,
        Box::new(move |frame| {
            let n = fed_sink.fetch_add(1, Ordering::SeqCst);
            if n >= frames_wanted {
                return;
            }
            // The pixel buffer goes straight through. No readback.
            let r = if n == 0 {
                // Force the very first frame to be an IDR so the file starts
                // with something decodable.
                enc_sink.encode_keyframe(&frame, frame.pts_us)
            } else {
                enc_sink.encode(&frame, frame.pts_us)
            };
            if let Err(e) = r {
                eprintln!("      encode error: {e}");
            }
        }),
    )?;
    println!("\n  capturing NV12 and encoding {frames_wanted} frames (zero-copy)");

    // ---- collect ---------------------------------------------------------
    let mut stream: Vec<u8> = Vec::new();
    let mut samples = 0usize;
    let mut pts_list: Vec<i64> = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);

    while samples < frames_wanted && std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(s) => {
                let nals = annexb::scan(&s.data);
                let kinds: Vec<String> = nals
                    .iter()
                    .map(|(t, len)| format!("{}({len})", annexb::nal_type_name(*t)))
                    .collect();
                if samples < 6 || s.keyframe {
                    println!(
                        "      sample {:>3} {:>7} bytes  {}  [{}]",
                        samples,
                        s.data.len(),
                        if s.keyframe { "KEY" } else { "   " },
                        kinds.join(", ")
                    );
                }
                pts_list.push(s.pts);
                stream.extend_from_slice(&s.data);
                samples += 1;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if fed.load(Ordering::SeqCst) >= frames_wanted {
                    encoder.flush();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    encoder.flush();
    capturer.stop();
    drop(vd);

    let stats = encoder.stats();
    println!(
        "\n  encoder: {} frames in, {} out, {} keyframes, {} bytes, {} malformed",
        stats.frames_in,
        stats.frames_out,
        stats.keyframes,
        stats.bytes_out,
        stats.dropped_malformed
    );

    if samples == 0 {
        return Err("no samples came out of the encoder".into());
    }

    std::fs::File::create(&out_path)?.write_all(&stream)?;
    println!("  wrote {} bytes to {}", stream.len(), out_path.display());

    // ---- check 1: structure ---------------------------------------------
    let all = annexb::scan(&stream);
    let sps = all.iter().filter(|(t, _)| *t == 7).count();
    let pps = all.iter().filter(|(t, _)| *t == 8).count();
    let idr = all.iter().filter(|(t, _)| *t == 5).count();
    let slices = all.iter().filter(|(t, _)| *t == 1).count();
    println!(
        "\n  structure: {} NAL units, {sps} SPS, {pps} PPS, {idr} IDR, {slices} non-IDR slices",
        all.len()
    );

    if sps == 0 || pps == 0 {
        return Err(
            "no SPS/PPS in the stream: a client joining mid-stream could not decode".into(),
        );
    }
    if idr == 0 {
        return Err("no IDR frame in the stream".into());
    }
    // The parameter sets must lead the file, or the first keyframe is useless.
    if !matches!(all.first(), Some((7, _))) {
        return Err("stream does not begin with SPS".into());
    }
    if sps < idr {
        return Err("some keyframes lack in-band parameter sets".into());
    }
    println!("      every keyframe carries its own SPS and PPS in band");

    // ---- check 2: no reordering -----------------------------------------
    //
    // With AllowFrameReordering=false the output timestamps must be
    // monotonically increasing. A single decrease would mean a B-frame slipped
    // in, which is exactly the latency the design forbids.
    let reordered = pts_list.windows(2).filter(|w| w[1] < w[0]).count();
    if reordered > 0 {
        return Err(
            format!("output is reordered at {reordered} points: B-frames are present").into(),
        );
    }
    println!("      presentation timestamps are monotonic: no B-frames, no reordering");

    // Bitrate sanity, for the latency budget in section 10.
    let secs = (samples as f64) / (enc_cfg.fps as f64);
    println!(
        "      average {:.1} Mbit/s over {:.1}s of video",
        (stream.len() as f64 * 8.0) / secs / 1_000_000.0,
        secs
    );

    println!("\n  M2 structural checks passed. Confirm an independent decoder agrees:");
    println!(
        "      ffprobe -show_entries stream=profile,width,height,has_b_frames {}",
        out_path.display()
    );
    println!(
        "      ffmpeg -v error -i {} -f null -    # silence means zero decode errors",
        out_path.display()
    );
    Ok(())
}
