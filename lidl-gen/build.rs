//! Link logos-lidl's C ABI — the JSON bridge over the canonical frontend that
//! `src/lidl_ffi.rs` binds. The nix build exports the package prefix as
//! `LOGOS_LIDL_ROOT`; its `lib/` holds the static archives. `logos_lidl_c`
//! (the C ABI) depends on the `logos_lidl` core and the C++ standard library.
//!
//! rustc resolves `static=` native libs through its own `-L native=` search
//! paths (not the C linker's default paths), so the directory holding the
//! archives must be on rustc's search path at every compile that links this
//! crate — including the rlib compile when it is a *build-dependency* (the test
//! fixtures build-depend on it). See flake.nix mkFixtureRustLib for how that
//! path reaches host build-dependency compiles under nix.

fn main() {
    // The `-L native=` search path comes from LOGOS_LIDL_ROOT when this build
    // script can see it (every TARGET-unit compile: the lidl-gen package, the
    // published CLI). When lidl-gen is consumed as a *build-dependency* (a HOST
    // unit), nixpkgs' CARGO_BUILD_TARGET keeps this env var out of the host
    // build-script environment, so the search path is supplied by the nix builder
    // via CARGO_TARGET_<triple>_RUSTFLAGS instead (see flake.nix mkFixtureRustLib).
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
