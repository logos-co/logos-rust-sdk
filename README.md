# Logos Rust SDK (Experimental)

A Rust SDK for interacting with Logos Core modules. This SDK provides a high-level API for initializing the core, loading plugins/modules, calling methods, and subscribing to events.

## Overview

The Logos Rust SDK wraps the `liblogos_core` C API, similar to how the [logos-js-sdk](https://github.com/logos-co/logos-js-sdk) and [logos-nim-sdk](https://github.com/logos-co/logos-core-poc/tree/main/logos-nim-sdk) work.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
logos-rust-sdk = { path = "../logos-rust-sdk" }
```

Or if using the workspace:

```toml
[dependencies]
logos-rust-sdk = { path = "../logos-rust-sdk" }
```

## Quick Start

```rust
use logos_rust_sdk::{LogosAPI, LogosError};

fn main() -> Result<(), LogosError> {
    // Initialize the SDK
    let mut logos = LogosAPI::new()?;
    logos.set_plugins_dir("/path/to/modules")?;
    logos.start()?;

    // Load plugins
    logos.load_plugins(&["capability_module", "waku_module", "chat"])?;

    // Get a plugin proxy
    let mut chat = logos.plugin("chat");

    // Call methods
    chat.call("initialize", &[])?;
    chat.call("joinChannel", &["baixa-chiado"])?;

    // Subscribe to events
    let messages_rx = chat.on("chatMessage")?;

    // Main event loop
    loop {
        // Process Qt events (required for callbacks to work)
        logos.process_events();

        // Check for incoming messages
        while let Ok(event) = messages_rx.try_recv() {
            // event.data is a JSON value with the event payload
            // For chatMessage: [timestamp, sender, message]
            if let Some(sender) = event.get_str(1) {
                if let Some(message) = event.get_str(2) {
                    println!("{}: {}", sender, message);
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
```

## API Reference

### LogosAPI

The main entry point for interacting with Logos Core.

```rust
// Create a new instance (initializes Qt and the core)
let mut logos = LogosAPI::new()?;

// Set the plugins directory (must be called before start)
logos.set_plugins_dir("/path/to/modules")?;

// Start the core (scans plugins, starts registry)
logos.start()?;

// Load a plugin
logos.load_plugin("chat")?;

// Load multiple plugins
logos.load_plugins(&["waku_module", "chat"])?;

// Get loaded plugins
let plugins = logos.get_loaded_plugins();

// Process Qt events (call periodically in your main loop)
logos.process_events();

// Get a plugin proxy
let chat = logos.plugin("chat");
```

### PluginProxy

A proxy for interacting with a specific plugin.

```rust
let mut chat = logos.plugin("chat");

// Call a method with string parameters
let rx = chat.call("joinChannel", &["my-channel"])?;

// Call a method with no parameters
let rx = chat.call("initialize", &[] as &[&str])?;

// Subscribe to events
let messages = chat.on("chatMessage")?;

// Check for results (non-blocking)
if let Ok(result) = rx.try_recv() {
    if result.success {
        println!("Method succeeded: {}", result.message);
    } else {
        println!("Method failed: {}", result.message);
    }
}

// Check for events (non-blocking)
while let Ok(event) = messages.try_recv() {
    println!("Event: {} - {:?}", event.event, event.data);
}
```

## Building

### With Nix

```bash
nix build .#logos-rust-sdk
```

### With Cargo

Ensure you have the required environment variables set:

```bash
export LOGOS_LIBLOGOS_ROOT=/path/to/logos-liblogos/build
export QT_FRAMEWORK_PATH=/opt/homebrew/opt/qt@6/lib  # macOS
cargo build -p logos-rust-sdk
```
