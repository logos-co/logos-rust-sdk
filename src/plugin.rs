//! Plugin proxy for method calls and event subscriptions, over the lp_* C ABI.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use crate::callback::{
    create_event_callback, create_method_callback, event_callback_ptr, json_to_message,
    CallResult, EventCallbackData, EventData,
};
use crate::error::LogosError;
use crate::ffi;
use crate::params::{params_to_lp_args, Param, ToParam};

/// The `timeout_ms` argument of `lp_invoke` / `lp_invoke_async`, in the ABI's
/// own units and type.
///
/// `logos_protocol.h` spells the contract out: *"timeout_ms <= 0 selects the
/// default timeout, currently 20s"*. So the ABI has exactly one sentinel and
/// one real range, and this type is the single place the Rust surface converts
/// into them — every `lp_invoke*` call in this crate passes `as_abi()`.
///
/// It is an ARGUMENT, not a property of anything: it is threaded down to each
/// `lp_invoke*` as a parameter and stored nowhere, so no call can inherit a
/// bound another call asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimeoutMs(c_int);

impl TimeoutMs {
    /// "Let the protocol decide." This is the literal `0` every call in this
    /// crate passed unconditionally before per-call timeouts were surfaced, so
    /// a call site that asks for no timeout still reaches the ABI byte-for-byte
    /// as it always did.
    pub(crate) const DEFAULT: TimeoutMs = TimeoutMs(0);

    /// Convert an idiomatic `Duration` into the ABI's millisecond `c_int`.
    ///
    /// Both ends of the range are REFUSED rather than quietly reinterpreted,
    /// because at both ends the reinterpretation would be a different answer
    /// wearing the caller's request as a disguise:
    ///
    /// * **Sub-millisecond** (`as_millis() == 0`, which includes
    ///   `Duration::ZERO`): the ABI reads `0` as "use the default", so a 500µs
    ///   timeout would become **20 seconds** — four orders of magnitude the
    ///   wrong way, and in the one direction a timeout exists to prevent.
    /// * **Longer than `c_int::MAX` ms** (~24.8 days): truncating to 32 bits
    ///   wraps (and can land `<= 0`, i.e. the default again), and saturating
    ///   would silently shorten a caller's stated bound.
    ///
    /// The valid range is therefore `1ms..=c_int::MAX ms`, and everything
    /// outside it is a `LogosError::InvalidTimeout` at the point the bad value
    /// was supplied.
    pub(crate) fn from_duration(timeout: Duration) -> Result<Self, LogosError> {
        let ms = timeout.as_millis();
        if ms == 0 {
            return Err(LogosError::InvalidTimeout {
                timeout,
                reason: "shorter than 1ms; the lp_* ABI's millisecond resolution \
                         cannot express it, and rounding to 0 would select the \
                         protocol DEFAULT (20s) instead"
                    .to_string(),
            });
        }
        if ms > c_int::MAX as u128 {
            return Err(LogosError::InvalidTimeout {
                timeout,
                reason: format!(
                    "longer than the lp_* ABI's maximum of {}ms (~24.8 days); \
                     it is refused rather than clamped",
                    c_int::MAX
                ),
            });
        }
        Ok(TimeoutMs(ms as c_int))
    }

    /// The value handed to `lp_invoke` / `lp_invoke_async`.
    pub(crate) fn as_abi(self) -> c_int {
        self.0
    }
}

/// Shared ownership of the underlying `lp_client`. The client is destroyed
/// when the LAST owner drops — the proxy itself or any live subscription.
/// lp_* handles are thread-safe per-handle (the logos_protocol.h threading
/// contract), so sharing the raw handle across threads is sound.
struct ClientHandle {
    client: *mut ffi::LpClient,
    /// The target's subscription-status watcher, installed at most once per
    /// client. It lives HERE rather than in a per-subscription box because the
    /// C ABI reports these edges per target: this handle is shared by every
    /// subscription to the module, and so is the callback.
    ///
    /// It is also what the trampoline's `user_data` points at — the Arc keeps
    /// the allocation alive for exactly as long as the client it destroys in
    /// `Drop`, so the pointer cannot outlive what reads it.
    /// `Arc` rather than `Box` so the trampoline can CLONE it out of the lock
    /// and call it with the mutex released — a watcher's obvious move on `Held`
    /// is `rearm_subscriptions()`, and holding the lock across that would
    /// deadlock against a concurrent installer.
    status: Mutex<Option<Arc<dyn Fn(SubStatus, u64) + Send + Sync + 'static>>>,
}
unsafe impl Send for ClientHandle {}
unsafe impl Sync for ClientHandle {}
impl Drop for ClientHandle {
    fn drop(&mut self) {
        if !self.client.is_null() {
            unsafe { ffi::lp_client_destroy(self.client) };
        }
    }
}

