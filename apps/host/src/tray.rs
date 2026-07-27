//! Menu bar UI: the connect URL, a QR code for it, the auth token, and quit.
//!
//! All of this is AppKit underneath, so every function here must be called on
//! the main thread.

/// Quit has to route through a clean pipeline shutdown rather than exiting the
/// process directly. Killing the process without dropping the `VirtualDisplay`
/// leaves a ghost monitor the user cannot remove without logging out.
#[allow(dead_code)] // wired up at M6, when main() stops being a todo!()
pub fn build() {
    todo!("M6: tray-icon and muda")
}
