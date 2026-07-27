//! Turning VideoToolbox output into an Annex-B elementary stream.
//!
//! # Two packagings of the same NAL units
//!
//! H.264 data is a sequence of NAL units, but there are two ways to mark where
//! each one begins, and VideoToolbox emits the wrong one for our purposes.
//!
//! **AVCC**, which is what VideoToolbox gives us, prefixes every NAL unit with
//! its length as a big-endian integer:
//!
//! ```text
//! [00 00 0A 2F][NAL bytes ...][00 00 01 F3][NAL bytes ...]
//!  ^ length                    ^ length
//! ```
//!
//! **Annex-B**, which is what WebRTC's H.264 payloader and every command line
//! decoder expect, separates them with a start code instead:
//!
//! ```text
//! [00 00 00 01][NAL bytes ...][00 00 00 01][NAL bytes ...]
//!  ^ start code                ^ start code
//! ```
//!
//! So conversion means walking the buffer, reading each length, and replacing
//! it with a start code. The lengths are consumed, not kept: a decoder finds
//! boundaries by scanning for start codes.
//!
//! # Why SPS and PPS have to be injected
//!
//! In AVCC the sequence and picture parameter sets live *outside* the sample
//! data, in the format description, because a container like MP4 stores them in
//! its header. An elementary stream has no header, so a decoder that joins
//! mid-stream would have no idea of the resolution or profile.
//!
//! We therefore prepend them to every keyframe. That costs a few dozen bytes
//! per keyframe and means any client can start decoding at any keyframe, which
//! is exactly what a viewer joining an in-progress stream needs.

/// The four-byte start code. The three-byte form `00 00 01` is also legal and
/// marginally smaller, but four is what everything emits by convention.
pub const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// Rewrites a length-prefixed AVCC buffer as Annex-B.
///
/// `nal_length_size` is almost always 4, but VideoToolbox reports it and the
/// format permits 1, 2 or 4, so it is read rather than assumed.
///
/// Returns `None` if the buffer is malformed, which in practice means a
/// truncated read rather than a real encoder fault.
pub fn avcc_to_annexb(avcc: &[u8], nal_length_size: usize) -> Option<Vec<u8>> {
    if !(1..=4).contains(&nal_length_size) {
        return None;
    }

    let mut out = Vec::with_capacity(avcc.len() + 16);
    let mut i = 0usize;

    while i + nal_length_size <= avcc.len() {
        let mut len = 0usize;
        for k in 0..nal_length_size {
            len = (len << 8) | avcc[i + k] as usize;
        }
        i += nal_length_size;

        if len == 0 || i + len > avcc.len() {
            return None;
        }

        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&avcc[i..i + len]);
        i += len;
    }

    // Trailing bytes that are not a whole NAL unit mean we misread something.
    if i != avcc.len() {
        return None;
    }
    Some(out)
}

/// Wraps each parameter set in a start code, ready to prepend to a keyframe.
pub fn parameter_sets_to_annexb(sets: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in sets {
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(s);
    }
    out
}

/// The NAL unit type in the low five bits of the header byte.
///
/// Useful for asserting a stream is well formed without decoding it: a valid
/// keyframe carries 7 (SPS), 8 (PPS) and 5 (IDR).
pub fn nal_type(first_byte: u8) -> u8 {
    first_byte & 0x1F
}

/// Human-readable NAL unit type, for diagnostics.
pub fn nal_type_name(t: u8) -> &'static str {
    match t {
        1 => "non-IDR slice",
        5 => "IDR slice",
        6 => "SEI",
        7 => "SPS",
        8 => "PPS",
        9 => "access unit delimiter",
        _ => "other",
    }
}

/// Walks an Annex-B buffer and reports each NAL unit's type and length.
///
/// This is the structural check M2 uses: it proves the bytes really are a
/// parseable elementary stream, independently of any decoder agreeing.
pub fn scan(annexb: &[u8]) -> Vec<(u8, usize)> {
    let mut out = Vec::new();
    let mut starts = Vec::new();

    let mut i = 0usize;
    while i + 3 < annexb.len() {
        if annexb[i] == 0 && annexb[i + 1] == 0 {
            if annexb[i + 2] == 1 {
                starts.push((i + 3, 3usize));
                i += 3;
                continue;
            } else if annexb[i + 2] == 0 && annexb[i + 3] == 1 {
                starts.push((i + 4, 4usize));
                i += 4;
                continue;
            }
        }
        i += 1;
    }

    for (n, (payload_start, _)) in starts.iter().enumerate() {
        let end = starts
            .get(n + 1)
            .map(|(s, code_len)| s - code_len)
            .unwrap_or(annexb.len());
        if *payload_start < end {
            out.push((nal_type(annexb[*payload_start]), end - payload_start));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_two_nals() {
        // Two AVCC units: a 3-byte SPS-ish and a 2-byte payload.
        let avcc = [0, 0, 0, 3, 0x67, 0xAA, 0xBB, 0, 0, 0, 2, 0x65, 0xCC];
        let out = avcc_to_annexb(&avcc, 4).expect("valid");
        assert_eq!(
            out,
            vec![0, 0, 0, 1, 0x67, 0xAA, 0xBB, 0, 0, 0, 1, 0x65, 0xCC]
        );
    }

    #[test]
    fn rejects_truncated_input() {
        // Declares 9 bytes but supplies 2.
        assert!(avcc_to_annexb(&[0, 0, 0, 9, 0x67, 0xAA], 4).is_none());
    }

    #[test]
    fn scan_finds_types_and_lengths() {
        let annexb = [0, 0, 0, 1, 0x67, 0xAA, 0xBB, 0, 0, 0, 1, 0x65, 0xCC];
        let nals = scan(&annexb);
        assert_eq!(nals, vec![(7, 3), (5, 2)]);
    }

    #[test]
    fn scan_handles_three_byte_start_codes() {
        let annexb = [0, 0, 1, 0x68, 0x11, 0, 0, 1, 0x65, 0x22];
        assert_eq!(scan(&annexb), vec![(8, 2), (5, 2)]);
    }

    #[test]
    fn round_trip_preserves_payloads() {
        let avcc = [0, 0, 0, 1, 0x67, 0, 0, 0, 4, 0x65, 1, 2, 3];
        let annexb = avcc_to_annexb(&avcc, 4).expect("valid");
        assert_eq!(scan(&annexb), vec![(7, 1), (5, 4)]);
    }
}
