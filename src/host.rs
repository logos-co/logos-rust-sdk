//! Embedding a Logos runtime in an application (cargo feature `host`).
//!
//! [`LogosCore::start`] spawns liblogos' `logos_runtime` and makes this process
//! its shell. It also declares the shell as this process's origin, so every
//! generated client (`XClient::new()`) calls modules as the shell. A module
//! loaded from a modules directory and one imported from a peered runtime are
//! reached the same way.
//!
//! liblogos and its `liblogos_protocol_plain` resolve at the final link; set
//! `LOGOS_HOST_LIB_DIR` to liblogos' `lib` output (see `lib.hostBuildSupport`).
//! Link that ONE protocol image: a second copy would hold its own tokens.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::error::LogosError;

#[repr(C)]
struct RawRuntime {
    _private: [u8; 0],
}
#[repr(C)]
struct RawConsumer {
    _private: [u8; 0],
}
#[repr(C)]
struct RawSubscription {
    _private: [u8; 0],
}

type ExitCb = extern "C" fn(reason: *const c_char, user_data: *mut c_void);
type ResultCb = extern "C" fn(ok: c_int, json: *const c_char, user_data: *mut c_void);
type EventCb = extern "C" fn(event: *const c_char, data: *const c_char, user_data: *mut c_void);

// Mirror of logos-liblogos src/logos_core/logos_core.h (the runtime-process part).
extern "C" {
    fn logos_runtime_spawn(config_json: *const c_char, out_error: *mut *mut c_char) -> *mut RawRuntime;
    fn logos_runtime_binding(runtime: *mut RawRuntime) -> *mut RawConsumer;
    fn logos_runtime_process_module(runtime: *mut RawRuntime, module_path: *const c_char) -> *mut c_char;
    fn logos_runtime_on_exit(runtime: *mut RawRuntime, cb: ExitCb, user_data: *mut c_void);
    fn logos_runtime_stop(runtime: *mut RawRuntime);
    fn logos_consumer_call(consumer: *mut RawConsumer, target: *const c_char, method: *const c_char,
                           args_json: *const c_char, timeout_ms: c_int, out_result_json: *mut *mut c_char,
                           out_error_json: *mut *mut c_char) -> c_int;
    fn logos_consumer_call_async(consumer: *mut RawConsumer, target: *const c_char, method: *const c_char,
                                 args_json: *const c_char, timeout_ms: c_int, cb: ResultCb,
                                 user_data: *mut c_void) -> c_int;
    fn logos_consumer_subscribe(consumer: *mut RawConsumer, target: *const c_char, event_name: *const c_char,
                                cb: EventCb, user_data: *mut c_void) -> *mut RawSubscription;
    fn logos_consumer_unsubscribe(subscription: *mut RawSubscription);
    fn logos_consumer_string_free(value: *mut c_char);
}

/// How far `load_module` walks the dependency graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadDeps {
    ModuleOnly,
    Required,
    RequiredAndOptional,
}

impl LoadDeps {
    fn as_str(self) -> &'static str {
        match self {
            LoadDeps::ModuleOnly => "module_only",
            LoadDeps::Required => "required",
            LoadDeps::RequiredAndOptional => "required_and_optional",
        }
    }
}

// Names the runtime keeps for itself; liblogos refuses them as a shell.
const RESERVED: &[&str] = &["core", "core_service", "capability_module", "modules_state",
                            "peering_identity", "peering_module", "peering_control", "package_ops",
                            "runtime"];

/// Whether `name` can be a shell: a module name that is not the runtime's own.
pub fn is_valid_shell_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && !lower.starts_with("logos_")
        && !RESERVED.contains(&lower.as_str())
}

