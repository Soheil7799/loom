//! The Notes chat: talking with the notes vault, as with a web chatbot. Left: its past chats
//! (newest first) under "New chat"; right: the chat. Each chat is an agent of its own in the
//! vault, kept out of the sidebar and of Scribe's journal; it starts only with its first
//! message, so a chat never sent to is never kept. Its model is the latest Sonnet unless
//! changed in the chat. How it treats the notes is in the notes-chat skill (loom's Claude
//! folder, editable).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::{glib, pango, prelude::*};
use serde_json::json;

use crate::agent::{Agent, Event};
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
        // One chat runs at a time: the one left goes (its conversation is kept by Claude Code).
        if let Some(old) = self.current.borrow_mut().take() {
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
        let weak = Rc::downgrade(self);
        agent.on_event(move |agent, event| {
            let Some(chat) = weak.upgrade() else { return };
            // A chat's first message makes it one of the vault's past chats.
            if matches!(event, Event::Session | Event::Done) && chat.current.borrow().as_ref().is_some_and(|c| Rc::ptr_eq(c, agent)) {
                chat.refresh();
            }
            if matches!(event, Event::Done) {
                chat.record(agent);
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

    /// The notes the chat's agent changed and the vault's git does not have yet (each against
    /// its last commit), told to the Notes history (read off the main loop).
    fn record(&self, agent: &Rc<Agent>) {
        let Some(vault) = crate::vault::current() else { return };
        let Some(on_edits) = self.on_edits.borrow().clone() else { return };
        let notes: Vec<String> = agent
            .written()
            .into_iter()
            .filter_map(|p| p.strip_prefix(&vault).ok().map(|r| r.to_string_lossy().into_owned()))
            .filter(|r| r.ends_with(".md") && !r.starts_with(".loom/") && !r.starts_with(".git/"))
            .collect();
        if notes.is_empty() {
            return;
        }
        let (session, title) = (agent.session_id().unwrap_or_default(), agent.title());
        glib::spawn_future_local(async move {
            let edits = gtk::gio::spawn_blocking(move || {
                notes
                    .into_iter()
                    .filter_map(|note| {
                        let after = std::fs::read_to_string(vault.join(&note)).ok()?;
                        let shown = std::process::Command::new("git").arg("-C").arg(&vault).args(["show", &format!("HEAD:{note}")]).output().ok();
                        let before = shown.filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
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
        self.chat_for(None);
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
                    let vault = history::real(&vault);
                    let pasts: Vec<Past> = all.into_iter().filter(|p| history::real(&p.cwd) == vault).collect();
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
