//! One connected client: a peer connection carrying a single H.264 video track.

use crate::RtcError;
use annex_core::protocol::{IceCandidate, Sdp};
use annex_core::EncodedSample;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264};
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtcp::receiver_report::ReceiverReport;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

#[derive(Debug, Clone)]
pub struct RtcConfig {
    pub bind_addr: SocketAddr,
    /// Required shared secret. The application always sets one; `None` means
    /// anyone who can reach the port can watch the screen, which is only ever
    /// appropriate in a test.
    pub auth_token: Option<String>,
    pub allow_input: bool,
    /// Ceiling on simultaneous sessions. Each costs a peer connection and a
    /// share of the encoder's output, so this is both a resource guard and a
    /// denial-of-service limit.
    pub max_clients: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for RtcConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:8787".parse().expect("valid literal"),
            auth_token: None,
            allow_input: false,
            max_clients: 4,
            width: 1920,
            height: 1080,
            fps: 30,
        }
    }
}

pub struct Session {
    pc: Arc<RTCPeerConnection>,
    track: Arc<TrackLocalStaticSample>,
    /// Fallback only, for a sample that arrives with no measured duration.
    frame_dur: Duration,
    /// Set when the client sends a picture loss indication.
    want_keyframe: Arc<AtomicBool>,
    /// Worst packet loss reported since the last read, as a percentage.
    loss_pct: Arc<AtomicU8>,
}

