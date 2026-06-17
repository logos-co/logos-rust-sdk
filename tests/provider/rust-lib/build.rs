//! Generates the module-impl C ABI scaffold (logos_module_* exports, typed
//! trait, RustModuleContext) from the .lidl contract — the Rust half of the
//! cdylib authoring path. The Qt glue half is generated from the same
//! contract by logos-module-builder (interface = "cdylib").
//!
//! The `.lidl` is parsed by shelling out to the prebuilt `logos-lidl-gen` CLI
//! (on PATH) into `$OUT_DIR/module_ast.json`; this build script reconstructs the
//! AST via `from_json` and runs the pure-Rust codegen. lidl-gen is depended on
//! with `default-features = false` so it links no C ABI here — that link cannot
//! be satisfied for a HOST build-dependency under nixpkgs, so the C frontend
//! runs out-of-process in the CLI instead.

use std::path::Path;
use std::process::Command;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let lidl = Path::new(&manifest).join("sdk_test_provider_module.lidl");
    println!("cargo:rerun-if-changed={}", lidl.display());

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let ast = Path::new(&out_dir).join("module_ast.json");

    let status = Command::new("logos-lidl-gen")
        .arg(&lidl)
        .arg("--to-json")
        .arg("-o")
        .arg(&ast)
        .status()
        .expect("run logos-lidl-gen --to-json (is it on PATH?)");
    assert!(status.success(), "logos-lidl-gen --to-json failed");

    let json = std::fs::read_to_string(&ast).expect("read generated module_ast.json");
    let module = logos_lidl_gen::from_json(&json).expect("parse module AST json");

    // The logos-protocol semver this module is built against (surfaced via
    // logos_module_get_protocol_version). The nix build exports it; plain
    // cargo builds fall back to the current protocol version.
    let version =
        std::env::var("LOGOS_PROTOCOL_VERSION").unwrap_or_else(|_| "0.1.0".to_string());
    let code = logos_lidl_gen::generate_provider(&module, &version);

    std::fs::write(Path::new(&out_dir).join("provider_gen.rs"), code)
        .expect("write generated provider scaffold");
}
