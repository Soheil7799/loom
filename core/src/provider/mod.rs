//! The agents' provider: everything that knows how Claude Code is spoken to lives under here.
//! The rest of loom speaks one language to it, loom's own (requests such as `session/new` and
//! `session/prompt`, updates such as `tool_call`), through [`Conn`]; `claude` turns that into
//! Claude Code's stream-json protocol and back.

mod claude;
pub use claude::user_mcp_servers;
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

pub type Reply = Box<dyn FnOnce(Result<Value, Value>)>;

pub struct Conn {
    link: Rc<claude::Link>,
}

/// Conversation `session` read back from Claude Code's own file as session updates (what
/// `session/load` replays), with no process started. None: no such conversation.
pub fn read_back(session: &str, cwd: &Path) -> Option<Vec<Value>> {
    claude::read_back(session, cwd)
}

/// Conversations `sessions` as `session/list` gives them (sessionId, cwd, title, updatedAt),
/// newest first, from Claude Code's files. Off the UI's thread.
pub fn list_of(sessions: &[String]) -> Vec<Value> {
    claude::list_of(sessions)
}

/// The conversations of `sessions` whose words hold every word of `query`, each with a line
/// of it. Reads Claude Code's files: off the UI's thread.
pub fn search(sessions: &[String], query: &str) -> Vec<(String, String)> {
    claude::search(sessions, query)
}

/// What runs an agent, as a command line (for an error that names it).
pub fn program() -> String {
    claude::program().to_string_lossy().into_owned()
}

/// What runs the agents, for people: "Claude Code 2.1.312".
pub fn describe() -> String {
    // Never waits for `claude --version` (this is shown on the main loop).
    match claude::known_version() {
        Some(version) => format!("Claude Code {version}"),
        None => "Claude Code".to_owned(),
    }
}

/// Said when the installed Claude Code may not speak what loom reads (its protocol is not
/// documented): older than the version loom was built and tested against, or a newer minor
/// version (2.1.x to 2.2). Newer patch versions say nothing: Claude Code ships one nearly
/// every day. Reads `claude --version` (once): call it off the main loop.
pub fn version_note() -> Option<String> {
    let Some(installed) = claude::installed_version() else {
        return Some("Claude Code was not found: install it, then start the agent again.".to_owned());
    };
    note_for(&installed, claude::BUILT_FOR)
}

fn note_for(installed: &str, built: &str) -> Option<String> {
    let number = |v: &str| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (new, old) = (number(installed), number(built));
    if new < old {
        Some(format!("Claude Code {installed} is older than {built}, the version this loom was built for. If something looks wrong or stops working, update Claude Code."))
    } else if new.get(..2) != old.get(..2) {
        Some(format!("Claude Code {installed} is newer than {built}, the version this loom was built for. If something looks wrong or stops working, check for a loom update."))
    } else {
        None
    }
}

#[cfg(test)]
mod version_tests {
    #[test]
    fn only_older_or_a_new_minor_version_is_said() {
        assert!(super::note_for("2.1.330", "2.1.330").is_none());
        assert!(super::note_for("2.1.345", "2.1.330").is_none());
        assert!(super::note_for("2.1.329", "2.1.330").unwrap().contains("older"));
        assert!(super::note_for("2.2.0", "2.1.330").unwrap().contains("newer"));
    }
}

impl Conn {
    /// A connection for an agent working in `cwd`. Claude Code starts with the session (on
    /// `session/new`, `session/load`, `session/resume`).
    /// `agent`: the agent it is for (its processes go in that agent's scope).
    pub fn spawn(cwd: &Path, agent: Option<u32>, on_incoming: impl Fn(Incoming) + 'static) -> std::io::Result<Rc<Conn>> {
        let on_incoming: Rc<dyn Fn(Incoming)> = Rc::new(on_incoming);
        Ok(Rc::new(Conn { link: claude::Link::new(cwd, agent, on_incoming)? }))
    }

    pub fn request(&self, method: &str, params: Value, reply: impl FnOnce(Result<Value, Value>) + 'static) {
        self.link.request(method, params, Box::new(reply))
    }

    /// The process id (0 while Claude Code is not started yet).
    pub fn pid(&self) -> u32 {
        self.link.pid()
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.link.notify(method, params)
    }

    pub fn respond(&self, id: Value, result: Value) {
        self.link.respond(id, result)
    }

    pub fn respond_error(&self, id: Value, code: i64, message: &str) {
        self.link.respond_error(id, code, message)
    }

    /// Stops the agent and everything it started.
    pub fn stop(&self) {
        self.link.stop()
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.stop();
    }
}
