//! `loomd`: loom's core as its own program, with no window. The agents (restored as loom
//! left them), their jobs, the bridges and the core API run here; every face of loom is a
//! client of it: the web page (`loom api web --open`), the phone, Telegram, the CLI.
//!
//! `loomd --web` says, on its stderr only, a link to the web page with the window's rights
//! (answering agents' questions); `loom api web` gives the CLI's.
//!
//! One core per app id: loomd doesn't start while a loom of its id answers, and loom's window
//! doesn't while loomd does. It ends on SIGTERM or Ctrl+C, saving the workspace and stopping
//! the agents and their jobs, as the window does when it closes.
//!
//! What a window shows the user, a UI shows instead: the files agents want seen or filled in
//! are the API's `asks` (`ask_done`, `ask_open`); the Notes chat's chats are opened with
//! `notes_open`; Scribe works as in the window (its lists: the `scribe` resource).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

mod notes;

use loom_core::agent::{Agent, Event};
use loom_core::api::{Error, Principal};
use loom_core::jobs::Jobs;
use loom_core::state::{self, Saved};
use loom_core::workspace::{Host, Workspace};
use serde_json::json;

const APP_ID: &str = "dev.shirvani.Loom";
/// The exit code when another core of the app id runs (the service isn't restarted for it).
const ANOTHER_CORE: u8 = 3;

/// What hosts the core here: the agents' parts, and what happens to them.
struct Daemon {
    app_id: String,
    ws: Rc<Workspace>,
    bridges: Rc<loom_core::bridges::Host>,
    scribe: Rc<loom_core::hub::Hub>,
    notes: Rc<notes::Notes>,
    /// Each project's jobs.
    jobs: RefCell<HashMap<PathBuf, Jobs>>,
    /// The window's size as loom last saved it (kept for when the window comes back).
    size: (i32, i32, bool),
    /// A save is due shortly (several changes, one save).
    saving: std::cell::Cell<bool>,
    /// `agents` is due to be published again, once the core's thread is idle.
    publishing: std::cell::Cell<bool>,
    me: Weak<Daemon>,
}

impl Daemon {
    fn save(&self) {
        state::save(&self.app_id, &self.ws.layout(None, None, self.size));
    }

    /// `agents` made what the agents are now (statuses change by `Changed` events alone).
    fn publish_soon(&self) {
        if self.publishing.replace(true) {
            return;
        }
        let me = self.me.clone();
        loom_core::rt::idle_once(move || {
            if let Some(daemon) = me.upgrade() {
                daemon.publishing.set(false);
                daemon.ws.publish_agents();
            }
        });
    }

    fn save_soon(&self) {
        if self.saving.replace(true) {
            return;
        }
        let me = self.me.clone();
        let _ = loom_core::rt::timeout_once(Duration::from_secs(2), move || {
            if let Some(daemon) = me.upgrade() {
                daemon.saving.set(false);
                daemon.save();
            }
        });
    }

    /// Closes `agent`, as the window does: it stops, and its conversation can be reopened.
    fn close(&self, agent: &Rc<Agent>) {
        // Its last words into the notes too.
        if let Some(session) = agent.session_id() {
            self.scribe.check_now(&session, true);
        }
        self.bridges.closed(agent.id);
        self.ws.close_agent(agent);
        self.save_soon();
    }

    /// What an agent's loom tools ask (`tool`): what needs no window, done; the rest said so.
    fn tool(&self, agent: u32, name: &str, params: &serde_json::Value) -> Result<serde_json::Value, String> {
        if let Some(answer) = self.ws.answer(agent, name, params) {
            return answer;
        }
        // What the window would show: an ask, for a UI to show (and a word on the desktop).
        let found = self.ws.agent(agent).or_else(|| self.notes.agents().into_iter().find(|a| a.id == agent));
        let found = found.ok_or("this agent is no longer open in loom")?;
        let asked = loom_core::asks::tool(&found, name, params).ok_or_else(|| format!("unknown request {name}"))?;
        if asked.is_ok() && matches!(name, "request_edit" | "notify" | "open_file") {
            let what = match name {
                "request_edit" => "waits for you to fill in a file",
                "open_file" => "wants you to see a file",
                _ => params["message"].as_str().unwrap_or("needs you"),
            };
            desktop(&found.title(), what);
        }
        asked
    }
}

