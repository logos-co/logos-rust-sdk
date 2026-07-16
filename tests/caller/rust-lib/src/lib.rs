//! Minimal caller module — integration test for logos-rust-sdk IPC, on the
//! common cdylib authoring path.
//!
//! Calls `sdk_test_provider_module.add(a, b)` via the SDK and returns the
//! result. A successful round-trip proves the full stack end-to-end: host →
//! Qt glue → C ABI dispatch → this impl → SDK lp_* call → provider → back.
//! The host-issued auth token reaches the SDK's protocol stack through the
//! generated `logos_module_accept_token` handshake — the seam the legacy
//! c-ffi path could not cross.

include!("provider_gen.rs");

use logos_rust_sdk::LogosModuleSDK;

// The bytes the `blobReady` subscription last received, summarised. -1 means the
// event has not been seen yet. Size proves the payload arrived non-empty (the
// #99-class drop left subscribers with zero bytes); the checksum proves the
// content is intact, not merely the right length.
static LAST_BLOB_SIZE: std::sync::Mutex<i64> = std::sync::Mutex::new(-1);
static LAST_BLOB_CHECKSUM: std::sync::Mutex<i64> = std::sync::Mutex::new(-1);

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

    fn last_blob_size(&mut self) -> i64 {
        *LAST_BLOB_SIZE.lock().unwrap()
    }

    fn last_blob_checksum(&mut self) -> i64 {
        *LAST_BLOB_CHECKSUM.lock().unwrap()
    }

    /// Subscribe to the provider's binary `blobReady` event once the host has
    /// wired this module up. The provider encodes the payload as the canonical
    /// tagged {"_bytes": ...} value; here it is decoded back to raw bytes with
    /// the SDK's own codec — the round trip the codegen exists to make work.
    fn on_context_ready(&mut self, _ctx: &RustModuleContext) {
        let sdk = LogosModuleSDK::new();
        let mut provider = sdk.plugin("sdk_test_provider_module");
        match provider.on("blobReady") {
            Ok(sub) => {
                std::thread::spawn(move || {
                    for ev in sub {
                        // Event args arrive as a JSON array: [seq, {"_bytes": ...}].
                        let payload = ev
                            .data
                            .as_array()
                            .and_then(|a| a.get(1))
                            .and_then(logos_rust_sdk::bytes::decode)
                            .unwrap_or_default();
                        let mut sum: i64 = 0;
                        for (i, b) in payload.iter().enumerate() {
                            sum += (*b as i64) * ((i as i64) % 31 + 1);
                        }
                        *LAST_BLOB_SIZE.lock().unwrap() = payload.len() as i64;
                        *LAST_BLOB_CHECKSUM.lock().unwrap() = sum;
                    }
                });
            }
            Err(e) => eprintln!("sdk_test_caller: subscribe(blobReady) failed: {}", e),
        }
    }
}

#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<CallerImpl>();
}
