//! Link logos-lidl's C ABI — only under the `ffi` feature (the default). The
//! archives' directory comes from `LOGOS_LIDL_ROOT`, which the nix build exports
//! and which every TARGET-unit compile (the lidl-gen package / CLI, ordinary
//! library use) sees.
//!
//! Why feature-gated: when this crate is a *build-dependency* under nixpkgs,
//! CARGO_BUILD_TARGET makes it a HOST unit and keeps env vars / RUSTFLAGS /
//! input lib dirs out of that host build — so no `-L` can reach the rlib's
//! bundle of `logos_lidl_c` and the link fails ("could not find native static
//! library"). Such consumers (the test fixtures) build with
//! `default-features = false`, pre-parse the `.lidl` to JSON at an outer step,
//! and use `from_json`; this build script then emits nothing.

fn main() {
    // No native linking unless the C-ABI frontend is compiled in.
    if std::env::var_os("CARGO_FEATURE_FFI").is_none() {
        return;
    }
    if let Ok(root) = std::env::var("LOGOS_LIDL_ROOT") {
        println!("cargo:rustc-link-search=native={root}/lib");
    }
    // Order matters for static linking: the C ABI uses the core's symbols.
    println!("cargo:rustc-link-lib=static=logos_lidl_c");
    println!("cargo:rustc-link-lib=static=logos_lidl");
    // logos_lidl_c is C++ (uses nlohmann/json) — pull in the C++ runtime.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" || target_os == "ios" {
        println!("cargo:rustc-link-lib=dylib=c++");
    } else {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
    println!("cargo:rerun-if-env-changed=LOGOS_LIDL_ROOT");
}
