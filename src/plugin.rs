//! Plugin proxy for method calls and event subscriptions, over the lp_* C ABI.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::mpsc::Receiver;

use crate::callback::{
    create_event_callback, create_method_callback, event_callback_ptr, json_to_message,
    CallResult, EventCallbackData, EventData,
};
use crate::error::LogosError;
use crate::ffi;
use crate::params::{params_to_lp_args, Param, ToParam};

pub struct PluginProxy {
    plugin_name: String,
    client: *mut ffi::LpClient,
    subscriptions: Vec<*mut ffi::LpSubscription>,
    event_callbacks: Vec<Box<EventCallbackData>>,
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
            client,
            subscriptions: Vec::new(),
            event_callbacks: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.plugin_name
    }

    fn client(&self) -> Result<*mut ffi::LpClient, LogosError> {
        if self.client.is_null() {
            return Err(LogosError::Other(format!(
                "Failed to create protocol client for {}",
                self.plugin_name
            )));
        }
        Ok(self.client)
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

    /// Subscribe to events from a plugin.
    /// Returns a channel receiver that yields `EventData` each time the event fires.
    pub fn on(&mut self, event: &str) -> Result<Receiver<EventData>, LogosError> {
        let client = self.client()?;
        let event_c = CString::new(event)?;

        let (rx, callback_data, callback) = create_event_callback(event);
        let user_data = event_callback_ptr(&callback_data);

        let sub = unsafe { ffi::lp_subscribe(client, event_c.as_ptr(), callback, user_data) };
        if sub.is_null() {
            return Err(LogosError::EventListenerFailed {
                plugin: self.plugin_name.clone(),
                event: event.to_string(),
                message: "lp_subscribe returned null".to_string(),
            });
        }

        self.subscriptions.push(sub);
        self.event_callbacks.push(callback_data);
        Ok(rx)
    }
}

impl Drop for PluginProxy {
    fn drop(&mut self) {
        // Cancel subscriptions first (after lp_unsubscribe returns the
        // callback will not fire again), then destroy the client.
        for sub in self.subscriptions.drain(..) {
            unsafe { ffi::lp_unsubscribe(sub) };
        }
        if !self.client.is_null() {
            unsafe { ffi::lp_client_destroy(self.client) };
        }
    }
}
