//! Entry point for the Logos Module SDK.

use crate::plugin::PluginProxy;

/// The main entry point for calling other Logos modules from within a module.
///
/// No initialization or lifecycle management is required — the underlying
/// `lp_*` protocol clients are created lazily. Simply create an instance and
/// call `plugin()` to get a proxy for any loaded module.
///
/// # Example
/// ```rust,no_run
/// use logos_rust_sdk::LogosModuleSDK;
///
/// let sdk = LogosModuleSDK::new();
/// let provider = sdk.plugin("rust_provider_module");
/// let result = provider.call_sync("add", &[5i64, 3i64]).unwrap();
/// println!("Result: {}", result.message);
/// ```
pub struct LogosModuleSDK;

impl LogosModuleSDK {
    /// Create a new SDK instance. No initialization is performed.
    pub fn new() -> Self {
        LogosModuleSDK
    }

    /// Get a proxy for communicating with the named plugin.
    pub fn plugin(&self, name: &str) -> PluginProxy {
        PluginProxy::new(name)
    }

    /// Shut down the SDK.
    ///
    /// Since the move to the lp_* C ABI each `PluginProxy` owns its own
    /// protocol client (released on drop), so there is no process-global
    /// connection state left to tear down. Kept for source compatibility.
    pub fn shutdown(&self) {}
}

/// The logos-protocol semver this SDK was linked against — the single
/// number governing Logos load/call compatibility (same MAJOR ⇔
/// compatible). Forwarded verbatim from the linked protocol library,
/// never minted by the SDK.
pub fn protocol_version() -> String {
    let ptr = unsafe { crate::ffi::lp_protocol_version() };
    if ptr.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// MAJOR component of the linked logos-protocol version.
pub fn protocol_abi_major() -> i32 {
    unsafe { crate::ffi::lp_protocol_abi_major() }
}

/// Save a host-issued auth token into the SDK's protocol stack so
/// subsequent calls to `module_name` authenticate.
///
/// This is the consumer half of the module-impl C ABI's
/// `logos_module_accept_token` runtime handshake: the Qt glue receives the
/// token from the host and forwards it across the C seam; the generated
/// provider scaffold (lidl-gen `--provider`) hands it here so the *same*
/// protocol stack the SDK invokes through holds the token — closing the
/// split where the glue's stack was authenticated but the Rust one wasn't.
pub fn save_token(module_name: &str, token: &str) -> bool {
    let (Ok(name_c), Ok(token_c)) = (
        std::ffi::CString::new(module_name),
        std::ffi::CString::new(token),
    ) else {
        return false;
    };
    unsafe { crate::ffi::lp_token_save(name_c.as_ptr(), token_c.as_ptr()) == crate::ffi::LP_OK }
}

/// Route a host-issued grant into THIS image's gate state.
///
/// The grant has to cross the C ABI rather than be recorded once by the host:
/// the host binary and this cdylib each link their own copy of logos-protocol,
/// so each has its own process-global grant state, exactly as each has its own
/// TokenManager. A grant the host records for itself is invisible to the gate
/// an `lp_token_keys()` call checks here.
///
/// Takes the raw pointer the C ABI hands us rather than a `&str`: the caller is
/// the generated `logos_module_grant_host_services` export, which receives it
/// straight from the host and has nothing to gain from a round trip through
/// `CStr`. A null pointer is refused rather than dereferenced.
///
/// # Safety
/// `services_json` must be null or a valid NUL-terminated C string.
pub unsafe fn grant_host_services(services_json: *const std::os::raw::c_char) -> i32 {
    if services_json.is_null() {
        return -1;
    }
    unsafe { crate::ffi::lp_grant_host_services(services_json) }
}

impl Default for LogosModuleSDK {
    fn default() -> Self {
        Self::new()
    }
}

// -- teardown ---------------------------------------------------------------
//
// The module-impl C ABI gained two teardown exports in logos-protocol 0.5
// (`logos_module_set_unload_done_callback` / `logos_module_about_to_unload`).
// The generated provider scaffold owns the exports — it is what can reach the
// author's impl — but the CALLBACK lives here, for the same reason
// `grant_host_services` does: it is SDK state, and a module that wants to
// signal completion from its own code should not have to reach back into
// generated symbols to do it.

/// A module's answer to "are you ready to be unloaded?".
///
/// Mirrors C++'s `LogosShutdown` and Qt Creator's `IPlugin::aboutToShutdown()`
/// contract: return `Synchronous` when teardown finished inline (the common
/// case, and the default), or `Asynchronous` to keep the host waiting until
/// [`unload_finished`] is called. The host enforces a grace period either way —
/// an `Asynchronous` module that never finishes is killed, not waited on
/// forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shutdown {
    /// Teardown is complete; the host may proceed immediately.
    Synchronous,
    /// Teardown continues in the background; the host waits for
    /// [`unload_finished`] (or its grace period, whichever comes first).
    Asynchronous,
}

