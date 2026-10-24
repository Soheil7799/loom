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
    pub chat: Rc<Chat>,
    pub tray: Rc<Tray>,
    session_id: RefCell<Option<String>>,
    title: RefCell<String>,
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
            cwd: saved.cwd,
            chat: Chat::new(),
            tray,
            session_id: RefCell::new(saved.session_id),
            title: RefCell::new(saved.title),
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
        *agent.chat.on_answer.borrow_mut() = Some(Rc::new(move |option| {
            weak.upgrade().is_some_and(|agent| agent.answer(0, option))
        }));
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
        Saved { cwd: self.cwd.clone(), session_id: self.session_id.borrow().clone(), title: self.title.borrow().clone() }
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
        let place = self.cwd.to_string_lossy().replace(&*gtk::glib::home_dir().to_string_lossy(), "~");
        self.chat.set_title(&self.title(), &place);
        self.chat.set_state(self.status.get().describe());
        self.chat.set_mode(&self.mode.borrow());
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
        let weak = Rc::downgrade(self);
        if let Some(session) = session {
            // The conversation comes back as updates; start from an empty view.
            self.chat.clear();
            self.files.borrow_mut().clear();
            self.replaying.set(true);
            conn.request("session/load", json!({ "sessionId": session, "cwd": cwd, "mcpServers": [] }), move |result| {
                let Some(agent) = weak.upgrade() else { return };
                agent.replaying.set(false);
                agent.emit(Event::Files);
                match result {
                    Ok(result) => agent.session_ready(&result),
                    // A session that never got a prompt is never saved: nothing was lost.
                    Err(err) if message(&err).contains("not found") => {
                        agent.chat.notice("The earlier conversation was empty; this is a new one.", "dim");
                        *agent.session_id.borrow_mut() = None;
                        agent.open_session();
                    }
                    // Anything else keeps the session, to try again later.
                    Err(err) => agent.fail(&format!("Could not resume this conversation: {}", message(&err))),
                }
            });
        } else {
            conn.request("session/new", json!({ "cwd": cwd, "mcpServers": [] }), move |result| {
                let Some(agent) = weak.upgrade() else { return };
                match result {
                    Ok(result) => {
                        *agent.session_id.borrow_mut() = result["sessionId"].as_str().map(str::to_owned);
                        agent.emit(Event::Session);
                        agent.session_ready(&result);
                    }
                    Err(err) => agent.fail(&format!("Could not open a session: {}", message(&err))),
                }
            });
        }
    }

    fn session_ready(self: &Rc<Self>, result: &Value) {
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
        self.send_next();
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
        self.refresh_status();
        let weak = Rc::downgrade(self);
        conn.request(
            "session/prompt",
            json!({ "sessionId": session, "prompt": [{ "type": "text", "text": text }] }),
            move |result| {
                let Some(agent) = weak.upgrade() else { return };
                agent.turn.set(false);
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
        let (Some(conn), Some(session)) = (self.conn.borrow().clone(), self.session_id.borrow().clone()) else { return };
        let weak = Rc::downgrade(self);
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
            "session_info_update" => {
                if let Some(title) = update["title"].as_str().filter(|t| !t.is_empty()) {
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
        }
    }
}

fn message(error: &Value) -> String {
    error["message"].as_str().map(str::to_owned).unwrap_or_else(|| error.to_string())
}
