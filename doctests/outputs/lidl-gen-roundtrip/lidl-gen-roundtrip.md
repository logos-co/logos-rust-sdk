# The Rust Generator: LIDL ⇄ Rust (provider & consumer)

Every module in Logos has a **contract** — the methods other modules may call
and the events they may subscribe to — written in **LIDL** (the Logos Interface
Definition Language). For Rust modules, `logos-lidl-gen` sits between that
contract and the Rust on both sides of a call, and this doc-test exercises the
three flows it supports, with the `logos-lidl-gen` binary built from the commit
under test:

1. **provider header → LIDL** (`--from-rust --trait`) — distil the contract out
   of a plain Rust `trait` the author writes, the Rust analog of authoring the
   contract in code.
2. **LIDL → provider** (`--provider`) — from that contract, emit the provider
   **scaffold** the author implements: a typed `trait`, a `RustModuleContext`,
   typed event emitters (`emit_fault`, …), and the `#[no_mangle]` C-ABI exports
   the host calls.
3. **LIDL → consumer** (default backend) — from the same contract, emit the
   typed **caller** client a *consumer* uses: sync callers, `…_async` callers,
   and event subscribers.

The example is deliberately a **complex interface**: methods of every arity
(zero to four parameters), the full set of round-trippable types (`int`,
`uint`, `float64`, `bool`, `tstr`, byte strings, typed arrays, and the `result`
error type), and multi-line documentation on both methods *and* events. Because
the contract is distilled from the provider trait and then drives both generated
sides, the doc-test shows the `.lidl` is the single source of truth: the
provider trait round-trips into it (Flow 2 reproduces the trait you started
with), and the consumer client comes back out of it. A final section adds the
remaining **composite** types (records, maps, optionals).

**What you'll build:** A rich `sensor_module` provider trait, the `.lidl` contract distilled from it, and both generated sides — the provider scaffold and the typed consumer client — all from this SDK commit.

**What you'll learn:**

- How to extract a `.lidl` contract from a plain Rust trait (`--from-rust --trait`)
- How `///` doc comments (including multi-line), a companion `<Trait>Events` trait, and default-bodied methods map to the contract
- How to generate the provider scaffold (the trait you implement) from a contract (`--provider`)
- How to generate a typed consumer client from the same contract (default backend)
- How every LIDL type maps into Rust — scalars, byte strings (`Vec<u8>`/`&[u8]`), arrays, and `Result`
- How composite types (records, maps, optionals) appear in generated code

## Prerequisites

