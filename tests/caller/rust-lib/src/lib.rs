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

/// The typed CONSUMER client for the dependency's contract — what
/// `logos-lidl-gen` (no `--provider`) emits and what real modules reach through
/// `modules().<dep>`. Checked in for the same reason `provider_gen.rs` is; see
/// the note on `mkFixtureRustLib` in ../../../flake.nix. Regenerate with:
///
///     logos-lidl-gen ../provider/rust-lib/sdk_test_provider_module.lidl \
///         -o src/provider_client_gen.rs
///
/// It is what makes the timeout assertions below cover the GENERATED wrapper
/// and not just the hand-written proxy underneath it.
#[allow(dead_code)] // the fixture uses a few of the generated entry points, not all
mod provider_client {
    include!("provider_client_gen.rs");
}

use logos_rust_sdk::LogosModuleSDK;
use provider_client::SdkTestProviderModuleClient;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

// The bytes the `blobReady` subscription last received, summarised. -1 means the
// event has not been seen yet. Size proves the payload arrived non-empty (the
// #99-class drop left subscribers with zero bytes); the checksum proves the
// content is intact, not merely the right length.
static LAST_BLOB_SIZE: std::sync::Mutex<i64> = std::sync::Mutex::new(-1);
static LAST_BLOB_CHECKSUM: std::sync::Mutex<i64> = std::sync::Mutex::new(-1);

// Outcome of the last start_timed_call_async, written from the SDK's async
// completion path. -1 = still in flight.
static ASYNC_ELAPSED_MS: AtomicI64 = AtomicI64::new(-1);
static ASYNC_OK: AtomicI64 = AtomicI64::new(-1);

// Elapsed times of the two back-to-back calls made by same_client_two_timeouts,
// same encoding as `encode` below. -1 = not run yet.
static PAIR_A_MS: AtomicI64 = AtomicI64::new(-1);
static PAIR_B_MS: AtomicI64 = AtomicI64::new(-1);

/// ONE provider client for the whole process, held for the module's lifetime —
/// the way a real module holds `modules().<dep>`.
///
/// Every timing measurement below goes through THIS object. That is what makes
/// the assertions about per-call timeouts mean anything: the client is a fixed
/// point, so a call that comes back at its own bound can only have got that
/// bound from the call itself. `provider_client_addr` hands the harness its
/// address so "same client" is checked rather than asserted.
fn provider() -> &'static SdkTestProviderModuleClient {
    static CLIENT: OnceLock<SdkTestProviderModuleClient> = OnceLock::new();
    CLIENT.get_or_init(SdkTestProviderModuleClient::new)
}

/// Elapsed milliseconds, negated when the call actually completed. One `int`
/// return carries both facts, and the sign can't be confused with the
/// magnitude because every sleep in the suite is far from zero.
fn encode(elapsed: Duration, completed: bool) -> i64 {
    let ms = elapsed.as_millis() as i64;
    if completed {
        -ms
    } else {
        ms
    }
}

