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
    // --- TEMP DIAGNOSTIC (remove before merge) ---------------------------------
    // Determine why the ubuntu ipc-test fixture build cannot find logos_lidl_c
    // when lidl-gen is consumed as a *build-dependency* (host unit). Report the
    // env + archive presence loudly; panic if the env/archive is missing so the
    // message is unmissable in CI even though build-dep build-script warnings are
    // normally suppressed.
    let diag_root = std::env::var("LOGOS_LIDL_ROOT");
    let diag_host = std::env::var("HOST").unwrap_or_default();
    let diag_target = std::env::var("TARGET").unwrap_or_default();
    let diag_a = diag_root
        .as_deref()
        .ok()
        .map(|r| std::path::Path::new(r).join("lib/liblogos_lidl_c.a"));
    let diag_exists = diag_a.as_ref().map(|p| p.exists()).unwrap_or(false);
    println!(
        "cargo:warning=DIAG lidl-gen build.rs: LOGOS_LIDL_ROOT={diag_root:?} HOST={diag_host} TARGET={diag_target} liblogos_lidl_c.a_exists={diag_exists}"
    );
    eprintln!(
        "DIAG-STDERR lidl-gen build.rs: LOGOS_LIDL_ROOT={diag_root:?} HOST={diag_host} TARGET={diag_target} liblogos_lidl_c.a_exists={diag_exists}"
    );
    if !diag_exists {
        panic!(
            "DIAG lidl-gen build.rs: cannot see logos_lidl_c archive — LOGOS_LIDL_ROOT={diag_root:?} HOST={diag_host} TARGET={diag_target}"
        );
    }
    // --- end diagnostic --------------------------------------------------------

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
