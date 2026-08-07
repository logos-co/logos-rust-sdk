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
//! `Result<serde_json::Value, String>`→result, `()`→void (returns only),
//! `Option<T>`→`?T` in a PARAMETER or an EVENT parameter.
//!
//! Optionality is admitted by exactly one rule: the value type must be a FIXED
//! POINT of the round trip, i.e. `rust_param_type(type_to_lidl(T))` must be the
//! same spelling the author wrote. A rust-first module is scaffolded
//! `--no-trait`, so the AUTHOR's trait is the dispatch's call site — anything
//! that does not map back onto their spelling fails to compile inside generated
//! code they never wrote, with an error pointing at a file they did not author.
//! Refusing it here means the error names the real cause instead.
//!
//! An optional RETURN is refused for a different reason entirely — see
//! [`OPTIONAL_RETURN_UNSUPPORTED`].

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

/// Why `-> Option<T>` is refused in the Rust-FIRST direction.
///
/// Be precise about the scope, because the platform does NOT ban optional
/// returns outright: logos-cpp-sdk's cdylib generator keeps them eligible
/// (`-> ?Point` and `-> ?tstr` are called out by name in its typeSupported), a
/// hand-written `.lidl` may declare one, and `test_fullapi_ext_{rust,cpp}`
/// exercise `echoOptional(v: ?tstr) -> ?tstr` as a cross-language conformance
/// case that the Python client tests too.
///
/// The problem is confined to the QT path. An empty optional is spelled JSON
/// null; logos-protocol's json→QVariant conversion turns null into an INVALID
/// QVariant, which is the same value that means "the call failed", and
/// core_service reports that as METHOD_FAILED. A Qt consumer of a `?T`-returning
/// method therefore cannot tell "found nothing" from "the call failed" — which
/// is why the Qt generators refuse to emit one at all (lidlCheckOptionalReturns
/// in logos-qt-sdk). Rust, C++-over-the-C-ABI and Python callers are unaffected;
/// they read the empty value through lp_invoke, which carries failure on a
/// separate channel.
///
/// This frontend refuses it as a deliberate SCOPE choice for the Rust-first
/// authoring surface, not because the shape is unrepresentable: a contract
/// derived here is published for cross-language consumption, and a Qt consumer
/// of it would hit the collision above. Relaxing this to match cpp-sdk is a
/// reasonable future change — it is a policy decision, not a bug fix.
pub(crate) const OPTIONAL_RETURN_UNSUPPORTED: &str =
    "an optional RETURN is not supported by the Rust-first frontend: an empty `?T` is spelled \
     JSON null, and a Qt consumer reads null as a FAILED call (logos_json_convert maps it to an \
     invalid QVariant, which core_service reports as METHOD_FAILED), so it could not tell \
     \"found nothing\" from \"the call failed\". Rust/C++/Python callers are unaffected. Take \
     `Option<T>` as a parameter, return Result<serde_json::Value, String>, or hand-write the \
     .lidl if the contract is not consumed from Qt";

/// The admission gate for the value type of an `Option<..>`.
///
/// Keyed on the AUTHOR'S SPELLING rather than the mapped LIDL type, because that
/// is the comparison that decides whether the crate compiles: `i32` and `i64`
/// both map to `int`, but only `i64` comes back out of `rust_param_type`; `&str`
/// and `String` both map to `tstr`, but only `String` comes back. Admitting a
/// non-fixed-point would emit a trait the author's own impl cannot satisfy.
///
fn option_value_is_fixed_point(ty: &syn::Type) -> Result<(), String> {
    let p = match ty {
        syn::Type::Path(p) => p,
        // &str / &[u8] inside an Option: the borrowed form never comes back out
        // of rust_param_type, which yields String / Vec<u8>.
        syn::Type::Reference(_) => {
            return Err("Option<&T> has no LIDL type — the generated trait takes the OWNED \
                        form, so write Option<String> or Option<Vec<u8>>"
                .into())
        }
        syn::Type::Tuple(t) if t.elems.is_empty() => {
            return Err("Option<()> has no LIDL type — `?void` is not a type".into())
        }
        _ => return Err(format!("Option<{}> is not a supported type", render(ty))),
    };
    let last = p.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
    match last.as_str() {
        "String" | "i64" | "u64" | "f64" | "bool" => Ok(()),
        "Vec" => match generic_arg(p).as_ref().and_then(type_name) {
            Some(n) if n == "u8" => Ok(()),
            _ => Err("Option<Vec<T>> has no LIDL type unless T is u8 (which is `?bstr`) — a \
                      typed array comes back from the generator as an untyped \
                      serde_json::Value, which will not match your signature"
                .into()),
        },
        "Option" => Err("Option<Option<T>> has no LIDL type — `?T` is TWO-state (a value, or \
                         empty) and Rust has exactly one empty inhabitant, so there is nowhere \
                         for a third state to live"
            .into()),
        "i32" | "u32" => Err(format!(
            "Option<{}> has no LIDL type — LIDL numbers are 64-bit, and the generated trait \
             would take Option<i64>/Option<u64>",
            last
        )),
        // `?any`. Matched on the last path segment, like the non-optional
        // `Value` arm above — a `my_crate::Value` lands here too and is mapped
        // to `any` just as it would be outside an Option, so the two arms agree.
        //
        // VALUE-LOSSY, deliberately: `any` already carries JSON null among its
        // inhabitants, so `?any` has two spellings of empty and they collapse —
        // `Some(Value::Null)` comes back as `None`. That is the two-state rule
        // holding at the type level (`?T` is a value or empty, never a third
        // thing), not a defect in the mapping.
        "Value" => Ok(()),
        "Result" => Err("Option<Result<..>> has no LIDL type — a result carries its own empty \
                         discriminant"
            .into()),
        other => Err(format!(
            "Option<{}> has no LIDL type — only String, i64, u64, f64, bool, Vec<u8> and \
             serde_json::Value may be \
             optional",
            other
        )),
    }
}

