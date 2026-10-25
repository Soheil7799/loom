//! One agent: its adapter process, its ACP session, and what it shows (chat, jobs, files).
//!
//! An agent can be suspended (no process, just the session id) and resumed: ACP's
//! session/load replays the conversation, so closing loom or idling frees the memory without
//! losing anything but a turn in flight.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::rc::Rc;
use std::time::{Duration, Instant};

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
    /// The agent stopped on an error (what it said).
    Failed(String),
    /// A copy of the conversation was made (fork, rewind): to open beside it.
    Forked(Saved),
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
    /// A question form rather than a permission (answered with its content).
    form: Option<Rc<crate::chat::FormCard>>,
    /// A step in the browser rather than a permission (answered accept/decline).
    url: Option<String>,
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
    /// Messages waiting to be sent: text and attached images (PNG).
    queue: RefCell<VecDeque<(String, Vec<glib::Bytes>)>>,
    /// `!command` runs and their output, for Claude with the next message (as Claude Code's
    /// own `!` does).
    context: RefCell<Vec<String>>,
    shells: Cell<u64>,
    /// The adapter can resume a session without replaying it.
    can_resume: Cell<bool>,
    /// Folders beyond `cwd` it may work in.
    dirs: RefCell<Vec<PathBuf>>,
    /// A turn Claude started by itself (a background task ended): since when, and the timer
    /// that ends it once quiet.
    autonomous: Cell<Option<Instant>>,
    quiet_timer: RefCell<Option<glib::SourceId>>,
    /// The `!command` running, if one is (its process group).
    shell_pid: Cell<Option<i32>>,
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
        let draft = saved.draft.clone();
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
            context: RefCell::default(),
            shells: Cell::new(0),
            shell_pid: Cell::new(None),
            can_resume: Cell::new(false),
            dirs: RefCell::new(saved.dirs.clone()),
            autonomous: Cell::new(None),
            quiet_timer: RefCell::default(),
            modes: RefCell::default(),
            mode: RefCell::new("default".to_owned()),
            pending: RefCell::default(),
            asked: Cell::new(0),
            files: RefCell::default(),
            repos: RefCell::default(),
            repo_status: RefCell::default(),
            reading_repo: Cell::new(false),
            planned: RefCell::default(),
            unseen: Cell::new(false),
            last_active: Cell::new(Instant::now()),
            focused: Cell::new(false),
            on_event: RefCell::default(),
        });
        let weak = Rc::downgrade(&agent);
        *agent.chat.on_submit.borrow_mut() = Some(Rc::new(move |(text, images)| {
            if let Some(agent) = weak.upgrade() {
                agent.submit_with(text, images);
            }
        }));
        let weak = Rc::downgrade(&agent);
        *agent.chat.on_rewind.borrow_mut() = Some(Rc::new(move |(at, text): (Option<String>, String)| {
            if let Some(agent) = weak.upgrade() {
                match at {
                    Some(at) => agent.fork(Some(at), text),
                    // Before Claude said anything: a fresh conversation with it.
                    None => {
                        let mut saved = Saved::new(agent.cwd.clone());
                        saved.draft = text;
                        saved.dirs = agent.dirs.borrow().clone();
                        agent.emit(Event::Forked(saved));
                    }
                }
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
        agent.chat.input.buffer().set_text(&draft);
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

    pub fn session_id(&self) -> Option<String> {
        self.session_id.borrow().clone()
    }

    pub fn saved(&self) -> Saved {
        Saved {
            cwd: self.cwd.clone(),
            session_id: self.session_id.borrow().clone(),
            title: self.title.borrow().clone(),
            named: self.named.get(),
            draft: self.chat.draft(),
            dirs: self.dirs.borrow().clone(),
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
        if self.is_form(serial) {
            return false;
        }
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
        } else if self.turn.get() || self.autonomous.get().is_some() {
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
                "_meta": { "terminal_output_delta": true },
                // Retries, warnings and compaction come as their own updates, not as text.
                "session": { "notices": {}, "compaction": {} },
                // Claude Code's questions (AskUserQuestion) and MCP servers' come as forms.
                "elicitation": { "form": {}, "url": {} }
            },
            "clientInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") }
        });
        let weak = Rc::downgrade(self);
        conn.request("initialize", params, move |result| {
            let Some(agent) = weak.upgrade() else { return };
            match result {
                Ok(result) => {
                    agent.can_resume.set(!result["agentCapabilities"]["sessionCapabilities"]["resume"].is_null());
                    agent.open_session();
                }
                Err(err) => agent.fail(&format!("The agent did not start: {}", message(&err))),
            }
        });
    }

    /// The session's parameters: folder, loom's MCP server, the extra folders.
    fn session_params(&self) -> Value {
        let mut params = json!({ "cwd": self.cwd.to_string_lossy(), "mcpServers": self.mcp_servers() });
        let dirs = self.dirs.borrow();
        if !dirs.is_empty() {
            params["additionalDirectories"] = dirs.iter().map(|d| d.to_string_lossy().into_owned()).collect::<Vec<_>>().into();
        }
        params
    }

    fn open_session(self: &Rc<Self>) {
        let Some(conn) = self.conn.borrow().clone() else { return };
        let cwd = self.cwd.to_string_lossy().into_owned();
        let session = self.session_id.borrow().clone();
        let servers = self.mcp_servers();
        let weak = Rc::downgrade(self);
        // Woken from an idle suspend, the view still holds the conversation: resume it as it
        // is (no replay, nothing cleared).
        if let Some(session) = session.clone().filter(|_| self.can_resume.get() && self.chat.has_messages()) {
            let mut params = self.session_params();
            params["sessionId"] = session.into();
            conn.request("session/resume", params, move |result| {
                let Some(agent) = weak.upgrade() else { return };
                match result {
                    Ok(result) => agent.session_ready(&result),
                    Err(_) => {
                        agent.can_resume.set(false);
                        agent.open_session();
                    }
                }
            });
            return;
        }
        if let Some(session) = session {
            // The conversation comes back as updates; start from an empty view.
            self.chat.clear();
            self.files.borrow_mut().clear();
            self.replaying.set(true);
            let mut params = self.session_params();
            params["sessionId"] = session.into();
            let _ = (&cwd, &servers);
            conn.request("session/load", params, move |result| {
                let Some(agent) = weak.upgrade() else { return };
                agent.replaying.set(false);
                agent.emit(Event::Files);
                // The view was cleared for the replay: what was typed meanwhile goes back.
                for (text, images) in agent.queue.borrow().iter() {
                    agent.chat.user(text);
                    agent.chat.user_images(images);
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
            conn.request("session/new", self.session_params(), move |result| {
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
        let unsent: Vec<_> = self.queue.borrow_mut().drain(..).collect();
        self.chat.restore_unsent(unsent);
        self.drop_questions("The agent stopped");
        self.chat.set_working(None);
        self.set_status(Status::Failed);
        // Not logged in: a way to log in, in the tray (Claude Code's own login).
        let lower = text.to_lowercase();
        if ["auth", "log in", "login", "logged", "401", "credential"].iter().any(|w| lower.contains(w)) {
            let weak = Rc::downgrade(self);
            self.chat.notice_action("Claude Code needs you to log in.", "Log in", Rc::new(move || {
                if let Some(agent) = weak.upgrade() {
                    agent.login();
                }
            }));
        }
        self.emit(Event::Failed(text.to_owned()));
    }

    /// Claude is at work without a prompt from loom (it went on by itself after a background
    /// task): shown as working until it has been quiet a moment.
    fn note_activity(self: &Rc<Self>) {
        if self.replaying.get() || self.turn.get() {
            return;
        }
        if self.autonomous.get().is_none() {
            let now = Instant::now();
            self.autonomous.set(Some(now));
            self.chat.set_working(Some(now));
            self.refresh_status();
        }
        if let Some(timer) = self.quiet_timer.take() {
            timer.remove();
        }
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(Duration::from_secs(4), move || {
            let Some(agent) = weak.upgrade() else { return };
            agent.quiet_timer.take();
            agent.end_autonomous(true);
        });
        *self.quiet_timer.borrow_mut() = Some(timer);
    }

    /// The self-started turn is over (`done`: it finished, rather than a prompt took over).
    fn end_autonomous(self: &Rc<Self>, done: bool) {
        if let Some(timer) = self.quiet_timer.take() {
            timer.remove();
        }
        let Some(started) = self.autonomous.take() else { return };
        if done {
            self.chat.set_working(None);
            self.chat.turn_done(started.elapsed().as_secs());
            if !self.focused.get() {
                self.unseen.set(true);
            }
            self.emit(Event::Done);
        }
        self.refresh_status();
    }

    /// A copy of this conversation (at `at`, the agent message to end on; None: all of it),
    /// announced as Forked with `draft` in its input.
    pub fn fork(self: &Rc<Self>, at: Option<String>, draft: String) {
        let (Some(conn), Some(session)) = (self.conn.borrow().clone(), self.session_id.borrow().clone()) else {
            self.chat.notice("Nothing to copy yet: send a message first.", "dim");
            return;
        };
        let mut params = self.session_params();
        params["sessionId"] = session.into();
        if let Some(at) = &at {
            params["_meta"] = json!({ "jetbrains": { "air": { "fork": { "version": 1, "messageId": at } } } });
        }
        let weak = Rc::downgrade(self);
        let title = format!("{} (copy)", self.title());
        conn.request("session/fork", params, move |result| {
            let Some(agent) = weak.upgrade() else { return };
            match result {
                Ok(result) => {
                    let Some(id) = result["sessionId"].as_str() else { return };
                    let mut saved = agent.saved();
                    saved.session_id = Some(id.to_owned());
                    saved.title = title.clone();
                    saved.named = true;
                    saved.draft = draft.clone();
                    // The fork's own process opens it; this one is not needed for it.
                    let _ = conn_close(&agent, id);
                    agent.emit(Event::Forked(saved));
                }
                Err(err) => agent.chat.notice(&format!("Could not copy the conversation: {}", message(&err)), "error"),
            }
        });
    }

    /// /clear: this agent starts a new conversation (the old one stays in the history).
    pub fn clear_conversation(self: &Rc<Self>) {
        self.stop();
        *self.session_id.borrow_mut() = None;
        if !self.named.get() {
            self.title.borrow_mut().clear();
        }
        self.chat.clear();
        self.chat.set_plan(&Value::Null);
        self.emit(Event::Session);
        self.start();
    }

    /// Lets it work in `dir` too (from the next start of its session: restarted now).
    pub fn add_dir(self: &Rc<Self>, dir: PathBuf) {
        if dir == self.cwd || self.dirs.borrow().contains(&dir) {
            return;
        }
        self.dirs.borrow_mut().push(dir.clone());
        self.chat.notice(&format!("Claude may work in {} too.", dir.display()), "dim");
        self.emit(Event::Session);
        self.restart();
    }

    /// Runs Claude Code's login in the tray, then starts the agent again once it is done.
    fn login(self: &Rc<Self>) {
        match self.tray.run_named("login", "claude /login", true, Some(self.cwd.clone())) {
            Ok(job) => self.tray.peek(&job),
            Err(err) => self.chat.notice(&err, "error"),
        }
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
        let unsent: Vec<_> = self.queue.borrow_mut().drain(..).collect();
        self.chat.restore_unsent(unsent);
        self.drop_questions("Stopped");
        self.chat.set_working(None);
        self.status.set(Status::Suspended);
        self.refresh_header();
        self.emit(Event::Changed);
    }

    // ---- turns ----------------------------------------------------------------------------

    /// A message, with images attached (PNG).
    pub fn submit_with(self: &Rc<Self>, text: String, images: Vec<glib::Bytes>) {
        self.last_active.set(Instant::now());
        // /clear: a fresh conversation in this place (the old one stays in the history).
        if images.is_empty() && text.trim() == "/clear" {
            self.clear_conversation();
            return;
        }
        if images.is_empty()
            && let Some(command) = text.trim_start().strip_prefix('!').map(str::trim).filter(|c| !c.is_empty())
        {
            self.shell(command.to_owned());
            return;
        }
        if !text.is_empty() {
            self.chat.user(&text);
        }
        self.chat.user_images(&images);
        // Claude is working: the message goes into the running turn, as in the terminal.
        if self.turn.get() && self.ready.get() {
            self.steer(text, images);
            return;
        }
        if !self.ready.get() {
            self.queue.borrow_mut().push_back((text, images));
            self.start();
            return;
        }
        self.queue.borrow_mut().push_back((text, images));
        self.send_next();
    }

    /// `!command`: runs it here, shows what it printed, and hands both to Claude with the
    /// next message.
    fn shell(self: &Rc<Self>, command: String) {
        if self.shell_pid.get().is_some() {
            self.chat.notice("A command is still running here (Esc stops it).", "dim");
            return;
        }
        let n = self.shells.get() + 1;
        self.shells.set(n);
        let id = format!("shell-{n}");
        self.chat.tool(&json!({
            "toolCallId": id, "title": format!("! {command}"), "kind": "execute",
            "status": "in_progress", "content": [{ "type": "terminal" }]
        }));
        self.chat.open_tool(&id);
        let mut shell = std::process::Command::new("bash");
        shell
            .arg("-c")
            .arg(&command)
            .current_dir(&self.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0);
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("HERDR_") {
                shell.env_remove(name);
            }
        }
        if let Some(socket) = crate::control::current() {
            shell.env("LOOM_SOCKET", socket);
        }
        let mut child = match shell.spawn() {
            Ok(child) => child,
            Err(err) => {
                self.chat.tool_output(&id, &format!("could not run it: {err}"));
                self.chat.tool(&json!({ "toolCallId": id, "status": "failed" }));
                return;
            }
        };
        self.shell_pid.set(Some(child.id() as i32));
        enum Out {
            Stdout(String),
            Stderr(String),
            Done(Option<i32>),
        }
        let (tx, rx) = async_channel::unbounded::<Out>();
        let readers: Vec<_> = [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)]
            .into_iter()
            .enumerate()
            .filter_map(|(i, stream)| {
                let mut stream = stream?;
                let tx = tx.clone();
                Some(std::thread::spawn(move || {
                    let mut buffer = [0u8; 8192];
                    while let Ok(n) = stream.read(&mut buffer) {
                        if n == 0 {
                            break;
                        }
                        let text = String::from_utf8_lossy(&buffer[..n]).into_owned();
                        let _ = tx.send_blocking(if i == 0 { Out::Stdout(text) } else { Out::Stderr(text) });
                    }
                }))
            })
            .collect();
        std::thread::spawn(move || {
            for reader in readers {
                let _ = reader.join();
            }
            let code = child.wait().ok().and_then(|s| s.code());
            let _ = tx.send_blocking(Out::Done(code));
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (mut stdout, mut stderr) = (String::new(), String::new());
            while let Ok(out) = rx.recv().await {
                let Some(agent) = weak.upgrade() else { return };
                match out {
                    Out::Stdout(text) | Out::Stderr(text) if text.is_empty() => {}
                    Out::Stdout(text) => {
                        agent.chat.tool_output(&id, &text.replace('\n', "\r\n"));
                        stdout.push_str(&text);
                    }
                    Out::Stderr(text) => {
                        agent.chat.tool_output(&id, &text.replace('\n', "\r\n"));
                        stderr.push_str(&text);
                    }
                    Out::Done(code) => {
                        agent.shell_pid.set(None);
                        agent.chat.tool_exit(&id, code.map(i64::from));
                        let status = if code == Some(0) { "completed" } else { "failed" };
                        agent.chat.tool(&json!({ "toolCallId": id, "status": status }));
                        agent.chat.open_tool(&id);
                        let clip = |text: &str| -> String {
                            if text.chars().count() > 30_000 {
                                format!("{}\n[… cut, {} characters in all]", text.chars().take(30_000).collect::<String>(), text.chars().count())
                            } else {
                                text.trim_end().to_owned()
                            }
                        };
                        let stopped = if code.is_none() { "\n[stopped by the user]" } else { "" };
                        agent.context.borrow_mut().push(format!(
                            "<bash-input>{command}</bash-input>\n<bash-stdout>{}{stopped}</bash-stdout><bash-stderr>{}</bash-stderr>",
                            clip(&stdout),
                            clip(&stderr)
                        ));
                        agent.chat.notice("Claude sees this with your next message.", "dim");
                        return;
                    }
                }
            }
        });
    }

    /// Esc while a `!command` runs: it stops (its whole process group).
    fn stop_shell(&self) -> bool {
        let Some(pid) = self.shell_pid.get() else { return false };
        // SAFETY: plain kill(2) on the group the command leads.
        unsafe { libc::kill(-pid, libc::SIGTERM) };
        true
    }

    /// A message as ACP content: `!command` runs first (as Claude Code puts them before the
    /// message), then the images, then the text.
    fn prompt_blocks(&self, text: &str, images: &[glib::Bytes]) -> Vec<Value> {
        let mut prompt: Vec<Value> = self.context.borrow().iter().map(|c| json!({ "type": "text", "text": c })).collect();
        for png in images {
            prompt.push(json!({ "type": "image", "mimeType": "image/png", "data": glib::base64_encode(png).as_str() }));
        }
        if !text.is_empty() || prompt.is_empty() {
            prompt.push(json!({ "type": "text", "text": text }));
        }
        prompt
    }

    /// Sends a message into the turn that is running. If the turn ended meanwhile, it goes as
    /// a prompt of its own; if steering fails, it waits for the turn's end (nothing is lost).
    fn steer(self: &Rc<Self>, text: String, images: Vec<glib::Bytes>) {
        let (Some(conn), Some(session)) = (self.conn.borrow().clone(), self.session_id.borrow().clone()) else {
            self.queue.borrow_mut().push_back((text, images));
            return;
        };
        let prompt = self.prompt_blocks(&text, &images);
        let weak = Rc::downgrade(self);
        let params = json!({ "sessionId": session, "prompt": prompt, "_meta": { "steering": { "idleBehavior": "promptRequired" } } });
        conn.request("_session/steering", params, move |result| {
            let Some(agent) = weak.upgrade() else { return };
            match result {
                Ok(answer) if answer["outcome"].as_str() == Some("promptRequired") => {
                    agent.queue.borrow_mut().push_back((text, images));
                    agent.send_next();
                }
                Ok(_) => agent.context.borrow_mut().clear(),
                Err(_) => {
                    agent.chat.notice("Queued: sent when this turn ends.", "dim");
                    agent.queue.borrow_mut().push_back((text, images));
                    agent.send_next();
                }
            }
        });
    }

    fn send_next(self: &Rc<Self>) {
        if self.turn.get() || !self.ready.get() {
            return;
        }
        let Some((text, images)) = self.queue.borrow_mut().pop_front() else { return };
        self.end_autonomous(false);
        let Some(conn) = self.conn.borrow().clone() else { return };
        let Some(session) = self.session_id.borrow().clone() else { return };
        self.turn.set(true);
        let started = Instant::now();
        self.chat.set_working(Some(started));
        self.refresh_status();
        let weak = Rc::downgrade(self);
        let prompt = self.prompt_blocks(&text, &images);
        self.context.borrow_mut().clear();
        conn.request(
            "session/prompt",
            json!({ "sessionId": session, "prompt": prompt }),
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
        // A `!command` running stops too (Esc stops what is going on, whichever it is).
        self.stop_shell();
        if !self.turn.get() && self.autonomous.get().is_none() {
            return;
        }
        let unsent: Vec<_> = self.queue.borrow_mut().drain(..).collect();
        self.chat.restore_unsent(unsent);
        // The protocol wants every open question answered as cancelled.
        let pending: Vec<Pending> = self.pending.borrow_mut().drain(..).collect();
        let conn = self.conn.borrow().clone();
        for question in pending {
            if let Some(conn) = &conn {
                let answer = if question.form.is_some() || question.url.is_some() { json!({ "action": "cancel" }) } else { json!({ "outcome": { "outcome": "cancelled" } }) };
                conn.respond(question.rpc, answer);
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

    /// Answers question `index` with option `option` (in a form: picks it). False if there
    /// is no such question or option.
    pub fn answer(self: &Rc<Self>, index: usize, option: usize) -> bool {
        let id = {
            let pending = self.pending.borrow();
            let Some(question) = pending.get(index) else { return false };
            if let Some(form) = &question.form {
                return form.pick(option);
            }
            let Some((id, _, _)) = question.options.get(option) else { return false };
            id.clone()
        };
        self.choose(index, &id);
        true
    }

    /// Alt+Y / Alt+N: the oldest permission question's first option of that kind ("allow",
    /// "reject").
    pub fn answer_kind(self: &Rc<Self>, kind: &str) -> bool {
        // The oldest question only: the one the keys are shown on.
        let option = self.pending.borrow().first().and_then(|q| q.options.iter().position(|(_, _, k)| k.starts_with(kind)));
        option.is_some_and(|option| self.answer(0, option))
    }

    /// Alt+Enter: sends the oldest question form as filled in (or shows what it lacks).
    pub fn send_form(self: &Rc<Self>) -> bool {
        let found = self.pending.borrow().iter().find_map(|q| q.form.as_ref().map(|f| (q.serial, f.checked())));
        let Some((serial, content)) = found else { return false };
        if let Some(content) = content {
            self.reply_form(serial, Some(content));
        }
        true
    }

    /// Whether question `serial` is a form (answered in the chat, not from the inbox).
    pub fn is_form(&self, serial: u64) -> bool {
        self.pending.borrow().iter().any(|q| q.serial == serial && q.form.is_some())
    }

    /// A form answered (None: skipped).
    fn reply_form(self: &Rc<Self>, serial: u64, content: Option<Value>) {
        let question = {
            let mut pending = self.pending.borrow_mut();
            let Some(index) = pending.iter().position(|p| p.serial == serial) else { return };
            pending.remove(index)
        };
        let summary = match (&content, &question.form) {
            (Some(_), Some(form)) => form.summary(),
            _ => "→ Skipped".to_owned(),
        };
        if let Some(conn) = self.conn.borrow().clone() {
            let answer = match content {
                Some(content) => json!({ "action": "accept", "content": content }),
                None => json!({ "action": "decline" }),
            };
            conn.respond(question.rpc, answer);
        }
        question.card.resolve(&summary);
        self.last_active.set(Instant::now());
        self.refresh_status();
        self.emit(Event::Changed);
        self.chat.input.grab_focus();
    }

    /// Claude (or an MCP server) asks something: a form in the chat.
    fn elicit(self: &Rc<Self>, rpc: Value, params: &Value) {
        // A step in the browser (an MCP server's sign-in): opened only when the user says so.
        if params["mode"].as_str() == Some("url") {
            let url = params["url"].as_str().unwrap_or("").to_owned();
            let message = params["message"].as_str().unwrap_or("A step in the browser").to_owned();
            let options = vec![
                ("open".to_owned(), "Open in the browser".to_owned(), "allow_once".to_owned()),
                ("decline".to_owned(), "Decline".to_owned(), "reject_once".to_owned()),
            ];
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
            let card = self.chat.permission(&message, Some(&url), false, &options, choose);
            self.pending.borrow_mut().push(Pending { serial, rpc, options, card, title: message.clone(), form: None, url: Some(url) });
            self.refresh_status();
            self.emit(Event::Asked(message));
            return;
        }
        // No mode means a form (older MCP servers leave it out).
        if !matches!(params["mode"].as_str(), None | Some("form")) {
            if let Some(conn) = self.conn.borrow().clone() {
                conn.respond(rpc, json!({ "action": "decline" }));
            }
            return;
        }
        let message = params["message"].as_str().unwrap_or("Claude asks").to_owned();
        let serial = self.asked.get() + 1;
        self.asked.set(serial);
        let weak = Rc::downgrade(self);
        let answer: Rc<dyn Fn(Option<Value>)> = Rc::new(move |content| {
            if let Some(agent) = weak.upgrade() {
                agent.reply_form(serial, content);
            }
        });
        let form = self.chat.form(&message, &params["requestedSchema"], answer);
        let card = form.card.clone();
        self.pending.borrow_mut().push(Pending { serial, rpc, options: Vec::new(), card, title: message.clone(), form: Some(form), url: None });
        self.refresh_status();
        self.emit(Event::Asked(message));
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
            let answer = match &question.url {
                Some(url) if option_id == "open" => {
                    gtk::UriLauncher::new(url).launch(None::<&gtk::Window>, None::<&gio::Cancellable>, |_| {});
                    json!({ "action": "accept" })
                }
                Some(_) => json!({ "action": "decline" }),
                None => json!({ "outcome": { "outcome": "selected", "optionId": option_id } }),
            };
            conn.respond(question.rpc, answer);
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
        let is_plan = input["plan"].is_string() && input["command"].is_null() && input["file_path"].is_null();
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
        let card = self.chat.permission(&format!("Allow {title}?"), detail.as_deref(), is_plan, &options, choose);
        self.pending.borrow_mut().push(Pending { serial, rpc, options, card, title: title.clone(), form: None, url: None });
        self.refresh_status();
        self.emit(Event::Asked(title));
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
                ("elicitation/create", Some(id)) => self.elicit(id, &params),
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
            "agent_message_chunk" => {
                self.note_activity();
                self.chat.agent_text_with(text(), update["messageId"].as_str());
            }
            "notice" => {
                let line = match update["description"].as_str() {
                    Some(more) => format!("{}: {more}", update["title"].as_str().unwrap_or("Notice")),
                    None => update["title"].as_str().unwrap_or("Notice").to_owned(),
                };
                let class = if update["severity"].as_str() == Some("error") { "error" } else { "dim" };
                self.chat.notice(&line, class);
            }
            "compaction_update" => match update["status"].as_str() {
                Some("in_progress") => self.chat.notice("Compacting the conversation (the context was nearly full)…", "dim"),
                Some("completed") => self.chat.notice("Compacted: Claude carries on with a summary of what came before.", "dim"),
                Some("failed") => self.chat.notice("Compacting failed.", "error"),
                _ => {}
            },
            "agent_thought_chunk" => self.chat.thought(text()),
            // Only a replay echoes what was typed; live, the chat already shows it.
            "user_message_chunk" if self.replaying.get() => match update["content"]["type"].as_str() {
                Some("image") => self.chat.user_image_data(update["content"]["data"].as_str().unwrap_or("")),
                _ => self.chat.user_chunk(text()),
            },
            "tool_call" | "tool_call_update" => {
                self.note_activity();
                self.tool(update);
            }
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
                self.chat.set_spend(cost(&update["cost"]), limit(&update["_meta"]["_claude/rateLimit"]));
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

        // A command's output stays with its tool call (the tray is for loom_run's jobs, the
        // ones meant to be watched). A replay has none to show.
        if self.replaying.get() {
            return;
        }
        let meta = &update["_meta"];
        if let Some(data) = meta["terminal_output_delta"]["data"].as_str().or_else(|| meta["terminal_output"]["data"].as_str()) {
            self.chat.tool_output(&id, data);
        }
        if meta.get("terminal_exit").is_some_and(|e| !e.is_null()) {
            self.chat.tool_exit(&id, meta["terminal_exit"]["exit_code"].as_i64());
        }
    }
}

/// A fork made through `agent`'s process is opened by a process of its own: this one lets
/// go of it (best effort; the adapter closes it when it stops anyway).
fn conn_close(agent: &Agent, session: &str) -> Option<()> {
    let conn = agent.conn.borrow().clone()?;
    conn.request("session/close", json!({ "sessionId": session }), |_| {});
    Some(())
}

/// "$0.42" from a usage update's cost.
fn cost(cost: &Value) -> Option<String> {
    let amount = cost["amount"].as_f64()?;
    Some(match cost["currency"].as_str() {
        Some("USD") | None => format!("${amount:.2}"),
        Some(other) => format!("{amount:.2} {other}"),
    })
}

/// ("5h 38%", "5-hour limit: 38% used, resets 14:00") from the account's rate limit.
fn limit(info: &Value) -> Option<(String, String)> {
    let used = info["utilization"].as_f64()?;
    let percent = if used <= 1.0 { used * 100.0 } else { used };
    let (short, long) = match info["rateLimitType"].as_str() {
        Some("five_hour") => ("5h", "5-hour limit"),
        Some("seven_day") => ("week", "Weekly limit"),
        Some("seven_day_opus") => ("week Opus", "Weekly Opus limit"),
        Some("seven_day_sonnet") => ("week Sonnet", "Weekly Sonnet limit"),
        Some("overage") | Some("seven_day_overage_included") => ("extra", "Extra usage"),
        _ => ("limit", "Limit"),
    };
    let resets = info["resetsAt"]
        .as_i64()
        .and_then(|t| glib::DateTime::from_unix_local(t).ok())
        .and_then(|t| t.format("%a %H:%M").ok())
        .map(|t| format!(", resets {t}"))
        .unwrap_or_default();
    Some((format!("{short} {percent:.0}%"), format!("{long}: {percent:.0}% used{resets}")))
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
