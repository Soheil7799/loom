//! The Notes chat: talking with the notes vault, as with a web chatbot. Its past chats (newest
//! first, by day) are the Notes view's sidebar page (`side`); the chat is its main page
//! (`stack`). Each chat is an agent of its own in the vault, kept out of the sidebar and of
//! Scribe's journal; it starts only with its first message, so a chat never sent to is never
//! kept. A chat left while it works keeps working (and goes once it is done). A chat can be
//! about one topic. Its model is the latest Sonnet unless changed in the chat. How it treats
//! the notes is in the notes-chat skill (loom's Claude folder, editable).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::{glib, pango, prelude::*};

use crate::chat_view::AgentChat;
use crate::agent::{Agent, Event, Status};
use crate::history::{self, Past};
use crate::jobs::Tray;

/// Ids of the chats' agents, apart from the sidebar's.
const FIRST_ID: u32 = 1_000_000;

use loom_core::notes_chat::{Before, Scope, chats, collect_edits, hint, prepare, remember_chat, remember_topic, saved_for, scope_of};


/// A chat with an agent alive: the one shown, or one left while it works.
struct Open {
    agent: Rc<Agent>,
    /// Its page in the stack: the chat, the starters over it while it has no message.
    page: gtk::Overlay,
    starters: gtk::Box,
    /// The notes it is changing in this turn, as they were before (None: new): read when its
    /// edit is asked for, so what it records is its own change, against what was there.
    before: Before,
}

