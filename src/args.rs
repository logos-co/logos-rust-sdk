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
//! A wrong argument COUNT reports `invalid_args` — see [`invalid_args`]. Both
//! languages changed together, so neither starts rejecting inputs the other
//! still accepts.
//!
//! Unknown method names deliberately keep the NULL reply. The Qt glue maps NULL
//! to an empty QVariant, and a caller that optimistically calls an optional
//! lifecycle hook and reads "no value" as "not implemented" would misread a
//! structured error as a real return value. `logos_module_get_methods` is the
//! supported way to ask what exists.

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

/// LIDL `any`: passed through untouched — `any` stops the recursion in
/// logos_codec.h too, so whatever the peer sent arrives verbatim.
pub fn as_value(args: &[Value], index: usize) -> Value {
    get(args, index).clone()
}

/// A LIDL type, as a runtime descriptor.
///
/// Rust module authors receive composites as `&serde_json::Value` — retyping them
/// to `Vec<i64>` / `HashMap<String, Vec<u8>>` would change every existing
/// module's trait signatures. So instead of decoding INTO a Rust type the way
/// C++'s `Codec<T>` does, the generated dispatch validates the value against the
/// declared LIDL type and then passes it through. Same acceptance, same error
/// messages, no source churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Int,
    Uint,
    Float64,
    Bool,
    Tstr,
    Bstr,
    /// Stops recursion, like `any` in logos_codec.h.
    Any,
    /// `[T]`
    Arr(&'static Ty),
    /// `{tstr: T}` — only the value type, since keys are always tstr.
    Map(&'static Ty),
    /// A record: its declared fields. A field that is absent decodes as null and
    /// is reported by name, matching the C++ codec's `arg0.field` path.
    Record(&'static [(&'static str, &'static Ty)]),
}

fn check(value: &Value, ty: &Ty, path: &str) -> Result<(), String> {
    let mismatch_at = |expected: &str| {
        Err(format!("expected {} at {}, got {}", expected, path, type_name(value)))
    };
    match ty {
        Ty::Any => Ok(()),
        Ty::Int => {
            if value.is_i64() || value.is_u64() { Ok(()) } else { mismatch_at("integer") }
        }
        Ty::Uint => {
            if value.is_u64() { Ok(()) } else { mismatch_at("integer") }
        }
        // An integral JSON number is a valid float64 — JSON has one number type.
        Ty::Float64 => if value.is_number() { Ok(()) } else { mismatch_at("number") },
        Ty::Bool => if value.is_boolean() { Ok(()) } else { mismatch_at("bool") },
        Ty::Tstr => if value.is_string() { Ok(()) } else { mismatch_at("string") },
        // Bytes keep the lenient set, exactly as a C++ provider does.
        Ty::Bstr => {
            if crate::bytes::decode_lenient(value).is_some() { Ok(()) } else { mismatch_at("bytes") }
        }
        Ty::Arr(elem) => match value.as_array() {
            None => mismatch_at("array"),
            Some(items) => {
                for (i, item) in items.iter().enumerate() {
                    check(item, elem, &format!("{}[{}]", path, i))?;
                }
                Ok(())
            }
        },
        Ty::Record(fields) => match value.as_object() {
            None => mismatch_at("object"),
            Some(entries) => {
                for (name, fty) in *fields {
                    let v = entries.get(*name).unwrap_or(&Value::Null);
                    check(v, fty, &format!("{}.{}", path, name))?;
                }
                Ok(())
            }
        },
        Ty::Map(val) => match value.as_object() {
            None => mismatch_at("object"),
            Some(entries) => {
                for (k, v) in entries {
                    check(v, val, &format!("{}.{}", path, k))?;
                }
                Ok(())
            }
        },
    }
}

/// A composite argument, validated against its declared LIDL type before it is
/// handed to the module. Reports the same path a C++ provider does (`arg1[0]`,
/// `arg1.key`), so a `[int]` carrying a string fails identically in both.
pub fn as_value_checked(args: &[Value], index: usize, ty: &Ty) -> Result<Value, String> {
    let v = get(args, index);
    check(v, ty, &at(index))?;
    Ok(v.clone())
}

/// A malformed call: the method exists but the argument count is wrong. Reports
/// why, instead of the NULL reply this used to be — the Qt glue turns NULL into
/// an empty QVariant, so "you passed 2 of 4 arguments" looked like a successful
/// empty answer. Same code and message as the C++ generated glue.
pub fn invalid_args(origin: &str, expected: usize, got: usize) -> Value {
    serde_json::json!({
        "code": "invalid_args",
        "message": format!("expected {} arguments, got {}", expected, got),
        "origin": origin,
    })
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
    fn composites_are_validated_recursively_like_cpp() {
        // [int] with a bad element reports the element's path.
        let args = vec![json!([1, "x"])];
        assert_eq!(
            as_value_checked(&args, 0, &Ty::Arr(&Ty::Int)).unwrap_err(),
            "expected integer at arg0[1], got string"
        );
        // Wrong container shape entirely.
        assert_eq!(
            as_value_checked(&vec![json!("nope")], 0, &Ty::Arr(&Ty::Int)).unwrap_err(),
            "expected array at arg0, got string"
        );
        assert_eq!(
            as_value_checked(&vec![json!([])], 0, &Ty::Map(&Ty::Int)).unwrap_err(),
            "expected object at arg0, got array"
        );
        // {tstr: T} reports the offending key.
        assert_eq!(
            as_value_checked(&vec![json!({"k": "x"})], 0, &Ty::Map(&Ty::Int)).unwrap_err(),
            "expected integer at arg0.k, got string"
        );
        // Nesting composes, and bytes keep the lenient set at any depth.
        assert!(as_value_checked(&vec![json!([[1, 2], []])], 0, &Ty::Arr(&Ty::Arr(&Ty::Int))).is_ok());
        assert!(as_value_checked(
            &vec![json!([crate::bytes::encode(&[0x80]), "raw"])],
            0,
            &Ty::Arr(&Ty::Bstr)
        )
        .is_ok());
        // `any` stops the recursion — anything goes, as in logos_codec.h.
        assert!(as_value_checked(&vec![json!([true, {"a": 1}])], 0, &Ty::Arr(&Ty::Any)).is_ok());
        // Empty containers are valid.
        assert!(as_value_checked(&vec![json!([])], 0, &Ty::Arr(&Ty::Int)).is_ok());
    }

    #[test]
    fn records_are_validated_field_by_field() {
        static PORT: Ty = Ty::Uint;
        static BLOB: Ty = Ty::Bstr;
        static STATUS: Ty = Ty::Record(&[("port", &PORT), ("blob", &BLOB)]);

        assert!(as_value_checked(&vec![json!({"port": 1, "blob": {"_bytes": "gAE"}})], 0, &STATUS).is_ok());
        // A wrong field type reports the FIELD path, like the C++ codec.
        assert_eq!(
            as_value_checked(&vec![json!({"port": "x", "blob": ""})], 0, &STATUS).unwrap_err(),
            "expected integer at arg0.port, got string"
        );
        // A missing field reads as null and is reported by name.
        assert_eq!(
            as_value_checked(&vec![json!({"port": 1})], 0, &STATUS).unwrap_err(),
            "expected bytes at arg0.blob, got null"
        );
        // Not an object at all.
        assert_eq!(
            as_value_checked(&vec![json!([])], 0, &STATUS).unwrap_err(),
            "expected object at arg0, got array"
        );
    }

    #[test]
    fn invalid_args_shape_matches_cpp() {
        let e = invalid_args("my_module", 4, 2);
        assert_eq!(e["code"], json!("invalid_args"));
        assert_eq!(e["message"], json!("expected 4 arguments, got 2"));
        assert_eq!(e["origin"], json!("my_module"));
    }

    #[test]
    fn dispatch_error_shape_matches_cpp() {
        let e = dispatch_failed("my_module", "expected integer at arg0, got string");
        assert_eq!(e["code"], json!("dispatch_failed"));
        assert_eq!(e["origin"], json!("my_module"));
        assert_eq!(e["message"], json!("expected integer at arg0, got string"));
    }
}
