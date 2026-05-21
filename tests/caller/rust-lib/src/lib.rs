//! Minimal caller module — integration test for logos-rust-sdk IPC.
//!
//! Calls `sdk_test_provider_module.add(a, b)` via the SDK and returns the result.
//! A successful round-trip proves the full IPC stack works end-to-end.

use logos_rust_sdk::LogosModuleSDK;

/// Call sdk_test_provider_module.add(a, b) via IPC and return the result.
///
/// Returns -1 on error so the test harness (logoscore) can detect failures.
#[no_mangle]
pub extern "C" fn sdk_test_caller_call_add(a: i64, b: i64) -> i64 {
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
