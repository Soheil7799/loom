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
    /// What loom's window last said it shows (`set_looking`): whether it is in use, the agents
    /// in view, and when it said so.
    looking: RefCell<(bool, Vec<u32>, std::time::Instant)>,
    /// `loomd_stop`: told to stop, as by SIGTERM (the window restarting an older loomd).
    stop: std::sync::Arc<tokio::sync::Notify>,
    me: Weak<Daemon>,
}

/// How long the window's `set_looking` holds: it says it again more often, so a window that
/// crashed keeps nothing quiet for longer than this.
const LOOKING_FOR: std::time::Duration = std::time::Duration::from_secs(60);

impl Daemon {
    /// Loom's window is in use (it shows its own notices then, not the desktop's).
    fn window_active(&self) -> bool {
        let looking = self.looking.borrow();
        looking.0 && looking.2.elapsed() < LOOKING_FOR
    }

    /// The user has agent `id` in front of them, in loom's window.
    fn looking_at(&self, id: u32) -> bool {
        self.window_active() && self.looking.borrow().1.contains(&id)
    }

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
        if asked.is_ok() && matches!(name, "request_edit" | "notify" | "open_file" | "show_job") && !self.window_active() {
            let what = match name {
                "request_edit" => "waits for you to fill in a file",
                "show_job" => "waits for you to type in a job's terminal",
                "open_file" => "wants you to see a file",
                _ => params["message"].as_str().unwrap_or("needs you"),
            };
            desktop(&found.title(), what, Some(&found.project));
        }
        asked
    }
}

impl Host for Daemon {
    fn agent_jobs(&self, project: &Path) -> Jobs {
        self.jobs
            .borrow_mut()
            .entry(project.to_path_buf())
            .or_insert_with(|| {
                let (jobs, events) = Jobs::new(project.to_path_buf());
                // In the API (`jobs`, `job/<id>`); nobody here reads the events again.
                drop(loom_core::jobs_api::published(&jobs, events));
                jobs
            })
            .clone()
    }

    fn agent_added(&self, agent: &Rc<Agent>) {
        // Its conversation readable while it rests (the window replays when it starts).
        agent.read_back_when_followed();
        self.save_soon();
    }

    fn agent_event(&self, agent: &Rc<Agent>, event: Event) {
        let looking = self.looking_at(agent.id);
        // Done while the window shows it: seen there already.
        if looking && matches!(event, Event::Done) {
            agent.mark_seen();
        }
        self.bridges.agent_event(agent, &event, looking);
        // A copy (rewind, fork) opens beside it, as in the window.
        if let Event::Forked(saved) = &event {
            self.ws.add_agent(saved.clone());
        }
        // Scribe reads conversations as in the window: quiet after a turn, before compacting.
        if let Some(session) = agent.session_id() {
            match event {
                Event::TurnEnded => self.scribe.turn_done(&session),
                Event::Compacting => self.scribe.check_now(&session, false),
                _ => {}
            }
        }
        // As the window does: nothing about what the user looks at; a question the window
        // shows itself while in use.
        if let Event::Asked(title) = &event
            && loom_core::settings::get().notify_questions
            && !self.window_active()
        {
            desktop(&agent.title(), &format!("Wants to: {title}"), Some(&agent.project));
        }
        if matches!(event, Event::Done) && loom_core::settings::get().notify_done && !looking {
            desktop(&agent.title(), "Finished", Some(&agent.project));
        }
        self.publish_soon();
        if !matches!(event, Event::Changed) {
            self.save_soon();
        }
    }

    fn notify(&self, agent: Option<&Rc<Agent>>, title: &str, body: &str) {
        match agent {
            Some(agent) => desktop(&agent.title(), body, Some(&agent.project)),
            None => desktop(title, body, None),
        }
    }

    fn schedules_changed(&self) {
        self.ws.publish_schedules();
        self.save_soon();
    }
}

