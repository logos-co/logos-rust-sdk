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

fn rust_param_type(ty: &TypeExpr) -> &'static str {
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "tstr") => "String",
        (TypeKind::Primitive, "int") => "i64",
        (TypeKind::Primitive, "uint") => "u64",
        (TypeKind::Primitive, "float64") => "f64",
        (TypeKind::Primitive, "bool") => "bool",
        (TypeKind::Primitive, "bstr") => "Vec<u8>",
        _ => "serde_json::Value",
    }
}

fn rust_return_type(ty: &TypeExpr) -> &'static str {
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "result") => "Result<serde_json::Value, String>",
        _ => rust_param_type(ty),
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
        (TypeKind::Array, _) => "QVariantList".into(),
        (TypeKind::Map, _) => "QVariantMap".into(),
        _ => "QVariant".into(),
    }
}

fn arg_from_json(ty: &TypeExpr, expr: &str) -> String {
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "tstr") => format!("{}.as_str().unwrap_or_default().to_string()", expr),
        (TypeKind::Primitive, "int") => format!("{}.as_i64().unwrap_or_default()", expr),
        (TypeKind::Primitive, "uint") => format!("{}.as_u64().unwrap_or_default()", expr),
        (TypeKind::Primitive, "float64") => format!("{}.as_f64().unwrap_or_default()", expr),
        (TypeKind::Primitive, "bool") => format!("{}.as_bool().unwrap_or_default()", expr),
        (TypeKind::Primitive, "bstr") => {
            format!("logos_rust_sdk::bytes::decode_lenient({}).unwrap_or_default()", expr)
        }
        _ => format!("{}.clone()", expr),
    }
}

fn ret_to_json(ty: &TypeExpr, expr: &str) -> String {
    match (&ty.kind, ty.name.as_str()) {
        (TypeKind::Primitive, "bstr") => format!("logos_rust_sdk::bytes::encode(&{})", expr),
        (TypeKind::Primitive, "result") => format!(
            "match {} {{ Ok(v) => serde_json::json!({{\"success\": true, \"value\": v, \"error\": null}}), \
             Err(e) => serde_json::json!({{\"success\": false, \"value\": null, \"error\": e}}) }}",
            expr
        ),
        _ => format!("serde_json::Value::from({})", expr),
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
            .map(|p| {
                let t = match (&p.ty.kind, p.ty.name.as_str()) {
                    (TypeKind::Primitive, "tstr") => "&str",
                    (TypeKind::Primitive, "bstr") => "&[u8]",
                    other => match other {
                        (TypeKind::Primitive, "int") => "i64",
                        (TypeKind::Primitive, "uint") => "u64",
                        (TypeKind::Primitive, "float64") => "f64",
                        (TypeKind::Primitive, "bool") => "bool",
                        _ => "&serde_json::Value",
                    },
                };
                format!("{}: {}", snake(&p.name), t)
            })
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
            .map(|p| match (&p.ty.kind, p.ty.name.as_str()) {
                (TypeKind::Primitive, "bstr") => {
                    format!("__logos_args.push(logos_rust_sdk::bytes::encode({}));", snake(&p.name))
                }
                // `any` is passed as `&serde_json::Value` (like Array/Map), so it
                // must be cloned, not fed to `Value::from` (no `From<&Value>`).
                // The remaining primitives (int/uint/float64/bool by value, tstr
                // as &str) do convert via `Value::from`.
                (TypeKind::Primitive, "any") => {
                    format!("__logos_args.push({}.clone());", snake(&p.name))
                }
                (TypeKind::Primitive, _) => {
                    format!("__logos_args.push(serde_json::Value::from({}));", snake(&p.name))
                }
                _ => format!("__logos_args.push({}.clone());", snake(&p.name)),
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
                .map(|p| format!("{}: {}", snake(&p.name), rust_param_type(&p.ty)))
                .collect();
            out.push_str(&format!(
                "    fn {}({}{}{}) -> {};\n",
                snake(&m.name),
                self_recv,
                if params.is_empty() { "" } else { ", " },
                params.join(", "),
                rust_return_type(&m.return_type)
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
        let n = m.params.len();
        let args: Vec<String> = m
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| arg_from_json(&p.ty, &format!("args.get({}).unwrap_or(&serde_json::Value::Null)", i)))
            .collect();
        // Only guard the arg count when the method actually takes parameters —
        // `if args.len() < 0` is a dead check on a zero-arg method.
        let guard = if n > 0 {
            format!("                if args.len() < {} {{ return None; }}\n", n)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "            \"{}\" => {{\n\
             {}\
             \x20               let result = imp.{}({});\n\
             \x20               Some({})\n\
             \x20           }}\n",
            m.name,
            guard,
            snake(&m.name),
            args.join(", "),
            ret_to_json(&m.return_type, "result")
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
}
