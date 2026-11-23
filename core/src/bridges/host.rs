//! Bridges: how something outside loom reaches it (the Telegram bot, your other devices
//! through the remote bridge, or whatever someone writes). Providers are what loom drives; bridges are what drive loom.
//!
//! Each bridge is a process of its own (`loom bridge <name>`), started by loom while the
//! settings switch it on and stopped with it (its stdin closes when loom goes). It talks to
//! loom over the core API, on a connection loom makes and hands it when starting it (fd 3):
//! nothing else can connect as it, so an agent can't answer its own permission requests.
//!
//! The bridge protocol (docs/bridges.md) rides on it for the bridges that still speak it (the
//! remote bridge, for the phone app already shipped): a request is the API's `bridge` command
//! (`{method, params}` → `{"ok": …}` or `{"error": "…"}`), the events are the `bridge/<name>`
//! resource's items, each added and taken away at once
//! (`{"event": "agent" | "asked" | "done" | "unsent" | "failed" | "closed" | "settings", …}`).
//! The Telegram bot is on the API itself (`link`).
//!
//! What a bridge can do through `bridge` is the methods in `Host::handle`, nothing else.

#[cfg(test)]
use super::asking_from;
use super::{Link, random, random_hex};

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use data_encoding::BASE64_NOPAD;
use serde_json::{Value, json};

use crate::agent::view::Png;
use crate::agent::{Agent, Asking, Event};
use crate::settings::Settings;
use crate::workspace::Workspace;

/// Pictures sent in any format, as PNG (the UI's toolkit reads them); called off the UI's thread.
pub type ToPng = Arc<dyn Fn(Vec<u8>) -> Result<Vec<u8>, String> + Send + Sync>;

/// The bridges loom has: their names (`loom bridge <name>`).
const BRIDGES: &[&str] = &["telegram", "remote"];

/// How long a pairing code shown in Settings stays good.
const PAIRING: Duration = Duration::from_secs(600);

/// The running bridges' secrets: (secret, bridge).
/// Events told so far (their items' ids).
static EVENTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Running {
    /// Kept open: the bridge stops when it closes (and loom with it).
    stdin: ChildStdin,
    /// Waited for and killed under this lock only, so it is never killed once reaped (its pid
    /// may be another program's by then).
    child: Arc<Mutex<Child>>,
    secret: String,
    started: Instant,
    /// What it was started with (a change to some of it starts it again).
    settings: Value,
}

pub struct Host {
    ws: RefCell<Weak<Workspace>>,
    to_png: RefCell<Option<ToPng>>,
    app_id: String,
    /// Where an older loom's bridge socket was (removed if left).
    socket: PathBuf,
    running: RefCell<HashMap<&'static str, Running>>,
    /// Starts that failed in a row, per bridge (the wait before the next grows).
    failures: RefCell<HashMap<&'static str, u32>>,
    /// The agent views last told, by agent: a change nobody would see is not told.
    told: RefCell<HashMap<u32, String>>,
    /// Bumped on every view: a bridge keeps the newest it got.
    seq: Cell<u64>,
    /// The code Settings shows to pair a device with the remote bridge, while it shows one.
    pairing: RefCell<Option<String>>,
    /// loom's Notes chat, when the app has one (see [`Notes`]).
    notes: RefCell<Option<Rc<dyn Notes>>>,
}

/// loom's Notes chat (the app's, not the workspace's: its chats are agents of their own). Only
/// the remote bridge reaches it: the phone's Vault tab.
pub trait Notes {
    /// `{vault, chats: [{session, title, updated, mark}], current}`: the vault's name (null when
    /// Notes is off), its past chats newest first (`mark`: "working", "blocked" or null), the
    /// session of the chat open. The list is read again in the background: ask again for it.
    fn chats(&self) -> Value;
    /// Chat `session` open (None: a new one), as if picked in loom: its agent.
    fn open(&self, session: Option<String>) -> Option<Rc<Agent>>;
    /// An open chat's agent.
    fn agent(&self, id: u32) -> Option<Rc<Agent>>;
}

thread_local! {
    static HOST: RefCell<Weak<Host>> = const { RefCell::new(Weak::new()) };
}

/// The bridges' host, while loom runs.
pub(crate) fn current() -> Option<Rc<Host>> {
    HOST.with(|h| h.borrow().upgrade())
}

/// The file descriptor a bridge finds its API connection on (`api_fd` in the hello line).
pub const API_FD: i32 = 3;

