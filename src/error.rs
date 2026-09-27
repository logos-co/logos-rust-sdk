//! Error types for the Logos Rust SDK.

use std::ffi::NulError;
use std::fmt;
use std::time::Duration;

#[derive(Debug)]
pub enum LogosError {
    PluginCallFailed {
        plugin: String,
        method: String,
        message: String,
    },
    EventListenerFailed {
        plugin: String,
        event: String,
        message: String,
    },
    /// A per-call timeout that cannot be expressed on the `lp_*` C ABI, whose
    /// `timeout_ms` is a `c_int` in which any value `<= 0` MEANS "use the
    /// protocol default" (20s). Rather than clamp — which would answer a
    /// different question than the caller asked, silently — the conversion
    /// refuses. Raised by the `*_with_timeout` entry points, at the point the
    /// bad value was supplied. See `PluginProxy::call_json_with_timeout`.
    InvalidTimeout {
        timeout: Duration,
        reason: String,
    },
    InvalidString(NulError),
    JsonError(String),
    ChannelClosed,
    Other(String),
}

impl LogosError {
    /// The protocol's error code ("not_authorised", "unauthorized", "timeout", ...)
    /// when the failure carried the canonical {code, message, origin} object.
    pub fn code(&self) -> Option<String> {
        match self {
            LogosError::PluginCallFailed { message, .. } => serde_json::from_str::<serde_json::Value>(message)
                .ok()
                .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(String::from)),
            _ => None,
        }
    }

    /// The target's grant does not cover the method. Never retried: a new token
    /// would carry the same grant.
    pub fn is_not_authorised(&self) -> bool {
        self.code().as_deref() == Some("not_authorised")
    }
}

impl fmt::Display for LogosError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogosError::PluginCallFailed { plugin, method, message } => {
                write!(f, "Method call {}.{}() failed: {}", plugin, method, message)
            }
            LogosError::EventListenerFailed { plugin, event, message } => {
                write!(f, "Failed to register event listener {}.{}: {}", plugin, event, message)
            }
            LogosError::InvalidTimeout { timeout, reason } => {
                write!(f, "Invalid call timeout {:?}: {}", timeout, reason)
            }
            LogosError::InvalidString(e) => {
                write!(f, "Invalid string (contains null byte): {}", e)
            }
            LogosError::JsonError(msg) => {
                write!(f, "JSON error: {}", msg)
            }
            LogosError::ChannelClosed => {
                write!(f, "Callback channel was closed unexpectedly")
            }
            LogosError::Other(msg) => {
                write!(f, "{}", msg)
            }
        }
    }
}

impl std::error::Error for LogosError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LogosError::InvalidString(e) => Some(e),
            _ => None,
        }
    }
}

impl From<NulError> for LogosError {
    fn from(e: NulError) -> Self {
        LogosError::InvalidString(e)
    }
}

impl From<serde_json::Error> for LogosError {
    fn from(e: serde_json::Error) -> Self {
        LogosError::JsonError(e.to_string())
    }
}

impl From<std::sync::mpsc::RecvError> for LogosError {
    fn from(_: std::sync::mpsc::RecvError) -> Self {
        LogosError::ChannelClosed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(message: &str) -> LogosError {
        LogosError::PluginCallFailed { plugin: "t".into(), method: "m".into(), message: message.into() }
    }

    #[test]
    fn the_code_comes_from_the_canonical_error_object() {
        let refused = failed(r#"{"code":"not_authorised","message":"outside the grant","origin":"t"}"#);
        assert_eq!(refused.code().as_deref(), Some("not_authorised"));
        assert!(refused.is_not_authorised());
        assert_eq!(failed(r#"{"code":"unauthorized","message":"","origin":"t"}"#).code().as_deref(),
                   Some("unauthorized"));
        assert!(!failed(r#"{"code":"unauthorized"}"#).is_not_authorised());
        assert_eq!(failed("lp_invoke failed with code -3").code(), None);
        assert_eq!(LogosError::ChannelClosed.code(), None);
    }
}