impl Host for Daemon {
    fn agent_parts(&self, project: &Path) -> (Jobs, Rc<dyn loom_core::agent::view::View>) {
        let jobs = self
            .jobs
            .borrow_mut()
            .entry(project.to_path_buf())
            .or_insert_with(|| {
                let (jobs, events) = Jobs::new(project.to_path_buf());
                // In the API (`jobs`, `job/<id>`); nobody here reads the events again.
                drop(loom_core::jobs_api::published(&jobs, events));
                jobs
            })
            .clone();
        (jobs, loom_core::agent::headless::Headless::new())
    }

    fn agent_added(&self, agent: &Rc<Agent>) {
        // Its conversation readable while it rests (the window replays when it starts).
        agent.read_back_when_followed();
        self.save_soon();
    }

    fn agent_event(&self, agent: &Rc<Agent>, event: Event) {
        // Nobody looks at it in a window: the bridges always may tell.
        self.bridges.agent_event(agent, &event, false);
        // A copy (rewind, fork) opens beside it, as in the window.
        if let Event::Forked(saved) = &event {
            self.ws.add_agent(saved.clone());
        }
        // Scribe reads conversations as in the window: quiet after a turn, before compacting.
        if let Some(session) = agent.session_id() {
            match event {
                Event::Done => self.scribe.turn_done(&session),
                Event::Compacting => self.scribe.check_now(&session, false),
                _ => {}
            }
        }
        if let Event::Asked(title) = &event
            && loom_core::settings::get().notify_questions
        {
            desktop(&agent.title(), &format!("Wants to: {title}"));
        }
        if matches!(event, Event::Done) && loom_core::settings::get().notify_done {
            desktop(&agent.title(), "Finished");
        }
        self.publish_soon();
        if !matches!(event, Event::Changed) {
            self.save_soon();
        }
    }

    fn notify(&self, agent: Option<&Rc<Agent>>, title: &str, body: &str) {
        match agent {
            Some(agent) => desktop(&agent.title(), body),
            None => desktop(title, body),
        }
    }

    fn schedules_changed(&self) {
        self.ws.publish_schedules();
        self.save_soon();
    }
}

/// A word on the desktop (`notify-send`, where there is one), and in loomd's output.
fn desktop(title: &str, body: &str) {
    eprintln!("loomd: {title}: {body}");
    if std::env::var_os("LOOM_TEST_INSTANCE").is_some() {
        return;
    }
    let spawned = std::process::Command::new("notify-send")
        .args(["--app-name=loom", title, body])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    // Waited for, off this thread (no zombie left).
    if let Ok(mut child) = spawned {
        std::thread::spawn(move || child.wait());
    }
}

/// Pictures sent in, any of the formats phones send (PNG, JPEG, WebP, GIF), as PNG.
fn to_png(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(bytes);
    }
    let picture = image::load_from_memory(&bytes).map_err(|e| format!("not a picture loomd reads: {e}"))?;
    let mut png = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(png.into_inner())
}

/// Scribe with no panel: what it does is in the `scribe` resource.
struct Unseen;

impl loom_core::hub::HubView for Unseen {
    fn refresh(&self) {}
    fn refresh_head(&self) {}
    fn status(&self, _text: &str) {}
    fn say(&self, text: &str, error: bool) {
        if error {
            eprintln!("loomd: Scribe: {text}");
        }
    }
    fn attention(&self) {}
    fn history_changed(&self) {}
}

fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("loomd: {err}");
            return std::process::ExitCode::from(1);
        }
    };
    let local = tokio::task::LocalSet::new();
    std::process::ExitCode::from(local.block_on(&runtime, run()))
}

