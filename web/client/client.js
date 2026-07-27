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
    // Shrink the jitter buffer. By default Chrome buffers a few hundred
    // milliseconds to smooth out network jitter, which is right for watching a
    // video and completely wrong for a second monitor: here, latency is the
    // whole product and a dropped frame matters far less than a late one.
    //
    // On a LAN there is almost no jitter to absorb, so asking for zero costs
    // essentially nothing. Two APIs, because browsers disagree on which they
    // support; setting both is harmless.
    try {
      event.receiver.jitterBufferTarget = 0;
    } catch {}
    try {
      event.receiver.playoutDelayHint = 0;
    } catch {}

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

  attachInput(pc);

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

// ---------------------------------------------------------------------------
// Input forwarding
// ---------------------------------------------------------------------------
//
// Only active when the host opened an "input" DataChannel, which it does only
// when input is enabled there. The client cannot turn this on by itself.

let inputChannel = null;

function attachInput(pc) {
  pc.ondatachannel = (event) => {
    if (event.channel.label !== "input") return;
    inputChannel = event.channel;
    inputChannel.onclose = () => {
      inputChannel = null;
    };
    document.body.classList.add("interactive");
  };
}

function sendInput(obj) {
  if (inputChannel && inputChannel.readyState === "open") {
    inputChannel.send(JSON.stringify(obj));
  }
}

// Where the picture actually is inside the element.
//
// `object-fit: contain` letterboxes the video, so the element's box and the
// picture's box are different rectangles. Normalising against the element gives
// coordinates that are wrong by the size of the bars, and the error grows the
// further the aspect ratios diverge. This computes the content box instead.
function pointToVideo(clientX, clientY) {
  const r = video.getBoundingClientRect();
  const vw = video.videoWidth;
  const vh = video.videoHeight;
  if (!vw || !vh) return null;

  const scale = Math.min(r.width / vw, r.height / vh);
  const shownW = vw * scale;
  const shownH = vh * scale;
  const offsetX = r.left + (r.width - shownW) / 2;
  const offsetY = r.top + (r.height - shownH) / 2;

  const x = (clientX - offsetX) / shownW;
  const y = (clientY - offsetY) / shownH;
  // Outside the picture, in the letterbox bars.
  if (x < 0 || x > 1 || y < 0 || y > 1) return null;
  return { x, y };
}

const BUTTONS = ["left", "middle", "right"];

function mods(e) {
  return { shift: e.shiftKey, ctrl: e.ctrlKey, alt: e.altKey, meta: e.metaKey };
}

video.addEventListener("mousemove", (e) => {
  const p = pointToVideo(e.clientX, e.clientY);
  if (p) sendInput({ kind: "mouseMove", x: p.x, y: p.y });
});

video.addEventListener("mousedown", (e) => {
  const p = pointToVideo(e.clientX, e.clientY);
  if (!p) return;
  e.preventDefault();
  // Position first: a click carries no coordinates of its own, so it lands
  // wherever the host's cursor already is.
  sendInput({ kind: "mouseMove", x: p.x, y: p.y });
  sendInput({ kind: "mouseButton", btn: BUTTONS[e.button] || "left", down: true });
});

window.addEventListener("mouseup", (e) => {
  // On window, not the video: releasing outside the element still has to end
  // the drag, or the host is left with a button held down forever.
  if (!inputChannel) return;
  sendInput({ kind: "mouseButton", btn: BUTTONS[e.button] || "left", down: false });
});

// Right-click should reach the Mac rather than open the browser's own menu.
video.addEventListener("contextmenu", (e) => {
  if (inputChannel) e.preventDefault();
});

video.addEventListener(
  "wheel",
  (e) => {
    if (!inputChannel) return;
    e.preventDefault();
    sendInput({ kind: "scroll", dx: e.deltaX, dy: e.deltaY });
  },
  { passive: false }
);

function onKey(down) {
  return (e) => {
    if (!inputChannel) return;
    // Leave the browser's own escape hatch alone, or there is no way out of
    // full screen without killing the tab.
    if (e.key === "Escape" && !document.fullscreenElement) return;
    e.preventDefault();
    // `code` is the physical key, so the Mac's own layout decides what it
    // produces. Sending `key` would mean the client's layout had already been
    // applied and the host's could not be.
    sendInput({ kind: "key", code: e.code, down, mods: mods(e) });
  };
}

window.addEventListener("keydown", onKey(true));
window.addEventListener("keyup", onKey(false));

connect();
