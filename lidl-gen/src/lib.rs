//! LIDL → Rust codegen backend.
//!
//! Consumes the language-neutral `.lidl` interchange — parsed/serialized by the
//! canonical logos-lidl frontend over its C ABI (`lidl_ffi`) rather than a Rust
//! reimplementation of the grammar — and emits a typed Rust client over
//! `logos_rust_sdk`: one method per LIDL `method` (typed params/returns via
//! `PluginProxy::call_json`), one `on_<event>()` subscriber per `event`. The
//! Rust-specific halves (Rust source → AST, AST → Rust code) stay here.

pub mod ast;
pub mod lidl_ffi;
pub mod rust_frontend;
pub mod rustgen;
pub mod rustgen_provider;

pub use ast::ModuleDecl;
pub use lidl_ffi::{parse, serialize};
pub use rust_frontend::extract_from_rust;
pub use rustgen::{generate, generate_deps};
pub use rustgen_provider::generate_provider;
