//! Writing captured frames to PNG.
//!
//! M1 only. The whole point of the milestone is producing a file you can open
//! and look at, which is the difference between "the code ran without erroring"
//! and "the capture actually contains the desktop".
//!
//! Nothing downstream will do this. From M2 the frame goes to VideoToolbox as a
//! GPU buffer and is never read by the CPU.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

/// Writes tightly packed BGRA pixels as an RGB PNG.
///
/// Two conversions happen here, and both are easy to get wrong:
///
/// - **Channel order.** Apple gives us blue, green, red, alpha. PNG wants red,
///   green, blue. Skipping the swap produces an image that looks right in shape
///   but has the reds and blues traded, which reads as a plausible photo of a
///   slightly wrong world and is easy to miss at a glance.
/// - **Alpha.** Screen captures are fully opaque, so the alpha channel is a
///   constant 255. Dropping it saves a quarter of the file for nothing lost.
pub fn write_bgra(path: &Path, bgra: &[u8], width: usize, height: usize) -> std::io::Result<()> {
    let mut rgb = vec![0u8; width * height * 3];
    for (src, dst) in bgra.chunks_exact(4).zip(rgb.chunks_exact_mut(3)) {
        dst[0] = src[2]; // R
        dst[1] = src[1]; // G
        dst[2] = src[0]; // B
    }

    let file = File::create(path)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);

    let mut writer = encoder
        .write_header()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    writer
        .write_image_data(&rgb)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}
