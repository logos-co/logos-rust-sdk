// No build-time linking needed.
//
// logos-rust-sdk declares `extern "C"` bindings to the lp_* C ABI exported by
// logos-protocol. These symbols are intentionally left unresolved at Rust
// compile time (rlib output). They are satisfied at final link time: for a
// module plugin, against the logos-protocol archive embedded in the plugin;
// for an out-of-plugin caller, against liblogos_protocol.{so,dylib} (see
// lib.callerBuildSupport in flake.nix).
fn main() {}