impl Session {
    /// Builds a peer connection with one H.264 track.
    ///
    /// # Why there are no ICE servers
    ///
    /// `RTCConfiguration::default()` has an empty `ice_servers` list and we
    /// leave it that way. With no STUN or TURN, ICE gathers only **host
    /// candidates**: the machine's own LAN addresses. Both peers are on the
    /// same network, so those connect directly.
    ///
    /// This removes the entire NAT traversal problem, every external
    /// dependency, and the multi-second candidate gathering that normally
    /// dominates connection setup. It is the single largest simplification in
    /// the design, and it is bought by declaring LAN-only in section 2.2.
    pub async fn new(cfg: &RtcConfig) -> Result<Self, RtcError> {
        let mut media = MediaEngine::default();
        media
            .register_default_codecs()
            .map_err(|e| RtcError::Negotiation(e.to_string()))?;

        let registry = register_default_interceptors(Registry::new(), &mut media)
            .map_err(|e| RtcError::Negotiation(e.to_string()))?;

        let api = APIBuilder::new()
            .with_media_engine(media)
            .with_interceptor_registry(registry)
            .build();

        let pc = Arc::new(
            api.new_peer_connection(RTCConfiguration::default())
                .await
                .map_err(|e| RtcError::Negotiation(e.to_string()))?,
        );

        // `packetization-mode=1` means the payloader may split a NAL unit
        // across RTP packets, which it must: a 200 KB keyframe cannot fit in
        // one datagram. `level-asymmetry-allowed` lets the two ends run
        // different levels. `profile-level-id=42e01f` is Constrained Baseline
        // 3.1, the value browsers accept most readily during negotiation.
        let track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_owned(),
                clock_rate: 90_000,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                        .to_owned(),
                ..Default::default()
            },
            "video".to_owned(),
            "annex".to_owned(),
        ));

        let sender = pc
            .add_track(track.clone())
            .await
            .map_err(|e| RtcError::Negotiation(e.to_string()))?;

        // RTCP has to be drained or it backs up, and it carries the two signals
        // that matter: PLI, meaning the client's decoder lost sync and needs a
        // fresh IDR, and REMB/receiver reports feeding congestion control.
        // Ignoring PLI leaves a broken client showing garbage until the next
        // scheduled keyframe, which at a 4 second interval is an eternity.
        let want_key = Arc::new(AtomicBool::new(false));
        // Worst fraction-lost seen since the last read, as a percentage. This
        // is the congestion signal: the receiver states, in every report, what
        // proportion of packets went missing.
        let loss = Arc::new(AtomicU8::new(0));
        {
            let want = Arc::clone(&want_key);
            let loss = Arc::clone(&loss);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 1500];
                while let Ok((packets, _)) = sender.read(&mut buf).await {
                    for p in packets {
                        let any = p.as_any();
                        if any.downcast_ref::<PictureLossIndication>().is_some() {
                            want.store(true, Ordering::SeqCst);
                        }
                        if let Some(rr) = any.downcast_ref::<ReceiverReport>() {
                            for r in &rr.reports {
                                // fraction_lost is expressed in 1/256 units.
                                let pct = (r.fraction_lost as u32 * 100 / 256) as u8;
                                loss.fetch_max(pct, Ordering::Relaxed);
                            }
                        }
                    }
                }
            });
        }

        Ok(Self {
            pc,
            track,
            frame_dur: Duration::from_secs_f64(1.0 / cfg.fps.max(1) as f64),
            want_keyframe: want_key,
            loss_pct: loss,
        })
    }

    /// Creates the offer and starts ICE gathering.
    pub async fn create_offer(&self) -> Result<Sdp, RtcError> {
        let offer = self
            .pc
            .create_offer(None)
            .await
            .map_err(|e| RtcError::Negotiation(e.to_string()))?;
        self.pc
            .set_local_description(offer.clone())
            .await
            .map_err(|e| RtcError::Negotiation(e.to_string()))?;
        Ok(offer.sdp)
    }

    pub async fn accept_answer(&self, sdp: Sdp) -> Result<(), RtcError> {
        let answer =
            RTCSessionDescription::answer(sdp).map_err(|e| RtcError::Negotiation(e.to_string()))?;
        self.pc
            .set_remote_description(answer)
            .await
            .map_err(|e| RtcError::Negotiation(e.to_string()))
    }

    /// Adds a candidate the client trickled to us.
    ///
    /// Late or duplicate candidates are normal in trickle ICE and must not tear
    /// the session down, so the error is swallowed rather than propagated.
    pub async fn add_ice(&self, c: IceCandidate) {
        let init = RTCIceCandidateInit {
            candidate: c.candidate,
            sdp_mid: c.sdp_mid,
            sdp_mline_index: c.sdp_mline_index,
            username_fragment: None,
        };
        let _ = self.pc.add_ice_candidate(init).await;
    }

    /// Pushes one encoded access unit onto the track.
    ///
    /// `write_sample` takes the Annex-B bitstream directly. webrtc-rs runs the
    /// H.264 payloader, splits it into RTP packets, and handles NACK
    /// retransmission, so nothing here has to know about packet sizes.
    pub async fn write_sample(&self, s: &EncodedSample) -> Result<(), RtcError> {
        // The duration is what advances the receiver's RTP clock, so it must be
        // the real gap since the previous frame rather than a nominal 1/fps.
        // See the note in `annex-encoder`: getting this wrong inflates the
        // client's jitter buffer and shows up as steadily worsening lag.
        let duration = if s.dur.is_zero() {
            self.frame_dur
        } else {
            s.dur
        };
        self.track
            .write_sample(&Sample {
                // Free: Bytes is reference counted, so this is an atomic
                // increment rather than a copy of the whole access unit.
                data: s.data.clone(),
                timestamp: SystemTime::now(),
                duration,
                ..Default::default()
            })
            .await
            .map_err(|_| RtcError::PeerClosed)
    }

    pub fn peer(&self) -> Arc<RTCPeerConnection> {
        Arc::clone(&self.pc)
    }

    /// Whether the client has asked for a keyframe since the last check.
    ///
    /// One-shot: reading it clears it, so exactly one IDR is forced per
    /// request rather than one per frame until someone notices.
    pub fn take_keyframe_request(&self) -> bool {
        self.want_keyframe.swap(false, Ordering::SeqCst)
    }

    /// Worst packet loss this client has reported since the last call, as a
    /// percentage. Reading it resets the high-water mark.
    pub fn take_loss_pct(&self) -> u8 {
        self.loss_pct.swap(0, Ordering::Relaxed)
    }

    pub async fn close(&self) {
        let _ = self.pc.close().await;
    }
}
