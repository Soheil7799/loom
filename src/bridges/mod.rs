//! Bridges: how something outside loom reaches it (the Telegram bot, your other devices
//! through the remote bridge, or whatever someone writes). Providers are what loom drives; bridges are what drive loom.
//!
//! Each bridge is a process of its own (`loom bridge <name>`), started by loom while the
//! settings switch it on and stopped with it (its stdin closes when loom goes). It talks to
//! loom over the bridge socket, which is not the agents' control socket: every request carries
//! the secret loom gave that bridge on its stdin when starting it (never on disk, in the
//! environment or the arguments), so an agent can't answer its own permission requests.
//!
//! One JSON object per line:
//! - `{"secret", "method", "params"}` → `{"ok": …}` or `{"error": "…"}`, one request a connection;
//! - `{"secret", "method": "subscribe"}`: the connection stays open and loom writes events on it,
//!   `{"event": "agent" | "asked" | "done" | "unsent" | "failed" | "closed" | "settings", …}`.
//!
//! What a bridge can do is the methods in `Host::handle`, nothing else.

pub use loom_core::bridges::{Link, random, telegram};
#[cfg(test)]
use loom_core::bridges::asking_from;
use loom_core::bridges::random_hex;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gtk::{gdk, gio, glib, prelude::*};
use serde_json::{Value, json};

use crate::chat_view::AgentChat;
use crate::agent::{Agent, Asking, Event};
use crate::app::App;
use crate::settings::Settings;

/// The bridges loom has: their names (`loom bridge <name>`).
const BRIDGES: &[&str] = &["telegram", "remote"];

/// How long a pairing code shown in Settings stays good.
const PAIRING: Duration = Duration::from_secs(600);

/// The running bridges' secrets: (secret, bridge).
static SECRETS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// The subscribed connections: (bridge, where its events go).
static SUBSCRIBERS: Mutex<Vec<(String, async_channel::Sender<String>)>> = Mutex::new(Vec::new());

struct Request {
    bridge: String,
    method: String,
    params: Value,
    reply: async_channel::Sender<Value>,
}

struct Running {
    process: gio::Subprocess,
    secret: String,
    started: Instant,
    /// What it was started with (a change to some of it starts it again).
    settings: Value,
}

pub struct Host {
    app: RefCell<Weak<App>>,
    app_id: String,
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
}

thread_local! {
    static HOST: RefCell<Weak<Host>> = RefCell::new(Weak::new());
}

/// The bridge socket of this app id, beside the control socket.
pub fn socket_path(app_id: &str) -> PathBuf {
    let control = crate::control::socket_path(app_id);
    let stem = control.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "loom".to_owned());
    control.with_file_name(format!("{stem}-bridges.sock"))
}


/// The remote bridge's program, `loom-remote` beside loom (it brings iroh; loom doesn't).
pub fn remote_program() -> Option<PathBuf> {
    let program = std::env::current_exe().ok()?.with_file_name("loom-remote");
    program.is_file().then_some(program)
}

