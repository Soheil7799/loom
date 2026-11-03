//! The bridge protocol (docs/bridges.md) on top of the core API (warp, step 4): a bridge asks
//! and hears as it always has, while underneath it follows `agents` and each agent's
//! `conversation/<id>` through the connection loom handed it down, and acts by the API's
//! commands, as the bridge loom says it is.
//!
//! - Agents' views (`{id, seq, title, project, status, unseen, busy, questions}`) are made
//!   from the `agents` items and the questions waiting in each conversation; a question's
//!   `serial` is its entry's number (`e12` → 12).
//! - Events are worked out from what changed: `agent` (any change), `asked` (a new
//!   question), `done` (a turn ended: `last_turn` grew), `failed`, `unsent`, `closed`.
//! - A form is filled in here (`agent::form`, as loom would), sent whole by `answer`.
//!
//! Runs on the bridge's core thread (`rt`).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::Duration;

use serde_json::{Value, json};

use crate::agent::form::FormModel;
use crate::api::shared::Shared;
use crate::api::{Applied, Message, Mirror};

/// What comes together is worked out together (a turn's end touches both resources).
const SETTLE: Duration = Duration::from_millis(60);

pub struct Link {
    api: Shared,
    /// This bridge's own settings, as loom gave them on stdin.
    settings: Value,
    agents: RefCell<Mirror>,
    agents_sub: RefCell<String>,
    /// Each agent's conversation: its subscription and copy.
    chats: RefCell<HashMap<String, (String, Mirror)>>,
    /// Forms being filled in: (agent, serial).
    forms: RefCell<HashMap<(String, u64), FormModel>>,
    /// The agents as last worked out (for what changed), and the questions told of.
    before: RefCell<Option<HashMap<String, Value>>>,
    asked: RefCell<HashSet<(String, u64)>>,
    /// Each agent's view as last told, but for its `seq` (a view told again unchanged isn't).
    told: RefCell<HashMap<String, String>>,
    seq: Cell<u64>,
    events: async_channel::Sender<Value>,
    scheduled: Cell<bool>,
    me: Weak<Link>,
}

impl Link {
    /// Over `stream` (the bridge's handed-down connection) for bridge `name`; the events go to
    /// the channel returned.
    pub fn start(stream: std::os::unix::net::UnixStream, name: &str, settings: Value) -> Result<(Rc<Link>, async_channel::Receiver<Value>), String> {
        let (messages, incoming) = async_channel::unbounded::<Message>();
        let api = Shared::new(stream, &format!("loom bridge {name}"), messages)?;
        let (events, heard) = async_channel::unbounded();
        let link = Rc::new_cyclic(|me| Link {
            api,
            settings,
            agents: RefCell::default(),
            agents_sub: RefCell::default(),
            chats: RefCell::default(),
            forms: RefCell::default(),
            before: RefCell::default(),
            asked: RefCell::default(),
            told: RefCell::default(),
            seq: Cell::new(0),
            events,
            scheduled: Cell::new(false),
            me: me.clone(),
        });
        let sub = link.subscribe("agents")?;
        *link.agents_sub.borrow_mut() = sub;
        let weak = Rc::downgrade(&link);
        crate::rt::spawn_local(async move {
            while let Ok(message) = incoming.recv().await {
                let Some(link) = weak.upgrade() else { break };
                link.apply(&message);
            }
            // loom has gone: so does the bridge, as with the old socket.
            if let Some(link) = weak.upgrade() {
                link.events.close();
            }
        });
        Ok((link, heard))
    }

    fn subscribe(&self, resource: &str) -> Result<String, String> {
        let answer = self.api.call("subscribe", json!({ "resource": resource }))?;
        Ok(answer["sub"].as_str().unwrap_or_default().to_owned())
    }

