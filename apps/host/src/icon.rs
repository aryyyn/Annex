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

// ---------------------------------------------------------------------------
// Application icon
// ---------------------------------------------------------------------------
//
// Separate from the menu bar glyph above, which is a monochrome template macOS
// tints itself. This one is the icon in Finder, the Dock and Spotlight, so it
// is full colour and carries the product's accent.
//
// Drawn rather than shipped as a file, so the repository holds no binary asset
// and the icon can never drift from the code that describes it.

/// Accent red, matching the design document.
const ACCENT: [u8; 3] = [0xC0, 0x39, 0x2B];
const ACCENT_DARK: [u8; 3] = [0x96, 0x2B, 0x20];

/// Renders the application icon at `size` pixels square.
///
/// Rasterised at four times the target and box-filtered down. There is no
/// anti-aliasing library here, and without supersampling the rounded corners
/// and the glyph edges come out visibly jagged at the smaller sizes macOS asks
/// for.
pub fn app_icon_rgba(size: usize) -> Vec<u8> {
    const SS: usize = 4;
    let hi = size * SS;
    let mut big = vec![0u8; hi * hi * 4];

    let fsize = hi as f32;
    // macOS icons sit inside their canvas rather than filling it, so the
    // artwork is inset and the corner radius follows the squircle convention.
    let margin = fsize * 0.09;
    let (x0, y0) = (margin, margin);
    let (x1, y1) = (fsize - margin, fsize - margin);
    let radius = (x1 - x0) * 0.225;

    for y in 0..hi {
        for x in 0..hi {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            if !inside_rounded_rect(fx, fy, x0, y0, x1, y1, radius) {
                continue;
            }
            // A vertical ramp so the tile has some depth rather than reading as
            // a flat sticker.
            let t = ((fy - y0) / (y1 - y0)).clamp(0.0, 1.0);
            let px = (y * hi + x) * 4;
            for c in 0..3 {
                let a = ACCENT[c] as f32;
                let b = ACCENT_DARK[c] as f32;
                big[px + c] = (a + (b - a) * t) as u8;
            }
            big[px + 3] = 255;
        }
    }

    // The glyph: a screen with a stand, in white, centred.
    let cx = fsize / 2.0;
    let sw = fsize * 0.46;
    let sh = sw * 0.62;
    let sx0 = cx - sw / 2.0;
    let sx1 = cx + sw / 2.0;
    let sy0 = fsize * 0.30;
    let sy1 = sy0 + sh;
    let stroke = fsize * 0.035;

    let mut white = |x: usize, y: usize| {
        if x < hi && y < hi {
            let px = (y * hi + x) * 4;
            if big[px + 3] > 0 {
                big[px] = 255;
                big[px + 1] = 255;
                big[px + 2] = 255;
            }
        }
    };

    for y in 0..hi {
        for x in 0..hi {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let outer = inside_rounded_rect(fx, fy, sx0, sy0, sx1, sy1, stroke * 1.6);
            let inner = inside_rounded_rect(
                fx,
                fy,
                sx0 + stroke,
                sy0 + stroke,
                sx1 - stroke,
                sy1 - stroke,
                stroke * 0.8,
            );
            if outer && !inner {
                white(x, y);
            }
        }
    }

    // Neck and foot.
    let neck_w = fsize * 0.08;
    for y in (sy1 as usize)..((sy1 + fsize * 0.07) as usize) {
        for x in ((cx - neck_w / 2.0) as usize)..((cx + neck_w / 2.0) as usize) {
            white(x, y);
        }
    }
    let foot_w = fsize * 0.26;
    let foot_y = sy1 + fsize * 0.07;
    for y in (foot_y as usize)..((foot_y + stroke * 1.2) as usize) {
        for x in ((cx - foot_w / 2.0) as usize)..((cx + foot_w / 2.0) as usize) {
            white(x, y);
        }
    }

    downsample(&big, hi, SS)
}

/// Signed test for a rounded rectangle, corners included.
fn inside_rounded_rect(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> bool {
    if x < x0 || x > x1 || y < y0 || y > y1 {
        return false;
    }
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= r * r + 0.001
}

/// Box filter by an integer factor, averaging in straight RGBA.
fn downsample(src: &[u8], src_dim: usize, factor: usize) -> Vec<u8> {
    let dst_dim = src_dim / factor;
    let mut out = vec![0u8; dst_dim * dst_dim * 4];
    let n = (factor * factor) as u32;
    for y in 0..dst_dim {
        for x in 0..dst_dim {
            let mut acc = [0u32; 4];
            for sy in 0..factor {
                for sx in 0..factor {
                    let i = ((y * factor + sy) * src_dim + (x * factor + sx)) * 4;
                    for c in 0..4 {
                        acc[c] += src[i + c] as u32;
                    }
                }
            }
            let o = (y * dst_dim + x) * 4;
            for c in 0..4 {
                out[o + c] = (acc[c] / n) as u8;
            }
        }
    }
    out
}

/// Writes the iconset macOS expects, for `iconutil` to compile into an `.icns`.
///
/// The names are fixed by the tool: it will not accept anything else.
pub fn write_iconset(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    // (points, scale) pairs. The 2x entries are what Retina displays use.
    const WANTED: [(usize, usize); 10] = [
        (16, 1),
        (16, 2),
        (32, 1),
        (32, 2),
        (128, 1),
        (128, 2),
        (256, 1),
        (256, 2),
        (512, 1),
        (512, 2),
    ];
    for (pt, scale) in WANTED {
        let px = pt * scale;
        let rgba = app_icon_rgba(px);
        let name = if scale == 1 {
            format!("icon_{pt}x{pt}.png")
        } else {
            format!("icon_{pt}x{pt}@2x.png")
        };
        let file = std::fs::File::create(dir.join(name))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), px as u32, px as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc
            .write_header()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        w.write_image_data(&rgba)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    Ok(())
}
