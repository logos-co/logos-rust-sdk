//! FFI bindings to liblogos_module_client (logos-module-client).

use std::ffi::{c_char, c_int, c_void};

/// C callback type matching LogosSdkCallback in logos_sdk_c.h.
pub type LogosSdkCallback = extern "C" fn(
    result: c_int,
    message: *const c_char,
    user_data: *mut c_void,
);

extern "C" {
    /// Call a plugin method synchronously.
    /// `timeout_ms <= 0` selects the default reply timeout.
    /// Returns a heap-allocated UTF-8 string that must be freed with logos_sdk_free_string.
    /// Returns NULL on failure.
    pub fn logos_sdk_call_method_sync(
        plugin_name: *const c_char,
        method_name: *const c_char,
        params_json: *const c_char,
        timeout_ms: c_int,
    ) -> *mut c_char;

    /// Free a string returned by logos_sdk_call_method_sync.
    pub fn logos_sdk_free_string(str: *mut c_char);

    /// Call a plugin method asynchronously.
    /// callback receives (1=success/0=failure, message, user_data) on completion.
    pub fn logos_sdk_call_method_async(
        plugin_name: *const c_char,
        method_name: *const c_char,
        params_json: *const c_char,
        callback: LogosSdkCallback,
        user_data: *mut c_void,
    );

    /// Register an event listener for a specific event from a plugin.
    /// callback fires each time the event is emitted with (1, json_event_data, user_data).
    pub fn logos_sdk_register_event(
        plugin_name: *const c_char,
        event_name: *const c_char,
        callback: LogosSdkCallback,
        user_data: *mut c_void,
    );

    /// Shut down the SDK's internal core client, releasing connections.
    /// Safe to call multiple times.
    pub fn logos_sdk_shutdown();
}
