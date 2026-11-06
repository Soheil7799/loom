//! The workspace: the projects and the agents open in them, the conversations closed lately,
//! the scheduled prompts, and what happens to agents over time (idle ones suspended, due
//! prompts sent). What is on screen (which agent has focus, the window) is the UI's.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use serde_json::{Value, json};

use crate::agent::view::View;
use crate::agent::{Agent, Event, Status};
use crate::jobs::Jobs;
use crate::schedule::{self, Schedule};
use crate::settings;
use crate::state::{self, Layout, Project, Saved};

/// What the workspace needs of the UI.
pub trait Host {
    /// A new agent's view, and its project's jobs.
    fn agent_parts(&self, project: &Path) -> (Jobs, Rc<dyn View>);
    /// A new agent, to place in the UI.
    fn agent_added(&self, agent: &Rc<Agent>);
    /// Something happened to an agent.
    fn agent_event(&self, agent: &Rc<Agent>, event: Event);
    /// A word on the desktop about `agent` (None: about loom itself, with `title`).
    fn notify(&self, agent: Option<&Rc<Agent>>, title: &str, body: &str);
    /// The schedules changed (or one ran): worth saving and showing again.
    fn schedules_changed(&self);
}

/// What a schedule's buttons do.
pub enum Act {
    RunNow,
    Pause,
    Remove,
}

/// A turn quiet this long, no tool running, is stalled (the UI says so).
pub const STALLED: Duration = Duration::from_secs(180);

/// Whether the user has an agent in front of them (the UI's answer).
pub type Looking = Rc<dyn Fn(&Agent) -> bool>;

pub struct Workspace {
    pub app_id: String,
    pub projects: RefCell<Vec<Project>>,
    pub agents: RefCell<Vec<Rc<Agent>>>,
    /// Conversations closed lately (to reopen).
    pub closed: RefCell<Vec<Saved>>,
    pub schedules: RefCell<Vec<Schedule>>,
    next_id: Cell<u32>,
    /// Whether the user has an agent in front of them in loom (the UI says; a bridge then
    /// stays quiet about it).
    pub looking: RefCell<Option<Looking>>,
    host: RefCell<Option<Weak<dyn Host>>>,
    me: Weak<Workspace>,
}

impl Workspace {
    pub fn new(app_id: &str) -> Rc<Workspace> {
        Rc::new_cyclic(|me| Workspace {
            app_id: app_id.to_owned(),
            projects: RefCell::default(),
            agents: RefCell::default(),
            closed: RefCell::default(),
            schedules: RefCell::default(),
            next_id: Cell::new(1),
            looking: RefCell::default(),
            host: RefCell::default(),
            me: me.clone(),
        })
    }

    pub fn set_host(&self, host: Weak<dyn Host>) {
        *self.host.borrow_mut() = Some(host);
    }

    fn host(&self) -> Option<Rc<dyn Host>> {
        self.host.borrow().as_ref().and_then(Weak::upgrade)
    }

    /// The saved workspace read back: projects, closed conversations, schedules and agents
    /// (the layout returned has what the UI puts back: focus, pinned, the window). Due
    /// schedules are sent from now on.
    pub fn restore(self: &Rc<Self>) -> Layout {
        let mut layout = state::load(&self.app_id);
        *self.projects.borrow_mut() = std::mem::take(&mut layout.projects);
        *self.closed.borrow_mut() = std::mem::take(&mut layout.closed);
        let mut schedules = std::mem::take(&mut layout.schedules);
        let now = schedule::now();
        // Missed by over an hour while loom was closed: the next one comes as usual.
        for s in schedules.iter_mut().filter(|s| s.next.is_some_and(|t| t < now.timestamp().as_second() - 3600)) {
            s.next = match schedule::next_run(&s.when, &now) {
                Ok((t, true)) => Some(t),
                _ => None,
            };
        }
        *self.schedules.borrow_mut() = schedules;
        for saved in std::mem::take(&mut layout.agents) {
            self.add_agent(saved);
        }
        let weak = Rc::downgrade(self);
        let _ = crate::rt::every(Duration::from_secs(20), move || {
            let Some(ws) = weak.upgrade() else { return false };
            ws.send_due();
            true
        });
        layout
    }

