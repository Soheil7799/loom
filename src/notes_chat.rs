//! The Notes chat: talking with the notes vault, as with a web chatbot. Its past chats (newest
//! first, by day) are the Notes view's sidebar page (`side`); the chat is its main page
//! (`stack`). Each chat is an agent of its own in the vault, kept out of the sidebar and of
//! Scribe's journal; it starts only with its first message, so a chat never sent to is never
//! kept. A chat left while it works keeps working (and goes once it is done). A chat can be
//! about one topic. Its model is the latest Sonnet unless changed in the chat. How it treats
//! the notes is in the notes-chat skill (loom's Claude folder, editable).

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use gtk::{pango, prelude::*};

use crate::history::{self, Past};


use loom_core::notes_chat::{Scope, hint};


/// A chat of loomd's (the window as its client): its agent's chat, its page, the starters.
struct RemoteOpen {
    agent: Rc<crate::remote_agent::RemoteAgent>,
    page: gtk::Overlay,
    starters: gtk::Box,
}

pub struct NotesChat {
    /// The sidebar's page: the vault, "New chat" and the past chats.
    pub side: gtk::Box,
    /// The main page: the chat shown.
    pub stack: gtk::Stack,
    heading: gtk::Label,
    list: gtk::ListBox,
    note: gtk::Label,
    /// The vault's past chats, newest first (the list's rows).
    pasts: RefCell<Vec<Past>>,
    /// The core API's `notes_chats`, followed: what the list draws (warp).
    feed: RefCell<Option<crate::feed::Feed>>,
    /// A chat waits for an answer (true) or none does: for the Notes button.
    pub on_attention: RefCell<Option<Rc<dyn Fn(bool)>>>,
    /// A chat was picked or made (to show the chat page).
    pub on_shown: RefCell<Option<Rc<dyn Fn()>>>,
    /// loomd's chats open here (the window as its client), and the one shown (its agent).
    remote_open: RefCell<Vec<RemoteOpen>>,
    remote_current: Cell<Option<u32>>,
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
            pasts: RefCell::default(),
            feed: RefCell::default(),
            on_attention: RefCell::default(),
            on_shown: RefCell::default(),
            remote_open: RefCell::default(),
            remote_current: Cell::new(None),
        });
        // loomd's client: its `notes_chats` is the list, and says who works.
        let weak = Rc::downgrade(&chat);
        *chat.feed.borrow_mut() = crate::feed::Feed::new("notes_chats", move || {
            if let Some(chat) = weak.upgrade() {
                chat.remote_changed();
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
        self.fill();
        match self.remote_current.get() {
            Some(id) => self.show_remote(id),
            None => self.new_chat(),
        }
    }

    /// The input of the chat shown, focused (to type at once).
    pub fn focus(&self) {
        if let Some(chat) = self.remote_chat() {
            chat.input.grab_focus();
        }
    }

    /// The chat shown is left: one at work (or waiting for you) keeps going, and goes once it
    /// is neither; else what it changed is recorded and it goes.
    fn set_aside(&self) {
        if self.remote_current.take().is_some() {
            self.close_idle_remote();
        }
    }

    /// A chat waits for an answer.
    pub fn waiting(&self) -> bool {
        self.items().iter().any(|i| i["mark"] == "blocked")
    }

    /// An agent for chat `session` (None: a new chat), about `scope` if given, shown.
    fn chat_for(self: &Rc<Self>, session: Option<String>, scope: Option<Scope>) {
        self.remote_chat_for(session, scope, None)
    }

    /// Over an empty chat: what it is about, and a few beginnings that fill the input.
    fn starters(&self, chat: &crate::chat::Chat, vault: &Path, scope: Option<&Scope>) -> gtk::Box {
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
            let input = chat.input.downgrade();
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

    /// A new chat (the one open, if it is still empty).
    pub fn new_chat(self: &Rc<Self>) {
        match self.remote_current.get().filter(|_| self.remote_chat().is_some_and(|c| !c.has_messages())) {
            Some(id) => self.show_remote(id),
            None => self.remote_chat_for(None, None, None),
        }
    }

    /// A new chat with `text` in its input, for the user to send (or change first).
    pub fn new_chat_saying(self: &Rc<Self>, text: &str) {
        match self.remote_chat().filter(|c| !c.has_messages()) {
            Some(chat) => say_in(&chat, text),
            None => self.remote_chat_for(None, None, Some(text.to_owned())),
        }
    }

    /// A new chat about topic `name`, its notes in `folder`.
    pub fn topic_chat(self: &Rc<Self>, name: &str, folder: &str) {
        self.chat_for(None, Some((name.to_owned(), folder.to_owned())));
    }

    /// The next past chat (`by` 1) or the one before (-1), from the one shown.
    pub fn step(self: &Rc<Self>, by: i32) {
        let session = self.current_session();
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

    /// Past chat `index` of the list.
    fn pick(self: &Rc<Self>, index: usize) {
        let Some(past) = self.pasts.borrow().get(index).cloned() else { return };
        match self.remote_current.get().filter(|id| self.session_of(*id).as_deref() == Some(past.session_id.as_str())) {
            Some(id) => self.show_remote(id),
            None => self.remote_chat_for(Some(past.session_id), None, None),
        }
    }

    fn fill(&self) {
        // Drawn from loomd's `notes_chats`.
        let items = self.items();
        // loomd's: its list is the past chats.
        if let Some(vault) = crate::vault::current()
        {
            *self.pasts.borrow_mut() = items
                .iter()
                .filter_map(|i| Some(Past { session_id: i["id"].as_str()?.to_owned(), cwd: vault.clone(), title: i["title"].as_str().map(str::to_owned), updated: i["updated"].as_str().unwrap_or_default().to_owned() }))
                .collect();
        }
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
        let session = self.current_session();
        let index = session.and_then(|s| self.pasts.borrow().iter().position(|p| p.session_id == s));
        match index.and_then(|i| self.list.row_at_index(i as i32)) {
            Some(row) => self.list.select_row(Some(&row)),
            None => self.list.unselect_all(),
        }
    }
}

impl NotesChat {
    /// `notes_chats` as last read.
    fn items(&self) -> Vec<serde_json::Value> {
        self.feed.borrow().as_ref().map(|feed| {
            feed.update();
            feed.items().clone()
        }).unwrap_or_default()
    }

    /// The conversation of the chat shown, once it has one.
    fn current_session(&self) -> Option<String> {
        self.session_of(self.remote_current.get()?)
    }

    /// The conversation of loomd's chat agent `id`, once it has one (`notes_chats` says).
    fn session_of(&self, id: u32) -> Option<String> {
        let id = id.to_string();
        self.items().iter().find(|i| i["agent"].as_str() == Some(id.as_str()))?["id"].as_str().map(str::to_owned)
    }

    /// The chat of loomd's chat shown, if any.
    pub fn remote_chat(&self) -> Option<Rc<crate::chat::Chat>> {
        let id = self.remote_current.get()?;
        self.remote_open.borrow().iter().find(|o| o.agent.id == id).map(|o| o.agent.chat.clone())
    }

    /// loomd's chat for `session` (None: a new one; `scope`: about that topic), shown; `text`
    /// put in its input. loomd opens it (`notes_open`), then it is drawn here.
    fn remote_chat_for(self: &Rc<Self>, session: Option<String>, scope: Option<Scope>, text: Option<String>) {
        let Some(vault) = crate::vault::current() else { return };
        self.set_aside();
        let open = session.as_ref().and_then(|s| self.remote_open.borrow().iter().map(|o| o.agent.id).find(|id| self.session_of(*id).as_ref() == Some(s)));
        if let Some(id) = open {
            self.remote_current.set(Some(id));
            return self.show_remote(id);
        }
        let mut params = serde_json::json!({});
        if let Some(session) = &session {
            params["session"] = session.as_str().into();
        }
        if let Some((topic, _)) = &scope {
            params["topic"] = topic.as_str().into();
        }
        let weak = Rc::downgrade(self);
        crate::conn::command_then("notes", "notes_open", params, move |answer| {
            let Some(chat) = weak.upgrade() else { return };
            let Some(id) = answer.and_then(|a| a["agent"].as_str().and_then(|a| a.parse::<u32>().ok())) else { return };
            if !chat.remote_open.borrow().iter().any(|o| o.agent.id == id) {
                let agent = crate::remote_agent::RemoteAgent::new(id);
                agent.chat.set_hint(&hint(scope.as_ref()));
                let starters = chat.starters(&agent.chat, &vault, scope.as_ref());
                // They go with the first message sent, not when loomd's list next changes.
                let send = agent.chat.on_submit.borrow().clone();
                let gone = starters.downgrade();
                *agent.chat.on_submit.borrow_mut() = Some(Rc::new(move |message| {
                    if let Some(starters) = gone.upgrade() {
                        starters.set_visible(false);
                    }
                    if let Some(send) = &send {
                        send(message);
                    }
                }));
                let page = gtk::Overlay::builder().child(&agent.chat.root).build();
                page.add_overlay(&starters);
                chat.stack.add_child(&page);
                chat.remote_open.borrow_mut().push(RemoteOpen { agent, page, starters });
            }
            chat.remote_current.set(Some(id));
            chat.show_remote(id);
            if let (Some(text), Some(shown)) = (&text, chat.remote_chat()) {
                say_in(&shown, text);
            }
        });
    }

    /// loomd's chat `id` shown.
    fn show_remote(&self, id: u32) {
        if let Some(shown) = self.on_shown.borrow().clone() {
            shown();
        }
        let session = self.session_of(id);
        let title = session.as_ref().and_then(|s| self.items().iter().find(|i| i["id"].as_str() == Some(s.as_str()))?["title"].as_str().map(str::to_owned));
        if let Some(open) = self.remote_open.borrow().iter().find(|o| o.agent.id == id) {
            self.stack.set_visible_child(&open.page);
            open.starters.set_visible(session.is_none() && !open.agent.chat.has_messages());
            let vault = crate::vault::current().map(|v| v.to_string_lossy().into_owned()).unwrap_or_default();
            open.agent.chat.set_title(title.as_deref().unwrap_or("New chat"), "your notes", &vault);
            open.agent.chat.input.grab_focus();
        }
        self.mark_selected();
    }

    /// loomd's `notes_chats` changed: the list drawn, chats left that rest closed, the Notes
    /// button told whether one waits.
    fn remote_changed(self: &Rc<Self>) {
        self.fill();
        self.close_idle_remote();
        if let Some(id) = self.remote_current.get() {
            self.show_title(id);
        }
        if let Some(attention) = self.on_attention.borrow().clone() {
            attention(self.waiting());
        }
    }

    /// Its title in the header, and the starters gone once it has a conversation.
    fn show_title(&self, id: u32) {
        let session = self.session_of(id);
        if let Some(open) = self.remote_open.borrow().iter().find(|o| o.agent.id == id) {
            open.starters.set_visible(session.is_none() && !open.agent.chat.has_messages());
        }
        let title = session.and_then(|s| self.items().iter().find(|i| i["id"].as_str() == Some(s.as_str()))?["title"].as_str().map(str::to_owned));
        if let (Some(title), Some(chat)) = (title, self.remote_chat()) {
            let vault = crate::vault::current().map(|v| v.to_string_lossy().into_owned()).unwrap_or_default();
            chat.set_title(&title, "your notes", &vault);
        }
    }

    /// loomd's chats left (not shown) that rest, closed there: one never sent to, or one
    /// whose conversation is neither working nor waiting. One sent to that has no
    /// conversation yet is kept until it has (it may be working).
    fn close_idle_remote(&self) {
        let current = self.remote_current.get();
        let items = self.items();
        let resting = |o: &RemoteOpen| {
            let id = o.agent.id.to_string();
            match items.iter().find(|i| i["agent"].as_str() == Some(id.as_str())) {
                Some(item) => item["mark"].is_null(),
                None => !o.agent.chat.has_messages(),
            }
        };
        let gone: Vec<u32> = self.remote_open.borrow().iter().filter(|o| Some(o.agent.id) != current && resting(o)).map(|o| o.agent.id).collect();
        for id in gone {
            let at = self.remote_open.borrow().iter().position(|o| o.agent.id == id);
            if let Some(at) = at {
                let open = self.remote_open.borrow_mut().remove(at);
                self.stack.remove(&open.page);
                crate::conn::command("notes", "notes_close", serde_json::json!({ "agent": id.to_string() }));
            }
        }
    }
}

/// `text` in `chat`'s input, for the user to send (or change first).
fn say_in(chat: &crate::chat::Chat, text: &str) {
    let buffer = chat.input.buffer();
    buffer.set_text(text);
    buffer.place_cursor(&buffer.end_iter());
    chat.input.grab_focus();
}