/// What `logos_runtime` is started with. Paths to the runtime and its hosts are
/// optional: liblogos finds them beside the app or the modules otherwise.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub shell: String,
    pub modules_dirs: Vec<PathBuf>,
    pub bundled_modules_dirs: Vec<PathBuf>,
    pub persistence_base_path: Option<PathBuf>,
    pub peering: Option<Value>,
    pub module_transports: Option<Value>,
    pub access_policy: Option<Value>,
    pub placement_policy: Option<Value>,
    pub runtime_path: Option<PathBuf>,
    pub host_plain_path: Option<PathBuf>,
    pub host_remote_path: Option<PathBuf>,
    /// Where every process of the runtime puts its sockets (`TMPDIR`); keep it
    /// short, a socket path holds at most ~104 bytes.
    pub tmp_dir: Option<PathBuf>,
}

impl Config {
    pub fn new(shell: impl Into<String>) -> Self {
        Config { shell: shell.into(), ..Config::default() }
    }
    pub fn modules_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.modules_dirs.push(dir.into());
        self
    }
    pub fn bundled_modules_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.bundled_modules_dirs.push(dir.into());
        self
    }
    pub fn persistence(mut self, dir: impl Into<PathBuf>) -> Self {
        self.persistence_base_path = Some(dir.into());
        self
    }
    /// The runtime's peering configuration (logos-peering `docs/api.md`).
    pub fn peering(mut self, config: Value) -> Self {
        self.peering = Some(config);
        self
    }
    pub fn runtime_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.runtime_path = Some(path.into());
        self
    }
    pub fn host_plain_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.host_plain_path = Some(path.into());
        self
    }
    pub fn host_remote_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.host_remote_path = Some(path.into());
        self
    }
    pub fn tmp_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.tmp_dir = Some(dir.into());
        self
    }

    /// The spawn line `logos_runtime` reads on its stdin.
    pub fn spawn_json(&self) -> Result<Value, LogosError> {
        if !is_valid_shell_name(&self.shell) {
            return Err(LogosError::Other(format!("'{}' cannot be a shell name", self.shell)));
        }
        let dirs = |list: &[PathBuf]| -> Value {
            Value::Array(list.iter().map(|p| Value::String(p.to_string_lossy().into_owned())).collect())
        };
        let mut config = Map::new();
        config.insert("shell".into(), Value::String(self.shell.clone()));
        config.insert("modules_dirs".into(), dirs(&self.modules_dirs));
        config.insert("bundled_modules_dirs".into(), dirs(&self.bundled_modules_dirs));
        if let Some(path) = &self.persistence_base_path {
            config.insert("persistence_base_path".into(), Value::String(path.to_string_lossy().into_owned()));
        }
        for (key, value) in [("peering_config", &self.peering), ("module_transports", &self.module_transports),
                             ("access_policy", &self.access_policy), ("placement_policy", &self.placement_policy)] {
            if let Some(value) = value {
                config.insert(key.into(), value.clone());
            }
        }
        Ok(Value::Object(config))
    }

    // liblogos and its hosts read these in every process they start.
    fn export_environment(&self) {
        let set = |name: &str, value: &Option<PathBuf>| {
            if let Some(path) = value {
                std::env::set_var(name, path);
            }
        };
        set("LOGOS_RUNTIME_PATH", &self.runtime_path);
        set("LOGOS_HOST_PLAIN_PATH", &self.host_plain_path);
        set("LOGOS_HOST_REMOTE_PATH", &self.host_remote_path);
        set("TMPDIR", &self.tmp_dir);
    }
}

type ExitHandler = Box<dyn FnOnce(String) + Send>;

#[derive(Default)]
struct ExitState {
    handler: Option<ExitHandler>,
    reason: Option<String>,
}

extern "C" fn exit_trampoline(reason: *const c_char, user_data: *mut c_void) {
    let state = unsafe { &*(user_data as *const Mutex<ExitState>) };
    let reason = unsafe { text(reason) }.unwrap_or_default();
    let handler = {
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        guard.reason = Some(reason.clone());
        guard.handler.take()
    };
    if let Some(handler) = handler {
        let _ = catch_unwind(AssertUnwindSafe(|| handler(reason)));
    }
}

struct Inner {
    runtime: *mut RawRuntime,
    binding: *mut RawConsumer,
    shell: String,
    exit: *const Mutex<ExitState>,
}

