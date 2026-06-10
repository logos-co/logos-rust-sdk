//! LIDL → Rust codegen backend.
//!
//! Parses the language-neutral `.lidl` interchange (the same grammar as
//! logos-lidl's C++ frontend) and emits a typed Rust client over
//! `logos_rust_sdk`: one method per LIDL `method` (typed params/returns via
//! `PluginProxy::call_json`), one `on_<event>()` subscriber per `event`.

pub mod ast;
pub mod parser;
pub mod rustgen;

pub use ast::ModuleDecl;
pub use parser::parse;
pub use rustgen::generate;