/// The C callback the host installs to learn that async teardown finished.
pub type UnloadDoneCb = unsafe extern "C" fn(*mut std::os::raw::c_void);

// The `void*` rides as a `usize` so the pair is `Send`: a raw pointer is not,
// and the callback is installed on the host's thread but fired from whichever
// thread the module finishes its work on — which is the whole point of the
// asynchronous answer.
static UNLOAD_CB: std::sync::Mutex<Option<(UnloadDoneCb, usize)>> = std::sync::Mutex::new(None);

/// Install the host's completion callback. `None` clears it.
///
/// Called by the generated `logos_module_set_unload_done_callback` export
/// before the host asks the module to unload, so a module that finishes inline
/// still has somewhere to signal.
pub fn set_unload_done_callback(cb: Option<UnloadDoneCb>, user_data: *mut std::os::raw::c_void) {
    *UNLOAD_CB.lock().unwrap() = cb.map(|f| (f, user_data as usize));
}

/// Signal that this module's asynchronous teardown is complete.
///
/// Only meaningful after returning [`Shutdown::Asynchronous`]; calling it
/// otherwise is harmless (the host is not waiting). Safe to call from any
/// thread — the host marshals the notification back to its own.
pub fn unload_finished() {
    // Copied out from under the lock: the callback re-enters the host, and
    // holding the SDK's mutex across that is how a teardown deadlock starts.
    let entry = *UNLOAD_CB.lock().unwrap();
    if let Some((cb, ud)) = entry {
        unsafe { cb(ud as *mut std::os::raw::c_void) };
    }
}

// -- WHO IS CALLING THIS DISPATCH -------------------------------------------
//
// The module-impl C ABI gained `logos_module_set_call_caller` in logos-protocol
// 0.6. The generated scaffold owns the EXPORT; the store and the author-facing
// accessor live here, for the same reason `grant_host_services` and the unload
// callback do — it is SDK state, and a handler should not have to reach into
// generated symbols to read it.
//
// WHY THE VALUE IS PUSHED ACROSS THE IMAGE BOUNDARY AT ALL. The host decides
// who the caller is (ModuleProxy has just matched the token the caller
// presented), so the obvious implementation is a thread-local in the protocol
// library that the handler reads back. It does not work, and it fails silently:
// the host binary and the module plugin EACH link their own copy of that
// library, and this was measured rather than assumed. `nm -m` on two shipped
// arm64 images in this workspace — liblogos_core.dylib (the host side) and
// counter_plugin.dylib (a module) — shows BOTH defining
// ModuleProxy::callRemoteMethod and TokenManager::instance(), each with its own
// copy of that function's local static in its own __DATA,__bss (0x1320d0 in the
// core, 0x3cdb8 in the plugin), and NEITHER image carrying a single undefined
// reference to the other's. Both headers read NOUNDEFS + TWOLEVEL, i.e. every
// intra-image call was bound to that image's own copy at static-link time; PE
// has no interposition at all. Only ELF's flat namespace would collapse the
// two, which is exactly the platform asymmetry that let two ABI breaks through.
// A thread-local set host-side is therefore NOT the one this crate would read:
// the push is the mechanism, not a fallback.
//
// A STACK, NOT A SLOT, and per THREAD. A handler that makes an outbound call
// spins a nested event loop, and a second inbound call can be delivered on the
// same thread inside it; with one slot the inner call's pop erases the outer
// call's caller, so the outer handler reads Unknown for the rest of its frame —
// a wrong identity rather than a missing one. And a concurrency:"multi" module
// serves several dispatches at once on different worker threads, so a
// process-global slot would hand every handler whichever caller was pushed
// last. Both are covered by tests below.
//
// VALID ONLY DURING A DISPATCH, ON THE DISPATCHING THREAD. A spawned worker, a
// timer callback, `on_context_ready` and an event emission all read
// [`LogosCaller::Unknown`] — correctly, since none of them has a caller. A
// handler that needs the identity beyond its own frame copies it at the top.

