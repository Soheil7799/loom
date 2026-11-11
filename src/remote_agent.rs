//! An agent of loomd's, drawn in the window as loomd's client: its chat built from the core
//! API's resources (its `agents` item, `conversation/<id>`, `usage`, `inputs`, `plan/<id>`,
//! `crew/<id>`, `mcp`, `workflows/<id>`, a subagent's `subagent/<id>/<sub>`), its buttons and
//! keys sent back as commands. The window's own agents
//! (the core in this process) are drawn by `chat_view` instead.

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
    /// The taken-back pictures `inputs` listed last (each attached once).
    draft_images: RefCell<Vec<String>>,
    /// Told when it asked loomd for a copy of itself (rewound: the copy is shown once listed).
    pub on_copy: RefCell<Option<Rc<dyn Fn()>>>,
    /// Its workflows' cards, by tool call, each with whether loomd says it still runs.
    runs: RefCell<Vec<WorkflowCard>>,
    me: RefCell<Weak<RemoteAgent>>,
    /// The subagent's conversation open in place of its own: its chat, followed.
    sub: RefCell<Option<SubView>>,
}

/// A subagent's conversation shown: its chat, and its resource followed.
type SubView = (Rc<Chat>, Option<Rc<Follow>>);

/// A workflow's card: its tool call, the card, and whether loomd says it still runs.
type WorkflowCard = (String, Rc<crate::workflow::Run>, Rc<Cell<bool>>);

/// The resources of agent `id`'s that it follows besides its conversation and usage, in the
/// order `feed_changed` knows them.
fn resources(id: u32) -> [String; 5] {
    ["inputs".to_owned(), format!("plan/{id}"), format!("crew/{id}"), "mcp".to_owned(), format!("workflows/{id}")]
}

impl RemoteAgent {
    pub fn new(id: u32) -> Rc<RemoteAgent> {
        let chat = Chat::new();
        chat.draw_from_entries(id);
        chat.set_show_thinking(loom_core::settings::get().show_thinking);
        let agent = Rc::new(RemoteAgent { id, chat, follows: RefCell::default(), feeds: RefCell::default(), item: RefCell::new(Value::Null), since: Cell::new(None), draft: RefCell::default(), draft_images: RefCell::default(), on_copy: RefCell::default(), runs: RefCell::default(), me: RefCell::default(), sub: RefCell::default() });
        *agent.me.borrow_mut() = Rc::downgrade(&agent);
        // Its buttons first: the messages drawn next take their callbacks as they are made.
        agent.wire();
        let mut follows = Vec::new();
        follows.extend(Follow::start(&loom_core::agent::conversation(id), Of::Conversation, &agent.chat));
        follows.extend(Follow::start("usage", Of::Usage(id.to_string()), &agent.chat));
        *agent.follows.borrow_mut() = follows;
        let mut feeds = Vec::new();
        for resource in resources(id) {
            let weak = Rc::downgrade(&agent);
            let name = resource.clone();
            feeds.extend(Feed::new(&resource, move || {
                if let Some(agent) = weak.upgrade() {
                    agent.feed_changed(&name);
                }
            }));
        }
        *agent.feeds.borrow_mut() = feeds;
        for resource in resources(id) {
            agent.feed_changed(&resource);
        }
        agent
    }

    /// Its conversation's entries as they are now.
    pub fn entries(&self) -> Vec<Value> {
        self.follows.borrow().first().map(|f| f.items()).unwrap_or_default()
    }

    /// Its oldest question waiting for the user (the one the keys are shown on).
    fn waiting_question(&self) -> Option<Value> {
        self.entries().into_iter().find(|e| e["kind"] == "question" && e["status"] == "awaiting_user")
    }

    /// Alt+1…9: its oldest question answered with option `option`. False if there is none (or
    /// it is a form: picked in the form itself).
    pub fn answer(&self, option: usize) -> bool {
        let Some(question) = self.waiting_question() else { return false };
        // A form: the option picked in it (sent with Alt+Enter).
        if question["form"].is_object() {
            return self.chat.pick_in_entry_form(option);
        }
        let Some(chosen) = question["options"].get(option).and_then(|o| o["id"].as_str()) else { return false };
        self.send("answer", json!({ "entry": question["id"], "option": chosen }));
        true
    }

