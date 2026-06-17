//! Link logos-lidl's C ABI — the JSON bridge over the canonical frontend that
//! `src/lidl_ffi.rs` binds. The nix build exports the package prefix as
//! `LOGOS_LIDL_ROOT`; its `lib/` holds the static archives. `logos_lidl_c`
//! (the C ABI) depends on the `logos_lidl` core and the C++ standard library.
//!
//! The archives are linked with the `-bundle` modifier so they are NOT bundled
//! into this crate's rlib: rustc would otherwise have to *find* `logos_lidl_c.a`
//! at rlib-compile time (via a `-L native=` search path), which fails when this
//! crate is consumed as a *build-dependency* (a HOST unit) under nixpkgs —
//! CARGO_BUILD_TARGET keeps LOGOS_LIDL_ROOT and RUSTFLAGS out of host build
//! scripts, so no search path is available there. With `-bundle` the requirement
//! defers to the final link of whatever includes this crate (e.g. the test
//! fixtures' build-script executable), where nix's cc-wrapper resolves it from
//! logos-lidl in nativeBuildInputs/buildInputs. The `-L native=` below (emitted
//! when LOGOS_LIDL_ROOT is visible, i.e. target-unit compiles) is a belt for
//! that final link.

fn main() {
    if let Ok(root) = std::env::var("LOGOS_LIDL_ROOT") {
        println!("cargo:rustc-link-search=native={root}/lib");
    }
    // `-bundle`: don't copy the archive into this rlib (see module docs) — defer
    // the link to the final artifact, where the cc-wrapper supplies the path.
    // Order matters for static linking: the C ABI uses the core's symbols.
    println!("cargo:rustc-link-lib=static:-bundle=logos_lidl_c");
    println!("cargo:rustc-link-lib=static:-bundle=logos_lidl");
    // logos_lidl_c is C++ (uses nlohmann/json) — pull in the C++ runtime.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" || target_os == "ios" {
        println!("cargo:rustc-link-lib=dylib=c++");
    } else {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
    println!("cargo:rerun-if-env-changed=LOGOS_LIDL_ROOT");
}
