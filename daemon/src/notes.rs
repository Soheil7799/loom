//! The Notes chat with no window: chats with the notes vault, each an agent working in it
//! (as loom's Notes view has them), opened through the API (`notes_open`) or by the phone
//! (the remote bridge's `notes_open`). Their past chats are the API's `notes_chats`; what
//! a chat's turn changed in the vault goes to Scribe's history, as in the window.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use loom_core::agent::{Agent, Event};
use loom_core::api::{Error, Principal};
use loom_core::history::Past;
use loom_core::jobs::Jobs;
use loom_core::notes_chat::{Before, chats, collect_edits, prepare, remember_chat, saved_for, scope_of};
use serde_json::{Value, json};

/// Notes chats' agents' ids start here (the window's do too: never an agent's of the sidebar).
const FIRST: u32 = 1_000_001;

struct Open {
    agent: Rc<Agent>,
    before: Before,
}

pub struct Notes {
    open: RefCell<Vec<Open>>,
    pasts: RefCell<Vec<Past>>,
    next: Cell<u32>,
    jobs: RefCell<Option<Jobs>>,
    scribe: Rc<loom_core::hub::Hub>,
    bridges: Rc<loom_core::bridges::Host>,
    me: Weak<Notes>,
}

impl Notes {
    pub fn new(scribe: Rc<loom_core::hub::Hub>, bridges: Rc<loom_core::bridges::Host>) -> Rc<Notes> {
        Rc::new_cyclic(|me| Notes { open: RefCell::default(), pasts: RefCell::default(), next: Cell::new(FIRST), jobs: RefCell::default(), scribe, bridges, me: me.clone() })
    }

    pub fn agents(&self) -> Vec<Rc<Agent>> {
        self.open.borrow().iter().map(|o| o.agent.clone()).collect()
    }

    /// The vault's past chats read again (in the background), then published.
    pub fn refresh(&self) {
        let Some(vault) = loom_core::vault::current() else {
            self.pasts.borrow_mut().clear();
            self.publish();
            return;
        };
        let me = self.me.clone();
        let ours = chats(&vault);
        loom_core::rt::spawn_local(async move {
            // Only the vault's own chats are read, not every past conversation (that listing
            // failed for good in a long-running loomd, 2026-11-11, cause not found).
            let wanted = ours.clone();
            let read = loom_core::rt::spawn_blocking(move || loom_core::history::list_of(&wanted)).await.ok().flatten();
            let Some(notes) = me.upgrade() else { return };
            if let Some(pasts) = read {
                *notes.pasts.borrow_mut() = pasts;
                return notes.publish();
            }
            // An ACP adapter keeps them: listed through it, shared with whoever lists meanwhile.
            loom_core::workspace::past(move |result| {
                let Some(notes) = me.upgrade() else { return };
                match result {
                    Ok(all) => *notes.pasts.borrow_mut() = all.into_iter().filter(|p| ours.contains(&p.session_id)).collect(),
                    Err(err) => loom_core::log::write("notes", &format!("past chats not listed: {err}")),
                }
                notes.publish();
            });
        });
    }

