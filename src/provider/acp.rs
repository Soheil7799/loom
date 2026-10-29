//! ACP through an adapter process (JSON-RPC over stdio): any ACP agent, e.g. claude-agent-acp.
//! Used when an adapter is set (Settings → Advanced, or LOOM_AGENT).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::glib;
use serde_json::{Value, json};

use super::process::Process;
use super::{Incoming, Reply};

pub struct Link {
    process: Rc<Process>,
    next_id: Cell<u64>,
    pending: RefCell<HashMap<u64, Reply>>,
}

/// The adapter command set: `LOOM_AGENT`, else the settings' (both split on spaces).
pub fn chosen() -> Option<Vec<String>> {
    let chosen = std::env::var("LOOM_AGENT").ok().filter(|c| !c.trim().is_empty()).unwrap_or_else(|| crate::settings::get().adapter);
    let parts: Vec<String> = chosen.split_whitespace().map(str::to_owned).collect();
    (!parts.is_empty()).then_some(parts)
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

impl Link {
    pub fn spawn(program: &[String], cwd: &Path, on_incoming: Rc<dyn Fn(Incoming)>) -> std::io::Result<Rc<Link>> {
        let slot: Rc<RefCell<std::rc::Weak<Link>>> = Rc::default();
        let reader = slot.clone();
        let process = Process::spawn(program, cwd, &[], "acp", move |line| {
            let Some(link) = reader.borrow().upgrade() else { return };
            match line {
                Some(value) => link.dispatch(value, &*on_incoming),
                None => {
                    crate::log::debug!(link.process.tag().to_owned(), "the agent process ended");
                    on_incoming(Incoming::Exited);
                    link.fail_pending();
                }
            }
        })?;
        let link = Rc::new(Link { process, next_id: Cell::new(0), pending: RefCell::new(HashMap::new()) });
        *slot.borrow_mut() = Rc::downgrade(&link);
        Ok(link)
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

    pub fn request(&self, method: &str, params: Value, reply: Reply) {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        self.pending.borrow_mut().insert(id, reply);
        self.process.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
    }

    pub fn pid(&self) -> u32 {
        self.process.pid()
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.process.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    pub fn respond(&self, id: Value, result: Value) {
        self.process.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    pub fn respond_error(&self, id: Value, code: i64, message: &str) {
        self.process.write(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }));
    }

    pub fn stop(&self) {
        self.process.stop();
    }
}