/// Process-global cache of ONE shared `lp_client` per (origin, target) pair.
///
/// Why: the protocol coalesces concurrent capability handshakes PER client, so
/// a fan-out that opens a fresh client per call (the old `modules().dep.x()`
/// behavior) fires N racing `requestModule` handshakes whose per-call tokens
/// overwrite each other on the target and get the in-flight calls rejected.
/// Sharing one client per target makes the N calls coalesce to a single
/// handshake — the rust analog of the C++ SDK's one persistent `LogosModules`.
///
/// The value is a `Weak` so the cache never pins a client past its real owners:
/// live proxies / async-call states / subscriptions hold the strong `Arc`, and
/// when the last drops, `lp_client_destroy` fires (teardown unchanged) and a
/// later lookup re-creates. Sharing across threads is sound by the same
/// per-handle thread-safety contract `EventSubscription` already relies on.
///
/// Keyed by (origin, target), not by target alone. The origin is latched once
/// per image (`set_module_origin`) and in a module it is latched before any
/// author code runs, so in practice one origin ever appears — but a client
/// carries its origin for life, and it is the origin the capability handshake
/// authenticates. Keying on it means a client built before the latch can never
/// be handed back after it, under a name it does not actually announce.
fn client_cache() -> &'static Mutex<HashMap<(String, String), Weak<ClientHandle>>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, String), Weak<ClientHandle>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The origin every client this image creates announces — this module's own
/// name, latched by the generated scaffold (see `api::set_module_origin`).
///
/// Empty when nothing declared one. Empty is the FAIL-CLOSED answer, and it is
/// deliberately not a guess: `capability_module::requestModule` refuses an
/// empty `fromModuleName` outright ("rejecting empty module name"), and
/// `ModuleProxy::saveToken` refuses to file a token under an empty caller. The
/// alternative — inventing a plausible name — is precisely the bug this
/// replaces, because the plausible name that was invented ("core") happened to
/// be a bootstrapKeys() anchor, and so carried the host's authority wherever
/// the callee looked at it.
///
/// Warned once per process rather than per client: the cause is one missing
/// declaration in the image, not a property of the call that tripped over it.
fn outbound_origin() -> String {
    match crate::api::module_origin() {
        Some(name) => name.to_string(),
        None => {
            static WARNED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!(
                    "logos-rust-sdk: creating an outbound client with NO module origin \
                     declared — the capability handshake will be refused. A module built \
                     by logos-module-builder gets this from its generated scaffold; a \
                     caller outside a plugin must call \
                     logos_rust_sdk::set_module_origin(\"<its own module name>\") first."
                );
            }
            String::new()
        }
    }
}

/// Get-or-create the shared client for `target`, announcing THIS module's own
/// name as the origin.
/// Returns None on the same failure surface as before (bad name / null client).
fn shared_client(target: &str) -> Option<Arc<ClientHandle>> {
    // Read the origin ONCE, before the lookup, and use that same value for the
    // key and for lp_client_create. Reading it twice could straddle the latch
    // and publish a client under a key it was not built with.
    let origin = outbound_origin();
    let key = (origin.clone(), target.to_string());

    // Fast path: a live client for this (origin, target) already exists.
    if let Some(existing) = {
        let map = client_cache().lock().unwrap_or_else(|e| e.into_inner());
        map.get(&key).and_then(Weak::upgrade)
    } {
        return Some(existing);
    }

    // Create OUTSIDE the lock. This used to run under the map lock, described
    // there as an "(uncontended, I/O-free)" call. It is neither: on a Qt-affine
    // transport — the default inside a module process — lp_client_create ends
    // in runOnQtMainThread, i.e. a Qt::BlockingQueuedConnection that blocks
    // until the Qt main thread services it. A worker holding the map lock would
    // then wait for the main thread, while the main thread — reaching this
    // function for ANY other target, e.g. from an inbound dispatch — blocks on
    // that same lock and so never returns to the event loop that would run the
    // construction. Neither ever proceeds. (logos-cpp-sdk's LpClient::ensure
    // and logos-qt-sdk's LpBridge::resultClient avoid the identical hazard the
    // same way.)
    let target_c = CString::new(target).ok()?;
    // The module's OWN name. Not a literal: see api::set_module_origin for what
    // announcing a bootstrapKeys() label instead was measured to do.
    let origin_c = CString::new(origin.as_str()).ok()?;
    let raw = unsafe {
        ffi::lp_client_create(target_c.as_ptr(), origin_c.as_ptr(), ptr::null(), ptr::null())
    };
    if raw.is_null() {
        return None;
    }
    let handle = Arc::new(ClientHandle { client: raw, status: Mutex::new(None) });

    // Publish under the lock, re-checking: two threads may have raced through
    // the gap above. The loser drops its handle, which destroys its own client
    // (lp_client_destroy is safe from any thread and defers teardown to the
    // owner thread), so the "one shared client per target" invariant that makes
    // concurrent calls coalesce into a single capability handshake still holds.
    let mut map = client_cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = map.get(&key).and_then(Weak::upgrade) {
        return Some(existing);
    }
    map.insert(key, Arc::downgrade(&handle));
    Some(handle)
}

/// A live event subscription: the channel of incoming events PLUS ownership
/// of everything that keeps it alive (the lp subscription, its callback
/// state, and a share of the client). Drop it to unsubscribe.
///
/// The lp callback is gated by the client's liveness — a bare
/// `Receiver` whose proxy was dropped would never see another event. This
/// handle is what makes the module-side pattern work:
///
/// ```ignore
/// let sub = modules().dep.on_some_event()?;       // proxy is a temporary
/// std::thread::spawn(move || {
///     for ev in sub { /* sub owns the client; events keep flowing */ }
/// });
/// ```
pub struct EventSubscription {
    rx: Receiver<EventData>,
    sub: *mut ffi::LpSubscription,
    _callback: Box<EventCallbackData>,
    _client: Arc<ClientHandle>,
}
// Safety: the lp subscription/client handles are thread-safe per-handle, and
// the callback box is only read by the lp trampoline.
unsafe impl Send for EventSubscription {}

/// One line per missing capability per process, not per call: a module
/// configuring forty deps against an old runtime should say so once.
fn warn_once_no_restart() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        eprintln!(
            "logos: set_restart_policy: this runtime is logos-protocol {}, which has no \
             per-module subscription restart policy (needs 0.9). Subscriptions are live, but \
             RestartPolicy::Manual is NOT in effect.",
            crate::api::protocol_version()
        );
    });
}

fn warn_once_no_status() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        eprintln!(
            "logos: on_subscription_status: this runtime is logos-protocol {}, which has no \
             per-module subscription status channel (needs 0.9). Subscriptions are live, but \
             the watcher will NOT fire.",
            crate::api::protocol_version()
        );
    });
}

