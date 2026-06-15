# logos-rust-sdk

A Rust SDK for calling other Logos modules from within a Logos module. Consumes the language-neutral **`lp_*` C ABI** from [`logos-protocol`](https://github.com/logos-co/logos-protocol) directly — typed sync/async calls, event subscriptions, and (via `logos-lidl-gen`) **LIDL-generated typed clients**.

## Overview

When writing a Logos module in Rust, you need a way to call methods on other loaded modules and subscribe to their events. This SDK provides that — it binds the `lp_*` C ABI directly and handles parameter serialization, callback trampolines, and channel-based result delivery.

The SDK is a **pure Rust rlib** with no build-time C library dependency. On the standard authoring path (a cdylib module built by logos-module-builder, `interface = "cdylib"`) the `lp_*` symbols resolve against the logos-protocol archive already linked into the plugin — one protocol stack shared by the generated Qt glue and your Rust code. For binaries *outside* a module (CLI tools, tests), the flake's `lib.callerBuildSupport` provides `logos-module-client`'s shared library, which re-exports the same `lp_*` surface.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
logos-rust-sdk = { path = "../../logos-rust-sdk" }
```

Inside a builder-built cdylib module no extra linking is needed — the plugin already carries the protocol stack. For a standalone (out-of-module) binary, link `liblogos_module_client` via the flake's `lib.callerBuildSupport` (it provides the build inputs and a setup hook).

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

// Subscribe to events. The returned EventSubscription OWNS the underlying
// subscription and a share of the client, so it keeps receiving after the
// proxy is dropped — move it into a listener thread and iterate it.
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

### `EventSubscription` and `EventData`

`on()` returns an `EventSubscription`: a `Send` handle bundling the event
channel with ownership of the lp subscription and a share of the client
(dropping a bare proxy would otherwise silently kill the subscription).
It supports `recv()`, `try_recv()`, blocking iteration (`for ev in sub`),
and unsubscribes on drop. Each received item is an `EventData`:

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

`logos-rust-sdk` declares `extern "C"` bindings to the `lp_*` protocol functions but links no C library at Rust compilation time. The Rust crate compiles to an `rlib` (or `staticlib` when used by a module), and the symbols resolve at final link:

```
librust_my_module.a       (your Rust staticlib, contains unresolved lp_* refs)
        ↓
CMake links plugin .dylib
  + logos-protocol archive          ← lp_* resolved here (one shared stack
        ↓                              with the generated Qt glue)
my_module_plugin.dylib    (complete, loadable Logos module)
```

Standalone binaries instead resolve `lp_*` against `liblogos_module_client.so` (see `lib.callerBuildSupport` in `flake.nix`).

## Example: using the SDK inside a Logos module

The standard pattern is the **cdylib authoring path** — see the executable doc-test (`doctests/cross-language-composition.test.yaml`) for a complete walkthrough that composes a Rust consumer over a Rust and a C++ provider, exercising module context, sync/async typed calls, event subscription, and concrete + interface dependencies. The caller side uses a typed client generated from the dependency's `.lidl` contract — every cross-module call is typed, never string-keyed:

```rust
// The builder generated `modules()` from the dep contracts; the trait + scaffold
// come from your own .lidl. No build.rs — metadata.json's codegen.rust drives it.
impl MyModule for MyImpl {
    // Synchronous typed call.
    fn total_via_dep(&mut self) -> i64 {
        match modules().counter.increment(1) {
            Ok(v) => v,
            Err(e) => { eprintln!("call failed: {}", e); -1 }
        }
    }

    // Asynchronous twin: fire and receive the typed result in a callback that
    // runs on the module's event loop after this method returns.
    fn bump_async(&mut self) {
        modules().counter.increment_async(1, |res| {
            if let Ok(v) = res { /* stash v; read it from a later method */ }
        });
    }
}
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

Two complementary checks exercise the SDK and the Rust-module pipeline it builds on (the cross-language composition showcases — typed calls and events crossing the C++/Rust boundary in both directions — live in [logos-module-builder](https://github.com/logos-co/logos-module-builder)'s doctests, since they exercise the builder across both SDKs):

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

- **Executable doc-test** (`doctests/cross-language-composition.test.yaml`) — a
  step-by-step, runnable tutorial that writes three modules from scratch on the
  **builder-driven cdylib path** (no `build.rs`): a Rust provider, a C++
  provider, and a Rust consumer that ties them together. It packages each as an
  `.lgx`, installs them with `lgpm`, and drives the consumer through a
  `logoscore` daemon to exercise the whole consumer surface — module context
  (`module_path` / `instance_id` / `instance_persistence_path`), **sync and
  async** typed calls, typed **event subscription**, and both a **concrete** and
  an **interface** dependency. Run it with the shared
  [`doctest`](https://github.com/logos-co/logos-doctest) CLI:

  ```bash
  cd doctests && ./run.sh
  ```

  The rendered tutorial is committed at
  `doctests/outputs/cross-language-composition/cross-language-composition.md`.