    /// One subscription message: into its copy; what changed is worked out shortly.
    fn apply(&self, message: &Message) {
        let applied = if *self.agents_sub.borrow() == message.sub {
            self.agents.borrow_mut().apply(message)
        } else {
            let mut chats = self.chats.borrow_mut();
            let Some((_, mirror)) = chats.values_mut().find(|(sub, _)| *sub == message.sub) else { return };
            mirror.apply(message)
        };
        if applied == Applied::Gap {
            let _ = self.api.call("resync", json!({ "sub": message.sub }));
        }
        if !self.scheduled.replace(true) {
            let me = self.me.clone();
            let _ = crate::rt::timeout_once(SETTLE, move || {
                if let Some(link) = me.upgrade() {
                    link.scheduled.set(false);
                    link.settle();
                }
            });
        }
    }

    /// Follows the agents' conversations, then tells what changed since last time.
    fn settle(&self) {
        let items: Vec<Value> = self.agents.borrow().items.clone();
        if !self.agents.borrow().synced {
            return;
        }
        // Conversations followed for the agents there are, and only those.
        let ids: Vec<String> = items.iter().filter_map(|a| a["id"].as_str().map(str::to_owned)).collect();
        for id in &ids {
            if !self.chats.borrow().contains_key(id)
                && let Ok(sub) = self.subscribe(&format!("conversation/{id}"))
            {
                self.chats.borrow_mut().insert(id.clone(), (sub, Mirror::default()));
            }
        }
        let gone: Vec<(String, String)> = self.chats.borrow().iter().filter(|(id, _)| !ids.contains(id)).map(|(id, (sub, _))| (id.clone(), sub.clone())).collect();
        for (id, sub) in gone {
            self.chats.borrow_mut().remove(&id);
            let _ = self.api.call("unsubscribe", json!({ "sub": sub }));
        }
        // A conversation just followed: its questions come with its snapshot, next time.
        let waiting = self.chats.borrow().values().any(|(_, m)| !m.synced);
        let now: HashMap<String, Value> = items.iter().filter_map(|a| Some((a["id"].as_str()?.to_owned(), a.clone()))).collect();
        let Some(before) = self.before.replace(Some(now.clone())) else {
            // The first time: nothing happened, it just is (the agents and their open
            // questions are known).
            for item in &items {
                let Some(id) = item["id"].as_str() else { continue };
                for serial in self.questions_of(id).iter().map(|(s, _)| *s) {
                    self.asked.borrow_mut().insert((id.to_owned(), serial));
                }
                let view = self.view_of(item);
                self.told.borrow_mut().insert(id.to_owned(), unnumbered(&view));
                self.emit(json!({ "event": "agent", "agent": view, "looking": item["looking"] }));
            }
            return;
        };
        for id in before.keys().filter(|id| !now.contains_key(*id)) {
            self.forms.borrow_mut().retain(|(agent, _), _| agent != id);
            self.asked.borrow_mut().retain(|(agent, _)| agent != id);
            self.told.borrow_mut().remove(id);
            self.emit(json!({ "event": "closed", "agent": id.parse::<u64>().unwrap_or(0) }));
        }
        for item in &items {
            let Some(id) = item["id"].as_str() else { continue };
            let old = before.get(id);
            let view = self.view_of(item);
            let looking = item["looking"].as_bool().unwrap_or(false);
            let grew = |field: &str, count: &str| item[field][count].as_u64().unwrap_or(0) > old.map_or(0, |o| o[field][count].as_u64().unwrap_or(0));
            let questions = self.questions_of(id);
            let open: HashSet<(String, u64)> = questions.iter().map(|(s, _)| (id.to_owned(), *s)).collect();
            let new = open.iter().any(|q| !self.asked.borrow().contains(q));
            self.asked.borrow_mut().retain(|(agent, s)| agent != id || open.contains(&(agent.clone(), *s)));
            self.asked.borrow_mut().extend(open);
            if self.told.borrow_mut().insert(id.to_owned(), unnumbered(&view)).as_deref() != Some(unnumbered(&view).as_str()) {
                self.emit(json!({ "event": "agent", "agent": view, "looking": looking }));
            }
            if new && !waiting {
                self.emit(json!({ "event": "asked", "agent": view, "looking": looking }));
            }
            if grew("last_turn", "turns") {
                let turn = &item["last_turn"];
                self.emit(json!({
                    "event": "done",
                    "agent": view,
                    "looking": looking,
                    "reply": self.last_reply(id),
                    "took": crate::paths::elapsed(turn["seconds"].as_u64().unwrap_or(0)),
                    "error": turn["error"],
                    "cancelled": turn["cancelled"].as_bool().unwrap_or(false),
                }));
            }
            if grew("failed", "times") {
                self.emit(json!({ "event": "failed", "agent": view, "looking": looking, "why": item["failed"]["why"] }));
            }
            if grew("unsent", "times") {
                self.emit(json!({ "event": "unsent", "agent": view, "looking": looking, "count": item["unsent"]["count"] }));
            }
        }
    }

