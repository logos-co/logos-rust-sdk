// No build-time linking needed.
//
// logos-rust-sdk declares `extern "C"` bindings to logos_sdk_* functions from
// logos-module-client. These symbols are intentionally left unresolved at Rust
// compile time (rlib output). They are satisfied at final link time when CMake
// links the module plugin .so/.dylib against liblogos_module_client.
fn main() {}