    /// `notes_chats`: `{id: its session, title, updated, mark, current: false}`, newest first.
    fn publish(&self) {
        let working: Vec<(String, &'static str)> = self.open.borrow().iter().filter(|o| o.agent.is_busy()).filter_map(|o| Some((o.agent.session_id()?, o.agent.status().css()))).collect();
        let items = self
            .pasts
            .borrow()
            .iter()
            .map(|past| {
                json!({
                    "id": past.session_id,
                    "title": past.title,
                    "updated": past.updated,
                    "mark": working.iter().find(|(s, _)| *s == past.session_id).map(|(_, css)| *css),
                    "current": false,
                    "agent": self.open.borrow().iter().find(|o| o.agent.session_id().as_deref() == Some(past.session_id.as_str())).map(|o| o.agent.id.to_string()),
                })
            })
            .collect();
        loom_core::api::hub().store("notes_chats").set(items);
    }

    /// Chat `session` open (None: a new one): its agent.
    pub fn open(&self, session: Option<String>) -> Option<Rc<Agent>> {
        self.open_about(session, None)
    }

    /// A chat, new or past; `topic`: a new one about that topic (its notes in reach).
    pub fn open_about(&self, session: Option<String>, topic: Option<&str>) -> Option<Rc<Agent>> {
        let vault = loom_core::vault::current()?;
        if let Some(session) = &session
            && let Some(open) = self.open.borrow().iter().find(|o| o.agent.session_id().as_ref() == Some(session))
        {
            return Some(open.agent.clone());
        }
        let scope = match topic {
            Some(name) => loom_core::topics::list(&vault).into_iter().find(|t| t.name == name).map(|t| (t.name, t.folder)),
            None => session.as_deref().and_then(|s| scope_of(&vault, s)),
        };
        let id = self.next.get();
        self.next.set(id + 1);
        let saved = saved_for(&vault, session, scope.as_ref());
        // made for the vault now: another chosen in Settings since gets jobs of its own
        // (their commands run in it)
        if self.jobs.borrow().as_ref().is_some_and(|jobs| jobs.cwd() != vault) {
            self.jobs.borrow_mut().take();
        }
        let jobs = self.jobs.borrow_mut().get_or_insert_with(|| {
            let (jobs, events) = Jobs::new(vault.clone());
            drop(loom_core::jobs_api::published(&jobs, events));
            jobs
        }).clone();
        let project = loom_core::repos::project_of(&vault);
        let agent = Agent::new(id, saved, project, jobs);
        let before = prepare(&agent, &vault, scope.as_ref());
        agent.read_back_when_followed();
        let me = self.me.clone();
        agent.on_event(move |agent, event| {
            if let Some(notes) = me.upgrade() {
                notes.event(agent, &event);
            }
        });
        self.open.borrow_mut().push(Open { agent: agent.clone(), before });
        self.publish();
        Some(agent)
    }

    fn event(&self, agent: &Rc<Agent>, event: &Event) {
        self.bridges.notes_event(agent, event, false);
        match event {
            // A chat's first message makes it one of the vault's past chats.
            Event::Session => {
                if let (Some(session), Some(vault)) = (agent.session_id(), loom_core::vault::current()) {
                    remember_chat(&vault, &session);
                }
                self.refresh();
            }
            Event::Done => {
                self.record(agent);
                if let Some(session) = agent.session_id() {
                    self.scribe.turn_done(&session);
                }
                self.refresh();
            }
            _ => self.publish(),
        }
    }

    /// What the chat's turn changed in the vault, to Scribe's history.
    fn record(&self, agent: &Rc<Agent>) {
        let open = self.open.borrow();
        let Some(open) = open.iter().find(|o| Rc::ptr_eq(&o.agent, agent)) else { return };
        let Some(vault) = loom_core::vault::current() else {
            open.before.borrow_mut().clear();
            return;
        };
        let (session, title, scribe) = (agent.session_id().unwrap_or_default(), agent.title(), self.scribe.clone());
        collect_edits(&vault, &open.before, move |edits| scribe.record_external(edits, &session, &title));
    }

    /// Closes chat agent `id` (it stops; the chat stays among the past ones).
    pub fn close(&self, id: u32) -> bool {
        let at = self.open.borrow().iter().position(|o| o.agent.id == id);
        let Some(at) = at else { return false };
        let open = self.open.borrow_mut().remove(at);
        if let Some(session) = open.agent.session_id() {
            self.scribe.check_now(&session, true);
        }
        self.bridges.notes_closed(id);
        open.agent.stop();
        loom_core::agent::close_resources(id);
        self.publish();
        true
    }

    /// Its chats' agents stopped, and the jobs run in the vault.
    pub fn stop_all(&self) {
        for open in self.open.borrow().iter() {
            open.agent.stop();
        }
        if let Some(jobs) = self.jobs.borrow().as_ref() {
            jobs.stop_all();
        }
    }
}

/// The phone's view of them (the remote bridge's `notes`, `notes_open`).
pub struct Remote(pub Weak<Notes>);

impl loom_core::bridges::host::Notes for Remote {
    fn chats(&self) -> Value {
        let off = json!({ "vault": null, "chats": [] });
        let (Some(notes), Some(vault)) = (self.0.upgrade(), loom_core::vault::current()) else { return off };
        notes.refresh();
        let chats: Vec<Value> = notes
            .pasts
            .borrow()
            .iter()
            .map(|past| {
                let open = notes.open.borrow().iter().find(|o| o.agent.session_id().as_deref() == Some(past.session_id.as_str())).map(|o| o.agent.clone());
                json!({
                    "session": past.session_id,
                    "title": past.title,
                    "updated": past.updated,
                    "mark": open.filter(|a| a.is_busy()).map(|a| a.status().css()),
                })
            })
            .collect();
        json!({ "vault": vault.file_name().map(|n| n.to_string_lossy().into_owned()), "chats": chats, "current": null })
    }

    fn open(&self, session: Option<String>) -> Option<Rc<Agent>> {
        self.0.upgrade()?.open(session)
    }

    fn agent(&self, id: u32) -> Option<Rc<Agent>> {
        self.0.upgrade()?.agents().into_iter().find(|a| a.id == id)
    }
}

/// `notes_open {session?}` → `{agent}` and `notes_close {agent}`, for people and the phone.
pub fn commands(notes: &Rc<Notes>) {
    let hub = loom_core::api::hub();
    let may = |who: &Principal| matches!(who, Principal::Window | Principal::Cli) || *who == Principal::Bridge("remote".into());
    let weak = Rc::downgrade(notes);
    hub.handle("notes_open", move |who, params| {
        if !may(who) {
            return Err(Error::Forbidden("the Notes chat".into()));
        }
        let notes = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
        let agent = notes.open_about(params["session"].as_str().map(str::to_owned), params["topic"].as_str()).ok_or_else(|| Error::Failed("Notes is off in loom (Settings → Add-ons → Notes).".into()))?;
        Ok(json!({ "agent": agent.id.to_string() }))
    });
    let weak = Rc::downgrade(notes);
    hub.handle("notes_close", move |who, params| {
        if !may(who) {
            return Err(Error::Forbidden("the Notes chat".into()));
        }
        let notes = weak.upgrade().ok_or_else(|| Error::NotFound("loom".into()))?;
        let id: u32 = params["agent"].as_str().and_then(|a| a.parse().ok()).ok_or_else(|| Error::Invalid("which chat (its agent)?".into()))?;
        if !notes.close(id) {
            return Err(Error::NotFound(format!("chat agent {id}")));
        }
        Ok(json!({}))
    });
}