- **Nix** with flakes enabled. Install from [nixos.org](https://nixos.org/download.html), then enable flakes:

```bash
mkdir -p ~/.config/nix
echo 'experimental-features = nix-command flakes' >> ~/.config/nix/nix.conf
```

Verify: `nix flake --help >/dev/null 2>&1 && echo "Flakes enabled"`

- A Linux or macOS machine.

---

## Step 1: Build the generator

`logos-lidl-gen` ships in the SDK's `lidl-gen` package. Build it so every
flow below runs through this generator. (In the workspace pipeline
`` pins it to the commit under test; on its own it builds the
published generator.)

### 1.1 Build logos-lidl-gen

```bash
nix build 'github:logos-co/logos-rust-sdk#lidl-gen'
```

The binary is now at `./result/bin/logos-lidl-gen`.

---

## Step 2: The provider trait

A Rust module's contract can be authored in code: a plain `trait` — the
**provider header** — where each required (non-defaulted) method is an IPC
method, events live on a companion `<Trait>Events` trait, and `///` doc
comments become descriptions. Default-bodied methods (like the framework's
`on_context_ready`) are hooks, not part of the contract. This one is
deliberately rich:

- **Every arity** — `temperature()` takes no parameters; `enable(on)` takes
  one; `record(id, value, note, valid)` takes four.
- **Every round-trippable type** — `i64`/`u64`, `f64`, `bool`, `String`,
  `Vec<u8>` (bytes), typed `Vec<T>` arrays, and
  `Result<serde_json::Value, String>`.
- **Documentation** — every method and event has a `///` doc comment, and
  several span multiple lines.
- **Events of varied arity** — `ready()` carries no payload, `fault(...)`
  carries three fields.

### 2.1 sensor.rs

```rust
//! The SensorModule contract, authored in Rust.

/// A sensor hub exposing typed readings, batch queries, and status events.
pub trait SensorModule {
    /// Returns the latest temperature reading in degrees Celsius.
    fn temperature(&mut self) -> f64;

    /// Enables or disables the sensor.
    /// Returns the new enabled state.
    fn enable(&mut self, on: bool) -> bool;

    /// Renames the sensor channel.
    fn rename(&mut self, id: u64, name: String) -> String;

    /// Calibrates a channel with an offset and a human-readable label.
    fn calibrate(&mut self, id: u64, offset: f64, label: String) -> bool;

    /// Records a reading and returns the new sample count.
    fn record(&mut self, id: u64, value: f64, note: String, valid: bool) -> i64;

    /// Flashes raw firmware bytes and echoes back the stored image.
    fn firmware(&mut self, image: Vec<u8>) -> Vec<u8>;

    /// Resolves a batch of channel ids to their labels.
    fn labels(&mut self, ids: Vec<u64>) -> Vec<String>;

    /// Computes the mean of a batch of samples.
    fn average(&mut self, samples: Vec<f64>) -> f64;

    /// Resets a channel; returns a structured success/error result.
    fn reset(&mut self, id: String) -> Result<serde_json::Value, String>;

    /// Framework hook — defaulted, so NOT part of the contract.
    fn on_context_ready(&mut self) {}
}

/// Events are declared on a companion `<Trait>Events` trait.
pub trait SensorModuleEvents {
    /// Fires once the sensor has finished warming up.
    fn ready(&self);

    /// Fires on each new reading with the channel id and value.
    fn reading(&self, id: u64, value: f64);

    /// Fires when a channel faults.
    /// Carries an error code, a message, and whether the fault is fatal.
    fn fault(&self, code: i64, message: String, fatal: bool);
}
```

---

## Step 3: Flow 1 — provider header → LIDL

`--from-rust --trait` parses the trait and writes back the `.lidl` contract.
The Rust types map straight to LIDL: `i64`→`int`, `u64`→`uint`,
`f64`→`float64`, `String`→`tstr`, `Vec<u8>`→`bstr`, `Vec<T>`→`[T]`, and
`Result<serde_json::Value, String>`→`result`. The defaulted
`on_context_ready` is excluded.

### 3.1 Extract the .lidl

`--module-name` overrides the default (snake_case of the trait); `fs::write` needs the target directory to exist, so create it first.

```bash
logos-lidl-gen --from-rust sensor.rs \
  --trait SensorModule \
  --module-name sensor_module --module-version 2.0.0 \
  -o extracted/sensor_module.lidl
```

### 3.2 Inspect the extracted contract

Every type came back intact, the four-parameter `record` kept its shape,
and the doc comments — including the multi-line ones, joined with `\n` —
are carried through as descriptions on methods and events alike. This
`.lidl` is now the single source of truth for both generated sides below.

```bash
cat extracted/sensor_module.lidl
```

---

## Step 4: Flow 2 — LIDL → provider

The provider side. From the contract, `--provider` emits the scaffold you
implement: the typed `trait SensorModule` (which reproduces the trait you
authored — the round trip closes), a `RustModuleContext`, the typed `emit_*`
event functions, and the `#[no_mangle]` `logos_module_*` C exports that wire
it to the host. Scalars map to `i64`/`u64`/`f64`/`bool`/`String`, `bstr` to
`Vec<u8>`, and `result` to `Result<serde_json::Value, String>`.

### 4.1 Generate the provider scaffold

```bash
logos-lidl-gen extracted/sensor_module.lidl --provider -o provider_gen.rs
```

### 4.2 Inspect the provider scaffold

```bash
grep -E 'trait|temperature|record|firmware|reset|emit_' provider_gen.rs
```

---

## Step 5: Flow 3 — LIDL → consumer

The consumer side. From the same contract, the default backend emits the
typed client a consumer uses to call `sensor_module`: a sync caller and an
`…_async` caller per method, and an `on_<event>()` subscriber per event.
Borrowed inputs (`&str`, `&[u8]`) keep calls allocation-free.

### 5.1 Generate the consumer client

```bash
logos-lidl-gen extracted/sensor_module.lidl -o client_gen.rs
```

### 5.2 Inspect the consumer client

```bash
grep -E 'SensorModuleClient|temperature|firmware|reset|on_ready|on_fault' client_gen.rs
```

---

## Step 6: The full type system: composite types

Beyond the round-trippable core above, LIDL also has **composite** types:
named record types (`type`), maps (`{K: V}`), and optionals (`?T`), plus the
untyped escape hatch `any`. A declared record becomes a real struct and `?T`
becomes `Option<T>`; a map, or an array of anything but a record, crosses as
untyped JSON and is carried as `serde_json::Value` (a `--from-rust`
extraction recovers the std-friendly subset shown earlier). Here is a
contract that uses all of them, taken straight to a provider scaffold.

### 6.1 geometry_module.lidl

```text
module geometry_module {
  version "1.0.0"
  description "Composite types: records, arrays-of-records, maps, and optionals"

  type Point {
    x: float64
    y: float64
  }

  method translate(p: Point, dx: float64, dy: float64) -> Point description "Translates a point by an offset."
  method bounds(points: [Point]) -> Point description "Returns the bounding corner of a set of points."
  method attributes(tags: {tstr: any}) -> {tstr: any} description "Echoes a string-keyed map of arbitrary values."
  method nearest(p: Point, limit: ?uint) -> ?Point description "Finds the nearest point within an optional limit; may return nothing."
  method describe(p: Point) -> any description "Returns an arbitrary JSON description of a point."

  event moved(from: Point, to: Point) description "Fires when a point moves, carrying both record values."
}
```

### 6.2 Generate the provider scaffold

```bash
logos-lidl-gen geometry_module.lidl --provider -o geometry_gen.rs
```

### 6.3 Inspect the composite signatures

A declared record (`Point`) is a real struct, and an optional (`?uint`,
`?Point`) is an `Option` — Rust's one way to spell "no value", which is
why `?T` has exactly two states and never three. Maps stay
`serde_json::Value`, the untyped carrier for the JSON that crosses the
process boundary, and plain scalars like `dx: f64` stay typed.

On the wire, empty is spelled by the slot: a record FIELD is left out
(a named slot can be), while a parameter or return is `null` (a
positional slot has no key to omit, and the arity must not change).
Decoding is liberal in the other direction — absent and `null` both
read as empty — but a present value of the wrong type is still an
error, exactly as it is for a required slot.

```bash
grep -E 'trait|translate|attributes|nearest|emit_moved' geometry_gen.rs
```
