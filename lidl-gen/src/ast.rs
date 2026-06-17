//! LIDL AST — the serde DTO for logos-lidl's JSON wire form.
//!
//! The canonical grammar (parse/serialize/validate) lives in logos-lidl and is
//! reached over its C ABI (see `lidl_ffi`); these structs are just the typed
//! shape of the JSON that crosses that boundary. Field names mirror
//! `lidl::ModuleDecl`; `#[serde(rename)]` maps the camelCase wire keys
//! (`returnType`, `jsonReturn`, ...) onto idiomatic Rust names.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TypeKind {
    Primitive,
    Array,
    Map,
    Optional,
    Named,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeExpr {
    pub kind: TypeKind,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub elements: Vec<TypeExpr>,
}

impl TypeExpr {
    pub fn primitive(name: &str) -> Self {
        TypeExpr { kind: TypeKind::Primitive, name: name.into(), elements: vec![] }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamDecl {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDecl {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: TypeExpr,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MethodDecl {
    pub name: String,
    #[serde(default)]
    pub params: Vec<ParamDecl>,
    #[serde(rename = "returnType")]
    pub return_type: TypeExpr,
    /// Doc comment carried from the author's `///` (or the .lidl
    /// `description "..."` clause). Surfaced as a `///` on the generated client.
    #[serde(default)]
    pub description: String,
    /// Return is LogosMap/LogosList (nlohmann::json) — derived from the type.
    #[serde(rename = "jsonReturn", default)]
    pub json_return: bool,
    /// Return is StdLogosResult — derived from the type.
    #[serde(rename = "resultReturn", default)]
    pub result_return: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventDecl {
    pub name: String,
    #[serde(default)]
    pub params: Vec<ParamDecl>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeDecl {
    pub name: String,
    #[serde(default)]
    pub fields: Vec<FieldDecl>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModuleDecl {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub depends: Vec<String>,
    #[serde(default)]
    pub types: Vec<TypeDecl>,
    #[serde(default)]
    pub methods: Vec<MethodDecl>,
    #[serde(default)]
    pub events: Vec<EventDecl>,
}