/// Is the LINKED logos-protocol at least `major.minor`?
///
/// Runtime rather than compile-time, and deliberately so: the version comes
/// from `lp_protocol_version()` in the protocol library this module is actually
/// linked against, so a feature guarded on it cannot disagree with the runtime
/// the way a build-time `--cfg` can.
///
/// Note what this does NOT do: it does not make a call site safe to LINK. An
/// unused `extern "C"` declaration is free, but a call site stamps an
/// unconditional undefined symbol into every module's staticlib. Guarding a
/// call in an `if` does not remove that reference — the SDK and the protocol
/// travel in one nix closure, which is what actually guarantees the symbol is
/// there. This guard is about SEMANTICS: answering 0/false rather than calling
/// into a runtime whose answer would be meaningless.
fn protocol_at_least(major: u32, minor: u32) -> bool {
    let v = crate::api::protocol_version();
    let mut it = v.split('.');
    let have_major: u32 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let have_minor: u32 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    (have_major, have_minor) >= (major, minor)
}

/// What happened to a TARGET MODULE's subscriptions, as reported by
/// [`PluginProxy::on_subscription_status`].
///
/// Per module, not per subscription: every subscription to a module shares the
/// provider's single handle, so they are lost and re-established together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SubStatus {
    /// Live. `generation` identifies THIS arming.
    Armed,
    /// The provider became unreachable. Events until the next `Armed` are
    /// gone. Not terminal — the SDK re-arms.
    Lost,
    /// Terminal; it will never fire again.
    Abandoned,
    /// The provider became unreachable AND the target's policy is
    /// [`RestartPolicy::Manual`], so nothing will re-arm itself. Delivered
    /// INSTEAD OF `Lost`, never alongside it. Revive with
    /// [`PluginProxy::rearm_subscriptions`].
    Held,
}

/// What the runtime does when a target's ARMED subscriptions lose their
/// provider. Set per module with [`PluginProxy::set_restart_policy`].
///
/// `Manual` means "do not RE-arm after a loss" and never "do not arm the first
/// time": a subscription taken before its provider is reachable is deferred and
/// armed under either policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum RestartPolicy {
    #[default]
    Automatic,
    Manual,
}

pub(crate) extern "C" fn status_trampoline(
    state: std::os::raw::c_int,
    generation: u64,
    _reason: *const std::os::raw::c_char,
    user_data: *mut std::os::raw::c_void,
) {
    if user_data.is_null() {
        return;
    }
    // An UNKNOWN code is DROPPED, never coerced. A newer protocol can add a
    // status this build has no name for, and reporting it as Armed -- the
    // numerically-first value -- would tell a subscriber its subscription is
    // live at the one moment that might not be true.
    let status = match state {
        ffi::LP_SUB_ARMED => SubStatus::Armed,
        ffi::LP_SUB_LOST => SubStatus::Lost,
        ffi::LP_SUB_ABANDONED => SubStatus::Abandoned,
        ffi::LP_SUB_HELD => SubStatus::Held,
        _ => return,
    };
    // The CLIENT handle, not a per-subscription box: this callback is installed
    // per target and outlives every individual subscription through it.
    //
    // Safety: `user_data` is `Arc::as_ptr` of the ClientHandle that owns the
    // client this edge came from. `Drop` calls `lp_client_destroy` — after
    // which the C side fires nothing — before the allocation is released, so
    // the pointer cannot outlive what reads it.
    let handle = unsafe { &*(user_data as *const ClientHandle) };
    let cb = handle
        .status
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(Arc::clone));
    if let Some(f) = cb {
        f(status, generation);
    }
}

impl EventSubscription {
    /// Block until the next event arrives (or the subscription dies).
    pub fn recv(&self) -> Result<EventData, std::sync::mpsc::RecvError> {
        self.rx.recv()
    }

    /// Non-blocking poll for a pending event.
    pub fn try_recv(&self) -> Result<EventData, std::sync::mpsc::TryRecvError> {
        self.rx.try_recv()
    }

    /// The underlying channel, for `select`-style integration.
    pub fn receiver(&self) -> &Receiver<EventData> {
        &self.rx
    }

}

/// Blocking iteration: `for ev in subscription { ... }` yields each event as
/// it arrives, ending if the subscription's channel closes.
impl Iterator for EventSubscription {
    type Item = EventData;
    fn next(&mut self) -> Option<EventData> {
        self.rx.recv().ok()
    }
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        // After lp_unsubscribe returns the callback will not fire again; the
        // client share (and the callback box) drop after.
        unsafe { ffi::lp_unsubscribe(self.sub) };
    }
}

/// Everything an in-flight async call needs to outlive the proxy that
/// launched it: the one-shot completion closure, the names for error
/// reporting, and — crucially — a share of the client. A `modules()`
/// dependency client is a temporary, so without holding this share the
/// client would be destroyed the instant the call statement ends, before
/// the result arrives. Reclaimed (and dropped) by the trampoline when the
/// result lands. Mirrors how [`EventSubscription`] keeps its client alive.
struct AsyncCallState {
    callback: Box<dyn FnOnce(Result<serde_json::Value, LogosError>) + Send>,
    plugin: String,
    method: String,
    _client: Arc<ClientHandle>,
}

