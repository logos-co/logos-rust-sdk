//! # Logos Rust SDK
//!
//! A Rust SDK for calling other Logos modules from within a module.
//!
//! This SDK wraps the `logos-module-client` C API (`logos_sdk_*`), which in turn
//! uses `LogosAPI` / `LogosAPIClient` over Qt Remote Objects IPC. No lifecycle
//! management is needed — connections are established lazily on first call.
//!
//! ## Example (inside a Logos module)
//!
//! ```rust,no_run
//! use logos_rust_sdk::{LogosModuleSDK, LogosError};
//!
//! // Call another module synchronously (suitable for use in Q_INVOKABLE-generated functions)
//! fn call_provider_add(a: i64, b: i64) -> i64 {
//!     let sdk = LogosModuleSDK::new();
//!     let provider = sdk.plugin("rust_provider_module");
//!     match provider.call_sync("add", &[a, b]) {
//!         Ok(result) if result.success => result.message.parse().unwrap_or(-1),
//!         _ => -1,
//!     }
//! }
//!
//! // Async call with channel-based result
//! fn call_provider_greet(name: &str) -> Result<(), LogosError> {
//!     let sdk = LogosModuleSDK::new();
//!     let provider = sdk.plugin("rust_provider_module");
//!     let rx = provider.call("greet", &[name])?;
//!     // Check later (non-blocking):
//!     if let Ok(result) = rx.try_recv() {
//!         println!("Greeting: {}", result.message);
//!     }
//!     Ok(())
//! }
//! ```

mod ffi;
pub mod bytes;
mod error;
mod params;
mod callback;
mod plugin;
mod api;

// Re-export public API
pub use error::LogosError;
pub use params::{Param, ToParam};
pub use callback::{CallResult, EventData};
pub use plugin::PluginProxy;
pub use api::{protocol_abi_major, protocol_version, save_token, LogosModuleSDK};
