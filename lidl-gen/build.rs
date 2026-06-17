//! Link logos-lidl's C ABI — the JSON bridge over the canonical frontend that
//! `src/lidl_ffi.rs` binds. `logos_lidl_c` (the C ABI) depends on the
//! `logos_lidl` core and the C++ standard library; both are static archives.
//!
//! Finding the archives' directory is the tricky part under nix. The obvious
//! `LOGOS_LIDL_ROOT` (the package prefix) works for every TARGET-unit compile,
//! but nixpkgs sets CARGO_BUILD_TARGET, so cargo treats this crate as a HOST
//! unit when it is a *build-dependency* (the test fixtures build-depend on it to
//! generate their scaffold) and keeps env vars / RUSTFLAGS out of that host
//! build script — `LOGOS_LIDL_ROOT` is then unset and no `-L` gets emitted.
//! `CARGO_MANIFEST_DIR`, by contrast, is handed to EVERY build script, host or
//! target. So the nix builder copies the archives into `<crate>/nix-lib` (see
//! flake.nix mkFixtureRustLib) and we search there first; the LOGOS_LIDL_ROOT
//! path remains the fallback for ordinary (target-unit / standalone) builds.

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let nix_lib = std::path::Path::new(&manifest).join("nix-lib");
    // TEMP DIAG3 (remove before merge): why ubuntu's host build-dep rlib can't
    // find the archive even though darwin's can.
    println!(
        "cargo:warning=DIAG3 manifest={manifest} nix_lib_exists={} a_exists={} listing={:?}",
        nix_lib.exists(),
        nix_lib.join("liblogos_lidl_c.a").exists(),
        std::fs::read_dir(std::path::Path::new(&manifest))
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned())).collect::<Vec<_>>())
            .unwrap_or_else(|e| vec![format!("readdir-err: {e}")])
    );
    if nix_lib.join("liblogos_lidl_c.a").exists() {
        println!("cargo:rustc-link-search=native={}", nix_lib.display());
    } else if let Ok(root) = std::env::var("LOGOS_LIDL_ROOT") {
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
