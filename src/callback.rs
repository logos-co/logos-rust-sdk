//! Callback management for bridging C callbacks to Rust channels.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::mpsc::{self, Receiver, Sender};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::ffi::LogosSdkCallback;

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

    /// Get the base64-decoded bytes of the string element at `index`.
    /// Returns `None` if absent, not a string, or not valid base64. Pairs with
    /// [`Param::bytes`](crate::Param::bytes) on the sending side.
    pub fn get_bytes(&self, index: usize) -> Option<Vec<u8>> {
        self.get_str(index).and_then(|s| BASE64.decode(s).ok())
    }
}

pub(crate) struct CallbackData {
    pub tx: Sender<CallResult>,
}

pub(crate) struct EventCallbackData {
    pub tx: Sender<EventData>,
    pub event_name: String,
}

pub(crate) extern "C" fn method_callback_trampoline(
    result: c_int,
    message: *const c_char,
    user_data: *mut c_void,
) {
    if user_data.is_null() {
        return;
    }

    let callback_data = unsafe { Box::from_raw(user_data as *mut CallbackData) };

    let message_str = if message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };

    let call_result = CallResult {
        success: result != 0,
        message: message_str,
    };

    let _ = callback_data.tx.send(call_result);
}

pub(crate) extern "C" fn event_callback_trampoline(
    result: c_int,
    message: *const c_char,
    user_data: *mut c_void,
) {
    if user_data.is_null() {
        return;
    }

    let callback_data = unsafe { &*(user_data as *const EventCallbackData) };

    if result == 0 {
        return;
    }

    let message_str = if message.is_null() {
        return;
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };

    let event_data = match EventData::from_json(&message_str) {
        Ok(data) => data,
        Err(_) => {
            // If parsing fails, create a simple event with the raw message
            EventData {
                event: callback_data.event_name.clone(),
                data: serde_json::Value::String(message_str),
            }
        }
    };

    let _ = callback_data.tx.send(event_data);
}

pub(crate) fn create_method_callback() -> (Receiver<CallResult>, *mut c_void, LogosSdkCallback) {
    let (tx, rx) = mpsc::channel();
    let callback_data = Box::new(CallbackData { tx });
    let user_data = Box::into_raw(callback_data) as *mut c_void;
    (rx, user_data, method_callback_trampoline)
}

pub(crate) fn create_event_callback(
    event_name: &str,
) -> (Receiver<EventData>, Box<EventCallbackData>, LogosSdkCallback) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_bytes_decodes_base64() {
        let ev = EventData::new(
            "messageReceived",
            serde_json::json!(["hash", "/logos-chat/1/x/proto", "AAEC/w=="]),
        );
        assert_eq!(ev.get_bytes(2), Some(vec![0u8, 1, 2, 255]));
    }

    #[test]
    fn get_bytes_none_when_absent_non_string_or_invalid() {
        let ev = EventData::new("e", serde_json::json!(["not base64!!", 5]));
        assert_eq!(ev.get_bytes(0), None); // invalid base64
        assert_eq!(ev.get_bytes(1), None); // not a string
        assert_eq!(ev.get_bytes(9), None); // out of range
    }
}
