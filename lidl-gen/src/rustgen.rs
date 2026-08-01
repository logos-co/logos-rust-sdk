//! Rust code generation: a typed client struct per LIDL module.

use crate::ast::*;
use std::collections::BTreeSet;

fn pascal(name: &str) -> String {
    name.split('_')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut c = s.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// The set of record names the module actually DECLARES.
///
/// A `Named` type is not automatically a record: the LIDL front end has no
/// `void` builtin, so `-> void` parses as `Named("void")` — and pascal-casing
/// every Named produced `-> Void`, a type that does not exist. Only a name in
/// `module.types` gets the struct; anything else keeps the untyped fallback it
/// had before records existed.
pub(crate) fn record_names(module: &ModuleDecl) -> BTreeSet<String> {
    module.types.iter().map(|t| t.name.clone()).collect()
}

/// Whether `ty` names a declared record.
pub(crate) fn is_record(ty: &TypeExpr, recs: &BTreeSet<String>) -> bool {
    ty.kind == TypeKind::Named && recs.contains(&ty.name)
}

/// Whether `ty` is a usable `?T` — an optional that actually carries a value
/// type. A degenerate Optional with no element (only reachable by hand-building
/// an AST) keeps the untyped fallback rather than recursing forever.
fn is_optional(ty: &TypeExpr) -> bool {
    ty.is_optional() && !ty.elements.is_empty()
}

/// Rust parameter type for a LIDL type.
/// The owned Rust type for a LIDL type. Records become their generated struct;
/// composites recurse, so `[Status]` is `Vec<Status>` and `{tstr: bstr}` is
/// `BTreeMap<String, Vec<u8>>`. `?T` is `Option<T>` — Rust's single empty
/// inhabitant, which is why `?T` is two-state and never three.
pub(crate) fn owned_type(ty: &TypeExpr, recs: &BTreeSet<String>) -> String {
    if is_optional(ty) {
        return format!("Option<{}>", owned_type(ty.value_type(), recs));
    }
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "tstr") => "String".into(),
        (TypeKind::Primitive, "int") => "i64".into(),
        (TypeKind::Primitive, "uint") => "u64".into(),
        (TypeKind::Primitive, "float64") => "f64".into(),
        (TypeKind::Primitive, "bool") => "bool".into(),
        (TypeKind::Primitive, "bstr") => "Vec<u8>".into(),
        (TypeKind::Named, n) if recs.contains(n) => pascal(n),
        (TypeKind::Array, _) if ty.elements.len() == 1 => {
            format!("Vec<{}>", owned_type(&ty.elements[0], recs))
        }
        (TypeKind::Map, _) if ty.elements.len() == 2 => format!(
            "std::collections::BTreeMap<String, {}>",
            owned_type(&ty.elements[1], recs)
        ),
        _ => "serde_json::Value".into(),
    }
}

/// `expr` (a place expression of the owned type) -> serde_json::Value.
///
/// `bstr` goes through the tagged-bytes codec at EVERY depth — a plain serde
/// encode would emit a number array that no other language decodes as bytes,
/// which is the bug class of logos-protocol #21/#23.
fn enc_expr(ty: &TypeExpr, expr: &str, recs: &BTreeSet<String>) -> String {
    // A `?T` reached HERE is a positional slot (an element of a container, a
    // return, an event param): there is no key to leave out, so empty is spelled
    // `null`. The one NAMED slot — a record field — is encoded by
    // `emit_records`, which omits the key instead.
    if is_optional(ty) {
        return format!(
            "match &{} {{ Some(__o) => {}, None => serde_json::Value::Null }}",
            expr,
            enc_expr(ty.value_type(), "__o", recs)
        );
    }
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "bstr") => {
            format!("logos_rust_sdk::bytes::encode(&{}[..])", expr)
        }
        (TypeKind::Named, n) if recs.contains(n) => format!("{}.to_json()", expr),
        (TypeKind::Array, _) if ty.elements.len() == 1 => format!(
            "serde_json::Value::Array({}.iter().map(|__e| {}).collect())",
            expr,
            enc_expr(&ty.elements[0], "__e", recs)
        ),
        (TypeKind::Map, _) if ty.elements.len() == 2 => format!(
            "serde_json::Value::Object({}.iter().map(|(__k, __v)| (__k.clone(), {})).collect())",
            expr,
            enc_expr(&ty.elements[1], "__v", recs)
        ),
        _ => format!("serde_json::json!({})", expr),
    }
}

/// `expr` (a `&serde_json::Value`) -> the owned type, as an expression using `?`
/// inside a function returning Option. Mirrors logos_codec.h's acceptance:
/// bytes take the lenient set, `any` passes through verbatim.
fn dec_expr(ty: &TypeExpr, expr: &str, recs: &BTreeSet<String>) -> String {
    // `?T`: null is the empty state. A PRESENT value still decodes as `T`, and
    // still fails the whole decode through `?` if it doesn't match — optional
    // adds one inhabitant, it doesn't stop type checking. (`expr` appears twice,
    // so callers pass a simple place expression here.)
    if is_optional(ty) {
        return format!(
            "if {}.is_null() {{ None }} else {{ Some({}) }}",
            expr,
            dec_expr(ty.value_type(), expr, recs)
        );
    }
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "tstr") => format!("{}.as_str()?.to_string()", expr),
        (TypeKind::Primitive, "int") => format!("{}.as_i64()?", expr),
        (TypeKind::Primitive, "uint") => format!("{}.as_u64()?", expr),
        (TypeKind::Primitive, "float64") => format!("{}.as_f64()?", expr),
        (TypeKind::Primitive, "bool") => format!("{}.as_bool()?", expr),
        (TypeKind::Primitive, "bstr") => {
            format!("logos_rust_sdk::bytes::decode_lenient({})?", expr)
        }
        (TypeKind::Named, n) if recs.contains(n) => format!("{}::from_json({})?", pascal(n), expr),
        (TypeKind::Array, _) if ty.elements.len() == 1 => format!(
            "{}.as_array()?.iter().map(|__e| Some({})).collect::<Option<Vec<_>>>()?",
            expr,
            dec_expr(&ty.elements[0], "__e", recs)
        ),
        (TypeKind::Map, _) if ty.elements.len() == 2 => format!(
            "{}.as_object()?.iter().map(|(__k, __v)| Some((__k.clone(), {}))).collect::<Option<std::collections::BTreeMap<_, _>>>()?",
            expr,
            dec_expr(&ty.elements[1], "__v", recs)
        ),
        _ => format!("{}.clone()", expr),
    }
}

