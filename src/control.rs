//! loom's control socket: the agents' `loom mcp` servers ask the running app to run jobs in
//! the tray, read their output, open files, notify. One JSON object per line each way:
//! `{"agent": id, "method": "...", "params": {...}}` → `{"ok": ...}` or `{"error": "..."}`.
//!
//! The socket lives in the user's runtime directory (only they can reach it).

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use gtk::glib;
use serde_json::{Value, json};

struct Request {
    agent: u32,
    method: String,
    params: Value,
    reply: async_channel::Sender<Value>,
}

thread_local! {
    static PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// The socket of this app id ($XDG_RUNTIME_DIR/loom/<name>.sock).
pub fn socket_path(app_id: &str) -> PathBuf {
    let name = crate::state::suffix(app_id).map_or("loom".to_owned(), |s| format!("loom-{s}"));
    glib::user_runtime_dir().join("loom").join(format!("{name}.sock"))
}

/// The socket the running app listens on (for agents' MCP servers to be told).
pub fn current() -> Option<PathBuf> {
    PATH.with(|p| p.borrow().clone())
}

/// Listens on `path`; `handle` answers each request on the main thread.
pub fn serve(path: PathBuf, handle: impl Fn(u32, &str, &Value) -> Result<Value, String> + 'static) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    // A socket left by a crash; this app is the only instance of its id (GApplication).
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("loom: no control socket at {}: {err}", path.display());
            return;
        }
    };
    PATH.with(|p| *p.borrow_mut() = Some(path));
    let (tx, rx) = async_channel::unbounded::<Request>();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let tx = tx.clone();
                    std::thread::spawn(move || connection(stream, tx));
                }
                // Out of file descriptors, say: wait rather than spin.
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(200)),
            }
        }
    });
    glib::spawn_future_local(async move {
        while let Ok(request) = rx.recv().await {
            let answer = match handle(request.agent, &request.method, &request.params) {
                Ok(value) => json!({ "ok": value }),
                Err(error) => json!({ "error": error }),
            };
            let _ = request.reply.try_send(answer);
        }
    });
}

fn connection(stream: UnixStream, tx: async_channel::Sender<Request>) {
    let Ok(mut writer) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(request) => {
                let (reply, answer) = async_channel::bounded(1);
                let request = Request {
                    agent: request["agent"].as_u64().unwrap_or(0) as u32,
                    method: request["method"].as_str().unwrap_or("").to_owned(),
                    params: request["params"].clone(),
                    reply,
                };
                if tx.send_blocking(request).is_err() {
                    return;
                }
                answer.recv_blocking().unwrap_or_else(|_| json!({ "error": "loom is closing" }))
            }
            Err(err) => json!({ "error": format!("bad request: {err}") }),
        };
        if writeln!(writer, "{answer}").is_err() {
            return;
        }
    }
}

/// One request from outside the app (the MCP server): the answer, or an error message.
pub fn call(socket: &str, agent: u32, method: &str, params: Value) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket).map_err(|e| format!("loom is not reachable ({e}); is it still open?"))?;
    writeln!(stream, "{}", json!({ "agent": agent, "method": method, "params": params })).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(|e| e.to_string())?;
    let answer: Value = serde_json::from_str(&line).map_err(|e| format!("bad answer from loom: {e}"))?;
    match answer.get("error").and_then(Value::as_str) {
        Some(error) => Err(error.to_owned()),
        None => Ok(answer["ok"].clone()),
    }
}
