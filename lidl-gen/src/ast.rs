//! LIDL AST — mirrors logos-lidl's lidl::ModuleDecl.

#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    Primitive,
    Array,
    Map,
    Optional,
    Named,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeExpr {
    pub kind: TypeKind,
    pub name: String,
    pub elements: Vec<TypeExpr>,
}

impl TypeExpr {
    pub fn primitive(name: &str) -> Self {
        TypeExpr { kind: TypeKind::Primitive, name: name.into(), elements: vec![] }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamDecl {
    pub name: String,
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodDecl {
    pub name: String,
    pub params: Vec<ParamDecl>,
    pub return_type: TypeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventDecl {
    pub name: String,
    pub params: Vec<ParamDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub fields: Vec<FieldDecl>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleDecl {
    pub name: String,
    pub version: String,
    pub description: String,
    pub category: String,
    pub depends: Vec<String>,
    pub types: Vec<TypeDecl>,
    pub methods: Vec<MethodDecl>,
    pub events: Vec<EventDecl>,
}
