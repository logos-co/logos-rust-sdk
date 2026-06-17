//! Generates the module-impl C ABI scaffold (logos_module_* exports, typed
//! trait, RustModuleContext) from the .lidl contract — the Rust half of the
//! cdylib authoring path. The Qt glue half is generated from the same
//! contract by logos-module-builder (interface = "cdylib").
//!
//! The `.lidl` is parsed into `module_ast.json` (next to it in the crate source)
//! by the nix builder, with `logos-lidl-gen --to-json`; this build script
//! reconstructs the AST via `from_json` and runs the pure-Rust codegen. lidl-gen
//! is depended on with `default-features = false` so it links no C ABI here —
//! that link cannot be satisfied for a HOST build-dependency under nixpkgs, and
//! the C frontend (env vars / PATH / input dirs / post-unpack writes are all
//! kept from a host build script there) runs at the outer build step instead.

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let ast = std::path::Path::new(&manifest).join("module_ast.json");
    println!("cargo:rerun-if-changed={}", ast.display());

    let json = std::fs::read_to_string(&ast).expect("read pre-parsed module_ast.json");
    let module = logos_lidl_gen::from_json(&json).expect("parse module AST json");

    // The logos-protocol semver this module is built against (surfaced via
    // logos_module_get_protocol_version). The nix build exports it; plain
    // cargo builds fall back to the current protocol version.
    let version =
        std::env::var("LOGOS_PROTOCOL_VERSION").unwrap_or_else(|_| "0.1.0".to_string());
    let code = logos_lidl_gen::generate_provider(&module, &version);

    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("provider_gen.rs");
    std::fs::write(&out, code).expect("write generated provider scaffold");
}