    /// The workspace as saved: agents in their order (`focused` and `pinned` among them),
    /// and the window's size.
    pub fn layout(&self, focused: Option<&Rc<Agent>>, pinned: Option<&Rc<Agent>>, (width, height, maximized): (i32, i32, bool)) -> Layout {
        let order = self.ordered(false);
        let at = |agent: Option<&Rc<Agent>>| agent.and_then(|f| order.iter().position(|a| Rc::ptr_eq(a, f)));
        Layout {
            projects: self.projects.borrow().clone(),
            agents: order.iter().map(|a| a.saved()).collect(),
            focused: at(focused).unwrap_or(0),
            pinned: at(pinned),
            closed: self.closed.borrow().clone(),
            width,
            height,
            maximized,
            schedules: self.schedules.borrow().clone(),
        }
    }

    /// `rename_project`, `set_collapsed`: the project's change, if there is that project
    /// (`project`: its folder). The host saves and redraws after.
    pub fn change_project(&self, params: &Value) -> Result<(), crate::api::Error> {
        use crate::api::Error;
        let path = params["project"].as_str().map(PathBuf::from).ok_or_else(|| Error::Invalid("which project (its folder)?".into()))?;
        let mut projects = self.projects.borrow_mut();
        let project = projects.iter_mut().find(|p| p.path == path).ok_or_else(|| Error::NotFound(format!("project {}", path.display())))?;
        if let Some(name) = params.get("name") {
            project.name = name.as_str().ok_or_else(|| Error::Invalid("name: a string (empty: the folder's)".into()))?.trim().to_owned();
        }
        if let Some(collapsed) = params.get("collapsed") {
            project.collapsed = collapsed.as_bool().ok_or_else(|| Error::Invalid("collapsed: true or false".into()))?;
        }
        drop(projects);
        self.publish_agents();
        Ok(())
    }

    /// `move_agent {agent, by}`: one up (-1) or down (1) among its project's agents; false if
    /// it is at that end already (or no such agent).
    pub fn move_agent(&self, id: u32, by: i32) -> bool {
        let mut agents = self.agents.borrow_mut();
        let Some(agent) = agents.iter().find(|a| a.id == id).cloned() else { return false };
        let mine: Vec<usize> = agents.iter().enumerate().filter(|(_, a)| a.project == agent.project).map(|(i, _)| i).collect();
        let Some(at) = mine.iter().position(|&i| agents[i].id == id) else { return false };
        let to = at as i32 + by;
        if to < 0 || to as usize >= mine.len() {
            return false;
        }
        agents.swap(mine[at], mine[to as usize]);
        drop(agents);
        self.publish_agents();
        true
    }

    /// `move_project {project, by}`: one up or down the sidebar; false at that end.
    pub fn move_project(&self, path: &Path, by: i32) -> bool {
        let mut projects = self.projects.borrow_mut();
        let Some(at) = projects.iter().position(|p| p.path == path) else { return false };
        let to = at as i32 + by;
        if to < 0 || to as usize >= projects.len() {
            return false;
        }
        projects.swap(at, to as usize);
        drop(projects);
        self.publish_agents();
        true
    }

    /// `remove_project`: the project's agents, to close before it goes; a conflict while any
    /// works (the window asks first; through the API, stop them first).
    pub fn project_to_remove(&self, params: &Value) -> Result<(PathBuf, Vec<Rc<Agent>>), crate::api::Error> {
        use crate::api::Error;
        let path = params["project"].as_str().map(PathBuf::from).ok_or_else(|| Error::Invalid("which project (its folder)?".into()))?;
        if !self.projects.borrow().iter().any(|p| p.path == path) {
            return Err(Error::NotFound(format!("project {}", path.display())));
        }
        let agents: Vec<Rc<Agent>> = self.agents.borrow().iter().filter(|a| a.project == path).cloned().collect();
        let busy = agents.iter().filter(|a| a.is_busy() || a.background_running()).count();
        if busy > 0 {
            return Err(Error::Conflict(format!("{busy} of its agents working: stop them first")));
        }
        Ok((path, agents))
    }

