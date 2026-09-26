// logos-rust-sdk declares `extern "C"` bindings to the lp_* C ABI exported by
// logos-protocol, left unresolved at Rust compile time (rlib output). They are
// satisfied at final link time: for a module plugin, against the logos-protocol
// archive embedded in the plugin; for an out-of-plugin caller, against
// liblogos_protocol_plain (see lib.callerBuildSupport in flake.nix).
//
// With the `host` feature an app also links liblogos, and the plain protocol
// image liblogos itself uses, from LOGOS_HOST_LIB_DIR.
fn main() {
    println!("cargo:rerun-if-env-changed=LOGOS_HOST_LIB_DIR");
    if std::env::var_os("CARGO_FEATURE_HOST").is_none() {
        return;
    }
    if let Some(dir) = std::env::var_os("LOGOS_HOST_LIB_DIR") {
        println!("cargo:rustc-link-search=native={}", dir.to_string_lossy());
        // liblogos is @rpath-named; this reaches only this crate's own tests,
        // an app sets its own rpath (or bundles the libraries beside it).
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.to_string_lossy());
    }
    println!("cargo:rustc-link-lib=dylib=logos_core");
    println!("cargo:rustc-link-lib=dylib=logos_protocol_plain");
}