    fn emit(&self, event: Value) {
        let _ = self.events.try_send(event);
    }

    /// Agent `id`'s questions waiting for an answer: (serial, entry).
    fn questions_of(&self, id: &str) -> Vec<(u64, Value)> {
        let chats = self.chats.borrow();
        let Some((_, mirror)) = chats.get(id) else { return Vec::new() };
        mirror
            .items
            .iter()
            .filter(|e| e["kind"] == "question" && e["status"] == "awaiting_user")
            .filter_map(|e| Some((e["id"].as_str()?.strip_prefix('e')?.parse().ok()?, e.clone())))
            .collect()
    }

    /// A question as bridges know it (`asking_json`'s shape).
    fn asking(&self, agent: &str, serial: u64, entry: &Value) -> Value {
        let options: Vec<Value> = entry["options"].as_array().into_iter().flatten().map(|o| json!([o["name"], o["kind"]])).collect();
        let mut asking = json!({ "serial": serial, "title": entry["text"], "detail": entry["detail"], "options": options, "fields": [], "is_form": false, "writable": false, "summary": "", "blank": false });
        if entry["form"].is_object() {
            let mut forms = self.forms.borrow_mut();
            let form = forms.entry((agent.to_owned(), serial)).or_insert_with(|| FormModel::from_schema(&entry["form"]));
            asking["fields"] = json!(form.choices());
            asking["is_form"] = json!(true);
            asking["writable"] = json!(form.writable());
            asking["summary"] = json!(form.summary());
            asking["blank"] = json!(form.content().is_ok_and(|c| c.as_object().is_none_or(|o| o.is_empty())));
        }
        asking
    }

    fn view_of(&self, item: &Value) -> Value {
        self.seq.set(self.seq.get() + 1);
        let id = item["id"].as_str().unwrap_or_default();
        let questions: Vec<Value> = self.questions_of(id).iter().map(|(serial, entry)| self.asking(id, *serial, entry)).collect();
        json!({
            "id": id.parse::<u64>().unwrap_or(0),
            "seq": self.seq.get(),
            "title": item["title"],
            "project": item["project"],
            "notes": false,
            "status": item["status"],
            "unseen": item["unseen"],
            "busy": item["busy"],
            "questions": questions,
        })
    }

    fn view(&self, id: &str) -> Result<Value, String> {
        let item = self.agents.borrow().items.iter().find(|a| a["id"] == id).cloned();
        item.map(|item| self.view_of(&item)).ok_or_else(|| "That agent is closed.".to_owned())
    }

    /// Its last reply (Markdown): the conversation's last agent text.
    fn last_reply(&self, id: &str) -> Value {
        let chats = self.chats.borrow();
        let reply = chats.get(id).and_then(|(_, m)| m.items.iter().rev().find(|e| e["kind"] == "agent").and_then(|e| e["text"].as_str().map(str::to_owned)));
        json!(reply)
    }