/// `command`'s process gets `stream` as `API_FD` (the only descriptor of ours it keeps).
fn hand_down(command: &mut Command, stream: &UnixStream) {
    let handed = std::os::fd::AsRawFd::as_raw_fd(stream);
    // SAFETY: dup2 and fcntl only, between fork and exec.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(move || {
            if handed == API_FD {
                // Already there: only kept open across exec.
                libc::fcntl(API_FD, libc::F_SETFD, 0);
            } else if libc::dup2(handed, API_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// The old bridge socket's path (gone; a file a crashed older loom left is removed).
fn old_socket_path(app_id: &str) -> PathBuf {
    let control = crate::control::socket_path(app_id);
    let stem = control.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "loom".to_owned());
    control.with_file_name(format!("{stem}-bridges.sock"))
}


/// The remote bridge's program, `loom-remote` beside loom (it brings iroh; loom doesn't).
pub fn remote_program() -> Option<PathBuf> {
    let program = std::env::current_exe().ok()?.with_file_name("loom-remote");
    program.is_file().then_some(program)
}

/// Device `id` (`name`) let in, and pairing closed (Settings' Accept, the API's pair_confirm).
pub fn accept_device(id: &str, name: &str) {
    crate::settings::update(|s| {
        if !s.remote_devices.iter().any(|(d, _)| d == id) {
            s.remote_devices.push((id.to_owned(), name.to_owned()));
        }
    });
    pair(false);
}

/// Opens pairing (a new code for Settings to show) or closes it.
pub fn pair(open: bool) {
    let Some(host) = HOST.with(|h| h.borrow().upgrade()) else { return };
    let code = open.then(|| random(12));
    *host.pairing.borrow_mut() = code.clone();
    host.apply(&crate::settings::get());
    if let Some(code) = code {
        let weak = Rc::downgrade(&host);
        let _ = crate::rt::timeout_once(PAIRING, move || {
            let Some(host) = weak.upgrade() else { return };
            if host.pairing.borrow().as_deref() == Some(code.as_str()) {
                pair(false);
            }
        });
    }
}

impl Host {
    pub fn new(app_id: &str) -> Rc<Host> {
        Rc::new(Host {
            ws: RefCell::new(Weak::new()),
            to_png: RefCell::default(),
            app_id: app_id.to_owned(),
            socket: old_socket_path(app_id),
            running: RefCell::default(),
            failures: RefCell::default(),
            told: RefCell::default(),
            seq: Cell::new(0),
            pairing: RefCell::new(None),
            notes: RefCell::default(),
        })
    }

    /// Starts the bridges the settings switch on (the API serves them).
    pub fn attach(self: &Rc<Self>, ws: &Rc<Workspace>, to_png: ToPng) {
        *self.ws.borrow_mut() = Rc::downgrade(ws);
        crate::agent::commands::set_to_png(to_png.clone());
        *self.to_png.borrow_mut() = Some(to_png);
        HOST.with(|h| *h.borrow_mut() = Rc::downgrade(self));
        let _ = std::fs::remove_file(&self.socket);
        self.apply(&crate::settings::get());
    }

    /// Starts, stops or tells each bridge as the settings say.
    pub fn apply(self: &Rc<Self>, s: &Settings) {
        // The remote bridge's key, made the first time it is switched on (saved, then applied).
        if s.remote_enabled && s.remote_key.is_empty()
            && let Some(key) = random_hex(32)
        {
            crate::rt::idle_once(move || crate::settings::update(|s| s.remote_key = key));
        }
        if !s.remote_enabled {
            *self.pairing.borrow_mut() = None;
        }
        for name in BRIDGES {
            let wanted = self.settings_for(name, s);
            let running = self.running.borrow().get(name).map(|r| r.settings.clone());
            match (wanted, running) {
                (None, running) => {
                    if running.is_some() {
                        self.stop(name);
                    }
                    let state = match *name {
                        "telegram" => (if s.telegram_enabled { Link::NeedsToken } else { Link::Off }).to_json(),
                        "remote" if !s.remote_enabled => json!({ "state": "off" }),
                        "remote" if remote_program().is_none() => json!({ "state": "failed", "why": "loom-remote is not installed beside loom." }),
                        "remote" if s.remote_key.is_empty() => json!({ "state": "failed", "why": "No key could be made (/dev/urandom can't be read)." }),
                        _ => json!({ "state": "off" }),
                    };
                    set_state(name, state);
                }
                (Some(_), None) => self.start(name),
                // Settings it can't take while running: started again with them.
                (Some(settings), Some(was)) if restarts_on(name).iter().any(|key| settings[key] != was[key]) => {
                    self.stop(name);
                    self.start(name);
                }
                (Some(settings), Some(_)) => {
                    if let Some(running) = self.running.borrow_mut().get_mut(name) {
                        running.settings = settings.clone();
                    }
                    emit(Some(name), json!({ "event": "settings", "settings": settings }));
                }
            }
        }
    }

    /// What bridge `name` is told of the settings; None: it is off.
    fn settings_for(&self, name: &str, s: &Settings) -> Option<Value> {
        match name {
            "telegram" => {
                let token = s.telegram_token.trim();
                (s.telegram_enabled && !token.is_empty()).then(|| {
                    json!({ "token": token, "user": s.telegram_user, "speech": crate::stt::configured(), "debug": s.debug_log })
                })
            }
            "remote" => (s.remote_enabled && !s.remote_key.is_empty() && remote_program().is_some()).then(|| {
                json!({
                    "key": s.remote_key,
                    "relay": s.remote_relay.trim(),
                    "devices": s.remote_devices.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
                    "pairing": *self.pairing.borrow(),
                    "debug": s.debug_log,
                })
            }),
            _ => None,
        }
    }

    /// Starts bridge `name`: its process, with its API connection handed down.
    fn start(self: &Rc<Self>, name: &'static str) {
        let Some(settings) = self.settings_for(name, &crate::settings::get()) else { return };
        let secret = random(32);
        // `loom bridge <name>` (from loomd too: the loom installed beside it).
        let Some(exe) = crate::paths::loom_program() else { return };
        // Its connection to the core API, made here: the bridge gets the other end as fd 3.
        let Ok((ours, theirs)) = UnixStream::pair() else { return };
        let mut command = Command::new(exe);
        command.args(["bridge", name]).stdin(Stdio::piped());
        hand_down(&mut command, &theirs);
        let process = match command.spawn() {
            Ok(process) => process,
            Err(err) => {
                crate::log::debug!("bridges", "{name} did not start: {err}");
                set_state(name, json!({ "state": "failed", "why": format!("The {} bridge did not start: {err}", title(name)) }));
                return;
            }
        };
        drop(theirs);
        if let Err(err) = crate::api::socket::hand(ours, crate::api::Principal::Bridge(name.to_owned()), process.id()) {
            crate::log::debug!("bridges", "{name} has no API connection: {err}");
        }
        // Known before the bridge can use it.
        let mut process = process;
        let Some(mut stdin) = process.stdin.take() else { return };
        // Its own settings too (its token: on this pipe only, never through the API).
        let hello = json!({ "app": self.app_id, "api_fd": API_FD, "settings": settings }).to_string() + "\n";
        let _ = stdin.write_all(hello.as_bytes()).and_then(|()| stdin.flush());
        crate::log::debug!("bridges", "{name} started (pid {})", process.id());
        set_state(name, json!({ "state": "connecting" }));
        let child = Arc::new(Mutex::new(process));
        let (weak, mine, waited) = (Rc::downgrade(self), secret.clone(), child.clone());
        crate::rt::spawn_local(async move {
            let _ = crate::rt::spawn_blocking(move || {
                while matches!(waited.lock().unwrap().try_wait(), Ok(None)) {
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .await;
            let Some(host) = weak.upgrade() else { return };
            host.ended(name, &mine);
        });
        self.running.borrow_mut().insert(name, Running { stdin, child, secret, started: Instant::now(), settings });
    }

    fn stop(&self, name: &str) {
        if let Some(running) = self.running.borrow_mut().remove(name) {
            drop(running.stdin);
            let mut child = running.child.lock().unwrap();
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
        }
        self.failures.borrow_mut().remove(name);
    }

    /// Bridge `name` (the run with `secret`) exited: started again if still wanted, a bit later
    /// each time it keeps failing.
    fn ended(self: &Rc<Self>, name: &'static str, secret: &str) {
        let started = {
            let mut running = self.running.borrow_mut();
            // Stopped (or restarted) on purpose: nothing to do.
            if running.get(name).is_none_or(|r| r.secret != secret) {
                return;
            }
            running.remove(name).map(|r| r.started)
        };
        let failures = {
            let mut failures = self.failures.borrow_mut();
            let count = failures.entry(name).or_insert(0);
            // It ran a while: this is a new failure, not the same one again.
            if started.is_some_and(|at| at.elapsed() > Duration::from_secs(60)) {
                *count = 0;
            }
            *count += 1;
            *count
        };
        let wait = (1u64 << failures.min(6)).min(60);
        crate::log::debug!("bridges", "{name} stopped; again in {wait} s");
        set_state(name, json!({ "state": "failed", "why": format!("The {} bridge stopped; starting it again in {wait} s.", title(name)) }));
        let weak = Rc::downgrade(self);
        let _ = crate::rt::timeout_once(Duration::from_secs(wait), move || {
            let Some(host) = weak.upgrade() else { return };
            if !host.running.borrow().contains_key(name) && host.settings_for(name, &crate::settings::get()).is_some() {
                host.start(name);
            }
        });
    }

    /// The app's Notes chat, for the remote bridge.
    pub fn set_notes(&self, notes: Rc<dyn Notes>) {
        *self.notes.borrow_mut() = Some(notes);
    }

    /// Agent `id`: one of the workspace's, or an open Notes chat's.
    fn find(&self, ws: &Workspace, id: u32) -> Option<Rc<Agent>> {
        ws.agent(id).or_else(|| self.notes.borrow().as_ref().and_then(|n| n.agent(id)))
    }

    /// An agent as bridges see it.
    fn view(&self, ws: &Workspace, agent: &Agent) -> Value {
        self.seq.set(self.seq.get() + 1);
        let questions: Vec<Value> = agent.open_questions().into_iter().filter_map(|serial| agent.asking(serial).map(|a| asking_json(serial, &a))).collect();
        json!({
            "id": agent.id,
            "seq": self.seq.get(),
            "title": agent.title(),
            // A Notes chat's agent is in no project of the sidebar.
            "project": if ws.agent(agent.id).is_some() { ws.project_label(&agent.project) } else { "Notes".to_owned() },
            "notes": ws.agent(agent.id).is_none(),
            "status": agent.status().css(),
            "unseen": agent.unseen.get(),
            "busy": agent.is_busy(),
            "questions": questions,
        })
    }

    /// What happened to an agent: told to the bridges. `looking`: the user has it in front of
    /// them in loom (a bridge then stays quiet).
    pub fn agent_event(&self, agent: &Agent, event: &Event, looking: bool) {
        self.tell(agent, event, looking, None);
    }

    /// What happened to a Notes chat: told to the remote bridge only (the Telegram bot knows
    /// nothing of Notes chats).
    pub fn notes_event(&self, agent: &Agent, event: &Event, looking: bool) {
        self.tell(agent, event, looking, Some("remote"));
    }

    fn tell(&self, agent: &Agent, event: &Event, looking: bool, to: Option<&str>) {
        if !subscribed() {
            return;
        }
        let Some(ws) = self.ws.borrow().upgrade() else { return };
        let view = self.view(&ws, agent);
        // The view but for its seq: a change to the same as last told is not told.
        let key = {
            let mut v = view.clone();
            v["seq"] = Value::Null;
            v.to_string()
        };
        let mut told = json!({ "agent": view, "looking": looking });
        match event {
            Event::Changed => {
                if self.told.borrow().get(&agent.id) == Some(&key) {
                    return;
                }
                told["event"] = json!("agent");
            }
            Event::Asked(_) => told["event"] = json!("asked"),
            // Each turn's reply goes to the phone as it ends (a turn that left background
            // agents going is not the end of the work, but its words are said).
            Event::TurnEnded => {
                told["event"] = json!("done");
                told["reply"] = json!(agent.view.last_reply());
                told["took"] = json!(crate::paths::elapsed(agent.last_took.get()));
                told["error"] = json!(agent.last_error.borrow().clone());
                told["cancelled"] = json!(agent.last_cancelled.get());
            }
            Event::Unsent(count) => {
                told["event"] = json!("unsent");
                told["count"] = json!(count);
            }
            Event::Failed(why) => {
                told["event"] = json!("failed");
                told["why"] = json!(why);
            }
            _ => return,
        }
        self.told.borrow_mut().insert(agent.id, key);
        emit(to, told);
    }

    /// A Notes chat's agent closed (only the remote bridge knew it).
    pub fn notes_closed(&self, agent: u32) {
        self.told.borrow_mut().remove(&agent);
        emit(Some("remote"), json!({ "event": "closed", "agent": agent }));
    }

    /// An agent closed.
    pub fn closed(&self, agent: u32) {
        self.told.borrow_mut().remove(&agent);
        emit(None, json!({ "event": "closed", "agent": agent }));
    }

    /// A request from bridge `bridge` (the API's `bridge` command), answered on `reply`.
    pub(crate) fn request(self: &Rc<Self>, bridge: &str, method: &str, p: &Value, reply: async_channel::Sender<Value>) {
        self.handle(bridge, method, p, reply);
    }

    /// A request from bridge `bridge`, answered on `reply` (now, or when it is done).
    fn handle(self: &Rc<Self>, bridge: &str, method: &str, p: &Value, reply: async_channel::Sender<Value>) {
        let answer = |result: Result<Value, String>| respond(&reply, result);
        let Some(ws) = self.ws.borrow().upgrade() else { return answer(Err("loom is closing".into())) };
        let agent = || {
            let id = p["agent"].as_u64().unwrap_or(0) as u32;
            self.find(&ws, id).ok_or_else(|| "That agent is closed.".to_owned())
        };
        let serial = p["serial"].as_u64().unwrap_or(0);
        let agents = || Value::Array(ws.agents_in_order().iter().map(|a| self.view(&ws, a)).collect());
        crate::log::debug!("bridges", "{bridge} → {method}");
        let result = match method {
            "hello" => Ok(json!({ "settings": self.settings_for(bridge, &crate::settings::get()), "agents": agents() })),
            "agents" => Ok(agents()),
            // A message: pictures (any format the UI reads) go as pasted ones. A shell command
            // (`!…`) runs only from loom's own window, never from a bridge.
            "send" if p["text"].as_str().unwrap_or("").trim_start().starts_with('!') => Err("! commands run only from loom itself.".to_owned()),
            // Pictures are read off the UI's thread (they may be large), then sent.
            "send" => {
                let id = match agent() {
                    Ok(agent) => agent.id,
                    Err(why) => return answer(Err(why)),
                };
                let data: Vec<String> = p["images"].as_array().into_iter().flatten().map(|d| d.as_str().unwrap_or("").to_owned()).collect();
                let (to_png, text) = (self.to_png.borrow().clone(), p["text"].as_str().unwrap_or("").to_owned());
                let (host, reply) = (self.clone(), reply.clone());
                crate::rt::spawn_local(async move {
                    let images = crate::rt::spawn_blocking(move || {
                        data.into_iter()
                            .map(|data| {
                                let bytes = decode(&data).ok_or("That picture could not be read.")?;
                                let to_png = to_png.as_ref().ok_or("Pictures can't be read here.")?;
                                to_png(bytes).map(Png::from).map_err(|_| "That picture could not be read.".to_owned())
                            })
                            .collect::<Result<Vec<Png>, String>>()
                    })
                    .await
                    .unwrap_or_else(|_| Err("That picture could not be read.".into()));
                    respond(&reply, images.and_then(|images| {
                        let ws = host.ws.borrow().upgrade().ok_or("loom is closing")?;
                        let agent = host.find(&ws, id).ok_or("That agent is closed.")?;
                        agent.submit_with(text, images);
                        Ok(host.view(&ws, &agent))
                    }));
                });
                return;
            }
            // A file for the agent: saved in its folder, under <bridge>-files/, off the UI's
            // thread. Its path back.
            "save" => {
                let dir = match agent() {
                    Ok(agent) => agent.cwd.join(format!("{bridge}-files")),
                    Err(why) => return answer(Err(why)),
                };
                let (name, data) = (safe_name(p["name"].as_str().unwrap_or("")), p["data"].as_str().unwrap_or("").to_owned());
                let reply = reply.clone();
                crate::rt::spawn_local(async move {
                    let saved = crate::rt::spawn_blocking(move || {
                        let bytes = decode(&data).ok_or("That file could not be read.")?;
                        let target = crate::files::free_name(&dir.join(name));
                        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&target, &bytes)).map_err(|e| format!("Could not save it in the agent's folder: {e}"))?;
                        Ok(json!(target.to_string_lossy()))
                    })
                    .await
                    .unwrap_or_else(|_| Err("Could not save it in the agent's folder.".into()));
                    respond(&reply, saved);
                });
                return;
            }
            // The account's limits (shared by every agent), and with `agent` that one's context
            // and cost. Null parts are not known yet (no answer from Claude since loom started).
            "usage" => Ok(json!({
                "account": crate::agent::transcript::account_limits(),
                "agent": p["agent"].as_u64().and_then(|id| self.find(&ws, id as u32)).map(|a| a.usage()),
            })),
            // The Notes chat, for the phone's Vault tab: its chats, and one of them open.
            "notes" if bridge == "remote" => Ok(self.notes.borrow().as_ref().map_or(json!({ "vault": null, "chats": [] }), |n| n.chats())),
            "notes_open" if bridge == "remote" => {
                let notes = self.notes.borrow().clone();
                let opened = notes.and_then(|n| n.open(p["session"].as_str().map(str::to_owned)));
                opened.map(|agent| self.view(&ws, &agent)).ok_or_else(|| "Notes is off in loom (Settings → Add-ons → Notes).".to_owned())
            }
            // The notes vault, read-only (browse.rs): a folder, a note, a search. Off the UI's thread.
            "notes_list" | "note_read" | "notes_search" if bridge == "remote" => {
                let Some(root) = crate::vault::current() else { return answer(Err("Notes is off in loom (Settings → Add-ons → Notes).".into())) };
                let (method, p, reply) = (method.to_owned(), p.clone(), reply.clone());
                crate::rt::spawn_local(async move {
                    let result = crate::rt::spawn_blocking(move || match method.as_str() {
                        "notes_list" => super::browse::list(&root, p["folder"].as_str().unwrap_or("")),
                        "note_read" => super::browse::read(&root, p["path"].as_str().unwrap_or("")),
                        _ => super::browse::search(&root, p["query"].as_str().unwrap_or(""), p["limit"].as_u64().map_or(30, |limit| limit.min(100)) as usize),
                    })
                    .await
                    .unwrap_or_else(|_| Err("Could not read the vault.".into()));
                    respond(&reply, result);
                });
                return;
            }
            // The files the agent created or changed, newest first: what it did, to look at.
            "files" => {
                let files = match agent() {
                    Ok(agent) => agent.files().into_iter().map(|(path, mark)| (path.strip_prefix(&agent.cwd).unwrap_or(&path).to_path_buf(), path, mark == crate::agent::Mark::Created)).collect::<Vec<_>>(),
                    Err(why) => return answer(Err(why)),
                };
                let reply = reply.clone();
                crate::rt::spawn_local(async move {
                    let listed = crate::rt::spawn_blocking(move || {
                        Value::Array(
                            files
                                .iter()
                                .map(|(name, path, created)| {
                                    let size = std::fs::metadata(path).ok().map(|m| m.len());
                                    json!({ "path": path.to_string_lossy(), "name": name.to_string_lossy(), "created": created, "exists": size.is_some(), "size": size })
                                })
                                .collect(),
                        )
                    })
                    .await
                    .map_err(|_| "Could not list them.".to_owned());
                    respond(&reply, listed);
                });
                return;
            }
            // One of them, to read: only a file this agent wrote (never any other path), text
            // only, up to FILE_READ bytes. Off the UI's thread.
            "file" => {
                let path = PathBuf::from(p["path"].as_str().unwrap_or(""));
                match agent() {
                    Ok(agent) if agent.files().iter().any(|(f, _)| *f == path) => {}
                    Ok(_) => return answer(Err("That is not a file this agent wrote.".into())),
                    Err(why) => return answer(Err(why)),
                }
                let reply = reply.clone();
                crate::rt::spawn_local(async move {
                    let read = crate::rt::spawn_blocking(move || read_text(&path))
                        .await
                        .unwrap_or_else(|_| Err("Could not read it.".into()));
                    respond(&reply, read);
                });
                return;
            }
            // "Always allow" is a decision made at the desk, never from a bridge.
            "answer" => agent().and_then(|agent| {
                let option = p["option"].as_u64().unwrap_or(0) as usize;
                let kind = agent.asking(serial).and_then(|a| a.options.get(option).map(|o| o.1.clone()));
                if kind.as_deref() == Some("allow_always") {
                    return Err("Not from outside loom.".to_owned());
                }
                let answered = agent.answer_serial(serial, option);
                Ok(json!({ "answered": answered, "agent": self.view(&ws, &agent) }))
            }),
            "form" => agent().and_then(|agent| {
                let (done, problem) = match p["do"].as_str().unwrap_or("") {
                    "pick" => (agent.pick_in_form(serial, p["field"].as_u64().unwrap_or(0) as usize, p["option"].as_u64().unwrap_or(0) as usize), None),
                    "write" => (agent.write_in_form(serial, p["text"].as_str().unwrap_or("")), None),
                    "send" => agent.send_form_serial(serial).map_or_else(|e| (false, Some(e)), |()| (true, None)),
                    "skip" => agent.skip_form(serial).map_or_else(|e| (false, Some(e)), |()| (true, None)),
                    other => return Err(format!("no form action {other}")),
                };
                Ok(json!({ "done": done, "problem": problem, "agent": self.view(&ws, &agent) }))
            }),
            "stop" => agent().map(|agent| {
                agent.cancel();
                self.view(&ws, &agent)
            }),
            "last_reply" => agent().map(|agent| json!(agent.view.last_reply())),
            // The whole conversation, or what changed since the device last asked.
            "history" => agent().map(|agent| agent.history(p["epoch"].as_str().unwrap_or(""), p["after"].as_u64().unwrap_or(0))),
            // Speech to text with loom's settings (its key never leaves loom); answered when heard.
            "transcribe" => {
                if !crate::stt::configured() {
                    return answer(Err("Speech to text is not set up in loom.".into()));
                }
                let Some(audio) = decode(p["audio"].as_str().unwrap_or("")) else { return answer(Err("That recording could not be read.".into())) };
                let reply = reply.clone();
                crate::stt::transcribe(audio, p["name"].as_str().unwrap_or("voice.ogg"), p["mime"].as_str().unwrap_or("audio/ogg"), move |result| {
                    let _ = reply.try_send(match result {
                        Ok(text) => json!({ "ok": text }),
                        Err(error) => json!({ "error": error }),
                    });
                });
                return;
            }
            "state" => {
                set_state(bridge, p.clone());
                Ok(json!({}))
            }
            other => Err(format!("unknown request {other}")),
        };
        answer(result);
    }
}

/// What a bridge is started again for when it changes (the remote bridge's endpoint is made
/// from its key and relay).
fn restarts_on(name: &str) -> &'static [&'static str] {
    match name {
        "remote" => &["key", "relay"],
        // The bot has its settings from its start only (on the API, no secrets go).
        "telegram" => &["token", "user", "speech", "debug"],
        _ => &[],
    }
}

/// A bridge's name, as said to the user.
fn title(name: &str) -> &'static str {
    match name {
        "telegram" => "Telegram",
        _ => "remote",
    }
}