/// The owned Rust type of a record FIELD. Both optional spellings land here:
/// `? name: T` carries its optionality in the field flag and `name: ?T` in the
/// type, and `FieldDecl::is_optional` reconciles them, so the two emit the
/// identical `Option<T>`.
fn field_type(f: &FieldDecl, recs: &BTreeSet<String>) -> String {
    if f.is_optional() {
        format!("Option<{}>", owned_type(f.value_type(), recs))
    } else {
        owned_type(&f.ty, recs)
    }
}

/// One Rust struct per `type` decl in the contract, with hand-emitted
/// to_json/from_json rather than a serde derive — a `bstr` field has to ride the
/// canonical {"_bytes": base64url} form, which derive(Serialize) would not do.
pub(crate) fn emit_records(module: &ModuleDecl) -> String {
    let recs = record_names(module);
    let mut out = String::new();
    for t in &module.types {
        let name = pascal(&t.name);
        out.push_str(&format!(
            "/// `{}` — a record declared by the `{}` contract.\n#[derive(Debug, Clone, PartialEq)]\npub struct {} {{\n",
            t.name, module.name, name
        ));
        for f in &t.fields {
            // `? name: T` and `name: ?T` are the SAME field. Only the second
            // spelling puts the optionality in the TYPE, so the field type must
            // come from the reconciled pair (is_optional + value_type) — asking
            // owned_type about `f.ty` alone would type the flag spelling as a
            // plain `T` while every encoder/decoder below treats it as empty-able.
            out.push_str(&format!(
                "    pub {}: {},\n",
                snake(&f.name),
                field_type(f, &recs)
            ));
        }
        out.push_str("}\n\n");

        out.push_str(&format!("impl {} {{\n", name));
        out.push_str("    /// The record as its canonical JSON object.\n");
        if t.fields.iter().any(|f| f.is_optional()) {
            out.push_str(
                "    ///\n\
                 \x20   /// An empty optional field is spelled by OMITTING its key: a record\n\
                 \x20   /// field is a NAMED slot, so it can simply be left out. Encoding is\n\
                 \x20   /// canonical (one spelling per state), which makes a round trip\n\
                 \x20   /// canonicalising rather than byte-identical — `{\"f\": null}` decodes\n\
                 \x20   /// to the same empty state and re-encodes as `{}`.\n",
            );
        }
        out.push_str("    pub fn to_json(&self) -> serde_json::Value {\n");
        out.push_str("        let mut o = serde_json::Map::new();\n");
        for f in &t.fields {
            let field = snake(&f.name);
            if f.is_optional() {
                out.push_str(&format!(
                    "        if let Some(__o) = &self.{} {{ o.insert(\"{}\".to_string(), {}); }}\n",
                    field,
                    f.name,
                    enc_expr(f.value_type(), "__o", &recs)
                ));
            } else {
                out.push_str(&format!(
                    "        o.insert(\"{}\".to_string(), {});\n",
                    f.name,
                    enc_expr(&f.ty, &format!("self.{}", field), &recs)
                ));
            }
        }
        out.push_str("        serde_json::Value::Object(o)\n    }\n\n");

        out.push_str("    /// Decode the canonical JSON object. None if a field is missing or\n");
        out.push_str("    /// has the wrong shape — the provider reports the same mismatch with\n");
        out.push_str("    /// a field path, so this is the consumer-side half of that contract.\n");
        if t.fields.iter().any(|f| f.is_optional()) {
            out.push_str(
                "    ///\n\
                 \x20   /// An OPTIONAL field is exempt: an absent key and an explicit null are\n\
                 \x20   /// the same empty state. A present-but-wrong-typed value still fails,\n\
                 \x20   /// there as everywhere.\n",
            );
        }
        out.push_str("    pub fn from_json(v: &serde_json::Value) -> Option<Self> {\n");
        out.push_str("        let o = v.as_object()?;\n");
        out.push_str("        Some(Self {\n");
        for f in &t.fields {
            let value = if f.is_optional() {
                // Both halves of the liberal decode in one expression: `None`
                // (absent key) and `Some(Null)` (explicit null) are one state.
                // The `?` inside the decode still short-circuits the whole
                // from_json when a PRESENT value has the wrong type.
                format!(
                    "match o.get(\"{}\") {{ None | Some(serde_json::Value::Null) => None, Some(__v) => Some({}) }}",
                    f.name,
                    dec_expr(f.value_type(), "__v", &recs)
                )
            } else {
                dec_expr(&f.ty, &format!("o.get(\"{}\")?", f.name), &recs)
            };
            out.push_str(&format!("            {}: {},\n", snake(&f.name), value));
        }
        out.push_str("        })\n    }\n}\n\n");
    }
    out
}

