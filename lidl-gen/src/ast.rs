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

    /// Whether this type expression is itself an optional (`?T`).
    ///
    /// For a record FIELD this is only half the question — see
    /// [`FieldDecl::is_optional`].
    pub fn is_optional(&self) -> bool {
        self.kind == TypeKind::Optional
    }

    /// The value carried by an optional: `?T` -> `T`, `T` -> `T`.
    ///
    /// Strips *every* leading Optional layer, because `?T` is TWO-state (a
    /// value of T, or empty) and optionality is therefore idempotent: `??T`
    /// denotes the same two states as `?T` and must never become a third one.
    /// A degenerate Optional carrying no element (only reachable by
    /// hand-building an AST) is returned as-is rather than dereferenced.
    pub fn value_type(&self) -> &TypeExpr {
        let mut t = self;
        while t.kind == TypeKind::Optional && !t.elements.is_empty() {
            t = &t.elements[0];
        }
        t
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
    /// The type exactly as written: `? name: T` leaves this `T` and sets
    /// `optional`; `name: ?T` leaves `optional` false and makes this an
    /// Optional wrapping T. Verbatim spelling — ask [`FieldDecl::is_optional`]
    /// / [`FieldDecl::value_type`] for the meaning.
    #[serde(rename = "type")]
    pub ty: TypeExpr,
    /// The leading-`?`-before-the-name spelling, verbatim. NOT the answer to
    /// "is this field optional" — reading it alone is a bug.
    #[serde(default)]
    pub optional: bool,
    /// The FRONTEND's own answer to "may this field be empty", carried across
    /// the C ABI as the derived, output-only `isOptional` key. `None` when the
    /// AST didn't come from a frontend that emits it (one built in Rust by
    /// `rust_frontend`, or an older logos-lidl). Never read this directly;
    /// call [`FieldDecl::is_optional`].
    #[serde(rename = "isOptional", default, skip_serializing)]
    pub frontend_is_optional: Option<bool>,
}

// --- Optionality ------------------------------------------------------------
//
// `?T` is TWO-state: a value of T, or empty. Never three-state — Rust has
// exactly one empty inhabitant (`None`), so `?T` maps onto `Option<T>` and
// there is nowhere for a third state to live.
//
// A record field has TWO equivalent spellings — the flag (`? name: T`) and the
// type kind (`name: ?T`) — and they MUST produce byte-identical generated code.
// The frontend reconciles them once (lidl::fieldIsOptional) and ships the
// answer over the C ABI as the derived `isOptional` key; these accessors read
// it, falling back to the same reconciliation for an AST that carries no
// derived keys (built in Rust, or parsed by a logos-lidl predating them).
// Backends call these and never look at `optional` / `kind` themselves, so the
// two spellings cannot drift apart in the emitted code.

impl FieldDecl {
    /// Whether this field may be empty. True for `? name: T` and `name: ?T`
    /// alike.
    pub fn is_optional(&self) -> bool {
        self.frontend_is_optional
            .unwrap_or(self.optional || self.ty.is_optional())
    }

    /// The type of the field's VALUE, with optionality stripped: `T` for
    /// `? name: T`, for `name: ?T`, and for the redundant `? name: ?T`.
    pub fn value_type(&self) -> &TypeExpr {
        self.ty.value_type()
    }
}

impl ParamDecl {
    /// A positional slot has no name to put a flag in front of, so a parameter
    /// carries only the type-kind spelling.
    pub fn is_optional(&self) -> bool {
        self.ty.is_optional()
    }

    pub fn value_type(&self) -> &TypeExpr {
        self.ty.value_type()
    }
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

impl MethodDecl {
    /// Whether the return may be empty (`-> ?T`). A return is a positional
    /// slot, so it too has only the type-kind spelling.
    pub fn return_is_optional(&self) -> bool {
        self.return_type.is_optional()
    }

    pub fn return_value_type(&self) -> &TypeExpr {
        self.return_type.value_type()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn field(json: &str) -> FieldDecl {
        serde_json::from_str(json).expect("decode field")
    }

    // The frontend answers "is this optional" once and ships the answer as the
    // derived `isOptional` key; that answer wins. Deriving it here instead is
    // what lets two backends — or one backend and the frontend — disagree.
    #[test]
    fn the_frontends_answer_is_the_answer() {
        // A field whose verbatim spelling says nothing about optionality, but
        // which the frontend reports as optional, IS optional.
        let f = field(
            r#"{"name":"label","type":{"kind":"primitive","name":"tstr"},"optional":false,"isOptional":true}"#,
        );
        assert!(f.is_optional());
        // ...and the converse: the derived key is not second-guessed either.
        let f = field(
            r#"{"name":"label","type":{"kind":"primitive","name":"tstr"},"optional":true,"isOptional":false}"#,
        );
        assert!(!f.is_optional());
    }

    // An AST with no derived keys — built in Rust by `rust_frontend`, or parsed
    // by a logos-lidl predating them — still has to reconcile BOTH spellings.
    #[test]
    fn both_spellings_reconcile_without_the_derived_key() {
        let flag = field(r#"{"name":"label","type":{"kind":"primitive","name":"tstr"},"optional":true}"#);
        let kind = field(
            r#"{"name":"label","type":{"kind":"optional","elements":[{"kind":"primitive","name":"tstr"}]}}"#,
        );
        assert!(flag.is_optional());
        assert!(kind.is_optional());
        // Same value type either way — that is what makes the two spellings
        // generate identical code.
        assert_eq!(flag.value_type(), kind.value_type());
        assert_eq!(flag.value_type().name, "tstr");

        let plain = field(r#"{"name":"id","type":{"kind":"primitive","name":"tstr"}}"#);
        assert!(!plain.is_optional());
        assert_eq!(plain.value_type().name, "tstr");
    }

    // `?T` is two-state, so optionality is idempotent: `??T` denotes the same
    // two states and must collapse rather than become a third.
    #[test]
    fn optionality_is_idempotent() {
        let inner = TypeExpr::primitive("int");
        let once = TypeExpr {
            kind: TypeKind::Optional,
            name: String::new(),
            elements: vec![inner.clone()],
        };
        let twice = TypeExpr {
            kind: TypeKind::Optional,
            name: String::new(),
            elements: vec![once.clone()],
        };
        assert_eq!(once.value_type(), &inner);
        assert_eq!(twice.value_type(), &inner);
        // A degenerate optional carrying nothing is returned as-is rather than
        // dereferenced into a panic.
        let empty = TypeExpr { kind: TypeKind::Optional, name: String::new(), elements: vec![] };
        assert_eq!(empty.value_type(), &empty);
    }

    // The derived key is the frontend's OUTPUT: it must not be fed back as if
    // it were part of the document, or a round trip would start asserting it.
    #[test]
    fn the_derived_key_is_not_serialized_back() {
        let f = field(
            r#"{"name":"label","type":{"kind":"primitive","name":"tstr"},"optional":true,"isOptional":true}"#,
        );
        let json = serde_json::to_string(&f).expect("encode field");
        assert!(!json.contains("isOptional"), "{}", json);
        assert!(json.contains("\"optional\":true"), "{}", json);
    }
}
