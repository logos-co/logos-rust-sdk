//! Entry point for the Logos Module SDK.

use crate::plugin::PluginProxy;

/// The main entry point for calling other Logos modules from within a module.
///
/// Unlike the old LogosAPI, this requires no initialization or lifecycle management —
/// logos-module-client handles connections lazily. Simply create an instance and
/// call `plugin()` to get a proxy for any loaded module.
///
/// # Example
/// ```rust,no_run
/// use logos_rust_sdk::LogosModuleSDK;
///
/// let sdk = LogosModuleSDK::new();
/// let provider = sdk.plugin("rust_provider_module");
/// let result = provider.call_sync("add", &[5i64, 3i64]).unwrap();
/// println!("Result: {}", result.message);
/// ```
pub struct LogosModuleSDK;

impl LogosModuleSDK {
    /// Create a new SDK instance. No initialization is performed.
    pub fn new() -> Self {
        LogosModuleSDK
    }

    /// Get a proxy for communicating with the named plugin.
    pub fn plugin(&self, name: &str) -> PluginProxy {
        PluginProxy::new(name)
    }

    /// Shut down the internal client, releasing all connections.
    /// Safe to call multiple times. Connections are lazily re-created on next use.
    pub fn shutdown(&self) {
        unsafe {
            crate::ffi::logos_sdk_shutdown();
        }
    }
}

impl Default for LogosModuleSDK {
    fn default() -> Self {
        Self::new()
    }
}