/// Who is calling the dispatch currently running on this thread.
///
/// Parsed from the JSON document the host pushes across the module-impl C ABI;
/// the normative definition of that document is in logos-protocol's
/// `cpp/logos_module_impl.h`, above `logos_module_set_call_caller`.
///
/// [`LogosCaller::Unknown`] is the FAIL-CLOSED value and it is IN BAND: it is
/// what an unnamed caller, an unreadable document and an arm minted by a newer
/// protocol all resolve to, and it is what [`current_caller`] answers when no
/// dispatch is in flight. Nothing here is spelled "verified" —
/// capability_module checks only that an asserted name EXISTS as a key, so the
/// strongest honest word for [`LogosCaller::Module`] is token-bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogosCaller {
    /// The caller could not be named — or there is no dispatch in flight.
    Unknown,
    /// The call presented one of the host bootstrap anchors. CARRIES NO NAME
    /// and must not gain one: "core" and "capability_module" hold the same
    /// token VALUE under two keys by construction, so a name here would be a
    /// coin flip presented as a fact.
    HostAnchor,
    /// A named module, token-bound. `instance` is optional from day one and
    /// [`LogosCaller::is_module`] deliberately ignores it.
    Module { name: String, instance: Option<String> },
    /// An isolated per-plugin identity derived from a parent module. Specified
    /// and parsed; nothing emits it yet.
    Derived { parent: String, leaf: String },
    /// An operator-issued named token. Specified and parsed; nothing emits it
    /// yet (it needs the transport's TokenValidator widened from bool).
    Operator { name: String },
}

