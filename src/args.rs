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

/// LIDL `?T` in a positional slot (a method argument).
///
/// `?T` is TWO-state: a value of T, or empty. Decode is liberal — an ABSENT
/// argument (which [`get`] already reads as null) and an explicit null are the
/// same empty state. A PRESENT value must still satisfy `T`: optional widens
/// the domain by exactly one inhabitant and does not disable type checking, so
/// `?tstr` given `42` fails exactly as `tstr` given `42` does.
fn as_opt<T>(
    args: &[Value],
    index: usize,
    decode: fn(&[Value], usize) -> Result<T, String>,
) -> Result<Option<T>, String> {
    if get(args, index).is_null() {
        return Ok(None);
    }
    decode(args, index).map(Some)
}

/// LIDL `?int`.
pub fn as_opt_i64(args: &[Value], index: usize) -> Result<Option<i64>, String> {
    as_opt(args, index, as_i64)
}

/// LIDL `?uint`.
pub fn as_opt_u64(args: &[Value], index: usize) -> Result<Option<u64>, String> {
    as_opt(args, index, as_u64)
}

/// LIDL `?float64`.
pub fn as_opt_f64(args: &[Value], index: usize) -> Result<Option<f64>, String> {
    as_opt(args, index, as_f64)
}

/// LIDL `?bool`.
pub fn as_opt_bool(args: &[Value], index: usize) -> Result<Option<bool>, String> {
    as_opt(args, index, as_bool)
}

/// LIDL `?tstr`.
pub fn as_opt_string(args: &[Value], index: usize) -> Result<Option<String>, String> {
    as_opt(args, index, as_string)
}

