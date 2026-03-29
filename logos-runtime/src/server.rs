/// Qt-free CBOR socket server for Rust modules.
///
/// Implements the same wire protocol as the C++ CborServer/CborTransportHost:
/// 4-byte big-endian length prefix + CBOR payload over Unix domain sockets or TCP.
///
/// Endpoint URLs:
///   "unix:///path" or "/path"  — Unix domain socket
///   "tcp://host:port"          — TCP socket

use crate::cbor;
use crate::dispatch::{CborDispatch, EventBroadcast, EventEmitter};
use crate::value::Value;

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

// ── Endpoint parsing ────────────────────────────────────────────────────────

/// Parsed endpoint URL for the CBOR server.
#[derive(Debug, Clone)]
pub enum CborEndpoint {
    Unix(String),
    Tcp { host: String, port: u16 },
}

impl CborEndpoint {
    /// Parse an endpoint URL string.
    ///
    /// - "tcp://host:port" → Tcp
    /// - "unix:///path" → Unix
    /// - "/path" (raw) → Unix
    pub fn parse(url: &str) -> Self {
        if let Some(rest) = url.strip_prefix("tcp://") {
            if let Some(colon) = rest.rfind(':') {
                let host = &rest[..colon];
                if let Ok(port) = rest[colon + 1..].parse::<u16>() {
                    if port > 0 && !host.is_empty() {
                        return CborEndpoint::Tcp {
                            host: host.to_string(),
                            port,
                        };
                    }
                }
            }
            // Invalid TCP URL, fall back to Unix with empty path
            return CborEndpoint::Unix(String::new());
        }

        if let Some(path) = url.strip_prefix("unix://") {
            return CborEndpoint::Unix(path.to_string());
        }

        // Raw path
        CborEndpoint::Unix(url.to_string())
    }

    /// Human-readable representation.
    pub fn to_string(&self) -> String {
        match self {
            CborEndpoint::Unix(path) => format!("unix://{}", path),
            CborEndpoint::Tcp { host, port } => format!("tcp://{}:{}", host, port),
        }
    }
}

// ── Type-erased stream writer for client tracking ───────────────────────────

/// A type-erased write handle for broadcasting events.
type ClientWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Holds the shared client list and broadcasts events to all connected clients.
struct ClientBroadcaster {
    clients: Arc<Mutex<Vec<ClientWriter>>>,
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
            write_frame_dyn(&mut **stream, &bytes).is_ok()
        });
    }
}

// ── CborServer ──────────────────────────────────────────────────────────────

pub struct CborServer {
    endpoint: CborEndpoint,
    dispatch: Arc<dyn CborDispatch>,
    running: Arc<AtomicBool>,
    clients: Arc<Mutex<Vec<ClientWriter>>>,
}

impl CborServer {
    /// Create a new server from a socket path or endpoint URL.
    pub fn new(endpoint_url: &str, mut dispatch: Box<dyn CborDispatch>) -> Self {
        let clients = Arc::new(Mutex::new(Vec::new()));
        let broadcaster = Arc::new(ClientBroadcaster {
            clients: clients.clone(),
        });
        dispatch.set_event_emitter(EventEmitter::new(broadcaster));

        CborServer {
            endpoint: CborEndpoint::parse(endpoint_url),
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
            write_frame_dyn(&mut **stream, &bytes).is_ok()
        });
    }

    /// Run the server, blocking until an error occurs or stopped.
    pub fn run(&self) -> io::Result<()> {
        self.running.store(true, Ordering::SeqCst);

        match &self.endpoint {
            CborEndpoint::Unix(path) => self.run_unix(path),
            CborEndpoint::Tcp { host, port } => self.run_tcp(host, *port),
        }
    }

    fn run_unix(&self, path: &str) -> io::Result<()> {
        // Remove stale socket
        let _ = std::fs::remove_file(path);

        let listener = UnixListener::bind(path)?;

        for stream in listener.incoming() {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            match stream {
                Ok(stream) => {
                    self.accept_unix_stream(stream);
                }
                Err(e) => {
                    if !self.running.load(Ordering::SeqCst) {
                        break;
                    }
                    eprintln!("Accept error: {}", e);
                }
            }
        }

        let _ = std::fs::remove_file(path);
        Ok(())
    }

    fn run_tcp(&self, host: &str, port: u16) -> io::Result<()> {
        let addr = format!("{}:{}", host, port);
        let listener = TcpListener::bind(&addr)?;

        // Disable Nagle on accepted connections for lower latency
        for stream in listener.incoming() {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            match stream {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    self.accept_tcp_stream(stream);
                }
                Err(e) => {
                    if !self.running.load(Ordering::SeqCst) {
                        break;
                    }
                    eprintln!("Accept error: {}", e);
                }
            }
        }

        Ok(())
    }

    fn accept_unix_stream(&self, stream: UnixStream) {
        let client_stream: Box<dyn Write + Send> = match stream.try_clone() {
            Ok(s) => Box::new(s),
            Err(_) => return,
        };
        let client: ClientWriter = Arc::new(Mutex::new(client_stream));
        self.clients.lock().unwrap().push(client);

        let dispatch = self.dispatch.clone();
        let running = self.running.clone();
        thread::spawn(move || {
            Self::handle_connection(stream, dispatch, running);
        });
    }

    fn accept_tcp_stream(&self, stream: TcpStream) {
        let client_stream: Box<dyn Write + Send> = match stream.try_clone() {
            Ok(s) => Box::new(s),
            Err(_) => return,
        };
        let client: ClientWriter = Arc::new(Mutex::new(client_stream));
        self.clients.lock().unwrap().push(client);

        let dispatch = self.dispatch.clone();
        let running = self.running.clone();
        thread::spawn(move || {
            Self::handle_connection(stream, dispatch, running);
        });
    }

    /// Stop the server.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Handle a single connection (works with both Unix and TCP streams).
    fn handle_connection<S: Read + Write>(
        mut stream: S,
        dispatch: Arc<dyn CborDispatch>,
        running: Arc<AtomicBool>,
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

        // Stream is closed when dropped
        drop(stream);
    }
}

