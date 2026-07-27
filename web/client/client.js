// Annex browser client.
//
// The whole job: open a WebSocket, complete the WebRTC handshake, and attach
// the incoming track to the video element. Roughly 150 lines of plain
// JavaScript, no framework and no build step. It is served by the host itself,
// so there is nothing to install on the client machine.

const video   = document.getElementById("screen");
const overlay = document.getElementById("overlay");
const status  = document.getElementById("status");
const goBtn   = document.getElementById("go");

// The token travels in the query string so the QR code can carry it. It is a
// LAN-only shared secret, not a credential worth protecting in transit.
const token = new URLSearchParams(location.search).get("token");

// No STUN and no TURN. Both machines are on the same network, so host
// candidates connect directly. This is what makes setup near instant.
const pc = new RTCPeerConnection({ iceServers: [] });

let ws;

function setStatus(text) {
  status.textContent = text;
}

function connect() {
  ws = new WebSocket(`ws://${location.host}/signal`);

  ws.onopen = () => {
    setStatus("Negotiating…");
    send({ type: "hello", token });
  };

  ws.onmessage = async (event) => {
    const msg = JSON.parse(event.data);

    switch (msg.type) {
      case "offer": {
        await pc.setRemoteDescription({ type: "offer", sdp: msg.sdp });
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        send({ type: "answer", sdp: answer.sdp });
        break;
      }
      case "ice":
        // Ignore late or duplicate candidates rather than failing the session.
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

  // The host may still be starting up, or the laptop may have slept. Retry
  // rather than making the user reload.
  ws.onclose = () => {
    setStatus("Disconnected. Retrying…");
    overlay.classList.remove("hidden");
    document.body.classList.remove("streaming");
    setTimeout(connect, 2000);
  };
}

function send(obj) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(obj));
}

pc.onicecandidate = ({ candidate }) => {
  if (candidate) send({ type: "ice", candidate });
};

pc.ontrack = (event) => {
  video.srcObject = event.streams[0];
  setStatus("Connected");
  overlay.classList.add("hidden");
  document.body.classList.add("streaming");

  // Full screen needs a user gesture in every browser, so it cannot be done
  // automatically. Offer a button instead.
  goBtn.hidden = false;
};

pc.onconnectionstatechange = () => {
  if (pc.connectionState === "failed") setStatus("Connection failed");
};

goBtn.onclick = () => {
  document.documentElement.requestFullscreen().catch(() => {});
};

// TODO M5: forward pointer and key events over a DataChannel. Send coordinates
// normalised against the video element's content box, not the window, because
// object-fit: contain letterboxes the video and the two differ.

connect();