// The binding's calls are thread-safe (each is an lp_client call underneath).
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

impl Drop for Inner {
    fn drop(&mut self) {
        unsafe {
            logos_runtime_stop(self.runtime);
            drop(Arc::from_raw(self.exit));
        }
    }
}

/// A running `logos_runtime`, and this process as its shell. Clones share it;
/// it stops when the last clone, subscription and pending call are gone.
#[derive(Clone)]
pub struct LogosCore {
    inner: Arc<Inner>,
}

impl LogosCore {
    /// Spawns the runtime and waits for it: up to two minutes while it loads
    /// its bundled modules, so keep it off a UI thread. One runtime per process.
    pub fn start(config: Config) -> Result<LogosCore, LogosError> {
        let spawn = config.spawn_json()?;
        if !crate::api::set_module_origin(&config.shell) {
            return Err(LogosError::Other(format!(
                "this process already calls modules as '{}'", crate::api::module_origin().unwrap_or("")
            )));
        }
        config.export_environment();
        let line = CString::new(spawn.to_string())?;
        let mut error: *mut c_char = std::ptr::null_mut();
        let runtime = unsafe { logos_runtime_spawn(line.as_ptr(), &mut error) };
        if runtime.is_null() {
            let why = unsafe { take_string(error) }.unwrap_or_else(|| "the runtime did not start".into());
            return Err(LogosError::Other(why));
        }
        let binding = unsafe { logos_runtime_binding(runtime) };
        let exit = Arc::into_raw(Arc::new(Mutex::new(ExitState::default())));
        unsafe { logos_runtime_on_exit(runtime, exit_trampoline, exit as *mut c_void) };
        Ok(LogosCore { inner: Arc::new(Inner { runtime, binding, shell: config.shell, exit }) })
    }

    pub fn shell_name(&self) -> &str {
        &self.inner.shell
    }

    /// Calls `handler` once if the runtime exits before [`LogosCore::stop`].
    pub fn on_exit(&self, handler: impl FnOnce(String) + Send + 'static) {
        let state = unsafe { &*self.inner.exit };
        let already = {
            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
            match guard.reason.clone() {
                Some(reason) => Some(reason),
                None => {
                    guard.handler = Some(Box::new(handler));
                    return;
                }
            }
        };
        if let Some(reason) = already {
            handler(reason);
        }
    }

    /// Calls `target.method(args)` as the shell; `args` is a JSON array.
    pub fn call(&self, target: &str, method: &str, args: Value, timeout: Duration) -> Result<Value, LogosError> {
        let (target_c, method_c, args_c) = call_strings(target, method, &args)?;
        let mut result: *mut c_char = std::ptr::null_mut();
        let mut error: *mut c_char = std::ptr::null_mut();
        let status = unsafe {
            logos_consumer_call(self.inner.binding, target_c.as_ptr(), method_c.as_ptr(), args_c.as_ptr(),
                                timeout_ms(timeout)?, &mut result, &mut error)
        };
        let result = unsafe { take_string(result) };
        let error = unsafe { take_string(error) };
        if status != 0 {
            return Err(call_failed(target, method, error.as_deref().unwrap_or("the call failed")));
        }
        fold_rejection(target, method, parse(result.as_deref()))
    }

    /// The async twin of [`LogosCore::call`]; `done` runs on a liblogos thread.
    pub fn call_async(&self, target: &str, method: &str, args: Value, timeout: Duration,
                      done: impl FnOnce(Result<Value, LogosError>) + Send + 'static) -> Result<(), LogosError> {
        let (target_c, method_c, args_c) = call_strings(target, method, &args)?;
        let state = Box::into_raw(Box::new(AsyncState {
            done: Box::new(done),
            target: target.to_string(),
            method: method.to_string(),
            _core: self.inner.clone(),
        }));
        let status = unsafe {
            logos_consumer_call_async(self.inner.binding, target_c.as_ptr(), method_c.as_ptr(), args_c.as_ptr(),
                                      timeout_ms(timeout)?, async_trampoline, state as *mut c_void)
        };
        if status != 0 {
            drop(unsafe { Box::from_raw(state) });
            return Err(call_failed(target, method, "the call was not sent"));
        }
        Ok(())
    }

