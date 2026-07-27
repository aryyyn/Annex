//! The menu bar icon, drawn in code rather than shipped as a file.
//!
//! `tray-icon` wants raw RGBA, so generating it here avoids a binary asset in
//! the repository and keeps the whole app a single self-contained executable.
//!
//! macOS renders a **template image** as a monochrome mask, recolouring it to
//! match the menu bar in light or dark mode. That is why only the alpha channel
//! carries the shape: the RGB is black throughout and the system decides the
//! actual colour. Baking in a colour would look wrong in one of the two modes.

/// Menu bar icons are measured in points; at 2x this is a 36 pixel bitmap.
const SIZE: usize = 36;

/// A small display glyph: a rounded screen with a stand.
pub fn tray_rgba() -> (Vec<u8>, u32, u32) {
    let mut px = vec![0u8; SIZE * SIZE * 4];

    let set = |px: &mut Vec<u8>, x: usize, y: usize, a: u8| {
        if x < SIZE && y < SIZE {
            let i = (y * SIZE + x) * 4;
            // Black with the shape in alpha, so macOS can tint it.
            px[i] = 0;
            px[i + 1] = 0;
            px[i + 2] = 0;
            px[i + 3] = px[i + 3].max(a);
        }
    };

    // Screen body: an outlined rounded rectangle.
    let (x0, y0, x1, y1) = (4usize, 7usize, 31usize, 25usize);
    let border = 3usize;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let edge = x < x0 + border || x > x1 - border || y < y0 + border || y > y1 - border;
            // Knock the corners off so it reads as rounded rather than boxy.
            let corner = (x < x0 + border && y < y0 + border)
                || (x > x1 - border && y < y0 + border)
                || (x < x0 + border && y > y1 - border)
                || (x > x1 - border && y > y1 - border);
            if edge && !corner {
                set(&mut px, x, y, 255);
            }
        }
    }

    // Stand: a neck and a foot.
    for y in 26..=28 {
        for x in 15..=20 {
            set(&mut px, x, y, 255);
        }
    }
    for y in 29..=31 {
        for x in 10..=25 {
            set(&mut px, x, y, 255);
        }
    }

    (px, SIZE as u32, SIZE as u32)
}

/// Renders a QR code for `text` as an RGBA image.
///
/// The point of the QR is that a phone or tablet can join without anyone typing
/// an IP address and port, which is the least pleasant part of a LAN-only
/// design. The quiet zone matters: without a margin of background around the
/// pattern many scanners simply will not lock on.
pub fn qr_rgba(text: &str, scale: usize) -> Option<(Vec<u8>, u32, u32)> {
    use qrcode::{EcLevel, QrCode};

    let code = QrCode::with_error_correction_level(text, EcLevel::M).ok()?;
    let modules = code.to_colors();
    let n = (modules.len() as f64).sqrt() as usize;
    if n * n != modules.len() {
        return None;
    }

    const QUIET: usize = 4;
    let dim = (n + QUIET * 2) * scale;
    let mut px = vec![255u8; dim * dim * 4];

    for y in 0..n {
        for x in 0..n {
            if modules[y * n + x] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px_x = (x + QUIET) * scale + dx;
                        let px_y = (y + QUIET) * scale + dy;
                        let i = (px_y * dim + px_x) * 4;
                        px[i] = 0;
                        px[i + 1] = 0;
                        px[i + 2] = 0;
                        px[i + 3] = 255;
                    }
                }
            }
        }
    }
    Some((px, dim as u32, dim as u32))
}

/// A QR code as monospaced text, for the terminal.
///
/// Two half-blocks per character cell, so one line of text carries two rows of
/// modules and the code comes out roughly square instead of stretched to twice
/// its height.
pub fn qr_text(text: &str) -> Option<String> {
    use qrcode::{EcLevel, QrCode};

    let code = QrCode::with_error_correction_level(text, EcLevel::M).ok()?;
    let modules = code.to_colors();
    let n = (modules.len() as f64).sqrt() as usize;
    if n * n != modules.len() {
        return None;
    }

    const QUIET: usize = 2;
    let dark = |x: isize, y: isize| -> bool {
        if x < 0 || y < 0 || x >= n as isize || y >= n as isize {
            return false;
        }
        modules[y as usize * n + x as usize] == qrcode::Color::Dark
    };

    let mut out = String::new();
    let lo = -(QUIET as isize);
    let hi = (n + QUIET) as isize;
    let mut y = lo;
    while y < hi {
        out.push_str("    ");
        for x in lo..hi {
            let top = dark(x, y);
            let bottom = dark(x, y + 1);
            // Inverted deliberately: terminals are usually dark, and a QR
            // scanner needs dark modules on a light field.
            out.push(match (top, bottom) {
                (true, true) => ' ',
                (true, false) => '▄',
                (false, true) => '▀',
                (false, false) => '█',
            });
        }
        out.push('\n');
        y += 2;
    }
    Some(out)
}