    /// Alt+Enter: its oldest question form sent as filled in.
    pub fn send_form(&self) -> bool {
        self.chat.send_entry_form()
    }

    /// Alt+Y / Alt+N: its oldest question's first option of that kind ("allow", "reject").
    pub fn answer_kind(&self, kind: &str) -> bool {
        let Some(question) = self.waiting_question() else { return false };
        let at = question["options"].as_array().and_then(|o| o.iter().position(|o| o["kind"].as_str().is_some_and(|k| k.starts_with(kind))));
        at.is_some_and(|at| self.answer(at))
    }

    /// Shift+Tab: its next permission mode.
    pub fn cycle_mode(&self) {
        let next = {
            let item = self.item.borrow();
            // Bypass is never one key press away (as the core's own cycle).
            let modes: Vec<&str> = item["modes"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|m| *m != "bypassPermissions").collect();
            let at = modes.iter().position(|m| Some(*m) == item["mode"].as_str());
            modes.get(at.map_or(0, |i| (i + 1) % modes.len())).map(|m| (*m).to_owned())
        };
        if let Some(next) = next {
            self.send("set_mode", json!({ "mode": next }));
        }
    }

    /// Command `method` for it; a refusal is said in its chat.
    fn send(&self, method: &str, mut params: Value) {
        params["agent"] = Value::String(self.id.to_string());
        let chat = Rc::downgrade(&self.chat);
        crate::conn::command_result(&format!("agent {}", self.id), method, params, move |result| {
            if let (Err(err), Some(chat)) = (result, chat.upgrade()) {
                chat.notice(&err.to_string(), "error");
            }
        });
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
        self.chat.set_starting((status == Status::Starting).then(|| format!("Starting Claude in {place}…")).as_deref());
        self.chat.set_state(if status == Status::Suspended { "" } else { status.describe() }, status.css());
        self.chat.set_mode(text("mode"));
        if let Some(options) = item.get("options") {
            let chosen: Vec<String> = item["chosen"].as_array().into_iter().flatten().filter_map(|c| c.as_str().map(str::to_owned)).collect();
            self.chat.set_options(options, &chosen);
            crate::prefs::seen_options(options);
        }
        if let Some(cwd) = item["cwd"].as_str() {
            self.chat.set_cwd(std::path::Path::new(cwd));
        }
        let busy = item["busy"].as_bool() == Some(true);
        if busy != self.since.get().is_some() {
            self.since.set(busy.then(Instant::now));
            self.chat.set_working(self.since.get());
        }
    }