fn param_type(ty: &TypeExpr, recs: &BTreeSet<String>) -> String {
    // `?T` borrows exactly as `T` does, wrapped in Rust's one empty inhabitant:
    // `?tstr` is `Option<&str>`, `?Status` is `Option<&Status>`.
    if is_optional(ty) {
        return format!("Option<{}>", param_type(ty.value_type(), recs));
    }
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "tstr") => "&str".into(),
        (TypeKind::Primitive, "int") => "i64".into(),
        (TypeKind::Primitive, "uint") => "u64".into(),
        (TypeKind::Primitive, "float64") => "f64".into(),
        (TypeKind::Primitive, "bool") => "bool".into(),
        (TypeKind::Primitive, "bstr") => "&[u8]".into(),
        // Records are real types. Additive: nothing generated records before, so
        // no existing consumer signature changes. Composites other than
        // [record] deliberately stay &serde_json::Value — retyping them WOULD
        // change every existing call site.
        (TypeKind::Named, n) if recs.contains(n) => format!("&{}", pascal(n)),
        (TypeKind::Array, _)
            if ty.elements.len() == 1 && is_record(&ty.elements[0], recs) =>
        {
            format!("&[{}]", pascal(&ty.elements[0].name))
        }
        _ => "&serde_json::Value".into(),
    }
}

/// Expression converting a parameter into its JSON wire value.
fn param_to_json(name: &str, ty: &TypeExpr, recs: &BTreeSet<String>) -> String {
    // An empty `?T` argument is spelled `null`, never a missing element: a
    // parameter is a POSITIONAL slot, and arity must never change. (The
    // provider's decode is liberal and accepts a short argument list too, but
    // this side always sends the full arity.)
    if is_optional(ty) {
        return format!(
            "match {} {{ Some(__o) => {}, None => serde_json::Value::Null }}",
            name,
            param_to_json("__o", ty.value_type(), recs)
        );
    }
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "bstr") => format!("logos_rust_sdk::bytes::encode({})", name),
        (TypeKind::Named, n) if recs.contains(n) => format!("{}.to_json()", name),
        (TypeKind::Array, _)
            if ty.elements.len() == 1 && is_record(&ty.elements[0], recs) =>
        {
            format!(
                "serde_json::Value::Array({}.iter().map(|__e| __e.to_json()).collect())",
                name
            )
        }
        (TypeKind::Primitive, "tstr") => format!("serde_json::Value::from({})", name),
        (TypeKind::Primitive, "int") | (TypeKind::Primitive, "uint")
        | (TypeKind::Primitive, "float64") | (TypeKind::Primitive, "bool") => {
            format!("serde_json::Value::from({})", name)
        }
        _ => format!("{}.clone()", name),
    }
}

/// (return type, conversion-from-json expression over `value`)
fn return_conv(ty: &TypeExpr, recs: &BTreeSet<String>) -> (String, String) {
    // `-> ?T` is `Option<T>`: null is the empty state (a return is positional,
    // so that is how the provider spells empty). A present value still has to
    // decode as `T` and still fails the call if it doesn't.
    if is_optional(ty) {
        let (inner_ty, inner_conv) = return_conv(ty.value_type(), recs);
        return (
            format!("Option<{}>", inner_ty),
            format!(
                "if value.is_null() {{ Ok(None) }} else {{ ({}).map(Some) }}",
                inner_conv
            ),
        );
    }
    match (&ty.kind, ty.name.as_str()) {
        // A void method has nothing to hand back. It used to fall to the
        // catch-all and return the raw serde_json::Value, which made the caller
        // inspect a value the contract says does not exist — and made every
        // consumer decide for itself whether the provider's answer meant
        // success. The call still fails through `?` if the RPC failed.
        // A BLOCK, not a bare statement: this conversion is also spliced into an
        // expression position by the async wrapper, where a `let` is a syntax error.
        (TypeKind::Named, "void") => ("()".into(), "{ let _ = value; Ok(()) }".into()),
        (TypeKind::Primitive, "tstr") => (
            "String".into(),
            "Ok(value.as_str().unwrap_or_default().to_string())".into(),
        ),
        (TypeKind::Primitive, "int") => (
            "i64".into(),
            "value.as_i64().ok_or_else(|| logos_rust_sdk::LogosError::JsonError(format!(\"expected int, got {}\", value)))".into(),
        ),
        (TypeKind::Primitive, "uint") => (
            "u64".into(),
            "value.as_u64().ok_or_else(|| logos_rust_sdk::LogosError::JsonError(format!(\"expected uint, got {}\", value)))".into(),
        ),
        (TypeKind::Primitive, "float64") => (
            "f64".into(),
            "value.as_f64().ok_or_else(|| logos_rust_sdk::LogosError::JsonError(format!(\"expected float64, got {}\", value)))".into(),
        ),
        (TypeKind::Primitive, "bool") => (
            "bool".into(),
            "value.as_bool().ok_or_else(|| logos_rust_sdk::LogosError::JsonError(format!(\"expected bool, got {}\", value)))".into(),
        ),
        (TypeKind::Primitive, "bstr") => (
            "Vec<u8>".into(),
            "logos_rust_sdk::bytes::decode(&value).ok_or_else(|| logos_rust_sdk::LogosError::JsonError(\"expected {\\\"_bytes\\\":...} payload\".to_string()))".into(),
        ),
        (TypeKind::Named, n) if recs.contains(n) => (
            pascal(n),
            format!(
                "{}::from_json(&value).ok_or_else(|| logos_rust_sdk::LogosError::JsonError(\"expected a {} object\".to_string()))",
                pascal(n),
                n
            ),
        ),
        (TypeKind::Array, _)
            if ty.elements.len() == 1 && is_record(&ty.elements[0], recs) =>
        {
            let rec = pascal(&ty.elements[0].name);
            (
                format!("Vec<{}>", rec),
                format!(
                    "value.as_array().and_then(|__a| __a.iter().map({}::from_json).collect::<Option<Vec<_>>>()).ok_or_else(|| logos_rust_sdk::LogosError::JsonError(\"expected an array of {} objects\".to_string()))",
                    rec, ty.elements[0].name
                ),
            )
        }
        _ => ("serde_json::Value".into(), "Ok(value)".into()),
    }
}

/// How many leading positional slots a peer MUST supply: everything up to and
/// including the last required one. A trailing `?T` may arrive as null or not
/// at all — absent and null are the same empty state on decode — so it does not
/// count toward the minimum. (The encoding side always sends the full arity.)
pub(crate) fn required_arity(params: &[ParamDecl]) -> usize {
    params.iter().rposition(|p| !p.is_optional()).map_or(0, |i| i + 1)
}

