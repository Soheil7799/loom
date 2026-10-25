//! One agent: its adapter process, its ACP session, and what it shows (chat, jobs, files).
//!
//! An agent can be suspended (no process, just the session id) and resumed: ACP's
//! session/load replays the conversation, so closing loom or idling frees the memory without
//! losing anything but a turn in flight.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use gtk::{gio, glib};
use serde_json::{Value, json};

use crate::acp::{self, Conn, Incoming};
use crate::chat::{Chat, PermissionCard};
use crate::files::Mark;
use crate::jobs::Tray;
use crate::state::Saved;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Suspended,
    Starting,
    Idle,
    Working,
    Blocked,
    Failed,
}

impl Status {
    /// How much an agent in this state needs the user: 0 most. `unseen`: it finished a turn
    /// nobody has looked at yet.
    pub fn urgency(self, unseen: bool) -> u8 {
        match self {
            Status::Blocked => 0,
            Status::Failed => 1,
            Status::Working => 2,
            Status::Idle if unseen => 3,
            _ => 4,
        }
    }

    pub fn css(self) -> &'static str {
        match self {
            Status::Suspended => "suspended",
            Status::Starting => "starting",
            Status::Idle => "idle",
            Status::Working => "working",
            Status::Blocked => "blocked",
            Status::Failed => "failed",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Status::Suspended => "suspended",
            Status::Starting => "starting…",
            Status::Idle => "",
            Status::Working => "working…",
            Status::Blocked => "needs you",
            Status::Failed => "stopped",
        }
    }
}

/// What the app hears about.
pub enum Event {
    /// Status, title or anything else the sidebar shows.
    Changed,
    /// The session id is known (or changed): worth saving.
    Session,
    Files,
    /// A permission question came in (its title).
    Asked(String),
    /// A turn finished.
    Done,
}

/// The latest repo's status as last read (None: git could not), and when.
#[derive(Clone)]
struct RepoCache {
    path: PathBuf,
    repo: Option<crate::repos::Repo>,
    /// None: out of date (an edit or command finished since).
    read_at: Option<Instant>,
}

struct Pending {
    serial: u64,
    rpc: Value,
    options: Vec<(String, String, String)>,
    card: PermissionCard,
    title: String,
}

pub struct Agent {
    pub id: u32,
    pub cwd: PathBuf,
    /// The repository it works in (see app::project_of).
    pub project: PathBuf,
    pub chat: Rc<Chat>,
    pub tray: Rc<Tray>,
    session_id: RefCell<Option<String>>,
    title: RefCell<String>,
    /// The title was given by hand.
    named: Cell<bool>,
    /// The session's settings (model, effort…) as the agent describes them.
    config: RefCell<Value>,
    /// The session was opened new (not resumed): the default mode applies.
    fresh: Cell<bool>,
    status: Cell<Status>,
    conn: RefCell<Option<Rc<Conn>>>,
    ready: Cell<bool>,
    turn: Cell<bool>,
    replaying: Cell<bool>,
    queue: RefCell<VecDeque<String>>,
    modes: RefCell<Vec<String>>,
    mode: RefCell<String>,
    pending: RefCell<Vec<Pending>>,
    asked: Cell<u64>,
    files: RefCell<Vec<(PathBuf, Mark)>>,
    /// The git repos it has read or changed files in, latest first.
    repos: RefCell<Vec<PathBuf>>,
    /// The latest repo's git status, and when it was read.
    repo_status: RefCell<Option<RepoCache>>,
    /// A git status read is under way (off the main thread).
    reading_repo: Cell<bool>,
    /// Commands in the tray still running for this agent.
    jobs: RefCell<Vec<Rc<crate::jobs::Job>>>,
    /// Files a tool call will write, kept until it completes (a refused edit writes nothing).
    planned: RefCell<HashMap<String, Vec<(PathBuf, Mark)>>>,
    /// A turn ended while someone looked elsewhere.
    pub unseen: Cell<bool>,
    pub last_active: Cell<Instant>,
    pub focused: Cell<bool>,
    on_event: RefCell<Option<Rc<dyn Fn(&Rc<Agent>, Event)>>>,
}

