/// Qt-free CBOR socket server for Rust modules.
///
/// Implements the same wire protocol as the C++ CborServer/CborTransportHost:
/// 4-byte big-endian length prefix + CBOR payload over Unix domain sockets.

use crate::cbor;
use crate::dispatch::{CborDispatch, EventBroadcast, EventEmitter};
use crate::value::Value;

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

/// Holds the shared client list and broadcasts events to all connected clients.
struct ClientBroadcaster {
    clients: Arc<Mutex<Vec<Arc<Mutex<UnixStream>>>>>,
}

impl EventBroadcast for ClientBroadcaster {
    fn broadcast_event(&self, name: &str, data: &[Value]) {
        let msg = Value::Map(vec![
            ("type".into(), Value::String("event".into())),
            ("name".into(), Value::String(name.to_string())),
            ("data".into(), Value::Array(data.to_vec())),
        ]);
        let bytes = cbor::encode(&msg);

        let mut clients = self.clients.lock().unwrap();
        clients.retain(|client| {
            let mut stream = client.lock().unwrap();
            write_frame(&mut *stream, &bytes).is_ok()
        });
    }
}

pub struct CborServer {
    socket_path: String,
    dispatch: Arc<dyn CborDispatch>,
    running: Arc<AtomicBool>,
    clients: Arc<Mutex<Vec<Arc<Mutex<UnixStream>>>>>,
}

impl CborServer {
    pub fn new(socket_path: &str, mut dispatch: Box<dyn CborDispatch>) -> Self {
        let clients = Arc::new(Mutex::new(Vec::new()));
        let broadcaster = Arc::new(ClientBroadcaster {
            clients: clients.clone(),
        });
        dispatch.set_event_emitter(EventEmitter::new(broadcaster));

        CborServer {
            socket_path: socket_path.to_string(),
            dispatch: Arc::from(dispatch),
            running: Arc::new(AtomicBool::new(false)),
            clients,
        }
    }

    pub fn module_name(&self) -> &str {
        self.dispatch.module_name()
    }

    /// Push an event to all connected clients.
    pub fn emit_event(&self, name: &str, data: &[Value]) {
        let msg = Value::Map(vec![
            ("type".into(), Value::String("event".into())),
            ("name".into(), Value::String(name.to_string())),
            ("data".into(), Value::Array(data.to_vec())),
        ]);
        let bytes = cbor::encode(&msg);

        let mut clients = self.clients.lock().unwrap();
        clients.retain(|client| {
            let mut stream = client.lock().unwrap();
            write_frame(&mut *stream, &bytes).is_ok()
        });
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
                    let client_stream = match stream.try_clone() {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    let client = Arc::new(Mutex::new(client_stream));
                    self.clients.lock().unwrap().push(client.clone());

                    let dispatch = self.dispatch.clone();
                    let running = self.running.clone();
                    let clients = self.clients.clone();

                    thread::spawn(move || {
                        Self::handle_connection_threaded(stream, dispatch, running, clients);
                    });
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

    fn handle_connection_threaded(
        mut stream: UnixStream,
        dispatch: Arc<dyn CborDispatch>,
        running: Arc<AtomicBool>,
        clients: Arc<Mutex<Vec<Arc<Mutex<UnixStream>>>>>,
    ) {
        // Step 1: Read bind handshake
        let bind_frame = match read_value(&mut stream) {
            Some(v) => v,
            None => return,
        };

        let bind_type = bind_frame
            .get("type")
            .and_then(|v| v.as_string())
            .unwrap_or("");
        let bind_name = bind_frame
            .get("name")
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

        if bind_name != dispatch.module_name() {
            let err = Value::Map(vec![
                ("type".into(), Value::String("bind_err".into())),
                (
                    "error".into(),
                    Value::String(format!("module not found: {}", bind_name)),
                ),
            ]);
            let _ = write_value(&mut stream, &err);
            return;
        }

        // Send bind_ok
        let ok = Value::Map(vec![("type".into(), Value::String("bind_ok".into()))]);
        if write_value(&mut stream, &ok).is_err() {
            return;
        }

        // Step 2: Handle requests
        while running.load(Ordering::SeqCst) {
            let req = match read_value(&mut stream) {
                Some(v) => v,
                None => break,
            };

            let msg_type = req
                .get("type")
                .and_then(|v| v.as_string())
                .unwrap_or("");

            match msg_type {
                "call" => {
                    let id = req.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                    let method = req
                        .get("method")
                        .and_then(|v| v.as_string())
                        .unwrap_or("");
                    let args: Vec<Value> = req
                        .get("args")
                        .and_then(|v| v.as_array())
                        .map(|a| a.to_vec())
                        .unwrap_or_default();

                    let result = dispatch.call_method(method, &args);

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
                    let json = dispatch.methods_json();

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

        // Unregister client — remove all entries whose underlying fd matches
        // We can't easily compare fds, so just remove closed/errored streams
        // by attempting a zero-length peek. Instead, remove by Arc pointer identity.
        {
            let mut client_list = clients.lock().unwrap();
            // We don't have a reference to the Arc we registered, so prune
            // dead connections — the stream we're closing will fail writes.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::CborDispatch;
    use std::sync::atomic::AtomicBool;

    struct MockDispatch;

    impl CborDispatch for MockDispatch {
        fn call_method(&self, _method: &str, _args: &[Value]) -> Value {
            Value::Bool(true)
        }
        fn module_name(&self) -> &str {
            "test_module"
        }
        fn module_version(&self) -> &str {
            "1.0.0"
        }
        fn methods_json(&self) -> &str {
            "[]"
        }
    }

    #[test]
    fn test_emit_event_to_connected_client() {
        // Create a Unix socket pair
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        // Connect a client
        let mut client = UnixStream::connect(&sock_path).unwrap();
        let (server_stream, _) = listener.accept().unwrap();

        // Set up the server's client tracking
        let clients: Arc<Mutex<Vec<Arc<Mutex<UnixStream>>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let client_copy = server_stream.try_clone().unwrap();
        clients
            .lock()
            .unwrap()
            .push(Arc::new(Mutex::new(client_copy)));

        // Build the server struct manually for testing emit_event
        let server = CborServer {
            socket_path: sock_path.to_string_lossy().to_string(),
            dispatch: Arc::new(MockDispatch),
            running: Arc::new(AtomicBool::new(true)),
            clients,
        };

        // Emit an event
        server.emit_event(
            "testEvent",
            &[Value::String("hello".into()), Value::Int(42)],
        );

        // Read the frame from the client side
        let mut header = [0u8; 4];
        client.read_exact(&mut header).unwrap();
        let len = u32::from_be_bytes(header) as usize;
        let mut buf = vec![0u8; len];
        client.read_exact(&mut buf).unwrap();

        let value = cbor::decode(&buf).unwrap();
        assert_eq!(
            value.get("type").and_then(|v| v.as_string()),
            Some("event")
        );
        assert_eq!(
            value.get("name").and_then(|v| v.as_string()),
            Some("testEvent")
        );
        let data = value.get("data").and_then(|v| v.as_array()).unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0].as_string(), Some("hello"));
        assert_eq!(data[1].as_i64(), Some(42));
    }
}