    /// A project's name: the one given, else its folder's.
    pub fn project_label(&self, path: &Path) -> String {
        let projects = self.projects.borrow();
        let named = projects.iter().find(|p| p.path == path).map(|p| p.name.trim()).filter(|n| !n.is_empty());
        named.map_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned()), str::to_owned)
    }

    pub fn ensure_project(&self, path: &Path) {
        let mut projects = self.projects.borrow_mut();
        if !projects.iter().any(|p| p.path == path) {
            projects.push(Project { path: path.to_owned(), collapsed: false, name: String::new() });
        }
    }

    /// An agent opened from `saved`, in its project.
    pub fn add_agent(&self, saved: Saved) -> Rc<Agent> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let project = crate::repos::project_of(&saved.cwd);
        self.ensure_project(&project);
        let host = self.host().expect("a workspace with a UI");
        let (jobs, view) = host.agent_parts(&project);
        let agent = Agent::new(id, saved, project, jobs, view);
        agent.view.set_show_thinking(settings::get().show_thinking);
        let weak = Rc::downgrade(&host);
        let ws = self.me.clone();
        agent.on_event(move |agent, event| {
            if let Some(ws) = ws.upgrade() {
                ws.publish_agents();
            }
            if let Some(host) = weak.upgrade() {
                host.agent_event(agent, event);
            }
        });
        self.agents.borrow_mut().push(agent.clone());
        self.publish_agents();
        // Reopened from `closed` (by the window, or `reopen`): not closed any more.
        self.publish_closed();
        host.agent_added(&agent);
        agent
    }

    /// Closes `agent`: it stops, its conversation is remembered to reopen (one that never
    /// started has nothing to reopen), and it goes (`remove_agent`).
    pub fn close_agent(&self, agent: &Rc<Agent>) {
        agent.stop();
        let saved = agent.saved();
        if saved.session_id.is_some() {
            let mut closed = self.closed.borrow_mut();
            closed.retain(|c| c.session_id != saved.session_id);
            closed.insert(0, saved);
            closed.truncate(state::CLOSED_KEPT);
        }
        self.remove_agent(agent);
    }

    /// Takes `agent` out of the workspace (the UI has closed it): its conversation's
    /// subscribers are told it is gone.
    pub fn remove_agent(&self, agent: &Rc<Agent>) {
        self.agents.borrow_mut().retain(|a| !Rc::ptr_eq(a, agent));
        crate::api::close(&crate::agent::conversation(agent.id));
        crate::api::close(&format!("crew/{}", agent.id));
        self.publish_agents();
        self.publish_closed();
    }

    /// The core API's `agents` and `projects` resources made what the workspace has now, in
    /// sidebar order (only what changed goes out).
    ///
    /// An agent: `id`, `title`, `project` (its name), `project_id`, `status` (`suspended`,
    /// `starting`, `idle`, `working`, `blocked`, `failed`), `unseen`, `busy`, `questions` (how
    /// many wait for an answer), `used` (when it was last used), `place` (where it works:
    /// repo, branch, changes), `edited` (files it changed), `awake` (never suspended).
    /// A project: `id`, `name`, `collapsed`.
    pub fn publish_agents(&self) {
        let projects = self
            .projects_in_order()
            .iter()
            .map(|p| json!({ "id": p.path.to_string_lossy(), "name": self.project_label(&p.path), "collapsed": p.collapsed }))
            .collect();
        crate::api::hub().store("projects").set(projects);
        let items = self
            .agents_in_order()
            .iter()
            .map(|agent| {
                json!({
                    "id": agent.id.to_string(),
                    "title": agent.title(),
                    "project": self.project_label(&agent.project),
                    "status": agent.status().css(),
                    "unseen": agent.unseen.get(),
                    "busy": agent.is_busy(),
                    "questions": agent.open_questions().len(),
                    "used": crate::api::time(agent.used.get() as i64),
                    "project_id": agent.project.to_string_lossy(),
                    "place": agent.place(),
                    "edited": agent.files().len(),
                    "awake": agent.awake.get(),
                    "options": agent.options(),
                    "stalled": agent.stalled(STALLED),
                    "looking": self.looking.borrow().as_ref().is_some_and(|f| f(agent)),
                    "mode": agent.modes().0,
                    "modes": agent.modes().1,
                    // How the last turn ended: `turns` grows with each one.
                    "last_turn": {
                        "turns": agent.turns_ended.get(),
                        "seconds": agent.last_took.get(),
                        "error": agent.last_error.borrow().clone(),
                        "cancelled": agent.last_cancelled.get(),
                    },
                    // Each grows with its event: `times`, and what the last one was.
                    "failed": { "times": agent.failures.borrow().0, "why": agent.failures.borrow().1 },
                    "unsent": { "times": agent.unsent.get().0, "count": agent.unsent.get().1 },
                })
            })
            .collect();
        crate::api::hub().store("agents").set(items);
        // What waits in each one's input: the user's words, so people's alone (not `agents`,
        // which agents and bridges read).
        let inputs = self
            .agents_in_order()
            .iter()
            .map(|a| {
                let (commands, suggestion) = a.composer();
                let commands: Vec<Value> = commands.into_iter().map(|(name, about)| json!({ "name": name, "about": about })).collect();
                json!({ "id": a.id.to_string(), "draft": a.view.draft(), "queued": a.queued(), "commands": commands, "suggestion": suggestion })
            })
            .collect();
        crate::api::hub().store("inputs").set(inputs);
        // What the overview's cards show: people's too (an agent's words, its questions).
        let cards = self
            .agents_in_order()
            .iter()
            .map(|a| json!({ "id": a.id.to_string(), "waiting": a.questions().into_iter().map(|(_, title, _)| title).collect::<Vec<_>>(), "reply": a.last_reply(240), "session": a.session_id() }))
            .collect();
        crate::api::hub().store("overview").set(cards);
    }

    /// The API's `schedules`: `{id, title, prompt, when, next, paused, last_run, folder,
    /// session}` (times RFC 3339).
    pub fn publish_schedules(&self) {
        let items = self
            .schedules
            .borrow()
            .iter()
            .map(|s| {
                json!({
                    "id": s.id.to_string(),
                    "title": s.title,
                    "prompt": s.prompt,
                    "when": s.when,
                    "next": s.next.map(crate::api::time),
                    "paused": s.paused,
                    "last_run": s.last_run.map(crate::api::time),
                    "folder": s.cwd.to_string_lossy(),
                    "session": s.session_id,
                })
            })
            .collect();
        crate::api::hub().store("schedules").set(items);
    }

    pub fn agent(&self, id: u32) -> Option<Rc<Agent>> {
        self.agents.borrow().iter().find(|a| a.id == id).cloned()
    }

    /// The agent whose process is `pid`: its adapter, or a job it ran.
    pub fn agent_of_process(&self, pid: u32) -> Option<u32> {
        let agents = self.agents.borrow();
        agents.iter().find(|a| a.adapter_pid() == Some(pid)).map(|a| a.id).or_else(|| agents.iter().find_map(|a| a.jobs.agent_of(pid)))
    }

    /// The open agent with conversation `session`.
    pub fn agent_with_session(&self, session: &str) -> Option<Rc<Agent>> {
        self.agents.borrow().iter().find(|a| a.session_id().as_deref() == Some(session)).cloned()
    }

    /// The projects in sidebar order: with sorting on, those with an agent waiting for you
    /// first.
    pub fn projects_in_order(&self) -> Vec<Project> {
        let mut projects = self.projects.borrow().clone();
        if settings::get().sort_by_urgency {
            let agents = self.agents.borrow();
            let needs_you = |p: &Project| agents.iter().any(|a| a.project == p.path && a.status() == Status::Blocked);
            projects.sort_by_key(|p| !needs_you(p));
        }
        projects
    }

    /// Agents in sidebar order: by project, each where it was put; with sorting on, one that
    /// needs you goes up until answered, then back to its place.
    pub fn agents_in_order(&self) -> Vec<Rc<Agent>> {
        self.ordered(settings::get().sort_by_urgency)
    }

    /// `urgent`: those waiting for you first. What is saved is the order without that.
    pub fn ordered(&self, urgent: bool) -> Vec<Rc<Agent>> {
        let projects = if urgent { self.projects_in_order() } else { self.projects.borrow().clone() };
        let agents = self.agents.borrow();
        let mut ordered: Vec<Rc<Agent>> = Vec::new();
        for project in &projects {
            let mut mine: Vec<Rc<Agent>> = agents.iter().filter(|a| a.project == project.path).cloned().collect();
            if urgent {
                mine.sort_by_key(|a| a.status() != Status::Blocked);
            }
            ordered.extend(mine);
        }
        // Any agent whose project is missing from the list still counts.
        ordered.extend(agents.iter().filter(|a| !projects.iter().any(|p| p.path == a.project)).cloned());
        ordered
    }

    /// Agents idle for longer than the settings say, and out of `in_view`, suspended (those
    /// with a long conversation compacted first, so resuming reads less).
    pub fn suspend_idle(&self, in_view: impl Fn(&Rc<Agent>) -> bool) {
        let minutes = std::env::var("LOOM_IDLE_MINUTES").ok().and_then(|m| m.parse().ok()).unwrap_or(settings::get().idle_minutes);
        if minutes == 0 {
            return;
        }
        let limit = Duration::from_secs(minutes * 60);
        let s = settings::get();
        let agents = self.agents.borrow().clone();
        for agent in agents {
            // A workflow it launched still runs: suspending it would stop the workflow.
            if agent.status() == Status::Idle && !in_view(&agent) && !agent.awake.get() && !agent.background_running() && agent.last_active.get().elapsed() > limit {
                // A draft in its input: something is still being said to it, nothing changes.
                // A command's output waiting for the next message would go in front of /compact.
                if s.rest_compact && agent.view.context_used() >= s.rest_compact_tokens && agent.view.draft().trim().is_empty() && !agent.has_pending() {
                    agent.compact_then_rest();
                } else {
                    agent.suspend();
                }
            }
        }
    }

    // ---- what agents ask (loom's MCP tools) --------------------------------------------------

    /// A request from agent `id`'s MCP tools that needs no UI: its project's jobs, the agents,
    /// the Notes vault. None: one for the UI (opening a file, a notice).
    pub fn answer(&self, id: u32, method: &str, params: &Value) -> Option<Result<Value, String>> {
        let agent = self.agent(id);
        let gone = || "this agent is no longer open in loom".to_owned();
        let name = || params["name"].as_str().filter(|n| !n.is_empty()).map(str::to_owned).ok_or("a job name is needed".to_owned());
        let job = |agent: &Agent, name: &str| agent.jobs.named(name).ok_or_else(|| format!("no job named {name}"));
        Some(match method {
            "run_job" => (|| {
                let agent = agent.as_ref().ok_or_else(gone)?;
                let name = name()?;
                let command = params["command"].as_str().filter(|c| !c.trim().is_empty()).ok_or("a command is needed")?;
                // A folder of its own: a repo inside the project, say (relative to the agent's).
                let cwd = match params["cwd"].as_str().filter(|c| !c.is_empty()) {
                    Some(dir) => {
                        let dir = agent.cwd.join(state::expand(Path::new(dir)));
                        if !dir.is_dir() {
                            return Err(format!("{} is not a folder", dir.display()));
                        }
                        Some(dir)
                    }
                    None => None,
                };
                agent.jobs.run_named(&name, command, params["restart"].as_bool().unwrap_or(false), cwd, Some(agent.id))?;
                Ok(json!({ "name": name }))
            })(),
            "job_output" => (|| {
                let agent = agent.as_ref().ok_or_else(gone)?;
                let name = name()?;
                let job = job(agent, &name)?;
                let lines = params["lines"].as_u64().unwrap_or(60).clamp(1, 2000) as usize;
                Ok(json!({ "name": name, "running": job.running, "exit_code": job.exit_code, "output": agent.jobs.output(job.id, lines) }))
            })(),
            "stop_job" => (|| {
                let agent = agent.as_ref().ok_or_else(gone)?;
                agent.jobs.stop(job(agent, &name()?)?.id);
                Ok(json!({}))
            })(),
            "list_jobs" => agent
                .as_ref()
                .ok_or_else(gone)
                .map(|agent| json!(agent.jobs.list().into_iter().map(|j| json!({ "name": j.name, "running": j.running, "exit_code": j.exit_code })).collect::<Vec<_>>())),
            // The Notes vault, for the MCP server's note tools (null: Notes is off).
            "notes_info" => Ok(json!({ "vault": crate::vault::current().map(|v| v.to_string_lossy().into_owned()) })),
            "list_agents" => Ok(json!(
                self.agents_in_order()
                    .iter()
                    .map(|a| json!({ "title": a.title(), "project": a.project.to_string_lossy(), "status": a.status().css(), "you": a.id == id }))
                    .collect::<Vec<_>>()
            )),
            _ => return None,
        })
    }

    // ---- scheduled prompts --------------------------------------------------------------

    /// A new schedule: `prompt` sent `when` to `agent`'s conversation (`same`) or a new one in
    /// its folder. Err: `when` makes no sense.
    pub fn schedule(&self, agent: &Agent, prompt: String, when: String, same: bool) -> Result<(), String> {
        let (next, _) = schedule::next_run(&when, &schedule::now())?;
        let id = self.schedules.borrow().iter().map(|s| s.id).max().unwrap_or(0) + 1;
        self.schedules.borrow_mut().push(Schedule {
            id,
            cwd: agent.cwd.clone(),
            session_id: if same { agent.session_id() } else { None },
            title: agent.title(),
            prompt,
            when,
            next: Some(next),
            paused: false,
            last_run: None,
        });
        if let Some(host) = self.host() {
            host.schedules_changed();
        }
        Ok(())
    }

    /// `schedules` in the API, and `schedule_act {id, act: run_now | pause | remove}` (pause
    /// toggles), for people.
    pub fn serve_schedules(self: &Rc<Self>) {
        self.publish_schedules();
        let weak = Rc::downgrade(self);
        crate::api::hub().handle("schedule_act", move |who, params| {
            use crate::api::{Error, Principal};
            if !matches!(who, Principal::Window | Principal::Cli) {
                return Err(Error::Forbidden("schedules, but for people".into()));
            }
            let ws = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
            let id: u64 = params["id"].as_str().and_then(|i| i.parse().ok()).ok_or_else(|| Error::Invalid("which schedule?".into()))?;
            if !ws.schedules.borrow().iter().any(|s| s.id == id) {
                return Err(Error::NotFound(format!("schedule {id}")));
            }
            let act = match params["act"].as_str() {
                Some("run_now") => Act::RunNow,
                Some("pause") => Act::Pause,
                Some("remove") => Act::Remove,
                _ => return Err(Error::Invalid("act: run_now, pause or remove".into())),
            };
            // Published by the host (`schedules_changed`).
            ws.act(id, act);
            Ok(serde_json::json!({}))
        });
    }

    /// The API's `closed`: conversations closed lately, newest first, `{id: its session, title,
    /// folder, used}`.
    pub fn publish_closed(&self) {
        let items = self
            .closed
            .borrow()
            .iter()
            .filter_map(|c| Some(json!({ "id": c.session_id.clone()?, "title": c.title, "folder": c.cwd.to_string_lossy(), "used": crate::api::time(c.used as i64) })))
            .collect();
        crate::api::hub().store("closed").set(items);
    }

    /// Conversations in the API, for people: `closed` and `reopen {session}` (one closed
    /// lately), `past_conversations {folder?}` (Claude Code's, newest first) and
    /// `resume {session, folder}` (one of those, as an agent); and `schedule_new {agent,
    /// prompt, when, same?}`.
    pub fn serve_conversations(self: &Rc<Self>) {
        use crate::api::{Error, Principal};
        let people = |who: &Principal| matches!(who, Principal::Window | Principal::Cli);
        self.publish_closed();
        let hub = crate::api::hub();
        let weak = Rc::downgrade(self);
        hub.handle("reopen", move |who, params| {
            if !people(who) {
                return Err(Error::Forbidden("reopening, but for people".into()));
            }
            let ws = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
            let session = params["session"].as_str().ok_or_else(|| Error::Invalid("which conversation (its session)?".into()))?;
            // Open already (resumed meanwhile): that one, never a second agent on it.
            if let Some(open) = ws.agent_with_session(session) {
                ws.closed.borrow_mut().retain(|c| c.session_id.as_deref() != Some(session));
                ws.publish_closed();
                return Ok(json!({ "agent": open.id.to_string() }));
            }
            let saved = ws.closed.borrow().iter().find(|c| c.session_id.as_deref() == Some(session)).cloned();
            let saved = saved.ok_or_else(|| Error::NotFound(format!("closed conversation {session}")))?;
            ws.closed.borrow_mut().retain(|c| *c != saved);
            let agent = ws.add_agent(saved);
            Ok(json!({ "agent": agent.id.to_string() }))
        });
        hub.handle_later("past_conversations", move |who, params, reply| {
            if !people(who) {
                return reply(Err(Error::Forbidden("past conversations, but for people".into())));
            }
            let folder = params["folder"].as_str().map(|f| state::expand(Path::new(f)));
            past(move |listed| {
                reply(match listed {
                    Ok(all) => Ok(json!({
                        "conversations": all
                            .iter()
                            .filter(|p| folder.as_ref().is_none_or(|f| p.cwd.starts_with(f)))
                            .take(200)
                            .map(|p| json!({ "session": p.session_id, "title": p.title, "folder": p.cwd.to_string_lossy(), "updated": p.updated }))
                            .collect::<Vec<_>>(),
                    })),
                    Err(why) => Err(Error::Failed(why)),
                });
            });
        });
        let weak = Rc::downgrade(self);
        hub.handle("resume", move |who, params| {
            if !people(who) {
                return Err(Error::Forbidden("resuming, but for people".into()));
            }
            let ws = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
            let session = params["session"].as_str().ok_or_else(|| Error::Invalid("which conversation (its session)?".into()))?;
            let folder = params["folder"].as_str().map(|f| state::expand(Path::new(f))).ok_or_else(|| Error::Invalid("resume needs its folder".into()))?;
            if !folder.is_absolute() {
                return Err(Error::Invalid(format!("{} is not a full path", folder.display())));
            }
            // Not closed any more (were it among them), and never a second agent on it.
            ws.closed.borrow_mut().retain(|c| c.session_id.as_deref() != Some(session));
            if let Some(open) = ws.agent_with_session(session) {
                ws.publish_closed();
                return Ok(json!({ "agent": open.id.to_string() }));
            }
            // A folder gone since: its project, else home (as the window resumes it).
            let saved = Saved { cwd: start_folder(&folder), session_id: Some(session.to_owned()), title: params["title"].as_str().unwrap_or("").to_owned(), named: false, draft: String::new(), dirs: Vec::new(), used: state::now(), awake: false };
            let agent = ws.add_agent(saved);
            Ok(json!({ "agent": agent.id.to_string() }))
        });
        let weak = Rc::downgrade(self);
        hub.handle("schedule_new", move |who, params| {
            if !people(who) {
                return Err(Error::Forbidden("schedules, but for people".into()));
            }
            let ws = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
            let id = params["agent"].as_str().and_then(|a| a.parse().ok()).ok_or_else(|| Error::Invalid("which agent?".into()))?;
            let agent = ws.agent(id).ok_or_else(|| Error::NotFound(format!("agent {id}")))?;
            let prompt = params["prompt"].as_str().filter(|p| !p.trim().is_empty()).ok_or_else(|| Error::Invalid("schedule_new needs a prompt".into()))?;
            let when = params["when"].as_str().ok_or_else(|| Error::Invalid("when? (09:00, weekdays 9:30, every 2h, in 20m…)".into()))?;
            // Published by the host (`schedules_changed`).
            ws.schedule(&agent, prompt.to_owned(), when.to_owned(), params["same"].as_bool().unwrap_or(true)).map_err(Error::Invalid)?;
            Ok(json!({}))
        });
    }

    /// A schedule's button pressed.
    pub fn act(&self, id: u64, act: Act) {
        match act {
            // It says so itself.
            Act::RunNow => return self.run_schedule(id, true),
            Act::Pause => {
                if let Some(s) = self.schedules.borrow_mut().iter_mut().find(|s| s.id == id) {
                    s.paused = !s.paused;
                    if !s.paused {
                        s.next = schedule::next_run(&s.when, &schedule::now()).ok().filter(|(_, repeats)| *repeats).map(|(t, _)| t).or(s.next);
                    }
                }
            }
            Act::Remove => self.schedules.borrow_mut().retain(|s| s.id != id),
        }
        if let Some(host) = self.host() {
            host.schedules_changed();
        }
    }

    /// Sends the prompts that are due.
    pub fn send_due(&self) {
        let clock = schedule::now();
        let now = clock.timestamp().as_second();
        // Missed by over an hour (the laptop slept): skipped, the next one comes as usual.
        for s in self.schedules.borrow_mut().iter_mut().filter(|s| !s.paused && s.next.is_some_and(|t| t < now - 3600)) {
            s.next = schedule::next_run(&s.when, &clock).ok().filter(|(_, repeats)| *repeats).map(|(t, _)| t);
        }
        let due: Vec<u64> = self.schedules.borrow().iter().filter(|s| !s.paused && s.next.is_some_and(|t| t <= now)).map(|s| s.id).collect();
        for id in due {
            self.run_schedule(id, false);
        }
    }

    /// Sends a scheduled prompt: to its conversation (reopened if closed), or a new one.
    /// `by_hand`: Run now (the schedule keeps its next time).
    pub fn run_schedule(&self, id: u64, by_hand: bool) {
        let Some(schedule) = self.schedules.borrow().iter().find(|s| s.id == id).cloned() else { return };
        let host = self.host();
        if !schedule.cwd.is_dir() {
            // Its folder is gone: nothing to run in. Paused, and said so.
            if let Some(s) = self.schedules.borrow_mut().iter_mut().find(|s| s.id == id) {
                s.paused = true;
            }
            if let Some(host) = &host {
                host.notify(None, "A scheduled prompt was paused", &format!("Its folder no longer exists: {}", schedule.cwd.display()));
                host.schedules_changed();
            }
            return;
        }
        let cwd = schedule.cwd.clone();
        let agent = match &schedule.session_id {
            Some(session) => self.agent_with_session(session).unwrap_or_else(|| {
                self.closed.borrow_mut().retain(|c| c.session_id.as_deref() != Some(session.as_str()));
                self.add_agent(Saved { cwd, session_id: Some(session.clone()), title: schedule.title.clone(), named: false, draft: String::new(), used: state::now(), awake: false, dirs: Vec::new() })
            }),
            None => self.add_agent(Saved::new(cwd)),
        };
        agent.submit_with(schedule.prompt.clone(), Vec::new());
        let now = schedule::now();
        if let Some(s) = self.schedules.borrow_mut().iter_mut().find(|s| s.id == id) {
            s.last_run = Some(now.timestamp().as_second());
            if !by_hand {
                s.next = match schedule::next_run(&s.when, &now) {
                    Ok((t, true)) => Some(t),
                    _ => None,
                };
            }
        }
        if let Some(host) = &host {
            if !by_hand && settings::get().notify_done {
                host.notify(Some(&agent), &agent.title(), "Scheduled prompt sent");
            }
            host.schedules_changed();
        }
    }
}