impl Agent {
    pub fn new(id: u32, saved: Saved, tray: Rc<Tray>) -> Rc<Agent> {
        let agent = Rc::new(Agent {
            id,
            project: crate::app::project_of(&saved.cwd),
            cwd: saved.cwd,
            chat: Chat::new(),
            tray,
            session_id: RefCell::new(saved.session_id),
            title: RefCell::new(saved.title),
            named: Cell::new(saved.named),
            config: RefCell::new(Value::Null),
            fresh: Cell::new(false),
            status: Cell::new(Status::Suspended),
            conn: RefCell::default(),
            ready: Cell::new(false),
            turn: Cell::new(false),
            replaying: Cell::new(false),
            queue: RefCell::default(),
            modes: RefCell::default(),
            mode: RefCell::new("default".to_owned()),
            pending: RefCell::default(),
            asked: Cell::new(0),
            files: RefCell::default(),
            repos: RefCell::default(),
            repo_status: RefCell::default(),
            reading_repo: Cell::new(false),
            jobs: RefCell::default(),
            planned: RefCell::default(),
            unseen: Cell::new(false),
            last_active: Cell::new(Instant::now()),
            focused: Cell::new(false),
            on_event: RefCell::default(),
        });
        let weak = Rc::downgrade(&agent);
        *agent.chat.on_submit.borrow_mut() = Some(Rc::new(move |text| {
            if let Some(agent) = weak.upgrade() {
                agent.submit(text);
            }
        }));
        let weak = Rc::downgrade(&agent);
        *agent.chat.on_cancel.borrow_mut() = Some(Rc::new(move |()| {
            if let Some(agent) = weak.upgrade() {
                agent.cancel();
            }
        }));
        let weak = Rc::downgrade(&agent);
        *agent.chat.on_cycle_mode.borrow_mut() = Some(Rc::new(move |()| {
            if let Some(agent) = weak.upgrade() {
                agent.cycle_mode();
            }
        }));
        let weak = Rc::downgrade(&agent);
        *agent.chat.on_option.borrow_mut() = Some(Rc::new(move |(id, value): (String, String)| {
            if let Some(agent) = weak.upgrade() {
                agent.set_option(&id, &value);
            }
        }));
        let weak = Rc::downgrade(&agent);
        *agent.chat.on_answer.borrow_mut() = Some(Rc::new(move |option| {
            weak.upgrade().is_some_and(|agent| agent.answer(0, option))
        }));
        agent.chat.set_cwd(&agent.cwd);
        // Started inside a repo: that is where it works until it touches another.
        if let Some(repo) = crate::repos::repo_of(&agent.cwd) {
            agent.repos.borrow_mut().push(repo);
        }
        agent.refresh_header();
        agent
    }

    pub fn on_event(&self, f: impl Fn(&Rc<Agent>, Event) + 'static) {
        *self.on_event.borrow_mut() = Some(Rc::new(f));
    }

    fn emit(self: &Rc<Self>, event: Event) {
        let f = self.on_event.borrow().clone();
        if let Some(f) = f {
            f(self, event);
        }
    }

    pub fn title(&self) -> String {
        let title = self.title.borrow();
        if title.is_empty() { "New agent".to_owned() } else { title.clone() }
    }

