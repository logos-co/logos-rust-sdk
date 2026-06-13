//! Entry point for the Logos Module SDK.

use crate::plugin::PluginProxy;

/// The main entry point for calling other Logos modules from within a module.
///
/// No initialization or lifecycle management is required — the underlying
/// `lp_*` protocol clients are created lazily. Simply create an instance and
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

    /// Shut down the SDK.
    ///
    /// Since the move to the lp_* C ABI each `PluginProxy` owns its own
    /// protocol client (released on drop), so there is no process-global
    /// connection state left to tear down. Kept for source compatibility.
    pub fn shutdown(&self) {}
}

/// The logos-protocol semver this SDK was linked against — the single
/// number governing Logos load/call compatibility (same MAJOR ⇔
/// compatible). Forwarded verbatim from the linked protocol library,
/// never minted by the SDK.
pub fn protocol_version() -> String {
    let ptr = unsafe { crate::ffi::lp_protocol_version() };
    if ptr.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// MAJOR component of the linked logos-protocol version.
pub fn protocol_abi_major() -> i32 {
    unsafe { crate::ffi::lp_protocol_abi_major() }
}

/// Save a host-issued auth token into the SDK's protocol stack so
/// subsequent calls to `module_name` authenticate.
///
/// This is the consumer half of the module-impl C ABI's
/// `logos_module_accept_token` runtime handshake: the Qt glue receives the
/// token from the host and forwards it across the C seam; the generated
/// provider scaffold (lidl-gen `--provider`) hands it here so the *same*
/// protocol stack the SDK invokes through holds the token — closing the
/// split where the glue's stack was authenticated but the Rust one wasn't.
pub fn save_token(module_name: &str, token: &str) -> bool {
    let (Ok(name_c), Ok(token_c)) = (
        std::ffi::CString::new(module_name),
        std::ffi::CString::new(token),
    ) else {
        return false;
    };
    unsafe { crate::ffi::lp_token_save(name_c.as_ptr(), token_c.as_ptr()) == crate::ffi::LP_OK }
}

impl Default for LogosModuleSDK {
    fn default() -> Self {
        Self::new()
    }
}