/// lp result trampoline for [`PluginProxy::call_json_async`]: reclaim the
/// boxed state, turn `(ok, json)` into a typed `Result<Value, _>` (the raw
/// JSON value on success; a `PluginCallFailed` carrying the canonical error
/// object's message on failure — the async analog of `call_json`'s sync error
/// path), and hand it to the one-shot callback.
extern "C" fn async_call_trampoline(ok: c_int, json: *const c_char, user_data: *mut c_void) {
    if user_data.is_null() {
        return;
    }
    let state = unsafe { Box::from_raw(user_data as *mut AsyncCallState) };
    let AsyncCallState { callback, plugin, method, _client } = *state;

    let raw = if json.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(json) }.to_string_lossy().into_owned()
    };

    let result = if ok != 0 {
        let value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw));
        // Same fold as the sync path: a rejection arrives as a successful
        // result, and this callback's Result is where it belongs.
        match crate::args::as_dispatch_rejection(&value) {
            Some(message) => Err(LogosError::PluginCallFailed {
                plugin: plugin.clone(),
                method: method.clone(),
                message: message.to_string(),
            }),
            None => Ok(value),
        }
    } else {
        let message = serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
            .unwrap_or(raw);
        Err(LogosError::PluginCallFailed { plugin, method, message })
    };

    callback(result);
    // `_client` (the held client share) drops here, after the callback ran.
}

/// A handle on another module.
///
/// Deliberately carries **no call policy**. A timeout is an argument to the
/// call that wants it (`*_with_timeout`), never a property of the proxy, so
/// two calls through the same proxy can want different bounds — which is the
/// normal case, since "how long is too long" is a fact about the method being
/// called, not about the module hosting it.
pub struct PluginProxy {
    plugin_name: String,
    client: Option<Arc<ClientHandle>>,
}

impl PluginProxy {
    pub(crate) fn new(plugin_name: impl Into<String>) -> Self {
        let plugin_name = plugin_name.into();
        // Share ONE lp_client per (origin, target) across all proxies for that
        // target, so a concurrent fan-out coalesces to a single capability
        // handshake instead of racing N. See client_cache()/shared_client().
        let client = shared_client(&plugin_name);
        PluginProxy { plugin_name, client }
    }

    pub fn name(&self) -> &str {
        &self.plugin_name
    }

    fn client(&self) -> Result<*mut ffi::LpClient, LogosError> {
        match &self.client {
            Some(handle) => Ok(handle.client),
            None => Err(LogosError::Other(format!(
                "Failed to create protocol client for {}",
                self.plugin_name
            ))),
        }
    }

    fn args_json<T: ToParam>(&self, params: &[T]) -> Result<CString, LogosError> {
        let typed: Vec<Param> = params
            .iter()
            .enumerate()
            .map(|(i, p)| p.to_param(&format!("arg{}", i)))
            .collect();
        let json = params_to_lp_args(&typed).map_err(|e| LogosError::JsonError(e.to_string()))?;
        CString::new(json).map_err(LogosError::InvalidString)
    }

    /// Call a plugin method asynchronously.
    /// Returns a channel receiver that will yield the result once available.
    /// Requires the Qt event loop to be processing (it runs automatically inside
    /// a loaded Logos module process).
    ///
    /// Waits the protocol default (20s). To bound this one call, use
    /// [`call_with_timeout`](Self::call_with_timeout).
    pub fn call<T: ToParam>(&self, method: &str, params: &[T]) -> Result<Receiver<CallResult>, LogosError> {
        self.call_inner(method, params, TimeoutMs::DEFAULT)
    }

    /// [`call`](Self::call), bounded: this call — and only this call — gives up
    /// after `timeout` instead of the protocol default.
    ///
    /// Errors with [`LogosError::InvalidTimeout`] if `timeout` cannot be
    /// expressed on the ABI; see [`call_json_with_timeout`](Self::call_json_with_timeout).
    pub fn call_with_timeout<T: ToParam>(
        &self,
        method: &str,
        params: &[T],
        timeout: Duration,
    ) -> Result<Receiver<CallResult>, LogosError> {
        self.call_inner(method, params, TimeoutMs::from_duration(timeout)?)
    }

    fn call_inner<T: ToParam>(
        &self,
        method: &str,
        params: &[T],
        timeout: TimeoutMs,
    ) -> Result<Receiver<CallResult>, LogosError> {
        let client = self.client()?;
        let method_c = CString::new(method)?;
        let args = self.args_json(params)?;

        let (rx, user_data, callback) = create_method_callback();
        unsafe {
            ffi::lp_invoke_async(
                client,
                method_c.as_ptr(),
                args.as_ptr(),
                timeout.as_abi(),
                callback,
                user_data,
            );
        }
        Ok(rx)
    }

    /// Call a plugin method with explicitly typed `Param` parameters, asynchronously.
    ///
    /// Waits the protocol default (20s); see
    /// [`call_with_params_with_timeout`](Self::call_with_params_with_timeout).
    pub fn call_with_params(
        &self,
        method: &str,
        params: &[Param],
    ) -> Result<Receiver<CallResult>, LogosError> {
        self.call_with_params_inner(method, params, TimeoutMs::DEFAULT)
    }

    /// [`call_with_params`](Self::call_with_params), bounded to `timeout` for
    /// this call only. (The name stutters because both halves are load-bearing:
    /// `_with_params` is the argument spelling, `_with_timeout` is the uniform
    /// suffix every bounded entry point in the SDK and in generated clients
    /// carries.)
    pub fn call_with_params_with_timeout(
        &self,
        method: &str,
        params: &[Param],
        timeout: Duration,
    ) -> Result<Receiver<CallResult>, LogosError> {
        self.call_with_params_inner(method, params, TimeoutMs::from_duration(timeout)?)
    }

