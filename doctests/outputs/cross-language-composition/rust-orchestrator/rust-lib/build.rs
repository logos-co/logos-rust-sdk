//! Generates from the COMMITTED .lidl (derived from src/lib.rs's trait by
//! `logos-lidl-gen --from-rust` — see the tour step): the module-impl
//! C ABI scaffold around the author's trait (emit_trait = false) plus the
//! typed dependency clients + Modules aggregate from the deps' contracts.

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let root = std::path::Path::new(&manifest);
    let lidl = root.join("rust_orchestrator_module.lidl");
    let dep = root.join("deps/cpp_counter_module.lidl");
    println!("cargo:rerun-if-changed={}", lidl.display());
    println!("cargo:rerun-if-changed={}", dep.display());

    let module =
        logos_lidl_gen::parse(&std::fs::read_to_string(&lidl).expect("read contract"))
            .expect("parse contract");
    let counter =
        logos_lidl_gen::parse(&std::fs::read_to_string(&dep).expect("read dep contract"))
            .expect("parse dep contract");

    let version =
        std::env::var("LOGOS_PROTOCOL_VERSION").unwrap_or_else(|_| "0.1.0".to_string());
    let mut code =
        logos_lidl_gen::rustgen_provider::generate_provider_with(&module, &version, false);
    code.push('\n');
    code.push_str(&logos_lidl_gen::generate_deps(&[(
        "counter".to_string(),
        counter,
    )]));

    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("provider_gen.rs");
    std::fs::write(&out, code).expect("write generated scaffold");
}
