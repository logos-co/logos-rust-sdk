//! Rust-source → LIDL frontend — the Rust analog of the C++ generator's
//! `--header-to-lidl` (impl-header parser).
//!
//! Lets a module author declare the interface IN RUST and derive the `.lidl`
//! contract from it, instead of the other way around. The conventions mirror
//! the C++ universal-module header:
//!
//! - The contract is a plain trait declared `: Send + 'static` (the same
//!   supertraits the generated scaffold's trait carries — the dispatch
//!   stores the impl in a Box<dyn Any + Send>): required methods (no
//!   default body) are the module's IPC methods. Methods WITH default
//!   bodies (e.g. the framework's `on_context_ready`) are not part of the
//!   contract.
//! - Events live on a companion trait named `<Trait>Events` — the Rust
//!   analog of the C++ `logos_events:` section. Each method is one event
//!   (return type must be `()`).
//! - `///` doc comments on a contract/event method become that method's
//!   `description` (the Rust analog of the C++ impl-header doc capture),
//!   carried into the `.lidl` and on to the generated client.
//!
//! Supported types (the std-convertible LIDL subset): `i64`→int, `u64`→uint,
//! `f64`→float64, `bool`→bool, `String`/`&str`→tstr, `Vec<u8>`/`&[u8]`→bstr,
//! `serde_json::Value`/`&serde_json::Value`→any,
//! `Result<serde_json::Value, String>`→result, `()`→void (returns only).

use crate::ast::*;

