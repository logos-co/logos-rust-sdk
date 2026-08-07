//! Rust provider generation: the common module-impl C ABI exports
//! (logos_module_impl.h) around a Rust implementation — the exact same
//! contract the C++ SDK emits, so the uniform Qt glue (and a future no-Qt
//! host) drives Rust and C++ modules identically.
//!
//! Generated layout for module `calc_demo`:
//!   - trait `CalcDemoModule` — typed methods the author implements
//!   - `RustModuleContext` accessor (module path / instance id /
//!     persistence path, stamped by the host via set_context)
//!   - typed event emitters (`emit_total_changed(...)`)
//!   - #[no_mangle] exports: logos_module_dispatch / get_methods /
//!     set_context / set_emit_callback / accept_token /
//!     get_protocol_version / string_free
//!
//! The author writes `struct MyImpl; impl CalcDemoModule for MyImpl {...}`
//! and calls the generated `logos_module_register!(MyImpl)`-equivalent via
//! `register_module::<MyImpl>()` in a ctor, or simply relies on the
//! generated `Default`-based instantiation.

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

/// Whether `ty` is a usable `?T`. A degenerate Optional carrying no value type
/// (only reachable by hand-building an AST) keeps the untyped fallback.
fn is_optional(ty: &TypeExpr) -> bool {
    ty.is_optional() && !ty.elements.is_empty()
}

fn rust_param_type(ty: &TypeExpr, recs: &BTreeSet<String>) -> String {
    // `?T` is `Option<T>` — Rust's single empty inhabitant, so the author gets
    // the two states the contract declares and no third one.
    if is_optional(ty) {
        return format!("Option<{}>", rust_param_type(ty.value_type(), recs));
    }
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "tstr") => "String".into(),
        (TypeKind::Primitive, "int") => "i64".into(),
        (TypeKind::Primitive, "uint") => "u64".into(),
        (TypeKind::Primitive, "float64") => "f64".into(),
        (TypeKind::Primitive, "bool") => "bool".into(),
        (TypeKind::Primitive, "bstr") => "Vec<u8>".into(),
        // A record the contract DECLARES is a real struct here too — the author
        // gets `s: Status`, not a serde_json::Value to pick apart. Other
        // composites stay Value: retyping THOSE would change existing impls.
        //
        // `recs.contains` is load-bearing, not defensive: the LIDL front end has
        // no `void` builtin, so `-> void` arrives here as Named("void"). Mapping
        // every Named to its pascal-cased struct emitted `-> Void` and broke
        // every provider with a void method.
        (TypeKind::Named, n) if recs.contains(n) => {
            crate::rustgen::owned_type(ty, recs)
        }
        (TypeKind::Array, _)
            if ty.elements.len() == 1 && crate::rustgen::is_record(&ty.elements[0], recs) =>
        {
            crate::rustgen::owned_type(ty, recs)
        }
        _ => "serde_json::Value".into(),
    }
}

fn is_void(ty: &TypeExpr) -> bool {
    // `void` is not a LIDL builtin, so it arrives as Named("void") and never as
    // a declared record. Checked by name in exactly the places that need it,
    // rather than added to the builtin table, so the parser stays the one
    // authority on what a LIDL type is.
    matches!(&ty.kind, TypeKind::Named) && ty.name == "void"
}

fn rust_return_type(ty: &TypeExpr, recs: &BTreeSet<String>) -> String {
    if is_optional(ty) {
        return format!("Option<{}>", rust_return_type(ty.value_type(), recs));
    }
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "result") => "Result<serde_json::Value, String>".into(),
        // A void method returns nothing. It used to fall to the catch-all and
        // hand the author `-> serde_json::Value`, which is how it ended up
        // returning JSON null: null is the failure token on the Qt slot above,
        // so the same void method answered `true` from a C++ provider and
        // METHOD_FAILED from this one.
        _ if is_void(ty) => "()".into(),
        _ => rust_param_type(ty, recs),
    }
}

/// Qt-style type names for the interface JSON — identical to what the C++
/// glue/host expects from getMethods().
fn qt_type_name(ty: &TypeExpr) -> String {
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "tstr") => "QString".into(),
        (TypeKind::Primitive, "bstr") => "QByteArray".into(),
        (TypeKind::Primitive, "int") => "int".into(),
        (TypeKind::Primitive, "uint") => "int".into(),
        (TypeKind::Primitive, "float64") => "double".into(),
        (TypeKind::Primitive, "bool") => "bool".into(),
        (TypeKind::Primitive, "result") => "LogosResult".into(),
        // Matches what the C++ backend advertises. Without this a void method
        // was published as returnType "QVariant" here and "void" there — a
        // second divergence, in the interface metadata rather than the value.
        (TypeKind::Named, "void") => "void".into(),
        (TypeKind::Array, _) => "QVariantList".into(),
        (TypeKind::Map, _) => "QVariantMap".into(),
        _ => "QVariant".into(),
    }
}

/// The LIDL type as a runtime `args::Ty` descriptor, so generated dispatch can
/// validate a composite argument against its declared shape. `&Ty::Int` and the
/// nested forms are constant expressions, so they promote to 'static.
fn ty_descriptor(ty: &TypeExpr, module: &ModuleDecl) -> String {
    let t = |n: &str| format!("logos_rust_sdk::args::Ty::{}", n);
    // `?T` widens the slot by exactly one inhabitant: Ty::Opt accepts null (and
    // an absent key, which reads as null) and otherwise checks T as usual.
    if is_optional(ty) {
        return format!(
            "logos_rust_sdk::args::Ty::Opt(&{})",
            ty_descriptor(ty.value_type(), module)
        );
    }
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "int") => t("Int"),
        (TypeKind::Primitive, "uint") => t("Uint"),
        (TypeKind::Primitive, "float64") => t("Float64"),
        (TypeKind::Primitive, "bool") => t("Bool"),
        (TypeKind::Primitive, "tstr") => t("Tstr"),
        (TypeKind::Primitive, "bstr") => t("Bstr"),
        (TypeKind::Array, _) if ty.elements.len() == 1 => {
            format!("logos_rust_sdk::args::Ty::Arr(&{})", ty_descriptor(&ty.elements[0], module))
        }
        (TypeKind::Map, _) if ty.elements.len() == 2 => {
            format!("logos_rust_sdk::args::Ty::Map(&{})", ty_descriptor(&ty.elements[1], module))
        }
        // A record declared by this contract expands to its fields, so a bad
        // field is reported by name (arg0.port) exactly as the C++ codec does.
        (TypeKind::Named, n) => match module.types.iter().find(|t| t.name == *n) {
            Some(rec) => {
                let fields: Vec<String> = rec
                    .fields
                    .iter()
                    // Both spellings of an optional field (`? name: T` and
                    // `name: ?T`) are reconciled by the frontend, so they emit
                    // the same Ty::Opt descriptor. An optional field's missing
                    // key stops being the mismatch it is for a required one.
                    .map(|f| {
                        let inner = ty_descriptor(f.value_type(), module);
                        let slot = if f.is_optional() {
                            format!("logos_rust_sdk::args::Ty::Opt(&{})", inner)
                        } else {
                            inner
                        };
                        format!("(\"{}\", &{})", f.name, slot)
                    })
                    .collect();
                format!("logos_rust_sdk::args::Ty::Record(&[{}])", fields.join(", "))
            }
            // An undeclared name has no shape to check against.
            None => t("Any"),
        },
        // `any` and anything else: stop recursing, as C++ does.
        _ => t("Any"),
    }
}

