//! Plugin proxy for method calls and event subscriptions.

use std::ffi::{CStr, CString};
use std::sync::mpsc::Receiver;

use crate::callback::{
    create_event_callback, create_method_callback, event_callback_ptr,
    CallResult, EventCallbackData, EventData,
};
use crate::error::LogosError;
use crate::ffi;
use crate::params::{params_to_json, Param, ToParam};

pub struct PluginProxy {
    plugin_name: String,
    event_callbacks: Vec<Box<EventCallbackData>>,
}

impl PluginProxy {
    pub(crate) fn new(plugin_name: impl Into<String>) -> Self {
        PluginProxy {
            plugin_name: plugin_name.into(),
            event_callbacks: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.plugin_name
    }

    /// Call a plugin method asynchronously.
    /// Returns a channel receiver that will yield the result once available.
    /// Requires the Qt event loop to be processing (it runs automatically inside
    /// a loaded Logos module process).
    pub fn call<T: ToParam>(&self, method: &str, params: &[T]) -> Result<Receiver<CallResult>, LogosError> {
        let plugin_name_c = CString::new(self.plugin_name.as_str())?;
        let method_c = CString::new(method)?;
        let params_json = params_to_json(params)?;
        let params_json_c = CString::new(params_json)?;

        let (rx, user_data, callback) = create_method_callback();

        unsafe {
            ffi::logos_sdk_call_method_async(
                plugin_name_c.as_ptr(),
                method_c.as_ptr(),
                params_json_c.as_ptr(),
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
        let plugin_name_c = CString::new(self.plugin_name.as_str())?;
        let method_c = CString::new(method)?;
        let params_json = serde_json::to_string(params)?;
        let params_json_c = CString::new(params_json)?;

        let (rx, user_data, callback) = create_method_callback();

        unsafe {
            ffi::logos_sdk_call_method_async(
                plugin_name_c.as_ptr(),
                method_c.as_ptr(),
                params_json_c.as_ptr(),
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
        let plugin_name_c = CString::new(self.plugin_name.as_str())?;
        let method_c = CString::new(method)?;
        let params_json = params_to_json(params)?;
        let params_json_c = CString::new(params_json)?;

        let raw = unsafe {
            ffi::logos_sdk_call_method_sync(
                plugin_name_c.as_ptr(),
                method_c.as_ptr(),
                params_json_c.as_ptr(),
            )
        };

        if raw.is_null() {
            return Ok(CallResult {
                success: false,
                message: format!(
                    "Synchronous call to {}.{}() returned null",
                    self.plugin_name, method
                ),
            });
        }

        let message = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();

        unsafe {
            ffi::logos_sdk_free_string(raw);
        }

        Ok(CallResult {
            success: true,
            message,
        })
    }

    /// Call a plugin method synchronously with explicitly typed `Param`
    /// parameters (e.g. mixing a string and a `Param::bytes`).
    /// See [`call_sync`](Self::call_sync) for the synchronous-call contract.
    pub fn call_sync_with_params(
        &self,
        method: &str,
        params: &[Param],
    ) -> Result<CallResult, LogosError> {
        let plugin_name_c = CString::new(self.plugin_name.as_str())?;
        let method_c = CString::new(method)?;
        let params_json = serde_json::to_string(params)?;
        let params_json_c = CString::new(params_json)?;

        let raw = unsafe {
            ffi::logos_sdk_call_method_sync(
                plugin_name_c.as_ptr(),
                method_c.as_ptr(),
                params_json_c.as_ptr(),
            )
        };

        if raw.is_null() {
            return Ok(CallResult {
                success: false,
                message: format!(
                    "Synchronous call to {}.{}() returned null",
                    self.plugin_name, method
                ),
            });
        }

        let message = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();

        unsafe {
            ffi::logos_sdk_free_string(raw);
        }

        Ok(CallResult {
            success: true,
            message,
        })
    }

    /// Call a plugin method with no parameters, synchronously.
    pub fn call_sync_no_params(&self, method: &str) -> Result<CallResult, LogosError> {
        let empty: &[&str] = &[];
        self.call_sync(method, empty)
    }

    /// Subscribe to events from a plugin.
    /// Returns a channel receiver that yields `EventData` each time the event fires.
    pub fn on(&mut self, event: &str) -> Result<Receiver<EventData>, LogosError> {
        let plugin_name_c = CString::new(self.plugin_name.as_str())?;
        let event_c = CString::new(event)?;

        let (rx, callback_data, callback) = create_event_callback(event);
        let user_data = event_callback_ptr(&callback_data);

        unsafe {
            ffi::logos_sdk_register_event(
                plugin_name_c.as_ptr(),
                event_c.as_ptr(),
                callback,
                user_data,
            );
        }

        self.event_callbacks.push(callback_data);

        Ok(rx)
    }
}