pub struct NotesChat {
    /// The sidebar's page: the vault, "New chat" and the past chats.
    pub side: gtk::Box,
    /// The main page: the chat shown.
    pub stack: gtk::Stack,
    heading: gtk::Label,
    list: gtk::ListBox,
    note: gtk::Label,
    current: RefCell<Option<Rc<Agent>>>,
    open: RefCell<Vec<Open>>,
    tray: RefCell<Option<Rc<Tray>>>,
    next_id: Cell<u32>,
    /// The vault's past chats, newest first (the list's rows).
    pasts: RefCell<Vec<Past>>,
    listing: RefCell<Option<std::rc::Weak<crate::provider::Conn>>>,
    /// The chats working or waiting, as last listed (their rows are marked).
    marked: RefCell<Vec<(String, &'static str)>>,
    /// The core API's `notes_chats`, followed: what the list draws (warp).
    feed: RefCell<Option<crate::feed::Feed>>,
    /// The notes a turn changed, for the Notes history: (note, before, after), the session and
    /// the chat's title.
    pub on_edits: RefCell<Option<Rc<dyn Fn(Vec<(String, Option<String>, String)>, String, String)>>>,
    /// A chat waits for an answer (true) or none does: for the Notes button.
    pub on_attention: RefCell<Option<Rc<dyn Fn(bool)>>>,
    /// A chat was picked or made (to show the chat page).
    pub on_shown: RefCell<Option<Rc<dyn Fn()>>>,
    /// What happened to a chat's agent (for the phone, through the remote bridge).
    pub on_agent_event: RefCell<Option<Rc<dyn Fn(&Rc<Agent>, &Event)>>>,
    /// A chat's agent closed (for the phone).
    pub on_closed: RefCell<Option<Rc<dyn Fn(u32)>>>,
}

/// Working, or waiting for an answer: a chat left like that keeps going.
fn at_work(agent: &Agent) -> bool {
    matches!(agent.status(), Status::Working | Status::Blocked)
}

impl NotesChat {
    pub fn new() -> Rc<NotesChat> {
        let side = gtk::Box::new(gtk::Orientation::Vertical, 6);
        side.add_css_class("notes-side");
        let heading = gtk::Label::builder().label("NOTES").xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        heading.add_css_class("project-name");
        side.append(&heading);
        let new = gtk::Button::builder().child(&gtk::Label::builder().label("+  New chat").xalign(0.0).build()).tooltip_text("A new chat with your notes (Ctrl+N)").build();
        new.add_css_class("flat");
        new.add_css_class("notes-new");
        side.append(&new);
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
        list.add_css_class("notes-chats");
        // Each day's chats under its name.
        list.set_header_func(|row, before| {
            let day = row.widget_name();
            if before.is_some_and(|b| b.widget_name() == day) {
                row.set_header(None::<&gtk::Widget>);
            } else {
                let label = gtk::Label::builder().label(day.as_str()).xalign(0.0).build();
                label.add_css_class("notes-day");
                row.set_header(Some(&label));
            }
        });
        side.append(&gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build());
        let note = gtk::Label::builder().xalign(0.0).wrap(true).build();
        note.add_css_class("dim");
        note.add_css_class("small");
        side.append(&note);
        let stack = gtk::Stack::new();
        let empty = gtk::Label::new(Some("Notes is off: switch it on in Settings → Add-ons → Notes."));
        empty.add_css_class("dim");
        stack.add_named(&empty, Some("empty"));
        let chat = Rc::new(NotesChat {
            side,
            stack,
            heading,
            list,
            note,
            current: RefCell::default(),
            open: RefCell::default(),
            tray: RefCell::default(),
            next_id: Cell::new(FIRST_ID),
            pasts: RefCell::default(),
            listing: RefCell::default(),
            marked: RefCell::default(),
            feed: RefCell::default(),
            on_edits: RefCell::default(),
            on_attention: RefCell::default(),
            on_shown: RefCell::default(),
            on_agent_event: RefCell::default(),
            on_closed: RefCell::default(),
        });
        // The core API's `notes_chats` (warp): read again for each new follower; the list
        // draws from it too.
        // (The list's own subscription first: it reads nothing anew when the window starts.)
        let store = loom_core::api::hub().store("notes_chats");
        *chat.feed.borrow_mut() = crate::feed::Feed::new("notes_chats", || {});
        let weak = Rc::downgrade(&chat);
        store.on_subscribe(move |_| {
            if let Some(chat) = weak.upgrade() {
                chat.refresh();
            }
        });
        let weak = Rc::downgrade(&chat);
        chat.list.connect_row_activated(move |_, row| {
            if let Some(chat) = weak.upgrade() {
                chat.pick(row.index() as usize);
            }
        });
        let weak = Rc::downgrade(&chat);
        new.connect_clicked(move |_| {
            if let Some(chat) = weak.upgrade() {
                chat.new_chat();
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
        let name = vault.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.heading.set_label(&format!("NOTES · {name}"));
        self.heading.set_tooltip_text(Some(&vault.to_string_lossy()));
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

    /// Every chat with an agent alive (the one shown, and those left while working).
    pub fn agents(&self) -> Vec<Rc<Agent>> {
        self.open.borrow().iter().map(|o| o.agent.clone()).collect()
    }

    /// The input of the chat shown, focused (to type at once).
    pub fn focus(&self) {
        if let Some(agent) = self.current.borrow().as_ref() {
            agent.chat().input.grab_focus();
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

    /// The chat shown is left: one at work (or waiting for you) keeps going, and goes once it
    /// is neither; else what it changed is recorded and it goes.
    fn set_aside(&self) {
        let Some(agent) = self.current.borrow_mut().take() else { return };
        agent.focused.set(false);
        if !at_work(&agent) {
            self.close(&agent);
        }
    }

    /// A chat left while at work, at work no more (done, stopped, failed): it goes. Looked at
    /// once the event is over, so a message it had waiting is sent first (and keeps it).
    fn close_if_done(self: &Rc<Self>, agent: &Rc<Agent>) {
        let (weak, agent) = (Rc::downgrade(self), Rc::downgrade(agent));
        glib::idle_add_local_once(move || {
            let (Some(chat), Some(agent)) = (weak.upgrade(), agent.upgrade()) else { return };
            let current = chat.current.borrow().as_ref().is_some_and(|c| Rc::ptr_eq(c, &agent));
            if !current && !at_work(&agent) && !agent.has_waiting() {
                chat.close(&agent);
            }
        });
    }

    /// A chat waits for an answer.
    pub fn waiting(&self) -> bool {
        self.open.borrow().iter().any(|o| o.agent.status() == Status::Blocked)
    }

    fn close(&self, agent: &Rc<Agent>) {
        let at = self.open.borrow().iter().position(|o| Rc::ptr_eq(&o.agent, agent));
        let Some(at) = at else { return };
        let open = self.open.borrow_mut().remove(at);
        self.record(&open);
        open.agent.stop();
        self.stack.remove(&open.page);
        let closed = self.on_closed.borrow().clone();
        if let Some(closed) = closed {
            closed(open.agent.id);
        }
    }

    /// An agent for chat `session` (None: a new chat), about `scope` if given, shown.
    fn chat_for(self: &Rc<Self>, session: Option<String>, scope: Option<Scope>) {
        let Some(vault) = crate::vault::current() else { return };
        self.set_aside();
        // Still at work from before: that one again.
        let alive = session.as_ref().and_then(|s| self.open.borrow().iter().find(|o| o.agent.session_id().as_ref() == Some(s)).map(|o| o.agent.clone()));
        if let Some(agent) = alive {
            *self.current.borrow_mut() = Some(agent.clone());
            self.show(&agent);
            return;
        }
        // A past chat about a topic is about it again.
        let scope = scope.or_else(|| scope_of(&vault, session.as_deref()?));
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let saved = saved_for(&vault, session.clone(), scope.as_ref());
        let tray = self.tray(&vault);
        let chat = crate::chat::Chat::new();
        let project = loom_core::repos::project_of(&vault);
        let agent = Agent::new(id, saved, project, tray.core(), crate::chat_view::ChatView::new(chat.clone(), &tray));
        crate::chat_view::wire(&agent, &chat);
        let before = prepare(&agent, &vault, scope.as_ref());
        agent.set_show_thinking(crate::settings::get().show_thinking);
        agent.chat().set_hint(&hint(scope.as_ref()));
        let starters = self.starters(&agent, &vault, scope.as_ref());
        starters.set_visible(session.is_none());
        let page = gtk::Overlay::builder().child(&agent.chat().root).build();
        page.add_overlay(&starters);
        let weak = Rc::downgrade(self);
        let topic = scope.as_ref().map(|(name, _)| name.clone());
        let shown_starters = starters.downgrade();
        agent.on_event(move |agent, event| {
            let Some(chat) = weak.upgrade() else { return };
            let tell = chat.on_agent_event.borrow().clone();
            if let Some(tell) = tell {
                tell(agent, &event);
            }
            // A chat's first message makes it one of the vault's past chats.
            if matches!(event, Event::Session)
                && let (Some(session), Some(vault)) = (agent.session_id(), crate::vault::current())
            {
                remember_chat(&vault, &session);
                if let Some(topic) = &topic {
                    remember_topic(&vault, &session, topic);
                }
                if let Some(starters) = shown_starters.upgrade() {
                    starters.set_visible(false);
                }
                chat.refresh();
            }
            let current = chat.current.borrow().as_ref().is_some_and(|c| Rc::ptr_eq(c, agent));
            if matches!(event, Event::Done) && current {
                if let Some(open) = chat.open.borrow().iter().find(|o| Rc::ptr_eq(&o.agent, agent)) {
                    chat.record(open);
                }
                chat.refresh();
            }
            // Left while it worked: once done (or stopped, or failed), it goes.
            if !current && !at_work(agent) {
                chat.close_if_done(agent);
            }
            if matches!(event, Event::Changed | Event::Asked(_) | Event::Done) {
                chat.attention();
            }
        });
        self.stack.add_child(&page);
        self.open.borrow_mut().push(Open { agent: agent.clone(), page, starters, before });
        *self.current.borrow_mut() = Some(agent.clone());
        // A past chat is read back now; a new one starts with its first message.
        if session.is_some() {
            agent.start();
        }
        self.show(&agent);
    }

    /// Over an empty chat: what it is about, and a few beginnings that fill the input.
    fn starters(&self, agent: &Rc<Agent>, vault: &Path, scope: Option<&Scope>) -> gtk::Box {
        let starters = gtk::Box::new(gtk::Orientation::Vertical, 8);
        starters.add_css_class("notes-starters");
        starters.set_halign(gtk::Align::Center);
        starters.set_valign(gtk::Align::Center);
        let name = vault.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (title, lines): (String, Vec<(String, String)>) = match scope {
            Some((topic, _)) => (
                format!("{topic} · {name}"),
                vec![
                    (format!("What is new in {topic}?"), format!("What is new in {topic} lately?")),
                    (format!("Sum up {topic}"), format!("Sum up what my notes on {topic} say.")),
                    (format!("Add to {topic}…"), format!("Keep this in {topic}: ")),
                ],
            ),
            None => (
                format!("Your notes · {name}"),
                vec![
                    ("What changed this week?".to_owned(), "What changed in my notes this week?".to_owned()),
                    ("What do my notes say about…".to_owned(), "What do my notes say about ".to_owned()),
                    ("Keep this…".to_owned(), "Keep this in my notes: ".to_owned()),
                ],
            ),
        };
        let heading = gtk::Label::new(Some(&title));
        heading.add_css_class("notes-starters-title");
        starters.append(&heading);
        let line = gtk::Label::builder().label("Ask what they say, or tell them what to keep. Every change shows in History, and can be undone.").wrap(true).max_width_chars(48).justify(gtk::Justification::Center).build();
        line.add_css_class("dim");
        starters.append(&line);
        for (label, text) in lines {
            let button = gtk::Button::with_label(&label);
            button.add_css_class("notes-starter");
            let input = agent.chat().input.downgrade();
            button.connect_clicked(move |_| {
                let Some(input) = input.upgrade() else { return };
                let buffer = input.buffer();
                buffer.set_text(&text);
                buffer.place_cursor(&buffer.end_iter());
                input.grab_focus();
            });
            starters.append(&button);
        }
        starters
    }

    /// The notes `open`'s chat changed since the last time (each against how it was just
    /// before the chat's edit), told to the Notes history. Only its own edits: a note changed
    /// only by someone else is not among them.
    fn record(&self, open: &Open) {
        // Not recorded (Notes off, no one to tell): what was read before this turn goes all
        // the same, or a later turn's change would run from it.
        let (Some(vault), Some(on_edits)) = (crate::vault::current(), self.on_edits.borrow().clone()) else {
            open.before.borrow_mut().clear();
            return;
        };
        let (session, title) = (open.agent.session_id().unwrap_or_default(), open.agent.title());
        collect_edits(&vault, &open.before, move |edits| on_edits(edits, session, title));
    }

    /// A chat waits for an answer (or none does): told on; the list marks who works or waits.
    fn attention(&self) {
        let waiting = self.waiting();
        if let Some(attention) = self.on_attention.borrow().clone() {
            attention(waiting);
        }
        let marked: Vec<(String, &'static str)> =
            self.open.borrow().iter().filter(|o| o.agent.is_busy()).filter_map(|o| Some((o.agent.session_id()?, o.agent.status().css()))).collect();
        if *self.marked.borrow() != marked {
            *self.marked.borrow_mut() = marked;
            self.fill();
        }
    }

    /// A new chat (the one open, if it is still empty).
    pub fn new_chat(self: &Rc<Self>) {
        let empty = self.current.borrow().as_ref().is_some_and(|a| a.session_id().is_none() && !a.is_busy());
        if empty {
            let current = self.current.borrow().clone();
            if let Some(agent) = current {
                self.show(&agent);
            }
            return;
        }
        self.chat_for(None, None);
    }

    /// A new chat with `text` in its input, for the user to send (or change first).
    pub fn new_chat_saying(self: &Rc<Self>, text: &str) {
        self.new_chat();
        let current = self.current.borrow().clone();
        if let Some(agent) = current {
            let buffer = agent.chat().input.buffer();
            buffer.set_text(text);
            buffer.place_cursor(&buffer.end_iter());
            agent.chat().input.grab_focus();
        }
    }

    /// A new chat about topic `name`, its notes in `folder`.
    pub fn topic_chat(self: &Rc<Self>, name: &str, folder: &str) {
        self.chat_for(None, Some((name.to_owned(), folder.to_owned())));
    }

    /// The next past chat (`by` 1) or the one before (-1), from the one shown.
    pub fn step(self: &Rc<Self>, by: i32) {
        let session = self.current.borrow().as_ref().and_then(|a| a.session_id());
        let count = self.pasts.borrow().len() as i32;
        if count == 0 {
            return;
        }
        let at = session.and_then(|s| self.pasts.borrow().iter().position(|p| p.session_id == s)).map_or(-1, |i| i as i32);
        let next = (at + by).clamp(0, count - 1);
        if next != at {
            self.pick(next as usize);
        }
    }

    /// The page shown (true) or left: its chat is watched only while it is shown.
    pub fn shown(&self, shown: bool) {
        if let Some(agent) = self.current.borrow().as_ref() {
            agent.focused.set(shown);
        }
    }

    /// loom quits: what the chats changed is recorded, and they stop.
    pub fn shutdown(&self) {
        let open: Vec<Open> = self.open.borrow_mut().drain(..).collect();
        for open in open {
            self.record(&open);
            open.agent.stop();
        }
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.stop_all();
        }
    }

    fn show(&self, agent: &Rc<Agent>) {
        if let Some(shown) = self.on_shown.borrow().clone() {
            shown();
        }
        self.publish();
        if let Some(open) = self.open.borrow().iter().find(|o| Rc::ptr_eq(&o.agent, agent)) {
            self.stack.set_visible_child(&open.page);
            open.starters.set_visible(agent.session_id().is_none() && !agent.is_busy());
        }
        agent.focused.set(true);
        agent.chat().input.grab_focus();
        self.mark_selected();
    }

    /// Past chat `index` of the list.
    fn pick(self: &Rc<Self>, index: usize) {
        let Some(past) = self.pasts.borrow().get(index).cloned() else { return };
        let current = self.current.borrow().clone().filter(|a| a.session_id().as_deref() == Some(past.session_id.as_str()));
        match current {
            // Shown again (another page may be in its place).
            Some(agent) => self.show(&agent),
            None => self.chat_for(Some(past.session_id), None),
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

    /// The core API's `notes_chats`: the vault's past chats, newest first, each `{id: its
    /// session, title, updated, mark: "working" | "blocked" | null, current}`.
    fn publish(&self) {
        let current = self.current.borrow().as_ref().and_then(|a| a.session_id());
        let marked = self.marked.borrow();
        let items = self
            .pasts
            .borrow()
            .iter()
            .map(|past| {
                serde_json::json!({
                    "id": past.session_id,
                    "title": past.title,
                    "updated": past.updated,
                    "mark": marked.iter().find(|(s, _)| *s == past.session_id).map(|(_, css)| *css),
                    "current": current.as_deref() == Some(past.session_id.as_str()),
                })
            })
            .collect();
        loom_core::api::hub().store("notes_chats").set(items);
    }

    fn fill(&self) {
        // Drawn from what the core API has: published, then read back as any client reads it.
        self.publish();
        loom_core::api::hub().store("notes_chats").flush();
        let items: Vec<serde_json::Value> = match self.feed.borrow().as_ref() {
            Some(feed) => {
                feed.update();
                feed.items().clone()
            }
            None => Vec::new(),
        };
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        for item in &items {
            let updated = item["updated"].as_str().unwrap_or_default();
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let words = gtk::Box::new(gtk::Orientation::Vertical, 1);
            words.set_hexpand(true);
            let title = gtk::Label::builder().label(item["title"].as_str().unwrap_or("A chat")).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            let when = gtk::Label::builder().label(history::ago(updated)).xalign(0.0).build();
            when.add_css_class("dim");
            when.add_css_class("small");
            words.append(&title);
            words.append(&when);
            line.append(&words);
            // Working, or waiting for you.
            if let Some(css) = item["mark"].as_str() {
                let dot = gtk::Box::builder().valign(gtk::Align::Center).build();
                dot.add_css_class("dot");
                dot.add_css_class(css);
                line.append(&dot);
            }
            let row = gtk::ListBoxRow::builder().child(&line).build();
            row.set_widget_name(history::day_of(updated));
            self.list.append(&row);
        }
        self.note.set_label(if self.pasts.borrow().is_empty() { "Your chats with the notes show here." } else { "" });
        self.mark_selected();
    }

    /// The row of the chat shown, selected (none while it has no message yet).
    fn mark_selected(&self) {
        let session = self.current.borrow().as_ref().and_then(|a| a.session_id());
        let index = session.and_then(|s| self.pasts.borrow().iter().position(|p| p.session_id == s));
        match index.and_then(|i| self.list.row_at_index(i as i32)) {
            Some(row) => self.list.select_row(Some(&row)),
            None => self.list.unselect_all(),
        }
    }
}

/// The Notes chat as the remote bridge reaches it (the phone's Vault tab): its chats, one of
/// them opened as if picked here, and the agents of those open.
pub struct Remote(pub std::rc::Weak<NotesChat>);

impl loom_core::bridges::host::Notes for Remote {
    fn chats(&self) -> serde_json::Value {
        let off = serde_json::json!({ "vault": null, "chats": [] });
        let (Some(chat), Some(vault)) = (self.0.upgrade(), crate::vault::current()) else { return off };
        // Read again for next time (the list comes from Claude Code, in the background).
        chat.refresh();
        let marked = chat.marked.borrow();
        let chats: Vec<serde_json::Value> = chat
            .pasts
            .borrow()
            .iter()
            .map(|past| {
                serde_json::json!({
                    "session": past.session_id,
                    "title": past.title,
                    "updated": past.updated,
                    "ago": history::ago(&past.updated),
                    "day": history::day_of(&past.updated),
                    "mark": marked.iter().find(|(s, _)| *s == past.session_id).map(|(_, css)| *css),
                })
            })
            .collect();
        serde_json::json!({
            "vault": vault.file_name().map(|n| n.to_string_lossy().into_owned()),
            "chats": chats,
            "current": chat.current().and_then(|a| a.session_id()),
        })
    }

    fn open(&self, session: Option<String>) -> Option<Rc<Agent>> {
        let chat = self.0.upgrade()?;
        crate::vault::current()?;
        match session {
            None => chat.new_chat(),
            Some(session) => {
                let open = chat.current().is_some_and(|a| a.session_id().as_deref() == Some(session.as_str()));
                if !open {
                    chat.chat_for(Some(session), None);
                }
            }
        }
        chat.current()
    }

    fn agent(&self, id: u32) -> Option<Rc<Agent>> {
        self.0.upgrade()?.agents().into_iter().find(|a| a.id == id)
    }
}