/// The typed Rust field for one event parameter. Scalars are owned; anything
/// else keeps the untyped Value, and `?T` wraps whichever it is in `Option`.
fn event_param_type(ty: &TypeExpr) -> String {
    if is_optional(ty) {
        return format!("Option<{}>", event_param_type(ty.value_type()));
    }
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "tstr") => "String".into(),
        (TypeKind::Primitive, "int") => "i64".into(),
        (TypeKind::Primitive, "uint") => "u64".into(),
        (TypeKind::Primitive, "float64") => "f64".into(),
        (TypeKind::Primitive, "bool") => "bool".into(),
        (TypeKind::Primitive, "bstr") => "Vec<u8>".into(),
        _ => "serde_json::Value".into(),
    }
}

/// Decode one event parameter out of the payload element `expr`, as an
/// expression using `?` inside a function returning Option.
fn event_param_decode(ty: &TypeExpr, expr: &str) -> String {
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "tstr") => format!("{}.as_str()?.to_string()", expr),
        (TypeKind::Primitive, "int") => format!("{}.as_i64()?", expr),
        (TypeKind::Primitive, "uint") => format!("{}.as_u64()?", expr),
        (TypeKind::Primitive, "float64") => format!("{}.as_f64()?", expr),
        (TypeKind::Primitive, "bool") => format!("{}.as_bool()?", expr),
        (TypeKind::Primitive, "bstr") => {
            format!("logos_rust_sdk::bytes::decode(&{})?", expr)
        }
        _ => format!("{}.clone()", expr),
    }
}

