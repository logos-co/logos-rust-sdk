//! Plugin proxy for method calls and event subscriptions.

use std::ffi::CString;
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

    pub fn call<T: ToParam>(&self, method: &str, params: &[T]) -> Result<Receiver<CallResult>, LogosError> {
        let plugin_name_c = CString::new(self.plugin_name.as_str())?;
        let method_c = CString::new(method)?;
        let params_json = params_to_json(params)?;
        let params_json_c = CString::new(params_json)?;

        let (rx, user_data, callback) = create_method_callback();

        unsafe {
            ffi::logos_core_call_plugin_method_async(
                plugin_name_c.as_ptr(),
                method_c.as_ptr(),
                params_json_c.as_ptr(),
                callback,
                user_data,
            );
        }

        Ok(rx)
    }

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
            ffi::logos_core_call_plugin_method_async(
                plugin_name_c.as_ptr(),
                method_c.as_ptr(),
                params_json_c.as_ptr(),
                callback,
                user_data,
            );
        }

        Ok(rx)
    }

    pub fn call_no_params(&self, method: &str) -> Result<Receiver<CallResult>, LogosError> {
        let empty: &[&str] = &[];
        self.call(method, empty)
    }

    /// Blocks until the result is received. Requires `process_events()` to be called from another thread.
    pub fn call_sync<T: ToParam>(&self, method: &str, params: &[T]) -> Result<CallResult, LogosError> {
        let rx = self.call(method, params)?;
        rx.recv().map_err(|_| LogosError::ChannelClosed)
    }

    pub fn on(&mut self, event: &str) -> Result<Receiver<EventData>, LogosError> {
        let plugin_name_c = CString::new(self.plugin_name.as_str())?;
        let event_c = CString::new(event)?;

        let (rx, callback_data, callback) = create_event_callback(event);
        let user_data = event_callback_ptr(&callback_data);

        unsafe {
            ffi::logos_core_register_event_listener(
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