/// Best-effort rendering of a type for an error message. `quote` is not a
/// dependency, so fall back to the last path segment when there is one.
fn render(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_else(|| "?".into()),
        syn::Type::Reference(_) => "&_".into(),
        syn::Type::Slice(_) => "[_]".into(),
        syn::Type::Tuple(_) => "(..)".into(),
        _ => "_".into(),
    }
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
                        // Without this guard, adding the Option arm below would
                        // silently WIDEN Vec<Option<T>> from a clean frontend
                        // error into `[?T]` — which comes back from the provider
                        // backend as an untyped serde_json::Value, so the
                        // published contract would be one the author's own impl
                        // cannot satisfy.
                        if elem.is_optional() {
                            return Err(
                                "Vec<Option<T>> has no LIDL type: `[?T]` comes back from the \
                                 generator as an untyped serde_json::Value, which will not \
                                 match your signature"
                                    .into(),
                            );
                        }
                        Ok(TypeExpr {
                            kind: TypeKind::Array,
                            name: String::new(),
                            elements: vec![elem],
                        })
                    }
                }
                // `Option<T>` is `?T` — the Rust-first half of optionality, the
                // mirror of the C++ header parser's `std::optional<T>`.
                // Everything the gate refuses is refused HERE, where the reason
                // is known, rather than as an unexplained `mismatched types`
                // inside the generated scaffold.
                "Option" => {
                    if is_return {
                        return Err(OPTIONAL_RETURN_UNSUPPORTED.into());
                    }
                    let inner = generic_arg(p).ok_or("Option missing type argument")?;
                    option_value_is_fixed_point(&inner)?;
                    Ok(TypeExpr::optional(type_to_lidl(&inner, /*is_return=*/ false)?))
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

    // --- optionality ------------------------------------------------------

    const OPT_SRC: &str = r#"
pub trait OptModule {
    fn find(&mut self, id: Option<String>, exact: bool) -> String;
    fn store(&mut self, blob: Option<Vec<u8>>) -> bool;
    fn tick(&mut self, n: Option<i64>, u: Option<u64>, f: Option<f64>, b: Option<bool>) -> f64;
    fn borrowed(&mut self, id: &Option<String>) -> bool;
    fn untyped(&mut self, v: Option<serde_json::Value>) -> bool;
}

pub trait OptModuleEvents {
    fn changed(&self, label: Option<String>);
}
"#;

    #[test]
    fn option_maps_to_the_optional_kind_in_a_parameter_and_an_event() {
        let m = extract_from_rust(OPT_SRC, "OptModule", None, "1.0.0").unwrap();

        // Every optional is a well-formed `?T`: Optional kind, empty name,
        // exactly one element. A degenerate Optional would be simultaneously
        // omittable, undecodable, and a serializer panic.
        let check = |p: &ParamDecl, value: &str| {
            assert!(p.is_optional(), "{} should be optional", p.name);
            assert_eq!(p.ty.kind, TypeKind::Optional);
            assert_eq!(p.ty.name, "", "an Optional node carries no name");
            assert_eq!(p.ty.elements.len(), 1, "exactly one element");
            assert_eq!(p.value_type().name, value);
        };

        let find = &m.methods[0];
        check(&find.params[0], "tstr");
        // A non-optional sibling is untouched.
        assert!(!find.params[1].is_optional());
        assert_eq!(find.params[1].ty.name, "bool");

        // Option<Vec<u8>> is `?bstr` — NOT an array of uint. The bstr
        // special-case must survive the Option wrapper.
        check(&m.methods[1].params[0], "bstr");

        let tick = &m.methods[2];
        check(&tick.params[0], "int");
        check(&tick.params[1], "uint");
        check(&tick.params[2], "float64");
        check(&tick.params[3], "bool");

        // &Option<T> unwraps to Option<T> via the reference arm.
        check(&m.methods[3].params[0], "tstr");

        // `?any` — admitted, and value-lossy by construction: `any` already
        // carries null, so Some(Value::Null) round-trips as None.
        check(&m.methods[4].params[0], "any");

        // Event parameters take optionals too.
        check(&m.events[0].params[0], "tstr");
    }

    #[test]
    fn only_a_fixed_point_may_be_optional() {
        // Each entry must be refused, and the message must name the reason —
        // these errors are the whole point of the gate, since the alternative
        // is an unexplained type mismatch inside generated code.
        let cases: &[(&str, &str)] = &[
            ("Option<Option<i64>>", "TWO-state"),
            ("Option<Vec<i64>>", "unless T is u8"),
            ("Option<&str>", "OWNED"),
            ("Option<i32>", "64-bit"),
            ("Option<u32>", "64-bit"),
            ("Option<()>", "?void"),
            ("Option<Result<serde_json::Value, String>>", "discriminant"),
            ("Option<std::collections::HashMap<String,String>>", "only String"),
            ("Option<MyStruct>", "only String"),
            ("Vec<Option<String>>", "Vec<Option<T>>"),
        ];
        for (spelling, needle) in cases {
            let src = format!("pub trait X {{ fn f(&mut self, p: {}) -> bool; }}", spelling);
            let err = extract_from_rust(&src, "X", None, "1.0.0")
                .expect_err(&format!("{} must be refused", spelling));
            assert!(
                err.contains(needle),
                "{}: message should explain `{}`, got: {}",
                spelling,
                needle,
                err
            );
        }
    }

    #[test]
    fn an_optional_return_is_refused_with_its_reason() {
        // TRIPWIRE. This goes red the day someone teaches the host path to tell
        // an empty optional apart from a failed call. When it does, read
        // OPTIONAL_RETURN_UNSUPPORTED before deleting it: the fix is to NARROW
        // the null-means-failure rule to methods that declare an optional
        // return, never to delete the rule.
        let src = "pub trait X { fn f(&mut self) -> Option<String>; }";
        let err = extract_from_rust(src, "X", None, "1.0.0").unwrap_err();
        assert!(err.contains("METHOD_FAILED"), "got: {}", err);
        assert!(err.contains("optional RETURN"), "got: {}", err);
    }

    #[test]
    fn optionals_survive_rust_to_lidl_to_rust() {
        // The loop nothing else in the crate closes: the author's trait is the
        // generated dispatch's call site, so every contract line must come back
        // out of generate_provider VERBATIM. Derived from SRC rather than
        // hardcoded, so it cannot be kept green by scoping it to what already
        // works.
        const SRC: &str = r#"
pub trait OptRoundTrip {
    fn find(&mut self, id: Option<String>) -> String;
    fn store(&mut self, blob: Option<Vec<u8>>) -> bool;
    fn tick(&mut self, n: Option<i64>, u: Option<u64>, f: Option<f64>, b: Option<bool>) -> f64;
    fn untyped(&mut self, v: Option<serde_json::Value>) -> bool;
    fn plain(&mut self, id: String, n: i64) -> bool;
}
"#;
        let m = extract_from_rust(SRC, "OptRoundTrip", None, "1.0.0").unwrap();

        // logos-lidl's serializer spells a type-kind optional with a space.
        let text = crate::serialize(&m);
        assert!(text.contains("method find(id: ? tstr) -> tstr"), "{}", text);
        assert!(text.contains("method store(blob: ? bstr) -> bool"), "{}", text);

        let reparsed = crate::parse(&text).expect("reparse");
        let code = crate::generate_provider(&reparsed, "0.3.0");

        for sig in SRC
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("fn ") && l.ends_with(';'))
        {
            assert!(code.contains(sig), "not a fixed point: {}\n{}", sig, code);
        }
    }
}