    /// Delivers `target`'s `event` to `handler` (on a liblogos thread) until the
    /// returned subscription drops. Do not drop it from inside its own handler.
    pub fn subscribe(&self, target: &str, event: &str,
                     handler: impl FnMut(&str, Value) + Send + 'static) -> Result<Subscription, LogosError> {
        let target_c = CString::new(target)?;
        let event_c = CString::new(event)?;
        let state: Box<Mutex<EventHandler>> = Box::new(Mutex::new(Box::new(handler)));
        let state = Box::into_raw(state);
        let raw = unsafe {
            logos_consumer_subscribe(self.inner.binding, target_c.as_ptr(), event_c.as_ptr(), event_trampoline,
                                     state as *mut c_void)
        };
        if raw.is_null() {
            drop(unsafe { Box::from_raw(state) });
            return Err(LogosError::EventListenerFailed {
                plugin: target.to_string(),
                event: event.to_string(),
                message: "the runtime refused the subscription".into(),
            });
        }
        Ok(Subscription { raw, state, _core: self.inner.clone() })
    }

    fn core_service(&self, method: &str, args: Value, timeout: Duration) -> Result<Value, LogosError> {
        self.call("core_service", method, args, timeout)
    }

    /// Ensures `name` is loaded; blocks for its bring-up.
    pub fn load_module(&self, name: &str, deps: LoadDeps) -> Result<Value, LogosError> {
        self.core_service("loadModule", json!([name, deps.as_str()]), Duration::from_secs(120))
    }

    pub fn unload_module(&self, name: &str) -> Result<Value, LogosError> {
        self.core_service("unloadModule", json!([name]), Duration::from_secs(120))
    }

    /// Every module the runtime knows, with its state.
    pub fn list_modules(&self) -> Result<Value, LogosError> {
        self.core_service("listModules", json!([]), Duration::from_secs(15))
    }

    pub fn module_stats(&self) -> Result<Value, LogosError> {
        self.core_service("getModuleStats", json!([]), Duration::from_secs(15))
    }

    /// Registers a module directory the runtime has not scanned; its name, or None if refused.
    pub fn process_module(&self, path: &Path) -> Option<String> {
        let path = CString::new(path.to_string_lossy().into_owned()).ok()?;
        unsafe { take_string(logos_runtime_process_module(self.inner.runtime, path.as_ptr())) }
    }

    /// Drops this handle; the runtime stops its modules in order and exits once
    /// nothing else holds it.
    pub fn stop(self) {}
}

type EventHandler = Box<dyn FnMut(&str, Value) + Send>;

/// An event subscription; dropping it unsubscribes.
pub struct Subscription {
    raw: *mut RawSubscription,
    state: *mut Mutex<EventHandler>,
    _core: Arc<Inner>,
}

unsafe impl Send for Subscription {}

impl Drop for Subscription {
    fn drop(&mut self) {
        // Once unsubscribe returns the handler will not run again.
        unsafe {
            logos_consumer_unsubscribe(self.raw);
            drop(Box::from_raw(self.state));
        }
    }
}

extern "C" fn event_trampoline(event: *const c_char, data: *const c_char, user_data: *mut c_void) {
    let state = unsafe { &*(user_data as *const Mutex<EventHandler>) };
    let name = unsafe { text(event) }.unwrap_or_default();
    let payload = parse(unsafe { text(data) }.as_deref());
    let mut handler = state.lock().unwrap_or_else(|e| e.into_inner());
    let _ = catch_unwind(AssertUnwindSafe(|| (handler)(&name, payload)));
}

struct AsyncState {
    done: Box<dyn FnOnce(Result<Value, LogosError>) + Send>,
    target: String,
    method: String,
    _core: Arc<Inner>,
}

