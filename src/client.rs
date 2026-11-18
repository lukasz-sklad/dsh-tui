//! Minimal JSON-RPC 2.0 client for the DeepSeek Harness SDK runtime.
//!
//! The wire contract is newline-delimited JSON-RPC over stdio, as documented in
//! `packages/sdk/protocol/README.md` of the deepseek-harness repository. The
//! Python SDK (`python/sdk/src/deepseek_harness/client.py`) is the reference
//! implementation this module mirrors.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

/// Result of waiting for the next queued frame with a bounded timeout.
#[derive(Debug, Clone)]
pub enum RecvOutcome<T> {
    Item(T),
    Timeout,
    Closed,
}

/// One server-to-client notification (a frame with `method` and no `id`).
#[derive(Debug, Clone)]
pub struct Notification {
    pub method: String,
    pub params: Value,
}

/// One server-to-client request (a frame with both `method` and `id`).
#[derive(Debug, Clone)]
pub struct IncomingRequest {
    pub id: String,
    /// Wire method (`approval/request`, later `user-question/ask`).
    #[allow(dead_code)]
    pub method: String,
    pub params: Value,
}

/// JSON-RPC error response, preserving the wire code and optional data.
#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    /// Preserved for callers that inspect error payloads (not printed by Display).
    #[allow(dead_code)]
    pub data: Option<Value>,
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

/// Client-side failure: either the runtime answered an error, or the transport broke.
#[derive(Debug)]
pub enum Error {
    Rpc(RpcError),
    Io(std::io::Error),
    Closed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Rpc(error) => write!(f, "JSON-RPC: {error}"),
            Error::Io(error) => write!(f, "transport: {error}"),
            Error::Closed => write!(f, "runtime closed the transport"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Io(error)
    }
}

/// Pending response waiters, keyed by request id.
type Pending = Arc<Mutex<HashMap<String, Sender<Result<Value, RpcError>>>>>;

/// Connected SDK runtime: the `dsh --profile sdk` child plus framed transport.
pub struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    pending: Pending,
    notifications: Receiver<Notification>,
    requests: Receiver<IncomingRequest>,
    /// Stderr lines from the runtime, delivered instead of written to the tty.
    stderr_lines: Receiver<String>,
    next_id: u64,
}

impl Client {
    /// Spawn `program args` (normally `dsh --profile sdk`) and start the stdout reader.
    ///
    /// `extra_env` extends the inherited environment for the child. `log_path`,
    /// when set, receives every wire frame (one JSON per line) as read.
    pub fn launch(
        program: &str,
        args: &[&str],
        extra_env: &[(String, String)],
        log_path: Option<&std::path::Path>,
    ) -> Result<Client, Error> {
        let mut command = Command::new(program);
        command
            .args(args)
            .envs(extra_env.iter().map(|(key, value)| (key.as_str(), value.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The child gets its own stderr pipe instead of inheriting ours.
            // Inheriting would share our terminal's open file description, and
            // crossterm's O_NONBLOCK on stdin would leak to the child's stderr,
            // which can wedge the runtime's event loop mid-turn.
            .stderr(Stdio::piped());
        let log_file = match log_path {
            Some(path) => Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|error| Error::Io(error))?,
            ),
            None => None,
        };
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().ok_or(Error::Closed)?;
        let stdin = child.stdin.take().ok_or(Error::Closed)?;
        let (stderr_tx, stderr_rx) = mpsc::channel();
        if let Some(stderr) = child.stderr.take() {
            thread::spawn(move || forward_stderr(stderr, stderr_tx));
        }
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (notify_tx, notify_rx) = mpsc::channel();
        let (request_tx, request_rx) = mpsc::channel();
        let reader_pending = Arc::clone(&pending);
        thread::spawn(move || read_frames(stdout, reader_pending, notify_tx, request_tx, log_file));
        Ok(Client {
            child,
            stdin: Some(stdin),
            pending,
            notifications: notify_rx,
            requests: request_rx,
            stderr_lines: stderr_rx,
            next_id: 1,
        })
    }

    /// Send one request and wait for its response.
    pub fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, Error> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let mut frame = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(params) = params {
            frame["params"] = params;
        }
        self.write_frame(&frame)?;
        rx.recv().map_err(|_| Error::Closed)?.map_err(Error::Rpc)
    }

