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
pub use api::{current_caller, current_caller_json, grant_host_services, module_origin,
              protocol_abi_major, protocol_version, save_token, set_call_caller,
              set_module_origin, set_unload_done_callback, unload_finished, LogosCaller,
              LogosModuleSDK, Shutdown, UnloadDoneCb};

// EVERY PATH THE GENERATED PROVIDER SCAFFOLD SPELLS, RESOLVED AT COMPILE TIME.
//
// lidl-gen emits calls like `logos_rust_sdk::save_token(&name, &tok)`
// as TEXT, and lidl-gen's own tests only grep that text. Nothing else in this
// repo compiles a scaffold against this crate at a protocol version where the
// newest call is emitted: `module-impl-abi` reads the emitted source and never
// builds it, and `ipc-test` builds modules at whatever protocol
// logos-module-builder pins, which lags the declaration by design.
//
// So a function that exists in api.rs but is missing from the `pub use` above
// passes every check in this repo and fails three repos downstream, in a module
// build, with "cannot find function ... in crate `logos_rust_sdk`" and a
// dead-code warning as the only hint. That is not hypothetical: it is what the
// first cut of the 0.8 inbound door did, and the failure surfaced only when a
// real module was compiled against a real 0.8 protocol.
//
// A `use` resolves the path and generates no code, so this costs nothing and
// cannot introduce an undefined lp_* symbol into a test binary. Add a line here
// whenever the generator learns to call something new BY PATH.
//
// Note what is deliberately absent: the 0.8 inbound door. lidl-gen declares and
// calls lp_token_save_inbound inside its own gated block precisely so that no
// unconditional item in this crate references a symbol older protocols do not
// export -- see the note in ffi.rs.
#[allow(unused_imports)]
mod generated_scaffold_paths {
    use crate::{
        grant_host_services, save_token, set_call_caller, set_module_origin,
        set_unload_done_callback, unload_finished,
    };
}
