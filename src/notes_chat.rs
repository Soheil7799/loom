//! The Notes chat: talking with the notes vault, as with a web chatbot. Left: its past chats
//! (newest first) under "New chat"; right: the chat. Each chat is an agent of its own in the
//! vault, kept out of the sidebar and of Scribe's journal; it starts only with its first
//! message, so a chat never sent to is never kept. Its model is the latest Sonnet unless
//! changed in the chat. How it treats the notes is in the notes-chat skill (loom's Claude
//! folder, editable).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::{glib, pango, prelude::*};
use serde_json::json;

use crate::agent::{Agent, Event, Status};
use crate::history::{self, Past};
use crate::jobs::Tray;
use crate::state::Saved;

/// Ids of the chats' agents, apart from the sidebar's.
const FIRST_ID: u32 = 1_000_000;

/// What the chat's agent is told, besides the skill.
const PROMPT: &str = "You are in loom's Notes chat: the working folder is the user's notes vault. Follow the notes-chat skill for how to read and change the notes.";

pub struct NotesChat {
    pub root: gtk::Paned,
    list: gtk::ListBox,
    stack: gtk::Stack,
    note: gtk::Label,
    /// The chat shown.
    current: RefCell<Option<Rc<Agent>>>,
    tray: RefCell<Option<Rc<Tray>>>,
    next_id: Cell<u32>,
    /// The vault's past chats, newest first (the list's rows after "New chat").
    pasts: RefCell<Vec<Past>>,
    listing: RefCell<Option<std::rc::Weak<crate::provider::Conn>>>,
    /// The notes a turn changed, for the Notes history: (note, before, after), the session and
    /// the chat's title.
    pub on_edits: RefCell<Option<Rc<dyn Fn(Vec<(String, Option<String>, String)>, String, String)>>>,
    /// The chat waits for an answer (true) or no longer does: for the sidebar's entry.
    pub on_attention: RefCell<Option<Rc<dyn Fn(bool)>>>,
    /// The notes the chat is changing in this turn, as they were before (None: new): read when
    /// its edit is asked for, so what it records is its own change, against what was there.
    before: Rc<RefCell<HashMap<PathBuf, Option<String>>>>,
}

/// The vault's own list of its Notes chats (`.loom/chats.json`): only those are listed.
fn chats_file(vault: &Path) -> PathBuf {
    vault.join(crate::vault::OWN).join("chats.json")
}