async fn run() -> u8 {
    // The core's thread: this one, in tokio's local set.
    loom_core::rt::set_host(
        |future| {
            tokio::task::spawn_local(future);
        },
        |after, f| {
            tokio::task::spawn_local(async move {
                tokio::time::sleep(after).await;
                f();
            });
        },
    );
    loom_core::api::set_host_kind("daemon");
    let app_id = std::env::var("LOOM_APP_ID").unwrap_or_else(|_| APP_ID.to_owned());
    let socket = loom_core::api::socket::socket_path(&app_id);
    if let Some(host) = loom_core::api::socket::running(&socket) {
        eprintln!("loomd: a loom ({host}) is running for {app_id} already: one core at a time");
        return ANOTHER_CORE;
    }
    loom_core::settings::init(&app_id);
    loom_core::audit::init(&app_id);
    // As the window starts its core: the log, loom's plugin for the agents (its skills and
    // tools), the journal, the vault.
    loom_core::log::name_for(&app_id);
    loom_core::log::install();
    loom_core::log::set(loom_core::settings::get().debug_log);
    loom_core::plugin::init(&app_id);
    loom_core::journal::init(&app_id);
    if let Some(vault) = loom_core::vault::current()
        && let Err(err) = loom_core::vault::prepare(&vault)
    {
        loom_core::log::write("notes", &format!("vault {}: {err}", vault.display()));
    }
    loom_core::asks::serve();
    loom_core::library::serve();
    if loom_core::paths::loom_program().is_none() {
        eprintln!("loomd: no `loom` program beside loomd: agents will have no loom tools, and the bridges won't start");
    }
    loom_core::notes_api::serve();
    loom_core::api::blobs::serve();
    loom_core::agent::commands::serve();
    loom_core::jobs_api::serve();
    loom_core::bridges::api::serve();
    loom_core::api::web::serve();
    loom_core::scope::probe();

    let ws = Workspace::new(&app_id);
    let size = {
        let layout = state::load(&app_id);
        (layout.width, layout.height, layout.maximized)
    };
    let bridges = loom_core::bridges::Host::new(&app_id);
    let scribe = loom_core::hub::Hub::new();
    let scribe_view: Rc<dyn loom_core::hub::HubView> = Rc::new(Unseen);
    scribe.set_view(Rc::downgrade(&scribe_view));
    scribe.serve();
    let notes = notes::Notes::new(scribe.clone(), bridges.clone());
    notes::commands(&notes);
    bridges.set_notes(Rc::new(notes::Remote(Rc::downgrade(&notes))));
    let daemon = Rc::new_cyclic(|me| Daemon {
        app_id: app_id.clone(),
        ws: ws.clone(),
        bridges,
        scribe,
        notes: notes.clone(),
        jobs: RefCell::default(),
        size,
        saving: std::cell::Cell::new(false),
        publishing: std::cell::Cell::new(false),
        me: me.clone(),
    });
    let host: Rc<dyn Host> = daemon.clone();
    ws.set_host(Rc::downgrade(&host));
    commands(&daemon);

    // The API first: the agents are told where it is, the bridges are handed connections.
    // The agent a process is under: the workspace's, or a Notes chat's.
    let owner = {
        let (ws, notes) = (Rc::downgrade(&ws), Rc::downgrade(&notes));
        move |pid| {
            let id = ws.upgrade()?.agent_of_process(pid).or_else(|| notes.upgrade()?.agents().iter().find(|a| a.adapter_pid() == Some(pid)).map(|a| a.id))?;
            Some(Principal::Agent(id.to_string()))
        }
    };
    if let Err(err) = loom_core::api::socket::serve(&socket, owner) {
        eprintln!("loomd: no API socket at {}: {err}", socket.display());
        return 1;
    }
    loom_core::control::set_current(socket.clone());
    ws.restore();
    ws.publish_agents();
    ws.serve_schedules();
    ws.serve_conversations();
    // Pictures sent in: any format phones send, as PNG.
    daemon.bridges.attach(&ws, std::sync::Arc::new(to_png));
    notes.refresh();
    let bridges = daemon.bridges.clone();
    loom_core::settings::on_change(move |s| bridges.apply(s));
    eprintln!("loomd: {} agents, API at {}", ws.agents.borrow().len(), socket.display());
    // `--web`: a link to the web page with the window's rights (it answers agents' questions),
    // said here only, to whoever started loomd (not through the API: what passes for the CLI
    // gets the CLI's).
    if std::env::args().any(|a| a == "--web") {
        match loom_core::api::web::open(Principal::Window, 0) {
            Ok(url) => eprintln!("loomd: loom's page, as its window (one use, within two minutes): {url}"),
            Err(why) => eprintln!("loomd: no web page: {why}"),
        }
    }

    // Each minute: idle agents suspended (as Settings says; nobody looks at one here), and
    // `stalled` brought up to date.
    let ticking = ws.clone();
    let _tick = loom_core::rt::every(Duration::from_secs(60), move || {
        ticking.suspend_idle(|_| false);
        ticking.publish_agents();
        true
    });
    // Until told to stop.
    let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(term) => term,
        Err(err) => {
            eprintln!("loomd: {err}");
            return 1;
        }
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    eprintln!("loomd: stopping");
    // Stopped first, saved after: what waited to be sent goes back into each agent's draft,
    // and is saved with it (as the window does).
    for agent in ws.agents.borrow().iter() {
        agent.stop();
    }
    notes.stop_all();
    daemon.save();
    drop(scribe_view);
    for jobs in daemon.jobs.borrow().values() {
        jobs.stop_all();
    }
    loom_core::api::web::close();
    let _ = std::fs::remove_file(&socket);
    0
}