    fn call_with_params_inner(
        &self,
        method: &str,
        params: &[Param],
        timeout: TimeoutMs,
    ) -> Result<Receiver<CallResult>, LogosError> {
        let client = self.client()?;
        let method_c = CString::new(method)?;
        let json = params_to_lp_args(params).map_err(|e| LogosError::JsonError(e.to_string()))?;
        let args = CString::new(json).map_err(LogosError::InvalidString)?;

        let (rx, user_data, callback) = create_method_callback();
        unsafe {
            ffi::lp_invoke_async(
                client,
                method_c.as_ptr(),
                args.as_ptr(),
                timeout.as_abi(),
                callback,
                user_data,
            );
        }
        Ok(rx)
    }

    /// Call a plugin method with no parameters, asynchronously.
    ///
    /// A spelling convenience over [`call`](Self::call) with an empty slice, so
    /// it has no bounded twin of its own — write
    /// `call_with_timeout(method, &[] as &[&str], timeout)`.
    pub fn call_no_params(&self, method: &str) -> Result<Receiver<CallResult>, LogosError> {
        let empty: &[&str] = &[];
        self.call(method, empty)
    }

    /// Call a plugin method synchronously.
    /// Suitable for use inside a `Q_INVOKABLE`-generated Rust function where the
    /// Qt event loop is already running in the module process.
    ///
    /// Waits the protocol default (20s); see
    /// [`call_sync_with_timeout`](Self::call_sync_with_timeout).
    pub fn call_sync<T: ToParam>(&self, method: &str, params: &[T]) -> Result<CallResult, LogosError> {
        self.call_sync_inner(method, params, TimeoutMs::DEFAULT)
    }

    /// [`call_sync`](Self::call_sync), bounded to `timeout` for this call only.
    pub fn call_sync_with_timeout<T: ToParam>(
        &self,
        method: &str,
        params: &[T],
        timeout: Duration,
    ) -> Result<CallResult, LogosError> {
        self.call_sync_inner(method, params, TimeoutMs::from_duration(timeout)?)
    }

    fn call_sync_inner<T: ToParam>(
        &self,
        method: &str,
        params: &[T],
        timeout: TimeoutMs,
    ) -> Result<CallResult, LogosError> {
        let client = self.client()?;
        let method_c = CString::new(method)?;
        let args = self.args_json(params)?;

        let mut result_json: *mut std::ffi::c_char = ptr::null_mut();
        let mut error_json: *mut std::ffi::c_char = ptr::null_mut();
        let rc = unsafe {
            ffi::lp_invoke(
                client,
                method_c.as_ptr(),
                args.as_ptr(),
                timeout.as_abi(),
                &mut result_json,
                &mut error_json,
            )
        };

        if rc != ffi::LP_OK {
            let message = if error_json.is_null() {
                format!("lp_invoke failed with code {}", rc)
            } else {
                let m = unsafe { CStr::from_ptr(error_json) }.to_string_lossy().into_owned();
                unsafe { ffi::lp_string_free(error_json) };
                m
            };
            return Ok(CallResult { success: false, message });
        }

        let (success, message) = if result_json.is_null() {
            (true, String::new())
        } else {
            let raw = unsafe { CStr::from_ptr(result_json) }.to_string_lossy().into_owned();
            unsafe { ffi::lp_string_free(result_json) };
            // `success` is this surface's error channel, so the rejection fold
            // belongs here too — reporting success for a call the provider
            // refused is the same defect on a different shape.
            match serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .as_ref()
                .and_then(crate::args::as_dispatch_rejection)
            {
                Some(message) => (false, message.to_string()),
                None => (true, json_to_message(&raw)),
            }
        };
        Ok(CallResult { success, message })
    }

    /// Call a plugin method synchronously with a raw JSON argument array,
    /// returning the raw JSON result value. This is the typed backbone the
    /// LIDL-generated wrappers build on (CallResult's `message` is a
    /// display string; typed callers need the JSON).
    ///
    /// Waits the protocol default (20s); the bounded twin is
    /// [`call_json_with_timeout`](Self::call_json_with_timeout), which is what
    /// a generated `<method>_with_timeout` calls.
    pub fn call_json(
        &self,
        method: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, LogosError> {
        self.call_json_inner(method, args, TimeoutMs::DEFAULT)
    }

    /// [`call_json`](Self::call_json), bounded: **this** call gives up after
    /// `timeout` instead of the protocol default (20s). Nothing is stored, so a
    /// second call through the same proxy is unaffected — that is the whole
    /// point of taking the timeout here rather than on the proxy.
    ///
    /// Errors with [`LogosError::InvalidTimeout`], before anything is sent, if
    /// `timeout` cannot be expressed on the `lp_*` ABI: sub-millisecond (where
    /// the ABI's millisecond `c_int` would round to `0`, its "use the default"
    /// sentinel — turning a 500µs bound into 20 seconds) or longer than
    /// `c_int::MAX` ms (~24.8 days). It is refused, never clamped.
    pub fn call_json_with_timeout(
        &self,
        method: &str,
        args: &serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, LogosError> {
        self.call_json_inner(method, args, TimeoutMs::from_duration(timeout)?)
    }

    fn call_json_inner(
        &self,
        method: &str,
        args: &serde_json::Value,
        timeout: TimeoutMs,
    ) -> Result<serde_json::Value, LogosError> {
        let client = self.client()?;
        let method_c = CString::new(method)?;
        let args_str = serde_json::to_string(args)
            .map_err(|e| LogosError::JsonError(e.to_string()))?;
        let args_c = CString::new(args_str).map_err(LogosError::InvalidString)?;

        let mut result_json: *mut std::ffi::c_char = ptr::null_mut();
        let mut error_json: *mut std::ffi::c_char = ptr::null_mut();
        let rc = unsafe {
            ffi::lp_invoke(
                client,
                method_c.as_ptr(),
                args_c.as_ptr(),
                timeout.as_abi(),
                &mut result_json,
                &mut error_json,
            )
        };

        if rc != ffi::LP_OK {
            let message = if error_json.is_null() {
                format!("lp_invoke failed with code {}", rc)
            } else {
                let m = unsafe { CStr::from_ptr(error_json) }.to_string_lossy().into_owned();
                unsafe { ffi::lp_string_free(error_json) };
                m
            };
            return Err(LogosError::PluginCallFailed {
                plugin: self.plugin_name.clone(),
                method: method.to_string(),
                message,
            });
        }

        let value = if result_json.is_null() {
            serde_json::Value::Null
        } else {
            let raw = unsafe { CStr::from_ptr(result_json) }.to_string_lossy().into_owned();
            unsafe { ffi::lp_string_free(result_json) };
            serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw))
        };
        // A provider that RAN and refused answers the canonical rejection
        // object as its RESULT, so rc is LP_OK and the decode above produced a
        // map. Fold it into the error channel the caller already reads, or the
        // typed wrapper downstream turns the refusal into a default value and
        // the caller never learns the call failed. Same fold the C++ generated
        // wrappers do; this one lives in the SDK rather than in generated code,
        // so no module has to be regenerated to get it.
        if let Some(message) = crate::args::as_dispatch_rejection(&value) {
            return Err(LogosError::PluginCallFailed {
                plugin: self.plugin_name.clone(),
                method: method.to_string(),
                message: message.to_string(),
            });
        }
        Ok(value)
    }

