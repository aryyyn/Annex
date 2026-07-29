# Contributing to Annex

Thanks for your interest. Annex is a Rust system that turns a laptop on your LAN into a real
extended display for a Mac. This document covers how to build it, the checks every change has
to pass, and a small number of architectural rules that exist for good reasons.

## Getting set up

Annex is macOS only. It builds against ScreenCaptureKit, VideoToolbox, CoreGraphics and the
`objc2` bindings.

- **Host:** macOS 12.3 or later, Apple Silicon or Intel.
- **Toolchain:** stable Rust. The channel, components and targets are pinned in
  `rust-toolchain.toml`, so `rustup` installs the right thing the first time you run `cargo`.

```bash
git clone git@github.com:aryyyn/Annex.git
cd Annex
cargo build --workspace
cargo test --workspace
```

The milestone harnesses are useful when you are working on one layer in isolation:

```bash
cargo run -p annex-host -- 20     # create a virtual display, hold 20s, remove it
cargo run -p annex-host -- m1 10  # capture 10 frames to ./captures
cargo run -p annex-host -- m2 45  # encode 45 frames to out.h264
cargo run -p annex-host -- m3     # stream the main display, open the printed URL
```

Anything that captures the screen needs the **Screen Recording** permission, and macOS reads
that grant at process start. Under `cargo run` the grant attaches to your terminal, so restart
the terminal after granting it. A signed `.app` bundle attaches it to Annex instead; see
`scripts/bundle.sh` and the README.

## Before you open a pull request

Every change must be green on all three of these. CI enforces them on macOS, so it is faster
to run them locally first:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Warnings are treated as errors. The tests are pure logic and do not need a display or any
permission.

## Architectural rules

These are not style preferences. Each one prevents a specific class of bug.

1. **Private Apple APIs live in exactly one crate.** `crates/virtual-display` is the only
   place allowed to touch `CGVirtualDisplay` and its friends. If a change would make another
   crate call a private API, stop and reconsider the design first. Confining it means a macOS
   update that shifts those APIs breaks one file, not the whole tree.

2. **Respect thread affinity.** AppKit, ScreenCaptureKit, CoreGraphics and the virtual display
   run on the main thread with a CFRunLoop. Tokio and webrtc-rs run on background threads. The
   two worlds talk through bounded channels only. Never call an Apple framework off the main
   thread; `CGEventPost` in particular silently drops events when posted from a worker.

3. **One crate, one responsibility.** Keep the public surface of each crate narrow and
   testable. Package names carry an `annex-` prefix so `annex-core` does not shadow the
   standard `core` crate at `use` sites.

4. **Do not weaken the security invariants.** Annex streams a desktop, so the threat is someone
   watching your screen. A token is always required, the WebSocket `Origin` is checked, `Host`
   must be a literal IP or localhost, tokens are compared in constant time, and there are caps
   on connections and message size. `crates/transport/src/auth.rs` has a test for each of these,
   including the drive-by and DNS-rebinding cases. If one of those tests starts failing,
   something real broke. See [SECURITY.md](SECURITY.md).

5. **Never transcribe the `CGVirtualDisplay` header from memory.** Copy it from DeskPad or
   BetterDummy and cross-check both. Field signatures have drifted across macOS releases.

## Errors and dependencies

- Library crates use `thiserror`. The binaries in `apps/` use `anyhow`.
- Shared dependency versions are declared once in the root `[workspace.dependencies]`.

## Commit and pull request style

- Commit subjects are imperative mood, lowercase, and carry no trailing period.
- Keep each commit focused on one change so history stays reviewable.
- In the pull request, describe what changed and how you verified it. If it touches capture,
  encode, transport or input, say what you ran and what you saw.

## Scope

Annex is LAN only by design: no STUN, no TURN, no relay, no account, and no data leaving your
network. Features that break that model (internet traversal, a cloud relay) are out of scope.
Audio forwarding, a native client, and HEVC are on the roadmap and welcome.

## Filing issues

Bugs and feature requests go through the templates under
[.github/ISSUE_TEMPLATE](.github/ISSUE_TEMPLATE). For anything security sensitive, follow
[SECURITY.md](SECURITY.md) instead of opening a public issue.
