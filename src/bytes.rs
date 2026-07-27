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

/// Accept the shapes a C++ provider accepts for a `bstr` argument, so the same
/// call answers the same way whichever language implements the module.
///
/// Canonical tagged form, plus:
///   * a JSON string  -> its raw bytes (a Qt consumer passing a QString, a CLI arg)
///   * a JSON number  -> its decimal text as bytes (QVariant(int)->QByteArray parity)
///   * an array of ints -> those byte values
///
/// `None` only for shapes no layer produces for bytes (bool, null, a non-tagged
/// object). Mirrors logos-protocol's `bytesFromJsonLenient`; keep the two in step.
pub fn decode_lenient(value: &serde_json::Value) -> Option<Vec<u8>> {
    if let Some(bytes) = decode(value) {
        return Some(bytes);
    }
    match value {
        serde_json::Value::String(s) => Some(s.as_bytes().to_vec()),
        serde_json::Value::Number(n) => Some(n.to_string().into_bytes()),
        serde_json::Value::Array(items) => Some(
            items
                .iter()
                .filter_map(|e| e.as_i64())
                .map(|v| (v & 0xff) as u8)
                .collect(),
        ),
        _ => None,
    }
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

/// The canonical `result` shape, so a Rust provider and a C++ one spell it
/// identically. C++ builds this in the generated glue (`lidlResultToJson`);
/// without a helper here every Rust module hand-rolled it, and a consumer
/// branching on `error` saw `null` from one provider and `""` from another.
///
/// `error` is JSON null when there is no error — never an empty string.
pub mod result {
    use serde_json::{json, Value};

    /// A successful result carrying `value`.
    pub fn ok(value: Value) -> Value {
        json!({ "success": true, "value": value, "error": Value::Null })
    }

    /// A failed result. An empty message still yields null, matching C++'s
    /// `r.error.empty() ? nullptr : r.error`.
    pub fn err(message: &str) -> Value {
        json!({
            "success": false,
            "value": Value::Null,
            "error": if message.is_empty() { Value::Null } else { Value::String(message.to_string()) },
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn matches_the_cpp_spelling() {
            let o = ok(json!(7));
            assert_eq!(o["success"], json!(true));
            assert_eq!(o["value"], json!(7));
            assert!(o["error"].is_null());

            let e = err("boom");
            assert_eq!(e["success"], json!(false));
            assert!(e["value"].is_null());
            assert_eq!(e["error"], json!("boom"));

            // An empty message is null, not "".
            assert!(err("")["error"].is_null());
        }
    }
}

#[cfg(test)]
mod parity_tests {
    use super::*;

    // The same vectors logos-protocol pins in tests/protocol/test_codec.cpp.
    // If one side changes, both must.
    const SPAN: [u8; 4] = [0x00, 0x7f, 0x80, 0xff];
    const SPAN_B64: &str = "AH-A_w";

    #[test]
    fn canonical_vectors_match_cpp() {
        assert_eq!(encode(&SPAN)["_bytes"], SPAN_B64);
        assert_eq!(decode(&encode(&SPAN)).unwrap(), SPAN.to_vec());
        assert_eq!(encode(&[])["_bytes"], "");
        assert!(decode(&encode(&[])).unwrap().is_empty());
    }

    #[test]
    fn padded_input_decodes_like_cpp() {
        assert_eq!(
            decode(&serde_json::json!({"_bytes": "AH-A_w=="})).unwrap(),
            SPAN.to_vec()
        );
    }

    #[test]
    fn lenient_accepts_what_cpp_providers_accept() {
        assert_eq!(decode_lenient(&serde_json::json!("ab")).unwrap(), b"ab".to_vec());
        assert_eq!(decode_lenient(&serde_json::json!(12)).unwrap(), b"12".to_vec());
        assert_eq!(
            decode_lenient(&serde_json::json!([0, 255])).unwrap(),
            vec![0x00, 0xff]
        );
        // The canonical form still wins over the array reading.
        assert_eq!(decode_lenient(&encode(&SPAN)).unwrap(), SPAN.to_vec());
        // Shapes no layer produces for bytes stay rejected.
        assert!(decode_lenient(&serde_json::json!(true)).is_none());
        assert!(decode_lenient(&serde_json::json!({"x": 1})).is_none());
    }
}
