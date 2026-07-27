//! A captured frame, and the CPU readback M1 needs to write PNGs.
//!
//! # Why `RawFrame` is opaque
//!
//! Nothing outside this crate and the encoder should be able to reach the
//! pixels, because reaching them means copying them off the GPU. Frames arrive
//! as `CVPixelBuffer`, which is GPU-resident, and VideoToolbox accepts the same
//! type, so the whole capture-to-encode path can be a handoff with no copy.
//!
//! [`RawFrame::to_bgra_vec`] deliberately breaks that, and exists only so M1
//! can prove the source works by writing an image you can open. Real streaming
//! never calls it.
//!
//! # Locking
//!
//! Reading a `CVPixelBuffer` from the CPU means locking its base address, which
//! blocks the GPU from touching it. The lock is held for as short a span as
//! possible and always released, including on an early return.

use objc2_core_media::CMSampleBuffer;
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};

use objc2_core_foundation::CFRetained;

/// One captured frame, still on the GPU.
pub struct RawFrame {
    pub(crate) buffer: CFRetained<CVPixelBuffer>,
    /// Presentation timestamp in microseconds on the capture clock.
    pub pts_us: i64,
}

impl RawFrame {
    pub fn width(&self) -> usize {
        CVPixelBufferGetWidth(&self.buffer)
    }

    pub fn height(&self) -> usize {
        CVPixelBufferGetHeight(&self.buffer)
    }

    /// The FourCC pixel format, for example `BGRA` or `420v`.
    pub fn format(&self) -> u32 {
        CVPixelBufferGetPixelFormatType(&self.buffer)
    }

    /// Bytes per row, which is **not** `width * 4`.
    ///
    /// CoreVideo pads each row out to a hardware-friendly alignment, so a
    /// 1920-wide BGRA buffer often has a stride of 7680 but can be larger. Any
    /// code that assumes `width * 4` produces a picture that shears
    /// progressively further sideways down the image, which is a memorable way
    /// to learn what stride means.
    pub fn bytes_per_row(&self) -> usize {
        CVPixelBufferGetBytesPerRow(&self.buffer)
    }

    /// Copies the frame to the CPU as tightly packed BGRA.
    ///
    /// M1 only. This is the copy the rest of the pipeline exists to avoid.
    /// Returns `None` if the buffer is not BGRA or cannot be locked.
    pub fn to_bgra_vec(&self) -> Option<(Vec<u8>, usize, usize)> {
        if self.format() != u32::from_be_bytes(*b"BGRA") {
            return None;
        }

        let w = self.width();
        let h = self.height();
        let stride = self.bytes_per_row();

        // SAFETY: locked for read only, and unlocked on every path below.
        let flags = CVPixelBufferLockFlags::ReadOnly;
        let lock = unsafe { CVPixelBufferLockBaseAddress(&self.buffer, flags) };
        if lock != 0 {
            return None;
        }

        let base = CVPixelBufferGetBaseAddress(&self.buffer);
        if base.is_null() {
            unsafe { CVPixelBufferUnlockBaseAddress(&self.buffer, flags) };
            return None;
        }

        // Copy row by row rather than in one block: the source is padded to
        // `stride`, the destination is packed to `w * 4`.
        let mut out = vec![0u8; w * h * 4];
        for row in 0..h {
            let src = unsafe { (base as *const u8).add(row * stride) };
            let dst = &mut out[row * w * 4..(row + 1) * w * 4];
            // SAFETY: `src` has at least `w * 4` readable bytes, since
            // `stride >= w * 4` by definition, and the row is in bounds.
            unsafe { std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), w * 4) };
        }

        unsafe { CVPixelBufferUnlockBaseAddress(&self.buffer, flags) };
        Some((out, w, h))
    }

    /// Mean brightness across the frame, 0.0 to 1.0.
    ///
    /// A cheap way to tell "captured actual content" from "captured a black
    /// rectangle", which is the difference between the pipeline working and the
    /// pipeline appearing to work. Sampled, not exhaustive.
    pub fn mean_luma(&self) -> Option<f32> {
        let (px, w, h) = self.to_bgra_vec()?;
        if w == 0 || h == 0 {
            return None;
        }
        // Every 64th pixel is plenty to distinguish black from not-black.
        let mut sum = 0u64;
        let mut n = 0u64;
        for chunk in px.chunks_exact(4 * 64) {
            let (b, g, r) = (chunk[0] as u64, chunk[1] as u64, chunk[2] as u64);
            // Rec. 601 luma, integer weights out of 1000.
            sum += (299 * r + 587 * g + 114 * b) / 1000;
            n += 1;
        }
        if n == 0 {
            return None;
        }
        Some(sum as f32 / n as f32 / 255.0)
    }
}

/// Pulls the image buffer out of a delivered sample, if it has one.
///
/// Returns `None` for status-only samples. ScreenCaptureKit sends those when
/// nothing changed, and they legitimately carry no pixels.
pub(crate) fn from_sample(sample: &CMSampleBuffer) -> Option<RawFrame> {
    // No cast needed: objc2-core-video declares
    // `pub type CVPixelBuffer = CVImageBuffer`, so these are one type. In
    // CoreVideo itself CVImageBufferRef and CVPixelBufferRef are both typedefs
    // of CVBufferRef, and the bindings preserve that.
    let buffer: CFRetained<CVPixelBuffer> = unsafe { sample.image_buffer() }?;

    let pts = unsafe { sample.presentation_time_stamp() };
    let pts_us = if pts.timescale > 0 {
        (pts.value as i128 * 1_000_000 / pts.timescale as i128) as i64
    } else {
        0
    };

    Some(RawFrame { buffer, pts_us })
}