    pub fn status(&self) -> Status {
        self.status.get()
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.status.get(), Status::Working | Status::Blocked | Status::Starting)
    }

    pub fn saved(&self) -> Saved {
        Saved {
            cwd: self.cwd.clone(),
            session_id: self.session_id.borrow().clone(),
            title: self.title.borrow().clone(),
            named: self.named.get(),
        }
    }

    /// Names the agent by hand (empty: back to the agent's own titles).
    pub fn rename(self: &Rc<Self>, title: &str) {
        *self.title.borrow_mut() = title.trim().to_owned();
        self.named.set(!title.trim().is_empty());
        self.refresh_header();
        self.emit(Event::Session);
        self.emit(Event::Changed);
    }

    /// Stops the process and starts it again, resuming the conversation.
    pub fn restart(self: &Rc<Self>) {
        self.stop();
        self.start();
    }

    /// The repos it worked in, latest first.
    pub fn repos(&self) -> Vec<PathBuf> {
        self.repos.borrow().clone()
    }

    /// The latest repo's status as last read. Never waits on git: a stale or missing one is
    /// read again in the background, and the agent reports Changed when it arrives.
    pub fn repo(self: &Rc<Self>) -> Option<crate::repos::Repo> {
        let path = self.repos.borrow().first().cloned()?;
        let cached = self.repo_status.borrow().clone().filter(|c| c.path == path);
        let fresh = cached.as_ref().and_then(|c| c.read_at).is_some_and(|at| at.elapsed().as_secs() < 5);
        // A suspended agent changes nothing: what was read last stays.
        if !fresh && !self.reading_repo.get() && (self.status.get() != Status::Suspended || cached.is_none()) {
            self.reading_repo.set(true);
            let weak = Rc::downgrade(self);
            let reading = path.clone();
            glib::spawn_future_local(async move {
                let found = gio::spawn_blocking(move || crate::repos::status(&reading)).await.ok().flatten();
                let Some(agent) = weak.upgrade() else { return };
                agent.reading_repo.set(false);
                let before = agent.repo_status.borrow().as_ref().filter(|c| c.path == path).map(|c| c.repo.clone());
                let changed = before.as_ref() != Some(&found);
                *agent.repo_status.borrow_mut() = Some(RepoCache { path, repo: found, read_at: Some(Instant::now()) });
                if changed {
                    agent.emit(Event::Changed);
                }
            });
        }
        cached.and_then(|c| c.repo)
    }

    /// The repo's status is out of date (kept on screen until the new one is read).
    fn repo_changed(&self) {
        if let Some(cache) = self.repo_status.borrow_mut().as_mut() {
            cache.read_at = None;
        }
    }

    /// Notes that it touched `path`; its repo moves to the front.
    fn touched(self: &Rc<Self>, path: &Path) {
        let Some(repo) = crate::repos::repo_of(path) else { return };
        // Only the project's repos (a note read in another one doesn't make it "where it works").
        if !repo.starts_with(&self.project) && !self.project.starts_with(&repo) {
            return;
        }
        let mut repos = self.repos.borrow_mut();
        if repos.first() == Some(&repo) {
            return;
        }
        repos.retain(|r| *r != repo);
        repos.insert(0, repo);
        repos.truncate(8);
        drop(repos);
        if !self.replaying.get() {
            self.emit(Event::Changed);
        }
    }

    pub fn files(&self) -> Vec<(PathBuf, Mark)> {
        self.files.borrow().clone()
    }

    /// Permission questions waiting, oldest first: (serial, title, option names and kinds).
    pub fn questions(&self) -> Vec<(u64, String, Vec<(String, String)>)> {
        self.pending
            .borrow()
            .iter()
            .map(|p| (p.serial, p.title.clone(), p.options.iter().map(|(_, name, kind)| (name.clone(), kind.clone())).collect()))
            .collect()
    }

    /// Answers the question numbered `serial` (if still open) with option `option`.
    pub fn answer_serial(self: &Rc<Self>, serial: u64, option: usize) -> bool {
        let index = self.pending.borrow().iter().position(|p| p.serial == serial);
        index.is_some_and(|index| self.answer(index, option))
    }

    fn set_status(self: &Rc<Self>, status: Status) {
        if self.status.replace(status) != status {
            self.refresh_header();
            self.emit(Event::Changed);
        }
    }

    /// Works the status out from the process, the turn and the questions.
    fn refresh_status(self: &Rc<Self>) {
        let status = if self.conn.borrow().is_none() {
            match self.status.get() {
                Status::Failed => Status::Failed,
                _ => Status::Suspended,
            }
        } else if !self.ready.get() {
            Status::Starting
        } else if !self.pending.borrow().is_empty() {
            Status::Blocked
        } else if self.turn.get() {
            Status::Working
        } else {
            Status::Idle
        };
        self.set_status(status);
    }

    fn refresh_header(&self) {
        let full = self.cwd.to_string_lossy().into_owned();
        // "project / subfolder", else the folder name: the full path is the tooltip.
        let project = self.project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let place = match self.cwd.strip_prefix(&self.project) {
            Ok(rest) if !rest.as_os_str().is_empty() => format!("{project} / {}", rest.display()),
            _ => project,
        };
        let status = self.status.get();
        self.chat.set_title(&self.title(), &place, &full.replace(&*gtk::glib::home_dir().to_string_lossy(), "~"));
        self.chat.set_state(status.describe(), status.css());
        self.chat.set_mode(&self.mode.borrow());
        let note = format!("Starting Claude in {place}…");
        self.chat.set_starting((status == Status::Starting).then_some(note.as_str()));
    }

    // ---- lifecycle ------------------------------------------------------------------------

    /// Starts the process and opens (or resumes) the session. Nothing if already running.
    pub fn start(self: &Rc<Self>) {
        if self.conn.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let conn = match Conn::spawn(&self.cwd, move |incoming| {
            if let Some(agent) = weak.upgrade() {
                agent.incoming(incoming);
            }
        }) {
            Ok(conn) => conn,
            Err(err) => {
                self.chat.notice(&format!("Could not start the agent ({}): {err}", acp::adapter_command().join(" ")), "error");
                self.set_status(Status::Failed);
                return;
            }
        };
        *self.conn.borrow_mut() = Some(conn.clone());
        self.ready.set(false);
        self.status.set(Status::Starting);
        self.refresh_header();
        self.emit(Event::Changed);
        let params = json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": { "readTextFile": false, "writeTextFile": false },
                "terminal": false,
                // Commands' output comes as display terminals (shown in the job tray).
                "_meta": { "terminal_output_delta": true }
            },
            "clientInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") }
        });
        let weak = Rc::downgrade(self);
        conn.request("initialize", params, move |result| {
            let Some(agent) = weak.upgrade() else { return };
            match result {
                Ok(_) => agent.open_session(),
                Err(err) => agent.fail(&format!("The agent did not start: {}", message(&err))),
            }
        });
    }

    fn open_session(self: &Rc<Self>) {
        let Some(conn) = self.conn.borrow().clone() else { return };
        let cwd = self.cwd.to_string_lossy().into_owned();
        let session = self.session_id.borrow().clone();
        let servers = self.mcp_servers();
        let weak = Rc::downgrade(self);
        if let Some(session) = session {
            // The conversation comes back as updates; start from an empty view.
            self.chat.clear();
            self.files.borrow_mut().clear();
            self.replaying.set(true);
            conn.request("session/load", json!({ "sessionId": session, "cwd": cwd, "mcpServers": servers }), move |result| {
                let Some(agent) = weak.upgrade() else { return };
                agent.replaying.set(false);
                agent.emit(Event::Files);
                // The view was cleared for the replay: what was typed meanwhile goes back.
                for text in agent.queue.borrow().iter() {
                    agent.chat.user(text);
                }
                match result {
                    Ok(result) => agent.session_ready(&result),
                    // A session that never got a prompt is never saved: nothing was lost.
                    Err(err) if message(&err).contains("not found") => {
                        *agent.session_id.borrow_mut() = None;
                        agent.open_session();
                    }
                    // Anything else keeps the session, to try again later.
                    Err(err) => agent.fail(&format!("Could not resume this conversation: {}", message(&err))),
                }
            });
        } else {
            conn.request("session/new", json!({ "cwd": cwd, "mcpServers": servers }), move |result| {
                let Some(agent) = weak.upgrade() else { return };
                match result {
                    Ok(result) => {
                        agent.fresh.set(true);
                        *agent.session_id.borrow_mut() = result["sessionId"].as_str().map(str::to_owned);
                        agent.emit(Event::Session);
                        agent.session_ready(&result);
                    }
                    Err(err) => agent.fail(&format!("Could not open a session: {}", message(&err))),
                }
            });
        }
    }

    /// loom's own MCP server (`loom mcp`), told how to reach this app and which agent it
    /// serves: its tools run jobs in the tray, open files for the user, notify.
    fn mcp_servers(&self) -> Value {
        let (Some(socket), Some(exe)) = (crate::control::current(), own_program()) else { return json!([]) };
        json!([{
            "name": "loom",
            "command": exe.to_string_lossy(),
            "args": ["mcp"],
            "env": [
                { "name": "LOOM_SOCKET", "value": socket.to_string_lossy() },
                { "name": "LOOM_AGENT", "value": self.id.to_string() }
            ]
        }])
    }

    /// Shows the session's settings (model, effort…) as pickers in the header.
    fn set_config(&self, options: &Value) {
        if !options.is_array() {
            return;
        }
        *self.config.borrow_mut() = options.clone();
        self.chat.set_options(options);
    }

    fn set_option(self: &Rc<Self>, id: &str, value: &str) {
        let (Some(conn), Some(session)) = (self.conn.borrow().clone(), self.session_id.borrow().clone()) else { return };
        let weak = Rc::downgrade(self);
        let name = id.to_owned();
        conn.request("session/set_config_option", json!({ "sessionId": session, "configId": id, "value": value }), move |result| {
            let Some(agent) = weak.upgrade() else { return };
            match result {
                Ok(result) => agent.set_config(&result["configOptions"]),
                Err(err) => agent.chat.notice(&format!("Could not change {name}: {}", message(&err)), "error"),
            }
        });
    }

    fn session_ready(self: &Rc<Self>, result: &Value) {
        self.set_config(&result["configOptions"]);
        if let Some(modes) = result.get("modes").filter(|m| !m.is_null()) {
            *self.modes.borrow_mut() = modes["availableModes"]
                .as_array()
                .map(|all| all.iter().filter_map(|m| m["id"].as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            if let Some(current) = modes["currentModeId"].as_str() {
                *self.mode.borrow_mut() = current.to_owned();
            }
        }
        self.ready.set(true);
        self.refresh_header();
        self.refresh_status();
        // A new conversation starts in the mode chosen in the settings.
        let wanted = crate::settings::get().default_mode;
        if self.fresh.replace(false) && !wanted.is_empty() && *self.mode.borrow() != wanted && self.modes.borrow().contains(&wanted) {
            self.set_mode(&wanted);
        }
        self.send_next();
    }

    fn set_mode(self: &Rc<Self>, mode: &str) {
        let (Some(conn), Some(session)) = (self.conn.borrow().clone(), self.session_id.borrow().clone()) else { return };
        let weak = Rc::downgrade(self);
        let next = mode.to_owned();
        conn.request("session/set_mode", json!({ "sessionId": session, "modeId": next }), move |result| {
            let Some(agent) = weak.upgrade() else { return };
            match result {
                Ok(_) => {
                    *agent.mode.borrow_mut() = next.clone();
                    agent.refresh_header();
                }
                Err(err) => agent.chat.notice(&format!("Could not change mode: {}", message(&err)), "error"),
            }
        });
    }

    fn fail(self: &Rc<Self>, text: &str) {
        self.chat.notice(text, "error");
        if let Some(conn) = self.conn.borrow_mut().take() {
            conn.stop();
        }
        self.ready.set(false);
        self.turn.set(false);
        self.queue.borrow_mut().clear();
        self.drop_questions("The agent stopped");
        self.end_jobs();
        self.chat.set_working(None);
        self.set_status(Status::Failed);
    }

    /// Stops the process, keeping the session to resume. False (and nothing done) while busy.
    pub fn suspend(self: &Rc<Self>) -> bool {
        if self.is_busy() {
            return false;
        }
        self.stop();
        true
    }

    /// Stops the process, busy or not.
    pub fn stop(self: &Rc<Self>) {
        if let Some(conn) = self.conn.borrow_mut().take() {
            conn.stop();
        }
        self.ready.set(false);
        self.turn.set(false);
        self.queue.borrow_mut().clear();
        self.drop_questions("Stopped");
        self.end_jobs();
        self.chat.set_working(None);
        self.status.set(Status::Suspended);
        self.refresh_header();
        self.emit(Event::Changed);
    }

    // ---- turns ----------------------------------------------------------------------------

    pub fn submit(self: &Rc<Self>, text: String) {
        self.last_active.set(Instant::now());
        self.chat.user(&text);
        if self.turn.get() || !self.ready.get() {
            if self.turn.get() {
                self.chat.notice("Queued: sent when this turn ends.", "dim");
            }
            self.queue.borrow_mut().push_back(text);
            self.start();
            return;
        }
        self.queue.borrow_mut().push_back(text);
        self.send_next();
    }

    fn send_next(self: &Rc<Self>) {
        if self.turn.get() || !self.ready.get() {
            return;
        }
        let Some(text) = self.queue.borrow_mut().pop_front() else { return };
        let Some(conn) = self.conn.borrow().clone() else { return };
        let Some(session) = self.session_id.borrow().clone() else { return };
        self.turn.set(true);
        let started = Instant::now();
        self.chat.set_working(Some(started));
        self.refresh_status();
        let weak = Rc::downgrade(self);
        conn.request(
            "session/prompt",
            json!({ "sessionId": session, "prompt": [{ "type": "text", "text": text }] }),
            move |result| {
                let Some(agent) = weak.upgrade() else { return };
                agent.turn.set(false);
                agent.chat.set_working(None);
                // The process died with the turn: fail() has said so already.
                if agent.conn.borrow().is_none() {
                    return;
                }
                agent.last_active.set(Instant::now());
                match result {
                    Ok(result) => {
                        if result["stopReason"].as_str() == Some("cancelled") {
                            agent.chat.notice("Stopped.", "dim");
                            agent.end_jobs();
                        } else if let Some(reason) = result["stopReason"].as_str().filter(|r| *r != "end_turn") {
                            agent.chat.notice(&format!("Turn ended: {}", reason.replace('_', " ")), "dim");
                        }
                    }
                    Err(err) => agent.chat.notice(&format!("Error: {}", message(&err)), "error"),
                }
                agent.chat.turn_done(started.elapsed().as_secs());
                if !agent.focused.get() {
                    agent.unseen.set(true);
                }
                agent.refresh_status();
                agent.emit(Event::Done);
                agent.send_next();
            },
        );
    }

    pub fn cancel(self: &Rc<Self>) {
        if !self.turn.get() {
            return;
        }
        self.queue.borrow_mut().clear();
        // The protocol wants every open question answered as cancelled.
        let pending: Vec<Pending> = self.pending.borrow_mut().drain(..).collect();
        let conn = self.conn.borrow().clone();
        for question in pending {
            if let Some(conn) = &conn {
                conn.respond(question.rpc, json!({ "outcome": { "outcome": "cancelled" } }));
            }
            question.card.resolve("Cancelled");
        }
        if let (Some(conn), Some(session)) = (conn, self.session_id.borrow().clone()) {
            conn.notify("session/cancel", json!({ "sessionId": session }));
        }
        self.refresh_status();
    }

    pub fn cycle_mode(self: &Rc<Self>) {
        let modes = self.modes.borrow().clone();
        // Bypass is never one key press away.
        let cycle: Vec<&String> = modes.iter().filter(|m| m.as_str() != "bypassPermissions").collect();
        if cycle.is_empty() {
            return;
        }
        let current = self.mode.borrow().clone();
        let next = cycle.iter().position(|m| **m == current).map_or(0, |i| (i + 1) % cycle.len());
        let next = cycle[next].clone();
        self.set_mode(&next);
    }

    // ---- permission questions -------------------------------------------------------------

    /// Answers question `index` with option `option`. False if there is no such question.
    pub fn answer(self: &Rc<Self>, index: usize, option: usize) -> bool {
        let id = {
            let pending = self.pending.borrow();
            let Some(question) = pending.get(index) else { return false };
            let Some((id, _, _)) = question.options.get(option) else { return false };
            id.clone()
        };
        self.choose(index, &id);
        true
    }

    fn choose(self: &Rc<Self>, index: usize, option_id: &str) {
        let question = {
            let mut pending = self.pending.borrow_mut();
            if index >= pending.len() {
                return;
            }
            pending.remove(index)
        };
        let name = question.options.iter().find(|(id, _, _)| id == option_id).map(|(_, name, _)| name.clone()).unwrap_or_default();
        if let Some(conn) = self.conn.borrow().clone() {
            conn.respond(question.rpc, json!({ "outcome": { "outcome": "selected", "optionId": option_id } }));
        }
        question.card.resolve(&format!("→ {name}"));
        self.last_active.set(Instant::now());
        self.refresh_status();
        // Still blocked on another question: the status did not change, the inbox did.
        self.emit(Event::Changed);
        self.chat.input.grab_focus();
    }

    fn ask(self: &Rc<Self>, rpc: Value, params: &Value) {
        let tool = &params["toolCall"];
        let title = tool["title"].as_str().unwrap_or("this tool").replace('`', "");
        let input = &tool["rawInput"];
        let detail = input["command"]
            .as_str()
            .or_else(|| input["file_path"].as_str())
            .or_else(|| input["url"].as_str())
            .or_else(|| input["plan"].as_str())
            .map(str::to_owned);
        let options: Vec<(String, String, String)> = params["options"]
            .as_array()
            .map(|options| {
                options
                    .iter()
                    .filter_map(|o| {
                        Some((o["optionId"].as_str()?.to_owned(), o["name"].as_str()?.to_owned(), o["kind"].as_str().unwrap_or("").to_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let serial = self.asked.get() + 1;
        self.asked.set(serial);
        let weak = Rc::downgrade(self);
        let choose: Rc<dyn Fn(String)> = Rc::new(move |id: String| {
            let Some(agent) = weak.upgrade() else { return };
            let index = agent.pending.borrow().iter().position(|p| p.serial == serial);
            if let Some(index) = index {
                agent.choose(index, &id);
            }
        });
        let card = self.chat.permission(&format!("Allow {title}?"), detail.as_deref(), &options, choose);
        self.pending.borrow_mut().push(Pending { serial, rpc, options, card, title: title.clone() });
        self.refresh_status();
        self.emit(Event::Asked(title));
    }

    /// Marks this agent's running commands ended: nothing more will be heard of them.
    fn end_jobs(&self) {
        for job in self.jobs.borrow_mut().drain(..) {
            if job.is_running() {
                job.finish(None);
            }
        }
    }

    fn drop_questions(&self, why: &str) {
        for question in self.pending.borrow_mut().drain(..) {
            question.card.resolve(why);
        }
    }

    // ---- what the agent sends -------------------------------------------------------------

    fn incoming(self: &Rc<Self>, incoming: Incoming) {
        match incoming {
            Incoming::Exited => {
                if self.conn.borrow().is_some() {
                    self.fail("The agent process ended.");
                }
            }
            Incoming::Call { id, method, params } => match (method.as_str(), id) {
                ("session/update", _) => self.update(&params["update"]),
                ("session/request_permission", Some(id)) => self.ask(id, &params),
                (_, Some(id)) => {
                    if let Some(conn) = self.conn.borrow().clone() {
                        conn.respond_error(id, -32601, "Method not found");
                    }
                }
                _ => {}
            },
        }
    }

    fn update(self: &Rc<Self>, update: &Value) {
        let text = || update["content"]["text"].as_str().unwrap_or("");
        match update["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" => self.chat.agent_text(text()),
            "agent_thought_chunk" => self.chat.thought(text()),
            // Only a replay echoes what was typed; live, the chat already shows it.
            "user_message_chunk" if self.replaying.get() => self.chat.user_chunk(text()),
            "tool_call" | "tool_call_update" => self.tool(update),
            "plan" => self.chat.set_plan(&update["entries"]),
            "available_commands_update" => {
                let commands = update["availableCommands"]
                    .as_array()
                    .map(|all| {
                        all.iter()
                            .filter_map(|c| Some((c["name"].as_str()?.to_owned(), c["description"].as_str().unwrap_or("").to_owned())))
                            // Remote control has no place on this machine.
                            .filter(|(name, _)| !name.contains("remote"))
                            .collect()
                    })
                    .unwrap_or_default();
                self.chat.set_commands(commands);
            }
            "current_mode_update" => {
                if let Some(mode) = update["currentModeId"].as_str() {
                    *self.mode.borrow_mut() = mode.to_owned();
                    self.refresh_header();
                }
            }
            "config_option_update" => self.set_config(&update["configOptions"]),
            "session_info_update" => {
                // A name given by hand stays.
                if let Some(title) = update["title"].as_str().filter(|t| !t.is_empty() && !self.named.get()) {
                    *self.title.borrow_mut() = title.to_owned();
                    self.refresh_header();
                    self.emit(Event::Session);
                    self.emit(Event::Changed);
                }
            }
            "usage_update" => {
                self.chat.set_usage(update["used"].as_u64().unwrap_or(0), update["size"].as_u64().unwrap_or(0));
            }
            _ => {}
        }
    }

    fn tool(self: &Rc<Self>, update: &Value) {
        self.chat.tool(update);
        let id = update["toolCallId"].as_str().unwrap_or("").to_owned();

        // Where it works: the repos of the files it reads and writes.
        let absolute = |p: &str| {
            let p = Path::new(p);
            if p.is_absolute() { p.to_owned() } else { self.cwd.join(p) }
        };
        for location in update["locations"].as_array().into_iter().flatten() {
            if let Some(path) = location["path"].as_str() {
                self.touched(&absolute(path));
            }
        }
        for item in update["content"].as_array().into_iter().flatten() {
            if let Some(path) = item["path"].as_str() {
                self.touched(&absolute(path));
            }
        }
        // A finished edit or command changes the repo's status.
        if update["status"].as_str() == Some("completed") {
            self.repo_changed();
        }

        // Files: noted from the diffs, kept once the call completes.
        if let Some(content) = update["content"].as_array() {
            let writes: Vec<(PathBuf, Mark)> = content
                .iter()
                .filter(|c| c["type"].as_str() == Some("diff"))
                .filter_map(|c| {
                    let path = Path::new(c["path"].as_str()?);
                    let path = if path.is_absolute() { path.to_owned() } else { self.cwd.join(path) };
                    Some((path, if c["oldText"].is_null() { Mark::Created } else { Mark::Changed }))
                })
                .collect();
            if !writes.is_empty() {
                self.planned.borrow_mut().insert(id.clone(), writes);
            }
        }
        match update["status"].as_str() {
            Some("completed") => {
                if let Some(writes) = self.planned.borrow_mut().remove(&id) {
                    let mut files = self.files.borrow_mut();
                    for (path, mark) in writes {
                        match files.iter().position(|(p, _)| *p == path) {
                            // Created stays created; the newest change goes on top.
                            Some(i) => {
                                let (p, old) = files.remove(i);
                                files.insert(0, (p, if old == Mark::Created { old } else { mark }));
                            }
                            None => files.insert(0, (path, mark)),
                        }
                    }
                    drop(files);
                    if !self.replaying.get() {
                        self.emit(Event::Files);
                    }
                }
            }
            Some("failed") => {
                self.planned.borrow_mut().remove(&id);
            }
            _ => {}
        }

        // Commands: their output goes to a job in the tray. A replay has none to show.
        if self.replaying.get() {
            return;
        }
        // The command itself, once known (the first report may carry only "Terminal"); the
        // report that names it need not carry terminal data, and the adapter names a
        // command's terminal after its tool call.
        let name = update["rawInput"]["command"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| update["title"].as_str().filter(|t| *t != "Terminal").map(|t| t.replace('`', "")))
            .map(|c| c.lines().next().unwrap_or(&c).to_owned());
        if let (Some(name), Some(job)) = (&name, self.tray.agent_job_for(&id)) {
            job.rename(name);
        }
        let meta = &update["_meta"];
        let terminal = meta["terminal_info"]["terminal_id"]
            .as_str()
            .or_else(|| meta["terminal_output_delta"]["terminal_id"].as_str())
            .or_else(|| meta["terminal_exit"]["terminal_id"].as_str());
        let Some(terminal) = terminal else { return };
        let job = match self.tray.agent_job_for(terminal) {
            Some(job) => {
                if let Some(name) = &name {
                    job.rename(name);
                }
                job
            }
            None => {
                let fallback = update["title"].as_str().unwrap_or("command").replace('`', "");
                let job = self.tray.agent_job(terminal, name.as_deref().unwrap_or(&fallback), &self.title());
                self.jobs.borrow_mut().push(job.clone());
                let tray = self.tray.clone();
                let shown = job.clone();
                self.chat.tool_terminal(&id, Rc::new(move || tray.peek(&shown)));
                job
            }
        };
        if let Some(data) = meta["terminal_output_delta"]["data"].as_str() {
            job.feed(data);
        } else if let Some(data) = meta["terminal_output"]["data"].as_str() {
            job.feed(data);
        }
        if meta.get("terminal_exit").is_some_and(|e| !e.is_null()) {
            job.finish(meta["terminal_exit"]["exit_code"].as_i64());
            self.jobs.borrow_mut().retain(|j| !Rc::ptr_eq(j, &job));
        }
    }
}

/// This program's path, for agents to start `loom mcp`. Replaced on disk while running (a
/// reinstall), /proc reports it with " (deleted)": the new file at that path is the one.
fn own_program() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let text = exe.to_string_lossy();
    Some(match text.strip_suffix(" (deleted)") {
        Some(path) => PathBuf::from(path),
        None => exe,
    })
}

fn message(error: &Value) -> String {
    error["message"].as_str().map(str::to_owned).unwrap_or_else(|| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_needs_you_comes_first() {
        let mut states = vec![
            (Status::Suspended, false),
            (Status::Idle, true),
            (Status::Working, false),
            (Status::Idle, false),
            (Status::Blocked, false),
            (Status::Failed, false),
        ];
        states.sort_by_key(|(s, unseen)| s.urgency(*unseen));
        assert_eq!(
            states,
            vec![
                (Status::Blocked, false),
                (Status::Failed, false),
                (Status::Working, false),
                (Status::Idle, true),
                (Status::Suspended, false),
                (Status::Idle, false),
            ]
        );
    }
}
