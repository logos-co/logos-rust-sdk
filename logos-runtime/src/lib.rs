//! Logos Runtime — Rust CBOR module runtime
//!
//! Provides the building blocks for Rust-based Logos modules:
//! - [`Value`] — neutral type system mirroring `logos::Value` in C++
//! - [`CborDispatch`] — trait for generated dispatch code
//! - [`CborServer`] — Unix/TCP socket server speaking the Logos CBOR wire protocol
//! - [`serve_cbor()`] — convenience entry point for module executables

mod value;
mod cbor;
mod server;
mod dispatch;

pub use value::{Value, LogosResult};
pub use dispatch::{CborDispatch, EventEmitter, EventBroadcast};
pub use server::{CborServer, CborEndpoint};

use std::env;

/// Convenience entry point for CBOR module executables.
///
/// Reads `--socket <path>` or `--endpoint <url>` from argv,
/// or `LOGOS_ENDPOINT` / `LOGOS_SOCKET_PATH` from environment,
/// creates a [`CborServer`], and blocks until stopped.
///
/// Endpoint URLs:
///   - `unix:///path` or raw `/path` — Unix domain socket
///   - `tcp://host:port` — TCP socket
pub fn serve_cbor(dispatch: Box<dyn CborDispatch>) {
    let args: Vec<String> = env::args().collect();
    let mut endpoint = String::new();

    // Parse --socket or --endpoint argument
    let mut i = 1;
    while i < args.len() {
        if (args[i] == "--socket" || args[i] == "--endpoint") && i + 1 < args.len() {
            endpoint = args[i + 1].clone();
            break;
        }
        i += 1;
    }

    // Fall back to environment variables
    if endpoint.is_empty() {
        if let Ok(env_ep) = env::var("LOGOS_ENDPOINT") {
            endpoint = env_ep;
        } else if let Ok(env_path) = env::var("LOGOS_SOCKET_PATH") {
            endpoint = env_path;
        }
    }

    // Default
    if endpoint.is_empty() {
        endpoint = format!("/tmp/logos_{}.sock", dispatch.module_name());
    }

    let server = CborServer::new(&endpoint, dispatch);
    eprintln!("{} listening on {}", server.module_name(), endpoint);

    if let Err(e) = server.run() {
        eprintln!("Server error: {}", e);
        std::process::exit(1);
    }
}
