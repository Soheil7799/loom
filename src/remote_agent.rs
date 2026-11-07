//! An agent of loomd's, drawn in the window as loomd's client: its chat built from the core
//! API's resources (its `agents` item, `conversation/<id>`, `usage`, `inputs`, `plan/<id>`,
//! `crew/<id>`, `mcp`), its buttons and keys sent back as commands. The window's own agents
//! (the core in this process) are drawn by `chat_view` instead.
//!
//! Not yet: rewinding from a message (the chat says the agent's message before it; the API
//! takes yours), a subagent's own conversation, workflows' cards, tool cards' buttons.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use loom_core::agent::Status;
use loom_core::agent::view::Crew;
use serde_json::{Value, json};

use crate::chat::Chat;
use crate::chat_view::{Follow, Of};
use crate::feed::Feed;

pub struct RemoteAgent {
    pub id: u32,
    pub chat: Rc<Chat>,
    /// Its conversation and usage, drawn as they change.
    follows: RefCell<Vec<Rc<Follow>>>,
    /// Its input's, plan's, subagents' and tool servers' resources.
    feeds: RefCell<Vec<Feed>>,
    /// Its `agents` item as last drawn, and since when it works.
    item: RefCell<Value>,
    since: Cell<Option<Instant>>,
    /// What its input held last as `inputs` gave it (a new draft goes in an empty input).
    draft: RefCell<String>,
}

impl RemoteAgent {
    pub fn new(id: u32) -> Rc<RemoteAgent> {
        let chat = Chat::new();
        chat.draw_from_entries(id);
        let agent = Rc::new(RemoteAgent { id, chat, follows: RefCell::default(), feeds: RefCell::default(), item: RefCell::new(Value::Null), since: Cell::new(None), draft: RefCell::default() });
        let mut follows = Vec::new();
        follows.extend(Follow::start(&loom_core::agent::conversation(id), Of::Conversation, &agent.chat));
        follows.extend(Follow::start("usage", Of::Usage(id.to_string()), &agent.chat));
        *agent.follows.borrow_mut() = follows;
        let mut feeds = Vec::new();
        for resource in ["inputs".to_owned(), format!("plan/{id}"), format!("crew/{id}"), "mcp".to_owned()] {
            let weak = Rc::downgrade(&agent);
            let name = resource.clone();
            feeds.extend(Feed::new(&resource, move || {
                if let Some(agent) = weak.upgrade() {
                    agent.feed_changed(&name);
                }
            }));
        }
        *agent.feeds.borrow_mut() = feeds;
        for resource in ["inputs".to_owned(), format!("plan/{id}"), format!("crew/{id}"), "mcp".to_owned()] {
            agent.feed_changed(&resource);
        }
        agent.wire();
        agent
    }

    fn send(&self, method: &str, mut params: Value) {
        params["agent"] = Value::String(self.id.to_string());
        crate::conn::command(&format!("agent {}", self.id), method, params);
    }

    /// Its `agents` item, drawn: the header (title, where, state, mode, options) and whether
    /// it works.
    pub fn update(&self, item: &Value) {
        if *self.item.borrow() == *item {
            return;
        }
        *self.item.borrow_mut() = item.clone();
        let text = |key: &str| item[key].as_str().unwrap_or_default();
        let place = if text("place").is_empty() { text("project").to_owned() } else { format!("{} / {}", text("project"), text("place")) };
        self.chat.set_title(text("title"), &place, &text("project_id").replace(&*loom_core::paths::home().to_string_lossy(), "~"));
        let status = Status::from_css(text("status"));
        self.chat.set_state(if status == Status::Suspended { "" } else { status.describe() }, status.css());
        self.chat.set_mode(text("mode"));
        if let Some(options) = item.get("options") {
            self.chat.set_options(options);
        }
        let busy = item["busy"].as_bool() == Some(true);
        if busy != self.since.get().is_some() {
            self.since.set(busy.then(Instant::now));
            self.chat.set_working(self.since.get());
        }
    }

    /// Its project's folder (as its `agents` item last said).
    pub fn project(&self) -> Option<std::path::PathBuf> {
        self.item.borrow()["project_id"].as_str().map(std::path::PathBuf::from)
    }