    /// Call a plugin method with no parameters, synchronously.
    ///
    /// A spelling convenience over [`call_sync`](Self::call_sync) with an empty
    /// slice, so it has no bounded twin of its own — write
    /// `call_sync_with_timeout(method, &[] as &[&str], timeout)`.
    pub fn call_sync_no_params(&self, method: &str) -> Result<CallResult, LogosError> {
        let empty: &[&str] = &[];
        self.call_sync(method, empty)
    }

    /// Call a plugin method asynchronously with a raw JSON argument array,
    /// delivering the raw JSON result value (or an error) to `callback`.
    ///
    /// This is the async twin of [`call_json`](Self::call_json) and the typed
    /// backbone the LIDL-generated `<method>_async` wrappers build on — the
    /// Rust analog of the C++ client's `<method>Async(..., callback)`. The
    /// callback is invoked **exactly once**: synchronously here if the call
    /// can't even be dispatched (bad client / arguments), otherwise from the
    /// protocol stack's completion path once the result lands — inside a
    /// loaded module that is the module's Qt event loop, so it fires after
    /// control returns to the loop (it will NOT complete while you block the
    /// current method).
    ///
    /// The in-flight call holds its own share of the client, so it completes
    /// even if `self` is a temporary (e.g. `modules().dep`) dropped at the end
    /// of the call statement.
    ///
    /// Waits the protocol default (20s); the bounded twin is
    /// [`call_json_async_with_timeout`](Self::call_json_async_with_timeout),
    /// which is what a generated `<method>_async_with_timeout` calls.
    pub fn call_json_async<F>(&self, method: &str, args: &serde_json::Value, callback: F)
    where
        F: FnOnce(Result<serde_json::Value, LogosError>) + Send + 'static,
    {
        self.call_json_async_inner(method, args, TimeoutMs::DEFAULT, callback)
    }

    /// [`call_json_async`](Self::call_json_async), bounded: **this** call gives
    /// up after `timeout`. Nothing is stored on the proxy, so a second async
    /// call through it is unaffected.
    ///
    /// A `timeout` the ABI cannot express is reported the way every other
    /// undispatchable call is on this surface — [`LogosError::InvalidTimeout`]
    /// delivered to `callback`, synchronously, exactly once, with nothing sent.
    /// (Returning a `Result` here instead would have made this the one async
    /// entry point with two error channels.)
    pub fn call_json_async_with_timeout<F>(
        &self,
        method: &str,
        args: &serde_json::Value,
        timeout: Duration,
        callback: F,
    ) where
        F: FnOnce(Result<serde_json::Value, LogosError>) + Send + 'static,
    {
        let timeout = match TimeoutMs::from_duration(timeout) {
            Ok(t) => t,
            Err(e) => return callback(Err(e)),
        };
        self.call_json_async_inner(method, args, timeout, callback)
    }

    fn call_json_async_inner<F>(
        &self,
        method: &str,
        args: &serde_json::Value,
        timeout: TimeoutMs,
        callback: F,
    ) where
        F: FnOnce(Result<serde_json::Value, LogosError>) + Send + 'static,
    {
        // Read before the callback is moved into the boxed state — Copy, so
        // this is just the call's timeout captured for the ABI call below.
        let timeout = timeout.as_abi();
        let client_handle = match &self.client {
            Some(h) => Arc::clone(h),
            None => {
                callback(Err(LogosError::Other(format!(
                    "Failed to create protocol client for {}",
                    self.plugin_name
                ))));
                return;
            }
        };
        let method_c = match CString::new(method) {
            Ok(c) => c,
            Err(e) => return callback(Err(LogosError::InvalidString(e))),
        };
        let args_str = match serde_json::to_string(args) {
            Ok(s) => s,
            Err(e) => return callback(Err(LogosError::JsonError(e.to_string()))),
        };
        let args_c = match CString::new(args_str) {
            Ok(c) => c,
            Err(e) => return callback(Err(LogosError::InvalidString(e))),
        };

        let state = Box::new(AsyncCallState {
            callback: Box::new(callback),
            plugin: self.plugin_name.clone(),
            method: method.to_string(),
            _client: Arc::clone(&client_handle),
        });
        let user_data = Box::into_raw(state) as *mut c_void;
        unsafe {
            ffi::lp_invoke_async(
                client_handle.client,
                method_c.as_ptr(),
                args_c.as_ptr(),
                timeout,
                async_call_trampoline,
                user_data,
            );
        }
    }

