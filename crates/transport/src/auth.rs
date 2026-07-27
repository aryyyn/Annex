//! Access control for the signalling endpoint.
//!
//! # What this is defending against
//!
//! Annex streams your desktop. The threat is not someone stealing a session
//! key: it is someone watching your screen. Two attackers matter, and they are
//! not the same.
//!
//! **Anyone on the same network.** A café, an office, a shared flat. Without a
//! secret, opening the URL is all it takes. This is handled by requiring a
//! token that is generated fresh at every launch and only ever shown to the
//! person at the keyboard, via the menu bar or the QR code.
//!
//! **Any website you happen to visit.** This one is less obvious and worse.
//! WebSockets are deliberately exempt from the same-origin policy: a page on
//! `evil.example` can open a socket to `ws://192.168.1.75:8787/signal` and the
//! browser will let it. Without a check, a page you visit while Annex is
//! running could complete the handshake and receive your screen. Browsers do
//! send an `Origin` header on WebSocket handshakes, which is the hook: we
//! require it to be either absent, meaning a non-browser client, or our own.
//!
//! **DNS rebinding** is the same attack wearing a hat. An attacker points
//! `evil.example` at our LAN address, so the page's origin genuinely *is* the
//! attacker's, and the `Origin` check passes. The `Host` header still names the
//! attacker's domain, though, so requiring a bare IP or localhost closes it.

use subtle::ConstantTimeEq;

/// Length of the generated token in bytes, before encoding.
///
/// 16 bytes is 128 bits. The token has to be typed or scanned by a human, so
/// this trades some length for usability; it is far beyond brute-forceable over
/// a network that also has a connection limit.
const TOKEN_BYTES: usize = 16;

/// Unambiguous alphabet: no `0`/`O`, no `1`/`l`/`I`.
///
/// The token appears in a URL somebody may read off one screen and type into
/// another, so characters that look alike are removed rather than trusted.
const ALPHABET: &[u8] = b"23456789abcdefghijkmnpqrstuvwxyzACDEFGHJKLMNPQRSTUVWXYZ";

/// A fresh token from the operating system's CSPRNG.
///
/// Regenerated on every launch on purpose. A token that outlived the process
/// would be a durable secret worth stealing, and there is no reason to have one
/// when the URL is re-shown at every start.
pub fn generate_token() -> String {
    let mut raw = [0u8; TOKEN_BYTES];
    // A failure here means the OS entropy source is unavailable, which is not a
    // situation to paper over with a weaker fallback.
    getrandom::fill(&mut raw).expect("OS CSPRNG unavailable");
    raw.iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

/// Compares two tokens without leaking the position of the first difference.
///
/// A naive `==` returns as soon as two bytes differ, so how long it takes
/// reveals how much of a guess was right, and an attacker can rebuild the token
/// one character at a time. Over a LAN the timing signal is small but real, and
/// the constant-time version costs nothing.
pub fn token_matches(expected: &str, provided: Option<&str>) -> bool {
    let Some(provided) = provided else {
        return false;
    };
    // Length is not secret: it is a fixed constant of the protocol.
    if provided.len() != expected.len() {
        return false;
    }
    expected.as_bytes().ct_eq(provided.as_bytes()).into()
}

/// Whether a WebSocket `Origin` may open a session.
///
/// Absent is allowed: non-browser clients such as the native client at M7 do
/// not send one, and a browser always does. So absence cannot be forged *by a
/// web page*, which is the attacker this check exists for.
pub fn origin_allowed(origin: Option<&str>, expected_host: &str) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    // `null` is what a sandboxed iframe or a `file://` page sends. Refuse it:
    // it is not our page and carries no useful provenance.
    if origin == "null" {
        return false;
    }
    match origin.split_once("://") {
        Some((scheme, host)) if scheme == "http" || scheme == "https" => host == expected_host,
        _ => false,
    }
}

/// Whether the `Host` header is one we are willing to answer on.
///
/// Only a literal IP address or localhost, never a name. A DNS name reaching us
/// means someone resolved their own domain to this machine, which is the
/// rebinding attack: the page's origin is genuinely theirs, so the `Origin`
/// check cannot help, but the `Host` header still gives them away.
pub fn host_allowed(host: Option<&str>) -> bool {
    let Some(host) = host else {
        // HTTP/1.1 requires Host. Something without one is not a browser.
        return false;
    };
    let name = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    let name = name.trim_start_matches('[').trim_end_matches(']');

    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    name.parse::<std::net::IpAddr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_and_unambiguous() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b, "two launches must not share a token");
        assert_eq!(a.len(), TOKEN_BYTES);
        for c in a.chars() {
            assert!(
                !"0O1lI".contains(c),
                "ambiguous character {c} would be mistyped"
            );
        }
    }

    #[test]
    fn token_compare_rejects_wrong_and_missing() {
        assert!(token_matches("abcdef", Some("abcdef")));
        assert!(!token_matches("abcdef", Some("abcdeg")));
        assert!(!token_matches("abcdef", Some("abcde")));
        assert!(!token_matches("abcdef", None));
        assert!(!token_matches("abcdef", Some("")));
    }

    #[test]
    fn origin_allows_our_own_page_and_non_browsers() {
        assert!(origin_allowed(
            Some("http://192.168.1.75:8787"),
            "192.168.1.75:8787"
        ));
        // Native clients send no Origin.
        assert!(origin_allowed(None, "192.168.1.75:8787"));
    }

    #[test]
    fn origin_rejects_drive_by_websites() {
        // The whole point: a page you visit must not be able to open a session.
        assert!(!origin_allowed(
            Some("https://evil.example"),
            "192.168.1.75:8787"
        ));
        assert!(!origin_allowed(
            Some("http://192.168.1.75:9999"),
            "192.168.1.75:8787"
        ));
        assert!(!origin_allowed(Some("null"), "192.168.1.75:8787"));
        assert!(!origin_allowed(
            Some("chrome-extension://abc"),
            "192.168.1.75:8787"
        ));
    }

    #[test]
    fn host_rejects_dns_rebinding() {
        assert!(host_allowed(Some("192.168.1.75:8787")));
        assert!(host_allowed(Some("127.0.0.1:8787")));
        assert!(host_allowed(Some("localhost:8787")));
        assert!(host_allowed(Some("[::1]:8787")));
        // A name pointed at our address is the rebinding attack.
        assert!(!host_allowed(Some("evil.example:8787")));
        assert!(!host_allowed(Some("annex.attacker.com")));
        assert!(!host_allowed(None));
    }
}
