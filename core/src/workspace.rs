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

pub struct Workspace {
    pub app_id: String,
    pub projects: RefCell<Vec<Project>>,
    pub agents: RefCell<Vec<Rc<Agent>>>,
    /// Conversations closed lately (to reopen).
    pub closed: RefCell<Vec<Saved>>,
    pub schedules: RefCell<Vec<Schedule>>,
    next_id: Cell<u32>,
    host: RefCell<Option<Weak<dyn Host>>>,
}

impl Workspace {
    pub fn new(app_id: &str) -> Rc<Workspace> {
        Rc::new(Workspace {
            app_id: app_id.to_owned(),
            projects: RefCell::default(),
            agents: RefCell::default(),
            closed: RefCell::default(),
            schedules: RefCell::default(),
            next_id: Cell::new(1),
            host: RefCell::default(),
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
        agent.on_event(move |agent, event| {
            if let Some(host) = weak.upgrade() {
                host.agent_event(agent, event);
            }
        });
        self.agents.borrow_mut().push(agent.clone());
        host.agent_added(&agent);
        agent
    }

    pub fn agent(&self, id: u32) -> Option<Rc<Agent>> {
        self.agents.borrow().iter().find(|a| a.id == id).cloned()
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
                agent.jobs.run_named(&name, command, params["restart"].as_bool().unwrap_or(false), cwd)?;
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

    /// A schedule's button pressed.
    pub fn act(&self, id: u64, act: Act) {
        match act {
            Act::RunNow => self.run_schedule(id, true),
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

/// Where a folder's conversation can still start: the folder, its project, else home.
pub fn start_folder(cwd: &Path) -> PathBuf {
    [cwd.to_owned(), crate::repos::project_of(cwd), crate::paths::home()].into_iter().find(|d| d.is_dir()).unwrap_or_else(|| cwd.to_owned())
}
