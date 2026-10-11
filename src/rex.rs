//! The Rex server's two wire protocols, as rmx uses them. Both are websockets
//! on the server's unix socket.
//!
//! Control (`/api/control`, `rex.control.v1`): JSON `request`s answered by
//! `response`s with the same id, plus a stream of `event`s.
//!
//! Data (`/api/data`, `rex.data.terminal.v1`), one per viewed block: send
//! `0x01 + ticket`, get WELCOME (`0x81 ...`, bytes 24..32 hold the token that
//! signs Rex's in-band APCs), then a binary GHOSTSNP snapshot, then raw pty
//! output. Input does not go here; it is the control `write` method.
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::ws::{self, BINARY, TEXT};
use crate::Msg;

type Pending = Arc<Mutex<HashMap<String, Sender<Result<Value, String>>>>>;

pub struct Control {
    writer: ws::Writer,
    pending: Pending,
    next: std::cell::Cell<u64>,
    pub client_id: String,
}

impl Control {
    /// `generation` tags this connection's messages, so a replaced one's
    /// last words are ignored.
    pub fn connect(socket: &Path, generation: u64, tx: Sender<Msg>) -> io::Result<Control> {
        let (writer, mut reader) = ws::connect(socket, "/api/control", "rex.control.v1")?;
        let hello = json!({"type": "hello", "client": {"kind": "cli", "platform": "macos", "name": "rmx", "version": env!("CARGO_PKG_VERSION")}});
        writer.send(TEXT, hello.to_string().as_bytes())?;
        let welcome: Value = serde_json::from_slice(&reader.recv()?.1)?;
        let Some(client_id) = welcome["client"]["id"].as_str().map(str::to_string) else {
            return Err(io::Error::other(format!("rex said {welcome}")));
        };
        let pending: Pending = Arc::default();
        let routes = pending.clone();
        thread::spawn(move || loop {
            let message = match reader.recv() {
                Ok((_, bytes)) => serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
                Err(e) => {
                    let _ = tx.send(Msg::ControlGone(generation, e.to_string()));
                    return;
                }
            };
            match message["type"].as_str() {
                Some("response") => {
                    let id = message["id"].as_str().unwrap_or_default();
                    if let Some(waiter) = routes.lock().unwrap().remove(id) {
                        let result = match message.get("error") {
                            Some(e) => Err(e["message"].as_str().unwrap_or("rex error").to_string()),
                            None => Ok(message["payload"].clone()),
                        };
                        let _ = waiter.send(result);
                    }
                }
                Some("event") => {
                    let _ = tx.send(Msg::Event(generation, message["payload"].clone()));
                }
                _ => {}
            }
        });
        Ok(Control { writer, pending, next: Default::default(), client_id })
    }

    fn request(&self, method: &str, payload: Value, waiter: Option<Sender<Result<Value, String>>>) -> Result<(), String> {
        self.next.set(self.next.get() + 1);
        let id = format!("rmx-{}", self.next.get());
        if let Some(w) = waiter {
            self.pending.lock().unwrap().insert(id.clone(), w);
        }
        let message = json!({"type": "request", "id": id, "method": method, "payload": payload});
        self.writer.send(TEXT, message.to_string().as_bytes()).map_err(|e| e.to_string())
    }

    /// Call and wait for the answer.
    pub fn call(&self, method: &str, payload: Value) -> Result<Value, String> {
        let (tx, rx) = mpsc::channel();
        self.request(method, payload, Some(tx))?;
        rx.recv_timeout(Duration::from_secs(10)).map_err(|_| format!("rex did not answer {method}"))?
    }

    /// Fire and forget, for keystrokes and resizes: their effects come back
    /// through the data stream.
    pub fn post(&self, method: &str, payload: Value) {
        let _ = self.request(method, payload, None);
    }

    /// Hang up; the reader thread ends with the socket.
    pub fn close(&self) {
        self.writer.shutdown();
    }