impl LogosCaller {
    /// Parse one caller document. TOTAL: every input yields a value, and an
    /// input this build does not understand yields [`LogosCaller::Unknown`].
    ///
    /// Hand-rolled over `serde_json::Value` rather than a `#[serde(tag =
    /// "kind")]` derive: the rules below are normative, they sit next to
    /// authorization-shaped predicates, and each one is worth being able to
    /// point at in the source. A derive would leave them implicit in serde's
    /// semantics — and would accept `{"kind":"module","name":""}` as a module
    /// whose name is the empty string.
    pub fn from_json(doc: &str) -> Self {
        // Rule 1: "kind" is MANDATORY. An unparseable document, a non-object,
        // a missing or non-string "kind", and empty input all land here.
        let Ok(value) = serde_json::from_str::<serde_json::Value>(doc) else {
            return Self::Unknown;
        };
        let Some(object) = value.as_object() else {
            return Self::Unknown;
        };
        let Some(kind) = object.get("kind").and_then(serde_json::Value::as_str) else {
            return Self::Unknown;
        };

        // Rule 4: a known arm missing a required field is Unknown, not a
        // partial value. An empty string is not a name — Module{name:""} would
        // answer is_module("") with true, a predicate widened by a malformed
        // document.
        let required = |key: &str| -> Option<String> {
            match object.get(key).and_then(serde_json::Value::as_str) {
                Some("") | None => None,
                Some(s) => Some(s.to_string()),
            }
        };

        // Rule 3 is structural here: nothing outside the fields named below is
        // read, so an arm can gain a field without a version bump.
        match kind {
            "unknown" => Self::Unknown,
            // Rule 5: no name is read, so none can be invented.
            "host" => Self::HostAnchor,
            "module" => {
                let Some(name) = required("name") else {
                    return Self::Unknown;
                };
                // Rule 6: `instance` is OPTIONAL, and an UNREADABLE one is
                // DROPPED rather than failing the identity.
                //
                // The other reading — fail closed, because instance
                // participates in identity() — was what this backend did until
                // a cross-language audit found C++ doing the opposite, each
                // with a green test asserting its own answer. The protocol was
                // silent, so both were folklore. It is normative now, and this
                // is the arm it chose:
                //
                //   `name` comes from the host's own token resolution and is
                //   not made less trustworthy by a malformed sibling field, and
                //   is_module(name) — the path essentially every caller takes —
                //   ignores instance entirely. Failing closed would turn a
                //   cosmetic wire defect into a fleet-wide authorization
                //   change, and would mean a protocol that later emits a richer
                //   `instance` silently stops every Rust module recognising
                //   callers that C++ modules still recognise.
                let instance = match object.get("instance") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(_) => required("instance"),
                };
                Self::Module { name, instance }
            }
            "derived" => match (required("parent"), required("leaf")) {
                (Some(parent), Some(leaf)) => Self::Derived { parent, leaf },
                _ => Self::Unknown,
            },
            "operator" => match required("name") {
                Some(name) => Self::Operator { name },
                None => Self::Unknown,
            },
            // Rule 2: an UNRECOGNISED arm is Unknown. Never a closest match,
            // never dropped. This is the only safe direction — adding an arm
            // can turn an old reader's is_module(x) from true to false, never
            // the reverse; a permissive fallback would silently WIDEN a
            // predicate that sits next to an authorization decision.
            _ => Self::Unknown,
        }
    }

    /// Whether the caller is the named module, ignoring which INSTANCE of it.
    ///
    /// The instance is ignored deliberately and permanently: the draft spec's
    /// authenticated invoker is an instance address rather than a bare name, so
    /// a call site written today keeps its meaning when instance addressing
    /// arrives. A caller that must distinguish instances compares
    /// [`LogosCaller::identity`].
    pub fn is_module(&self, name: &str) -> bool {
        matches!(self, Self::Module { name: n, .. } if n == name)
    }

    /// Whether the caller is that derived leaf of that parent module. Both
    /// halves must match: a leaf is not its parent, and two parents can carry
    /// leaves of the same name.
    pub fn is_derived(&self, parent: &str, leaf: &str) -> bool {
        matches!(self, Self::Derived { parent: p, leaf: l } if p == parent && l == leaf)
    }

    /// A stable, machine-comparable spelling of this caller — for a map key, a
    /// structured log field, or an equality test that must not lose the
    /// instance.
    ///
    /// Every arm carries its KIND, so two arms can never collide: an operator
    /// named "ops" and a module named "ops" are different callers and must not
    /// key the same entry. Unknown has an identity too — it is in band, not the
    /// empty string.
    pub fn identity(&self) -> String {
        match self {
            Self::Unknown => "unknown".to_string(),
            Self::HostAnchor => "host".to_string(),
            Self::Module { name, instance: None } => format!("module:{name}"),
            Self::Module { name, instance: Some(instance) } => format!("module:{name}#{instance}"),
            Self::Derived { parent, leaf } => format!("derived:{parent}/{leaf}"),
            Self::Operator { name } => format!("operator:{name}"),
        }
    }

    /// A phrase for a log line or an error message, e.g. "module chat_module".
    /// Not stable and not for comparison — use [`LogosCaller::identity`] for
    /// that.
    pub fn describe_for_human(&self) -> String {
        match self {
            Self::Unknown => "an unidentified caller".to_string(),
            Self::HostAnchor => "the host".to_string(),
            Self::Module { name, instance: None } => format!("module {name}"),
            Self::Module { name, instance: Some(instance) } => {
                format!("module {name} (instance {instance})")
            }
            Self::Derived { parent, leaf } => format!("{leaf}, derived from {parent}"),
            Self::Operator { name } => format!("operator {name}"),
        }
    }
}

