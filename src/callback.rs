//! Callback management for bridging C callbacks to Rust channels.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::mpsc::{self, Receiver, Sender};

use serde::{Deserialize, Serialize};

use crate::ffi::{LpEventCb, LpResultCb};

#[derive(Debug, Clone)]
pub struct CallResult {
    pub success: bool,
    pub message: String,
}

impl CallResult {
    pub fn ok(message: impl Into<String>) -> Self {
        CallResult {
            success: true,
            message: message.into(),
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        CallResult {
            success: false,
            message: message.into(),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.success
    }

    pub fn is_err(&self) -> bool {
        !self.success
    }

    pub fn into_result(self) -> Result<String, String> {
        if self.success {
            Ok(self.message)
        } else {
            Err(self.message)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventData {
    pub event: String,
    pub data: serde_json::Value,
}

impl EventData {
    pub fn new(event: impl Into<String>, data: serde_json::Value) -> Self {
        EventData {
            event: event.into(),
            data,
        }
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn as_array(&self) -> Option<&Vec<serde_json::Value>> {
        self.data.as_array()
    }

    pub fn get(&self, index: usize) -> Option<&serde_json::Value> {
        self.data.as_array().and_then(|arr| arr.get(index))
    }

    pub fn get_str(&self, index: usize) -> Option<&str> {
        self.get(index).and_then(|v| v.as_str())
    }
}

pub(crate) struct CallbackData {
    pub tx: Sender<CallResult>,
}

pub(crate) struct EventCallbackData {
    pub tx: Sender<EventData>,
    pub event_name: String,
}

/// Historical message semantics: the result as a plain string — JSON
/// strings unquoted, null → "", anything else compact JSON. Keeps
/// `result.message.parse::<i64>()` etc. working across the lp_* move.
pub(crate) fn json_to_message(json: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(serde_json::Value::Null) => String::new(),
        Ok(v) => v.to_string(),
        Err(_) => json.to_owned(),
    }
}

pub(crate) extern "C" fn method_callback_trampoline(
    ok: c_int,
    json: *const c_char,
    user_data: *mut c_void,
) {
    if user_data.is_null() {
        return;
    }

    let callback_data = unsafe { Box::from_raw(user_data as *mut CallbackData) };

    let json_str = if json.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(json) }
            .to_string_lossy()
            .into_owned()
    };

    // A provider that RAN and refused answers the canonical rejection object as
    // its RESULT, so `ok` is non-zero and the message below would render the
    // rejection as if it were a value. `success` is this surface's error
    // channel; fold it there. Matches PluginProxy::call_json / call_sync.
    let rejection = if ok != 0 {
        serde_json::from_str::<serde_json::Value>(&json_str)
            .ok()
            .as_ref()
            .and_then(crate::args::as_dispatch_rejection)
            .map(String::from)
    } else {
        None
    };

    let call_result = match rejection {
        Some(message) => CallResult { success: false, message },
        None => CallResult {
            success: ok != 0,
            message: json_to_message(&json_str),
        },
    };

    let _ = callback_data.tx.send(call_result);
}

pub(crate) extern "C" fn event_callback_trampoline(
    event_name: *const c_char,
    data_json: *const c_char,
    user_data: *mut c_void,
) {
    if user_data.is_null() {
        return;
    }

    let callback_data = unsafe { &*(user_data as *const EventCallbackData) };

    let name = if event_name.is_null() {
        callback_data.event_name.clone()
    } else {
        unsafe { CStr::from_ptr(event_name) }
            .to_string_lossy()
            .into_owned()
    };

    let payload = if data_json.is_null() {
        serde_json::Value::Array(vec![])
    } else {
        let raw = unsafe { CStr::from_ptr(data_json) }.to_string_lossy().into_owned();
        serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw))
    };

    let event_data = EventData { event: name, data: payload };
    let _ = callback_data.tx.send(event_data);
}

pub(crate) fn create_method_callback() -> (Receiver<CallResult>, *mut c_void, LpResultCb) {
    let (tx, rx) = mpsc::channel();
    let callback_data = Box::new(CallbackData { tx });
    let user_data = Box::into_raw(callback_data) as *mut c_void;
    (rx, user_data, method_callback_trampoline)
}

pub(crate) fn create_event_callback(
    event_name: &str,
) -> (Receiver<EventData>, Box<EventCallbackData>, LpEventCb) {
    let (tx, rx) = mpsc::channel();
    let callback_data = Box::new(EventCallbackData {
        tx,
        event_name: event_name.to_string(),
    });
    (rx, callback_data, event_callback_trampoline)
}

pub(crate) fn event_callback_ptr(callback_data: &EventCallbackData) -> *mut c_void {
    callback_data as *const EventCallbackData as *mut c_void
}