/// Collect a method's `///` doc comment (desugared by syn into `#[doc = "..."]`
/// attributes) into a single description string, mirroring the C++ impl-header
/// parser's `joinDocLines`: one line per attribute, leading/trailing blank
/// lines dropped, joined with `\n`.
fn doc_from_attrs(attrs: &[syn::Attribute]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
                let line = s.value();
                // `///` desugars with a leading space; drop exactly one.
                lines.push(line.strip_prefix(' ').unwrap_or(&line).to_string());
            }
        }
    }
    while lines.first().map_or(false, |l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().map_or(false, |l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
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

fn type_to_lidl(ty: &syn::Type, is_return: bool) -> Result<TypeExpr, String> {
    // Unwrap references: &str, &[u8], &serde_json::Value.
    if let syn::Type::Reference(r) = ty {
        return type_to_lidl(&r.elem, is_return);
    }
    match ty {
        syn::Type::Tuple(t) if t.elems.is_empty() => {
            if is_return {
                Ok(TypeExpr::primitive("void"))
            } else {
                Err("unit type () is not a valid parameter".into())
            }
        }
        syn::Type::Slice(s) => {
            // &[u8] arrives here after the reference unwrap.
            if type_name(&s.elem) == Some("u8".into()) {
                Ok(TypeExpr::primitive("bstr"))
            } else {
                Err(format!("unsupported slice type: {}", quote_type(ty)))
            }
        }
        syn::Type::Path(p) => {
            let segs: Vec<String> =
                p.path.segments.iter().map(|s| s.ident.to_string()).collect();
            let last = segs.last().cloned().unwrap_or_default();
            match last.as_str() {
                "i64" | "i32" => Ok(TypeExpr::primitive("int")),
                "u64" | "u32" => Ok(TypeExpr::primitive("uint")),
                "f64" => Ok(TypeExpr::primitive("float64")),
                "bool" => Ok(TypeExpr::primitive("bool")),
                "String" | "str" => Ok(TypeExpr::primitive("tstr")),
                "Value" => Ok(TypeExpr::primitive("any")),
                "Vec" => {
                    let inner = generic_arg(p).ok_or("Vec missing type argument")?;
                    if type_name(&inner) == Some("u8".into()) {
                        Ok(TypeExpr::primitive("bstr"))
                    } else {
                        let elem = type_to_lidl(&inner, false)?;
                        Ok(TypeExpr {
                            kind: TypeKind::Array,
                            name: String::new(),
                            elements: vec![elem],
                        })
                    }
                }
                "Result" if is_return => Ok(TypeExpr::primitive("result")),
                other => Err(format!("unsupported type: {}", other)),
            }
        }
        _ => Err(format!("unsupported type shape: {}", quote_type(ty))),
    }
}

fn type_name(ty: &syn::Type) -> Option<String> {
    if let syn::Type::Path(p) = ty {
        p.path.segments.last().map(|s| s.ident.to_string())
    } else {
        None
    }
}

fn generic_arg(p: &syn::TypePath) -> Option<syn::Type> {
    let seg = p.path.segments.last()?;
    if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
        for a in &args.args {
            if let syn::GenericArgument::Type(t) = a {
                return Some(t.clone());
            }
        }
    }
    None
}

fn quote_type(ty: &syn::Type) -> String {
    // Debug-ish rendering without pulling in `quote`.
    format!("{:?}", std::mem::discriminant(ty))
}

fn signature_to_params(sig: &syn::Signature) -> Result<Vec<ParamDecl>, String> {
    let mut params = Vec::new();
    for input in &sig.inputs {
        match input {
            syn::FnArg::Receiver(_) => {} // &self / &mut self
            syn::FnArg::Typed(pt) => {
                let name = match &*pt.pat {
                    syn::Pat::Ident(id) => id.ident.to_string(),
                    _ => return Err(format!("unsupported parameter pattern in {}", sig.ident)),
                };
                let ty = type_to_lidl(&pt.ty, false)
                    .map_err(|e| format!("{}({}): {}", sig.ident, name, e))?;
                params.push(ParamDecl { name, ty });
            }
        }
    }
    Ok(params)
}

/// Extract a LIDL ModuleDecl from Rust source: `trait_name` is the contract
/// trait; events come from the companion `<trait_name>Events` trait when
/// present. `module_name` defaults to snake_case of the trait name.
pub fn extract_from_rust(
    source: &str,
    trait_name: &str,
    module_name: Option<&str>,
    version: &str,
) -> Result<ModuleDecl, String> {
    let file: syn::File =
        syn::parse_file(source).map_err(|e| format!("Rust parse error: {}", e))?;

    let mut module = ModuleDecl {
        name: module_name
            .map(str::to_string)
            .unwrap_or_else(|| snake(trait_name)),
        version: version.to_string(),
        ..Default::default()
    };

    let events_trait = format!("{}Events", trait_name);
    let mut found_contract = false;

    for item in &file.items {
        let syn::Item::Trait(tr) = item else { continue };
        let ident = tr.ident.to_string();

        if ident == trait_name {
            found_contract = true;
            for ti in &tr.items {
                let syn::TraitItem::Fn(f) = ti else { continue };
                // Default-bodied methods (framework hooks, helpers) are not
                // part of the IPC contract.
                if f.default.is_some() {
                    continue;
                }
                let return_type = match &f.sig.output {
                    syn::ReturnType::Default => TypeExpr::primitive("void"),
                    syn::ReturnType::Type(_, t) => type_to_lidl(t, true)
                        .map_err(|e| format!("{}: {}", f.sig.ident, e))?,
                };
                module.methods.push(MethodDecl {
                    name: f.sig.ident.to_string(),
                    params: signature_to_params(&f.sig)?,
                    return_type,
                    description: doc_from_attrs(&f.attrs),
                    // The return-shape flags are re-derived from the LIDL return
                    // type by logos-lidl when the .lidl is parsed for codegen, so
                    // they need not be set here (the serializer ignores them).
                    json_return: false,
                    result_return: false,
                });
            }
        } else if ident == events_trait {
            for ti in &tr.items {
                let syn::TraitItem::Fn(f) = ti else { continue };
                if !matches!(f.sig.output, syn::ReturnType::Default) {
                    return Err(format!(
                        "event {} must not declare a return type",
                        f.sig.ident
                    ));
                }
                module.events.push(EventDecl {
                    name: f.sig.ident.to_string(),
                    params: signature_to_params(&f.sig)?,
                    description: doc_from_attrs(&f.attrs),
                });
            }
        }
    }

    if !found_contract {
        return Err(format!("trait {} not found in source", trait_name));
    }
    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
/// The calc contract, declared in Rust.
pub trait RustCalcModule {
    /// Add two numbers.
    /// Returns their sum.
    fn add(&mut self, a: i64, b: i64) -> i64;
    fn greet(&mut self, name: String) -> String;
    fn store(&mut self, data: Vec<u8>) -> bool;
    fn fetch(&mut self) -> Result<serde_json::Value, String>;
    /// Framework hook — defaulted, so NOT part of the contract.
    fn on_context_ready(&mut self, _ctx: &RustModuleContext) {}
}

pub trait RustCalcModuleEvents {
    /// Fires when the running total changes.
    fn total_changed(&self, total: i64);
}
"#;

    #[test]
    fn extracts_contract_from_rust() {
        let m = extract_from_rust(SRC, "RustCalcModule", None, "1.0.0").unwrap();
        assert_eq!(m.name, "rust_calc_module");
        assert_eq!(m.version, "1.0.0");
        assert_eq!(m.methods.len(), 4); // on_context_ready excluded
        assert_eq!(m.methods[0].name, "add");
        assert_eq!(m.methods[0].params[0].ty.name, "int");
        assert_eq!(m.methods[1].params[0].ty.name, "tstr");
        assert_eq!(m.methods[2].params[0].ty.name, "bstr");
        assert_eq!(m.methods[3].return_type.name, "result");
        assert_eq!(m.events.len(), 1);
        assert_eq!(m.events[0].name, "total_changed");
        assert_eq!(m.events[0].params[0].ty.name, "int");
    }

    #[test]
    fn captures_doc_comments_as_descriptions() {
        let m = extract_from_rust(SRC, "RustCalcModule", None, "1.0.0").unwrap();
        // Multi-line `///` joined with a newline; blank-trimmed.
        assert_eq!(m.methods[0].description, "Add two numbers.\nReturns their sum.");
        // Undocumented method → empty description.
        assert_eq!(m.methods[1].description, "");
        // Event docs are captured too.
        assert_eq!(m.events[0].description, "Fires when the running total changes.");
    }

    #[test]
    fn rejects_unknown_types() {
        let bad = "pub trait X { fn f(&mut self, p: std::collections::HashMap<String,String>) -> i64; }";
        assert!(extract_from_rust(bad, "X", None, "1.0.0").is_err());
    }
}
