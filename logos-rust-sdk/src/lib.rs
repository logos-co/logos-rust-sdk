//! # Logos Rust SDK
//!
//! A Rust SDK for interacting with Logos Core modules.
//!
//! This SDK provides a high-level API for:
//! - Initializing and managing the Logos Core lifecycle
//! - Loading and unloading plugins/modules
//! - Calling methods on plugins asynchronously
//! - Subscribing to events from plugins
//!
//! ## Example
//!
//! ```rust,no_run
//! use logos_rust_sdk::{LogosAPI, LogosError};
//!
//! fn main() -> Result<(), LogosError> {
//!     // Initialize the SDK
//!     let logos = LogosAPI::new()?;
//!     logos.set_plugins_dir("/path/to/modules")?;
//!     logos.start()?;
//!
//!     // Load plugins
//!     logos.load_plugin("waku_module")?;
//!     logos.load_plugin("chat")?;
//!
//!     // Get a plugin proxy and call methods
//!     let chat = logos.plugin("chat");
//!     chat.call("initialize", &[])?;
//!     chat.call("joinChannel", &["my-channel"])?;
//!
//!     // Subscribe to events
//!     let messages_rx = chat.on("chatMessage")?;
//!
//!     // Main loop
//!     loop {
//!         logos.process_events();
//!         while let Ok(event) = messages_rx.try_recv() {
//!             println!("Got event: {:?}", event);
//!         }
//!     }
//! }
//! ```

mod ffi;
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
pub use api::LogosAPI;