    /// One bridge request (docs/bridges.md), answered from the API.
    pub fn call(&self, method: &str, p: &Value) -> Result<Value, String> {
        let agent = p["agent"].as_u64().map(|a| a.to_string()).unwrap_or_default();
        let serial = p["serial"].as_u64().unwrap_or(0);
        match method {
            "hello" => Ok(json!({ "settings": self.settings, "agents": self.call("agents", p)? })),
            "agents" => {
                let items = self.agents.borrow().items.clone();
                Ok(Value::Array(items.iter().map(|item| self.view_of(item)).collect()))
            }
            "send" => {
                self.api.call("send", json!({ "agent": agent, "text": p["text"], "images": p["images"] }))?;
                self.view(&agent)
            }
            "save" => Ok(self.api.call("save", json!({ "agent": agent, "name": p["name"], "data": p["data"] }))?["path"].clone()),
            "stop" => {
                self.api.call("stop", json!({ "agent": agent }))?;
                self.view(&agent)
            }
            "last_reply" => Ok(self.last_reply(&agent)),
            "answer" => {
                let entry = self.questions_of(&agent).into_iter().find(|(s, _)| *s == serial).map(|(_, e)| e);
                let Some(entry) = entry else { return Ok(json!({ "answered": false, "agent": self.view(&agent)? })) };
                let option = &entry["options"][p["option"].as_u64().unwrap_or(0) as usize];
                if option["kind"] == "allow_always" {
                    return Err("Not from outside loom.".to_owned());
                }
                let answered = self.api.call("answer", json!({ "agent": agent, "entry": entry["id"], "option": option["id"] })).is_ok();
                Ok(json!({ "answered": answered, "agent": self.view(&agent)? }))
            }
            "form" => {
                let entry = self.questions_of(&agent).into_iter().find(|(s, _)| *s == serial).map(|(_, e)| e);
                let Some(entry) = entry.filter(|e| e["form"].is_object()) else {
                    return Ok(json!({ "done": false, "problem": "Already answered.", "agent": self.view(&agent)? }));
                };
                // Made as `view` would, if it hasn't been yet.
                self.asking(&agent, serial, &entry);
                let key = (agent.clone(), serial);
                let (done, problem) = match p["do"].as_str().unwrap_or("") {
                    "pick" => (self.forms.borrow_mut().get_mut(&key).is_some_and(|f| f.pick_in(p["field"].as_u64().unwrap_or(0) as usize, p["option"].as_u64().unwrap_or(0) as usize).is_some()), None),
                    "write" => (self.forms.borrow_mut().get_mut(&key).is_some_and(|f| f.write(p["text"].as_str().unwrap_or("")).is_some()), None),
                    "send" => {
                        let content = self.forms.borrow().get(&key).map(FormModel::content);
                        match content {
                            Some(Ok(content)) => match self.api.call("answer", json!({ "agent": agent, "entry": entry["id"], "content": content })) {
                                Ok(_) => (true, None),
                                Err(why) => (false, Some(why)),
                            },
                            Some(Err(lacking)) => (false, Some(lacking)),
                            None => (false, Some("Already answered.".to_owned())),
                        }
                    }
                    "skip" => match self.api.call("answer", json!({ "agent": agent, "entry": entry["id"], "skip": true })) {
                        Ok(_) => (true, None),
                        Err(why) => (false, Some(why)),
                    },
                    other => return Err(format!("no form action {other}")),
                };
                if done && matches!(p["do"].as_str(), Some("send" | "skip")) {
                    self.forms.borrow_mut().remove(&key);
                }
                Ok(json!({ "done": done, "problem": problem, "agent": self.view(&agent)? }))
            }
            "transcribe" => Ok(self.api.call("transcribe", json!({ "audio": p["audio"], "name": p["name"], "mime": p["mime"] }))?["text"].clone()),
            "state" => self.api.call("bridge_state", p.clone()),
            other => Err(format!("unknown request {other}")),
        }
    }

    /// For a request made off the core's thread (speech to text, which takes a while): the
    /// API connection alone.
    pub fn shared(&self) -> Shared {
        self.api.clone()
    }
}

/// A view but for its `seq`, to tell whether it changed.
fn unnumbered(view: &Value) -> String {
    let mut view = view.clone();
    view["seq"] = Value::Null;
    view.to_string()
}
