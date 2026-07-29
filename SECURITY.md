# Security Policy

Annex streams a live desktop over the local network. The threat it defends against is someone
watching your screen, not a stolen session key. The security model is summarized in the
[README](README.md#security) and specified in full in section 11 of the design document
(`docs/architecture.html`).

## Reporting a vulnerability

Please do not open a public issue for security problems.

Use GitHub's private vulnerability reporting: go to the repository's **Security** tab and
choose **Report a vulnerability**. That opens a private advisory visible only to the
maintainer. Include the version or commit, your macOS version, and the steps to reproduce.

Expect an acknowledgement within a few days. Once a fix is ready, the advisory and a patched
release are published together.

## What counts as a vulnerability

Anything that lets a party who is not the person at the keyboard see the screen, drive the
input, or bypass one of the guarantees below.

The current guarantees, audited 27 July 2026 and covered by tests in
`crates/transport/src/auth.rs`:

- **A fresh 128-bit token is required on every launch.** It is generated from the OS CSPRNG
  and the handshake refuses without it.
- **The WebSocket `Origin` is checked**, so a page you merely visited cannot open a session
  and receive your screen. A forged origin gets 403.
- **`Host` must be a literal IP or localhost**, which closes DNS rebinding. A mismatch gets 421.
- **Tokens are compared in constant time**, there is a ceiling on concurrent clients, and a cap
  on signalling message size. Rejected handshakes are counted so probing is visible.

WebRTC media is always encrypted with DTLS-SRTP. Signalling is plain `ws://` on the LAN by
deliberate choice: it carries no media, and reading it does not help decrypt anything.

A regression that weakens any of the above is a security bug even if no external attacker is
demonstrated. If a test in `auth.rs` starts failing, treat it as real.

## Known and accepted residual risks

These are documented, not oversights. Reporting them is welcome only if you have found a way
to make them worse than described.

- The token rides in the URL query string so a QR code can carry it, so it lands in the
  client browser's history. It is regenerated at every launch to limit the value of a leaked one.
- Annex is LAN only. It assumes the local network is not fully hostile; it does not defend a
  session against an attacker who already has the token and network access.

## Scope

In scope: the host binary, the transport and input crates, the web client, and the bundling
script. Out of scope: the private-API behaviour of macOS itself, and vulnerabilities that
require the attacker to already control the user's Mac.
