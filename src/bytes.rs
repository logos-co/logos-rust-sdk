//! Canonical C-ABI bytes encoding: `{"_bytes": "<base64url, unpadded>"}`.
//!
//! Binary data crossing the lp_* boundary uses this lossless, NUL-safe
//! tagged form (matching logos-protocol's QVariant<->JSON converter and the
//! plain wire). LIDL `bstr` parameters/returns in generated wrappers encode
//! and decode through these helpers. Pure Rust, no extra dependencies.

const ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(n >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(n >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(n >> 6) as usize & 0x3f] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[n as usize & 0x3f] as char);
        }
    }
    out
}

fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    fn idx(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let s = s.trim_end_matches('=');
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len() * 3 / 4);
    let mut i = 0;
    while i + 4 <= b.len() {
        let n = (idx(b[i])? << 18) | (idx(b[i + 1])? << 12) | (idx(b[i + 2])? << 6) | idx(b[i + 3])?;
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
        i += 4;
    }
    match b.len() - i {
        0 => {}
        2 => {
            let n = (idx(b[i])? << 18) | (idx(b[i + 1])? << 12);
            out.push((n >> 16) as u8);
        }
        3 => {
            let n = (idx(b[i])? << 18) | (idx(b[i + 1])? << 12) | (idx(b[i + 2])? << 6);
            out.push((n >> 16) as u8);
            out.push((n >> 8) as u8);
        }
        _ => return None,
    }
    Some(out)
}

/// Encode binary data as the tagged `{"_bytes": ...}` JSON value.
pub fn encode(bytes: &[u8]) -> serde_json::Value {
    serde_json::json!({ "_bytes": b64url_encode(bytes) })
}

/// Decode a tagged `{"_bytes": ...}` JSON value back to binary data.
/// Returns None when the value is not the canonical single-key form.
pub fn decode(value: &serde_json::Value) -> Option<Vec<u8>> {
    let obj = value.as_object()?;
    if obj.len() != 1 {
        return None;
    }
    b64url_decode(obj.get("_bytes")?.as_str()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_nul_bytes() {
        let data = b"x\x00y".to_vec();
        let v = encode(&data);
        assert_eq!(v["_bytes"], "eAB5");
        assert_eq!(decode(&v).unwrap(), data);
    }

    #[test]
    fn round_trips_all_byte_values() {
        let data: Vec<u8> = (0u8..=255).collect();
        assert_eq!(decode(&encode(&data)).unwrap(), data);
    }

    #[test]
    fn rejects_non_canonical_objects() {
        assert!(decode(&serde_json::json!({"_bytes": "AA", "x": 1})).is_none());
        assert!(decode(&serde_json::json!({"_bytes": 42})).is_none());
        assert!(decode(&serde_json::json!("AA")).is_none());
    }
}
