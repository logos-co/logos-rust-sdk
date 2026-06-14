//! Plugin proxy for method calls and event subscriptions, over the lp_* C ABI.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use crate::callback::{
    create_event_callback, create_method_callback, event_callback_ptr, json_to_message,
    CallResult, EventCallbackData, EventData,
};
use crate::error::LogosError;
use crate::ffi;
use crate::params::{params_to_lp_args, Param, ToParam};

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
}

impl PluginProxy {
    pub(crate) fn new(plugin_name: impl Into<String>) -> Self {
        let plugin_name = plugin_name.into();
        // The facade's historical origin identity is "core" (matches the
        // previous logos_sdk_* behavior inside a module process).
        let client = CString::new(plugin_name.as_str())
            .ok()
            .map(|target| {
                let origin = CString::new("core").unwrap();
                unsafe {
                    ffi::lp_client_create(
                        target.as_ptr(),
                        origin.as_ptr(),
                        ptr::null(),
                        ptr::null(),
                    )
                }
            })
            .unwrap_or(ptr::null_mut());
        PluginProxy {
            plugin_name,
            client: if client.is_null() {
                None
            } else {
                Some(Arc::new(ClientHandle(client)))
            },
        }
    }

    pub fn name(&self) -> &str {
        &self.plugin_name
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
            ffi::lp_invoke_async(client, method_c.as_ptr(), args.as_ptr(), 0, callback, user_data);
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
            ffi::lp_invoke_async(client, method_c.as_ptr(), args.as_ptr(), 0, callback, user_data);
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
        self.call_sync_timeout_ms(method, params, 0)
    }

    /// Call a plugin method synchronously, bounding the wait by `timeout`.
    ///
    /// The timeout overrides the transport default for this one call — use it
    /// for a method whose reply legitimately takes longer than the default,
    /// e.g. a network-bootstrap handshake. A zero or out-of-range `timeout`
    /// falls back to the transport default (`lp_invoke` treats `timeout_ms <= 0`
    /// that way).
    pub fn call_sync_with_timeout<T: ToParam>(
        &self,
        method: &str,
        params: &[T],
        timeout: Duration,
    ) -> Result<CallResult, LogosError> {
        self.call_sync_timeout_ms(method, params, duration_to_timeout_ms(timeout))
    }

    fn call_sync_timeout_ms<T: ToParam>(
        &self,
        method: &str,
        params: &[T],
        timeout_ms: c_int,
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
                timeout_ms,
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
                0,
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
                0,
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

/// Convert a `Duration` to the `lp_invoke` `timeout_ms` argument, saturating at
/// the positive `c_int` range. `lp_invoke` treats `timeout_ms <= 0` as "use the
/// transport default", so a zero duration naturally selects it.
fn duration_to_timeout_ms(timeout: Duration) -> c_int {
    timeout.as_millis().min(c_int::MAX as u128) as c_int
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_to_timeout_ms_passes_through_zeroes_and_saturates() {
        assert_eq!(duration_to_timeout_ms(Duration::from_secs(35)), 35_000);
        // Zero falls back to the transport default (lp_invoke treats <= 0 that way).
        assert_eq!(duration_to_timeout_ms(Duration::ZERO), 0);
        // Out-of-range durations saturate rather than wrapping to a negative.
        assert_eq!(duration_to_timeout_ms(Duration::MAX), c_int::MAX);
    }
}