/// `result` as a bridge reads it.
fn respond(reply: &async_channel::Sender<Value>, result: Result<Value, String>) {
    let _ = reply.try_send(match result {
        Ok(value) => json!({ "ok": value }),
        Err(error) => json!({ "error": error }),
    });
}

/// The bridges whose events are followed (`bridge/<name>`).
fn followers() -> Vec<crate::api::Store> {
    let hub = crate::api::hub();
    ["remote", "telegram"].iter().filter_map(|name| hub.existing(&format!("bridge/{name}"))).filter(crate::api::Store::followed).collect()
}

fn subscribed() -> bool {
    !followers().is_empty()
}

/// An event to every bridge following its events (`to`: only that one): an item added to
/// `bridge/<name>` and taken away at once (a follower reads the ops; a snapshot is empty).
fn emit(to: Option<&str>, event: Value) {
    for store in followers() {
        if to.is_some_and(|to| store.name() != format!("bridge/{to}")) {
            continue;
        }
        let id = format!("e{}", EVENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1);
        store.upsert(json!({ "id": id, "event": event }));
        store.remove(&id);
    }
}

fn asking_json(serial: u64, a: &Asking) -> Value {
    json!({
        "serial": serial,
        "title": a.title,
        "detail": a.detail,
        "options": a.options,
        "fields": a.fields,
        "is_form": a.is_form,
        "writable": a.writable,
        "summary": a.summary,
        "blank": a.blank,
    })
}


