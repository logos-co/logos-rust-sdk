//! Typed argument access for generated provider dispatch.
//!
//! Generated glue used `Value::as_i64().unwrap_or_default()` and friends, so a
//! wrong-typed argument became `0` / `""` / `false` / empty bytes and the
//! author's method ran on it. A C++ provider rejects the same call with
//! `{"code":"dispatch_failed","message":"expected integer at arg1, got string"}`
//! (logos-protocol's `CodecError`, surfaced by the generated dispatch), so the
//! same consumer got a wrong answer from one language and an error from the
//! other.
//!
//! These accessors fail instead, with the message wording and the `argN` path
//! logos-protocol uses, so the two languages report the same thing.
//!
//! Note what is deliberately NOT changed: too-few-arguments still yields a NULL
//! dispatch reply, because that is what the C++ glue does
//! (`if (args.size() < N) return nullptr;`). Turning it into a structured error
//! here would create the mirror image of the bug this module fixes.

use serde_json::Value;

/// The type names logos-protocol reports, i.e. nlohmann::json's `type_name()`.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn at(index: usize) -> String {
    format!("arg{}", index)
}

fn mismatch(expected: &str, index: usize, value: &Value) -> String {
    format!("expected {} at {}, got {}", expected, at(index), type_name(value))
}

/// The argument at `index`, or `Null` when absent.
pub fn get(args: &[Value], index: usize) -> &Value {
    args.get(index).unwrap_or(&Value::Null)
}

/// LIDL `int`.
pub fn as_i64(args: &[Value], index: usize) -> Result<i64, String> {
    let v = get(args, index);
    v.as_i64().ok_or_else(|| mismatch("integer", index, v))
}

/// LIDL `uint`.
pub fn as_u64(args: &[Value], index: usize) -> Result<u64, String> {
    let v = get(args, index);
    v.as_u64().ok_or_else(|| mismatch("integer", index, v))
}

/// LIDL `float64`. An integral JSON number is accepted — JSON has one number
/// type, so a whole `double` may arrive either way (protocol does the same).
pub fn as_f64(args: &[Value], index: usize) -> Result<f64, String> {
    let v = get(args, index);
    v.as_f64().ok_or_else(|| mismatch("number", index, v))
}

/// LIDL `bool`.
pub fn as_bool(args: &[Value], index: usize) -> Result<bool, String> {
    let v = get(args, index);
    v.as_bool().ok_or_else(|| mismatch("bool", index, v))
}

/// LIDL `tstr`.
pub fn as_string(args: &[Value], index: usize) -> Result<String, String> {
    let v = get(args, index);
    v.as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| mismatch("string", index, v))
}

/// LIDL `bstr`, accepting the same shapes a C++ provider accepts
/// (see [`crate::bytes::decode_lenient`]).
pub fn as_bytes(args: &[Value], index: usize) -> Result<Vec<u8>, String> {
    let v = get(args, index);
    crate::bytes::decode_lenient(v).ok_or_else(|| mismatch("bytes", index, v))
}

/// LIDL `any` / composites: passed through untouched, as before. Typed
/// validation of `[T]` and `{tstr: T}` is not implemented here yet — a C++
/// provider validates those recursively and reports `arg1[0]`, so this is the
/// remaining gap between the two languages.
pub fn as_value(args: &[Value], index: usize) -> Value {
    get(args, index).clone()
}

/// The canonical structured error a failed dispatch returns, matching the object
/// C++ generated glue emits.
pub fn dispatch_failed(origin: &str, message: &str) -> Value {
    serde_json::json!({
        "code": "dispatch_failed",
        "message": message,
        "origin": origin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wrong_type_reports_like_cpp() {
        let args = vec![json!("nope"), json!(7)];
        assert_eq!(
            as_i64(&args, 0).unwrap_err(),
            "expected integer at arg0, got string"
        );
        assert_eq!(
            as_bool(&args, 1).unwrap_err(),
            "expected bool at arg1, got number"
        );
        // A missing argument reports as null rather than panicking.
        assert_eq!(
            as_string(&args, 5).unwrap_err(),
            "expected string at arg5, got null"
        );
    }

    #[test]
    fn right_type_passes_through() {
        let args = vec![json!(-3), json!(4), json!(1.5), json!(true), json!("s")];
        assert_eq!(as_i64(&args, 0).unwrap(), -3);
        assert_eq!(as_u64(&args, 1).unwrap(), 4);
        assert_eq!(as_f64(&args, 2).unwrap(), 1.5);
        assert!(as_bool(&args, 3).unwrap());
        assert_eq!(as_string(&args, 4).unwrap(), "s");
        // An integral number is a valid float64.
        assert_eq!(as_f64(&vec![json!(2)], 0).unwrap(), 2.0);
    }

    #[test]
    fn bytes_keep_the_lenient_set() {
        assert_eq!(as_bytes(&vec![json!("ab")], 0).unwrap(), b"ab".to_vec());
        assert_eq!(
            as_bytes(&vec![crate::bytes::encode(&[0x80])], 0).unwrap(),
            vec![0x80]
        );
        // Shapes no layer produces for bytes now FAIL instead of silently
        // yielding an empty vector.
        assert_eq!(
            as_bytes(&vec![json!(true)], 0).unwrap_err(),
            "expected bytes at arg0, got boolean"
        );
    }

    #[test]
    fn dispatch_error_shape_matches_cpp() {
        let e = dispatch_failed("my_module", "expected integer at arg0, got string");
        assert_eq!(e["code"], json!("dispatch_failed"));
        assert_eq!(e["origin"], json!("my_module"));
        assert_eq!(e["message"], json!("expected integer at arg0, got string"));
    }
}