    /// Subscribe to events from a plugin.
    ///
    /// Returns an [`EventSubscription`]: a channel of incoming `EventData`
    /// that OWNS the underlying lp subscription and a share of the client, so
    /// it stays live after the proxy is dropped — including when moved into a
    /// listener thread (the handle is `Send`). Drop it to unsubscribe.
    pub fn on(&mut self, event: &str) -> Result<EventSubscription, LogosError> {
        let client_handle = match &self.client {
            Some(h) => Arc::clone(h),
            None => {
                return Err(LogosError::Other(format!(
                    "Failed to create protocol client for {}",
                    self.plugin_name
                )))
            }
        };
        let event_c = CString::new(event)?;

        let (rx, callback_data, callback) = create_event_callback(event);
        let user_data = event_callback_ptr(&callback_data);

        let sub =
            unsafe { ffi::lp_subscribe(client_handle.client, event_c.as_ptr(), callback, user_data) };
        if sub.is_null() {
            return Err(LogosError::EventListenerFailed {
                plugin: self.plugin_name.clone(),
                event: event.to_string(),
                message: "lp_subscribe returned null".to_string(),
            });
        }

        Ok(EventSubscription {
            rx,
            sub,
            _callback: callback_data,
            _client: client_handle,
        })
    }

    /// Watch this TARGET MODULE's subscription transitions.
    ///
    /// Per module, not per subscription: every subscription through this proxy
    /// shares the provider's single handle, so they are lost and re-established
    /// together. A per-subscription watcher would have reported one event once
    /// per subscription.
    ///
    /// The pair worth watching for is `Lost` -> `Armed` with a HIGHER
    /// generation: the provider restarted, so every subscription is new and
    /// everything emitted in between is unrecoverable. Under
    /// [`RestartPolicy::Manual`] you get `Held` instead and it stays down until
    /// [`PluginProxy::rearm_subscriptions`].
    ///
    /// Installable before the first [`PluginProxy::on`], and replays the
    /// current state, so there is no order in which the arm can be missed.
    ///
    /// Warns once against a runtime older than logos-protocol 0.9 rather than
    /// degrading silently.
    pub fn on_subscription_status<F>(&mut self, f: F) -> Result<(), LogosError>
    where
        F: Fn(SubStatus, u64) + Send + Sync + 'static,
    {
        let handle = match &self.client {
            Some(h) => Arc::clone(h),
            None => {
                return Err(LogosError::Other(format!(
                    "Failed to create protocol client for {}",
                    self.plugin_name
                )))
            }
        };
        if !protocol_at_least(0, 9) {
            warn_once_no_status();
            return Ok(());
        }
        {
            let mut guard = handle
                .status
                .lock()
                .map_err(|_| LogosError::Other("subscription status lock poisoned".into()))?;
            *guard = Some(Arc::new(f));
        }
        // `Arc::as_ptr`, so the trampoline reads the same allocation the client
        // is destroyed from. Installed AFTER the closure is in place: the C
        // side replays the current state synchronously from inside this call.
        let user_data = Arc::as_ptr(&handle) as *mut std::os::raw::c_void;
        unsafe {
            ffi::lp_client_set_subscription_status_cb(
                handle.client,
                Some(status_trampoline as ffi::LpSubStatusCb),
                user_data,
            );
        }
        Ok(())
    }

    /// Which establishment this module is on: 0 = never armed, 1 = the first,
    /// N+1 after each re-establishment.
    ///
    /// Reading this beside each received event and seeing it change IS the gap
    /// detector — no status callback required. Events emitted while it was down
    /// reached nobody and cannot be recovered.
    ///
    /// Returns 0 against a runtime older than logos-protocol 0.9.
    pub fn subscription_generation(&mut self) -> u64 {
        if !protocol_at_least(0, 9) {
            return 0;
        }
        match self.client() {
            Ok(c) => unsafe { ffi::lp_client_subscription_generation(c) },
            Err(_) => 0,
        }
    }

