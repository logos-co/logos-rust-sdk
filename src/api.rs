//! Main Logos API entry point.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::ptr;
use std::cell::RefCell;

use crate::error::LogosError;
use crate::ffi;
use crate::plugin::PluginProxy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiState {
    /// API created but not initialized
    Created,
    /// API initialized (logos_core_init called)
    Initialized,
    /// API started (logos_core_start called)
    Started,
}

pub struct LogosAPI {
    state: ApiState,
    plugins_dir: Option<String>,
    /// Cache of plugin proxies
    plugin_cache: RefCell<HashMap<String, PluginProxy>>,
}

impl LogosAPI {
    pub fn new() -> Result<Self, LogosError> {
        unsafe {
            ffi::logos_core_init(0, ptr::null_mut());
        }

        Ok(LogosAPI {
            state: ApiState::Initialized,
            plugins_dir: None,
            plugin_cache: RefCell::new(HashMap::new()),
        })
    }

    pub fn set_plugins_dir(&mut self, path: &str) -> Result<(), LogosError> {
        if self.state == ApiState::Started {
            return Err(LogosError::AlreadyStarted);
        }

        let path_c = CString::new(path)?;
        
        unsafe {
            ffi::logos_core_set_plugins_dir(path_c.as_ptr());
        }

        self.plugins_dir = Some(path.to_string());
        Ok(())
    }

    pub fn start(&mut self) -> Result<(), LogosError> {
        if self.state == ApiState::Started {
            return Err(LogosError::AlreadyStarted);
        }

        if self.state != ApiState::Initialized {
            return Err(LogosError::NotInitialized);
        }

        unsafe {
            ffi::logos_core_start();
        }

        self.state = ApiState::Started;
        Ok(())
    }

    pub fn process_events(&self) {
        unsafe {
            ffi::logos_core_process_events();
        }
    }

    pub fn load_plugin(&self, name: &str) -> Result<(), LogosError> {
        if self.state != ApiState::Started {
            return Err(LogosError::NotInitialized);
        }

        let name_c = CString::new(name)?;
        let result = unsafe { ffi::logos_core_load_plugin(name_c.as_ptr()) };

        if result != 0 {
            Ok(())
        } else {
            Err(LogosError::PluginLoadFailed(name.to_string()))
        }
    }

    pub fn load_plugins(&self, names: &[&str]) -> Result<(), LogosError> {
        for name in names {
            self.load_plugin(name)?;
        }
        Ok(())
    }

    pub fn unload_plugin(&self, name: &str) -> Result<(), LogosError> {
        if self.state != ApiState::Started {
            return Err(LogosError::NotInitialized);
        }

        let name_c = CString::new(name)?;
        let result = unsafe { ffi::logos_core_unload_plugin(name_c.as_ptr()) };

        if result != 0 {
            // Remove from cache if present
            self.plugin_cache.borrow_mut().remove(name);
            Ok(())
        } else {
            Err(LogosError::PluginUnloadFailed(name.to_string()))
        }
    }

    pub fn process_plugin(&self, path: &str) -> Result<String, LogosError> {
        let path_c = CString::new(path)?;
        
        let result = unsafe { ffi::logos_core_process_plugin(path_c.as_ptr()) };

        if result.is_null() {
            Err(LogosError::PluginProcessFailed(path.to_string()))
        } else {
            let name = unsafe { CStr::from_ptr(result) }
                .to_string_lossy()
                .into_owned();
            Ok(name)
        }
    }

    pub fn get_loaded_plugins(&self) -> Vec<String> {
        let mut plugins = Vec::new();

        unsafe {
            let plugins_ptr = ffi::logos_core_get_loaded_plugins();
            if !plugins_ptr.is_null() {
                let mut i = 0;
                loop {
                    let plugin_ptr = *plugins_ptr.offset(i);
                    if plugin_ptr.is_null() {
                        break;
                    }
                    if let Ok(plugin) = CStr::from_ptr(plugin_ptr).to_str() {
                        plugins.push(plugin.to_string());
                    }
                    i += 1;
                }
            }
        }

        plugins
    }

    pub fn get_known_plugins(&self) -> Vec<String> {
        let mut plugins = Vec::new();

        unsafe {
            let plugins_ptr = ffi::logos_core_get_known_plugins();
            if !plugins_ptr.is_null() {
                let mut i = 0;
                loop {
                    let plugin_ptr = *plugins_ptr.offset(i);
                    if plugin_ptr.is_null() {
                        break;
                    }
                    if let Ok(plugin) = CStr::from_ptr(plugin_ptr).to_str() {
                        plugins.push(plugin.to_string());
                    }
                    i += 1;
                }
            }
        }

        plugins
    }

    pub fn plugin(&self, name: &str) -> PluginProxy {
        PluginProxy::new(name)
    }

    pub fn is_started(&self) -> bool {
        self.state == ApiState::Started
    }

    pub fn is_initialized(&self) -> bool {
        self.state != ApiState::Created
    }

    pub fn plugins_dir(&self) -> Option<&str> {
        self.plugins_dir.as_deref()
    }
}

impl Drop for LogosAPI {
    fn drop(&mut self) {
        unsafe {
            ffi::logos_core_cleanup();
        }
    }
}
