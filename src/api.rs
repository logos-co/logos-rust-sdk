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

/// Route a host-issued grant into THIS image's gate state.
///
/// The grant has to cross the C ABI rather than be recorded once by the host:
/// the host binary and this cdylib each link their own copy of logos-protocol,
/// so each has its own process-global grant state, exactly as each has its own
/// TokenManager. A grant the host records for itself is invisible to the gate
/// an `lp_token_keys()` call checks here.
///
/// Takes the raw pointer the C ABI hands us rather than a `&str`: the caller is
/// the generated `logos_module_grant_host_services` export, which receives it
/// straight from the host and has nothing to gain from a round trip through
/// `CStr`. A null pointer is refused rather than dereferenced.
///
/// # Safety
/// `services_json` must be null or a valid NUL-terminated C string.
pub unsafe fn grant_host_services(services_json: *const std::os::raw::c_char) -> i32 {
    if services_json.is_null() {
        return -1;
    }
    unsafe { crate::ffi::lp_grant_host_services(services_json) }
}

impl Default for LogosModuleSDK {
    fn default() -> Self {
        Self::new()
    }
}

// -- teardown ---------------------------------------------------------------
//
// The module-impl C ABI gained two teardown exports in logos-protocol 0.5
// (`logos_module_set_unload_done_callback` / `logos_module_about_to_unload`).
// The generated provider scaffold owns the exports — it is what can reach the
// author's impl — but the CALLBACK lives here, for the same reason
// `grant_host_services` does: it is SDK state, and a module that wants to
// signal completion from its own code should not have to reach back into
// generated symbols to do it.

/// A module's answer to "are you ready to be unloaded?".
///
/// Mirrors C++'s `LogosShutdown` and Qt Creator's `IPlugin::aboutToShutdown()`
/// contract: return `Synchronous` when teardown finished inline (the common
/// case, and the default), or `Asynchronous` to keep the host waiting until
/// [`unload_finished`] is called. The host enforces a grace period either way —
/// an `Asynchronous` module that never finishes is killed, not waited on
/// forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shutdown {
    /// Teardown is complete; the host may proceed immediately.
    Synchronous,
    /// Teardown continues in the background; the host waits for
    /// [`unload_finished`] (or its grace period, whichever comes first).
    Asynchronous,
}

/// The C callback the host installs to learn that async teardown finished.
pub type UnloadDoneCb = unsafe extern "C" fn(*mut std::os::raw::c_void);

// The `void*` rides as a `usize` so the pair is `Send`: a raw pointer is not,
// and the callback is installed on the host's thread but fired from whichever
// thread the module finishes its work on — which is the whole point of the
// asynchronous answer.
static UNLOAD_CB: std::sync::Mutex<Option<(UnloadDoneCb, usize)>> = std::sync::Mutex::new(None);

/// Install the host's completion callback. `None` clears it.
///
/// Called by the generated `logos_module_set_unload_done_callback` export
/// before the host asks the module to unload, so a module that finishes inline
/// still has somewhere to signal.
pub fn set_unload_done_callback(cb: Option<UnloadDoneCb>, user_data: *mut std::os::raw::c_void) {
    *UNLOAD_CB.lock().unwrap() = cb.map(|f| (f, user_data as usize));
}

/// Signal that this module's asynchronous teardown is complete.
///
/// Only meaningful after returning [`Shutdown::Asynchronous`]; calling it
/// otherwise is harmless (the host is not waiting). Safe to call from any
/// thread — the host marshals the notification back to its own.
pub fn unload_finished() {
    // Copied out from under the lock: the callback re-enters the host, and
    // holding the SDK's mutex across that is how a teardown deadlock starts.
    let entry = *UNLOAD_CB.lock().unwrap();
    if let Some((cb, ud)) = entry {
        unsafe { cb(ud as *mut std::os::raw::c_void) };
    }
}