/// Base64 as clients send it: line breaks and missing padding are fine (Android's default
/// breaks lines; GLib, which read it before, took both).
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let clean: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    BASE64_NOPAD.decode(clean.trim_end_matches('=').as_bytes()).ok()
}

/// A sent file's name, safe as a file name here (no folders, nothing hidden).
pub fn safe_name(name: &str) -> String {
    let name: String = name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
    let name = name.trim_start_matches('.').trim();
    if name.is_empty() { "file".to_owned() } else { name.to_owned() }
}

// ---- the bridges as Settings shows them ----------------------------------------------------
//
// Each bridge's state is what it last said (the "state" request) or what loom knows of it
// (off, starting, failed): `{"state": "off" | "connecting" | "failed" | …, …}`. The Telegram
// bridge's is a `Link`.


thread_local! {
    static STATES: RefCell<HashMap<String, Value>> = RefCell::default();
    static WATCHERS: RefCell<Vec<(String, Box<dyn Fn(&Value) -> bool>)>> = RefCell::new(Vec::new());
}

/// Bridge `name`'s state now.
pub fn state_of(name: &str) -> Value {
    STATES.with(|s| s.borrow().get(name).cloned()).unwrap_or_else(|| json!({ "state": "off" }))
}

/// Calls `f` with bridge `name`'s state now and on every change, while it returns true.
pub fn watch(name: &str, f: impl Fn(&Value) -> bool + 'static) {
    if f(&state_of(name)) {
        WATCHERS.with(|w| w.borrow_mut().push((name.to_owned(), Box::new(f))));
    }
}