    /// One of its resources changed: what of it the chat shows, drawn again.
    fn feed_changed(&self, name: &str) {
        let feeds = self.feeds.borrow();
        let id = self.id.to_string();
        let index = ["inputs".to_owned(), format!("plan/{id}"), format!("crew/{id}"), "mcp".to_owned()].iter().position(|r| r == name);
        let Some(feed) = index.and_then(|i| feeds.get(i)) else { return };
        feed.update();
        let items = feed.items();
        match index {
            Some(0) => {
                let Some(mine) = items.iter().find(|i| i["id"] == id.as_str()) else { return };
                let commands = mine["commands"].as_array().into_iter().flatten().map(|c| (c["name"].as_str().unwrap_or_default().to_owned(), c["about"].as_str().unwrap_or_default().to_owned())).collect();
                self.chat.set_commands(commands);
                self.chat.set_suggestion(mine["suggestion"].as_str().filter(|s| !s.is_empty()).map(str::to_owned));
                let queued: Vec<String> = mine["queued"].as_array().into_iter().flatten().filter_map(|q| q.as_str().map(str::to_owned)).collect();
                self.chat.show_queued(&queued);
                // A draft given (a rewind's message, one taken back) goes into an empty input.
                let draft = mine["draft"].as_str().unwrap_or_default().to_owned();
                if *self.draft.borrow() != draft && !draft.is_empty() && self.chat.draft().is_empty() {
                    self.chat.input.buffer().set_text(&draft);
                }
                *self.draft.borrow_mut() = draft;
            }
            Some(1) => self.chat.set_plan(&Value::Array(items.clone())),
            Some(2) => {
                self.chat.crew_clear();
                for c in items.iter() {
                    let ago = |key: &str| loom_core::api::seconds(&c[key]).map_or(Instant::now(), |t| Instant::now() - Duration::from_secs((std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) - t).max(0) as u64));
                    self.chat.crew_set(Crew {
                        key: c["id"].as_str().unwrap_or_default().to_owned(),
                        name: c["name"].as_str().unwrap_or_default().to_owned(),
                        step: c["step"].as_str().unwrap_or_default().to_owned(),
                        at: ago("at"),
                        started: ago("started"),
                        state: c["state"].as_str().map(str::to_owned),
                        ended: c.get("ended").filter(|e| !e.is_null()).map(|_| ago("ended")),
                        guessed: c["guessed"].as_bool() == Some(true),
                    });
                }
            }
            _ => {
                let mine = items.iter().find(|i| i["id"] == id.as_str());
                let servers = mine.map(|m| m["servers"].as_array().into_iter().flatten().map(|s| (s["name"].as_str().unwrap_or_default().to_owned(), s["status"].as_str().unwrap_or_default().to_owned(), s["error"].as_str().map(str::to_owned))).collect());
                self.chat.show_mcp(servers);
            }
        }
    }

    /// The chat's buttons and keys, as commands to loomd.
    fn wire(self: &Rc<Self>) {
        let chat = &self.chat;
        let me = |f: fn(&RemoteAgent)| {
            let weak: Weak<RemoteAgent> = Rc::downgrade(self);
            Rc::new(move |()| {
                if let Some(agent) = weak.upgrade() {
                    f(&agent);
                }
            })
        };
        *chat.on_cancel.borrow_mut() = Some(me(|a| a.send("stop", json!({}))));
        *chat.on_send_now.borrow_mut() = Some(me(|a| a.send("send_now", json!({}))));
        *chat.on_cycle_mode.borrow_mut() = Some(me(|a| {
            let item = a.item.borrow();
            let modes: Vec<&str> = item["modes"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let at = modes.iter().position(|m| Some(*m) == item["mode"].as_str());
            if let Some(next) = modes.get(at.map_or(0, |i| (i + 1) % modes.len())) {
                a.send("set_mode", json!({ "mode": next }));
            }
        }));
        let weak = Rc::downgrade(self);
        *chat.on_submit.borrow_mut() = Some(Rc::new(move |(text, images): (String, Vec<gtk::glib::Bytes>)| {
            if let Some(agent) = weak.upgrade() {
                let images: Vec<String> = images.iter().map(|b| loom_core::api::wire::base64(b)).collect();
                agent.send("send", json!({ "text": text, "images": images }));
            }
        }));
        let weak = Rc::downgrade(self);
        *chat.on_unqueue.borrow_mut() = Some(Rc::new(move |index: usize| {
            if let Some(agent) = weak.upgrade() {
                agent.send("unqueue", json!({ "index": index }));
            }
        }));
        let weak = Rc::downgrade(self);
        *chat.on_option.borrow_mut() = Some(Rc::new(move |(option, value): (String, String)| {
            if let Some(agent) = weak.upgrade() {
                agent.send("set_option", json!({ "option": option, "value": value }));
            }
        }));
        let weak = Rc::downgrade(self);
        *chat.on_crew_stop.borrow_mut() = Some(Rc::new(move |key: String| {
            if let Some(agent) = weak.upgrade() {
                agent.send("stop_subagent", json!({ "subagent": key }));
            }
        }));
        let weak = Rc::downgrade(self);
        *chat.on_mcp.borrow_mut() = Some(Rc::new(move |(action, server, on): (String, String, bool)| {
            let act = match action.as_str() {
                "reconnect" => "reconnect",
                "toggle" if on => "on",
                "toggle" => "off",
                _ => return,
            };
            if let Some(agent) = weak.upgrade() {
                agent.send("mcp_server", json!({ "server": server, "act": act }));
            }
        }));
    }
}