thread_local! {
    // The RAW documents, innermost last. Raw because the push happens on EVERY
    // dispatch into this module while the read happens only in the handlers
    // that ask: storing the document costs one copy at the seam and leaves the
    // parse to be paid by the reader.
    static CALL_CALLERS: std::cell::RefCell<Vec<String>> =
        std::cell::RefCell::new(Vec::new());
}

/// Push (non-null) or pop (null) the caller of the dispatch about to run on
/// this thread. The target of the generated `logos_module_set_call_caller`
/// export; module code should not call it.
///
/// NULL is a VALUE in this ABI, not an error: it pops the innermost entry, and
/// a pop with nothing pushed is a documented no-op. Takes the raw pointer the C
/// ABI hands us for exactly that reason — the nullness is the message, so there
/// is nothing to gain from a round trip through `&str` before the decision.
///
/// Never panics, whatever the host does: a panic here would unwind out of an
/// `extern "C"` frame in the host's dispatch path. A document that is not valid
/// UTF-8 is taken lossily (it will fail to parse, and a caller that cannot be
/// named is Unknown), and a thread already tearing its TLS down is a no-op.
///
/// # Safety
/// `caller_json` must be null or a valid NUL-terminated C string.
pub unsafe fn set_call_caller(caller_json: *const std::os::raw::c_char) {
    if caller_json.is_null() {
        let _ = CALL_CALLERS.try_with(|stack| stack.borrow_mut().pop());
        return;
    }
    let document = unsafe { std::ffi::CStr::from_ptr(caller_json) }
        .to_string_lossy()
        .into_owned();
    let _ = CALL_CALLERS.try_with(|stack| stack.borrow_mut().push(document));
}

/// The RAW caller document of the dispatch running on this thread, or `None`
/// when no dispatch is in flight here.
///
/// Two things [`current_caller`] cannot do: distinguish "no dispatch on this
/// thread" (`None`) from "a dispatch whose caller could not be named"
/// (`{"kind":"unknown"}`), and forward an arm minted by a NEWER protocol
/// onward without flattening it to Unknown. Everything else should use the
/// typed accessor.
pub fn current_caller_json() -> Option<String> {
    CALL_CALLERS
        .try_with(|stack| stack.borrow().last().cloned())
        .ok()
        .flatten()
}

/// Who is calling the method your handler is running — the ambient accessor a
/// module author uses.
///
/// Returns [`LogosCaller::Unknown`] when the caller could not be named AND when
/// there is no dispatch in flight on this thread (a worker, a timer,
/// `on_context_ready`, an event emission). That fold is deliberate: Unknown is
/// the fail-closed answer to "who is calling?", and a handler asking the
/// question wants one in-band value rather than two shapes of nothing.
///
/// Parsed rather than raw so there is exactly ONE reader of this document in
/// the Rust world. The alternative — handing every module the JSON — would have
/// each author re-derive the forward-compatibility rule, and any one of them
/// doing it permissively silently widens a predicate that sits next to an
/// authorization decision.
///
/// ```ignore
/// fn delete_everything(&mut self) -> bool {
///     if !logos_rust_sdk::current_caller().is_module("admin_module") {
///         return false;   // Unknown lands here too, which is the point
///     }
///     /* ... */
///     true
/// }
/// ```
pub fn current_caller() -> LogosCaller {
    match current_caller_json() {
        Some(document) => LogosCaller::from_json(&document),
        None => LogosCaller::Unknown,
    }
}

#[cfg(test)]
mod caller_tests {
    use super::*;
    use std::ffi::CString;

