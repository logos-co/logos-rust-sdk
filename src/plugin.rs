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

    /// The timeout as a `Duration`, or `None` when it defers to the protocol
    /// default (the `<= 0` sentinel).
    pub(crate) fn as_duration(self) -> Option<Duration> {
        (self.0 > 0).then(|| Duration::from_millis(self.0 as u64))
    }
}

/// Shared ownership of the underlying `lp_client`. The client is destroyed
/// when the LAST owner drops — the proxy itself or any live subscription.
/// lp_* handles are thread-safe per-handle (the logos_protocol.h threading
/// contract), so sharing the raw handle across threads is sound.
struct ClientHandle(*mut ffi::LpClient);
unsafe impl Send for ClientHandle {}
unsafe impl Sync for ClientHandle {}
impl Drop for ClientHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ffi::lp_client_destroy(self.0) };
        }
    }
}

/// Process-global cache of ONE shared `lp_client` per target module name.
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
fn client_cache() -> &'static Mutex<HashMap<String, Weak<ClientHandle>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Weak<ClientHandle>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get-or-create the shared client for `target` (origin is always "core" here).
/// Returns None on the same failure surface as before (bad name / null client).
fn shared_client(target: &str) -> Option<Arc<ClientHandle>> {
    let mut map = client_cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = map.get(target).and_then(Weak::upgrade) {
        return Some(existing); // a live client for this target — reuse it
    }
    // None live: create one, cache a Weak, hand back the strong Arc. The lp
    // call happens here under the (uncontended, I/O-free) map lock; the actual
    // invoke runs later off the proxy's cloned Arc, outside the lock.
    let target_c = CString::new(target).ok()?;
    let origin = CString::new("core").unwrap();
    let raw = unsafe {
        ffi::lp_client_create(target_c.as_ptr(), origin.as_ptr(), ptr::null(), ptr::null())
    };
    if raw.is_null() {
        return None;
    }
    let handle = Arc::new(ClientHandle(raw));
    map.insert(target.to_string(), Arc::downgrade(&handle));
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
        Ok(serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw)))
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

pub struct PluginProxy {
    plugin_name: String,
    client: Option<Arc<ClientHandle>>,
    /// The timeout every call through THIS proxy carries.
    /// [`TimeoutMs::DEFAULT`] unless a timeout-scoped view was taken with
    /// [`PluginProxy::with_timeout`].
    timeout: TimeoutMs,
}

impl PluginProxy {
    pub(crate) fn new(plugin_name: impl Into<String>) -> Self {
        let plugin_name = plugin_name.into();
        // Share ONE lp_client per target (origin "core") across all proxies for
        // that target, so a concurrent fan-out coalesces to a single capability
        // handshake instead of racing N. See client_cache()/shared_client().
        let client = shared_client(&plugin_name);
        PluginProxy {
            plugin_name,
            client,
            timeout: TimeoutMs::DEFAULT,
        }
    }

    pub fn name(&self) -> &str {
        &self.plugin_name
    }

    /// A **timeout-scoped view** of this proxy: every call made through the
    /// returned proxy — sync and async, typed and untyped — carries `timeout`
    /// instead of the protocol default (20s).
    ///
    /// The returned proxy shares the SAME underlying `lp_client` (no second
    /// capability handshake, no second connection); only the timeout differs.
    /// `self` is left untouched, so a scoped timeout can never leak into a call
    /// that did not ask for one:
    ///
    /// ```ignore
    /// let quick = dep.with_timeout(Duration::from_millis(500))?;
    /// quick.call_json("slow", &args);   // gives up after ~500ms
    /// dep.call_json("slow", &args);     // still the 20s default
    /// ```
    ///
    /// This is how a per-call timeout reaches every entry point without adding
    /// a parameter to any of them — existing call sites keep compiling and keep
    /// the default.
    ///
    /// Fails with [`LogosError::InvalidTimeout`] if `timeout` cannot be
    /// expressed on the ABI (sub-millisecond, or longer than ~24.8 days). It is
    /// never silently clamped, and the error is raised where the bad value was
    /// supplied rather than on some later call.
    pub fn with_timeout(&self, timeout: Duration) -> Result<PluginProxy, LogosError> {
        Ok(PluginProxy {
            plugin_name: self.plugin_name.clone(),
            client: self.client.clone(),
            timeout: TimeoutMs::from_duration(timeout)?,
        })
    }

    /// The timeout in force for calls made through this proxy, or `None` when
    /// it defers to the protocol default.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout.as_duration()
    }

    fn client(&self) -> Result<*mut ffi::LpClient, LogosError> {
        match &self.client {
            Some(handle) => Ok(handle.0),
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
    pub fn call<T: ToParam>(&self, method: &str, params: &[T]) -> Result<Receiver<CallResult>, LogosError> {
        let client = self.client()?;
        let method_c = CString::new(method)?;
        let args = self.args_json(params)?;

        let (rx, user_data, callback) = create_method_callback();
        unsafe {
            ffi::lp_invoke_async(
                client,
                method_c.as_ptr(),
                args.as_ptr(),
                self.timeout.as_abi(),
                callback,
                user_data,
            );
        }
        Ok(rx)
    }

    /// Call a plugin method with explicitly typed `Param` parameters, asynchronously.
    pub fn call_with_params(
        &self,
        method: &str,
        params: &[Param],
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
                self.timeout.as_abi(),
                callback,
                user_data,
            );
        }
        Ok(rx)
    }

    /// Call a plugin method with no parameters, asynchronously.
    pub fn call_no_params(&self, method: &str) -> Result<Receiver<CallResult>, LogosError> {
        let empty: &[&str] = &[];
        self.call(method, empty)
    }

    /// Call a plugin method synchronously.
    /// Suitable for use inside a `Q_INVOKABLE`-generated Rust function where the
    /// Qt event loop is already running in the module process.
    pub fn call_sync<T: ToParam>(&self, method: &str, params: &[T]) -> Result<CallResult, LogosError> {
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
                self.timeout.as_abi(),
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

        let message = if result_json.is_null() {
            String::new()
        } else {
            let raw = unsafe { CStr::from_ptr(result_json) }.to_string_lossy().into_owned();
            unsafe { ffi::lp_string_free(result_json) };
            json_to_message(&raw)
        };
        Ok(CallResult { success: true, message })
    }

    /// Call a plugin method synchronously with a raw JSON argument array,
    /// returning the raw JSON result value. This is the typed backbone the
    /// LIDL-generated wrappers build on (CallResult's `message` is a
    /// display string; typed callers need the JSON).
    pub fn call_json(
        &self,
        method: &str,
        args: &serde_json::Value,
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
                self.timeout.as_abi(),
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
        Ok(value)
    }

    /// Call a plugin method with no parameters, synchronously.
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
    pub fn call_json_async<F>(&self, method: &str, args: &serde_json::Value, callback: F)
    where
        F: FnOnce(Result<serde_json::Value, LogosError>) + Send + 'static,
    {
        // Read before the callback is moved into the boxed state — Copy, so
        // this is just the proxy's timeout captured for the ABI call below.
        let timeout = self.timeout.as_abi();
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
                client_handle.0,
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
            unsafe { ffi::lp_subscribe(client_handle.0, event_c.as_ptr(), callback, user_data) };
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
        assert_eq!(TimeoutMs::DEFAULT.as_duration(), None);
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
            assert_eq!(t.as_duration(), Some(Duration::from_millis(ms as u64)));
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