/// The commands only a host gives: agents started and closed (by people), and agents' tools.
fn commands(daemon: &Rc<Daemon>) {
    let hub = loom_core::api::hub();
    let human = |who: &Principal| matches!(who, Principal::Window | Principal::Cli);
    let weak = Rc::downgrade(daemon);
    hub.handle("new_agent", move |who, params| {
        if !human(who) {
            return Err(Error::Forbidden("starting agents".into()));
        }
        let daemon = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
        let folder = params["folder"].as_str().map(|f| state::expand(Path::new(f))).ok_or_else(|| Error::Invalid("new_agent needs a folder".into()))?;
        if !folder.is_absolute() || !folder.is_dir() {
            return Err(Error::Invalid(format!("{} is not a folder", folder.display())));
        }
        let agent = daemon.ws.add_agent(Saved::new(folder));
        daemon.ws.publish_agents();
        Ok(json!({ "agent": agent.id.to_string() }))
    });
    let weak = Rc::downgrade(daemon);
    hub.handle("close_agent", move |who, params| {
        if !human(who) {
            return Err(Error::Forbidden("closing agents".into()));
        }
        let daemon = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
        let id = params["agent"].as_str().ok_or_else(|| Error::Invalid("which agent?".into()))?;
        let agent = id.parse().ok().and_then(|id| daemon.ws.agent(id)).ok_or_else(|| Error::NotFound(format!("agent {id}")))?;
        daemon.close(&agent);
        Ok(json!({}))
    });
    for name in ["rename_project", "set_collapsed"] {
        let weak = Rc::downgrade(daemon);
        hub.handle(name, move |who, params| {
            if !human(who) {
                return Err(Error::Forbidden("changing projects".into()));
            }
            let daemon = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
            daemon.ws.change_project(params)?;
            daemon.save_soon();
            Ok(json!({}))
        });
    }
    let weak = Rc::downgrade(daemon);
    hub.handle("remove_project", move |who, params| {
        if !human(who) {
            return Err(Error::Forbidden("removing projects".into()));
        }
        let daemon = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
        let (path, agents) = daemon.ws.project_to_remove(params)?;
        for agent in &agents {
            daemon.close(agent);
        }
        daemon.ws.projects.borrow_mut().retain(|p| p.path != path);
        daemon.ws.publish_agents();
        daemon.save_soon();
        Ok(json!({}))
    });
    let weak = Rc::downgrade(daemon);
    hub.handle("tool", move |who, params| {
        let Principal::Agent(id) = who else { return Err(Error::Forbidden("an agent's tools, but for agents".into())) };
        let id: u32 = id.parse().map_err(|_| Error::Forbidden("an agent's tools, but for agents loom started".into()))?;
        let daemon = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
        let name = params["name"].as_str().ok_or_else(|| Error::Invalid("which tool?".into()))?;
        daemon.tool(id, name, &params["params"]).map_err(Error::Failed)
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_jpeg_from_a_phone_becomes_a_png() {
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(4, 3, image::Rgb([200, 30, 30])).write_to(&mut jpeg, image::ImageFormat::Jpeg).unwrap();
        let png = super::to_png(jpeg.into_inner()).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(super::to_png(b"not a picture".to_vec()).is_err());
    }
}
