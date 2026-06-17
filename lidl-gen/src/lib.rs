//! LIDL → Rust codegen backend.
//!
//! Consumes the language-neutral `.lidl` interchange — parsed/serialized by the
//! canonical logos-lidl frontend over its C ABI (`lidl_ffi`) rather than a Rust
//! reimplementation of the grammar — and emits a typed Rust client over
//! `logos_rust_sdk`: one method per LIDL `method` (typed params/returns via
//! `PluginProxy::call_json`), one `on_<event>()` subscriber per `event`. The
//! Rust-specific halves (Rust source → AST, AST → Rust code) stay here.

pub mod ast;
// The C-ABI frontend (`parse`/`serialize` over logos-lidl) is behind the `ffi`
// feature (on by default). It links logos-lidl's static C archives, which cannot
// be put on the linker path of a HOST build-dependency under nixpkgs (see
// build.rs). Build scripts that consume this crate as a build-dependency turn
// the feature off and feed a pre-parsed AST through `from_json` instead.
#[cfg(feature = "ffi")]
pub mod lidl_ffi;
pub mod rust_frontend;
pub mod rustgen;
pub mod rustgen_provider;

pub use ast::ModuleDecl;
#[cfg(feature = "ffi")]
pub use lidl_ffi::{parse, serialize};
pub use rust_frontend::extract_from_rust;
pub use rustgen::{generate, generate_deps};
pub use rustgen_provider::generate_provider;

/// Deserialize a [`ModuleDecl`] from its JSON form — the AST shape emitted by
/// logos-lidl's `lidl_parse_to_json` C ABI and the `logos-lidl-gen --to-json`
/// CLI mode. Pure Rust, available without the `ffi` feature, so a build script
/// can pre-parse the `.lidl` at an outer build step (where the C frontend *can*
/// link) and codegen from the JSON here.
pub fn from_json(json: &str) -> Result<ModuleDecl, serde_json::Error> {
    serde_json::from_str(json)
}
