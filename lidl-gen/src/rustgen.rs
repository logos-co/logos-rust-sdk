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

/// Rust parameter type for a LIDL type.
/// The owned Rust type for a LIDL type. Records become their generated struct;
/// composites recurse, so `[Status]` is `Vec<Status>` and `{tstr: bstr}` is
/// `BTreeMap<String, Vec<u8>>`.
pub(crate) fn owned_type(ty: &TypeExpr, recs: &BTreeSet<String>) -> String {
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
            out.push_str(&format!("    pub {}: {},\n", snake(&f.name), owned_type(&f.ty, &recs)));
        }
        out.push_str("}\n\n");

        out.push_str(&format!("impl {} {{\n", name));
        out.push_str("    /// The record as its canonical JSON object.\n");
        out.push_str("    pub fn to_json(&self) -> serde_json::Value {\n");
        out.push_str("        let mut o = serde_json::Map::new();\n");
        for f in &t.fields {
            out.push_str(&format!(
                "        o.insert(\"{}\".to_string(), {});\n",
                f.name,
                enc_expr(&f.ty, &format!("self.{}", snake(&f.name)), &recs)
            ));
        }
        out.push_str("        serde_json::Value::Object(o)\n    }\n\n");

        out.push_str("    /// Decode the canonical JSON object. None if a field is missing or\n");
        out.push_str("    /// has the wrong shape — the provider reports the same mismatch with\n");
        out.push_str("    /// a field path, so this is the consumer-side half of that contract.\n");
        out.push_str("    pub fn from_json(v: &serde_json::Value) -> Option<Self> {\n");
        out.push_str("        let o = v.as_object()?;\n");
        out.push_str("        Some(Self {\n");
        for f in &t.fields {
            out.push_str(&format!(
                "            {}: {},\n",
                snake(&f.name),
                dec_expr(&f.ty, &format!("o.get(\"{}\")?", f.name), &recs)
            ));
        }
        out.push_str("        })\n    }\n}\n\n");
    }
    out
}

fn param_type(ty: &TypeExpr, recs: &BTreeSet<String>) -> String {
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
                        .map(|p| format!("{}: {}", p.name, p.ty.name))
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
            e.params.len(),
            event_struct
        ));
        for (i, p) in e.params.iter().enumerate() {
            let field = snake(&p.name);
            let expr = match (&p.ty.kind, p.ty.name.as_str()) {
                (TypeKind::Primitive, "tstr") => format!("arr[{}].as_str()?.to_string()", i),
                (TypeKind::Primitive, "int") => format!("arr[{}].as_i64()?", i),
                (TypeKind::Primitive, "uint") => format!("arr[{}].as_u64()?", i),
                (TypeKind::Primitive, "float64") => format!("arr[{}].as_f64()?", i),
                (TypeKind::Primitive, "bool") => format!("arr[{}].as_bool()?", i),
                (TypeKind::Primitive, "bstr") => {
                    format!("logos_rust_sdk::bytes::decode(&arr[{}])?", i)
                }
                _ => format!("arr[{}].clone()", i),
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
            let field_ty = match (&p.ty.kind, p.ty.name.as_str()) {
                (TypeKind::Primitive, "tstr") => "String",
                (TypeKind::Primitive, "int") => "i64",
                (TypeKind::Primitive, "uint") => "u64",
                (TypeKind::Primitive, "float64") => "f64",
                (TypeKind::Primitive, "bool") => "bool",
                (TypeKind::Primitive, "bstr") => "Vec<u8>",
                _ => "serde_json::Value",
            };
            out.push_str(&format!("    pub {}: {},\n", snake(&p.name), field_ty));
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
        assert!(m.types[0].fields[1].optional);
        assert_eq!(m.methods[0].params[1].ty.kind, TypeKind::Optional);
        assert_eq!(m.events[0].params[0].ty.name, "bstr");
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
