//! The agents' providers: everything that knows how a coding agent is spoken to lives under
//! here. The rest of loom speaks one language to all of them, ACP's (requests such as
//! `session/new` and `session/prompt`, updates such as `tool_call`), through [`Conn`].
//!
//! - `claude`: Claude Code itself, over its stream-json protocol (the default).
//! - `acp`: any ACP adapter process (set in Settings → Advanced, or `LOOM_AGENT`).

mod acp;
mod claude;
mod process;

use std::path::Path;
use std::rc::Rc;

use serde_json::Value;


/// What the agent sends on its own: a notification, or a request (when `id` is set) that
/// must get an answer.
pub enum Incoming {
    Call { id: Option<Value>, method: String, params: Value },
    /// The process ended (or closed its output).
    Exited,
}

pub(crate) type Reply = Box<dyn FnOnce(Result<Value, Value>)>;

enum Link {
    Acp(Rc<acp::Link>),
    Claude(Rc<claude::Link>),
}

pub struct Conn {
    link: Link,
}

/// The adapter command loom would run for ACP (the one set, else its default).
pub fn adapter_command() -> Vec<String> {
    acp::chosen().unwrap_or_else(acp::default_adapter)
}

/// The ACP adapter run when none is set.
pub fn default_adapter() -> Vec<String> {
    acp::default_adapter()
}

/// What runs the agents, for people: "Claude Code 2.1.312", or the adapter's command.
pub fn describe() -> String {
    match acp::chosen() {
        Some(command) => format!("ACP adapter: {}", command.join(" ")),
        None => match claude::installed_version() {
            Some(version) => format!("Claude Code {version}"),
            None => "Claude Code (not found)".to_owned(),
        },
    }
}

/// Said when the installed Claude Code is not the version loom was built and tested against
/// (its protocol is not documented, so a newer one may change what loom reads); nothing when
/// they match, or while an ACP adapter is used. Reads `claude --version` (once): call it off
/// the main loop.
pub fn version_note() -> Option<String> {
    if acp::chosen().is_some() {
        return None;
    }
    let built = claude::BUILT_FOR;
    let Some(installed) = claude::installed_version() else {
        return Some("Claude Code was not found: install it, or set an ACP adapter in Settings → Advanced.".to_owned());
    };
    let number = |v: &str| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    match number(&installed).cmp(&number(built)) {
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => Some(format!("Claude Code {installed} is newer than {built}, the version this loom was built for. If something looks wrong or stops working, check for a loom update.")),
        std::cmp::Ordering::Less => Some(format!("Claude Code {installed} is older than {built}, the version this loom was built for. If something looks wrong or stops working, update Claude Code.")),
    }
}

impl Conn {
    /// A connection for an agent working in `cwd`. Claude Code starts with the session (on
    /// `session/new`, `session/load`, `session/resume`); an adapter starts now.
    pub fn spawn(cwd: &Path, on_incoming: impl Fn(Incoming) + 'static) -> std::io::Result<Rc<Conn>> {
        let on_incoming: Rc<dyn Fn(Incoming)> = Rc::new(on_incoming);
        let link = match acp::chosen() {
            Some(program) => Link::Acp(acp::Link::spawn(&program, cwd, on_incoming)?),
            None => Link::Claude(claude::Link::new(cwd, on_incoming)?),
        };
        Ok(Rc::new(Conn { link }))
    }

    pub fn request(&self, method: &str, params: Value, reply: impl FnOnce(Result<Value, Value>) + 'static) {
        match &self.link {
            Link::Acp(link) => link.request(method, params, Box::new(reply)),
            Link::Claude(link) => link.request(method, params, Box::new(reply)),
        }
    }

    /// The process id (0 while Claude Code is not started yet).
    pub fn pid(&self) -> u32 {
        match &self.link {
            Link::Acp(link) => link.pid(),
            Link::Claude(link) => link.pid(),
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        match &self.link {
            Link::Acp(link) => link.notify(method, params),
            Link::Claude(link) => link.notify(method, params),
        }
    }

    pub fn respond(&self, id: Value, result: Value) {
        match &self.link {
            Link::Acp(link) => link.respond(id, result),
            Link::Claude(link) => link.respond(id, result),
        }
    }

    pub fn respond_error(&self, id: Value, code: i64, message: &str) {
        match &self.link {
            Link::Acp(link) => link.respond_error(id, code, message),
            Link::Claude(link) => link.respond_error(id, code, message),
        }
    }

    /// Stops the agent and everything it started.
    pub fn stop(&self) {
        match &self.link {
            Link::Acp(link) => link.stop(),
            Link::Claude(link) => link.stop(),
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.stop();
    }
}