    // The two halves of the C ABI, as the generated export calls them: a
    // non-NULL document PUSHES, NULL POPS the innermost.
    fn push(doc: &str) {
        let c = CString::new(doc).unwrap();
        unsafe { set_call_caller(c.as_ptr()) };
    }
    fn pop() {
        unsafe { set_call_caller(std::ptr::null()) };
    }

    // ── the document → the type: the normative rules, in the order a reader
    //    applies them (logos-protocol cpp/logos_module_impl.h) ──────────────

    #[test]
    fn every_specified_arm_parses() {
        assert_eq!(
            LogosCaller::from_json(r#"{"kind":"unknown"}"#),
            LogosCaller::Unknown
        );
        assert_eq!(
            LogosCaller::from_json(r#"{"kind":"host"}"#),
            LogosCaller::HostAnchor
        );
        assert_eq!(
            LogosCaller::from_json(r#"{"kind":"module","name":"chat_module"}"#),
            LogosCaller::Module { name: "chat_module".into(), instance: None }
        );
        assert_eq!(
            LogosCaller::from_json(r#"{"kind":"module","name":"chat_module","instance":"a41f"}"#),
            LogosCaller::Module { name: "chat_module".into(), instance: Some("a41f".into()) }
        );
        assert_eq!(
            LogosCaller::from_json(r#"{"kind":"derived","parent":"wallet_module","leaf":"wallet_ui"}"#),
            LogosCaller::Derived { parent: "wallet_module".into(), leaf: "wallet_ui".into() }
        );
        assert_eq!(
            LogosCaller::from_json(r#"{"kind":"operator","name":"ops-readonly"}"#),
            LogosCaller::Operator { name: "ops-readonly".into() }
        );
    }

    // THE forward-compatibility property, and the reason the parser is total.
    // A host one protocol ahead of this module can push an arm this build has
    // never heard of. Degrading to Unknown is the only safe direction: it can
    // turn an old reader's is_module(x) from true to false, never the reverse,
    // and these predicates sit next to authorization decisions. A closest
    // match, a dropped document or a panic would each widen one.
    #[test]
    fn an_unrecognised_arm_degrades_to_unknown_rather_than_panicking() {
        assert_eq!(
            LogosCaller::from_json(r#"{"kind":"fleet","id":"7"}"#),
            LogosCaller::Unknown
        );
        let near_miss = LogosCaller::from_json(r#"{"kind":"module_v2","name":"chat_module"}"#);
        assert_eq!(near_miss, LogosCaller::Unknown);
        assert!(!near_miss.is_module("chat_module"));
    }

    // Rule 1: "kind" is MANDATORY. Missing, non-string, unparseable or empty
    // input ⇒ unknown — in band, never spelled by absence.
    #[test]
    fn a_malformed_document_is_unknown_in_band() {
        for doc in [
            "",
            "   ",
            "not json",
            "[]",
            "\"host\"",
            "null",
            "{}",
            r#"{"kind":5}"#,
            r#"{"kind":null}"#,
            r#"{"kind":"module""#,
        ] {
            assert_eq!(LogosCaller::from_json(doc), LogosCaller::Unknown, "{doc:?}");
        }
    }

    // Rule 4: a known arm missing a required field ⇒ unknown, not a partial
    // value. An empty name is not a name: Module{name:""} would answer
    // is_module("") true, which is a predicate widened by a malformed document.
    #[test]
    fn a_known_arm_missing_a_required_field_is_unknown_not_partial() {
        for doc in [
            r#"{"kind":"module"}"#,
            r#"{"kind":"module","name":""}"#,
            r#"{"kind":"module","name":7}"#,
            r#"{"kind":"derived","parent":"wallet_module"}"#,
            r#"{"kind":"derived","leaf":"wallet_ui"}"#,
            r#"{"kind":"operator"}"#,
        ] {
            assert_eq!(LogosCaller::from_json(doc), LogosCaller::Unknown, "{doc:?}");
        }
    }

    // Rule 6, the malformed half. CONFORMANCE: this exact document is asserted
    // identically by the C++ backend (logos-cpp-sdk tests/sdk/test_logos_caller.cpp).
    // The two disagreed until an audit caught it — each returning its own answer
    // with a green test pinning it, so neither suite could see the divergence.
    // Keep the two in step; the protocol header is the authority.
    #[test]
    fn an_unreadable_instance_is_dropped_and_the_module_is_still_identified() {
        assert_eq!(
            LogosCaller::from_json(r#"{"kind":"module","name":"chat_module","instance":7}"#),
            LogosCaller::Module { name: "chat_module".to_string(), instance: None }
        );
    }

    // Rule 3: unrecognised FIELDS inside a known arm are ignored, so an arm can
    // gain a field without a version bump.
    #[test]
    fn unrecognised_fields_inside_a_known_arm_are_ignored() {
        assert_eq!(
            LogosCaller::from_json(r#"{"kind":"module","name":"chat_module","hops":2}"#),
            LogosCaller::Module { name: "chat_module".into(), instance: None }
        );
    }

    // Rule 5: "host" carries NO name and must not gain one — "core" and
    // "capability_module" hold the same token VALUE under two keys, so a name
    // there would be a coin flip presented as a fact.
    #[test]
    fn the_host_arm_carries_no_name_even_when_one_is_offered() {
        assert_eq!(
            LogosCaller::from_json(r#"{"kind":"host","name":"core"}"#),
            LogosCaller::HostAnchor
        );
    }

    // Rule 6: is_module(name) deliberately ignores the instance, so a call site
    // written today keeps its meaning when instance addressing arrives.
    #[test]
    fn is_module_ignores_the_instance_but_identity_does_not() {
        let bare = LogosCaller::from_json(r#"{"kind":"module","name":"chat_module"}"#);
        let inst =
            LogosCaller::from_json(r#"{"kind":"module","name":"chat_module","instance":"a41f"}"#);
        assert!(bare.is_module("chat_module"));
        assert!(inst.is_module("chat_module"));
        assert!(!inst.is_module("chat_modul"));
        // A caller that DOES care about instances compares the full identity.
        assert_ne!(bare.identity(), inst.identity());
    }

    #[test]
    fn is_derived_matches_both_halves_or_neither() {
        let d = LogosCaller::from_json(
            r#"{"kind":"derived","parent":"wallet_module","leaf":"wallet_ui"}"#,
        );
        assert!(d.is_derived("wallet_module", "wallet_ui"));
        assert!(!d.is_derived("wallet_module", "wallet_settings"));
        assert!(!d.is_derived("chat_module", "wallet_ui"));
        // A derived leaf is not its parent module, and is not a module at all.
        assert!(!d.is_module("wallet_module"));
        assert!(!d.is_module("wallet_ui"));
    }

    // identity() is the machine-comparable form, so it must never let two
    // different KINDS collide: an operator and a module may be spelled with the
    // same name, and an ACL keyed on identity() would then confuse them.
    #[test]
    fn identity_never_collides_across_arms() {
        let m = LogosCaller::from_json(r#"{"kind":"module","name":"ops"}"#);
        let o = LogosCaller::from_json(r#"{"kind":"operator","name":"ops"}"#);
        assert_ne!(m.identity(), o.identity());
        assert!(!o.is_module("ops"));
        // Unknown is in band here too: it has an identity, it is not empty.
        assert!(!LogosCaller::Unknown.identity().is_empty());
    }

    #[test]
    fn describe_for_human_names_what_it_can_and_admits_what_it_cannot() {
        assert!(LogosCaller::from_json(r#"{"kind":"module","name":"chat_module"}"#)
            .describe_for_human()
            .contains("chat_module"));
        assert!(LogosCaller::from_json(
            r#"{"kind":"module","name":"chat_module","instance":"a41f"}"#
        )
        .describe_for_human()
        .contains("a41f"));
        let d = LogosCaller::from_json(
            r#"{"kind":"derived","parent":"wallet_module","leaf":"wallet_ui"}"#,
        );
        assert!(d.describe_for_human().contains("wallet_ui"));
        assert!(d.describe_for_human().contains("wallet_module"));
        // The host arm must not acquire a name on the way to a log line.
        let h = LogosCaller::HostAnchor.describe_for_human();
        assert!(!h.contains("core") && !h.contains("capability_module"), "{h}");
        assert!(!LogosCaller::Unknown.describe_for_human().is_empty());
    }

    // ── the per-thread stack ────────────────────────────────────────────────

    // A background thread, a timer, on_context_ready and event emission all
    // read Unknown — correctly, since none of them has a caller.
    #[test]
    fn with_no_dispatch_in_flight_the_caller_is_unknown_in_band() {
        assert_eq!(current_caller_json(), None);
        assert_eq!(current_caller(), LogosCaller::Unknown);
    }

    #[test]
    fn a_push_is_visible_to_the_handler_and_null_pops_it() {
        push(r#"{"kind":"module","name":"chat_module"}"#);
        assert!(current_caller().is_module("chat_module"));
        pop();
        assert_eq!(current_caller(), LogosCaller::Unknown);
    }

    // A handler that makes an outbound call spins a nested event loop, and QtRO
    // can deliver a SECOND inbound call on the same thread inside it. With a
    // single slot the inner pop erases the OUTER dispatch's caller: the outer
    // handler's identity evaporates mid-frame, silently, and it reads Unknown
    // for the rest of its life. This is why the ABI is a stack.
    #[test]
    fn a_nested_dispatch_restores_the_outer_caller_when_it_pops() {
        push(r#"{"kind":"module","name":"outer_module"}"#);
        push(r#"{"kind":"host"}"#);
        assert_eq!(current_caller(), LogosCaller::HostAnchor);
        pop();
        assert!(
            current_caller().is_module("outer_module"),
            "the outer caller evaporated when the nested dispatch popped: {:?}",
            current_caller()
        );
        pop();
        assert_eq!(current_caller(), LogosCaller::Unknown);
    }

    // "a pop with nothing pushed is a no-op" — the ABI says so, and a module
    // that underflowed its stack would be a module that panics inside the
    // host's dispatch path.
    #[test]
    fn a_pop_with_nothing_pushed_is_a_no_op() {
        pop();
        pop();
        assert_eq!(current_caller(), LogosCaller::Unknown);
        push(r#"{"kind":"host"}"#);
        assert_eq!(current_caller(), LogosCaller::HostAnchor);
        pop();
    }

    // PER THREAD, not per process. A concurrency:"multi" module serves several
    // dispatches at once on different worker threads; one process-global slot
    // would hand every handler whichever caller was pushed last — a WRONG
    // identity rather than a missing one.
    #[test]
    fn the_caller_is_per_thread_not_per_process() {
        push(r#"{"kind":"module","name":"chat_module"}"#);
        let (seen_before_push, seen_after) = std::thread::spawn(|| {
            let before = current_caller();
            push(r#"{"kind":"operator","name":"ops-readonly"}"#);
            let after = current_caller();
            pop();
            (before, after)
        })
        .join()
        .unwrap();
        assert_eq!(
            seen_before_push,
            LogosCaller::Unknown,
            "a worker thread read another thread's caller"
        );
        assert_eq!(seen_after, LogosCaller::Operator { name: "ops-readonly".into() });
        // ...and this thread's own frame is untouched by all of that.
        assert!(current_caller().is_module("chat_module"));
        pop();
    }

    // The raw document is where "no dispatch here" and "a dispatch whose caller
    // could not be named" are still distinguishable; the typed accessor folds
    // both to Unknown, which is the fail-closed answer a handler wants.
    #[test]
    fn the_raw_document_keeps_a_distinction_the_typed_accessor_folds_away() {
        assert_eq!(current_caller_json(), None);
        push(r#"{"kind":"unknown"}"#);
        assert_eq!(current_caller_json().as_deref(), Some(r#"{"kind":"unknown"}"#));
        assert_eq!(current_caller(), LogosCaller::Unknown);
        pop();
        assert_eq!(current_caller_json(), None);
    }
}