fn chats(vault: &Path) -> Vec<String> {
    std::fs::read_to_string(chats_file(vault)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn remember_chat(vault: &Path, session: &str) {
    let mut all = chats(vault);
    if all.iter().any(|s| s == session) {
        return;
    }
    all.push(session.to_owned());
    let _ = std::fs::create_dir_all(vault.join(crate::vault::OWN));
    let _ = std::fs::write(chats_file(vault), serde_json::to_string(&all).unwrap_or_default());
}

/// `path` with its folder's links resolved (the vault may be reached through a link).
fn resolved(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => history::real(parent).join(name),
        _ => history::real(path),
    }
}

impl NotesChat {
    pub fn new() -> Rc<NotesChat> {
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
        list.add_css_class("notes-chats");
        let note = gtk::Label::builder().xalign(0.0).wrap(true).build();
        note.add_css_class("dim");
        note.add_css_class("small");
        let side = gtk::Box::new(gtk::Orientation::Vertical, 6);
        side.add_css_class("notes-chat-side");
        let heading = gtk::Label::builder().label("NOTES CHAT").xalign(0.0).build();
        heading.add_css_class("project-name");
        side.append(&heading);
        side.append(&gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build());
        side.append(&note);
        let stack = gtk::Stack::new();
        let empty = gtk::Label::new(Some("Notes is off: switch it on in Settings → Add-ons → Notes."));
        empty.add_css_class("dim");
        stack.add_named(&empty, Some("empty"));
        let root = gtk::Paned::builder().orientation(gtk::Orientation::Horizontal).start_child(&side).end_child(&stack).position(260).shrink_start_child(false).build();
        let chat = Rc::new(NotesChat {
            root,
            list,
            stack,
            note,
            current: RefCell::default(),
            tray: RefCell::default(),
            next_id: Cell::new(FIRST_ID),
            pasts: RefCell::default(),
            listing: RefCell::default(),
            on_edits: RefCell::default(),
            on_attention: RefCell::default(),
            before: Rc::default(),
        });
        let weak = Rc::downgrade(&chat);
        chat.list.connect_row_activated(move |_, row| {
            if let Some(chat) = weak.upgrade() {
                chat.pick(row.index() as usize);
            }
        });
        chat
    }

    /// Shown: the list read again, and a chat ready to type in (a new one, if none is open).
    pub fn open(self: &Rc<Self>) {
        let Some(vault) = crate::vault::current() else {
            self.stack.set_visible_child_name("empty");
            return;
        };
        self.refresh();
        let current = self.current.borrow().clone();
        match current {
            Some(agent) if agent.cwd == vault => self.show(&agent),
            _ => self.new_chat(),
        }
    }

    /// The chat shown, if any.
    pub fn current(&self) -> Option<Rc<Agent>> {
        self.current.borrow().clone()
    }

    /// The input of the chat shown, focused (to type at once).
    pub fn focus(&self) {
        if let Some(agent) = self.current.borrow().as_ref() {
            agent.chat.input.grab_focus();
        }
    }

    fn tray(&self, vault: &PathBuf) -> Rc<Tray> {
        let tray = self.tray.borrow().clone().filter(|t| t.cwd() == vault);
        tray.unwrap_or_else(|| {
            let tray = Tray::new(vault.clone());
            *self.tray.borrow_mut() = Some(tray.clone());
            tray
        })
    }

    /// An agent for chat `session` (None: a new chat), shown.
    fn chat_for(self: &Rc<Self>, session: Option<String>) {
        let Some(vault) = crate::vault::current() else { return };
        // One chat runs at a time: the one left goes (its conversation is kept by Claude Code;
        // what it changed so far is recorded).
        let old = self.current.borrow_mut().take();
        if let Some(old) = old {
            self.record(&old);
            old.stop();
            self.stack.remove(&old.chat.root);
        }
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let saved = Saved {
            cwd: vault.clone(),
            session_id: session.clone(),
            title: if session.is_some() { String::new() } else { "New chat".to_owned() },
            named: false,
            draft: String::new(),
            dirs: Vec::new(),
            used: 0,
            awake: false,
        };
        let agent = Agent::new(id, saved, self.tray(&vault));
        agent.no_journal.set(true);
        *agent.extra_options.borrow_mut() = json!({ "model": "sonnet", "appendSystemPrompt": PROMPT });
        agent.set_show_thinking(crate::settings::get().show_thinking);
        // A note it is about to edit, read as it is first.
        let (before, root) = (self.before.clone(), history::real(&vault));
        *agent.on_tool.borrow_mut() = Some(Rc::new(move |update: &serde_json::Value| {
            if update["sessionUpdate"] != "tool_call" || update["kind"] != "edit" {
                return;
            }
            let paths = update["locations"].as_array().into_iter().flatten().filter_map(|l| l["path"].as_str());
            for path in paths {
                let path = resolved(Path::new(path));
                if path.starts_with(&root) && !before.borrow().contains_key(&path) {
                    let was = std::fs::read_to_string(&path).ok();
                    before.borrow_mut().insert(path, was);
                }
            }
        }));
        let weak = Rc::downgrade(self);
        agent.on_event(move |agent, event| {
            let Some(chat) = weak.upgrade() else { return };
            let current = chat.current.borrow().as_ref().is_some_and(|c| Rc::ptr_eq(c, agent));
            // A chat's first message makes it one of the vault's past chats.
            if matches!(event, Event::Session) && current
                && let (Some(session), Some(vault)) = (agent.session_id(), crate::vault::current())
            {
                remember_chat(&vault, &session);
                chat.refresh();
            }
            if matches!(event, Event::Done) {
                chat.record(agent);
                if current {
                    chat.refresh();
                }
            }
            if matches!(event, Event::Changed | Event::Asked(_) | Event::Done) && current {
                let waiting = agent.status() == Status::Blocked;
                if let Some(attention) = chat.on_attention.borrow().clone() {
                    attention(waiting);
                }
            }
        });
        self.stack.add_child(&agent.chat.root);
        *self.current.borrow_mut() = Some(agent.clone());
        // A past chat is read back now; a new one starts with its first message.
        if session.is_some() {
            agent.start();
        }
        self.show(&agent);
    }

    /// The notes the chat changed since the last time (each against how it was just before
    /// the chat's edit), told to the Notes history. Only its own edits: a note changed only by
    /// someone else is not among them.
    fn record(&self, agent: &Rc<Agent>) {
        let taken: Vec<(PathBuf, Option<String>)> = self.before.borrow_mut().drain().collect();
        let Some(vault) = crate::vault::current() else { return };
        let Some(on_edits) = self.on_edits.borrow().clone() else { return };
        if taken.is_empty() {
            return;
        }
        let root = history::real(&vault);
        let (session, title) = (agent.session_id().unwrap_or_default(), agent.title());
        glib::spawn_future_local(async move {
            let edits = gtk::gio::spawn_blocking(move || {
                taken
                    .into_iter()
                    .filter_map(|(path, before)| {
                        let note = path.strip_prefix(&root).ok()?.to_string_lossy().into_owned();
                        if !note.ends_with(".md") || note.starts_with(".loom/") || note.starts_with(".git/") {
                            return None;
                        }
                        let after = std::fs::read_to_string(&path).ok()?;
                        (before.as_deref() != Some(after.as_str())).then_some((note, before, after))
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            if !edits.is_empty() {
                on_edits(edits, session, title);
            }
        });
    }

    fn new_chat(self: &Rc<Self>) {
        // The new chat open is still empty: it is the one.
        let empty = self.current.borrow().as_ref().is_some_and(|a| a.session_id().is_none() && !a.is_busy());
        if empty {
            let current = self.current.borrow().clone();
            if let Some(agent) = current {
                self.show(&agent);
            }
            return;
        }
        self.chat_for(None);
    }

    /// The page shown (true) or left: its chat is watched only while it is shown.
    pub fn shown(&self, shown: bool) {
        if let Some(agent) = self.current.borrow().as_ref() {
            agent.focused.set(shown);
        }
    }

    /// loom quits: what the chat changed is recorded, and it stops.
    pub fn shutdown(&self) {
        let current = self.current.borrow().clone();
        if let Some(agent) = current {
            self.record(&agent);
            agent.stop();
        }
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.stop_all();
        }
    }

    fn show(&self, agent: &Rc<Agent>) {
        self.stack.set_visible_child(&agent.chat.root);
        agent.focused.set(true);
        agent.chat.input.grab_focus();
        self.mark_selected();
    }

    /// Row `index`: "New chat", else a past chat.
    fn pick(self: &Rc<Self>, index: usize) {
        if index == 0 {
            self.new_chat();
            return;
        }
        let Some(past) = self.pasts.borrow().get(index - 1).cloned() else { return };
        let open = self.current.borrow().as_ref().and_then(|a| a.session_id()) == Some(past.session_id.clone());
        if !open {
            self.chat_for(Some(past.session_id));
        }
    }

    /// The vault's past chats, read again (Claude Code's own list of conversations).
    fn refresh(self: &Rc<Self>) {
        let Some(vault) = crate::vault::current() else { return };
        if self.listing.borrow().as_ref().is_some_and(|l| l.upgrade().is_some()) {
            return;
        }
        let weak = Rc::downgrade(self);
        let listing = history::list(move |result| {
            let Some(chat) = weak.upgrade() else { return };
            chat.listing.borrow_mut().take();
            match result {
                Ok(all) => {
                    let ours = chats(&vault);
                    let pasts: Vec<Past> = all.into_iter().filter(|p| ours.contains(&p.session_id)).collect();
                    *chat.pasts.borrow_mut() = pasts;
                    chat.fill();
                }
                Err(err) => chat.note.set_label(&format!("Could not list the past chats: {err}")),
            }
        });
        *self.listing.borrow_mut() = listing;
    }

    fn fill(&self) {
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let new = gtk::Label::builder().label("+  New chat").xalign(0.0).build();
        new.add_css_class("notes-chat-new");
        self.list.append(&gtk::ListBoxRow::builder().child(&new).build());
        let now = glib::DateTime::now_utc().ok();
        for past in self.pasts.borrow().iter() {
            let line = gtk::Box::new(gtk::Orientation::Vertical, 1);
            let title = gtk::Label::builder().label(past.title.as_deref().unwrap_or("A chat")).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            let when = gtk::Label::builder().label(now.as_ref().map(|n| history::ago(&past.updated, n)).unwrap_or_default()).xalign(0.0).build();
            when.add_css_class("dim");
            when.add_css_class("small");
            line.append(&title);
            line.append(&when);
            self.list.append(&gtk::ListBoxRow::builder().child(&line).build());
        }
        self.note.set_label(if self.pasts.borrow().is_empty() { "Your chats with the notes show here." } else { "" });
        self.mark_selected();
    }

    /// The row of the chat shown, selected ("New chat" while it has no message yet).
    fn mark_selected(&self) {
        let session = self.current.borrow().as_ref().and_then(|a| a.session_id());
        let index = session.and_then(|s| self.pasts.borrow().iter().position(|p| p.session_id == s)).map_or(0, |i| i + 1);
        if let Some(row) = self.list.row_at_index(index as i32) {
            self.list.select_row(Some(&row));
        }
    }
}
