//! FFI bindings to liblogos_core.

use std::ffi::{c_char, c_int, c_void};

pub type LogosAsyncCallback = extern "C" fn(
    result: c_int,
    message: *const c_char,
    user_data: *mut c_void,
);

extern "C" {
    pub fn logos_core_init(argc: c_int, argv: *mut *mut c_char);
    pub fn logos_core_set_plugins_dir(plugins_dir: *const c_char);
    pub fn logos_core_start();
    pub fn logos_core_exec() -> c_int;
    pub fn logos_core_cleanup();
    pub fn logos_core_process_events();
}

extern "C" {
    pub fn logos_core_process_plugin(plugin_path: *const c_char) -> *const c_char;
    pub fn logos_core_load_plugin(plugin_name: *const c_char) -> c_int;
    pub fn logos_core_unload_plugin(plugin_name: *const c_char) -> c_int;
    pub fn logos_core_get_loaded_plugins() -> *mut *mut c_char;
    pub fn logos_core_get_known_plugins() -> *mut *mut c_char;
}

extern "C" {
    pub fn logos_core_call_plugin_method_async(
        plugin_name: *const c_char,
        method_name: *const c_char,
        params_json: *const c_char,
        callback: LogosAsyncCallback,
        user_data: *mut c_void,
    );

    pub fn logos_core_register_event_listener(
        plugin_name: *const c_char,
        event_name: *const c_char,
        callback: LogosAsyncCallback,
        user_data: *mut c_void,
    );
}

extern "C" {
    pub fn logos_core_get_token(module_name: *const c_char) -> *const c_char;
}