// ── Frame I/O (generic over Read/Write) ─────────────────────────────────────

fn write_frame_dyn(stream: &mut dyn Write, data: &[u8]) -> io::Result<()> {
    let len = data.len() as u32;
    let header = len.to_be_bytes();
    stream.write_all(&header)?;
    stream.write_all(data)?;
    stream.flush()
}

fn read_frame(stream: &mut dyn Read) -> io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    let mut buf = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut buf)?;
    }
    Ok(buf)
}

fn write_value(stream: &mut dyn Write, value: &Value) -> io::Result<()> {
    let bytes = cbor::encode(value);
    write_frame_dyn(stream, &bytes)
}

fn read_value(stream: &mut dyn Read) -> Option<Value> {
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
        let clients: Arc<Mutex<Vec<ClientWriter>>> = Arc::new(Mutex::new(Vec::new()));
        let client_copy = server_stream.try_clone().unwrap();
        let writer: Box<dyn Write + Send> = Box::new(client_copy);
        clients
            .lock()
            .unwrap()
            .push(Arc::new(Mutex::new(writer)));

        // Build the server struct manually for testing emit_event
        let server = CborServer {
            endpoint: CborEndpoint::Unix(sock_path.to_string_lossy().to_string()),
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

    #[test]
    fn test_emit_event_over_tcp() {
        // Create a TCP listener on an ephemeral port
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        // Connect a client
        let mut client = TcpStream::connect(addr).unwrap();
        let (server_stream, _) = listener.accept().unwrap();

        // Set up the server's client tracking
        let clients: Arc<Mutex<Vec<ClientWriter>>> = Arc::new(Mutex::new(Vec::new()));
        let client_copy = server_stream.try_clone().unwrap();
        let writer: Box<dyn Write + Send> = Box::new(client_copy);
        clients
            .lock()
            .unwrap()
            .push(Arc::new(Mutex::new(writer)));

        let server = CborServer {
            endpoint: CborEndpoint::Tcp {
                host: "127.0.0.1".into(),
                port: addr.port(),
            },
            dispatch: Arc::new(MockDispatch),
            running: Arc::new(AtomicBool::new(true)),
            clients,
        };

        // Emit an event
        server.emit_event("tcpEvent", &[Value::String("tcp_hello".into())]);

        // Read from client
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
            Some("tcpEvent")
        );
    }

    #[test]
    fn test_endpoint_parse_unix() {
        match CborEndpoint::parse("unix:///tmp/test.sock") {
            CborEndpoint::Unix(path) => assert_eq!(path, "/tmp/test.sock"),
            _ => panic!("expected Unix"),
        }
    }

    #[test]
    fn test_endpoint_parse_raw_path() {
        match CborEndpoint::parse("/tmp/test.sock") {
            CborEndpoint::Unix(path) => assert_eq!(path, "/tmp/test.sock"),
            _ => panic!("expected Unix"),
        }
    }

    #[test]
    fn test_endpoint_parse_tcp() {
        match CborEndpoint::parse("tcp://localhost:9100") {
            CborEndpoint::Tcp { host, port } => {
                assert_eq!(host, "localhost");
                assert_eq!(port, 9100);
            }
            _ => panic!("expected Tcp"),
        }
    }

    #[test]
    fn test_endpoint_parse_tcp_with_ip() {
        match CborEndpoint::parse("tcp://192.168.1.100:8080") {
            CborEndpoint::Tcp { host, port } => {
                assert_eq!(host, "192.168.1.100");
                assert_eq!(port, 8080);
            }
            _ => panic!("expected Tcp"),
        }
    }
}
