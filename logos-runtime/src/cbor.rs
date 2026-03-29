/// CBOR codec for [`Value`] using ciborium.
///
/// Encodes/decodes Value to/from CBOR bytes, wire-compatible with the
/// C++ `logos::Value` CBOR codec.

use crate::value::Value;
use ciborium::value::Value as CborValue;

/// Encode a Value to CBOR bytes.
pub fn encode(value: &Value) -> Vec<u8> {
    let cbor_val = to_cbor_value(value);
    let mut buf = Vec::new();
    ciborium::into_writer(&cbor_val, &mut buf)
        .expect("CBOR encoding should not fail for valid Value");
    buf
}

/// Decode a Value from CBOR bytes.
pub fn decode(data: &[u8]) -> Result<Value, String> {
    let cbor_val: CborValue = ciborium::from_reader(data)
        .map_err(|e| format!("CBOR decode error: {}", e))?;
    Ok(from_cbor_value(&cbor_val))
}

fn to_cbor_value(v: &Value) -> CborValue {
    match v {
        Value::Null => CborValue::Null,
        Value::Bool(b) => CborValue::Bool(*b),
        Value::Int(i) => CborValue::Integer((*i).into()),
        Value::Uint(u) => CborValue::Integer((*u).into()),
        Value::Float(f) => CborValue::Float(*f),
        Value::String(s) => CborValue::Text(s.clone()),
        Value::Bytes(b) => CborValue::Bytes(b.clone()),
        Value::Array(arr) => {
            CborValue::Array(arr.iter().map(to_cbor_value).collect())
        }
        Value::Map(entries) => {
            CborValue::Map(entries.iter().map(|(k, v)| {
                (CborValue::Text(k.clone()), to_cbor_value(v))
            }).collect())
        }
    }
}

fn from_cbor_value(cv: &CborValue) -> Value {
    match cv {
        CborValue::Null => Value::Null,
        CborValue::Bool(b) => Value::Bool(*b),
        CborValue::Integer(i) => {
            // ciborium::Integer can be i128
            let n: i128 = (*i).into();
            if n >= 0 && n <= i64::MAX as i128 {
                Value::Int(n as i64)
            } else if n < 0 && n >= i64::MIN as i128 {
                Value::Int(n as i64)
            } else {
                Value::Uint(n as u64)
            }
        }
        CborValue::Float(f) => Value::Float(*f),
        CborValue::Text(s) => Value::String(s.clone()),
        CborValue::Bytes(b) => Value::Bytes(b.clone()),
        CborValue::Array(arr) => {
            Value::Array(arr.iter().map(from_cbor_value).collect())
        }
        CborValue::Map(entries) => {
            Value::Map(entries.iter().filter_map(|(k, v)| {
                if let CborValue::Text(key) = k {
                    Some((key.clone(), from_cbor_value(v)))
                } else {
                    None // skip non-string keys
                }
            }).collect())
        }
        CborValue::Tag(_, inner) => from_cbor_value(inner),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_null_round_trip() {
        let v = Value::Null;
        let bytes = encode(&v);
        let decoded = decode(&bytes).unwrap();
        assert!(decoded.is_null());
    }

    #[test]
    fn test_bool_round_trip() {
        let v = Value::Bool(true);
        let decoded = decode(&encode(&v)).unwrap();
        assert_eq!(decoded.as_bool(), Some(true));
    }

    #[test]
    fn test_int_round_trip() {
        let v = Value::Int(-42);
        let decoded = decode(&encode(&v)).unwrap();
        assert_eq!(decoded.as_i64(), Some(-42));
    }

    #[test]
    fn test_uint_round_trip() {
        // Large positive int round-trips via Int since it fits in i64
        let v = Value::Int(1000);
        let decoded = decode(&encode(&v)).unwrap();
        assert_eq!(decoded.as_i64(), Some(1000));
    }

    #[test]
    fn test_float_round_trip() {
        let v = Value::Float(3.14159);
        let decoded = decode(&encode(&v)).unwrap();
        assert!((decoded.as_f64().unwrap() - 3.14159).abs() < 1e-10);
    }

    #[test]
    fn test_string_round_trip() {
        let v = Value::String("hello world".into());
        let decoded = decode(&encode(&v)).unwrap();
        assert_eq!(decoded.as_string(), Some("hello world"));
    }

    #[test]
    fn test_bytes_round_trip() {
        let v = Value::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let decoded = decode(&encode(&v)).unwrap();
        assert_eq!(decoded.as_bytes(), Some(&[0xDE, 0xAD, 0xBE, 0xEF][..]));
    }

    #[test]
    fn test_array_round_trip() {
        let v = Value::Array(vec![
            Value::Int(1),
            Value::String("two".into()),
            Value::Bool(true),
        ]);
        let decoded = decode(&encode(&v)).unwrap();
        let arr = decoded.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_i64(), Some(1));
        assert_eq!(arr[1].as_string(), Some("two"));
    }

    #[test]
    fn test_map_round_trip() {
        let v = Value::Map(vec![
            ("name".into(), Value::String("Alice".into())),
            ("age".into(), Value::Int(30)),
        ]);
        let decoded = decode(&encode(&v)).unwrap();
        assert_eq!(decoded.get("name").and_then(|v| v.as_string()), Some("Alice"));
        assert_eq!(decoded.get("age").and_then(|v| v.as_i64()), Some(30));
    }

    #[test]
    fn test_nested_round_trip() {
        let v = Value::Map(vec![
            ("list".into(), Value::Array(vec![Value::Int(1), Value::Int(2)])),
            ("flag".into(), Value::Bool(true)),
        ]);
        let decoded = decode(&encode(&v)).unwrap();
        let list = decoded.get("list").unwrap().as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(decoded.get("flag").and_then(|v| v.as_bool()), Some(true));
    }
}