    /// Answer a server-to-client request with a result.
    pub fn respond(&mut self, id: &str, result: Value) -> Result<(), Error> {
        self.write_frame(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    /// Answer a server-to-client request with a JSON-RPC error.
    #[allow(dead_code)]
    pub fn respond_error(&mut self, id: &str, code: i64, message: &str) -> Result<(), Error> {
        self.write_frame(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        }))
    }

    /// Non-blocking read of the next queued notification, if any.
    pub fn try_recv_notification(&self) -> Option<Notification> {
        self.notifications.try_recv().ok()
    }

    /// Blocking read of the next notification; `None` when the runtime closed.
    #[allow(dead_code)]
    pub fn recv_notification(&self) -> Option<Notification> {
        self.notifications.recv().ok()
    }

    /// Read the next notification, waiting at most `timeout`; `None` on timeout
    /// or when the runtime closed.
    #[allow(dead_code)]
    pub fn recv_notification_timeout(&self, timeout: std::time::Duration) -> Option<Notification> {
        self.notifications.recv_timeout(timeout).ok()
    }

    /// Wait up to `timeout` for the next notification, distinguishing timeout from EOF.
    pub fn recv_notification_step(&self, timeout: std::time::Duration) -> RecvOutcome<Notification> {
        match self.notifications.recv_timeout(timeout) {
            Ok(notification) => RecvOutcome::Item(notification),
            Err(mpsc::RecvTimeoutError::Timeout) => RecvOutcome::Timeout,
            Err(mpsc::RecvTimeoutError::Disconnected) => RecvOutcome::Closed,
        }
    }

    /// Non-blocking read of the next runtime stderr line, if any.
    pub fn try_recv_stderr(&self) -> Option<String> {
        self.stderr_lines.try_recv().ok()
    }

    /// Non-blocking read of the next server-to-client request, if any.
    pub fn try_recv_request(&self) -> Option<IncomingRequest> {
        self.requests.try_recv().ok()
    }

    /// Ask the runtime to shut down, close stdin, and reap the child.
    pub fn shutdown(&mut self) {
        let _ = self.request("shutdown", None);
        // stdin EOF is the launcher's bounded-shutdown signal; dropping the
        // handle closes it so the runtime can finish flushing durable state.
        self.stdin.take();
        let _ = self.child.wait();
    }

    /// Write one framed JSON-RPC message to the runtime.
    fn write_frame(&mut self, frame: &Value) -> Result<(), Error> {
        let stdin = self.stdin.as_mut().ok_or(Error::Closed)?;
        stdin.write_all(frame.to_string().as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }
}

/// Forward the runtime's stderr pipe into the line channel.
fn forward_stderr(stderr: impl std::io::Read, sender: Sender<String>) {
    let reader = BufReader::new(stderr);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let _ = sender.send(line);
    }
}

/// Read newline-delimited JSON frames from the runtime and route them.
fn read_frames(
    stdout: impl std::io::Read,
    pending: Pending,
    notify_tx: Sender<Notification>,
    request_tx: Sender<IncomingRequest>,
    log_file: Option<std::fs::File>,
) {
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(mut file) = log_file.as_ref() {
            use std::io::Write;
            let _ = writeln!(&mut file, "{line}");
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = value.get("id").and_then(Value::as_str).map(str::to_string);
        let method = value.get("method").and_then(Value::as_str).map(str::to_string);
        match (id, method) {
            (Some(id), Some(method)) => {
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                let _ = request_tx.send(IncomingRequest { id, method, params });
            }
            (Some(id), None) => {
                let result = if let Some(error) = value.get("error") {
                    Err(RpcError {
                        code: error.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                        message: error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("JSON-RPC error")
                            .to_string(),
                        data: error.get("data").cloned(),
                    })
                } else {
                    Ok(value.get("result").cloned().unwrap_or(Value::Null))
                };
                if let Some(tx) = pending.lock().unwrap().remove(&id) {
                    let _ = tx.send(result);
                }
            }
            (None, Some(method)) => {
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                let _ = notify_tx.send(Notification { method, params });
            }
            (None, None) => {}
        }
    }
}