/// Generate the typed Rust client for a LIDL module.
pub fn generate(module: &ModuleDecl) -> String {
    let struct_name = format!("{}Client", pascal(&module.name));
    let recs = record_names(module);
    let mut out = String::new();
    out.push_str(&format!(
        "// Generated by logos-lidl-gen from the `{}` LIDL contract — do not edit.\n\
         //\n\
         // Typed caller + event subscribers over logos_rust_sdk's lp_* consumer.\n\n\
         use logos_rust_sdk::{{EventData, EventSubscription, LogosError, LogosModuleSDK, PluginProxy}};\n\n",
        module.name
    ));

    out.push_str(&emit_records(module));

    out.push_str(&format!("pub struct {} {{\n    proxy: PluginProxy,\n}}\n\n", struct_name));
    out.push_str(&format!("impl {} {{\n", struct_name));
    out.push_str(&format!(
        "    /// Client bound to the contract's own module name (`{}`) —\n\
         \x20   /// the concrete-dependency pattern.\n\
         \x20   pub fn new() -> Self {{\n        Self {{ proxy: LogosModuleSDK::new().plugin(\"{}\") }}\n    }}\n\n\
         \x20   /// Bind the same typed surface to ANY provider chosen at runtime —\n\
         \x20   /// the interface-dependency pattern: the contract names the shape,\n\
         \x20   /// the caller names the module.\n\
         \x20   pub fn bind(module_name: &str) -> Self {{\n        Self {{ proxy: LogosModuleSDK::new().plugin(module_name) }}\n    }}\n",
        module.name, module.name
    ));

    // Per-call timeout, exposed ONCE per client rather than as an extra
    // parameter on every method. The lp_* ABI has carried `timeout_ms` all
    // along; adding it to the generated signatures would have broken every
    // existing `client.method(a, b)` call site (Rust has no default
    // arguments), and would have needed a second entry point for the async
    // twin as well. A timeout-scoped VIEW covers both surfaces and every
    // method at once, and leaves `self` — and therefore every existing call
    // site — on the protocol default.
    out.push_str(
        "\n    /// The same typed surface with a per-call timeout: EVERY method on\n\
         \x20   /// the returned client — sync and async alike — gives up after\n\
         \x20   /// `timeout` instead of waiting for the protocol default (20s).\n\
         \x20   ///\n\
         \x20   /// `self` is unchanged and keeps the default, so scoping a timeout\n\
         \x20   /// here can never leak into a call that did not ask for one:\n\
         \x20   ///\n\
         \x20   /// ```ignore\n\
         \x20   /// let quick = client.with_timeout(std::time::Duration::from_millis(500))?;\n\
         \x20   /// quick.slow_thing()?;      // fails after ~500ms\n\
         \x20   /// client.slow_thing()?;     // still the 20s default\n\
         \x20   /// ```\n\
         \x20   ///\n\
         \x20   /// Fails with `LogosError::InvalidTimeout` if the duration cannot be\n\
         \x20   /// expressed on the protocol ABI (sub-millisecond, or longer than\n\
         \x20   /// ~24.8 days). It is refused, never clamped.\n\
         \x20   pub fn with_timeout(&self, timeout: std::time::Duration) -> Result<Self, LogosError> {\n\
         \x20       Ok(Self { proxy: self.proxy.with_timeout(timeout)? })\n\
         \x20   }\n",
    );

    for m in &module.methods {
        let fn_name = snake(&m.name);
        let params_sig: Vec<String> = m
            .params
            .iter()
            .map(|p| format!("{}: {}", snake(&p.name), param_type(&p.ty, &recs)))
            .collect();
        let args: Vec<String> = m
            .params
            .iter()
            .map(|p| param_to_json(&snake(&p.name), &p.ty, &recs))
            .collect();
        let (ret_ty, conv) = return_conv(&m.return_type, &recs);
        // Carry the contract's doc comment onto the generated method.
        out.push('\n');
        for line in m.description.lines() {
            out.push_str(&format!("    /// {}\n", line));
        }
        out.push_str(&format!(
            "    pub fn {}(&self{}{}) -> Result<{}, LogosError> {{\n\
             \x20       let args = serde_json::Value::Array(vec![{}]);\n\
             \x20       let value = self.proxy.call_json(\"{}\", &args)?;\n\
             \x20       {}\n\
             \x20   }}\n",
            fn_name,
            if params_sig.is_empty() { "" } else { ", " },
            params_sig.join(", "),
            ret_ty,
            args.join(", "),
            m.name,
            conv
        ));

        // Async twin of the sync method — feature parity with the C++ client's
        // `<method>Async(..., callback)`. Fires the call and delivers the typed
        // result to a one-shot callback; inside a module the callback runs on
        // the Qt event loop, so it lands after the current method returns.
        let async_params = if params_sig.is_empty() {
            "&self, callback: F".to_string()
        } else {
            format!("&self, {}, callback: F", params_sig.join(", "))
        };
        out.push_str(&format!(
            "\n    /// Async twin of [`Self::{}`]: fire the call and receive the typed\n\
             \x20   /// result in `callback` once it lands — the Rust analog of the C++\n\
             \x20   /// client's `{}Async`. The callback runs from the protocol\n\
             \x20   /// completion path (the module's Qt event loop), so it fires after\n\
             \x20   /// the current method returns, never inline.\n\
             \x20   pub fn {}_async<F>({})\n\
             \x20   where\n\
             \x20       F: FnOnce(Result<{}, LogosError>) + Send + 'static,\n\
             \x20   {{\n\
             \x20       let args = serde_json::Value::Array(vec![{}]);\n\
             \x20       self.proxy.call_json_async(\"{}\", &args, move |result| {{\n\
             \x20           callback(result.and_then(|value| {}));\n\
             \x20       }});\n\
             \x20   }}\n",
            fn_name,
            m.name,
            fn_name,
            async_params,
            ret_ty,
            args.join(", "),
            m.name,
            conv
        ));
    }

    for e in &module.events {
        let event_struct = format!("{}Event", pascal(&e.name));
        // Author's event doc first, then the generated subscription notes.
        out.push('\n');
        for line in e.description.lines() {
            out.push_str(&format!("    /// {}\n", line));
        }
        out.push_str(&format!(
            "    /// Subscribe to the `{}` event. Payload arrives as a JSON array{};\n\
             \x20   /// decode each received item with [`Self::decode_{}`]. The returned\n\
             \x20   /// subscription owns its client share — move it into a listener\n\
             \x20   /// thread and iterate it; drop it to unsubscribe.\n\
             \x20   pub fn on_{}(&mut self) -> Result<EventSubscription, LogosError> {{\n\
             \x20       self.proxy.on(\"{}\")\n\
             \x20   }}\n",
            e.name,
            if e.params.is_empty() {
                String::new()
            } else {
                format!(
                    " of [{}]",
                    e.params
                        .iter()
                        .map(|p| {
                            format!(
                                "{}: {}{}",
                                p.name,
                                if p.is_optional() { "?" } else { "" },
                                p.value_type().name
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            snake(&e.name),
            snake(&e.name),
            e.name
        ));
        // Typed decoder: strict positional decode of the JSON-array payload
        // into the per-event struct — the Rust analog of the C++ wrappers'
        // typed event callbacks.
        out.push_str(&format!(
            "\n    /// Decode a received `{}` payload into its typed form.\n\
             \x20   /// Returns None if the payload doesn't match the contract.\n\
             \x20   pub fn decode_{}(ev: &EventData) -> Option<{}> {{\n\
             \x20       let arr = ev.data.as_array()?;\n\
             \x20       if arr.len() < {} {{ return None; }}\n\
             \x20       Some({} {{\n",
            e.name,
            snake(&e.name),
            event_struct,
            // Only the REQUIRED prefix has to be there: an optional trailing
            // parameter may arrive as null or not at all (decode is liberal in
            // an optional slot, absent and null being the same empty state).
            required_arity(&e.params),
            event_struct
        ));
        for (i, p) in e.params.iter().enumerate() {
            let field = snake(&p.name);
            let expr = if p.is_optional() {
                format!(
                    "match arr.get({}) {{ None | Some(serde_json::Value::Null) => None, Some(__v) => Some({}) }}",
                    i,
                    event_param_decode(p.value_type(), "__v")
                )
            } else {
                event_param_decode(&p.ty, &format!("arr[{}]", i))
            };
            out.push_str(&format!("            {}: {},\n", field, expr));
        }
        out.push_str("        })\n    }\n");
    }

    out.push_str("}\n\n");

    // Per-event typed payload structs (named after the event, not the module,
    // so they read naturally at the call site: `TotalChangedEvent { total }`).
    for e in &module.events {
        let event_struct = format!("{}Event", pascal(&e.name));
        out.push_str(&format!(
            "/// Typed payload of the `{}` event.\n#[derive(Debug, Clone)]\npub struct {} {{\n",
            e.name, event_struct
        ));
        for p in &e.params {
            out.push_str(&format!(
                "    pub {}: {},\n",
                snake(&p.name),
                event_param_type(&p.ty)
            ));
        }
        out.push_str("}\n\n");
    }
    out.push_str(&format!(
        "impl Default for {} {{\n    fn default() -> Self {{\n        Self::new()\n    }}\n}}\n",
        struct_name
    ));
    out
}

/// Generate typed dependency clients plus a `Modules` aggregate — the Rust
/// analog of C++'s generated `LogosModules` (`modules().calc.add(...)`).
/// Each entry pairs the aggregate field name with the dependency's parsed
/// contract; the client code is namespaced under a module of that name so
/// several dependencies' generated types can't collide.
pub fn generate_deps(deps: &[(String, ModuleDecl)]) -> String {
    let mut out = String::new();
    out.push_str(
        "// Typed dependency clients + the Modules aggregate — generated by\n\
         // logos-lidl-gen from the dependencies' LIDL contracts. Do not edit.\n\n",
    );
    for (name, decl) in deps {
        let field = snake(name);
        out.push_str(&format!("pub mod {} {{\n", field));
        for line in generate(decl).lines() {
            if line.is_empty() {
                out.push('\n');
            } else {
                out.push_str("    ");
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push_str("}\n\n");
    }

    out.push_str(
        "/// Typed access to this module's declared dependencies — one client\n\
         /// per dependency, bound to its contract's module name.\n\
         pub struct Modules {\n",
    );
    for (name, decl) in deps {
        let field = snake(name);
        out.push_str(&format!(
            "    pub {}: {}::{}Client,\n",
            field,
            field,
            pascal(&decl.name)
        ));
    }
    out.push_str("}\n\n");

    out.push_str("impl Modules {\n    pub fn new() -> Self {\n        Self {\n");
    for (name, decl) in deps {
        let field = snake(name);
        out.push_str(&format!(
            "            {}: {}::{}Client::new(),\n",
            field,
            field,
            pascal(&decl.name)
        ));
    }
    out.push_str("        }\n    }\n}\n\n");
    out.push_str(
        "impl Default for Modules {\n    fn default() -> Self {\n        Self::new()\n    }\n}\n\n\
         /// Convenience constructor mirroring C++'s `modules()` accessor.\n\
         pub fn modules() -> Modules {\n    Modules::new()\n}\n\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    const SAMPLE: &str = r#"
module calc_module {
  version "1.0.0"
  depends []
  method add(a: int, b: int) -> int description "Add two numbers"
  method describe(name: tstr) -> tstr
  method store(data: bstr) -> bool
  method dump() -> any
  event resultReady(total: int) description "Fires when a result is ready"
}
"#;

    #[test]
    fn generates_typed_client() {
        let m = parse(SAMPLE).unwrap();
        let code = generate(&m);
        assert!(code.contains("pub struct CalcModuleClient"));
        assert!(code.contains("pub fn add(&self, a: i64, b: i64) -> Result<i64, LogosError>"));
        assert!(code.contains("pub fn describe(&self, name: &str) -> Result<String, LogosError>"));
        assert!(code.contains("pub fn store(&self, data: &[u8]) -> Result<bool, LogosError>"));
        assert!(code.contains("logos_rust_sdk::bytes::encode(data)"));
        assert!(code.contains("pub fn dump(&self) -> Result<serde_json::Value, LogosError>"));
        // Async twins: parity with the C++ client's <method>Async(..., callback).
        assert!(code.contains("pub fn add_async<F>(&self, a: i64, b: i64, callback: F)"));
        assert!(code.contains("F: FnOnce(Result<i64, LogosError>) + Send + 'static,"));
        assert!(code.contains("self.proxy.call_json_async(\"add\", &args, move |result|"));
        // No-arg method still takes only the callback.
        assert!(code.contains("pub fn dump_async<F>(&self, callback: F)"));
        assert!(code.contains("pub fn on_result_ready(&mut self)"));
        assert!(code.contains("self.proxy.call_json(\"add\", &args)"));
        // Runtime binding: same typed surface, provider chosen at call time
        // (the interface-dependency pattern).
        assert!(code.contains("pub fn bind(module_name: &str) -> Self"));
        // Typed event payloads: per-event struct + strict decoder.
        assert!(code.contains("pub struct ResultReadyEvent"));
        assert!(code.contains("pub total: i64"));
        assert!(code.contains("pub fn decode_result_ready(ev: &EventData) -> Option<ResultReadyEvent>"));
        // Contract doc comments are carried onto the generated method/event.
        assert!(code.contains("/// Add two numbers"));
        assert!(code.contains("/// Fires when a result is ready"));
    }


    // A binary EVENT payload, SUBSCRIBE side. The typed payload struct must
    // carry real bytes (Vec<u8>), and the decoder must run the bstr arg through
    // the canonical tagged-bytes codec rather than clone the raw JSON. No prior
    // test covered bstr in an event — the gap the C++ analog (logos-cpp-sdk#99)
    // fell through. Mirrors delivery_module's messageReceived(..., payload: bstr).
    #[test]
    fn decodes_binary_event_payload() {
        let m = parse(
            "module delivery_module {\n  \
             version \"1.0.0\"\n  depends []\n  \
             event messageReceived(topic: tstr, payload: bstr)\n\
             }",
        )
        .unwrap();
        let code = generate(&m);
        // Typed payload struct carries real bytes, not serde_json::Value.
        assert!(code.contains("pub struct MessageReceivedEvent"));
        assert!(code.contains("pub payload: Vec<u8>"));
        // The decoder runs the bstr arg (index 1) through the tagged-bytes codec.
        assert!(code.contains(
            "pub fn decode_message_received(ev: &EventData) -> Option<MessageReceivedEvent>"
        ));
        assert!(code.contains("logos_rust_sdk::bytes::decode(&arr[1])?"));
        // A tstr arg in the same event still decodes as a plain string — the
        // bstr branch is not applied to every param.
        assert!(code.contains("arr[0].as_str()?.to_string()"));
    }

    /// Per-call timeout on the generated client. The lp_* ABI has always taken
    /// `timeout_ms`; the Rust surface hardcoded 0. It is exposed as ONE
    /// timeout-scoped view per client instead of an extra parameter per method,
    /// because Rust has no default arguments — see `with_timeout` in rustgen.
    #[test]
    fn generates_timeout_scoped_view() {
        let m = parse(SAMPLE).unwrap();
        let code = generate(&m);
        assert!(code.contains(
            "pub fn with_timeout(&self, timeout: std::time::Duration) -> Result<Self, LogosError>"
        ));
        // It must FORWARD to the proxy, which is what actually reaches the ABI.
        // A view that stored a timeout nobody passed to lp_invoke would read as
        // a guarantee and be none.
        assert!(code.contains("Ok(Self { proxy: self.proxy.with_timeout(timeout)? })"));
        // Exactly one per client — not one per method.
        assert_eq!(code.matches("pub fn with_timeout(").count(), 1);
    }

    /// The point of the scoped view: NOTHING about the existing entry points
    /// changed. Every signature a caller already writes must be emitted
    /// byte-for-byte as before, with no timeout parameter anywhere in it.
    #[test]
    fn existing_call_sites_keep_their_signatures() {
        let m = parse(SAMPLE).unwrap();
        let code = generate(&m);
        // Sync: unchanged arity and types.
        assert!(code.contains("pub fn add(&self, a: i64, b: i64) -> Result<i64, LogosError>"));
        assert!(code.contains("pub fn dump(&self) -> Result<serde_json::Value, LogosError>"));
        // Async: unchanged — still just the args plus the callback.
        assert!(code.contains("pub fn add_async<F>(&self, a: i64, b: i64, callback: F)"));
        assert!(code.contains("pub fn dump_async<F>(&self, callback: F)"));
        // No method grew a timeout parameter, and no parallel per-method entry
        // point was minted.
        assert!(!code.contains("timeout: std::time::Duration, callback"));
        assert!(!code.contains("_with_timeout("));
        for line in code.lines() {
            let sig = line.trim_start();
            if sig.starts_with("pub fn ") && !sig.starts_with("pub fn with_timeout(") {
                assert!(
                    !sig.contains("Duration"),
                    "a timeout parameter leaked into an existing entry point: {}",
                    sig
                );
            }
        }
    }

    #[test]
    fn generates_modules_aggregate() {
        let calc = parse(SAMPLE).unwrap();
        let auth = parse("module auth_module { depends [] method login(user: tstr) -> bool }").unwrap();
        let code = generate_deps(&[("calc".to_string(), calc), ("auth".to_string(), auth)]);
        assert!(code.contains("pub mod calc {"));
        assert!(code.contains("pub mod auth {"));
        assert!(code.contains("pub struct Modules {"));
        assert!(code.contains("pub calc: calc::CalcModuleClient,"));
        assert!(code.contains("pub auth: auth::AuthModuleClient,"));
        assert!(code.contains("pub fn modules() -> Modules"));
    }

    #[test]
    fn parser_handles_keywords_as_names() {
        let m = parse("module module { depends [] method method(version: tstr) -> bool }").unwrap();
        assert_eq!(m.name, "module");
        assert_eq!(m.methods[0].name, "method");
        assert_eq!(m.methods[0].params[0].name, "version");
    }

    #[test]
    fn parser_full_grammar() {
        let m = parse(
            "module x { version \"2.0.0\" depends [a, b] \
             type T { id: tstr ? blob: bstr tags: [tstr] meta: {tstr: any} } \
             method f(t: T, n: ? int) -> result \
             event e(payload: bstr) }",
        )
        .unwrap();
        assert_eq!(m.depends, vec!["a", "b"]);
        assert_eq!(m.types[0].fields.len(), 4);
        assert_eq!(m.events[0].params[0].ty.name, "bstr");
    }

    // ── Optionality ─────────────────────────────────────────────────────────
    //
    // These used to be one assertion that the grammar PARSED `?` — which it
    // always did — while no backend read the flag at all: every optional slot
    // came out as a plain `T` or an untyped Value. What has to be true is about
    // the EMITTED code, so that is what is asserted here.

    const OPTIONALS: &str = r#"
module opt_module {
  version "1.0.0"
  depends []
  type Account {
    id: tstr
    ? label: tstr
    note: ?bstr
  }
  method find(id: ?tstr) -> ?Account
  method describe(a: Account) -> tstr
  event changed(id: tstr, label: ?tstr)
}
"#;

    // R3: `? name: T` and `name: ?T` are one meaning with two spellings, and
    // MUST produce byte-identical code. A backend that reads only the type kind
    // types the flag spelling as a plain `T` — which is exactly what happened
    // to the struct FIELD while its encoder already treated it as empty-able.
    #[test]
    fn both_optional_spellings_emit_identical_code() {
        let head = "module m { version \"1.0.0\" depends [] type T { ";
        let tail = " } method f(t: T) -> tstr }";
        let flag = parse(&format!("{}? label: tstr{}", head, tail)).expect("parse flag spelling");
        let kind = parse(&format!("{}label: ?tstr{}", head, tail)).expect("parse type spelling");
        // Different spelling in the AST...
        assert!(flag.types[0].fields[0].optional);
        assert_eq!(kind.types[0].fields[0].ty.kind, TypeKind::Optional);
        // ...one answer, and one emitted result — client and provider alike.
        assert!(flag.types[0].fields[0].is_optional());
        assert!(kind.types[0].fields[0].is_optional());
        assert_eq!(generate(&flag), generate(&kind));
        assert_eq!(
            crate::rustgen_provider::generate_provider(&flag, "0.1.0"),
            crate::rustgen_provider::generate_provider(&kind, "0.1.0")
        );
        // And it is the OPTIONAL shape both landed on, not the plain one.
        assert!(generate(&flag).contains("pub label: Option<String>"), "{}", generate(&flag));
    }

    // A record field is the one NAMED slot: empty is spelled by omitting the
    // key. Decode is liberal — absent and null are the same empty state.
    #[test]
    fn optional_record_field_omits_the_key_and_decodes_liberally() {
        let m = parse(OPTIONALS).expect("parse");
        let code = generate(&m);

        // Rust's single empty inhabitant, for both spellings and for bstr.
        assert!(code.contains("pub id: String,"), "{}", code);
        assert!(code.contains("pub label: Option<String>,"), "{}", code);
        assert!(code.contains("pub note: Option<Vec<u8>>,"), "{}", code);

        // ENCODE: the key is left out entirely when empty — never written null.
        assert!(
            code.contains(
                "if let Some(__o) = &self.label { o.insert(\"label\".to_string(), serde_json::json!(__o)); }"
            ),
            "{}",
            code
        );
        // A bstr field still rides the tagged form when it IS present.
        assert!(
            code.contains(
                "if let Some(__o) = &self.note { o.insert(\"note\".to_string(), logos_rust_sdk::bytes::encode(&__o[..])); }"
            ),
            "{}",
            code
        );
        // The required field is untouched: still written unconditionally.
        assert!(code.contains("o.insert(\"id\".to_string(), serde_json::json!(self.id));"), "{}", code);

        // DECODE: absent and null alike are empty; a present value still has to
        // decode as T (the `?` fails the whole record, as for a required field).
        assert!(
            code.contains(
                "label: match o.get(\"label\") { None | Some(serde_json::Value::Null) => None, Some(__v) => Some(__v.as_str()?.to_string()) },"
            ),
            "{}",
            code
        );
        assert!(
            code.contains(
                "note: match o.get(\"note\") { None | Some(serde_json::Value::Null) => None, Some(__v) => Some(logos_rust_sdk::bytes::decode_lenient(__v)?) },"
            ),
            "{}",
            code
        );
        // Required fields keep the strict `?` decode.
        assert!(code.contains("id: o.get(\"id\")?.as_str()?.to_string(),"), "{}", code);
    }

    // A parameter, a return and an event payload are POSITIONAL slots: they
    // have no key to omit, so empty is `null` and the arity never changes.
    #[test]
    fn optional_positional_slots_are_spelled_null_on_the_wire() {
        let m = parse(OPTIONALS).expect("parse");
        let code = generate(&m);

        // Typed both ways: `Option<&str>` in, `Option<Account>` out.
        assert!(
            code.contains("pub fn find(&self, id: Option<&str>) -> Result<Option<Account>, LogosError>"),
            "{}",
            code
        );
        // The argument is still a slot in the array — arity is never changed.
        assert!(
            code.contains(
                "vec![match id { Some(__o) => serde_json::Value::from(__o), None => serde_json::Value::Null }]"
            ),
            "{}",
            code
        );
        // A null return is the empty state; a present one still decodes as the
        // record and still fails the call if it doesn't.
        assert!(
            code.contains("if value.is_null() { Ok(None) } else { (Account::from_json(&value)"),
            "{}",
            code
        );
        // The async twin agrees with its sync sibling.
        assert!(
            code.contains("F: FnOnce(Result<Option<Account>, LogosError>) + Send + 'static,"),
            "{}",
            code
        );

        // Event payloads: typed struct field + a decoder that reads an absent
        // element and a null element as the same empty state.
        assert!(code.contains("pub label: Option<String>,"), "{}", code);
        assert!(
            code.contains(
                "label: match arr.get(1) { None | Some(serde_json::Value::Null) => None, Some(__v) => Some(__v.as_str()?.to_string()) },"
            ),
            "{}",
            code
        );
        // Only the REQUIRED prefix is demanded of the payload.
        assert!(code.contains("if arr.len() < 1 { return None; }"), "{}", code);
        assert!(code.contains("id: arr[0].as_str()?.to_string(),"), "{}", code);
    }

    // Records: a `type` decl becomes a real Rust struct, and methods take and
    // return it instead of an untyped serde_json::Value. Additive — nothing
    // generated records before, so no existing signature changes.
    const RECORDS: &str = r#"
module info_module {
  version "1.0.0"
  depends []
  type Status {
    port: uint
    name: tstr
    blob: bstr
  }
  method describeStatus(s: Status) -> tstr
  method makeStatus() -> Status
  method makeStatuses() -> [Status]
}
"#;

    #[test]
    fn records_become_typed_rust_structs() {
        let m = parse(RECORDS).expect("parse");
        let code = generate(&m);

        // The struct, with each field at its 1-1 Rust type.
        assert!(code.contains("pub struct Status"), "{}", code);
        assert!(code.contains("pub port: u64"), "{}", code);
        assert!(code.contains("pub name: String"), "{}", code);
        assert!(code.contains("pub blob: Vec<u8>"), "{}", code);

        // A bstr field rides the TAGGED form in both directions. A serde derive
        // would emit a number array here, which no other language decodes as
        // bytes — the logos-protocol #21/#23 bug class.
        assert!(code.contains("logos_rust_sdk::bytes::encode(&self.blob[..])"), "{}", code);
        assert!(code.contains("logos_rust_sdk::bytes::decode_lenient"), "{}", code);

        // Methods speak the record, including inside a container.
        assert!(code.contains("pub fn describe_status(&self, s: &Status)"), "{}", code);
        assert!(code.contains("s.to_json()"), "{}", code);
        assert!(code.contains("-> Result<Status, LogosError>"), "{}", code);
        assert!(code.contains("-> Result<Vec<Status>, LogosError>"), "{}", code);
        assert!(code.contains("Status::from_json"), "{}", code);
    }

    // Composites that are NOT records keep their existing shape: retyping them
    // would change every existing consumer call site, which records do not.
    #[test]
    fn non_record_composites_are_unchanged() {
        let m = parse(SAMPLE).expect("parse");
        let code = generate(&m);
        assert!(code.contains("-> Result<serde_json::Value, LogosError>"), "{}", code);
    }

    // Consumer-side half of the same trap: `-> void` must not become
    // `-> Result<Void, LogosError>`.
    #[test]
    fn void_is_not_a_record_on_the_client_either() {
        let src = r#"
module v_module {
  version "1.0.0"
  depends []
  type Status {
    port: uint
  }
  method doVoid() -> void
  method getStatus() -> Status
}
"#;
        let m = crate::parse(src).expect("parse");
        let code = generate(&m);
        // (a plain contains("Void") would match the method name `doVoid`)
        assert!(!code.contains("Result<Void"), "void leaked in as a struct:\n{}", code);
        assert!(!code.contains("struct Void"), "void leaked in as a struct:\n{}", code);
        assert!(!code.contains("Void::from_json"), "void leaked in as a struct:\n{}", code);
        assert!(code.contains("pub fn do_void(&self) -> Result<(), LogosError>"), "{}", code);
        assert!(code.contains("pub fn get_status(&self) -> Result<Status, LogosError>"), "{}", code);
    }
}