pub(crate) fn set_state(name: &str, state: Value) {
    if state_of(name) == state {
        return;
    }
    STATES.with(|s| s.borrow_mut().insert(name.to_owned(), state.clone()));
    super::api::publish_states();
    let watchers = WATCHERS.with(|w| std::mem::take(&mut *w.borrow_mut()));
    let kept: Vec<_> = watchers.into_iter().filter(|(of, f)| of != name || f(&state)).collect();
    WATCHERS.with(|w| {
        let mut w = w.borrow_mut();
        let added = std::mem::take(&mut *w);
        *w = kept;
        w.extend(added);
    });
}

#[cfg(test)]
mod tests {
    /// An event for the remote bridge reaches it as an item added (then taken away) to
    /// `bridge/remote`, which no one else may follow.
    #[test]
    fn a_bridges_events_come_on_its_own_resource() {
        use crate::api::{Body, Op, Principal};
        crate::rt::set_host(|_| {}, |_, _| {});
        super::super::api::serve();
        let hub = crate::api::hub();
        let phone = hub.connect(Principal::Bridge("remote".into()));
        let sub = phone.subscribe("bridge/remote", || {}).unwrap();
        sub.drain();
        assert!(hub.connect(Principal::Cli).subscribe("bridge/remote", || {}).is_err(), "the CLI doesn't read it");
        assert!(hub.connect(Principal::Bridge("telegram".into())).subscribe("bridge/remote", || {}).is_err(), "nor another bridge");
        super::emit(Some("remote"), serde_json::json!({ "event": "done", "agent": { "id": 1 } }));
        super::emit(Some("telegram"), serde_json::json!({ "event": "not for the phone" }));
        hub.store("bridge/remote").flush();
        let ops: Vec<Op> = sub.drain().into_iter().flat_map(|m| match m.body { Body::Changes { ops } => ops, _ => Vec::new() }).collect();
        let events: Vec<serde_json::Value> = ops.iter().filter_map(|op| match op { Op::Upsert { item } => Some(item["event"].clone()), _ => None }).collect();
        assert_eq!(events, [serde_json::json!({ "event": "done", "agent": { "id": 1 } })]);
        assert!(hub.store("bridge/remote").items().is_empty(), "a snapshot holds no old events");
    }