    /// The folder it works in (as its `agents` item last said).
    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        self.item.borrow()["cwd"].as_str().map(std::path::PathBuf::from)
    }

    /// Its project's folder (as its `agents` item last said).
    pub fn project(&self) -> Option<std::path::PathBuf> {
        self.item.borrow()["project_id"].as_str().map(std::path::PathBuf::from)
    }

    /// One of its resources changed: what of it the chat shows, drawn again.
    fn feed_changed(&self, name: &str) {
        let feeds = self.feeds.borrow();
        let id = self.id.to_string();
        let index = resources(self.id).iter().position(|r| r == name);
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
                let given = *self.draft.borrow() != draft && !draft.is_empty() && self.chat.draft().is_empty();
                if given {
                    self.chat.input.buffer().set_text(&draft);
                }
                *self.draft.borrow_mut() = draft;
                // and the pictures of the messages taken back, attached again (once each)
                let images: Vec<String> = mine["draft_images"].as_array().into_iter().flatten().filter_map(|i| i.as_str().map(str::to_owned)).collect();
                let fresh: Vec<String> = images.iter().filter(|i| !self.draft_images.borrow().contains(i)).cloned().collect();
                *self.draft_images.borrow_mut() = images;
                for blob in fresh {
                    let chat = Rc::downgrade(&self.chat);
                    crate::conn::command_result(&format!("agent {}", self.id), "blob", json!({ "id": blob }), move |answer| {
                        let png = answer.ok().and_then(|a| loom_core::bridges::host::decode(a["data"].as_str()?));
                        if let (Some(chat), Some(png)) = (chat.upgrade(), png)
                            && let Ok(texture) = gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_owned(png))
                        {
                            chat.attach(&texture);
                        }
                    });
                }
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
            Some(4) => self.workflows_changed(&items),
            _ => {
                let mine = items.iter().find(|i| i["id"] == id.as_str());
                let servers = mine.map(|m| m["servers"].as_array().into_iter().flatten().map(|s| (s["name"].as_str().unwrap_or_default().to_owned(), s["status"].as_str().unwrap_or_default().to_owned(), s["error"].as_str().map(str::to_owned))).collect());
                self.chat.show_mcp(servers);
            }
        }
    }

    /// Its `workflows`: each a card as the window's own agents' (read from the workflow's
    /// files, on this computer), Stop as stop_workflow; the note under the conversation.
    fn workflows_changed(&self, items: &[Value]) {
        for item in items {
            let Some(tool_id) = item["id"].as_str() else { continue };
            let running = item["running"] == true;
            let known = self.runs.borrow().iter().find(|(id, _, _)| id == tool_id).map(|(_, run, alive)| (run.clone(), alive.clone()));
            let run = match known {
                Some((run, alive)) => {
                    alive.set(running);
                    run
                }
                None => {
                    let alive = Rc::new(Cell::new(running));
                    let launch = crate::workflow::Launch {
                        task: item["task"].as_str().unwrap_or_default().to_owned(),
                        dir: std::path::PathBuf::from(item["dir"].as_str().unwrap_or_default()),
                        name: item["name"].as_str().unwrap_or_default().to_owned(),
                        summary: item["summary"].as_str().unwrap_or_default().to_owned(),
                    };
                    let still = alive.clone();
                    let run = crate::workflow::Run::new(launch, running, move || still.get());
                    let chat = Rc::downgrade(&self.chat);
                    *run.on_open.borrow_mut() = Some(Rc::new(move |path: std::path::PathBuf, label: String| {
                        let Some(chat) = chat.upgrade() else { return };
                        let viewer = Chat::viewer();
                        viewer.set_show_thinking(loom_core::settings::get().show_thinking);
                        crate::workflow::fill_transcript(&viewer, &path);
                        chat.show_aside(viewer.root.upcast_ref(), &format!("Workflow agent · {label}"));
                    }));
                    let (me, workflow) = (self.me.borrow().clone(), tool_id.to_owned());
                    *run.on_stop.borrow_mut() = Some(Rc::new(move || {
                        if let Some(agent) = me.upgrade() {
                            agent.send("stop_workflow", json!({ "workflow": workflow }));
                        }
                    }));
                    let me = self.me.borrow().clone();
                    *run.on_changed.borrow_mut() = Some(Rc::new(move || {
                        if let Some(agent) = me.upgrade() {
                            agent.workflow_note();
                        }
                    }));
                    self.chat.block(&run.root);
                    self.runs.borrow_mut().push((tool_id.to_owned(), run.clone(), alive));
                    run
                }
            };
            run.stop_unanswered(item["unanswered"].as_u64().unwrap_or(0));
            // Over, as loomd says (its files may not say so yet).
            if !running && !run.done() {
                match item["state"].as_str() {
                    Some("stopped") => run.stopped(),
                    Some("failed") => run.ended("failed", None),
                    _ => run.ended("completed", None),
                }
            }
        }
        self.workflow_note();
    }

    /// "Workflow x: 12 of 30 agents done", where "Working…" is between turns, while any runs.
    fn workflow_note(&self) {
        let runs = self.runs.borrow();
        let live: Vec<&Rc<crate::workflow::Run>> = runs.iter().filter(|(_, run, _)| !run.done()).map(|(_, run, _)| run).collect();
        let (done, all) = live.iter().fold((0, 0), |(d, a), run| {
            let (rd, ra) = run.counts();
            (d + rd, a + ra)
        });
        self.chat.set_workflow_note(match live.as_slice() {
            [] => None,
            [one] => Some(format!("Workflow {}: {done} of {all} agents done", one.name)),
            more => Some(format!("{} workflows: {done} of {all} agents done", more.len())),
        });
    }

    /// Subagent `key`'s conversation, in place of its own (loomd's `subagent/<agent>/<key>`);
    /// a task with none of its own: its card in the conversation.
    fn open_sub(&self, key: &str) {
        if let Some(rest) = key.strip_prefix("task:") {
            let call = rest.split_once(':').map_or(rest, |(_, call)| call);
            self.chat.go_to_tool(call);
            return;
        }
        let viewer = Chat::viewer();
        viewer.set_show_thinking(loom_core::settings::get().show_thinking);
        let follow = Follow::start(&format!("subagent/{}/{key}", self.id), Of::Conversation, &viewer);
        let crew = self.feeds.borrow().get(2).map(|f| f.items().clone()).unwrap_or_default();
        let name = crew.iter().find(|c| c["id"] == key).and_then(|c| c["name"].as_str().map(str::to_owned)).unwrap_or_else(|| "Subagent".to_owned());
        self.chat.show_aside(viewer.root.upcast_ref(), &format!("Subagent · {name}"));
        *self.sub.borrow_mut() = Some((viewer, follow));
    }

    /// The chat's buttons and keys, as commands to loomd.
    fn wire(self: &Rc<Self>) {
        let chat = &self.chat;
        // What is typed is loomd's draft too (kept when the window closes), a moment after the
        // typing stops; sending empties the input, and so the draft (it came back each time).
        let typing: Rc<RefCell<Option<gtk::glib::SourceId>>> = Rc::default();
        let weak = Rc::downgrade(self);
        chat.input.buffer().connect_changed(move |_| {
            let weak = weak.clone();
            let slot = typing.clone();
            let source = gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                slot.borrow_mut().take();
                let Some(agent) = weak.upgrade() else { return };
                let text = agent.chat.draft();
                if *agent.draft.borrow() != text {
                    *agent.draft.borrow_mut() = text.clone();
                    agent.send("set_draft", json!({ "text": text }));
                }
            });
            if let Some(old) = typing.borrow_mut().replace(source) {
                old.remove();
            }
        });
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
        *chat.on_cycle_mode.borrow_mut() = Some(me(RemoteAgent::cycle_mode));
        let weak = Rc::downgrade(self);
        *chat.on_submit.borrow_mut() = Some(Rc::new(move |(text, images): (String, Vec<gtk::glib::Bytes>)| {
            if let Some(agent) = weak.upgrade() {
                let images: Vec<String> = images.iter().map(|b| loom_core::api::wire::base64(b)).collect();
                // Refused (loomd restarting, the link down): the text goes back into the
                // input it was taken from, unless something was typed there since.
                let chat = Rc::downgrade(&agent.chat);
                let params = json!({ "text": text, "images": images, "agent": agent.id.to_string() });
                crate::conn::command_result(&format!("agent {}", agent.id), "send", params, move |result| {
                    if let (Err(err), Some(chat)) = (result, chat.upgrade()) {
                        chat.notice(&err.to_string(), "error");
                        if chat.draft().is_empty() {
                            chat.input.buffer().set_text(&text);
                        }
                    }
                });
            }
        }));
        let weak = Rc::downgrade(self);
        *chat.on_rewind_entry.borrow_mut() = Some(Rc::new(move |(entry, files): (String, bool)| {
            if let Some(agent) = weak.upgrade() {
                agent.send("rewind", json!({ "entry": entry, "files": files }));
                let told = agent.on_copy.borrow().clone();
                if let Some(told) = told {
                    told();
                }
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
        *chat.on_crew_open.borrow_mut() = Some(Rc::new(move |key: String| {
            if let Some(agent) = weak.upgrade() {
                agent.open_sub(&key);
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
