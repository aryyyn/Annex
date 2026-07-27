//! The JSON over WebSocket handshake that bootstraps WebRTC, plus the HTTP
//! routes that serve the client.
//!
//! Signalling exists because two peers cannot describe themselves to each other
//! over a connection that does not exist yet. This is the out of band channel
//! carrying session descriptions and ICE candidates until the real peer
//! connection takes over and this socket goes quiet.
//!
//! The media path is always encrypted by DTLS-SRTP. This socket is plain `ws`
//! on the LAN by default, which is a deliberate choice: it carries no media,
//! and an attacker already on your network who reads it still cannot decrypt
//! anything. Upgrade to `wss` with a self-signed certificate if that trade does
//! not suit.
//!
//! # The client is compiled in
//!
//! The page, script and stylesheet are embedded with `include_str!` rather than
//! read from disk. The host is a single binary the user may run from anywhere,
//! so depending on a relative path to `web/client` would break the moment it
//! moved.

use crate::{AppState, Session};
use annex_core::protocol::{ClientMsg, HostMsg};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::sync::atomic::Ordering;
use std::sync::Arc;

const INDEX_HTML: &str = include_str!("../../../web/client/index.html");
const CLIENT_JS: &str = include_str!("../../../web/client/client.js");
const STYLE_CSS: &str = include_str!("../../../web/client/style.css");

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(|| async { Html(INDEX_HTML) }))
        .route("/client.js", get(|| async { js(CLIENT_JS) }))
        .route("/style.css", get(|| async { css(STYLE_CSS) }))
        // Browsers request this unprompted on every page load. Answering with
        // an empty 204 keeps a spurious 404 out of the console.
        .route(
            "/favicon.ico",
            get(|| async { axum::http::StatusCode::NO_CONTENT }),
        )
        .route("/signal", get(ws_upgrade))
        .with_state(state)
}

fn js(body: &'static str) -> Response {
    (
        [("content-type", "application/javascript; charset=utf-8")],
        body,
    )
        .into_response()
}