/// A word on the desktop (`notify-send`, where there is one), and in loomd's output; with the
/// icon of the project it is about.
fn desktop(title: &str, body: &str, project: Option<&std::path::Path>) {
    eprintln!("loomd: {title}: {body}");
    if std::env::var_os("LOOM_TEST_INSTANCE").is_some() {
        return;
    }
    let mut send = std::process::Command::new("notify-send");
    send.arg("--app-name=loom");
    // Notifications come in the app icon's colours: the dark fox (none the first time, while
    // its logo is looked for: never a wait on loomd's loop).
    if let Some(icon) = project.and_then(|p| loom_core::project_icon::ready(p, &loom_core::project_files::projects(), true)) {
        send.arg("--icon").arg(icon);
    }
    let spawned = send
        .args([title, body])
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
    loom_core::project_files::serve();
    loom_core::board::serve();
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
        looking: RefCell::new((false, Vec::new(), std::time::Instant::now())),
        stop: std::sync::Arc::default(),
        me: me.clone(),
    });
    let weak = Rc::downgrade(&daemon);
    *ws.looking.borrow_mut() = Some(Rc::new(move |agent| weak.upgrade().is_some_and(|d| d.looking_at(agent.id))));
    let host: Rc<dyn Host> = daemon.clone();
    ws.set_host(Rc::downgrade(&host));
    commands(&daemon);

    // The API first: the agents are told where it is, the bridges are handed connections.
    // The agent a process is under: the workspace's, or a Notes chat's.
    let owner = {
        let (ws, notes) = (Rc::downgrade(&ws), Rc::downgrade(&notes));
        move |pid| {
            let id = ws.upgrade()?.agent_of_process(pid).or_else(|| notes.upgrade()?.agents().iter().find(|a| a.claude_pid() == Some(pid)).map(|a| a.id))?;
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
    // Meaning search (Settings): Ollama started and the model pulled when it is on.
    loom_core::embed::serve();
    loom_core::browser::serve();
    // With the debug log on: memory every 3 minutes, loomd's and each agent's (read off the
    // main loop; what the window logged when its agents were its own).
    let (watched, last) = (Rc::downgrade(&daemon), Rc::new(std::cell::Cell::new(None::<u64>)));
    std::mem::forget(loom_core::rt::every(std::time::Duration::from_secs(180), move || {
        let Some(daemon) = watched.upgrade() else { return false };
        if loom_core::log::on() {
            let agents: Vec<loom_core::memory::AgentInfo> = daemon
                .ws
                .agents_in_order()
                .iter()
                .map(|a| (a.id, format!("{} · {}", daemon.ws.project_label(&a.project), a.title()), a.status().css().to_owned(), a.claude_pid()))
                .collect();
            let last = last.clone();
            loom_core::rt::spawn_local(async move {
                let previous = last.get();
                if let Ok((text, total)) = loom_core::rt::spawn_blocking(move || loom_core::memory::snapshot(&agents, previous)).await {
                    last.set(Some(total));
                    loom_core::log::write("memory", &text);
                }
            });
        }
        true
    }));
    let bridges = daemon.bridges.clone();
    loom_core::settings::on_change(move |s| bridges.apply(s));
    // Next-message suggestions switched on or off: the agents running are told (as the
    // window tells its own).
    let (agents, suggesting) = (Rc::downgrade(&ws), std::cell::Cell::new(loom_core::settings::get().suggest_prompts));
    loom_core::settings::on_change(move |s| {
        if suggesting.replace(s.suggest_prompts) != s.suggest_prompts
            && let Some(ws) = agents.upgrade()
        {
            for agent in ws.agents.borrow().iter() {
                agent.set_suggestions(s.suggest_prompts);
            }
        }
    });
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
    let stop = daemon.stop.clone();
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
        _ = stop.notified() => {}
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
    loom_core::browser::stop_all();
    loom_core::api::web::close();
    let _ = std::fs::remove_file(&socket);
    0
}

/// The commands only a host gives: agents started and closed (by people), and agents' tools.
fn commands(daemon: &Rc<Daemon>) {
    let hub = loom_core::api::hub();
    let human = |who: &Principal| matches!(who, Principal::Window | Principal::Cli);
    let weak = Rc::downgrade(daemon);
    hub.handle("loomd_stop", move |who, _| {
        if !matches!(who, Principal::Window) && !(*who == Principal::Cli && loom_core::api::test_instance()) {
            return Err(Error::Forbidden("stopping loomd: loom's window only".into()));
        }
        let daemon = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
        // Answered first: a moment for the answer to go out before loomd stops.
        let stop = daemon.stop.clone();
        let _ = loom_core::rt::timeout_once(Duration::from_millis(200), move || stop.notify_one());
        Ok(serde_json::json!({}))
    });
    let weak = Rc::downgrade(daemon);
    hub.handle("set_looking", move |who, params| {
        // (the CLI too in a test instance, so the end-to-end tests can say it)
        if !matches!(who, Principal::Window) && !(*who == Principal::Cli && loom_core::api::test_instance()) {
            return Err(Error::Forbidden("set_looking: loom's window only".into()));
        }
        let daemon = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
        let active = params["active"].as_bool().ok_or_else(|| Error::Invalid("set_looking needs active".into()))?;
        let agents: Vec<u32> = params["agents"].as_array().into_iter().flatten().filter_map(|a| a.as_str()?.parse().ok()).collect();
        let before = std::mem::replace(&mut *daemon.looking.borrow_mut(), (active, agents, std::time::Instant::now()));
        // What the window shows in use is seen: its dot goes. One just opened there is used
        // (the palette's and the overview's "last used" order).
        for id in daemon.looking.borrow().1.clone() {
            if let Some(agent) = daemon.ws.agent(id).filter(|_| daemon.looking_at(id)) {
                agent.mark_seen();
                if !(before.0 && before.1.contains(&id)) {
                    agent.mark_used();
                }
            }
        }
        daemon.publish_soon();
        Ok(serde_json::json!({}))
    });
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
    // An agent in a worktree of its own (worktrees.rs): git runs off loomd's thread.
    let weak = Rc::downgrade(daemon);
    hub.handle_later("new_worktree_agent", move |who, params, reply| {
        if !human(who) {
            return reply(Err(Error::Forbidden("starting agents".into())));
        }
        let project = match params["project"].as_str().map(|p| loom_core::project_files::inside(p, &loom_core::project_files::projects())) {
            Some(Ok(project)) => project,
            _ => return reply(Err(Error::Forbidden("only a project of the sidebar's".into()))),
        };
        let branch = params["branch"].as_str().unwrap_or_default().trim().to_owned();
        let weak = weak.clone();
        loom_core::rt::spawn_local(async move {
            let made = loom_core::rt::spawn_blocking(move || loom_core::worktrees::add(&project, &branch)).await.unwrap_or_else(|_| Err("git stopped".into()));
            let Some(daemon) = weak.upgrade() else { return reply(Err(Error::NotFound("loom".into()))) };
            reply(made.map_err(Error::Invalid).map(|dir| {
                let agent = daemon.ws.add_agent(Saved::new(dir));
                daemon.ws.publish_agents();
                json!({ "agent": agent.id.to_string() })
            }));
        });
    });
    for method in ["worktree_merge", "worktree_remove"] {
        let weak = Rc::downgrade(daemon);
        hub.handle_later(method, move |who, params, reply| {
            if !human(who) {
                return reply(Err(Error::Forbidden("an agent's worktree".into())));
            }
            let Some(daemon) = weak.upgrade() else { return reply(Err(Error::NotFound("loom".into()))) };
            let id = params["agent"].as_str().unwrap_or_default();
            let Some(agent) = id.parse().ok().and_then(|id| daemon.ws.agent(id)) else { return reply(Err(Error::NotFound(format!("agent {id}")))) };
            let dir = agent.cwd.clone();
            if loom_core::worktrees::made_from(&dir).is_none() {
                return reply(Err(Error::Invalid("it doesn't work in a worktree of its own".into())));
            }
            if method == "worktree_remove" && agent.is_busy() {
                return reply(Err(Error::Conflict("it is working: stop it first".into())));
            }
            let (weak, agent) = (weak.clone(), Rc::downgrade(&agent));
            loom_core::rt::spawn_local(async move {
                let done = loom_core::rt::spawn_blocking(move || if method == "worktree_merge" { loom_core::worktrees::merge(&dir) } else { loom_core::worktrees::remove(&dir) }).await.unwrap_or_else(|_| Err("git stopped".into()));
                // closed once its folder is gone (refused: it stays open, as it was)
                if let (true, Ok(_), Some(daemon), Some(agent)) = (method == "worktree_remove", &done, weak.upgrade(), agent.upgrade()) {
                    daemon.close(&agent);
                }
                reply(done.map(|said| json!({ "said": said })).map_err(Error::Invalid));
            });
        });
    }
    let weak = Rc::downgrade(daemon);
    hub.handle("board_work", move |who, params| {
        if !human(who) {
            return Err(Error::Forbidden("starting agents".into()));
        }
        let daemon = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
        let project = params["project"].as_str().ok_or_else(|| Error::Invalid("which project?".into()))?;
        let project = loom_core::project_files::inside(project, &loom_core::project_files::projects()).map_err(|_| Error::Forbidden("only a project of the sidebar's".into()))?;
        let id = params["card"].as_u64().ok_or_else(|| Error::Invalid("which card?".into()))? as u32;
        let board = loom_core::board::Board::load(&project);
        let card = board.cards().find(|c| c.id == id).ok_or_else(|| Error::NotFound(format!("card c{id}")))?;
        let mut message = format!("Work on card c{id} of this project's board: {}", card.title);
        if !card.body.trim().is_empty() {
            message.push_str(&format!("\n\n{}", card.body.trim()));
        }
        message.push_str(&format!("\n\nWhen it is done, note what you did on c{id} and move it to Done (loom_board_note, loom_board_move)."));
        // An agent of this project's, given (it takes the card after its turn, if working), or a
        // new one; checked before the card moves.
        let given = match params["agent"].as_str() {
            Some(given) => {
                let agent = given.parse().ok().and_then(|n| daemon.ws.agent(n)).ok_or_else(|| Error::NotFound(format!("agent {given}")))?;
                if agent.project != project {
                    return Err(Error::Invalid(format!("agent {given} is in another project")));
                }
                Some(agent)
            }
            None => None,
        };
        loom_core::board::act(&project, "board_move", &json!({ "card": id, "lane": "Doing" }), "you").expect("a board command").map_err(Error::Invalid)?;
        let agent = given.unwrap_or_else(|| daemon.ws.add_agent(Saved::new(project.clone())));
        agent.card.set(Some(id));
        agent.submit_with(message, Vec::new());
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
    for (name, needs) in [("rename_project", "name"), ("set_collapsed", "collapsed")] {
        let weak = Rc::downgrade(daemon);
        hub.handle(name, move |who, params| {
            if !human(who) {
                return Err(Error::Forbidden("changing projects".into()));
            }
            if params.get(needs).is_none() {
                return Err(Error::Invalid(format!("{name} needs {needs}")));
            }
            let daemon = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
            daemon.ws.change_project(params)?;
            daemon.save_soon();
            Ok(json!({}))
        });
    }
    for name in ["move_agent", "move_project"] {
        let weak = Rc::downgrade(daemon);
        hub.handle(name, move |who, params| {
            if !human(who) {
                return Err(Error::Forbidden("reordering the sidebar".into()));
            }
            let daemon = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
            let by = params["by"].as_i64().filter(|b| *b == 1 || *b == -1).ok_or_else(|| Error::Invalid("by: -1 or 1".into()))? as i32;
            let moved = match name {
                "move_agent" => daemon.ws.move_agent(params["agent"].as_str().and_then(|a| a.parse().ok()).ok_or_else(|| Error::Invalid("which agent?".into()))?, by),
                _ => daemon.ws.move_project(Path::new(params["project"].as_str().ok_or_else(|| Error::Invalid("which project?".into()))?), by),
            };
            if moved {
                daemon.save_soon();
            }
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