/// LIDL `?bstr`.
pub fn as_opt_bytes(args: &[Value], index: usize) -> Result<Option<Vec<u8>>, String> {
    as_opt(args, index, as_bytes)
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
    /// `?T` — the slot may be EMPTY. Two-state: a value of T, or empty. An
    /// absent key and an explicit null are the same empty state; a present
    /// value is still checked against `T`.
    Opt(&'static Ty),
    /// `[T]`
    Arr(&'static Ty),
    /// `{tstr: T}` — only the value type, since keys are always tstr.
    Map(&'static Ty),
    /// A record: its declared fields. A field that is absent decodes as null and
    /// is reported by name, matching the C++ codec's `arg0.field` path — unless
    /// the field is `Opt`, which accepts that null as empty.
    Record(&'static [(&'static str, &'static Ty)]),
}

fn check(value: &Value, ty: &Ty, path: &str) -> Result<(), String> {
    let mismatch_at = |expected: &str| {
        Err(format!("expected {} at {}, got {}", expected, path, type_name(value)))
    };
    match ty {
        Ty::Any => Ok(()),
        // Decode is liberal in an optional slot: absent (already null by the
        // time it reaches here — `get` and the Record arm both coerce a missing
        // slot to null) and explicit null are the SAME empty state. A PRESENT
        // value still has to satisfy T, and reports the mismatch at this exact
        // path: optional widens the domain by one inhabitant, it does not turn
        // type checking off.
        Ty::Opt(inner) => {
            if value.is_null() { Ok(()) } else { check(value, inner, path) }
        }
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

/// The CLOSED SET of `code` values that mark a result as a provider REFUSAL
/// rather than a value. The single source of truth for [`as_dispatch_rejection`]
/// in this crate, and the vocabulary its C++ twins duplicate.
///
/// Why a closed set and not "any {code,message,origin} object": a method may
/// legitimately RETURN a three-string map, and an `any` return certainly can.
/// Matching the shape alone would let user data impersonate a refusal.
///
/// * `dispatch_failed` — the provider ran and refused the argument VALUES; see
///   [`dispatch_failed`].
/// * `invalid_args` — wrong argument COUNT; see [`invalid_args`]. This crate has
///   EMITTED it since arity checking landed, and nothing detected it — an arity
///   error read back to a typed consumer as a successful call returning a map.
/// * `unknown_method` — nothing emits this yet, listed on purpose. An unknown
///   method is currently answered with a bare null, indistinguishable from a
///   legitimate null return, and closing that needs a provider-contract change
///   across the SDKs. Widening a detector is backwards-compatible on its own;
///   a new provider code shipped against narrow detectors would arrive at
///   consumers as DATA — the same silent-success bug, freshly minted.
pub const REJECTION_CODES: [&str; 3] = ["dispatch_failed", "invalid_args", "unknown_method"];

/// The inverse of [`dispatch_failed`]: recognise the canonical rejection object
/// when it arrives as a call's RESULT, and hand back its message.
///
/// A provider that RAN and refused answers this object as its result, not as a
/// transport error, so `lp_invoke` reports success and a consumer that only
/// decodes the value turns the rejection into a default (`0`, `""`, an empty
/// list) — the refusal vanishes. Consumers fold it into their error channel
/// instead; see `PluginProxy::call_json` and friends.
///
/// The match is NARROW — those three fields, all strings, and a code from
/// [`REJECTION_CODES`] — for the same reason the C++ twin
/// (`logosDispatchRejectionJson`, emitted into every generated wrapper) is
/// narrow: a method legitimately returning a map, or an `any`, must never
/// false-match. Anything a user can put in a map would otherwise be enough to
/// fake a failure. An unrecognised code, a 2- or 4-key object, and a non-string
/// value all stay DATA.
pub fn as_dispatch_rejection(value: &Value) -> Option<&str> {
    let obj = value.as_object()?;
    if obj.len() != 3 {
        return None;
    }
    let code = obj.get("code")?.as_str()?;
    let message = obj.get("message")?.as_str()?;
    obj.get("origin")?.as_str()?;
    if !REJECTION_CODES.contains(&code) {
        return None;
    }
    Some(message)
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

    // `?T` is TWO-state: a value of T, or empty. Absent and explicit null are
    // the SAME empty state on decode; a present value is still checked.
    #[test]
    fn optional_slots_accept_empty_but_still_check_a_present_value() {
        static TSTR: Ty = Ty::Tstr;
        static OPT_TSTR: Ty = Ty::Opt(&TSTR);

        // Explicit null decodes as empty...
        assert!(as_value_checked(&vec![json!(null)], 0, &OPT_TSTR).is_ok());
        // ...and so does an absent argument (which `get` reads as null).
        assert!(as_value_checked(&vec![], 0, &OPT_TSTR).is_ok());
        // A value of T is fine, of course.
        assert!(as_value_checked(&vec![json!("hi")], 0, &OPT_TSTR).is_ok());
        // But a PRESENT wrong-typed value is still an error, with the same
        // message the required slot gives: optional widens the domain by
        // exactly one inhabitant, it does not turn type checking off.
        assert_eq!(
            as_value_checked(&vec![json!(42)], 0, &OPT_TSTR).unwrap_err(),
            "expected string at arg0, got number"
        );
        // The required twin still rejects both empties.
        assert_eq!(
            as_value_checked(&vec![json!(null)], 0, &TSTR).unwrap_err(),
            "expected string at arg0, got null"
        );

        // The scalar accessors have the same two-state behaviour, typed.
        assert_eq!(as_opt_string(&vec![json!("hi")], 0).unwrap(), Some("hi".to_string()));
        assert_eq!(as_opt_string(&vec![json!(null)], 0).unwrap(), None);
        assert_eq!(as_opt_string(&vec![], 0).unwrap(), None);
        assert_eq!(
            as_opt_string(&vec![json!(42)], 0).unwrap_err(),
            "expected string at arg0, got number"
        );
        assert_eq!(as_opt_i64(&vec![json!(-3)], 0).unwrap(), Some(-3));
        assert_eq!(as_opt_bytes(&vec![json!(null)], 0).unwrap(), None);
        assert_eq!(
            as_opt_bytes(&vec![crate::bytes::encode(&[0x80])], 0).unwrap(),
            Some(vec![0x80])
        );
        assert_eq!(as_opt_bool(&vec![json!(true)], 0).unwrap(), Some(true));
        assert_eq!(as_opt_u64(&vec![json!(4)], 0).unwrap(), Some(4));
        assert_eq!(as_opt_f64(&vec![json!(1.5)], 0).unwrap(), Some(1.5));
    }

    // An optional RECORD FIELD: the missing key that is a mismatch for a
    // required field is the empty state for an optional one — and nesting
    // still composes.
    #[test]
    fn optional_record_fields_may_be_absent_or_null() {
        static PORT: Ty = Ty::Uint;
        static TSTR: Ty = Ty::Tstr;
        static LABEL: Ty = Ty::Opt(&TSTR);
        static STATUS: Ty = Ty::Record(&[("port", &PORT), ("label", &LABEL)]);

        // Present, absent and null are all accepted for `label`.
        assert!(as_value_checked(&vec![json!({"port": 1, "label": "a"})], 0, &STATUS).is_ok());
        assert!(as_value_checked(&vec![json!({"port": 1})], 0, &STATUS).is_ok());
        assert!(as_value_checked(&vec![json!({"port": 1, "label": null})], 0, &STATUS).is_ok());
        // A present wrong-typed value still reports the FIELD path.
        assert_eq!(
            as_value_checked(&vec![json!({"port": 1, "label": 7})], 0, &STATUS).unwrap_err(),
            "expected string at arg0.label, got number"
        );
        // The REQUIRED field is unaffected: absent is still a mismatch.
        assert_eq!(
            as_value_checked(&vec![json!({"label": "a"})], 0, &STATUS).unwrap_err(),
            "expected integer at arg0.port, got null"
        );
        // Optionality composes inside containers, and reports the element path.
        static OPT_ARR: Ty = Ty::Arr(&LABEL);
        assert!(as_value_checked(&vec![json!(["a", null])], 0, &OPT_ARR).is_ok());
        assert_eq!(
            as_value_checked(&vec![json!(["a", 7])], 0, &OPT_ARR).unwrap_err(),
            "expected string at arg0[1], got number"
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

    #[test]
    fn a_rejection_object_is_recognised_and_yields_its_message() {
        let v = dispatch_failed("my_module", "expected integer at arg0, got string");
        assert_eq!(
            as_dispatch_rejection(&v),
            Some("expected integer at arg0, got string")
        );
    }

    #[test]
    fn a_plain_value_is_not_a_rejection() {
        for v in [json!(0), json!(""), json!([]), json!(null), json!(false)] {
            assert_eq!(as_dispatch_rejection(&v), None, "{v}");
        }
    }

    // Every code in the closed set is detected, not just `dispatch_failed`.
    //
    // `invalid_args` is the one that was LIVE and undetected: this crate has
    // emitted it since arity checking landed, and until the detector was
    // widened a consumer decoded it as data — `logosctl call m isPositive` with
    // the argument missing exited 0, status "ok", the refusal object as the
    // result.
    #[test]
    fn every_rejection_code_is_detected() {
        for code in REJECTION_CODES {
            let v = json!({"code": code, "message": "m", "origin": "o"});
            assert_eq!(as_dispatch_rejection(&v), Some("m"), "{code}");
        }
    }

    // The real constructor, not a hand-built object: what a provider actually
    // sends for an arity error is recognised.
    #[test]
    fn a_real_invalid_args_object_is_a_rejection() {
        let v = invalid_args("my_module", 4, 2);
        assert_eq!(as_dispatch_rejection(&v), Some("expected 4 arguments, got 2"));
    }

    // The false-match surface. A method returning a map — or an `any` — puts
    // user data exactly where the detector looks, so anything short of the
    // narrow shape has to be refused, or a caller could fake a failed call.
    //
    // These NEGATIVES are what keep the widened match CLOSED. Widening from one
    // literal to a set is one edit away from "any object with a code", and that
    // would hand every method returning a three-string map to the error channel.
    #[test]
    fn a_user_map_never_false_matches() {
        // Right code, wrong arity — 2 keys and 4 keys.
        assert_eq!(
            as_dispatch_rejection(&json!({"code": "dispatch_failed", "message": "m"})),
            None
        );
        assert_eq!(
            as_dispatch_rejection(
                &json!({"code": "dispatch_failed", "message": "m", "origin": "o", "extra": 1})
            ),
            None
        );
        // Right arity and keys, code OUTSIDE the closed set. This is the whole
        // point of a closed set: a method may legitimately answer
        // {code, message, origin} with a code of its own.
        for code in [
            "",
            "ok",
            "not_found",
            "DISPATCH_FAILED",
            "dispatch_failed ",
            "invalid_argument",
            "unknown_methods",
            "user_error",
        ] {
            assert_eq!(
                as_dispatch_rejection(&json!({"code": code, "message": "m", "origin": "o"})),
                None,
                "{code:?}"
            );
        }
        // Right shape and a good code, but a non-string value in each slot.
        for code in REJECTION_CODES {
            assert_eq!(
                as_dispatch_rejection(&json!({"code": code, "message": 7, "origin": "o"})),
                None,
                "{code}"
            );
            assert_eq!(
                as_dispatch_rejection(&json!({"code": code, "message": "m", "origin": null})),
                None,
                "{code}"
            );
            assert_eq!(
                as_dispatch_rejection(&json!({"code": 1, "message": "m", "origin": "o"})),
                None,
                "{code}"
            );
        }
    }
}

