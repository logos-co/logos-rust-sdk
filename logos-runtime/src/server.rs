/// Qt-free CBOR socket server for Rust modules.
///
/// Implements the same wire protocol as the C++ CborServer/CborTransportHost:
/// 4-byte big-endian length prefix + CBOR payload over Unix domain sockets.

use crate::cbor;
use crate::dispatch::CborDispatch;
use crate::value::Value;

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

pub struct CborServer {
    socket_path: String,
    dispatch: Box<dyn CborDispatch>,
    running: Arc<AtomicBool>,
}

impl CborServer {
    pub fn new(socket_path: &str, dispatch: Box<dyn CborDispatch>) -> Self {
        CborServer {
            socket_path: socket_path.to_string(),
            dispatch,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn module_name(&self) -> &str {
        self.dispatch.module_name()
    }

    /// Run the server, blocking until an error occurs or the socket is closed.
    pub fn run(&self) -> io::Result<()> {
        // Remove stale socket
        let _ = std::fs::remove_file(&self.socket_path);

        let listener = UnixListener::bind(&self.socket_path)?;
        self.running.store(true, Ordering::SeqCst);

        for stream in listener.incoming() {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            match stream {
                Ok(stream) => {
                    self.handle_connection(stream);
                }
                Err(e) => {
                    if !self.running.load(Ordering::SeqCst) {
                        break;
                    }
                    eprintln!("Accept error: {}", e);
                }
            }
        }

        let _ = std::fs::remove_file(&self.socket_path);
        Ok(())
    }

    /// Stop the server.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn handle_connection(&self, mut stream: UnixStream) {
        // Step 1: Read bind handshake
        let bind_frame = match read_value(&mut stream) {
            Some(v) => v,
            None => return,
        };

        let bind_type = bind_frame.get("type")
            .and_then(|v| v.as_string())
            .unwrap_or("");
        let bind_name = bind_frame.get("name")
            .and_then(|v| v.as_string())
            .unwrap_or("");

        if bind_type != "bind" || bind_name.is_empty() {
            let err = Value::Map(vec![
                ("type".into(), Value::String("bind_err".into())),
                ("error".into(), Value::String("invalid handshake".into())),
            ]);
            let _ = write_value(&mut stream, &err);
            return;
        }

        if bind_name != self.dispatch.module_name() {
            let err = Value::Map(vec![
                ("type".into(), Value::String("bind_err".into())),
                ("error".into(), Value::String(format!("module not found: {}", bind_name))),
            ]);
            let _ = write_value(&mut stream, &err);
            return;
        }

        // Send bind_ok
        let ok = Value::Map(vec![
            ("type".into(), Value::String("bind_ok".into())),
        ]);
        if write_value(&mut stream, &ok).is_err() {
            return;
        }

        // Step 2: Handle requests
        while self.running.load(Ordering::SeqCst) {
            let req = match read_value(&mut stream) {
                Some(v) => v,
                None => break,
            };

            let msg_type = req.get("type")
                .and_then(|v| v.as_string())
                .unwrap_or("");

            match msg_type {
                "call" => {
                    let id = req.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                    let method = req.get("method")
                        .and_then(|v| v.as_string())
                        .unwrap_or("");
                    let args: Vec<Value> = req.get("args")
                        .and_then(|v| v.as_array())
                        .map(|a| a.to_vec())
                        .unwrap_or_default();

                    let result = self.dispatch.call_method(method, &args);

                    let resp = Value::Map(vec![
                        ("id".into(), Value::Uint(id)),
                        ("type".into(), Value::String("result".into())),
                        ("value".into(), result),
                    ]);
                    if write_value(&mut stream, &resp).is_err() {
                        break;
                    }
                }
                "methods" => {
                    let id = req.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                    let json = self.dispatch.methods_json();

                    let resp = Value::Map(vec![
                        ("id".into(), Value::Uint(id)),
                        ("type".into(), Value::String("result".into())),
                        ("value".into(), Value::String(json.to_string())),
                    ]);
                    if write_value(&mut stream, &resp).is_err() {
                        break;
                    }
                }
                _ => {}
            }
        }

        let _ = stream.shutdown(Shutdown::Both);
    }
}

// ── Frame I/O ────────────────────────────────────────────────────────────────

fn write_frame(stream: &mut UnixStream, data: &[u8]) -> io::Result<()> {
    let len = data.len() as u32;
    let header = len.to_be_bytes();
    stream.write_all(&header)?;
    stream.write_all(data)?;
    stream.flush()
}

fn read_frame(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    let mut buf = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut buf)?;
    }
    Ok(buf)
}

fn write_value(stream: &mut UnixStream, value: &Value) -> io::Result<()> {
    let bytes = cbor::encode(value);
    write_frame(stream, &bytes)
}

fn read_value(stream: &mut UnixStream) -> Option<Value> {
    let bytes = read_frame(stream).ok()?;
    cbor::decode(&bytes).ok()
}
