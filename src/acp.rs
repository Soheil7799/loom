//! The agent process and the JSON-RPC it speaks over stdio: ACP, one JSON object per line.
//!
//! The adapter runs in its own process group, so stopping it also stops the Claude Code
//! process it starts (otherwise that one would linger and keep its memory).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use serde_json::{Value, json};

/// What the agent sends on its own: a notification, or a request (when `id` is set) that
/// must get an answer.
pub enum Incoming {
    Call { id: Option<Value>, method: String, params: Value },
    /// The process ended (or closed its output).
    Exited,
}

type Reply = Box<dyn FnOnce(Result<Value, Value>)>;

pub struct Conn {
    stdin: RefCell<Option<ChildStdin>>,
    child: RefCell<Option<Child>>,
    next_id: Cell<u64>,
    pending: RefCell<HashMap<u64, Reply>>,
}

/// The adapter command: `LOOM_AGENT`, else the settings' (both split on spaces), else the
/// default.
pub fn adapter_command() -> Vec<String> {
    let chosen = std::env::var("LOOM_AGENT").ok().filter(|c| !c.trim().is_empty()).unwrap_or_else(|| crate::settings::get().adapter);
    let parts: Vec<String> = chosen.split_whitespace().map(str::to_owned).collect();
    if parts.is_empty() { default_adapter() } else { parts }
}

/// The copy loom installs, else claude-agent-acp on PATH.
pub fn default_adapter() -> Vec<String> {
    let installed = installed_dir().join("node_modules/.bin/claude-agent-acp");
    if installed.exists() {
        return vec![installed.to_string_lossy().into_owned()];
    }
    vec!["claude-agent-acp".to_owned()]
}

fn installed_dir() -> PathBuf {
    glib::user_data_dir().join("loom/adapter")
}

/// The installed adapter's version (from its package.json).
pub fn adapter_version() -> Option<String> {
    let manifest = installed_dir().join("node_modules/@agentclientprotocol/claude-agent-acp/package.json");
    let text = std::fs::read_to_string(manifest).ok()?;
    serde_json::from_str::<Value>(&text).ok()?["version"].as_str().map(str::to_owned)
}

/// Where the adapters' stderr goes.
pub fn log_path() -> PathBuf {
    crate::state::dir().join("agents.log")
}

impl Conn {
    pub fn spawn(cwd: &Path, on_incoming: impl Fn(Incoming) + 'static) -> std::io::Result<Rc<Conn>> {
        let program = adapter_command();
        let log = log_path();
        if let Some(dir) = log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let stderr = std::fs::OpenOptions::new().create(true).append(true).open(&log)?;
        let mut command = Command::new(&program[0]);
        command
            .args(&program[1..])
            .current_dir(cwd)
            // Started from inside a Claude Code session (tests), the SDK would refuse to nest.
            .env_remove("CLAUDECODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .process_group(0);
        // SAFETY: prctl only, between fork and exec.
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = async_channel::unbounded::<Option<Value>>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).split(b'\n') {
                let Ok(line) = line else { break };
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                match serde_json::from_slice(&line) {
                    Ok(value) => {
                        if tx.send_blocking(Some(value)).is_err() {
                            return;
                        }
                    }
                    Err(err) => eprintln!("loom: unreadable line from the agent ({err})"),
                }
            }
            let _ = tx.send_blocking(None);
        });

        let conn = Rc::new(Conn {
            stdin: RefCell::new(child.stdin.take()),
            child: RefCell::new(Some(child)),
            next_id: Cell::new(0),
            pending: RefCell::new(HashMap::new()),
        });
        let weak = Rc::downgrade(&conn);
        glib::spawn_future_local(async move {
            while let Ok(message) = rx.recv().await {
                let Some(conn) = weak.upgrade() else { break };
                match message {
                    Some(value) => conn.dispatch(value, &on_incoming),
                    None => {
                        on_incoming(Incoming::Exited);
                        conn.fail_pending();
                        break;
                    }
                }
            }
        });
        Ok(conn)
    }

    fn dispatch(&self, mut value: Value, on_incoming: &dyn Fn(Incoming)) {
        if let Some(method) = value.get("method").and_then(Value::as_str).map(str::to_owned) {
            let id = value.get("id").cloned();
            let params = value.get_mut("params").map(Value::take).unwrap_or(Value::Null);
            on_incoming(Incoming::Call { id, method, params });
            return;
        }
        let Some(id) = value.get("id").and_then(Value::as_u64) else { return };
        let reply = self.pending.borrow_mut().remove(&id);
        if let Some(reply) = reply {
            match value.get_mut("error") {
                Some(error) => reply(Err(error.take())),
                None => reply(Ok(value.get_mut("result").map(Value::take).unwrap_or(Value::Null))),
            }
        }
    }

    /// Answers every request still waiting, as failed: the agent is gone.
    fn fail_pending(&self) {
        let pending: Vec<Reply> = self.pending.borrow_mut().drain().map(|(_, reply)| reply).collect();
        for reply in pending {
            reply(Err(json!({ "message": "the agent stopped" })));
        }
    }

    pub fn request(&self, method: &str, params: Value, reply: impl FnOnce(Result<Value, Value>) + 'static) {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        self.pending.borrow_mut().insert(id, Box::new(reply));
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    pub fn respond(&self, id: Value, result: Value) {
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    pub fn respond_error(&self, id: Value, code: i64, message: &str) {
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }));
    }

    fn write(&self, value: &Value) {
        if let Some(stdin) = self.stdin.borrow_mut().as_mut() {
            let mut line = value.to_string();
            line.push('\n');
            if stdin.write_all(line.as_bytes()).and_then(|()| stdin.flush()).is_err() {
                eprintln!("loom: the agent is not reading");
            }
        }
    }

    /// Stops the agent and everything it started: SIGTERM to the group, SIGKILL after 3 s.
    pub fn stop(&self) {
        self.stdin.borrow_mut().take();
        let Some(mut child) = self.child.borrow_mut().take() else { return };
        let group = child.id() as i32;
        // SAFETY: plain kill(2) on the group this process leads.
        unsafe { libc::kill(-group, libc::SIGTERM) };
        std::thread::spawn(move || {
            for _ in 0..30 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            // Whatever is left of the group (the leader may go first and leave Claude Code).
            // SAFETY: as above.
            unsafe { libc::kill(-group, libc::SIGKILL) };
            let _ = child.wait();
        });
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.stop();
    }
}
