//! Generates the module-impl C ABI scaffold (logos_module_* exports, typed
//! trait, RustModuleContext) from the .lidl contract — the Rust half of the
//! cdylib authoring path. The Qt glue half is generated from the same
//! contract by logos-module-builder (interface = "cdylib").

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let lidl = std::path::Path::new(&manifest).join("sdk_test_provider_module.lidl");
    println!("cargo:rerun-if-changed={}", lidl.display());

    let source = std::fs::read_to_string(&lidl).expect("read .lidl contract");
    let module = logos_lidl_gen::parse(&source).expect("parse .lidl contract");

    // The logos-protocol semver this module is built against (surfaced via
    // logos_module_get_protocol_version). The nix build exports it; plain
    // cargo builds fall back to the current protocol version.
    let version =
        std::env::var("LOGOS_PROTOCOL_VERSION").unwrap_or_else(|_| "0.1.0".to_string());
    let code = logos_lidl_gen::generate_provider(&module, &version);

    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("provider_gen.rs");
    std::fs::write(&out, code).expect("write generated provider scaffold");
}