    #[test]
    fn a_bridge_finds_its_connection_on_fd_3() {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "echo hello >&3"]);
        super::hand_down(&mut command, &theirs);
        command.status().unwrap();
        drop(theirs);
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(ours), &mut line).unwrap();
        assert_eq!(line, "hello\n");
    }

    use super::*;

    #[test]
    fn a_question_reads_back_as_sent() {
        let asking = Asking {
            title: "Pick".into(),
            detail: Some("why".into()),
            options: vec![("Allow".into(), "allow_once".into())],
            fields: vec![("Colour".into(), vec!["Red".into(), "Blue".into()], true, vec![false, true])],
            is_form: true,
            writable: true,
            summary: "→ Colour: Blue".into(),
            blank: false,
        };
        let back = asking_from(&asking_json(7, &asking));
        assert_eq!(back.title, asking.title);
        assert_eq!(back.detail, asking.detail);
        assert_eq!(back.options, asking.options);
        assert_eq!(back.fields, asking.fields);
        assert_eq!((back.is_form, back.writable, back.blank), (true, true, false));
        assert_eq!(back.summary, asking.summary);
    }

    #[test]
    fn link_reads_back() {
        for link in [
            Link::Off,
            Link::Connecting,
            Link::Failed("no".into()),
            Link::Connected { bot: "b".into(), user: 4, code: Some("c".into()), candidate: Some((9, "@x".into())), problem: None },
        ] {
            assert_eq!(Link::from_json(&link.to_json()), link);
        }
    }

    #[test]
    fn base64_with_line_breaks_or_no_padding_reads() {
        assert_eq!(decode("aGVsbG8gd29y\nbGQ=\n").as_deref(), Some(&b"hello world"[..]));
        assert_eq!(decode("aGVsbG8gd29ybGQ").as_deref(), Some(&b"hello world"[..]));
        assert_eq!(decode("not base64!"), None);
    }

    #[test]
    fn sent_file_names_stay_in_their_folder() {
        assert_eq!(safe_name("../../.bashrc"), "_.._.bashrc");
        assert_eq!(safe_name(".env"), "env");
        assert_eq!(safe_name("report final.pdf"), "report final.pdf");
        assert_eq!(safe_name(""), "file");
    }
}