/// One measured `sleep(sleep_ms)` on the shared client.
///
/// `timeout_ms > 0` takes the BOUNDED entry point, `sleep_with_timeout`;
/// `timeout_ms <= 0` takes the untouched `sleep`, exactly as an existing call
/// site writes it, so the unbounded arm really exercises the default path
/// rather than a 20s bound spelled out by hand. Same client either way — only
/// the entry point differs, which is the whole shape of the change.
fn measure_sync(sleep_ms: i64, timeout_ms: i64) -> i64 {
    let started = Instant::now();
    let outcome = if timeout_ms > 0 {
        provider().sleep_with_timeout(sleep_ms, Duration::from_millis(timeout_ms as u64))
    } else {
        provider().sleep(sleep_ms)
    };
    let elapsed = started.elapsed();
    let completed = match &outcome {
        Ok(v) => *v == sleep_ms,
        Err(e) => {
            eprintln!(
                "sdk_test_caller: sleep({}ms) under timeout={}ms did not complete: {}",
                sleep_ms, timeout_ms, e
            );
            false
        }
    };
    encode(elapsed, completed)
}

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

    /// SYNC half of the per-call-timeout proof.
    ///
    /// Calls the provider's `sleep(sleep_ms)` through the GENERATED typed
    /// wrapper on the process's ONE provider client — `sleep_with_timeout` when
    /// `timeout_ms > 0`, plain `sleep` otherwise. The clock is what answers the
    /// question: a timeout that is accepted and then dropped on the floor comes
    /// back at `sleep_ms`.
    fn timed_call(&mut self, sleep_ms: i64, timeout_ms: i64) -> i64 {
        measure_sync(sleep_ms, timeout_ms)
    }

    /// TWO calls, ONE client, TWO different bounds, back to back — the thing a
    /// client-scoped timeout could not express without building a second
    /// client, and the reason the timeout is an argument to the call.
    ///
    /// `drain_ms` is slept between them: the provider is single-dispatch and is
    /// still serving the first `sleep(sleep_ms)` after call A gives up, so
    /// without the drain call B would measure the queue rather than its own
    /// bound. Read the two elapsed times back with `last_pair_a_ms` /
    /// `last_pair_b_ms`; returns 1 once both have run.
    fn same_client_two_timeouts(
        &mut self,
        sleep_ms: i64,
        timeout_a_ms: i64,
        timeout_b_ms: i64,
        drain_ms: i64,
    ) -> i64 {
        PAIR_A_MS.store(-1, Ordering::SeqCst);
        PAIR_B_MS.store(-1, Ordering::SeqCst);
        PAIR_A_MS.store(measure_sync(sleep_ms, timeout_a_ms), Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(drain_ms.max(0) as u64));
        PAIR_B_MS.store(measure_sync(sleep_ms, timeout_b_ms), Ordering::SeqCst);
        1
    }

    fn last_pair_a_ms(&mut self) -> i64 {
        PAIR_A_MS.load(Ordering::SeqCst)
    }

    fn last_pair_b_ms(&mut self) -> i64 {
        PAIR_B_MS.load(Ordering::SeqCst)
    }

    /// Address of the process-wide provider client, so the harness can check
    /// that every measurement really did go through the same object rather
    /// than take that on trust.
    fn provider_client_addr(&mut self) -> i64 {
        provider() as *const SdkTestProviderModuleClient as i64
    }

    /// ASYNC half of the same proof, through the generated `sleep_async` /
    /// `sleep_async_with_timeout` on the same shared client. Returns
    /// immediately — the result lands on the module's event loop and is read
    /// back with `last_async_elapsed_ms` / `last_async_ok`.
    fn start_timed_call_async(&mut self, sleep_ms: i64, timeout_ms: i64) -> i64 {
        ASYNC_ELAPSED_MS.store(-1, Ordering::SeqCst);
        ASYNC_OK.store(-1, Ordering::SeqCst);
        let started = Instant::now();
        let on_done = move |result: Result<i64, logos_rust_sdk::LogosError>| {
            let completed = matches!(&result, Ok(v) if *v == sleep_ms);
            if let Err(e) = &result {
                eprintln!("sdk_test_caller: async sleep({}ms) did not complete: {}", sleep_ms, e);
            }
            ASYNC_ELAPSED_MS.store(started.elapsed().as_millis() as i64, Ordering::SeqCst);
            ASYNC_OK.store(i64::from(completed), Ordering::SeqCst);
        };
        if timeout_ms > 0 {
            provider().sleep_async_with_timeout(
                sleep_ms,
                Duration::from_millis(timeout_ms as u64),
                on_done,
            );
        } else {
            provider().sleep_async(sleep_ms, on_done);
        }
        0
    }

    fn last_async_elapsed_ms(&mut self) -> i64 {
        ASYNC_ELAPSED_MS.load(Ordering::SeqCst)
    }

    fn last_async_ok(&mut self) -> i64 {
        ASYNC_OK.load(Ordering::SeqCst)
    }

    /// A timeout the `lp_*` ABI cannot express must be REFUSED, not clamped —
    /// and refused where it was supplied, not on some later call. Returns the
    /// error's Display text (empty if the call was accepted, which is itself
    /// the failure the harness looks for).
    fn refused_timeout_reason(&mut self, timeout_us: i64) -> String {
        match provider().sleep_with_timeout(0, Duration::from_micros(timeout_us.max(0) as u64)) {
            Err(logos_rust_sdk::LogosError::InvalidTimeout { reason, .. }) => reason,
            Err(other) => format!("wrong error: {}", other),
            Ok(_) => String::new(),
        }
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
