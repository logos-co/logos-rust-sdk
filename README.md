# logos-rust-sdk

A Rust SDK for calling other Logos modules from within a Logos module. Consumes the language-neutral **`lp_*` C ABI** from [`logos-protocol`](https://github.com/logos-co/logos-protocol) directly — typed sync/async calls, event subscriptions, and (via `logos-lidl-gen`) **LIDL-generated typed clients**.

## Overview

When writing a Logos module in Rust, you need a way to call methods on other loaded modules and subscribe to their events. This SDK provides that — it sits on top of `logos-module-client`'s `logos_sdk_*` C API and handles parameter serialization, callback trampolines, and channel-based result delivery.

The SDK is a **pure Rust rlib** with no build-time C library dependency. The `logos_sdk_*` symbols it references are resolved at final link time when CMake links your module plugin (`.so`/`.dylib`) against `liblogos_module_client`.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
logos-rust-sdk = { path = "../../logos-rust-sdk" }
```

In your module's `CMakeLists.txt`, ensure `liblogos_module_client` is linked so the FFI symbols resolve:

```cmake
find_library(LOGOS_MODULE_CLIENT_LIB logos_module_client
    HINTS $ENV{LOGOS_MODULE_CLIENT_ROOT}/lib)
target_link_libraries(${MODULE_NAME} PRIVATE ${LOGOS_MODULE_CLIENT_LIB})
```

## Quick Start

```rust
use logos_rust_sdk::LogosModuleSDK;

let sdk = LogosModuleSDK::new();
let provider = sdk.plugin("rust_provider_module");

// Synchronous call — use this inside Q_INVOKABLE-generated functions
let result = provider.call_sync("add", &[5i64, 3i64])?;
println!("5 + 3 = {}", result.message);  // "8"

// Asynchronous call — returns a channel receiver
let rx = provider.call("greet", &["World"])?;
if let Ok(result) = rx.try_recv() {
    println!("{}", result.message);  // "Hello, World! (from Rust provider)"
}
```

## API Reference

### `LogosModuleSDK`

The main entry point. No initialization is needed — `logos-module-client` manages connections lazily.

```rust
use logos_rust_sdk::LogosModuleSDK;

let sdk = LogosModuleSDK::new();           // no-op, no lifecycle management
let proxy = sdk.plugin("some_module");     // get a proxy for any loaded module
sdk.shutdown();                            // optional: release internal connections
```

### `PluginProxy`

A proxy for calling methods and subscribing to events on a specific module.

```rust
let mut proxy = sdk.plugin("other_module");

// Synchronous call (blocks until result arrives via Qt Remote Objects IPC)
// Use this inside Rust functions that are called by Q_INVOKABLE-generated glue
let result = proxy.call_sync("method_name", &[arg1, arg2])?;
if result.success {
    println!("Result: {}", result.message);
}

// Synchronous call with no parameters
let result = proxy.call_sync_no_params("version")?;

// Asynchronous call (returns immediately, result arrives via channel)
let rx = proxy.call("method_name", &["arg1", "arg2"])?;
if let Ok(result) = rx.try_recv() {
    println!("Result: {}", result.message);
}

// Asynchronous call with no parameters
let rx = proxy.call_no_params("initialize")?;

// Asynchronous call with explicit Param types (for mixed-type parameters)
use logos_rust_sdk::params::Param;
let rx = proxy.call_with_params("setValues", &[
    Param::string("name", "Alice"),
    Param::int("age", 30),
    Param::bool("active", true),
])?;

// Subscribe to events
let events = proxy.on("dataChanged")?;
while let Ok(event) = events.try_recv() {
    println!("Event: {} — {:?}", event.event, event.data);
}
```

### `CallResult`

Returned by all `call_*` methods.

```rust
pub struct CallResult {
    pub success: bool,
    pub message: String,  // return value as a string, or error message
}
```

### `EventData`

Delivered through channels returned by `on()`.

```rust
pub struct EventData {
    pub event: String,            // event name
    pub data: serde_json::Value,  // event payload as JSON
}
```

### `ToParam` trait

The `call` and `call_sync` methods accept any slice of values that implement `ToParam`. Built-in implementations:

| Rust type | Logos param type |
|-----------|-----------------|
| `&str`, `String`, `&String` | `"string"` |
| `i32`, `i64`, `u32`, `u64`, `usize` | `"int"` |
| `f32`, `f64` | `"double"` |
| `bool` | `"bool"` |

Parameters are auto-named `arg0`, `arg1`, … matching the order expected by the callee module.

### `LogosError`

All fallible methods return `Result<_, LogosError>`:

| Variant | Cause |
|---------|-------|
| `PluginCallFailed` | Method call returned an error from the remote module |
| `EventListenerFailed` | Event registration failed |
| `InvalidString` | A string argument contained a null byte |
| `JsonError` | Parameter serialization failed |
| `ChannelClosed` | The callback channel was dropped unexpectedly |
| `Other` | Miscellaneous error with a descriptive message |

## How symbols resolve

`logos-rust-sdk` declares `extern "C"` bindings to `logos_sdk_*` functions but does **not** link against `liblogos_module_client` at Rust compilation time. The Rust crate compiles to an `rlib` (or `staticlib` when used by a module). The unresolved `logos_sdk_*` symbols are satisfied when CMake links the final module plugin:

```
librust_caller.a          (your Rust staticlib, contains unresolved logos_sdk_* refs)
        ↓
CMake links plugin .dylib
  + liblogos_module_client.dylib   ← logos_sdk_* symbols resolved here
        ↓
rust_caller_module_plugin.dylib    (complete, loadable Logos module)
```

## Example: using the SDK inside a Logos module

See [`logos-rust-example-module`](https://github.com/logos-co/logos-rust-example-module) for a complete working example with two modules communicating through IPC.

The caller module's `lib.rs` pattern:

```rust
use logos_rust_sdk::LogosModuleSDK;

const PROVIDER: &str = "rust_provider_module";

#[no_mangle]
pub extern "C" fn rust_caller_call_add(a: i64, b: i64) -> i64 {
    let sdk = LogosModuleSDK::new();
    let provider = sdk.plugin(PROVIDER);
    match provider.call_sync("add", &[a, b]) {
        Ok(result) => result.message.parse::<i64>().unwrap_or(-1),
        Err(e) => {
            eprintln!("IPC call failed: {}", e);
            -1
        }
    }
}
```

The corresponding C header declared for `c-ffi` codegen:

```c
int64_t rust_caller_call_add(int64_t a, int64_t b);
```

## Building

The SDK itself has no standalone Nix build artifact — it is a library crate consumed by module builds. To work on it:

```bash
# Enter a dev shell with Rust toolchain
nix develop

# Run unit tests (params serialization, etc.)
cargo test
```

## Testing

Two complementary checks exercise the SDK and the Rust-module pipeline it builds on:

- **IPC integration test** (`tests/`) — builds a minimal provider + caller module
  pair on the **cdylib authoring path**: each fixture is a `.lidl` contract from
  which `lidl-gen --provider` generates the Rust module-impl C ABI scaffold
  (`logos_module_*` exports, typed trait, `RustModuleContext`) and
  logos-module-builder (`interface = "cdylib"`) generates the uniform Qt glue.
  The author writes the trait impl plus a `#[no_mangle] fn logos_module_install()`
  hook; the plugin links one logos-protocol stack shared by the glue and the SDK,
  so the host token forwarded through `logos_module_accept_token` authenticates
  the caller's outbound `add()` call. Verified end-to-end through `logoscore`:

  ```bash
  nix build 'path:./tests#checks.x86_64-linux.ipc-test' \
    --override-input logos-rust-sdk path:. --print-build-logs
  ```

- **Executable doc-test** (`doctests/rust-provider-module.test.yaml`) — a
  step-by-step, runnable tutorial that writes a pure-Rust Logos module from
  scratch (the `provider` pattern: Rust `staticlib` → `c-ffi` codegen → Qt plugin),
  packages it as an `.lgx`, installs it with `lgpm`, and calls its methods through
  a `logoscore` daemon. It documents and verifies the callee side of the IPC stack
  this SDK builds on. Run it with the shared [`doctest`](https://github.com/logos-co/logos-doctest)
  CLI:

  ```bash
  cd doctests && ./run.sh
  ```

  The rendered tutorial is committed at `doctests/outputs/rust-provider-module.md`.
