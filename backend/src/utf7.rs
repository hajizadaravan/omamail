//! IMAP modified UTF-7 (RFC 3501 §5.1.3) for mailbox names, so Gmail labels
//! such as "Şəxsi" display correctly. Paths are sent back to the server
//! exactly as LIST returned them, so only decoding is needed at runtime.

use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;

pub fn decode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('-') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let chunk = &after[..end];
        if chunk.is_empty() {
            out.push('&');
        } else {
            match decode_chunk(chunk) {
                Some(text) => out.push_str(&text),
                None => out.push_str(&rest[start..start + end + 2]),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn decode_chunk(chunk: &str) -> Option<String> {
    let bytes = STANDARD_NO_PAD.decode(chunk.replace(',', "/")).ok()?;
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes.chunks(2).map(|p| u16::from_be_bytes([p[0], p[1]])).collect();
    String::from_utf16(&units).ok()
}

#[cfg(test)]
pub fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        let bytes: Vec<u8> = pending.iter().flat_map(|u| u.to_be_bytes()).collect();
        out.push('&');
        out.push_str(&STANDARD_NO_PAD.encode(bytes).replace('/', ","));
        out.push('-');
        pending.clear();
    };
    for ch in name.chars() {
        if (' '..='~').contains(&ch) {
            flush(&mut pending, &mut out);
            if ch == '&' {
                out.push_str("&-");
            } else {
                out.push(ch);
            }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(ch.encode_utf16(&mut buf));
        }
    }
    flush(&mut pending, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_non_ascii_labels() {
        for name in ["Şəxsi", "İş/Ödənişlər", "R&D", "plain", "日本語 & more", "😀"] {
            assert_eq!(decode(&encode(name)), name, "{name}");
        }
    }

    #[test]
    fn matches_rfc_example() {
        assert_eq!(encode("~peter/mail/台北/日本語"), "~peter/mail/&U,BTFw-/&ZeVnLIqe-");
        assert_eq!(decode("~peter/mail/&U,BTFw-/&ZeVnLIqe-"), "~peter/mail/台北/日本語");
        assert_eq!(encode("R&D"), "R&-D");
    }

    #[test]
    fn keeps_malformed_input() {
        assert_eq!(decode("broken&abc"), "broken&abc");
        assert_eq!(decode("bad&!!!-x"), "bad&!!!-x");
    }
}
