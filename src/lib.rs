//! # Logos Rust SDK
//!
//! The runtime behind the **typed, generated clients** a Rust Logos module uses
//! to call other modules. It binds the language-neutral `lp_*` C ABI from
//! `logos-protocol` directly; `logos-module-builder` runs `logos-lidl-gen` to
//! generate a typed client per dependency on top of it, so module code calls
//! `modules().<dep>.<method>(...)` rather than naming this crate's types.
//!
//! Inside a module built on the cdylib path the `lp_*` symbols resolve against
//! the protocol archive already linked into the plugin. No lifecycle management
//! is needed — connections are established lazily on first call.
//!
//! ## Example (generated typed client, inside a Logos module)
//!
//! ```ignore
//! impl MyModule for MyImpl {
//!     // Synchronous typed call to a concrete dependency.
//!     fn total(&mut self) -> i64 {
//!         modules().counter_module.increment(1).unwrap_or(-1)
//!     }
//!
//!     // Asynchronous twin: the typed result lands on the event loop, after
//!     // this method returns.
//!     fn bump_async(&mut self) {
//!         modules().counter_module.increment_async(1, |res| {
//!             if let Ok(v) = res { /* stash v; read it back from a later method */ }
//!         });
//!     }
//! }
//! ```
//!
//! `modules()`, the typed dependency clients, the trait, and `context()` are all
//! emitted by the builder from the contracts. See the full walkthrough in
//! `doctests/cross-language-composition.test.yaml`.

mod ffi;
pub mod args;
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
pub use plugin::{EventSubscription, PluginProxy};
pub use api::{protocol_abi_major, protocol_version, save_token, LogosModuleSDK};