extern "C" fn async_trampoline(ok: c_int, json: *const c_char, user_data: *mut c_void) {
    let state = unsafe { Box::from_raw(user_data as *mut AsyncState) };
    let raw = unsafe { text(json) };
    let result = if ok != 0 {
        fold_rejection(&state.target, &state.method, parse(raw.as_deref()))
    } else {
        Err(call_failed(&state.target, &state.method, raw.as_deref().unwrap_or("the call failed")))
    };
    let AsyncState { done, .. } = *state;
    let _ = catch_unwind(AssertUnwindSafe(|| done(result)));
}

fn call_strings(target: &str, method: &str, args: &Value) -> Result<(CString, CString, CString), LogosError> {
    if !args.is_array() {
        return Err(LogosError::Other(format!("{target}.{method}: arguments must be a JSON array")));
    }
    Ok((CString::new(target)?, CString::new(method)?, CString::new(args.to_string())?))
}

fn timeout_ms(timeout: Duration) -> Result<c_int, LogosError> {
    match c_int::try_from(timeout.as_millis()) {
        Ok(ms) if ms > 0 => Ok(ms),
        _ => Err(LogosError::InvalidTimeout { timeout, reason: "must be between 1 ms and i32::MAX ms".into() }),
    }
}

fn parse(raw: Option<&str>) -> Value {
    match raw {
        None => Value::Null,
        Some(text) => serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string())),
    }
}

fn fold_rejection(target: &str, method: &str, value: Value) -> Result<Value, LogosError> {
    match crate::args::as_dispatch_rejection(&value) {
        Some(message) => Err(call_failed(target, method, message)),
        None => Ok(value),
    }
}

fn call_failed(target: &str, method: &str, error: &str) -> LogosError {
    // An error JSON carries its message; anything else is the message.
    let message = serde_json::from_str::<Value>(error)
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| error.to_string());
    LogosError::PluginCallFailed { plugin: target.to_string(), method: method.to_string(), message }
}

unsafe fn text(value: *const c_char) -> Option<String> {
    (!value.is_null()).then(|| CStr::from_ptr(value).to_string_lossy().into_owned())
}

unsafe fn take_string(value: *mut c_char) -> Option<String> {
    let out = text(value);
    if !value.is_null() {
        logos_consumer_string_free(value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_names_exclude_the_runtimes_own() {
        assert!(is_valid_shell_name("core_demo"));
        assert!(is_valid_shell_name("basecamp"));
        for bad in ["", "Core_Service", "capability_module", "logos_app", "9lives", "a-b", "runtime"] {
            assert!(!is_valid_shell_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_spawn_line_carries_what_was_configured() {
        let config = Config::new("core_demo")
            .modules_dir("/m")
            .bundled_modules_dir("/b")
            .persistence("/p")
            .peering(json!({"name": "phone"}));
        let line = config.spawn_json().unwrap();
        assert_eq!(line, json!({
            "shell": "core_demo",
            "modules_dirs": ["/m"],
            "bundled_modules_dirs": ["/b"],
            "persistence_base_path": "/p",
            "peering_config": {"name": "phone"}
        }));
    }

    #[test]
    fn a_reserved_shell_is_refused_before_anything_starts() {
        assert!(Config::new("core_service").spawn_json().is_err());
    }

    #[test]
    fn timeouts_outside_the_c_int_range_are_refused() {
        assert!(timeout_ms(Duration::from_millis(0)).is_err());
        assert!(timeout_ms(Duration::from_secs(u64::MAX / 2)).is_err());
        assert_eq!(timeout_ms(Duration::from_millis(1500)).unwrap(), 1500);
    }

    #[test]
    fn error_json_yields_its_message() {
        match call_failed("m", "f", r#"{"code":"dispatch_failed","message":"remote/timeout"}"#) {
            LogosError::PluginCallFailed { message, .. } => assert_eq!(message, "remote/timeout"),
            other => panic!("{other:?}"),
        }
    }
}