/// The `args::as_*` suffix that decodes this type into an owned Rust scalar,
/// or None for a composite (which arrives as a validated `serde_json::Value`).
/// One place, so the accessor choice and everything keyed off it agree.
fn scalar_accessor(ty: &TypeExpr) -> Option<&'static str> {
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "tstr") => Some("string"),
        (TypeKind::Primitive, "int") => Some("i64"),
        (TypeKind::Primitive, "uint") => Some("u64"),
        (TypeKind::Primitive, "float64") => Some("f64"),
        (TypeKind::Primitive, "bool") => Some("bool"),
        (TypeKind::Primitive, "bstr") => Some("bytes"),
        _ => None,
    }
}

/// The accessor for one parameter: `Ok(value)` or the C++-matching mismatch
/// message. Composites stay a pass-through clone (see args::as_value) — typed
/// validation of [T]/{tstr: T} is the remaining gap against C++.
fn arg_accessor(ty: &TypeExpr, index: usize, module: &ModuleDecl) -> (String, bool) {
    // `?T` reads through the `as_opt_*` twin of T's accessor: an absent
    // argument and an explicit null are the same empty state, and a present
    // one is checked against T exactly as before.
    let opt = is_optional(ty);
    match scalar_accessor(ty.value_type()) {
        Some(f) => (
            format!(
                "logos_rust_sdk::args::{}{}(args, {})",
                if opt { "as_opt_" } else { "as_" },
                f,
                index
            ),
            true,
        ),
        // Composites keep arriving as serde_json::Value (retyping them would
        // change every existing module's trait signatures), but they are now
        // VALIDATED against the declared LIDL type first — so a [int] carrying a
        // string fails here exactly as it does in a C++ provider, with the same
        // arg0[1] path, instead of reaching the module unchecked.
        None => (
            format!(
                "logos_rust_sdk::args::as_value_checked(args, {}, &{})",
                index,
                ty_descriptor(ty, module)
            ),
            true,
        ),
    }
}

/// Whether a validated value still has to be lifted into `Option` by hand.
///
/// An optional SCALAR arrives already typed (`as_opt_*` returns `Option<T>`)
/// and an optional RECORD is lifted by its decode, but an optional composite
/// that stays untyped comes back as a raw `serde_json::Value` whose null IS the
/// empty state — while the trait signature says `Option<serde_json::Value>`.
/// Without this step the two disagree and the generated module does not compile.
fn needs_optional_lift(ty: &TypeExpr, recs: &BTreeSet<String>) -> bool {
    is_optional(ty)
        && scalar_accessor(ty.value_type()).is_none()
        && record_decode_for(ty, recs).is_none()
}

/// How to turn a validated serde_json::Value into the record struct the impl
/// signature asks for. None when the parameter is not record-shaped.
fn record_decode_for(ty: &TypeExpr, recs: &BTreeSet<String>) -> Option<String> {
    let vty = ty.value_type();
    let decode = match (&vty.kind, vty.name.as_str()) {
        (TypeKind::Named, n) if recs.contains(n) => {
            Some(format!("{}::from_json(&__V__)", pascal(n)))
        }
        (TypeKind::Array, _)
            if vty.elements.len() == 1 && crate::rustgen::is_record(&vty.elements[0], recs) =>
        {
            Some(format!(
                "__V__.as_array().and_then(|__a| __a.iter().map({}::from_json).collect::<Option<Vec<_>>>())",
                pascal(&vty.elements[0].name)
            ))
        }
        _ => None,
    }?;
    // `?Status` is `Option<Status>`: null (which the Ty::Opt check above has
    // already accepted) is the empty state; anything else must still decode as
    // the record. The outer Option is the "malformed" channel the caller
    // reports on, so empty is a successful `Some(None)`.
    Some(if is_optional(ty) {
        format!("if __V__.is_null() {{ Some(None) }} else {{ ({}).map(Some) }}", decode)
    } else {
        decode
    })
}

fn ret_to_json(ty: &TypeExpr, expr: &str, recs: &BTreeSet<String>) -> String {
    // A return is a POSITIONAL slot: it has no key to omit, so an empty `?T`
    // is spelled `null` (the record-field case, the one named slot, omits the
    // key instead — see rustgen::emit_records).
    if is_optional(ty) {
        return format!(
            "match {} {{ Some(__o) => {}, None => serde_json::Value::Null }}",
            expr,
            ret_to_json(ty.value_type(), "__o", recs)
        );
    }
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "bstr") => format!("logos_rust_sdk::bytes::encode(&{})", expr),
        (TypeKind::Primitive, "result") => format!(
            "match {} {{ Ok(v) => serde_json::json!({{\"success\": true, \"value\": v, \"error\": null}}), \
             Err(e) => serde_json::json!({{\"success\": false, \"value\": null, \"error\": e}}) }}",
            expr
        ),
        (TypeKind::Named, n) if recs.contains(n) => format!("{}.to_json()", expr),
        (TypeKind::Array, _)
            if ty.elements.len() == 1 && crate::rustgen::is_record(&ty.elements[0], recs) =>
        {
            format!(
                "serde_json::Value::Array({}.iter().map(|__e| __e.to_json()).collect())",
                expr
            )
        }
        // Both edits are required together: serde_json has
        // `impl From<()> for Value` producing Value::Null, so changing only the
        // signature to `()` would still emit null and change nothing.
        // `true` matches what the C++ backend puts on the C ABI.
        _ if is_void(ty) => format!("{{ {}; serde_json::Value::Bool(true) }}", expr),
        _ => format!("serde_json::Value::from({})", expr),
    }
}

/// The borrowed Rust type one event parameter is emitted with. `?T` wraps that
/// borrow in `Option`, so an emitter spells "no value" the one way Rust has.
///
/// Takes `recs` for the same reason `rust_param_type` does: a record the
/// contract DECLARES is a real struct on this side too. Without it every
/// non-primitive collapsed to `&serde_json::Value`, so a module whose methods
/// took `Point` had to hand-build a JSON object to emit `moved(from: Point)` —
/// a typed API with one untyped hole in it.
fn emit_param_type(ty: &TypeExpr, recs: &BTreeSet<String>) -> String {
    if is_optional(ty) {
        return format!("Option<{}>", emit_param_type(ty.value_type(), recs));
    }
    if crate::rustgen::is_record(ty, recs) {
        return format!("&{}", crate::rustgen::owned_type(ty, recs));
    }
    // `[Record]` borrows as a slice — the emitter never needs to own it.
    if ty.kind == TypeKind::Array
        && ty.elements.len() == 1
        && crate::rustgen::is_record(&ty.elements[0], recs)
    {
        return format!("&[{}]", crate::rustgen::owned_type(&ty.elements[0], recs));
    }
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "tstr") => "&str".into(),
        (TypeKind::Primitive, "bstr") => "&[u8]".into(),
        (TypeKind::Primitive, "int") => "i64".into(),
        (TypeKind::Primitive, "uint") => "u64".into(),
        (TypeKind::Primitive, "float64") => "f64".into(),
        (TypeKind::Primitive, "bool") => "bool".into(),
        _ => "&serde_json::Value".into(),
    }
}

