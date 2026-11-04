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
//! What only a window does is not here: opening files for the user (`loom_open`,
//! `loom_edit` answer that loomd has no window), the Notes chat and Scribe (yet).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use loom_core::agent::{Agent, Event};
use loom_core::api::{Error, Principal};
use loom_core::jobs::Jobs;
use loom_core::state::{self, Saved};
use loom_core::workspace::{Host, Workspace};
use serde_json::json;

const APP_ID: &str = "dev.shirvani.Loom";

/// What hosts the core here: the agents' parts, and what happens to them.
struct Daemon {
    app_id: String,
    ws: Rc<Workspace>,
    bridges: Rc<loom_core::bridges::Host>,
    /// Each project's jobs.
    jobs: RefCell<HashMap<PathBuf, Jobs>>,
    /// The window's size as loom last saved it (kept for when the window comes back).
    size: (i32, i32, bool),
    /// A save is due shortly (several changes, one save).
    saving: std::cell::Cell<bool>,
    me: Weak<Daemon>,
}

impl Daemon {
    fn save(&self) {
        state::save(&self.app_id, &self.ws.layout(None, None, self.size));
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
        self.bridges.closed(agent.id);
        agent.stop();
        let saved = agent.saved();
        if saved.session_id.is_some() {
            let mut closed = self.ws.closed.borrow_mut();
            closed.retain(|c| c.session_id != saved.session_id);
            closed.insert(0, saved);
            closed.truncate(state::CLOSED_KEPT);
        }
        self.ws.remove_agent(agent);
        self.save_soon();
    }

    /// What an agent's loom tools ask (`tool`): what needs no window, done; the rest said so.
    fn tool(&self, agent: u32, name: &str, params: &serde_json::Value) -> Result<serde_json::Value, String> {
        if let Some(answer) = self.ws.answer(agent, name, params) {
            return answer;
        }
        match name {
            "notify" => {
                eprintln!("loomd: agent {agent}: {}", params["message"].as_str().or(params["text"].as_str()).unwrap_or(""));
                Ok(json!({}))
            }
            _ => Err(format!("loom runs without a window here (loomd): nothing can be shown to the user ({name})")),
        }
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

    fn agent_added(&self, _agent: &Rc<Agent>) {
        self.save_soon();
    }

    fn agent_event(&self, agent: &Rc<Agent>, event: Event) {
        // Nobody looks at it in a window: the bridges always may tell.
        self.bridges.agent_event(agent, &event, false);
        if !matches!(event, Event::Changed) {
            self.ws.publish_agents();
            self.save_soon();
        }
    }

    fn notify(&self, agent: Option<&Rc<Agent>>, title: &str, body: &str) {
        match agent {
            Some(agent) => eprintln!("loomd: {}: {body}", agent.title()),
            None => eprintln!("loomd: {title}: {body}"),
        }
    }

    fn schedules_changed(&self) {
        self.save_soon();
    }
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
        return 1;
    }
    loom_core::settings::init(&app_id);
    loom_core::audit::init(&app_id);
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
    let daemon = Rc::new_cyclic(|me| Daemon {
        app_id: app_id.clone(),
        ws: ws.clone(),
        bridges: loom_core::bridges::Host::new(&app_id),
        jobs: RefCell::default(),
        size,
        saving: std::cell::Cell::new(false),
        me: me.clone(),
    });
    let host: Rc<dyn Host> = daemon.clone();
    ws.set_host(Rc::downgrade(&host));
    commands(&daemon);

    // The API first: the agents are told where it is, the bridges are handed connections.
    let owner = {
        let ws = Rc::downgrade(&ws);
        move |pid| ws.upgrade()?.agent_of_process(pid).map(|id| Principal::Agent(id.to_string()))
    };
    if let Err(err) = loom_core::api::socket::serve(&socket, owner) {
        eprintln!("loomd: no API socket at {}: {err}", socket.display());
        return 1;
    }
    loom_core::control::set_current(socket.clone());
    ws.restore();
    ws.publish_agents();
    // Pictures sent in: PNG as it is (other formats need a toolkit to read them: the window).
    daemon.bridges.attach(
        &ws,
        std::sync::Arc::new(|bytes: Vec<u8>| if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { Ok(bytes) } else { Err("loomd takes pictures as PNG only".to_owned()) }),
    );
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
    daemon.save();
    for agent in ws.agents.borrow().iter() {
        agent.stop();
    }
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
    let weak = Rc::downgrade(daemon);
    hub.handle("tool", move |who, params| {
        let Principal::Agent(id) = who else { return Err(Error::Forbidden("an agent's tools, but for agents".into())) };
        let id: u32 = id.parse().map_err(|_| Error::Forbidden("an agent's tools, but for agents loom started".into()))?;
        let daemon = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
        let name = params["name"].as_str().ok_or_else(|| Error::Invalid("which tool?".into()))?;
        daemon.tool(id, name, &params["params"]).map_err(Error::Failed)
    });
}