/// How much of a file a bridge reads (`file`): enough for code, not a whole log.
const FILE_READ: u64 = 512 * 1024;

/// A file as text for `file`: `{text, size, truncated}`, or `{binary, size}` when it isn't text.
pub(crate) fn read_text(path: &std::path::Path) -> Result<Value, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| "It is no longer there.".to_owned())?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut bytes = Vec::new();
    file.take(FILE_READ).read_to_end(&mut bytes).map_err(|e| format!("Could not read it: {e}"))?;
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return Ok(json!({ "binary": true, "size": size }));
    }
    Ok(json!({ "text": String::from_utf8_lossy(&bytes), "size": size, "truncated": size > FILE_READ }))
}

#[cfg(test)]
mod read_text_tests {
    use super::*;

    #[test]
    fn text_binary_and_long_files_read_as_said() {
        let dir = std::env::temp_dir().join(format!("loom-read-text-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("a.rs");
        std::fs::write(&text, "fn main() {}\n").unwrap();
        assert_eq!(read_text(&text).unwrap(), json!({ "text": "fn main() {}\n", "size": 13, "truncated": false }));
        let binary = dir.join("b.png");
        std::fs::write(&binary, [0x89, b'P', b'N', b'G', 0, 1, 2]).unwrap();
        assert_eq!(read_text(&binary).unwrap()["binary"], json!(true));
        let long = dir.join("c.log");
        std::fs::write(&long, "x".repeat(FILE_READ as usize + 10)).unwrap();
        let read = read_text(&long).unwrap();
        assert_eq!(read["truncated"], json!(true));
        assert_eq!(read["text"].as_str().unwrap().len() as u64, FILE_READ);
        assert!(read_text(&dir.join("gone")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
