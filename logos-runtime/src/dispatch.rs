use crate::value::Value;

use std::sync::Arc;

/// Trait for broadcasting events to connected clients.
pub trait EventBroadcast: Send + Sync {
    fn broadcast_event(&self, name: &str, data: &[Value]);
}

/// Handle for emitting events from dispatch code.
#[derive(Clone)]
pub struct EventEmitter {
    inner: Arc<dyn EventBroadcast>,
}

impl EventEmitter {
    pub fn new(broadcast: Arc<dyn EventBroadcast>) -> Self {
        EventEmitter { inner: broadcast }
    }
    pub fn emit(&self, name: &str, data: &[Value]) {
        self.inner.broadcast_event(name, data);
    }
}

/// Trait for generated CBOR dispatch code.
///
/// Each Rust module backend generates a concrete implementation
/// that routes method calls to the user's trait implementation.
pub trait CborDispatch: Send + Sync {
    /// Dispatch a method call by name and return the result.
    fn call_method(&self, method: &str, args: &[Value]) -> Value;

    /// Return the module name (as declared in LIDL).
    fn module_name(&self) -> &str;

    /// Return the module version.
    fn module_version(&self) -> &str;

    /// Return a JSON array of method descriptors.
    fn methods_json(&self) -> &str;

    /// Called by CborServer to provide event emitter. Default: no-op.
    fn set_event_emitter(&mut self, _emitter: EventEmitter) {}
}
