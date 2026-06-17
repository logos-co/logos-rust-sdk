//! Link logos-lidl's C ABI — the JSON bridge over the canonical frontend that
//! `src/lidl_ffi.rs` binds. The nix build exports the package prefix as
//! `LOGOS_LIDL_ROOT`; its `lib/` holds the static archives. `logos_lidl_c`
//! (the C ABI) depends on the `logos_lidl` core and the C++ standard library.
//!
//! These directives propagate to anything that links this crate — including the
//! lib consumed as a build-dependency (e.g. the test fixtures' build scripts).
//! Under nix, `logos-lidl` is also a buildInput, so the archives are on the
//! linker's default search path even where the `-L` below isn't inherited.

fn main() {
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