fn css(body: &'static str) -> Response {
    ([("content-type", "text/css; charset=utf-8")], body).into_response()
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// One client, start to finish.
///
/// Order matters and is not arbitrary:
///
/// 1. Wait for `hello` and check the token. Nothing is built for a client that
///    cannot authenticate.
/// 2. Build the peer connection and create the offer. The host offers because
///    it is the side with a track to describe.
/// 3. Register the ICE callback **before** sending the offer, so candidates
///    that gather immediately are not lost.
/// 4. Pump signalling and frames concurrently until either end goes away.
async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    use futures_util::{SinkExt, StreamExt};
    let (mut sink, mut stream) = socket.split();

    // ---- 1. hello and token ---------------------------------------------
    let hello = match stream.next().await {
        Some(Ok(Message::Text(t))) => serde_json::from_str::<ClientMsg>(&t).ok(),
        _ => None,
    };
    let token = match hello {
        Some(ClientMsg::Hello { token }) => token,
        _ => {
            let _ = sink
                .send(Message::Text(
                    json(&HostMsg::Error {
                        message: "expected hello first".into(),
                    })
                    .into(),
                ))
                .await;
            return;
        }
    };
    if state.cfg.auth_token.is_some() && token != state.cfg.auth_token {
        let _ = sink
            .send(Message::Text(
                json(&HostMsg::Error {
                    message: "bad or missing token".into(),
                })
                .into(),
            ))
            .await;
        return;
    }

    // ---- 2. peer connection and offer -----------------------------------
    let session = match Session::new(&state.cfg).await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            let _ = sink
                .send(Message::Text(
                    json(&HostMsg::Error {
                        message: e.to_string(),
                    })
                    .into(),
                ))
                .await;
            return;
        }
    };
    state.sessions.lock().await.push(Arc::clone(&session));
    state
        .stats
        .clients_connected
        .fetch_add(1, Ordering::Relaxed);
    state.stats.clients_now.fetch_add(1, Ordering::Relaxed);

    // ---- 3. ICE callback, registered before the offer goes out ----------
    let (ice_tx, mut ice_rx) = tokio::sync::mpsc::unbounded_channel::<HostMsg>();
    session.peer().on_ice_candidate(Box::new(move |c| {
        let tx = ice_tx.clone();
        Box::pin(async move {
            if let Some(c) = c {
                if let Ok(init) = c.to_json() {
                    let _ = tx.send(HostMsg::Ice {
                        candidate: annex_core::protocol::IceCandidate {
                            candidate: init.candidate,
                            sdp_mid: init.sdp_mid,
                            sdp_mline_index: init.sdp_mline_index,
                        },
                    });
                }
            }
        })
    }));

    // A picture loss indication means the client's decoder lost sync, so
    // everything until the next keyframe is garbage. Flag it and let the
    // capture loop force one IDR.
    {
        let want = Arc::clone(&state.want_keyframe);
        let stats = Arc::clone(&state.stats);
        session
            .peer()
            .on_peer_connection_state_change(Box::new(move |s| {
                use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
                if s == RTCPeerConnectionState::Connected {
                    // A newly connected client has no reference frame, so it
                    // needs an IDR before anything decodes.
                    want.store(true, Ordering::SeqCst);
                    stats.keyframe_requests.fetch_add(1, Ordering::Relaxed);
                }
                Box::pin(async {})
            }));
    }

    let offer = match session.create_offer().await {
        Ok(s) => s,
        Err(e) => {
            let _ = sink
                .send(Message::Text(
                    json(&HostMsg::Error {
                        message: e.to_string(),
                    })
                    .into(),
                ))
                .await;
            return;
        }
    };
    let _ = sink
        .send(Message::Text(json(&HostMsg::Offer { sdp: offer }).into()))
        .await;
    let _ = sink
        .send(Message::Text(
            json(&HostMsg::Config {
                w: state.cfg.width,
                h: state.cfg.height,
                fps: state.cfg.fps,
            })
            .into(),
        ))
        .await;

    // ---- 4. pump ---------------------------------------------------------
    let mut frames = state.frames.subscribe();
    let session_rx = Arc::clone(&session);
    let stats = Arc::clone(&state.stats);

    // Media goes out on its own task so a slow signalling socket cannot stall
    // frame delivery, and vice versa.
    let media = tokio::spawn(async move {
        loop {
            match frames.recv().await {
                Ok(s) => {
                    if session_rx.write_sample(&s).await.is_err() {
                        break;
                    }
                }
                // Lagged means this client could not keep up and the channel
                // overwrote frames it had not read. Dropping them is correct:
                // stale frames arriving late are worse than a brief glitch.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    stats.samples_dropped.fetch_add(n, Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
    });

    loop {
        tokio::select! {
            Some(msg) = ice_rx.recv() => {
                if sink.send(Message::Text(json(&msg).into())).await.is_err() {
                    break;
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(t))) => {
                        match serde_json::from_str::<ClientMsg>(&t) {
                            Ok(ClientMsg::Answer { sdp }) => {
                                if let Err(e) = session.accept_answer(sdp).await {
                                    let _ = sink.send(Message::Text(
                                        json(&HostMsg::Error { message: e.to_string() }).into()
                                    )).await;
                                    break;
                                }
                            }
                            Ok(ClientMsg::Ice { candidate }) => session.add_ice(candidate).await,
                            Ok(_) => {}
                            Err(_) => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }

    media.abort();
    session.close().await;
    state.stats.clients_now.fetch_sub(1, Ordering::Relaxed);
    state
        .sessions
        .lock()
        .await
        .retain(|s| !Arc::ptr_eq(s, &session));
}

fn json(m: &HostMsg) -> String {
    serde_json::to_string(m).unwrap_or_else(|_| "{}".into())
}
