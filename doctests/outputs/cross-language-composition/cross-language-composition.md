# Cross-Language Composition: a Rust Consumer Driving a Rust and a C++ Module

This doc-test builds **three Logos modules — two providers (one Rust, one
C++) and one Rust consumer that ties them together** — and drives them
end-to-end through a headless `logoscore` daemon. It is the feature tour for
[`logos-rust-sdk`](https://github.com/logos-co/logos-rust-sdk): everything a
Rust module needs to consume other modules, in one place.

The cast:

| Module | Language | Role | Shows |
|---|---|---|---|
| `rust_calc_module` | Rust | provider | a basic Rust module: typed methods + a typed `computed` event emitter |
| `cpp_greeter_module` | C++ | provider | a basic contract-first C++ module on the same cdylib path |
| `rust_orchestrator_module` | Rust | consumer | module context, **sync + async** typed calls, **event subscription**, a **concrete** dependency AND an **interface** dependency |

The orchestrator depends on the two providers two different ways:

- **Concrete dependency** on `rust_calc_module` — its typed client is
  generated from the calc module's published contract and reached through
  `modules().rust_calc_module`. The host auto-loads it.
- **Interface dependency** on a `greeter` contract — the orchestrator codes
  against the *shape*, then binds it to a concrete provider chosen at runtime
  with `GreeterClient::bind("cpp_greeter_module")`. No build-time coupling to
  the C++ module at all.

And it exercises the full consumer surface against them:

```
rust_orchestrator_module
  ├─ whereami()      → module context: modulePath / instanceID / instancePersistencePath
  ├─ tally(a,b)      → modules().rust_calc_module.add(a,b)            (SYNC typed call)
  ├─ tally_async(a,b)→ modules().rust_calc_module.add_async(a,b, cb)  (ASYNC typed call)
  ├─ last_computed() → value captured from rust_calc_module's `computed` EVENT
  └─ hello(name)     → GreeterClient::bind("cpp_greeter_module").greet(name)  (INTERFACE dep)
```

Every cross-module call goes through a **typed, generated** wrapper — no
string-keyed `call_sync`/`call` anywhere in the module code.

Two things make the modules small. First, the contract is the single source
of truth: each module's `.lidl` drives both the Qt-plugin glue and the typed
Rust scaffold. Second — and new here — **there is no `build.rs`**:
`logos-module-builder` runs the Rust generator (`logos-lidl-gen --provider`)
and compiles the crate itself, exactly as it runs the C++ generator. The
author writes only the trait impl and the install hook; the module's
`flake.nix` and `CMakeLists.txt` are as small as a C++ module's.

**What you'll build:** Three composed modules — a Rust provider, a C++ provider, and a Rust consumer — wired through typed generated bindings, showcasing module context, sync/async calls, event subscription, and concrete + interface dependencies, all driven through a logoscore daemon.

**What you'll learn:**

- How to author a Rust Logos module with NO build.rs — the builder generates the C-ABI scaffold from metadata.json's codegen.rust and compiles the crate
- How a Rust module reads its context: module path, instance id, and per-instance persistence path
- Calling another module's typed methods synchronously (modules().<dep>.method) and asynchronously (.method_async(..., callback))
- Subscribing to another module's typed events and receiving decoded payloads
- The difference between a CONCRETE dependency (modules().<dep>, auto-loaded) and an INTERFACE dependency (Client::bind(name), bound at runtime)
- Composing a Rust consumer over both a Rust and a C++ provider — the consumer neither knows nor cares which language a provider is written in

## Prerequisites

- **Nix** with flakes enabled. Install from [nixos.org](https://nixos.org/download.html), then enable flakes:

```bash
mkdir -p ~/.config/nix
echo 'experimental-features = nix-command flakes' >> ~/.config/nix/nix.conf
```

Verify: `nix flake --help >/dev/null 2>&1 && echo "Flakes enabled"`

- **A Linux or macOS machine.** Nix provides every toolchain (Qt, CMake, cargo, rustc) during the builds, so no separate Rust install is needed.

---

## Step 1: Build the tools

Two tools drive the tour: `logoscore` (the headless module runtime) and
`lgpm` (the local package installer). Note what is *not* here: there is no
separate code-generator build — the module builder runs the Rust and C++
generators itself.

> The builder-driven Rust path and the cdylib authoring path currently
> live on the protocol-extraction branches, so `logoscore` is pinned to
> the matching head and the modules below pin `logos-module-builder` /
> `logos-rust-sdk` to theirs. Once the chain merges, drop the pins and
> build plain master.

### 1.1 Build logoscore

```bash
nix build 'github:logos-co/logos-logoscore-cli/11be0e5afad9c96c42048a8780e8b4ca10729e24' --out-link ./logos
```

This brings in the whole module-runtime stack plus the bundled
`capability_module` (required for the auth handshake when loading
modules in daemon mode).

### 1.2 Build lgpm

```bash
nix build 'github:logos-co/logos-package-manager#cli' -o lgpm
```

---

## Step 2: Module 1 — a basic Rust provider (`rust_calc_module`)

A small Rust module exposing two arithmetic methods and emitting a typed
`computed` event after each one. Its logic is pure safe Rust — and there
is **no `build.rs`**: `metadata.json`'s `codegen.rust` tells the builder
to generate the module-impl C ABI scaffold (trait, `RustModuleContext`,
typed `emit_computed` emitter, C exports) from the `.lidl` and compile the
crate to a static archive.

### 2.1 The contract

Create `rust-calc/rust-lib/rust_calc_module.lidl`. The `event`
declaration is what makes the generated `emit_computed` emitter (and,
for consumers, the typed `on_computed` subscription) exist:

```text
module rust_calc_module {
  version "1.0.0"
  description "Basic Rust provider: arithmetic plus a typed computed event"
  method add(a: int, b: int) -> int
  method multiply(a: int, b: int) -> int
  event computed(total: int)
}
```

### 2.2 The crate manifest — note: no build-dependency

Create `rust-calc/rust-lib/Cargo.toml`. The crate depends only on the
SDK (for the runtime the generated scaffold uses) and `serde_json`.
There is **no `logos-lidl-gen` build-dependency** — the builder runs
the generator, not a `build.rs`:

```toml
[package]
name = "rust_calc"
version = "1.0.0"
edition = "2021"

# Standalone crate — the empty table keeps cargo from adopting any
# workspace it finds in a parent directory.
[workspace]

[lib]
crate-type = ["staticlib"]

[dependencies]
serde_json = "1"
logos-rust-sdk = { git = "https://github.com/logos-co/logos-rust-sdk", rev = "441b936f2bcb309ea8f42f017f4612c139a97297" }
```

> The `rev` pins the branch the builder-driven Rust path currently
> lives on; drop it (or pin a release tag) once the chain merges.

### 2.3 The module logic

Create `rust-calc/rust-lib/src/lib.rs`. `include!` pulls in the
builder-generated scaffold from `generated/provider_gen.rs`; you
implement the generated `RustCalcModule` trait and define the install
hook. `emit_computed` is the typed emitter generated from the
contract's `event`:

```rust
//! Basic Rust provider on the builder-driven cdylib path (no build.rs). The
//! module-impl C ABI scaffold + the typed `emit_computed` event emitter are
//! generated by the builder from the .lidl; the author writes the trait impl.

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/generated/provider_gen.rs"));

#[derive(Default)]
struct CalcImpl;

impl RustCalcModule for CalcImpl {
    fn add(&mut self, a: i64, b: i64) -> i64 {
        let total = a + b;
        emit_computed(total); // typed event emitter (generated from the contract)
        total
    }

    fn multiply(&mut self, a: i64, b: i64) -> i64 {
        let total = a.saturating_mul(b);
        emit_computed(total);
        total
    }
}

#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<CalcImpl>();
}
```

The generated `include!` brings `use std::sync::Mutex;` (and a few
others) into scope, so author code that needs those types should refer
to them by full path rather than re-`use`-ing them.

### 2.4 metadata.json — codegen.rust drives the build

`interface: "cdylib"` selects the cdylib path; `codegen.lidl` points
at the contract; and the new `codegen.rust` block names the crate
directory and the static archive it produces (`librust_calc.a`). That
block is the entire signal that makes the builder generate the Rust
scaffold and compile the crate:

```json
{
  "name": "rust_calc_module",
  "version": "1.0.0",
  "description": "Basic Rust provider: arithmetic plus a typed computed event",
  "author": "Logos Core Team",
  "type": "core",
  "interface": "cdylib",
  "category": "general",
  "main": "rust_calc_module_plugin",
  "dependencies": [],
  "include": [],
  "capabilities": [],
  "codegen": {
    "lidl": "rust-lib/rust_calc_module.lidl",
    "rust": { "crate": "rust-lib", "staticlib": "rust_calc" }
  },
  "nix": {
    "external_libraries": [],
    "packages": { "build": [], "runtime": [] },
    "cmake": { "find_packages": [], "extra_include_dirs": [] }
  }
}
```

### 2.5 CMakeLists.txt — trivial, like a C++ module

No `find_library`, no manual link lines: the builder stages the Rust
archive and links it (its `cmake/LogosModule.cmake` does this from
`codegen.rust`). The author writes the same one-liner a C++ module
would:

```cmake
cmake_minimum_required(VERSION 3.14)
project(RustCalcModulePlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LOGOS_MODULE_BUILDER_ROOT is not set.")
endif()

configure_file(${CMAKE_CURRENT_SOURCE_DIR}/metadata.json
               ${CMAKE_CURRENT_BINARY_DIR}/metadata.json COPYONLY)

logos_module(NAME rust_calc_module)
```

### 2.6 flake.nix — trivial, plus the logos-rust-sdk input

The builder reads `logos-lidl-gen` from the module's
`flakeInputs.logos-rust-sdk`, so a Rust module lists one extra input
beyond a C++ module — `logos-rust-sdk`. Everything else is the
standard `mkLogosModule` shape; there is no `buildRustPackage` here:

```nix
{
  description = "Basic Rust provider with a typed event";
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/113b2e1228d059393f12050db9eeaa57a5123536";
    # Provides logos-lidl-gen (the contract->scaffold generator the builder
    # runs) and the SDK the crate links. One extra input vs a C++ module.
    logos-rust-sdk.url = "github:logos-co/logos-rust-sdk/441b936f2bcb309ea8f42f017f4612c139a97297";
  };
  outputs = inputs@{ self, logos-module-builder, ... }:
    let
      nixpkgs = logos-module-builder.inputs.nixpkgs;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems f;
    in {
      packages = forAllSystems (system:
        (logos-module-builder.lib.mkLogosModule {
          src = ./.;
          configFile = ./metadata.json;
          flakeInputs = inputs;
        }).packages.${system});
    };
}
```

### 2.7 Build it

Generate the lockfile (network happens here, not inside the sandbox),
initialise git so the flake sees the files, and build the `.lgx`. The
builder runs `logos-lidl-gen --provider`, compiles the crate, links
the archive into the Qt glue plugin, and bundles it.

```bash
cd rust-calc
(cd rust-lib && nix run nixpkgs#cargo -- generate-lockfile)
git init && git add -A && nix flake update && git add flake.lock
nix build .#lgx -o calc-lgx
```

The builder also **publishes the contract** as `packages.<system>.lidl`
— the orchestrator generates its typed `modules().rust_calc_module`
client from it without ever building this plugin again.

---

## Step 3: Module 2 — a basic C++ provider (`cpp_greeter_module`)

A second provider, this time C++. Same cdylib path, same one-contract
story — the implementation is a hand-written, Qt-free class, and
`codegen.impl_class` tells the builder to generate the C-ABI wrapper
around it. It exists here to be the orchestrator's *interface*
dependency: a different language behind the same kind of typed call.

### 3.1 The contract

```text
module cpp_greeter_module {
  version "1.0.0"
  description "Contract-first C++ cdylib module: a greeter"
  method greet(name: tstr) -> tstr
}
```

### 3.2 The implementation — plain C++, no Qt

```cpp
#pragma once
#include <string>

// Hand-written, Qt-free impl of the cpp_greeter_module.lidl contract — the
// generated C-ABI wrapper + uniform Qt glue are built around this class.
class CppGreeterImpl {
public:
    std::string greet(std::string name) {
        if (name.empty()) name = "World";
        return "Hello, " + name + "! (from C++ greeter)";
    }
};
```

### 3.3 metadata.json — cdylib interface, impl class named

```json
{
  "name": "cpp_greeter_module",
  "version": "1.0.0",
  "description": "Contract-first C++ cdylib module: a greeter",
  "author": "Logos Core Team",
  "type": "core",
  "interface": "cdylib",
  "category": "general",
  "main": "cpp_greeter_module_plugin",
  "dependencies": [],
  "include": [],
  "capabilities": [],
  "codegen": {
    "lidl": "cpp_greeter_module.lidl",
    "impl_class": "CppGreeterImpl",
    "impl_header": "cpp_greeter_module_impl.h"
  },
  "nix": {
    "external_libraries": [],
    "packages": { "build": [], "runtime": [] },
    "cmake": { "find_packages": [], "extra_include_dirs": [] }
  }
}
```

### 3.4 CMakeLists.txt

```cmake
cmake_minimum_required(VERSION 3.14)
project(CppGreeterModulePlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LOGOS_MODULE_BUILDER_ROOT is not set.")
endif()

configure_file(${CMAKE_CURRENT_SOURCE_DIR}/metadata.json
               ${CMAKE_CURRENT_BINARY_DIR}/metadata.json COPYONLY)

logos_module(
    NAME cpp_greeter_module
    INCLUDE_DIRS ${CMAKE_CURRENT_SOURCE_DIR}/src
)
```

### 3.5 flake.nix

A pure C++ module needs only the builder — no `logos-rust-sdk` input:

```nix
{
  description = "Contract-first C++ cdylib module: a greeter";
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/113b2e1228d059393f12050db9eeaa57a5123536";
  };
  outputs = inputs@{ self, logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
```

### 3.6 Build it

```bash
cd cpp-greeter
git init && git add -A && nix flake update && git add flake.lock
nix build .#lgx -o greeter-lgx
```

---

## Step 4: Module 3 — the Rust consumer (`rust_orchestrator_module`)

The star. It declares its own contract, depends on the calc module
concretely and on a `greeter` interface abstractly, and reads its context.
Its implementation is where the whole consumer surface comes together.

### 4.1 The consumer's contract

Create `rust-orchestrator/rust-lib/rust_orchestrator_module.lidl` —
the methods this module exposes:

```text
module rust_orchestrator_module {
  version "1.0.0"
  description "Rust consumer: concrete + interface deps, context, sync/async, events"
  method tally(a: int, b: int) -> int
  method tally_async(a: int, b: int) -> int
  method last_async() -> int
  method hello(name: tstr) -> tstr
  method whereami() -> tstr
  method last_computed() -> int
}
```

### 4.2 The interface contract

Create `rust-orchestrator/rust-lib/greeter.lidl` — a standalone
*interface*: a method shape with no committed implementation. The
orchestrator generates a typed client from it and binds it to a
concrete provider at runtime. The module name here (`greeter`) is the
interface's identity, NOT the provider's:

```text
module greeter {
  version "1.0.0"
  description "Greeter interface — bound to a concrete provider module at runtime"
  method greet(name: tstr) -> tstr
}
```

### 4.3 The crate manifest

```toml
[package]
name = "rust_orchestrator"
version = "1.0.0"
edition = "2021"

[workspace]

[lib]
crate-type = ["staticlib"]

[dependencies]
serde_json = "1"
logos-rust-sdk = { git = "https://github.com/logos-co/logos-rust-sdk", rev = "441b936f2bcb309ea8f42f017f4612c139a97297" }
```

### 4.4 The implementation — the whole consumer surface

Create `rust-orchestrator/rust-lib/src/lib.rs`. Everything typed here
— `modules()`, the `RustCalcModuleClient` (with `add` and the async
`add_async`), the `greeter::GreeterClient` with `bind`, the
`on_computed`/`decode_computed` event API, `RustModuleContext` and
`context()` — is generated by the builder from the contracts above and
the dependencies' contracts. The author writes only the trait impl:

```rust
//! Rust CONSUMER on the builder-driven cdylib path (no build.rs). Shows the
//! whole Rust-SDK surface:
//!   - a CONCRETE dependency (modules().rust_calc_module) — sync AND async calls
//!   - an INTERFACE dependency (the `greeter` contract bound to a concrete
//!     provider at runtime via GreeterClient::bind)
//!   - module CONTEXT (module_path / instance_id / instance_persistence_path)
//!   - typed event SUBSCRIPTION to the calc provider's `computed` event
//! The trait, the typed dependency clients, modules(), and RustModuleContext
//! are all generated by the builder from the contracts.

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/generated/provider_gen.rs"));

// Results that arrive on the event loop (async completion, event delivery)
// land here; the module reads them back through its own methods. The generated
// scaffold already imports Mutex, so refer to it by full path.
static LAST_ASYNC: std::sync::Mutex<i64> = std::sync::Mutex::new(-1);
static LAST_COMPUTED: std::sync::Mutex<i64> = std::sync::Mutex::new(-1);

#[derive(Default)]
struct Orchestrator;

impl RustOrchestratorModule for Orchestrator {
    /// SYNC typed call to the concrete dependency.
    fn tally(&mut self, a: i64, b: i64) -> i64 {
        modules().rust_calc_module.add(a, b).unwrap_or(-1)
    }

    /// ASYNC typed call: fire and return immediately; the typed result lands on
    /// the event loop and is stashed for `last_async()` to read.
    fn tally_async(&mut self, a: i64, b: i64) -> i64 {
        modules().rust_calc_module.add_async(a, b, |res| {
            if let Ok(v) = res {
                *LAST_ASYNC.lock().unwrap() = v;
            }
        });
        0
    }

    fn last_async(&mut self) -> i64 {
        *LAST_ASYNC.lock().unwrap()
    }

    /// INTERFACE dependency: the `greeter` contract bound to a concrete provider
    /// module chosen at runtime — the consumer codes against the shape, not the
    /// module.
    fn hello(&mut self, name: String) -> String {
        greeter::GreeterClient::bind("cpp_greeter_module")
            .greet(&name)
            .unwrap_or_else(|e| format!("greet failed: {}", e))
    }

    /// Module CONTEXT: the three host-stamped identity fields.
    fn whereami(&mut self) -> String {
        match context() {
            Some(c) => format!(
                "module_path={} | instance_id={} | persistence={}",
                c.module_path, c.instance_id, c.instance_persistence_path
            ),
            None => "context not ready".to_string(),
        }
    }

    fn last_computed(&mut self) -> i64 {
        *LAST_COMPUTED.lock().unwrap()
    }

    /// One-time setup once the host stamps the context. Subscribe to the calc
    /// provider's typed `computed` event; the subscription owns its client
    /// share, so move it into a listener thread.
    fn on_context_ready(&mut self, ctx: &RustModuleContext) {
        eprintln!(
            "orchestrator ready: module_path={} instance_id={} persistence={}",
            ctx.module_path, ctx.instance_id, ctx.instance_persistence_path
        );
        let mut calc = modules().rust_calc_module;
        if let Ok(sub) = calc.on_computed() {
            std::thread::spawn(move || {
                for ev in sub {
                    if let Some(e) = rust_calc_module::RustCalcModuleClient::decode_computed(&ev) {
                        *LAST_COMPUTED.lock().unwrap() = e.total;
                    }
                }
            });
        }
    }
}

#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<Orchestrator>();
}
```

`modules().rust_calc_module` is the **concrete** dependency — a client
bound to the calc module's own name. `greeter::GreeterClient::bind(...)`
is the **interface** dependency — the same typed surface pointed at a
provider named at runtime. `add_async`'s callback runs from the
protocol completion path (the module's Qt event loop), so it fires
after the current method returns, never inline — which is why the
result is read back through a later `last_async()` call.

### 4.5 metadata.json — concrete deps, interface deps, codegen.rust

Three things matter here: `dependencies` lists the **concrete**
dependency (auto-loaded, drives `modules().rust_calc_module`);
`interface_dependencies` declares the local `greeter` interface (no
provider named — that happens at runtime via `bind`); and
`codegen.rust` marks this as a builder-built Rust module:

```json
{
  "name": "rust_orchestrator_module",
  "version": "1.0.0",
  "description": "Rust consumer: concrete + interface deps, context, sync/async typed calls, event subscription",
  "author": "Logos Core Team",
  "type": "core",
  "interface": "cdylib",
  "category": "general",
  "main": "rust_orchestrator_module_plugin",
  "dependencies": ["rust_calc_module"],
  "interface_dependencies": [
    { "name": "greeter", "file": "rust-lib/greeter.lidl" }
  ],
  "include": [],
  "capabilities": [],
  "codegen": {
    "lidl": "rust-lib/rust_orchestrator_module.lidl",
    "rust": { "crate": "rust-lib", "staticlib": "rust_orchestrator" }
  },
  "nix": {
    "external_libraries": [],
    "packages": { "build": [], "runtime": [] },
    "cmake": { "find_packages": [], "extra_include_dirs": [] }
  }
}
```

### 4.6 CMakeLists.txt

```cmake
cmake_minimum_required(VERSION 3.14)
project(RustOrchestratorModulePlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LOGOS_MODULE_BUILDER_ROOT is not set.")
endif()

configure_file(${CMAKE_CURRENT_SOURCE_DIR}/metadata.json
               ${CMAKE_CURRENT_BINARY_DIR}/metadata.json COPYONLY)

logos_module(NAME rust_orchestrator_module)
```

### 4.7 flake.nix

Beyond the builder and the SDK, the orchestrator inputs its **concrete
dependency's** flake (`rust_calc_module`) — that is where its published
`.lidl` comes from. The interface dependency (`greeter`) is local, so
it needs no input. The placeholder URL is locked to the real checkout
at build time with `--override-input`:

```nix
{
  description = "Rust consumer: concrete + interface deps, context, sync/async, events";
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/113b2e1228d059393f12050db9eeaa57a5123536";
    logos-rust-sdk.url = "github:logos-co/logos-rust-sdk/441b936f2bcb309ea8f42f017f4612c139a97297";
    # The concrete dependency's flake (its published .lidl drives
    # modules().rust_calc_module). Placeholder — locked to the real checkout
    # at build time via --override-input (nix rejects relative paths here).
    rust_calc_module.url = "path:/path/to/rust-calc";
  };
  outputs = inputs@{ self, logos-module-builder, ... }:
    let
      nixpkgs = logos-module-builder.inputs.nixpkgs;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems f;
    in {
      packages = forAllSystems (system:
        (logos-module-builder.lib.mkLogosModule {
          src = ./.;
          configFile = ./metadata.json;
          flakeInputs = inputs;
        }).packages.${system});
    };
}
```

### 4.8 Build it

Same shape as the calc module, with the concrete dependency locked to
the sibling checkout. The builder generates the orchestrator's
scaffold — including the typed `modules().rust_calc_module` client (from
the calc module's published contract) and the `greeter` client (from
the local interface) — compiles the crate, and links it into the glue:

```bash
cd rust-orchestrator
(cd rust-lib && nix run nixpkgs#cargo -- generate-lockfile)
git init && git add -A
nix flake update --override-input rust_calc_module path:$PWD/../rust-calc
git add flake.lock
nix build .#lgx -o orch-lgx --override-input rust_calc_module path:$PWD/../rust-calc
```

---

## Step 5: Install and run the trio

Install all three packages plus the bundled `capability_module`, start the
daemon, load the modules, and exercise every feature.

### 5.1 Install the modules

```bash
mkdir -p modules
cp -RL ./logos/modules/. ./modules/
./lgpm/bin/lgpm --modules-dir ./modules --allow-unsigned install --file rust-calc/calc-lgx/*.lgx
./lgpm/bin/lgpm --modules-dir ./modules --allow-unsigned install --file cpp-greeter/greeter-lgx/*.lgx
./lgpm/bin/lgpm --modules-dir ./modules --allow-unsigned install --file rust-orchestrator/orch-lgx/*.lgx

```

### 5.2 Start the daemon

```bash
logoscore -D -m ./modules > logs.txt &
```

```bash
sleep 3
```

### 5.3 Load the modules

The orchestrator's **concrete** dependency (`rust_calc_module`) is
auto-loaded with it. The C++ greeter is an **interface** provider —
decoupled from the orchestrator's dependency list — so load it
explicitly first.

```bash
logoscore load-module cpp_greeter_module
```

```bash
logoscore load-module rust_orchestrator_module
```

`rust_calc_module` shows up in `dependencies_loaded` — the host resolved and loaded the concrete dependency automatically.

### 5.4 Module context: who and where am I?

`whereami` returns the three host-stamped context fields from
`context()`: the loaded module's path, its instance id, and its
per-instance persistence directory (a writable, instance-scoped path):

```bash
logoscore call rust_orchestrator_module whereami
```

### 5.5 Sync typed call to the concrete dependency

`tally(5, 3)` is `modules().rust_calc_module.add(5, 3)` — a typed,
synchronous cross-module call. No string-keyed dispatch:

```bash
logoscore call rust_orchestrator_module tally 5 3
```

### 5.6 The typed event arrived

Calling `add` made the calc module `emit_computed(8)`. The orchestrator
subscribed to that event in `on_context_ready` and stored the value;
`last_computed` reads it back:

```bash
logoscore call rust_orchestrator_module last_computed
```

### 5.7 Async typed call

`tally_async(10, 20)` fires `add_async` and returns immediately (`0`);
the typed result is delivered on the event loop after the method
returns:

```bash
logoscore call rust_orchestrator_module tally_async 10 20
```

```bash
sleep 1
```

### 5.8 The async result landed

`last_async` reads the value the async callback stored once the event loop delivered it:

```bash
logoscore call rust_orchestrator_module last_async
```

### 5.9 The async add emitted an event too

The async `add(10, 20)` also emitted `computed(30)`, so the subscription value has advanced:

```bash
logoscore call rust_orchestrator_module last_computed
```

### 5.10 Interface dependency: Rust → C++

`hello("World")` runs `GreeterClient::bind("cpp_greeter_module").greet(...)`
— the `greeter` interface bound to the C++ provider chosen at runtime.
The consumer never built against the C++ module; it only named it in
`bind`:

```bash
logoscore call rust_orchestrator_module hello World
```

### 5.11 Stop the daemon

```bash
logoscore stop
```

```bash
sleep 2
```

```bash
logoscore status
```