/// The JSON value one event argument contributes to the payload array. An
/// event parameter is a POSITIONAL slot, so an empty `?T` is `null` and the
/// payload keeps its arity.
fn emit_param_value(ty: &TypeExpr, name: &str, recs: &BTreeSet<String>) -> String {
    if is_optional(ty) {
        return format!(
            "match {} {{ Some(__o) => {}, None => serde_json::Value::Null }}",
            name,
            emit_param_value(ty.value_type(), "__o", recs)
        );
    }
    // A record carries its own encoder — the same `to_json` the dispatch uses
    // for a record RETURN, so an event payload and a method result serialize a
    // Point identically.
    if crate::rustgen::is_record(ty, recs) {
        return format!("{}.to_json()", name);
    }
    if ty.kind == TypeKind::Array
        && ty.elements.len() == 1
        && crate::rustgen::is_record(&ty.elements[0], recs)
    {
        return format!(
            "serde_json::Value::Array({}.iter().map(|__e| __e.to_json()).collect())",
            name
        );
    }
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "bstr") => format!("logos_rust_sdk::bytes::encode({})", name),
        // `any` is passed as `&serde_json::Value` (like Array/Map), so it must
        // be cloned, not fed to `Value::from` (no `From<&Value>`). The
        // remaining primitives (int/uint/float64/bool by value, tstr as &str)
        // do convert via `Value::from`.
        (TypeKind::Primitive, "any") => format!("{}.clone()", name),
        (TypeKind::Primitive, _) => format!("serde_json::Value::from({})", name),
        _ => format!("{}.clone()", name),
    }
}

fn interface_json(module: &ModuleDecl) -> serde_json::Value {
    let mut entries = Vec::new();
    for m in &module.methods {
        let sig = format!(
            "{}({})",
            m.name,
            m.params.iter().map(|p| qt_type_name(&p.ty)).collect::<Vec<_>>().join(",")
        );
        let mut obj = serde_json::json!({
            "name": m.name,
            "signature": sig,
            "returnType": qt_type_name(&m.return_type),
            "isInvokable": true,
        });
        if !m.params.is_empty() {
            obj["parameters"] = serde_json::Value::Array(
                m.params
                    .iter()
                    .map(|p| serde_json::json!({"type": qt_type_name(&p.ty), "name": p.name}))
                    .collect(),
            );
        }
        entries.push(obj);
    }
    for e in &module.events {
        let sig = format!(
            "{}({})",
            e.name,
            e.params.iter().map(|p| qt_type_name(&p.ty)).collect::<Vec<_>>().join(",")
        );
        let mut obj = serde_json::json!({
            "type": "event",
            "name": e.name,
            "signature": sig,
        });
        if !e.params.is_empty() {
            obj["parameters"] = serde_json::Value::Array(
                e.params
                    .iter()
                    .map(|p| serde_json::json!({"type": qt_type_name(&p.ty), "name": p.name}))
                    .collect(),
            );
        }
        entries.push(obj);
    }
    serde_json::Value::Array(entries)
}