/// Opens pairing (a new code for Settings to show) or closes it.
pub fn pair(open: bool) {
    let Some(host) = HOST.with(|h| h.borrow().upgrade()) else { return };
    let code = open.then(|| random(12));
    *host.pairing.borrow_mut() = code.clone();
    host.apply(&crate::settings::get());
    if let Some(code) = code {
        let weak = Rc::downgrade(&host);
        glib::timeout_add_local_once(PAIRING, move || {
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
            app: RefCell::new(Weak::new()),
            app_id: app_id.to_owned(),
            socket: socket_path(app_id),
            running: RefCell::default(),
            failures: RefCell::default(),
            told: RefCell::default(),
            seq: Cell::new(0),
            pairing: RefCell::new(None),
        })
    }

    /// Listens on the bridge socket, then starts the bridges the settings switch on.
    pub fn attach(self: &Rc<Self>, app: &Rc<App>) {
        *self.app.borrow_mut() = Rc::downgrade(app);
        HOST.with(|h| *h.borrow_mut() = Rc::downgrade(self));
        self.listen();
        self.apply(&crate::settings::get());
    }

    fn listen(self: &Rc<Self>) {
        if let Some(dir) = self.socket.parent() {
            let _ = std::fs::create_dir_all(dir);
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let _ = std::fs::remove_file(&self.socket);
        let listener = match UnixListener::bind(&self.socket) {
            Ok(listener) => listener,
            Err(err) => {
                eprintln!("loom: no bridge socket at {}: {err}", self.socket.display());
                return;
            }
        };
        let (tx, rx) = async_channel::unbounded::<Request>();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let tx = tx.clone();
                        std::thread::spawn(move || connection(stream, tx));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(request) = rx.recv().await {
                let Some(host) = weak.upgrade() else { break };
                host.handle(&request.bridge, &request.method, &request.params, request.reply);
            }
        });
    }

    /// Starts, stops or tells each bridge as the settings say.
    pub fn apply(self: &Rc<Self>, s: &Settings) {
        // The remote bridge's key, made the first time it is switched on (saved, then applied).
        if s.remote_enabled && s.remote_key.is_empty()
            && let Some(key) = random_hex(32)
        {
            glib::idle_add_local_once(move || crate::settings::update(|s| s.remote_key = key));
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

    fn start(self: &Rc<Self>, name: &'static str) {
        let Some(settings) = self.settings_for(name, &crate::settings::get()) else { return };
        let secret = random(32);
        let Ok(exe) = std::env::current_exe() else { return };
        let process = match gio::Subprocess::newv(&[exe.as_os_str(), "bridge".as_ref(), name.as_ref()], gio::SubprocessFlags::STDIN_PIPE) {
            Ok(process) => process,
            Err(err) => {
                crate::log::debug!("bridges", "{name} did not start: {err}");
                set_state(name, json!({ "state": "failed", "why": format!("The {} bridge did not start: {err}", title(name)) }));
                return;
            }
        };
        // Known before the bridge can use it.
        SECRETS.lock().unwrap().push((secret.clone(), name.to_owned()));
        let hello = json!({ "socket": self.socket, "secret": secret, "app": self.app_id }).to_string() + "\n";
        if let Some(stdin) = process.stdin_pipe() {
            let _ = stdin.write_all(hello.as_bytes(), None::<&gio::Cancellable>);
        }
        crate::log::debug!("bridges", "{name} started (pid {:?})", process.identifier());
        set_state(name, json!({ "state": "connecting" }));
        let weak = Rc::downgrade(self);
        let (waited, mine) = (process.clone(), secret.clone());
        glib::spawn_future_local(async move {
            let _ = waited.wait_future().await;
            let Some(host) = weak.upgrade() else { return };
            host.ended(name, &mine);
        });
        self.running.borrow_mut().insert(name, Running { process, secret, started: Instant::now(), settings });
    }

    fn stop(&self, name: &str) {
        if let Some(running) = self.running.borrow_mut().remove(name) {
            forget_secret(&running.secret);
            running.process.force_exit();
        }
        self.failures.borrow_mut().remove(name);
    }

    /// Bridge `name` (the run with `secret`) exited: started again if still wanted, a bit later
    /// each time it keeps failing.
    fn ended(self: &Rc<Self>, name: &'static str, secret: &str) {
        forget_secret(secret);
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
        glib::timeout_add_seconds_local_once(wait as u32, move || {
            let Some(host) = weak.upgrade() else { return };
            if !host.running.borrow().contains_key(name) && host.settings_for(name, &crate::settings::get()).is_some() {
                host.start(name);
            }
        });
    }

    /// An agent as bridges see it.
    fn view(&self, app: &App, agent: &Agent) -> Value {
        self.seq.set(self.seq.get() + 1);
        let questions: Vec<Value> = agent.open_questions().into_iter().filter_map(|serial| agent.asking(serial).map(|a| asking_json(serial, &a))).collect();
        json!({
            "id": agent.id,
            "seq": self.seq.get(),
            "title": agent.title(),
            "project": app.project_label(&agent.project),
            "status": agent.status().css(),
            "unseen": agent.unseen.get(),
            "busy": agent.is_busy(),
            "questions": questions,
        })
    }

    /// What happened to an agent: told to the bridges. `looking`: the user has it in front of
    /// them in loom (a bridge then stays quiet).
    pub fn agent_event(&self, agent: &Agent, event: &Event, looking: bool) {
        if !subscribed() {
            return;
        }
        let Some(app) = self.app.borrow().upgrade() else { return };
        let view = self.view(&app, agent);
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
            Event::Done => {
                told["event"] = json!("done");
                told["reply"] = json!(agent.chat().last_reply());
                told["took"] = json!(crate::chat::elapsed(agent.last_took.get()));
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
        emit(None, told);
    }

    /// An agent closed.
    pub fn closed(&self, agent: u32) {
        self.told.borrow_mut().remove(&agent);
        emit(None, json!({ "event": "closed", "agent": agent }));
    }

    /// A request from bridge `bridge`, answered on `reply` (now, or when it is done).
    fn handle(&self, bridge: &str, method: &str, p: &Value, reply: async_channel::Sender<Value>) {
        let answer = |result: Result<Value, String>| {
            let _ = reply.try_send(match result {
                Ok(value) => json!({ "ok": value }),
                Err(error) => json!({ "error": error }),
            });
        };
        let Some(app) = self.app.borrow().upgrade() else { return answer(Err("loom is closing".into())) };
        let agent = || {
            let id = p["agent"].as_u64().unwrap_or(0) as u32;
            app.agents_listed().into_iter().find(|a| a.id == id).ok_or_else(|| "That agent is closed.".to_owned())
        };
        let serial = p["serial"].as_u64().unwrap_or(0);
        let agents = || Value::Array(app.agents_listed().iter().map(|a| self.view(&app, a)).collect());
        crate::log::debug!("bridges", "{bridge} → {method}");
        let result = match method {
            "hello" => Ok(json!({ "settings": self.settings_for(bridge, &crate::settings::get()), "agents": agents() })),
            "agents" => Ok(agents()),
            // A message: pictures (any format GTK reads) go as pasted ones. A shell command
            // (`!…`) runs only from loom's own window, never from a bridge.
            "send" if p["text"].as_str().unwrap_or("").trim_start().starts_with('!') => Err("! commands run only from loom itself.".to_owned()),
            "send" => agent().and_then(|agent| {
                let mut images = Vec::new();
                for data in p["images"].as_array().into_iter().flatten() {
                    let bytes = glib::base64_decode(data.as_str().unwrap_or(""));
                    let png = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).map_err(|_| "That picture could not be read.".to_owned())?;
                    images.push(png.save_to_png_bytes().to_vec());
                }
                agent.submit_with(p["text"].as_str().unwrap_or("").to_owned(), images);
                Ok(self.view(&app, &agent))
            }),
            // A file for the agent: saved in its folder, under <bridge>-files/. Its path back.
            "save" => agent().and_then(|agent| {
                let dir = agent.cwd.join(format!("{bridge}-files"));
                let target = crate::files::free_name(&dir.join(safe_name(p["name"].as_str().unwrap_or(""))));
                let bytes = glib::base64_decode(p["data"].as_str().unwrap_or(""));
                std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&target, &bytes)).map_err(|e| format!("Could not save it in the agent's folder: {e}"))?;
                Ok(json!(target.to_string_lossy()))
            }),
            // "Always allow" is a decision made at the desk, never from a bridge.
            "answer" => agent().and_then(|agent| {
                let option = p["option"].as_u64().unwrap_or(0) as usize;
                let kind = agent.asking(serial).and_then(|a| a.options.get(option).map(|o| o.1.clone()));
                if kind.as_deref() == Some("allow_always") {
                    return Err("Not from outside loom.".to_owned());
                }
                let answered = agent.answer_serial(serial, option);
                Ok(json!({ "answered": answered, "agent": self.view(&app, &agent) }))
            }),
            "form" => agent().and_then(|agent| {
                let (done, problem) = match p["do"].as_str().unwrap_or("") {
                    "pick" => (agent.pick_in_form(serial, p["field"].as_u64().unwrap_or(0) as usize, p["option"].as_u64().unwrap_or(0) as usize), None),
                    "write" => (agent.write_in_form(serial, p["text"].as_str().unwrap_or("")), None),
                    "send" => agent.send_form_serial(serial).map_or_else(|e| (false, Some(e)), |()| (true, None)),
                    "skip" => agent.skip_form(serial).map_or_else(|e| (false, Some(e)), |()| (true, None)),
                    other => return Err(format!("no form action {other}")),
                };
                Ok(json!({ "done": done, "problem": problem, "agent": self.view(&app, &agent) }))
            }),
            "stop" => agent().map(|agent| {
                agent.cancel();
                self.view(&app, &agent)
            }),
            "last_reply" => agent().map(|agent| json!(agent.chat().last_reply())),
            // Speech to text with loom's settings (its key never leaves loom); answered when heard.
            "transcribe" => {
                if !crate::stt::configured() {
                    return answer(Err("Speech to text is not set up in loom.".into()));
                }
                let audio = glib::base64_decode(p["audio"].as_str().unwrap_or(""));
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

fn forget_secret(secret: &str) {
    SECRETS.lock().unwrap().retain(|(s, _)| s != secret);
}

fn subscribed() -> bool {
    !SUBSCRIBERS.lock().unwrap().is_empty()
}

/// An event to every subscribed bridge (`to`: only that one).
fn emit(to: Option<&str>, event: Value) {
    let line = event.to_string();
    SUBSCRIBERS.lock().unwrap().retain(|(bridge, tx)| to.is_some_and(|to| to != bridge) || tx.try_send(line.clone()).is_ok());
}

fn connection(stream: UnixStream, tx: async_channel::Sender<Request>) {
    let Ok(mut writer) = stream.try_clone() else { return };
    let mut line = String::new();
    if BufReader::new(stream).read_line(&mut line).is_err() {
        return;
    }
    let request: Value = match serde_json::from_str(&line) {
        Ok(request) => request,
        Err(err) => {
            let _ = writeln!(writer, "{}", json!({ "error": format!("bad request: {err}") }));
            return;
        }
    };
    let secret = request["secret"].as_str().unwrap_or("");
    let bridge = SECRETS.lock().unwrap().iter().find(|(s, _)| !secret.is_empty() && s == secret).map(|(_, b)| b.clone());
    let Some(bridge) = bridge else {
        let _ = writeln!(writer, "{}", json!({ "error": "not a bridge of this loom" }));
        return;
    };
    let method = request["method"].as_str().unwrap_or("").to_owned();
    if method == "subscribe" {
        let (events, rx) = async_channel::unbounded::<String>();
        SUBSCRIBERS.lock().unwrap().push((bridge, events));
        while let Ok(event) = rx.recv_blocking() {
            if writeln!(writer, "{event}").is_err() {
                return;
            }
        }
        return;
    }
    let (reply, answer) = async_channel::bounded(1);
    if tx.send_blocking(Request { bridge, method, params: request["params"].clone(), reply }).is_err() {
        return;
    }
    let answer = answer.recv_blocking().unwrap_or_else(|_| json!({ "error": "loom is closing" }));
    let _ = writeln!(writer, "{answer}");
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


/// A sent file's name, safe as a file name here (no folders, nothing hidden).
fn safe_name(name: &str) -> String {
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

fn set_state(name: &str, state: Value) {
    if state_of(name) == state {
        return;
    }
    STATES.with(|s| s.borrow_mut().insert(name.to_owned(), state.clone()));
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
    fn sent_file_names_stay_in_their_folder() {
        assert_eq!(safe_name("../../.bashrc"), "_.._.bashrc");
        assert_eq!(safe_name(".env"), "env");
        assert_eq!(safe_name("report final.pdf"), "report final.pdf");
        assert_eq!(safe_name(""), "file");
    }
}
