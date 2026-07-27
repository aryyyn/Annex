//! The Annex host binary.
//!
//! # Thread layout, which is the whole reason this file is fiddly
//!
//! Apple's frameworks and Rust's async runtime both want to own a thread, and
//! they cannot share one.
//!
//! ScreenCaptureKit, AppKit and the virtual display callbacks all need a thread
//! running a Core Foundation run loop, which in practice means the main thread.
//! Once `NSApplication` takes it over, that thread blocks in the run loop and
//! never returns.
//!
//! So the ordering below is not stylistic. Start tokio on background threads
//! *first*, keep a handle to it, then hand the main thread to the run loop and
//! let it block. The two halves talk over bounded channels: encoded samples
//! flow out from the capture queue to tokio, and phase-2 input events flow back
//! from tokio to the main thread for injection.
//!
//! The one rule that prevents most of the crashes here: never touch AppKit off
//! the main thread.

mod tray;

fn main() {
    // M0: create the virtual display, print its id, sleep, then drop it and
    // confirm the monitor disappears. That is the entire first milestone.
    //
    // M3 onwards:
    //   1. parse config
    //   2. start the tokio runtime on background threads, keep the handle
    //   3. Pipeline::start, which brings up display, capture, encode, transport
    //   4. build the tray menu and show the URL and QR code
    //   5. hand the main thread to the run loop and block
    //   6. on quit, shut the pipeline down so the display is removed cleanly
    todo!("M0: see crates/virtual-display")
}
