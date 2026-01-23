//! Parameter serialization to JSON format for logos_core.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub value: String,
    #[serde(rename = "type")]
    pub param_type: String,
}

impl Param {
    pub fn new(name: impl Into<String>, value: impl Into<String>, param_type: impl Into<String>) -> Self {
        Param {
            name: name.into(),
            value: value.into(),
            param_type: param_type.into(),
        }
    }

    pub fn string(name: impl Into<String>, value: impl Into<String>) -> Self {
        Param::new(name, value, "string")
    }

    pub fn int(name: impl Into<String>, value: i64) -> Self {
        Param::new(name, value.to_string(), "int")
    }

    pub fn double(name: impl Into<String>, value: f64) -> Self {
        Param::new(name, value.to_string(), "double")
    }

    pub fn bool(name: impl Into<String>, value: bool) -> Self {
        Param::new(name, value.to_string(), "bool")
    }
}

pub trait ToParam {
    fn to_param(&self, name: &str) -> Param;
    fn param_type() -> &'static str;
}

impl ToParam for &str {
    fn to_param(&self, name: &str) -> Param { Param::string(name, *self) }
    fn param_type() -> &'static str { "string" }
}

impl ToParam for String {
    fn to_param(&self, name: &str) -> Param { Param::string(name, self.as_str()) }
    fn param_type() -> &'static str { "string" }
}

impl ToParam for &String {
    fn to_param(&self, name: &str) -> Param { Param::string(name, self.as_str()) }
    fn param_type() -> &'static str { "string" }
}

impl ToParam for i32 {
    fn to_param(&self, name: &str) -> Param { Param::int(name, *self as i64) }
    fn param_type() -> &'static str { "int" }
}

impl ToParam for i64 {
    fn to_param(&self, name: &str) -> Param { Param::int(name, *self) }
    fn param_type() -> &'static str { "int" }
}

impl ToParam for u32 {
    fn to_param(&self, name: &str) -> Param { Param::int(name, *self as i64) }
    fn param_type() -> &'static str { "int" }
}

impl ToParam for u64 {
    fn to_param(&self, name: &str) -> Param { Param::int(name, *self as i64) }
    fn param_type() -> &'static str { "int" }
}

impl ToParam for usize {
    fn to_param(&self, name: &str) -> Param { Param::int(name, *self as i64) }
    fn param_type() -> &'static str { "int" }
}

impl ToParam for f32 {
    fn to_param(&self, name: &str) -> Param { Param::double(name, *self as f64) }
    fn param_type() -> &'static str { "double" }
}

impl ToParam for f64 {
    fn to_param(&self, name: &str) -> Param { Param::double(name, *self) }
    fn param_type() -> &'static str { "double" }
}

impl ToParam for bool {
    fn to_param(&self, name: &str) -> Param { Param::bool(name, *self) }
    fn param_type() -> &'static str { "bool" }
}

pub fn params_to_json<T: ToParam>(params: &[T]) -> Result<String, serde_json::Error> {
    let params: Vec<Param> = params
        .iter()
        .enumerate()
        .map(|(i, p)| p.to_param(&format!("arg{}", i)))
        .collect();
    serde_json::to_string(&params)
}

pub fn params_vec_to_json(params: &[Param]) -> Result<String, serde_json::Error> {
    serde_json::to_string(params)
}

pub fn empty_params_json() -> String {
    "[]".to_string()
}

pub fn infer_string_params(values: &[&str]) -> Vec<Param> {
    values
        .iter()
        .enumerate()
        .map(|(i, v)| Param::string(format!("arg{}", i), *v))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_param() {
        let p = "hello".to_param("arg0");
        assert_eq!(p.name, "arg0");
        assert_eq!(p.value, "hello");
        assert_eq!(p.param_type, "string");
    }

    #[test]
    fn test_int_param() {
        let p = 42i32.to_param("count");
        assert_eq!(p.name, "count");
        assert_eq!(p.value, "42");
        assert_eq!(p.param_type, "int");
    }

    #[test]
    fn test_bool_param() {
        let p = true.to_param("enabled");
        assert_eq!(p.name, "enabled");
        assert_eq!(p.value, "true");
        assert_eq!(p.param_type, "bool");
    }

    #[test]
    fn test_params_to_json() {
        let json = params_to_json(&["hello", "world"]).unwrap();
        assert!(json.contains("\"name\":\"arg0\""));
        assert!(json.contains("\"value\":\"hello\""));
        assert!(json.contains("\"type\":\"string\""));
    }

    #[test]
    fn test_empty_params() {
        let json = empty_params_json();
        assert_eq!(json, "[]");
    }
}