/// Claude Code's past conversations, listed once for all who ask meanwhile, and kept a
/// moment (each listing starts an adapter process).
pub fn past(done: impl FnOnce(Result<Vec<crate::history::Past>, String>) + 'static) {
    let fresh = KEPT.with_borrow(|k| k.as_ref().filter(|(at, _)| at.elapsed() < Duration::from_secs(30)).map(|(_, all)| all.clone()));
    if let Some(all) = fresh {
        return done(Ok(all));
    }
    let now = LISTINGS.get();
    // A listing under way since the last change: waited for. One begun before it (stale): its
    // waiters move to a new one.
    let joined = WAITING.with_borrow_mut(|w| match w {
        Some((of, waiting)) if *of == now => {
            waiting.push(Box::new(done));
            None
        }
        stale => {
            let mut waiting = stale.take().map(|(_, w)| w).unwrap_or_default();
            waiting.push(Box::new(done));
            *stale = Some((now, waiting));
            Some(now)
        }
    });
    let Some(of) = joined else { return };
    let _ = crate::history::list(move |listed| {
        // Overtaken (a conversation began meanwhile): not kept; its waiters (if no newer
        // listing took them) ask again, which lists anew.
        if LISTINGS.get() != of {
            let mine = WAITING.with_borrow(|w| w.as_ref().is_some_and(|(at, _)| *at == of));
            if mine {
                for done in WAITING.with_borrow_mut(Option::take).map(|(_, w)| w).unwrap_or_default() {
                    past(done);
                }
            }
            return;
        }
        if let Ok(all) = &listed {
            KEPT.with_borrow_mut(|k| *k = Some((std::time::Instant::now(), all.clone())));
        }
        for done in WAITING.with_borrow_mut(Option::take).map(|(_, w)| w).unwrap_or_default() {
            done(listed.clone());
        }
    });
}

type Waiting = Vec<Box<dyn FnOnce(Result<Vec<crate::history::Past>, String>)>>;

thread_local! {
    /// The last listing of past conversations, and when it was made.
    static KEPT: RefCell<Option<(std::time::Instant, Vec<crate::history::Past>)>> = const { RefCell::new(None) };
    /// The listing under way (begun at which change), and who waits for it.
    static WAITING: RefCell<Option<(u64, Waiting)>> = const { RefCell::new(None) };
    /// Changes so far (a conversation began): a listing begun before the last is stale.
    static LISTINGS: Cell<u64> = const { Cell::new(0) };
}

/// The listing kept is stale (a conversation began): the next `past` lists again, and one
/// under way isn't kept.
pub fn forget_past() {
    KEPT.with_borrow_mut(|k| *k = None);
    LISTINGS.set(LISTINGS.get() + 1);
}

/// Where a folder's conversation can still start: the folder, its project, else home.
pub fn start_folder(cwd: &Path) -> PathBuf {
    [cwd.to_owned(), crate::repos::project_of(cwd), crate::paths::home()].into_iter().find(|d| d.is_dir()).unwrap_or_else(|| cwd.to_owned())
}