    /// What happens to this module's subscriptions when its provider goes away.
    ///
    /// `Manual` means "do not RE-arm after a loss", never "do not arm the first
    /// time" — a subscription taken before the provider is reachable is
    /// deferred and armed under either policy.
    ///
    /// Warns once against a runtime older than logos-protocol 0.9.
    pub fn set_restart_policy(&mut self, policy: RestartPolicy) -> Result<(), LogosError> {
        if !protocol_at_least(0, 9) {
            if policy == RestartPolicy::Manual {
                warn_once_no_restart();
            }
            return Ok(());
        }
        let c = self.client()?;
        let json = match policy {
            RestartPolicy::Manual => CString::new(r#"{"restart":"manual"}"#)?,
            _ => CString::new(r#"{"restart":"automatic"}"#)?,
        };
        unsafe { ffi::lp_client_set_subscription_options(c, json.as_ptr()) };
        Ok(())
    }

    /// Revive this module's held subscriptions.
    ///
    /// Safe to call from inside the status callback: the ABI posts the revive
    /// to the client's owner thread rather than marshalling it synchronously,
    /// which is what keeps it from inverting against the delivery guard.
    ///
    /// Answers "accepted", not "re-armed" — watch for `Armed` at a higher
    /// generation for that. False if nothing is held, or against a runtime
    /// older than logos-protocol 0.9.
    pub fn rearm_subscriptions(&mut self) -> bool {
        if !protocol_at_least(0, 9) {
            return false;
        }
        match self.client() {
            Ok(c) => unsafe { ffi::lp_client_rearm_subscriptions(c) != 0 },
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    /// The claim the whole default path rests on: a call site that asks for no
    /// timeout hands the ABI the same `0` it always did, which
    /// `logos_protocol.cpp`'s `lpTimeout()` maps to `Timeout()` — the 20s
    /// default. If this ever became a positive number, every existing caller
    /// would silently acquire a bound it never asked for.
    #[test]
    fn default_is_the_abi_default_sentinel() {
        assert_eq!(TimeoutMs::DEFAULT.as_abi(), 0);
        assert!(TimeoutMs::DEFAULT.as_abi() <= 0, "must select the protocol default");
    }

    #[test]
    fn duration_becomes_whole_milliseconds() {
        for (d, ms) in [
            (Duration::from_millis(1), 1),
            (Duration::from_millis(500), 500),
            (Duration::from_secs(1), 1000),
            (Duration::from_secs(90), 90_000),
        ] {
            let t = TimeoutMs::from_duration(d).expect("in range");
            assert_eq!(t.as_abi(), ms, "{:?}", d);
        }
        // Sub-millisecond remainders truncate toward the whole millisecond the
        // ABI can carry; only a value that truncates to ZERO is refused.
        assert_eq!(
            TimeoutMs::from_duration(Duration::from_micros(1_500)).unwrap().as_abi(),
            1
        );
    }

    /// A sub-millisecond timeout must NOT round to 0 — on this ABI 0 means
    /// "use the default", so rounding would turn a 500µs bound into 20s.
    #[test]
    fn sub_millisecond_is_refused_not_rounded_into_the_default() {
        for d in [Duration::ZERO, Duration::from_nanos(1), Duration::from_micros(999)] {
            match TimeoutMs::from_duration(d) {
                Err(LogosError::InvalidTimeout { timeout, .. }) => assert_eq!(timeout, d),
                other => panic!("expected InvalidTimeout for {:?}, got {:?}", d, other.map(|t| t.as_abi())),
            }
        }
    }

    /// Anything past `c_int::MAX` ms must be refused, not saturated and not
    /// truncated: `(i32::MAX as u64 + 1) as i32` is `i32::MIN`, i.e. negative,
    /// i.e. the 20s default — the exact quiet wrong answer.
    #[test]
    fn out_of_range_is_refused_not_saturated_or_wrapped() {
        let max_ok = Duration::from_millis(c_int::MAX as u64);
        assert_eq!(TimeoutMs::from_duration(max_ok).unwrap().as_abi(), c_int::MAX);

        for d in [
            Duration::from_millis(c_int::MAX as u64 + 1),
            Duration::from_secs(60 * 60 * 24 * 30), // 30 days
            Duration::MAX,
        ] {
            match TimeoutMs::from_duration(d) {
                Err(LogosError::InvalidTimeout { timeout, .. }) => assert_eq!(timeout, d),
                other => panic!(
                    "expected InvalidTimeout for {:?}, got {:?}",
                    d,
                    other.map(|t| t.as_abi())
                ),
            }
        }
    }

    /// The refusal has to say what went wrong; an opaque error would just move
    /// the surprise from runtime to the log.
    #[test]
    fn refusal_explains_itself() {
        let too_small = TimeoutMs::from_duration(Duration::from_micros(1)).unwrap_err().to_string();
        assert!(too_small.contains("1ms"), "{}", too_small);
        assert!(too_small.contains("DEFAULT"), "{}", too_small);

        let too_big = TimeoutMs::from_duration(Duration::MAX).unwrap_err().to_string();
        assert!(too_big.contains("clamped"), "{}", too_big);
    }
}

#[cfg(test)]
mod origin_tests {
    use super::*;
    use crate::api::{module_origin, set_module_origin};

    /// ONE test, deliberately, for a process-global `OnceLock`: the states are
    /// ordered (unset → set → contested) and `cargo test` runs test functions
    /// concurrently in a single process, so splitting them would make the
    /// "unset" assertion race whichever test set it first. Nothing else in the
    /// crate touches the origin, so the sequence below is the whole lifecycle.
    ///
    /// What this pins, and why each half matters:
    ///
    ///  * UNSET reads as EMPTY, not as a guess. `shared_client` hands this
    ///    string to `lp_client_create` as the origin, and the origin is the
    ///    identity `capability_module.requestModule` authenticates. An empty
    ///    one is refused there by name ("rejecting empty module name") — fail
    ///    closed. The alternative, a plausible-looking default, is the very
    ///    defect this replaces: the default that was there was "core", a
    ///    `TokenManager::bootstrapKeys()` anchor, so every Rust module in the
    ///    fleet announced itself as the HOST.
    ///
    ///  * A REPEAT of the same name succeeds. The generated scaffold calls
    ///    `set_module_origin` from `ensure_ready`, i.e. on every C-ABI entry
    ///    point, so "already set to this" is the common case and must not read
    ///    as a failure.
    ///
    ///  * A DIFFERENT name is refused AND does not take effect. One cdylib is
    ///    one module; a second identity latching over the first would re-key
    ///    every client created after it, and silently.
    #[test]
    fn the_origin_is_declared_once_and_never_guessed() {
        assert_eq!(module_origin(), None, "nothing may pre-set the origin");
        assert_eq!(
            outbound_origin(),
            "",
            "an undeclared origin must be empty (fail closed), never a default name"
        );

        assert!(set_module_origin("calc_aggregator"), "first declaration");
        assert_eq!(module_origin(), Some("calc_aggregator"));
        assert_eq!(outbound_origin(), "calc_aggregator");

        assert!(
            set_module_origin("calc_aggregator"),
            "re-declaring the SAME name is what the scaffold does on every entry point"
        );
        assert_eq!(outbound_origin(), "calc_aggregator");

        assert!(
            !set_module_origin("core"),
            "a second, different identity must be refused"
        );
        assert_eq!(
            outbound_origin(),
            "calc_aggregator",
            "a refused re-declaration must not take effect"
        );
    }
}
