//! Minimal caller module — integration test for logos-rust-sdk IPC, on the
//! common cdylib authoring path.
//!
//! Calls `sdk_test_provider_module.add(a, b)` via the SDK and returns the
//! result. A successful round-trip proves the full stack end-to-end: host →
//! Qt glue → C ABI dispatch → this impl → SDK lp_* call → provider → back.
//! The host-issued auth token reaches the SDK's protocol stack through the
//! generated `logos_module_accept_token` handshake — the seam the legacy
//! c-ffi path could not cross.

include!(concat!(env!("OUT_DIR"), "/provider_gen.rs"));

use logos_rust_sdk::LogosModuleSDK;

#[derive(Default)]
struct CallerImpl;

impl SdkTestCallerModule for CallerImpl {
    /// Call sdk_test_provider_module.add(a, b) via IPC and return the result.
    ///
    /// Returns -1 on error so the test harness (logoscore) can detect failures.
    fn call_add(&mut self, a: i64, b: i64) -> i64 {
        let sdk = LogosModuleSDK::new();
        let provider = sdk.plugin("sdk_test_provider_module");
        match provider.call_sync("add", &[a, b]) {
            Ok(r) if r.success => r.message.parse::<i64>().unwrap_or(-1),
            Ok(r) => {
                eprintln!("sdk_test_caller: add() failed: {}", r.message);
                -1
            }
            Err(e) => {
                eprintln!("sdk_test_caller: add() IPC error: {}", e);
                -1
            }
        }
    }
}

#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<CallerImpl>();
}