// concurrency:"multi" instance + install block (replaces the single-mode one).
// `__TRAIT__` is substituted with the contract trait name. The instance is a
// shared `Arc<dyn Any + Send + Sync>`; the INSTANCE mutex guards construction
// only, so dispatch clones the Arc and runs the handler on `&self` with no lock
// held — calls to one module overlap. Ends at `match method {` so the shared
// per-method arms (and the closing block) append exactly as for single mode.
const MULTI_INSTALL_BLOCK: &str = r##"type DispatchFn = fn(&str, &[serde_json::Value]) -> Option<serde_json::Value>;
type EnsureFn = fn(bool);
struct Registered {
    dispatch: DispatchFn,
    ensure: EnsureFn,
}
static REGISTERED: Mutex<Option<Registered>> = Mutex::new(None);
// Shared across worker threads; the mutex guards CONSTRUCTION only.
static INSTANCE: Mutex<Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>> = Mutex::new(None);
static HOOK_FIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Install `T` as the module implementation (Default-constructed once).
///
/// `on_context_ready` fires AT MODULE LOAD, before the module is published for
/// inbound calls — single-threaded at that point, so it matches the single-mode
/// scaffold's "fires once before the first dispatch" contract.
pub fn install<T: __TRAIT__ + Default>() {
    fn ensure_impl<T: __TRAIT__ + Default>(require_emit: bool) {
        let inst = {
            let mut guard = INSTANCE.lock().unwrap();
            if guard.is_none() {
                *guard = Some(std::sync::Arc::new(T::default()));
            }
            guard.clone()
        };
        if HOOK_FIRED.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let Some(ctx) = context() else { return; };
        if require_emit && EMIT.lock().unwrap().cb.is_none() {
            return;
        }
        if let Some(inst) = inst {
            if let Ok(imp) = inst.downcast::<T>() {
                HOOK_FIRED.store(true, std::sync::atomic::Ordering::SeqCst);
                imp.on_context_ready(&ctx);
            }
        }
    }
    fn dispatch_impl<T: __TRAIT__ + Default>(method: &str, args: &[serde_json::Value]) -> Option<serde_json::Value> {
        let inst = {
            let mut guard = INSTANCE.lock().unwrap();
            if guard.is_none() {
                *guard = Some(std::sync::Arc::new(T::default()));
            }
            guard.clone()
        };
        let imp: std::sync::Arc<T> = inst?.downcast::<T>().ok()?;
        match method {
"##;

/// Generate the Rust provider scaffold. `protocol_version` is the
/// logos-protocol semver this module is built against (stamped by the
/// build, surfaced through logos_module_get_protocol_version).
pub fn generate_provider(module: &ModuleDecl, protocol_version: &str) -> String {
    generate_provider_with(module, protocol_version, true, false)
}

/// Like [`generate_provider`], with control over trait emission and dispatch
/// concurrency. Pass `emit_trait = false` for the Rust-declared
/// (contract-in-language) flow: the author hand-writes the trait — including the
/// defaulted `on_context_ready` hook — and the scaffold only adds context,
/// emitters, dispatch and the C ABI exports around it (see rust_frontend).
///
/// `multi = true` emits the `concurrency: "multi"` scaffold: the instance is
/// shared (`Arc<T>`, `T: Send + Sync`), methods take `&self`, the instance mutex
/// guards CONSTRUCTION only, and an extra `logos_module_dispatch_async` C export
/// runs each handler on its own worker thread so calls to one module overlap.
/// The author owns thread-safety (interior mutability). `multi = false`
/// (default) is the single-threaded event-loop scaffold (`&mut self`, the mutex
/// held across the handler).
pub fn generate_provider_with(
    module: &ModuleDecl,
    protocol_version: &str,
    emit_trait: bool,
    multi: bool,
) -> String {
    let recs = crate::rustgen::record_names(module);
    let pascal_name = pascal(&module.name);
    // sdk_test_provider_module -> SdkTestProviderModule, not ...ModuleModule
    let trait_name = if pascal_name.ends_with("Module") {
        pascal_name
    } else {
        format!("{}Module", pascal_name)
    };
    // concurrency:"multi" — methods take &self (shared; the author uses interior
    // mutability) and the trait is Sync; "single" — &mut self (exclusive, no
    // author-side locking).
    let self_recv = if multi { "&self" } else { "&mut self" };
    // "multi" must be Send + Sync (shared across worker threads). "single" runs
    // entirely on the module subprocess's one event-loop thread, so its impl need
    // not be Send — only `'static` — which lets single-mode modules hold
    // non-Send state (e.g. an engine whose trait objects aren't Send). The
    // single-mode INSTANCE static below is made `Sync` via a single-threaded
    // safety wrapper accordingly.
    let trait_bounds = if multi { "Send + Sync + 'static" } else { "'static" };
    let mut out = String::new();

    out.push_str(&format!(
        "// Generated by logos-lidl-gen --provider from the `{}` LIDL contract — do not edit.\n\
         //\n\
         // The common module-impl C ABI (logos_module_impl.h) around a Rust impl:\n\
         // implement `{}` on your type and define the install hook:\n\
         //\n\
         //     #[no_mangle]\n\
         //     pub extern \"Rust\" fn logos_module_install() {{ install::<YourImpl>(); }}\n\
         //\n\
         // The first dispatch invokes it lazily — a plain symbol reference, so\n\
         // no ctor/init-section tricks are needed for static linking.\n\n\
         use std::ffi::{{c_char, c_int, c_void, CStr, CString}};\n\
         use std::sync::Mutex;\n\n",
        module.name, trait_name
    ));

    // -- context ------------------------------------------------------------
    out.push_str(
        "#[derive(Debug, Clone, Default)]\n\
         pub struct RustModuleContext {\n\
         \x20   pub module_path: String,\n\
         \x20   pub instance_id: String,\n\
         \x20   pub instance_persistence_path: String,\n\
         }\n\n\
         static CONTEXT: Mutex<Option<RustModuleContext>> = Mutex::new(None);\n\n\
         /// The module identity/context stamped by the host (None before\n\
         /// set_context — mirrors LogosModuleContext::isContextReady()).\n\
         pub fn context() -> Option<RustModuleContext> {\n\
         \x20   CONTEXT.lock().unwrap().clone()\n\
         }\n\n",
    );

    // -- emit callback + typed emitters --------------------------------------
    out.push_str(
        "type EmitCb = extern \"C\" fn(*const c_char, *const c_char, *mut c_void);\n\
         struct EmitState {\n\
         \x20   cb: Option<(EmitCb, usize)>,\n\
         }\n\
         unsafe impl Send for EmitState {}\n\
         static EMIT: Mutex<EmitState> = Mutex::new(EmitState { cb: None });\n\n\
         fn emit_event(name: &str, payload: &serde_json::Value) {\n\
         \x20   let state = EMIT.lock().unwrap();\n\
         \x20   if let Some((cb, ud)) = state.cb {\n\
         \x20       let name_c = CString::new(name).unwrap_or_default();\n\
         \x20       let json_c = CString::new(payload.to_string()).unwrap_or_default();\n\
         \x20       cb(name_c.as_ptr(), json_c.as_ptr(), ud as *mut c_void);\n\
         \x20   }\n\
         }\n\n",
    );

    for e in &module.events {
        let fn_name = format!("emit_{}", snake(&e.name));
        let params_sig: Vec<String> = e
            .params
            .iter()
            .map(|p| format!("{}: {}", snake(&p.name), emit_param_type(&p.ty, &recs)))
            .collect();
        // The accumulator is named `__logos_args`, not `payload`: an event
        // parameter is free to be called `payload` (delivery_module's
        // messageReceived(..., payload: bstr, ...) does exactly that), and a
        // plainly-named accumulator would shadow it — silently emitting the
        // accumulator instead of the argument for scalars, and failing to
        // compile for a bstr param (bytes::encode(&[u8]) handed a Vec). The
        // `__logos_` prefix is this crate's reserved-internal convention.
        let pushes: Vec<String> = e
            .params
            .iter()
            .map(|p| {
                format!("__logos_args.push({});", emit_param_value(&p.ty, &snake(&p.name), &recs))
            })
            .collect();
        out.push_str(&format!(
            "/// Typed emitter for the `{}` event.\n\
             pub fn {}({}) {{\n\
             \x20   let mut __logos_args: Vec<serde_json::Value> = Vec::new();\n\
             \x20   {}\n\
             \x20   emit_event(\"{}\", &serde_json::Value::Array(__logos_args));\n\
             }}\n\n",
            e.name,
            fn_name,
            params_sig.join(", "),
            pushes.join("\n    "),
            e.name
        ));
    }

    // -- the trait ------------------------------------------------------------
    if emit_trait {
        // The trait signatures name the record structs, so emit them here too.
        // Same emitter the client generator uses, so both sides agree field for
        // field and a bstr field rides the tagged form in both.
        out.push_str(&crate::rustgen::emit_records(module));
        out.push_str(&format!("pub trait {}: {} {{\n", trait_name, trait_bounds));
        out.push_str(&format!(
            "    /// One-time setup hook: fires after the host has stamped the module\n\
             \x20   /// context (path / instance id / persistence path) and before the\n\
             \x20   /// first method dispatch — the Rust analog of C++'s\n\
             \x20   /// LogosModuleContext::onContextReady().\n\
             \x20   fn on_context_ready({}, _ctx: &RustModuleContext) {{}}\n\n",
            self_recv,
        ));
        for m in &module.methods {
            let params: Vec<String> = m
                .params
                .iter()
                .map(|p| format!("{}: {}", snake(&p.name), rust_param_type(&p.ty, &recs)))
                .collect();
            let ret = rust_return_type(&m.return_type, &recs);
            out.push_str(&format!(
                "    fn {}({}{}{}){};\n",
                snake(&m.name),
                self_recv,
                if params.is_empty() { "" } else { ", " },
                params.join(", "),
                // `-> ()` is legal but nobody writes it; a void method should read
                // like one in the trait the author implements.
                if ret == "()" { String::new() } else { format!(" -> {}", ret) }
            ));
        }
        out.push_str("}\n\n");
    } else {
        out.push_str(&format!(
            "// Contract trait `{}` is author-declared in this crate (Rust-first\n\
             // flow); the scaffold generates everything around it.\n\n",
            trait_name
        ));
    }

    // -- instance + install ----------------------------------------------------
    if multi {
        // concurrency:"multi" — the instance is shared across worker threads. The
        // INSTANCE mutex guards CONSTRUCTION only; each dispatch clones the Arc and
        // runs the handler on `&self` with NO lock held, so calls to one module
        // overlap. Emitted as a raw string (no format! escaping) — only the trait
        // name is substituted.
        out.push_str(&MULTI_INSTALL_BLOCK.replace("__TRAIT__", &trait_name));
    } else {
    out.push_str(&format!(
        "type DispatchFn = fn(&str, &[serde_json::Value]) -> Option<serde_json::Value>;\n\
         type EnsureFn = fn(bool);\n\
         struct Registered {{\n\
         \x20   dispatch: DispatchFn,\n\
         \x20   ensure: EnsureFn,\n\
         }}\n\
         static REGISTERED: Mutex<Option<Registered>> = Mutex::new(None);\n\
         // A concurrency:\"single\" module runs entirely on one thread (its\n\
         // subprocess event loop): install / on_context_ready / dispatch all touch\n\
         // INSTANCE from that thread, so the impl never crosses threads and need\n\
         // not be Send. This single-threaded Sync wrapper lets a non-Send impl\n\
         // live in a static (cf. the EmitState unsafe impl above).\n\
         struct SingleInstance(Mutex<Option<Box<dyn std::any::Any>>>);\n\
         unsafe impl Sync for SingleInstance {{}}\n\
         static INSTANCE: SingleInstance = SingleInstance(Mutex::new(None));\n\
         static HOOK_FIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);\n\n\
         /// Install `T` as the module implementation (Default-constructed once).\n\
         ///\n\
         /// `on_context_ready` fires AT MODULE LOAD — as soon as the host has\n\
         /// delivered both the context (set_context) and the event plumbing\n\
         /// (set_emit_callback), which the glue does during registration,\n\
         /// before the module is published for inbound calls. This matches\n\
         /// C++'s onContextReady-in-onInit semantics: subscriptions, outbound\n\
         /// calls and typed emission all work from the hook without waiting\n\
         /// for a first inbound dispatch. (For hosts that never wire an emit\n\
         /// callback, the hook still fires before the first dispatch.)\n\
         pub fn install<T: {} + Default>() {{\n\
         \x20   fn ensure_impl<T: {} + Default>(require_emit: bool) {{\n\
         \x20       let mut guard = INSTANCE.0.lock().unwrap();\n\
         \x20       if guard.is_none() {{\n\
         \x20           *guard = Some(Box::new(T::default()));\n\
         \x20       }}\n\
         \x20       if HOOK_FIRED.load(std::sync::atomic::Ordering::SeqCst) {{\n\
         \x20           return;\n\
         \x20       }}\n\
         \x20       let Some(ctx) = context() else {{ return; }};\n\
         \x20       if require_emit && EMIT.lock().unwrap().cb.is_none() {{\n\
         \x20           return;\n\
         \x20       }}\n\
         \x20       if let Some(imp) = guard.as_mut().unwrap().downcast_mut::<T>() {{\n\
         \x20           HOOK_FIRED.store(true, std::sync::atomic::Ordering::SeqCst);\n\
         \x20           imp.on_context_ready(&ctx);\n\
         \x20       }}\n\
         \x20   }}\n\
         \x20   fn dispatch_impl<T: {} + Default>(method: &str, args: &[serde_json::Value]) -> Option<serde_json::Value> {{\n\
         \x20       let mut guard = INSTANCE.0.lock().unwrap();\n\
         \x20       if guard.is_none() {{\n\
         \x20           *guard = Some(Box::new(T::default()));\n\
         \x20       }}\n\
         \x20       let imp: &mut T = guard.as_mut().unwrap().downcast_mut::<T>()?;\n\
         \x20       match method {{\n",
        trait_name, trait_name, trait_name
    ));
    }

    for m in &module.methods {
        // Only the REQUIRED prefix of the parameter list has to be present. A
        // trailing `?T` may arrive as null or not at all: absent and null are
        // the same empty state on decode, and `args::get` already reads a
        // missing slot as null. Callers still SEND the full arity — encode is
        // canonical, decode is liberal.
        let n = crate::rustgen::required_arity(&m.params);
        // Each parameter becomes a fallible let-binding, so a wrong-typed
        // argument returns the canonical dispatch_failed object instead of
        // silently substituting 0 / "" / false / empty bytes and running the
        // author's method on it. Matches what the C++ glue does when
        // logos::CodecError propagates out of a decode.
        let mut bindings = String::new();
        let mut idents: Vec<String> = Vec::new();
        for (i, p) in m.params.iter().enumerate() {
            let (accessor, fallible) = arg_accessor(&p.ty, i, module);
            let record_decode = record_decode_for(&p.ty, &recs);
            let ident = format!("__logos_a{}", i);
            if fallible {
                bindings.push_str(&format!(
                    "                let {} = match {} {{ Ok(v) => v, Err(e) => return Some(logos_rust_sdk::args::dispatch_failed(\"{}\", &e)) }};\n",
                    ident, accessor, module.name
                ));
                // as_value_checked validated the shape and reported any bad
                // field by name; this turns the validated value into the struct.
                if let Some(dec) = &record_decode {
                    bindings.push_str(&format!(
                        "                let {} = match {} {{ Some(v) => v, None => return Some(logos_rust_sdk::args::dispatch_failed(\"{}\", \"arg{}: malformed record\")) }};\n",
                        ident,
                        dec.replace("__V__", &ident),
                        module.name,
                        i
                    ));
                } else if needs_optional_lift(&p.ty, &recs) {
                    // The value was validated as `?T` above, so null here is the
                    // empty state and nothing else needs checking — just give it
                    // the shape the trait declares.
                    bindings.push_str(&format!(
                        "                let {0} = if {0}.is_null() {{ None }} else {{ Some({0}) }};\n",
                        ident
                    ));
                }
            } else {
                bindings.push_str(&format!("                let {} = {};\n", ident, accessor));
            }
            idents.push(ident);
        }
        let args: Vec<String> = idents;
        // Only guard the arg count when the method actually takes parameters —
        // `if args.len() < 0` is a dead check on a zero-arg method.
        let guard = if n > 0 {
            format!(
                "                if args.len() < {} {{ return Some(logos_rust_sdk::args::invalid_args(\"{}\", {}, args.len())); }}\n",
                n, module.name, n
            )
        } else {
            String::new()
        };
        out.push_str(&format!(
            "            \"{}\" => {{\n\
             {}\
             {}\
             \x20               let result = imp.{}({});\n\
             \x20               Some({})\n\
             \x20           }}\n",
            m.name,
            guard,
            bindings,
            snake(&m.name),
            args.join(", "),
            ret_to_json(&m.return_type, "result", &recs)
        ));
    }

    out.push_str(
        "            _ => None,\n\
         \x20       }\n\
         \x20   }\n\
         \x20   *REGISTERED.lock().unwrap() = Some(Registered {\n\
         \x20       dispatch: dispatch_impl::<T>,\n\
         \x20       ensure: ensure_impl::<T>,\n\
         \x20   });\n\
         }\n\n\
         /// Run the author's install hook (once) and give the ready-latch a\n\
         /// chance to fire `on_context_ready`. Called from every C-ABI entry\n\
         /// point: set_context / set_emit_callback latch on full wiring;\n\
         /// dispatch passes require_emit = false as the no-event-host fallback.\n\
         fn ensure_ready(require_emit: bool) {\n\
         \x20   if REGISTERED.lock().unwrap().is_none() {\n\
         \x20       unsafe { __logos_install_hook::logos_module_install() };\n\
         \x20   }\n\
         \x20   let ensure = REGISTERED.lock().unwrap().as_ref().map(|r| r.ensure);\n\
         \x20   if let Some(f) = ensure {\n\
         \x20       f(require_emit);\n\
         \x20   }\n\
         }\n\n\
         mod __logos_install_hook {\n\
         \x20   // Defined by the module author (a #[no_mangle] fn at crate root):\n\
         \x20   // call `install::<YourImpl>()` once. Scoped inside a module so the\n\
         \x20   // declaration and the author's definition don't collide by path;\n\
         \x20   // they meet at the symbol level. Referenced (not just expected) so\n\
         \x20   // static linking always pulls the author's object in — invoked\n\
         \x20   // lazily on first dispatch.\n\
         \x20   extern \"Rust\" {\n\
         \x20       pub(super) fn logos_module_install();\n\
         \x20   }\n\
         }\n\n",
    );

    // -- C ABI exports -----------------------------------------------------------
    let iface = interface_json(module).to_string().replace('\\', "\\\\").replace('"', "\\\"");
    out.push_str(&format!(
        "fn to_c_string(s: String) -> *mut c_char {{\n\
         \x20   CString::new(s).map(CString::into_raw).unwrap_or(std::ptr::null_mut())\n\
         }}\n\n\
         #[no_mangle]\n\
         pub extern \"C\" fn logos_module_dispatch(method: *const c_char, args_json: *const c_char) -> *mut c_char {{\n\
         \x20   if method.is_null() {{ return std::ptr::null_mut(); }}\n\
         \x20   let method = unsafe {{ CStr::from_ptr(method) }}.to_string_lossy().into_owned();\n\
         \x20   let args: Vec<serde_json::Value> = if args_json.is_null() {{\n\
         \x20       Vec::new()\n\
         \x20   }} else {{\n\
         \x20       let raw = unsafe {{ CStr::from_ptr(args_json) }}.to_string_lossy();\n\
         \x20       match serde_json::from_str::<serde_json::Value>(&raw) {{\n\
         \x20           Ok(serde_json::Value::Array(a)) => a,\n\
         \x20           _ => return std::ptr::null_mut(),\n\
         \x20       }}\n\
         \x20   }};\n\
         \x20   ensure_ready(false);\n\
         \x20   // Copy the dispatch fn pointer out and RELEASE the REGISTERED\n\
         \x20   // lock BEFORE running the handler. A concurrency:\"multi\" module's\n\
         \x20   // glue calls this from worker threads; holding the mutex across\n\
         \x20   // the handler would serialize every call (peak overlap 1).\n\
         \x20   // dispatch_impl resolves the instance internally (a cloned Arc in\n\
         \x20   // multi mode, the boxed instance behind the SingleInstance mutex in\n\
         \x20   // single mode) without holding REGISTERED, so dropping the lock here\n\
         \x20   // first is safe. Same release-before-call shape as ensure_ready.\n\
         \x20   let dispatch = match REGISTERED.lock().unwrap().as_ref() {{ Some(r) => r.dispatch, None => return std::ptr::null_mut() }};\n\
         \x20   match dispatch(&method, &args) {{\n\
         \x20       Some(value) => to_c_string(value.to_string()),\n\
         \x20       None => std::ptr::null_mut(),\n\
         \x20   }}\n\
         }}\n\n\
         #[no_mangle]\n\
         pub extern \"C\" fn logos_module_get_methods() -> *mut c_char {{\n\
         \x20   to_c_string(\"{}\".to_string())\n\
         }}\n\n\
         #[no_mangle]\n\
         pub extern \"C\" fn logos_module_set_context(\n\
         \x20   module_path: *const c_char,\n\
         \x20   instance_id: *const c_char,\n\
         \x20   instance_persistence_path: *const c_char,\n\
         ) {{\n\
         \x20   fn s(p: *const c_char) -> String {{\n\
         \x20       if p.is_null() {{ String::new() }} else {{ unsafe {{ CStr::from_ptr(p) }}.to_string_lossy().into_owned() }}\n\
         \x20   }}\n\
         \x20   *CONTEXT.lock().unwrap() = Some(RustModuleContext {{\n\
         \x20       module_path: s(module_path),\n\
         \x20       instance_id: s(instance_id),\n\
         \x20       instance_persistence_path: s(instance_persistence_path),\n\
         \x20   }});\n\
         \x20   ensure_ready(true);\n\
         }}\n\n\
         #[no_mangle]\n\
         pub extern \"C\" fn logos_module_set_emit_callback(cb: Option<EmitCb>, user_data: *mut c_void) {{\n\
         \x20   EMIT.lock().unwrap().cb = cb.map(|f| (f, user_data as usize));\n\
         \x20   ensure_ready(true);\n\
         }}\n\n\
         static TOKENS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());\n\n\
         #[no_mangle]\n\
         pub extern \"C\" fn logos_module_accept_token(module_name: *const c_char, token: *const c_char) -> c_int {{\n\
         \x20   if module_name.is_null() || token.is_null() {{ return -1; }}\n\
         \x20   let name = unsafe {{ CStr::from_ptr(module_name) }}.to_string_lossy().into_owned();\n\
         \x20   let tok = unsafe {{ CStr::from_ptr(token) }}.to_string_lossy().into_owned();\n\
         \x20   // The runtime handshake: hand the host-issued token to the SDK's\n\
         \x20   // protocol stack so this module's *outbound* calls authenticate —\n\
         \x20   // the same stack the typed client wrappers invoke through.\n\
         \x20   logos_rust_sdk::save_token(&name, &tok);\n\
         \x20   TOKENS.lock().unwrap().push((name, tok));\n\
         \x20   0\n\
         }}\n\n\
         /// The logos-protocol semver this module was built against\n\
         /// (stamped at generation time by the build; never minted here).\n\
         #[no_mangle]\n\
         pub extern \"C\" fn logos_module_get_protocol_version() -> *const c_char {{\n\
         \x20   static VERSION: &str = \"{}\\0\";\n\
         \x20   VERSION.as_ptr() as *const c_char\n\
         }}\n\n\
         #[no_mangle]\n\
         pub extern \"C\" fn logos_module_string_free(s: *mut c_char) {{\n\
         \x20   if !s.is_null() {{\n\
         \x20       unsafe {{ drop(CString::from_raw(s)) }};\n\
         \x20   }}\n\
         }}\n",
        iface, protocol_version
    ));

    // concurrency:"multi" needs NO extra C ABI here. The sync
    // logos_module_dispatch above is already safe to call CONCURRENTLY in multi
    // mode (the shared Arc instance is cloned per call and no lock is held across
    // the handler — see MULTI_INSTALL_BLOCK). The Qt glue is what spawns a worker
    // per call and pushes the deferred completion event over the existing channel;
    // this cdylib just serves those concurrent dispatch calls. The provider/host
    // ABI is unchanged — an old host loads and forwards a multi module unmodified.

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    const SAMPLE: &str = r#"
module rust_calc {
  version "1.0.0"
  depends []
  method add(a: int, b: int) -> int
  method greet(name: tstr) -> tstr
  method fetch() -> result
  event totalChanged(total: int)
}
"#;

    #[test]
    fn generates_provider_scaffold() {
        let m = parse(SAMPLE).unwrap();
        let code = generate_provider(&m, "0.1.0");
        assert!(code.contains("pub trait RustCalcModule"));
        assert!(code.contains("fn add(&mut self, a: i64, b: i64) -> i64;"));
        assert!(code.contains("fn fetch(&mut self) -> Result<serde_json::Value, String>;"));
        assert!(code.contains("pub fn emit_total_changed(total: i64)"));
        assert!(code.contains("logos_module_dispatch"));
        assert!(code.contains("logos_module_get_protocol_version"));
        assert!(code.contains("\"0.1.0\\0\""));
        assert!(code.contains("logos_module_set_context"));
        // accept_token forwards into the SDK's protocol stack (the runtime
        // handshake that authenticates this module's outbound calls)
        assert!(code.contains("logos_rust_sdk::save_token(&name, &tok)"));
        // first dispatch lazily invokes the author's install hook
        assert!(code.contains("fn logos_module_install()"));
        assert!(code.contains("unsafe { __logos_install_hook::logos_module_install() }"));
        // onContextReady parity: defaulted trait hook, fired once after
        // construction (context is stamped before any dispatch)
        assert!(code.contains("fn on_context_ready(&mut self, _ctx: &RustModuleContext) {}"));
        assert!(code.contains("imp.on_context_ready(&ctx);"));
        // interface JSON carries Qt-style names + tagged events for host parity
        assert!(code.contains("QString"));
        assert!(code.contains("totalChanged"));
    }

    // A binary EVENT payload, EMIT side. The typed emitter must take the bstr
    // param as &[u8] and run it through the canonical tagged-bytes encoder — the
    // provider analog of the consumer decode covered in rustgen.rs. Uses the
    // exact shape of delivery_module's messageReceived(..., payload: bstr, ...),
    // including a bstr param literally named `payload`, which also guards the
    // accumulator-shadowing regression below.
    #[test]
    fn emits_binary_event_payload() {
        let m = parse(
            "module delivery_module {\n  \
             version \"1.0.0\"\n  depends []\n  \
             event messageReceived(message_hash: tstr, payload: bstr, timestamp: int)\n\
             }",
        )
        .unwrap();
        let code = generate_provider(&m, "0.1.0");
        // The bstr param is borrowed as &[u8] and encoded via the tagged codec;
        // scalars are pushed as-is.
        assert!(code.contains(
            "pub fn emit_message_received(message_hash: &str, payload: &[u8], timestamp: i64)"
        ));
        assert!(code.contains("logos_rust_sdk::bytes::encode(payload)"));
        assert!(code.contains("serde_json::Value::from(message_hash)"));
        assert!(code.contains("serde_json::Value::from(timestamp)"));
    }

    // Regression: the emitter's local accumulator must NOT be named after a
    // value a user could pick for a parameter. `payload` is the obvious one
    // (delivery_module uses it). If the accumulator were `payload`, it would
    // shadow the &[u8] argument — `bytes::encode(payload)` would then be handed
    // the Vec<serde_json::Value> accumulator and fail to compile (and a scalar
    // `payload` would silently emit the accumulator instead of the value).
    #[test]
    fn event_emitter_accumulator_does_not_shadow_a_payload_param() {
        let m = parse(
            "module m {\n  version \"1.0.0\"\n  depends []\n  \
             event ev(payload: bstr)\n}",
        )
        .unwrap();
        let code = generate_provider(&m, "0.1.0");
        // The argument reaches the encoder untouched...
        assert!(code.contains("logos_rust_sdk::bytes::encode(payload)"));
        // ...because the accumulator is a reserved-internal name, not `payload`.
        assert!(!code.contains("let mut payload"));
        assert!(code.contains("let mut __logos_args"));
    }

    #[test]
    fn trait_name_not_doubled_for_module_suffix() {
        let m = parse(
            "module sdk_test_provider_module {\n  version \"0.1.0\"\n  method add(a: int, b: int) -> int\n}\n",
        )
        .unwrap();
        let code = generate_provider(&m, "0.1.0");
        assert!(code.contains("pub trait SdkTestProviderModule:"));
        assert!(!code.contains("SdkTestProviderModuleModule"));
    }

    #[test]
    fn multi_mode_emits_shared_self_for_concurrent_dispatch() {
        let m = parse(SAMPLE).unwrap();
        let code = generate_provider_with(&m, "0.1.0", true, true);
        // multi contract: &self receivers + Send + Sync trait bound — the
        // compile-time guarantee that lets logos_module_dispatch be called
        // CONCURRENTLY (the Qt glue spawns a worker per call).
        assert!(code.contains("pub trait RustCalcModule: Send + Sync + 'static"));
        assert!(code.contains("fn add(&self, a: i64, b: i64) -> i64;"));
        assert!(code.contains("fn on_context_ready(&self, _ctx: &RustModuleContext)"));
        // shared instance, no exclusive borrow held across the handler
        assert!(code.contains("std::sync::Arc<dyn std::any::Any + Send + Sync>"));
        assert!(code.contains("let imp: std::sync::Arc<T> ="));
        assert!(!code.contains("downcast_mut::<T>()"));
        // the shared per-method arm still calls imp.<method> (same as single)
        assert!(code.contains("let result = imp.add("));
        // NO new C ABI: concurrency rides on the existing sync dispatch — the Qt
        // glue defers (sentinel + completion event), not a dispatch_async export.
        // The provider/host ABI is unchanged in both modes.
        assert!(code.contains("pub extern \"C\" fn logos_module_dispatch("));
        assert!(!code.contains("logos_module_dispatch_async"));
        // The C-ABI dispatch must RELEASE the REGISTERED lock before running the
        // handler — otherwise concurrent multi calls (the glue spawns a worker
        // per call) serialize on that mutex and peak overlap collapses to 1. The
        // fn pointer is copied out, the lock drops, THEN it's called.
        assert!(code.contains("let dispatch = match REGISTERED.lock().unwrap().as_ref()"));
        assert!(code.contains("match dispatch(&method, &args)"));
        assert!(!code.contains("(registered.dispatch)(&method, &args)"));

        // single mode: &mut self, a Box instance (not the Arc multi mode uses).
        // The Box is `dyn Any` WITHOUT `+ Send` since #22 lifted Send on
        // single-mode instances (they never leave the subprocess event-loop
        // thread); it lives behind the SingleInstance Sync wrapper.
        let single = generate_provider_with(&m, "0.1.0", true, false);
        assert!(single.contains("fn add(&mut self, a: i64, b: i64) -> i64;"));
        assert!(single.contains("Box<dyn std::any::Any>"));
        assert!(!single.contains("Box<dyn std::any::Any + Send>"));
        assert!(!single.contains("logos_module_dispatch_async"));
        assert!(!single.contains("Send + Sync + 'static"));
    }

    // Records on the PROVIDER side: a Rust module author writes `s: Status`,
    // not a serde_json::Value to pick apart. The dispatch validates the shape
    // (Ty::Record reports arg0.field) and then materialises the struct.
    #[test]
    fn event_emitters_speak_records_too() {
        // The gap this closes: methods took `Status` while the emitter for an
        // event carrying the SAME record still took `&serde_json::Value`, so a
        // typed API had one untyped hole and the author had to hand-build the
        // JSON object to fire an event.
        let src = r#"
module info_module {
  version "1.0.0"
  depends []
  type Status {
    port: uint
    blob: bstr
  }
  method describeStatus(s: Status) -> tstr
  event statusChanged(s: Status, history: [Status], previous: ?Status, note: tstr)
}
"#;
        let m = crate::parse(src).expect("parse");
        let code = generate_provider(&m, "0.2.0");

        // Borrowed on the emitter — it never needs to own the payload.
        assert!(
            code.contains(
                "pub fn emit_status_changed(s: &Status, history: &[Status], \
                 previous: Option<&Status>, note: &str)"
            ),
            "{}",
            code
        );
        // Encoded by the record's OWN to_json — the same encoder a record
        // RETURN uses, so an event payload and a method result serialize a
        // Status identically.
        assert!(code.contains("__logos_args.push(s.to_json());"), "{}", code);
        assert!(
            code.contains("history.iter().map(|__e| __e.to_json()).collect()"),
            "{}",
            code
        );
        // An empty `?Status` is still null, and the payload keeps its arity.
        assert!(
            code.contains("match previous { Some(__o) => __o.to_json(), None => serde_json::Value::Null }"),
            "{}",
            code
        );
        // A non-record parameter in the same signature is untouched.
        assert!(
            code.contains("__logos_args.push(serde_json::Value::from(note));"),
            "{}",
            code
        );
    }

    #[test]
    fn provider_speaks_records() {
        let src = r#"
module info_module {
  version "1.0.0"
  depends []
  type Status {
    port: uint
    blob: bstr
  }
  method describeStatus(s: Status) -> tstr
  method makeStatuses() -> [Status]
}
"#;
        let m = crate::parse(src).expect("parse");
        let code = generate_provider(&m, "0.2.0");

        // The struct is declared here too — the trait signatures name it.
        assert!(code.contains("pub struct Status"), "{}", code);
        // Typed trait, in and out, including inside a container.
        assert!(code.contains("fn describe_status(&mut self, s: Status) -> String;"), "{}", code);
        assert!(code.contains("fn make_statuses(&mut self) -> Vec<Status>;"), "{}", code);
        // Validated first (field paths), then decoded into the struct.
        assert!(code.contains("Ty::Record(&[(\"port\""), "{}", code);
        assert!(code.contains("Status::from_json(&__logos_a0)"), "{}", code);
        // Returns encode through the record's own to_json.
        assert!(code.contains("__e.to_json()"), "{}", code);
    }

    // Optionality on the PROVIDER side. `?T` is two-state, so the author's
    // trait speaks `Option<T>` — Rust's one empty inhabitant — and the dispatch
    // validates against Ty::Opt, which accepts the empty state and nothing else
    // extra.
    #[test]
    fn provider_speaks_optionals() {
        let src = r#"
module opt_module {
  version "1.0.0"
  depends []
  type Account {
    id: tstr
    ? label: tstr
    note: ?bstr
  }
  method find(id: ?tstr) -> ?Account
  method rename(id: tstr, label: ?tstr) -> bool
  method describe(a: Account) -> tstr
  method tagsOf(tags: ?[tstr]) -> bool
  event changed(id: tstr, label: ?tstr)
}
"#;
        let m = crate::parse(src).expect("parse");
        let code = generate_provider(&m, "0.2.0");

        // The trait the author implements: Option<T> in and out.
        assert!(code.contains("fn find(&mut self, id: Option<String>) -> Option<Account>;"), "{}", code);
        assert!(
            code.contains("fn rename(&mut self, id: String, label: Option<String>) -> bool;"),
            "{}",
            code
        );
        // The record struct agrees, for BOTH spellings of an optional field.
        assert!(code.contains("pub label: Option<String>,"), "{}", code);
        assert!(code.contains("pub note: Option<Vec<u8>>,"), "{}", code);

        // The argument reads through the as_opt_* twin: absent and null are the
        // same empty state, a present value is still checked against T.
        assert!(
            code.contains("logos_rust_sdk::args::as_opt_string(args, 0)"),
            "{}",
            code
        );
        // A record's optional fields become Ty::Opt in the runtime descriptor —
        // again for both spellings — so a missing key stops being the mismatch
        // it still is for `id`.
        assert!(code.contains("(\"id\", &logos_rust_sdk::args::Ty::Tstr)"), "{}", code);
        assert!(
            code.contains("(\"label\", &logos_rust_sdk::args::Ty::Opt(&logos_rust_sdk::args::Ty::Tstr))"),
            "{}",
            code
        );
        assert!(
            code.contains("(\"note\", &logos_rust_sdk::args::Ty::Opt(&logos_rust_sdk::args::Ty::Bstr))"),
            "{}",
            code
        );

        // A return is positional: empty is null, never a missing value.
        assert!(
            code.contains("Some(match result { Some(__o) => __o.to_json(), None => serde_json::Value::Null })"),
            "{}",
            code
        );

        // Arity: only the REQUIRED prefix is demanded. `find`'s one parameter is
        // optional, so its arm carries no guard at all — the binding follows the
        // arm directly...
        assert!(
            code.contains(
                "\"find\" => {\n                let __logos_a0 = match logos_rust_sdk::args::as_opt_string(args, 0)"
            ),
            "an all-optional method must not demand arguments:\n{}",
            code
        );
        // ...while `rename` still demands its one REQUIRED parameter, and lets
        // the trailing optional be omitted rather than insisting on 2.
        assert!(
            code.contains(
                "\"rename\" => {\n                if args.len() < 1 { return Some(logos_rust_sdk::args::invalid_args(\"opt_module\", 1, args.len())); }"
            ),
            "{}",
            code
        );
        assert!(
            !code.contains("invalid_args(\"opt_module\", 2, args.len())"),
            "a trailing optional must not count toward the minimum arity:\n{}",
            code
        );

        // An optional composite that stays UNTYPED still has to reach the trait
        // as the `Option<Value>` the signature declares — `as_value_checked`
        // hands back the raw value, whose null is the empty state. Emitting the
        // Option in the signature without this lift does not compile.
        assert!(
            code.contains("fn tags_of(&mut self, tags: Option<serde_json::Value>) -> bool;"),
            "{}",
            code
        );
        assert!(
            code.contains(
                "let __logos_a0 = if __logos_a0.is_null() { None } else { Some(__logos_a0) };"
            ),
            "an optional untyped composite must be lifted into Option:\n{}",
            code
        );

        // The typed emitter takes Option and spells empty as null, keeping the
        // payload's arity.
        assert!(code.contains("pub fn emit_changed(id: &str, label: Option<&str>)"), "{}", code);
        assert!(
            code.contains(
                "__logos_args.push(match label { Some(__o) => serde_json::Value::from(__o), None => serde_json::Value::Null });"
            ),
            "{}",
            code
        );
    }

    // `void` is NOT a LIDL builtin — the front end hands it back as
    // Named("void"), exactly like a record name. Treating every Named as a
    // record emitted `fn do_void(&mut self) -> Void;`, a type that does not
    // exist, breaking every provider with a void method (test_fullapi_rust
    // among them). Only a name the contract DECLARES is a record.
    #[test]
    fn void_is_not_a_record() {
        let src = r#"
module v_module {
  version "1.0.0"
  depends []
  type Status {
    port: uint
  }
  method doVoid() -> void
  method takeStatus(s: Status) -> Status
  method takeUndeclared(u: NotDeclared) -> tstr
}
"#;
        let m = crate::parse(src).expect("parse");
        let code = generate_provider(&m, "0.2.0");

        // (a plain contains("Void") would match the method name `doVoid`)
        assert!(!code.contains("-> Void"), "void leaked in as a struct:\n{}", code);
        assert!(!code.contains("struct Void"), "void leaked in as a struct:\n{}", code);
        assert!(!code.contains("Void::from_json"), "void leaked in as a struct:\n{}", code);
        assert!(code.contains("fn do_void(&mut self);"), "{}", code);
        // `()` alone is not enough: serde_json's `impl From<()> for Value` yields
        // Null, so the dispatch must emit the value explicitly.
        assert!(code.contains("serde_json::Value::Bool(true)"), "void must not dispatch to null:\n{}", code);
        // A declared record still gets its struct...
        assert!(code.contains("fn take_status(&mut self, s: Status) -> Status;"), "{}", code);
        // ...and an UNDECLARED Named type keeps the untyped fallback rather
        // than naming a struct nobody emits.
        assert!(code.contains("fn take_undeclared(&mut self, u: serde_json::Value)"), "{}", code);
    }
}
