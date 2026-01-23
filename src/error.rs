//! Error types for the Logos Rust SDK.

use std::ffi::NulError;
use std::fmt;

#[derive(Debug)]
pub enum LogosError {
    NotInitialized,
    AlreadyInitialized,
    AlreadyStarted,
    SetPluginsDirFailed(String),
    StartFailed(String),
    PluginLoadFailed(String),
    PluginUnloadFailed(String),
    PluginProcessFailed(String),
    MethodCallFailed {
        plugin: String,
        method: String,
        message: String,
    },
    EventListenerFailed {
        plugin: String,
        event: String,
        message: String,
    },
    InvalidString(NulError),
    JsonError(String),
    ChannelClosed,
    Timeout,
    Other(String),
}

impl fmt::Display for LogosError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogosError::NotInitialized => {
                write!(f, "Logos SDK has not been initialized")
            }
            LogosError::AlreadyInitialized => {
                write!(f, "Logos SDK has already been initialized")
            }
            LogosError::AlreadyStarted => {
                write!(f, "Logos SDK has already been started")
            }
            LogosError::SetPluginsDirFailed(path) => {
                write!(f, "Failed to set plugins directory: {}", path)
            }
            LogosError::StartFailed(msg) => {
                write!(f, "Failed to start Logos Core: {}", msg)
            }
            LogosError::PluginLoadFailed(name) => {
                write!(f, "Failed to load plugin: {}", name)
            }
            LogosError::PluginUnloadFailed(name) => {
                write!(f, "Failed to unload plugin: {}", name)
            }
            LogosError::PluginProcessFailed(path) => {
                write!(f, "Failed to process plugin file: {}", path)
            }
            LogosError::MethodCallFailed { plugin, method, message } => {
                write!(f, "Method call {}.{}() failed: {}", plugin, method, message)
            }
            LogosError::EventListenerFailed { plugin, event, message } => {
                write!(f, "Failed to register event listener {}.{}: {}", plugin, event, message)
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
            LogosError::Timeout => {
                write!(f, "Operation timed out")
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

impl<T> From<std::sync::mpsc::SendError<T>> for LogosError {
    fn from(_: std::sync::mpsc::SendError<T>) -> Self {
        LogosError::ChannelClosed
    }
}

impl From<std::sync::mpsc::RecvError> for LogosError {
    fn from(_: std::sync::mpsc::RecvError) -> Self {
        LogosError::ChannelClosed
    }
}