    pub fn block(&self, session: &str, block: &str, method: &str, args: Value) -> Result<Value, String> {
        self.call("block.method", json!({"session_id": session, "block_id": block, "method": method, "args": args}))
    }

    pub fn post_block(&self, session: &str, block: &str, method: &str, args: Value) {
        self.post("block.method", json!({"session_id": session, "block_id": block, "method": method, "args": args}));
    }
}

/// A live view of one block's output. Dropping it hangs up.
pub struct DataStream {
    writer: ws::Writer,
    /// Signs the `ESC _ rex1;<token>;{json} ESC \` messages Rex puts in the
    /// stream, so a program cannot forge one.
    pub token: u64,
    /// Output messages read off the socket so far.
    pub received: Arc<AtomicU64>,
}

impl DataStream {
    pub fn open(socket: &Path, ticket: &[u8], generation: u64, tx: Sender<Msg>) -> io::Result<DataStream> {
        let (writer, mut reader) = ws::connect(socket, "/api/data", "rex.data.terminal.v1")?;
        let mut hello = vec![0x01];
        hello.extend_from_slice(ticket);
        writer.send(BINARY, &hello)?;
        let (_, welcome) = reader.recv()?;
        if welcome.first() != Some(&0x81) || welcome.len() < 32 {
            return Err(io::Error::other(format!("rex rejected the data connection ({:02x?})", welcome.get(..2))));
        }
        let token = u64::from_le_bytes(welcome[24..32].try_into().unwrap());
        // The snapshot is Ghostty's binary state dump; rmx seeds its screen
        // from the `format` method instead and only keeps the live bytes.
        let received = Arc::new(AtomicU64::new(0));
        let (_, first) = reader.recv()?;
        if !first.starts_with(b"GHOSTSNP") {
            received.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(Msg::Data(generation, first));
        }
        let count = received.clone();
        thread::spawn(move || loop {
            match reader.recv() {
                Ok((_, bytes)) => {
                    count.fetch_add(1, Ordering::SeqCst);
                    if tx.send(Msg::Data(generation, bytes)).is_err() {
                        return;
                    }
                }
                Err(_) => {
                    let _ = tx.send(Msg::DataGone(generation));
                    return;
                }
            }
        });
        Ok(DataStream { writer, token, received })
    }
}

impl Drop for DataStream {
    fn drop(&mut self) {
        self.writer.shutdown();
    }
}

/// The `rex` binary inside the installed Rex app. Where it sits in the bundle
/// has moved between releases, so look for it.
pub fn find_rex() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("REX_BIN") {
        return Some(path.into());
    }
    let roots = [PathBuf::from("/Applications"), crate::home().join("Applications")];
    let apps = ["Rex Beta.app", "Rex.app"];
    roots.iter().flat_map(|r| apps.iter().map(move |a| r.join(a).join("Contents/Helpers"))).find_map(|dir| find_file(&dir, "rex", 4))
}

fn find_file(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    let hit = entries.iter().map(|e| e.path()).find(|p| p.is_file() && p.file_name().is_some_and(|n| n == name));
    hit.or_else(|| entries.iter().map(|e| e.path()).filter(|p| depth > 0 && p.is_dir()).find_map(|p| find_file(&p, name, depth - 1)))
}

/// Start Rex's server the way the Rex app does. A no-op when one is running;
/// the Rex app attaches to it on its own.
pub fn start_server(rex: &Path) -> Result<(), String> {
    let flags = ["--websocket-origin-pattern", "https://rex.superlogical.com", "--log-max-size", "50MiB", "--log-format", "text"];
    run(Command::new(rex).args(["server", "start"]).args(flags))
}

/// Stop the running server. Every open tab ends with it.
pub fn stop_server(rex: &Path) -> Result<(), String> {
    run(Command::new(rex).args(["--autostart=false", "server", "stop"]))
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
}
