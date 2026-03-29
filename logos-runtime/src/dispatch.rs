use crate::value::Value;

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
}
