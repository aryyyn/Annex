// Annex browser client.
//
// The whole job: open a WebSocket, complete the WebRTC handshake, and attach
// the incoming track to the video element. Plain JavaScript, no framework and
// no build step. The host serves this file itself, so there is nothing to
// install on the client machine.
//
// The host offers and this side answers, because the host is the one with a
// media track to describe.

const video = document.getElementById("screen");
const overlay = document.getElementById("overlay");
const statusEl = document.getElementById("status");
const goBtn = document.getElementById("go");
const statsEl = document.getElementById("stats");

// The token travels in the query string so a QR code can carry it. It is a
// LAN-only shared secret, not a credential worth protecting in transit.
const token = new URLSearchParams(location.search).get("token");

let pc = null;
let ws = null;
let retryDelay = 500;

function setStatus(text) {
  statusEl.textContent = text;
}

function send(obj) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(obj));
}

function newPeerConnection() {
  // No STUN and no TURN. Both machines are on the same network, so ICE gathers
  // only host candidates and they connect directly. This is what makes setup
  // near instant instead of the usual multi-second candidate gathering.
  const pc = new RTCPeerConnection({ iceServers: [] });

  pc.onicecandidate = ({ candidate }) => {
    // A null candidate means gathering finished. Nothing to send.
    if (candidate) send({ type: "ice", candidate: candidate.toJSON() });
  };

  pc.ontrack = (event) => {
    video.srcObject = event.streams[0];
    setStatus("Connected");
    overlay.classList.add("hidden");
    document.body.classList.add("streaming");
    // Full screen needs a user gesture in every browser, so it cannot be done
    // automatically. Offer a button instead.
    goBtn.hidden = false;
    startStatsLoop(pc);
  };

  // Exposed for debugging and for the headless verification harness. Handy in
  // devtools too: `await __pc.getStats()` tells you what the browser really
  // thinks of the stream.
  window.__pc = pc;

  pc.onconnectionstatechange = () => {
    if (pc.connectionState === "failed") {
      setStatus("Connection failed");
      overlay.classList.remove("hidden");
    }
  };

  return pc;
}

function connect() {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  ws = new WebSocket(`${proto}//${location.host}/signal`);

  ws.onopen = () => {
    retryDelay = 500;
    setStatus("Negotiating…");
    pc = newPeerConnection();
    send({ type: "hello", token });
  };

  ws.onmessage = async (event) => {
    let msg;
    try {
      msg = JSON.parse(event.data);
    } catch {
      return;
    }

    switch (msg.type) {
      case "offer": {
        await pc.setRemoteDescription({ type: "offer", sdp: msg.sdp });
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        send({ type: "answer", sdp: answer.sdp });
        break;
      }
      case "ice":
        // Late or duplicate candidates are normal in trickle ICE and must not
        // fail the session, so this deliberately swallows the error.
        await pc.addIceCandidate(msg.candidate).catch(() => {});
        break;
      case "config":
        document.title = `Annex ${msg.w} by ${msg.h}`;
        break;
      case "error":
        setStatus(`Rejected: ${msg.message}`);
        ws.close();
        break;
    }
  };

  // The host may still be starting up, or the laptop may have slept. Retry with
  // a backoff rather than making the user reload.
  ws.onclose = () => {
    setStatus(`Disconnected, retrying in ${Math.round(retryDelay / 100) / 10}s`);
    overlay.classList.remove("hidden");
    document.body.classList.remove("streaming");
    goBtn.hidden = true;
    if (pc) {
      pc.close();
      pc = null;
    }
    setTimeout(connect, retryDelay);
    retryDelay = Math.min(retryDelay * 2, 10000);
  };
}

// A live read of what the browser thinks of the stream. Useful when the picture
// looks wrong: framesDecoded stuck means nothing is arriving, a climbing
// framesDropped means the client cannot keep up.
function startStatsLoop(pc) {
  setInterval(async () => {
    const report = await pc.getStats();
    report.forEach((s) => {
      if (s.type === "inbound-rtp" && s.kind === "video") {
        const kbps = Math.round(((s.bytesReceived || 0) * 8) / 1000);
        statsEl.textContent =
          `${s.frameWidth || 0}x${s.frameHeight || 0} · ` +
          `${Math.round(s.framesPerSecond || 0)} fps · ` +
          `${s.framesDecoded || 0} decoded · ` +
          `${s.framesDropped || 0} dropped · ` +
          `${kbps} kb total`;
      }
    });
  }, 1000);
}

goBtn.onclick = () => {
  document.documentElement.requestFullscreen().catch(() => {});
};

// Pressing a key or clicking is how you would drive the second screen. TODO M5:
// forward these over a DataChannel. Coordinates must be normalised against the
// video element's *content box*, not the window, because object-fit: contain
// letterboxes the video and the two differ.

connect();
