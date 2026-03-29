/// Neutral value type mirroring `logos::Value` in C++.
///
/// Supports the same type set: Null, Bool, Int, Uint, Float, String, Bytes, Array, Map.
/// Serialized to/from CBOR for wire compatibility.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Map(Vec<(String, Value)>),
}

impl Value {
    // ── Type queries ─────────────────────────────────────────────

    pub fn is_null(&self) -> bool { matches!(self, Value::Null) }
    pub fn is_bool(&self) -> bool { matches!(self, Value::Bool(_)) }
    pub fn is_int(&self) -> bool { matches!(self, Value::Int(_)) }
    pub fn is_uint(&self) -> bool { matches!(self, Value::Uint(_)) }
    pub fn is_float(&self) -> bool { matches!(self, Value::Float(_)) }
    pub fn is_string(&self) -> bool { matches!(self, Value::String(_)) }
    pub fn is_bytes(&self) -> bool { matches!(self, Value::Bytes(_)) }
    pub fn is_array(&self) -> bool { matches!(self, Value::Array(_)) }
    pub fn is_map(&self) -> bool { matches!(self, Value::Map(_)) }

    // ── Accessors ────────────────────────────────────────────────

    pub fn as_bool(&self) -> Option<bool> {
        if let Value::Bool(b) = self { Some(*b) } else { None }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            Value::Uint(v) => Some(*v as i64),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Uint(v) => Some(*v),
            Value::Int(v) if *v >= 0 => Some(*v as u64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(v) => Some(*v),
            Value::Int(v) => Some(*v as f64),
            Value::Uint(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        if let Value::String(s) = self { Some(s) } else { None }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let Value::Bytes(b) = self { Some(b) } else { None }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        if let Value::Array(a) = self { Some(a) } else { None }
    }

    pub fn as_map(&self) -> Option<&[(String, Value)]> {
        if let Value::Map(m) = self { Some(m) } else { None }
    }

    // ── Map access ───────────────────────────────────────────────

    pub fn get(&self, key: &str) -> Option<&Value> {
        if let Value::Map(entries) = self {
            for (k, v) in entries {
                if k == key {
                    return Some(v);
                }
            }
        }
        None
    }
}

impl Default for Value {
    fn default() -> Self {
        Value::Null
    }
}

/// Structured result type mirroring `logos::Result` in C++.
#[derive(Debug, Clone)]
pub struct LogosResult {
    pub success: bool,
    pub value: Value,
    pub error: Value,
}

impl LogosResult {
    pub fn ok(value: Value) -> Self {
        LogosResult {
            success: true,
            value,
            error: Value::Null,
        }
    }

    pub fn fail(message: &str) -> Self {
        LogosResult {
            success: false,
            value: Value::Null,
            error: Value::String(message.to_string()),
        }
    }

    pub fn to_value(&self) -> Value {
        Value::Map(vec![
            ("success".to_string(), Value::Bool(self.success)),
            ("value".to_string(), self.value.clone()),
            ("error".to_string(), self.error.clone()),
        ])
    }

    pub fn from_value(v: &Value) -> Self {
        let success = v.get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let value = v.get("value").cloned().unwrap_or(Value::Null);
        let error = v.get("error").cloned().unwrap_or(Value::Null);
        LogosResult { success, value, error }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_types() {
        assert!(Value::Null.is_null());
        assert!(Value::Bool(true).is_bool());
        assert!(Value::Int(42).is_int());
        assert!(Value::Uint(42).is_uint());
        assert!(Value::Float(3.14).is_float());
        assert!(Value::String("hello".into()).is_string());
        assert!(Value::Bytes(vec![1, 2, 3]).is_bytes());
        assert!(Value::Array(vec![]).is_array());
        assert!(Value::Map(vec![]).is_map());
    }

    #[test]
    fn test_accessors() {
        assert_eq!(Value::Bool(true).as_bool(), Some(true));
        assert_eq!(Value::Int(-5).as_i64(), Some(-5));
        assert_eq!(Value::Uint(100).as_u64(), Some(100));
        assert_eq!(Value::Float(2.5).as_f64(), Some(2.5));
        assert_eq!(Value::String("hi".into()).as_string(), Some("hi"));
    }

    #[test]
    fn test_map_access() {
        let m = Value::Map(vec![
            ("key".into(), Value::Int(42)),
            ("other".into(), Value::String("val".into())),
        ]);
        assert_eq!(m.get("key").and_then(|v| v.as_i64()), Some(42));
        assert!(m.get("missing").is_none());
    }

    #[test]
    fn test_logos_result_round_trip() {
        let r = LogosResult::ok(Value::String("data".into()));
        let v = r.to_value();
        let restored = LogosResult::from_value(&v);
        assert!(restored.success);
        assert_eq!(restored.value.as_string(), Some("data"));
    }

    #[test]
    fn test_logos_result_fail() {
        let r = LogosResult::fail("something broke");
        let v = r.to_value();
        let restored = LogosResult::from_value(&v);
        assert!(!restored.success);
        assert_eq!(restored.error.as_string(), Some("something broke"));
    }
}
