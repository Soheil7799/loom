//! The Telegram bridge (`loom bridge telegram`), which loom runs while its settings switch it
//! on. It lists the agents, tells their finished replies and their questions, and takes
//! messages and answers for them: from the one Telegram account the settings allow, and
//! nobody else.
//!
//! Two threads talk to Telegram (one waits for updates, one sends); everything else happens on
//! this process's main loop. It reaches the agents only through loom's bridge socket, and
//! nothing listens on this machine: the bot asks Telegram.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use gtk::{gio, glib};
use serde_json::{Value, json};

use super::Link;
use super::client::Client;
use crate::agent::{Asking, Status};

/// Telegram takes 4096 characters a message; a long reply goes in parts of this many.
const PART: usize = 3500;

/// What a sent message is about, for replies and edits.
#[derive(Clone, Copy)]
enum About {
    Nothing,
    Agent(u32),
    /// An agent's question (agent, serial).
    Question(u32, u64),
}

enum In {
    Update(Value),
    /// A message sent: its id.
    Sent(About, i64),
    Online(String),
    /// Telegram can't be reached, or refuses the token.
    Problem(String),
    /// A message that could not be sent (the bot is still connected): why, and what it was about.
    Unsent(String, About),
}

struct Call {
    method: &'static str,
    body: Value,
    about: About,
    /// Formatted text's plain version, sent instead if Telegram can't read the formatting.
    plain: Option<String>,
}

// The menu under the chat (a keyboard of Telegram's: each button sends its text).
const AGENTS: &str = "🧭 Agents";
const NEEDS: &str = "🟠 Needs me";
const STOP: &str = "⏹ Stop";
const LAST: &str = "📄 Last reply";
const BACK: &str = "⬅ Back";

/// An agent as loom last told it.
#[derive(Clone)]
struct View {
    id: u32,
    /// Newer views have higher ones (an event may arrive after a fresher answer).
    seq: u64,
    title: String,
    project: String,
    status: Status,
    unseen: bool,
    busy: bool,
    questions: Vec<(u64, Asking)>,
}

impl View {
    fn from(v: &Value) -> Option<View> {
        Some(View {
            id: v["id"].as_u64()? as u32,
            seq: v["seq"].as_u64().unwrap_or(0),
            title: v["title"].as_str().unwrap_or("").to_owned(),
            project: v["project"].as_str().unwrap_or("").to_owned(),
            status: Status::from_css(v["status"].as_str().unwrap_or("")),
            unseen: v["unseen"].as_bool().unwrap_or(false),
            busy: v["busy"].as_bool().unwrap_or(false),
            questions: v["questions"].as_array().into_iter().flatten().map(|q| (q["serial"].as_u64().unwrap_or(0), super::asking_from(q))).collect(),
        })
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn status(&self) -> Status {
        self.status
    }

    fn is_busy(&self) -> bool {
        self.busy
    }

    fn open_questions(&self) -> Vec<u64> {
        self.questions.iter().map(|(serial, _)| *serial).collect()
    }

    fn asking(&self, serial: u64) -> Option<Asking> {
        self.questions.iter().find(|(s, _)| *s == serial).map(|(_, a)| a.clone())
    }
}

struct Bot {
    loom: Client,
    /// The agents as loom last told them, in its sidebar's order.
    agents: RefCell<Vec<View>>,
    /// Speech to text is set up in loom.
    speech: Cell<bool>,
    /// Bumped to stop the running threads (a new token, switched off).
    generation: Arc<AtomicU64>,
    /// The next update to ask for, kept across restarts with the same token (0: not known).
    offset: Arc<AtomicI64>,
    calls: RefCell<Option<async_channel::Sender<Call>>>,
    /// What it was started with: (token, allowed account).
    running: RefCell<Option<(String, i64)>>,
    /// The token the bookkeeping below is about.
    token: RefCell<String>,
    user: Cell<i64>,
    /// Linking an account: what to send the bot, shown only in the settings.
    code: RefCell<Option<String>>,
    /// Just linked: say so once connected.
    greet: Cell<bool>,
    /// The account that sent the code, waiting for the user to link it in loom.
    candidate: RefCell<Option<(i64, String)>>,
    /// "Connected as @bot." while it is.
    online: RefCell<Option<String>>,
    /// Who plain messages go to.
    talking_to: Cell<Option<u32>>,
    /// Message id → the agent it is about.
    about: RefCell<HashMap<i64, u32>>,
    /// Questions told: (agent, serial) → their message (None while it is being sent) and text.
    told: RefCell<HashMap<(u32, u64), (Option<i64>, String)>>,
    /// The question being answered from here (not "answered in loom").
    answering: Cell<Option<(u32, u64)>>,
    /// The agents keyboard's buttons: their text → the agent.
    labels: RefCell<HashMap<String, u32>>,
    /// The agent whose "typing…" shows (while it works on a message from here), and its timer.
    typing: Cell<Option<u32>>,
    typing_timer: RefCell<Option<glib::SourceId>>,
    /// Stopped from here (and when): its turn's end is not told again.
    stopped_here: Cell<Option<(u32, std::time::Instant)>>,
}

impl Bot {
    fn new(loom: Client) -> Rc<Bot> {
        Rc::new(Bot {
            loom,
            agents: RefCell::default(),
            speech: Cell::new(false),
            generation: Arc::new(AtomicU64::new(0)),
            offset: Arc::new(AtomicI64::new(0)),
            calls: RefCell::default(),
            running: RefCell::default(),
            token: RefCell::default(),
            user: Cell::new(0),
            code: RefCell::default(),
            greet: Cell::new(false),
            candidate: RefCell::default(),
            online: RefCell::default(),
            talking_to: Cell::new(None),
            about: RefCell::default(),
            told: RefCell::default(),
            answering: Cell::new(None),
            labels: RefCell::default(),
            typing: Cell::new(None),
            typing_timer: RefCell::default(),
            stopped_here: Cell::new(None),
        })
    }

    /// Starts or restarts it as loom's settings say (loom stops the bridge when it is off).
    fn apply(self: &Rc<Self>, s: &Value) {
        let token = s["token"].as_str().unwrap_or("").trim().to_owned();
        let wanted = (!token.is_empty()).then(|| (token.clone(), s["user"].as_i64().unwrap_or(0)));
        if *self.running.borrow() == wanted {
            return;
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.calls.borrow_mut().take();
        // Off until now: what was sent meanwhile is not acted on (as at a first start).
        if self.running.borrow().is_none() {
            self.offset.store(0, Ordering::SeqCst);
        }
        *self.running.borrow_mut() = wanted.clone();
        let Some((token, user)) = wanted else {
            self.set_state(Link::NeedsToken);
            return;
        };
        // Another bot: what was told was told by the old one.
        if *self.token.borrow() != token {
            *self.token.borrow_mut() = token.clone();
            self.offset.store(0, Ordering::SeqCst);
            self.told.borrow_mut().clear();
            self.about.borrow_mut().clear();
            self.talking_to.set(None);
        }
        // Questions still on their way when stopped never get there: told again later.
        self.told.borrow_mut().retain(|_, (message, _)| message.is_some());
        // Another account is another chat: its message ids mean other messages.
        if user != self.user.get() {
            self.talking_to.set(None);
            self.told.borrow_mut().clear();
            self.about.borrow_mut().clear();
        }
        if self.user.get() == 0 && user != 0 && self.candidate.borrow().as_ref().is_some_and(|(id, _)| *id == user) {
            self.greet.set(true);
        }
        self.candidate.borrow_mut().take();
        self.user.set(user);
        *self.code.borrow_mut() = (user == 0).then(|| super::random(8));
        self.online.borrow_mut().take();
        self.set_state(Link::Connecting);
        let generation = self.generation.load(Ordering::SeqCst);
        let (to_main, from_threads) = async_channel::unbounded::<In>();
        let (calls, to_send) = async_channel::unbounded::<Call>();
        *self.calls.borrow_mut() = Some(calls);
        let (flag, offset) = (self.generation.clone(), self.offset.clone());
        let (tx, key) = (to_main.clone(), token.clone());
        std::thread::spawn(move || poll(&key, &flag, generation, &offset, &tx));
        let flag = self.generation.clone();
        let (weak, mine) = (Rc::downgrade(self), token.clone());
        std::thread::spawn(move || send(&token, &flag, generation, &to_send, &to_main));
        glib::spawn_future_local(async move {
            while let Ok(incoming) = from_threads.recv().await {
                let Some(bot) = weak.upgrade() else { break };
                // Restarted meanwhile: what this bot already received is still handled (its
                // offset is taken); another bot's, or a stopped one's, is not.
                if bot.generation.load(Ordering::SeqCst) != generation
                    && (bot.running.borrow().as_ref().map(|(t, _)| t) != Some(&mine) || !matches!(incoming, In::Update(_) | In::Sent(..)))
                {
                    continue;
                }
                bot.incoming(incoming);
            }
        });
    }

    /// The settings loom gives this bridge (null: none, loom is stopping it).
    fn settings(self: &Rc<Self>, s: &Value) {
        if s.is_null() {
            return;
        }
        crate::log::set(s["debug"].as_bool().unwrap_or(false));
        self.speech.set(s["speech"].as_bool().unwrap_or(false));
        self.apply(s);
    }

    /// How it is, for loom's settings to show.
    fn set_state(&self, state: Link) {
        let _ = self.loom("state", state.to_json());
    }

    /// One request to loom.
    fn loom(&self, method: &str, params: Value) -> Result<Value, String> {
        let result = self.loom.call(method, params);
        if let Err(why) = &result {
            crate::log::debug!("telegram", "loom said no to {method}: {why}");
        }
        result
    }

    /// An agent's view, if newer than the one kept.
    fn seen(&self, v: &Value) {
        let Some(view) = View::from(v) else { return };
        let mut agents = self.agents.borrow_mut();
        match agents.iter_mut().find(|a| a.id == view.id) {
            Some(kept) if kept.seq <= view.seq => *kept = view,
            Some(_) => {}
            None => agents.push(view),
        }
    }

    /// The agents again, in loom's order.
    fn refresh(&self) {
        if let Ok(list) = self.loom("agents", json!({})) {
            *self.agents.borrow_mut() = list.as_array().into_iter().flatten().filter_map(View::from).collect();
        }
    }

    fn last_reply(&self, agent: u32) -> Option<String> {
        self.loom("last_reply", json!({ "agent": agent })).ok()?.as_str().map(str::to_owned)
    }

    /// A message for `agent` (pictures as they came: loom reads them).
    fn send(&self, agent: u32, text: &str, images: Vec<Vec<u8>>) -> Result<(), String> {
        let images: Vec<String> = images.iter().map(|b| glib::base64_encode(b).to_string()).collect();
        let view = self.loom("send", json!({ "agent": agent, "text": text, "images": images }))?;
        self.seen(&view);
        Ok(())
    }

    /// Answers permission question `serial` with option `option`: whether it was.
    fn answer(&self, agent: u32, serial: u64, option: usize) -> bool {
        match self.loom("answer", json!({ "agent": agent, "serial": serial, "option": option })) {
            Ok(r) => {
                self.seen(&r["agent"]);
                r["answered"].as_bool() == Some(true)
            }
            Err(_) => false,
        }
    }

    /// A form action (`what`: `{"do": "pick" | "write" | "send" | "skip", …}`): done, and why not.
    fn form(&self, agent: u32, serial: u64, mut what: Value) -> (bool, Option<String>) {
        what["agent"] = json!(agent);
        what["serial"] = json!(serial);
        match self.loom("form", what) {
            Ok(r) => {
                self.seen(&r["agent"]);
                (r["done"].as_bool() == Some(true), r["problem"].as_str().map(str::to_owned))
            }
            Err(why) => (false, Some(why)),
        }
    }

    fn on(&self) -> bool {
        self.calls.borrow().is_some() && self.user.get() != 0
    }

    fn call(&self, method: &'static str, body: Value, about: About) {
        self.call_with(method, body, about, None);
    }

    fn call_with(&self, method: &'static str, body: Value, about: About, plain: Option<String>) {
        crate::log::debug!("telegram", "→ {method}");
        if let Some(calls) = self.calls.borrow().as_ref() {
            let _ = calls.send_blocking(Call { method, body, about, plain });
        }
    }

    /// A plain message, with the menu under it (its counts kept current).
    fn say(&self, text: &str, about: About) {
        let parts = split(text);
        let last = parts.len().saturating_sub(1);
        for (i, part) in parts.into_iter().enumerate() {
            let mut body = json!({ "chat_id": self.user.get(), "text": part, "link_preview_options": { "is_disabled": true } });
            if i == last {
                body["reply_markup"] = self.menu();
            }
            self.call("sendMessage", body, about);
        }
    }

    /// `header` (plain), then Claude's Markdown `text` formatted as Telegram shows it: bold,
    /// code, lists… (plain if Telegram can't read it).
    fn say_markdown(&self, header: &str, text: &str, about: About) {
        let parts = split_markdown(text);
        let last = parts.len().saturating_sub(1);
        for (i, part) in parts.into_iter().enumerate() {
            let (formatted, plain) = if i == 0 && !header.is_empty() {
                (format!("{}\n\n{}", escape(header), html(&part)), format!("{header}\n\n{part}"))
            } else {
                (html(&part), part)
            };
            let mut body = json!({ "chat_id": self.user.get(), "text": formatted, "parse_mode": "HTML", "link_preview_options": { "is_disabled": true } });
            if i == last {
                body["reply_markup"] = self.menu();
            }
            self.call_with("sendMessage", body, about, Some(plain));
        }
    }

    /// The menu: agents, what needs you (how many), and for the agent written to: stop it,
    /// its last reply. The input says who a message goes to.
    fn menu(&self) -> Value {
        let waiting: usize = self.agents.borrow().iter().map(|a| a.questions.len()).sum();
        let needs = if waiting > 0 { format!("{NEEDS} ({waiting})") } else { NEEDS.to_owned() };
        let to = match self.talking_to.get().and_then(|id| self.agent(id)) {
            Some(agent) => format!("To {}", agent.title()),
            None => format!("Pick an agent: {AGENTS}"),
        };
        json!({
            "keyboard": [[{ "text": AGENTS }, { "text": needs }], [{ "text": STOP }, { "text": LAST }]],
            "resize_keyboard": true,
            "is_persistent": true,
            "input_field_placeholder": clip(&to, 64),
        })
    }

    /// The agents as the keyboard (state, project, name), and Back.
    fn agents_menu(&self) {
        self.refresh();
        let agents = self.agents.borrow().clone();
        if agents.is_empty() {
            self.say("No agents open in loom.", About::Nothing);
            return;
        }
        let mut labels = self.labels.borrow_mut();
        labels.clear();
        let mut rows = Vec::new();
        let mut lines = vec!["Tap an agent to talk to it:".to_owned()];
        for agent in &agents {
            let (mark, state) = describe(agent);
            let project = &agent.project;
            let mut label = clip(&format!("{mark} {project} · {}", agent.title()), 60);
            // Two alike (same project and name): told apart.
            if labels.contains_key(&label) {
                label = format!("{label} ({})", agent.id);
            }
            labels.insert(label.clone(), agent.id);
            rows.push(json!([{ "text": label }]));
            let current = if self.talking_to.get() == Some(agent.id) { "  ← writing to" } else { "" };
            lines.push(format!("{mark} {project} · {} — {state}{current}", agent.title()));
        }
        drop(labels);
        rows.push(json!([{ "text": BACK }]));
        let body = json!({
            "chat_id": self.user.get(), "text": lines.join("\n"),
            "reply_markup": { "keyboard": rows, "resize_keyboard": true, "is_persistent": true, "input_field_placeholder": "Pick an agent" },
        });
        self.call("sendMessage", body, About::Nothing);
    }

    /// Messages now go to `agent`: who it is, what it said last, what it asks.
    fn pick(&self, agent: &View) {
        self.talking_to.set(Some(agent.id));
        let (mark, state) = describe(agent);
        let project = &agent.project;
        let header = format!("{mark} {project} · {} — {state}. Your messages go to it.", agent.title());
        match self.last_reply(agent.id) {
            Some(last) => self.say_markdown(&format!("{header}\n\nIts last reply:"), &last, About::Agent(agent.id)),
            None => self.say(&header, About::Agent(agent.id)),
        }
        // Questions not told yet (those told keep their message).
        for serial in agent.open_questions() {
            self.tell(agent, serial);
        }
    }

    /// Every open question: those already told pointed at again, the rest told.
    fn needs_me(&self) {
        self.refresh();
        let mut any = false;
        let agents = self.agents.borrow().clone();
        for agent in agents {
            for serial in agent.open_questions() {
                any = true;
                let told = self.told.borrow().get(&(agent.id, serial)).and_then(|(message, _)| *message);
                match told {
                    Some(message) => {
                        let body = json!({
                            "chat_id": self.user.get(), "text": format!("🟠 {} is still waiting for this.", agent.title()),
                            "reply_parameters": { "message_id": message, "allow_sending_without_reply": true },
                            "reply_markup": self.menu(),
                        });
                        self.call("sendMessage", body, About::Agent(agent.id));
                    }
                    None => self.tell(&agent, serial),
                }
            }
        }
        if !any {
            self.say("Nothing needs you right now.", About::Nothing);
        }
    }

    /// "typing…" while `agent` works on what was sent from here (Telegram shows it 5 s a time).
    fn typing(self: &Rc<Self>, agent: u32) {
        // One timer at a time: a new message (or another agent) starts it over.
        self.stop_typing();
        self.typing.set(Some(agent));
        let action = |bot: &Bot| bot.call("sendChatAction", json!({ "chat_id": bot.user.get(), "action": "typing" }), About::Nothing);
        action(self);
        let (weak, started) = (Rc::downgrade(self), std::time::Instant::now());
        let timer = glib::timeout_add_seconds_local(4, move || {
            let Some(bot) = weak.upgrade() else { return glib::ControlFlow::Break };
            // Starting (resumed) and waiting on a question are still at it.
            let busy = bot.agent(agent).is_some_and(|a| matches!(a.status(), Status::Working | Status::Starting | Status::Blocked));
            if !bot.on() || !busy || started.elapsed().as_secs() > 30 * 60 {
                bot.typing.set(None);
                bot.typing_timer.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            // Waiting on you: no "typing" then (it comes back once answered).
            if bot.agent(agent).is_some_and(|a| a.status() != Status::Blocked) {
                action(&bot);
            }
            glib::ControlFlow::Continue
        });
        *self.typing_timer.borrow_mut() = Some(timer);
    }

    fn stop_typing(&self) {
        self.typing.set(None);
        if let Some(timer) = self.typing_timer.take() {
            timer.remove();
        }
    }

    /// One message (cut short if it must), with buttons.
    fn ask(&self, text: &str, keyboard: Value, about: About) {
        let body = json!({
            "chat_id": self.user.get(), "text": clip(text, PART), "link_preview_options": { "is_disabled": true },
            "reply_markup": { "inline_keyboard": keyboard },
        });
        self.call("sendMessage", body, about);
    }

    fn edit(&self, message: i64, text: &str, keyboard: Option<Value>) {
        let mut body = json!({ "chat_id": self.user.get(), "message_id": message, "text": clip(text, PART) });
        body["reply_markup"] = json!({ "inline_keyboard": keyboard.unwrap_or_else(|| json!([])) });
        self.call("editMessageText", body, About::Nothing);
    }

    fn connected(&self, bot: &str, problem: Option<String>) -> Link {
        Link::Connected { bot: bot.to_owned(), user: self.user.get(), code: self.code.borrow().clone(), candidate: self.candidate.borrow().clone(), problem }
    }

    fn incoming(self: &Rc<Self>, incoming: In) {
        match incoming {
            In::Online(name) => {
                self.set_state(self.connected(&name, None));
                // The menu under the chat replaces slash commands.
                self.call("deleteMyCommands", json!({}), About::Nothing);
                *self.online.borrow_mut() = Some(name);
                if self.greet.take() && self.user.get() != 0 {
                    self.say(&format!("Linked: loom now answers only this account. {AGENTS} below lists your agents."), About::Nothing);
                }
            }
            In::Problem(problem) => {
                crate::log::debug!("telegram", "problem: {problem}");
                self.online.borrow_mut().take();
                self.set_state(Link::Failed(problem));
            }
            In::Unsent(why, about) => {
                // A question that never arrived can be told again (tapping its agent does).
                if let About::Question(agent, serial) = about {
                    self.told.borrow_mut().remove(&(agent, serial));
                }
                crate::log::debug!("telegram", "not sent: {why}");
                let state = match self.online.borrow().as_deref() {
                    Some(name) => self.connected(name, Some(why)),
                    None => Link::Failed(why),
                };
                self.set_state(state);
            }
            In::Sent(about, message) => match about {
                About::Nothing => {}
                About::Agent(agent) => self.remember(message, agent),
                About::Question(agent, serial) => {
                    self.remember(message, agent);
                    let open = self.agent(agent).is_some_and(|a| a.open_questions().contains(&serial));
                    let mut told = self.told.borrow_mut();
                    if open {
                        if let Some(entry) = told.get_mut(&(agent, serial)) {
                            entry.0 = Some(message);
                        }
                    } else if let Some((_, text)) = told.remove(&(agent, serial)) {
                        // Answered in loom while this was on its way.
                        drop(told);
                        self.edit(message, &format!("{text}\n\n→ Answered in loom"), None);
                    }
                }
            },
            In::Update(update) => self.update(&update),
        }
    }

    fn remember(&self, message: i64, agent: u32) {
        let mut about = self.about.borrow_mut();
        // The oldest go first (message ids grow).
        if about.len() > 2000
            && let Some(cut) = about.keys().copied().min().map(|oldest| oldest + 500)
        {
            about.retain(|id, _| *id > cut);
        }
        about.insert(message, agent);
    }

    /// Only the allowed account's private messages and button presses count; anyone else's
    /// are dropped. Before an account is linked, only the code from the settings links one.
    fn update(self: &Rc<Self>, update: &Value) {
        crate::log::debug!("telegram", "← update {} (from {})", update["update_id"], update.pointer("/message/from/id").or_else(|| update.pointer("/callback_query/from/id")).unwrap_or(&Value::Null));
        let (from, item) = if let Some(query) = update.get("callback_query") {
            (&query["from"], query)
        } else if let Some(message) = update.get("message") {
            (&message["from"], message)
        } else {
            return;
        };
        let id = from["id"].as_i64().unwrap_or(0);
        let private = item.get("data").is_some() || item["chat"]["type"].as_str() == Some("private");
        if id == 0 || !private {
            return;
        }
        if self.user.get() == 0 {
            let name = from["username"].as_str().map(|u| format!("@{u}")).or_else(|| from["first_name"].as_str().map(str::to_owned)).unwrap_or_default();
            self.link(id, name, item["text"].as_str().unwrap_or(""));
            return;
        }
        if id != self.user.get() {
            return;
        }
        if item.get("data").is_some() {
            self.pressed(item);
        } else {
            self.message(item);
        }
    }

    /// Setting up: the account that sends the code shown in the settings is the one allowed.
    fn link(&self, id: i64, name: String, text: &str) {
        crate::log::debug!("telegram", "link attempt from {id}");
        let Some(code) = self.code.borrow().clone() else { return };
        // "/start <code>" (a t.me link) or the code alone: proposed, for the user to confirm
        // in loom's settings. Nothing is said to the sender.
        if text.split_whitespace().last() == Some(code.as_str()) {
            *self.candidate.borrow_mut() = Some((id, name));
            if let Some(bot) = self.online.borrow().clone() {
                self.set_state(self.connected(&bot, None));
            }
            return;
        }
        // Anything else is ignored. (The code is never replaced on wrong tries: anyone could
        // then keep it changing; at 8 characters it is not guessed.)
    }

    fn agent(&self, id: u32) -> Option<View> {
        self.agents.borrow().iter().find(|a| a.id == id).cloned()
    }

    fn message(self: &Rc<Self>, message: &Value) {
        // A photo, or a file: fetched, then to the agent (a picture as one it sees).
        if message.get("photo").is_some() || message.get("document").is_some() {
            self.attachment(message);
            return;
        }
        if message.get("voice").is_some() || message.get("audio").is_some() {
            self.voice(message);
            return;
        }
        let Some(text) = message["text"].as_str().map(str::trim) else {
            self.say("Text, photos, files and voice for now (no video or stickers).", About::Nothing);
            return;
        };
        let command = text.split_whitespace().next().unwrap_or("").split('@').next().unwrap_or("");
        // The menu's buttons (and the commands of before, still understood).
        if text == AGENTS || matches!(command, "/start" | "/agents" | "/list") {
            self.agents_menu();
            return;
        }
        if text == BACK {
            self.say(HELP, About::Nothing);
            return;
        }
        if command == "/help" {
            self.say(HELP, About::Nothing);
            return;
        }
        if text.starts_with(NEEDS) {
            self.needs_me();
            return;
        }
        let picked = self.labels.borrow().get(text).copied();
        if let Some(id) = picked {
            match self.agent(id) {
                Some(agent) => self.pick(&agent),
                // Closed since the keyboard was sent: not a message for anyone.
                None => {
                    self.say("That agent is closed.", About::Nothing);
                    self.agents_menu();
                }
            }
            return;
        }
        if text == STOP || text == LAST {
            match self.talking_to.get().and_then(|id| self.agent(id)) {
                None => self.agents_menu(),
                Some(agent) if text == STOP => {
                    if agent.is_busy() {
                        if matches!(agent.status(), Status::Working | Status::Blocked) {
                            self.stopped_here.set(Some((agent.id, std::time::Instant::now())));
                        }
                        if let Ok(view) = self.loom("stop", json!({ "agent": agent.id })) {
                            self.seen(&view);
                        }
                        self.say(&format!("⏹ Stopped {}.", agent.title()), About::Agent(agent.id));
                    } else {
                        self.say(&format!("{} isn't working.", agent.title()), About::Agent(agent.id));
                    }
                }
                Some(agent) => match self.last_reply(agent.id) {
                    Some(last) => self.say_markdown(&format!("📄 {}:", agent.title()), &last, About::Agent(agent.id)),
                    None => self.say(&format!("{} hasn't replied yet.", agent.title()), About::Agent(agent.id)),
                },
            }
            return;
        }
        self.deliver(message, text, false);
    }

    /// Text for an agent (typed, or heard in a voice message): a reply to one of its question
    /// forms fills the form in; else it is the agent's next message. Never a shell command;
    /// heard, never a slash command either (a mishearing must not clear a conversation).
    fn deliver(self: &Rc<Self>, message: &Value, text: &str, heard: bool) {
        let replied = message["reply_to_message"]["message_id"].as_i64();
        // A reply to a question: that question, of that agent.
        let question = replied.and_then(|m| self.told.borrow().iter().find(|(_, (told, _))| *told == Some(m)).map(|(key, _)| *key));
        if let Some((id, serial)) = question
            && let Some(agent) = self.agent(id)
            && agent.asking(serial).is_some_and(|a| a.is_form)
        {
            if self.form(agent.id, serial, json!({ "do": "write", "text": text })).0 {
                self.react(message);
                self.say("Written in. Press Send on the question to answer.", About::Nothing);
            } else {
                self.say("This question takes no answer of your own here: pick an option (or answer it in loom).", About::Nothing);
            }
            return;
        }
        let target = question.map(|(id, _)| id).or_else(|| replied.and_then(|m| self.about.borrow().get(&m).copied())).or(self.talking_to.get());
        let Some(agent) = target.and_then(|id| self.agent(id)) else {
            self.agents_menu();
            return;
        };
        // A shell command would run here unasked: only from the window.
        if text.starts_with('!') {
            self.say("! commands run only from loom itself.", About::Nothing);
            return;
        }
        if heard && text.starts_with('/') {
            self.say("That sounded like a command: voice doesn't run commands. Type it if you meant it.", About::Nothing);
            return;
        }
        let new = self.talking_to.replace(Some(agent.id)) != Some(agent.id);
        if let Err(why) = self.send(agent.id, text, Vec::new()) {
            self.say(&format!("Not sent: {why}"), About::Agent(agent.id));
            return;
        }
        self.react(message);
        // Written to another agent by replying: the menu says so (who the input goes to).
        if new {
            self.say(&format!("Now writing to {}.", agent.title()), About::Agent(agent.id));
        }
        self.typing(agent.id);
    }

    /// Who a message goes to: the agent whose message it replies to, else the one written to.
    fn target_of(&self, message: &Value) -> Option<View> {
        let replied = message["reply_to_message"]["message_id"].as_i64();
        let id = replied.and_then(|m| self.about.borrow().get(&m).copied()).or(self.talking_to.get());
        id.and_then(|id| self.agent(id))
    }

    /// A photo or a file sent to the bot. A picture (a photo, or an image sent as a file) goes
    /// with its caption as the agent's message, as if pasted in loom; any other file is saved
    /// in the agent's folder (telegram-files/) and the agent is told where.
    fn attachment(self: &Rc<Self>, message: &Value) {
        let Some(agent) = self.target_of(message) else {
            self.say("Pick the agent first (🧭 Agents), then send it again.", About::Nothing);
            return;
        };
        let caption = message["caption"].as_str().unwrap_or("").trim().to_owned();
        let (file, name, picture) = if let Some(sizes) = message["photo"].as_array() {
            // The largest of the sizes Telegram keeps.
            (sizes.last().cloned().unwrap_or_default(), "photo.jpg".to_owned(), true)
        } else {
            let document = message["document"].clone();
            let name = document["file_name"].as_str().unwrap_or("file").to_owned();
            let picture = document["mime_type"].as_str().is_some_and(|m| m.starts_with("image/"));
            (document, name, picture)
        };
        self.fetch(message, &file, move |bot, bytes| {
            bot.talking_to.set(Some(agent.id));
            // A picture as a pasted one; any other file saved in the agent's folder by loom.
            let sent = if picture {
                bot.send(agent.id, &caption, vec![bytes])
            } else {
                let data = glib::base64_encode(&bytes).to_string();
                bot.loom("save", json!({ "agent": agent.id, "name": name, "data": data })).and_then(|path| {
                    let note = format!("I sent you a file from Telegram, saved at {}", path.as_str().unwrap_or(""));
                    let text = if caption.is_empty() { note } else { format!("{caption}\n\n({note})") };
                    bot.send(agent.id, &text, Vec::new())
                })
            };
            match sent {
                Ok(()) => bot.typing(agent.id),
                Err(why) => bot.say(&why, About::Agent(agent.id)),
            }
        });
    }

    /// A file Telegram keeps for `message` (`file`: its photo size, document, voice…): fetched
    /// in the background, then `then` with its bytes. Over Telegram's 20 MB for bots, or
    /// failing, it is said instead.
    fn fetch(self: &Rc<Self>, message: &Value, file: &Value, then: impl FnOnce(&Rc<Bot>, Vec<u8>) + 'static) {
        let Some(file_id) = file["file_id"].as_str().map(str::to_owned) else { return };
        if file["file_size"].as_u64().is_some_and(|size| size > 20 * 1024 * 1024) {
            self.say("That is over 20 MB, more than a bot can fetch from Telegram.", About::Nothing);
            return;
        }
        let Some((token, _)) = self.running.borrow().clone() else { return };
        self.react(message);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let fetched = gio::spawn_blocking(move || download(&token, &file_id)).await.unwrap_or_else(|_| Err("the download stopped".into()));
            let Some(bot) = weak.upgrade() else { return };
            match fetched {
                Ok(bytes) => then(&bot, bytes),
                Err(why) => bot.say(&format!("Could not fetch it from Telegram: {why}"), About::Nothing),
            }
        });
    }

    /// A voice message: written down by the speech-to-text service set in loom, shown back
    /// ("Heard: …"), and sent to the agent as text.
    fn voice(self: &Rc<Self>, message: &Value) {
        if !self.speech.get() {
            self.say("Voice messages need speech to text: set it up in loom, Settings → Advanced.", About::Nothing);
            return;
        }
        if self.target_of(message).is_none() {
            self.say("Pick the agent first (🧭 Agents), then send it again.", About::Nothing);
            return;
        }
        let file = if message.get("voice").is_some() { message["voice"].clone() } else { message["audio"].clone() };
        let mime = file["mime_type"].as_str().unwrap_or("audio/ogg").to_owned();
        // The services tell the format from the name's extension.
        let name = file["file_name"].as_str().map(str::to_owned).unwrap_or_else(|| format!("voice.{}", audio_extension(&mime)));
        let caption = message["caption"].as_str().unwrap_or("").trim().to_owned();
        // The agent is found again when it is heard (a reply to a question may fill it in).
        let message = message.clone();
        self.fetch(&message.clone(), &file, move |bot, audio| {
            let (weak, loom) = (Rc::downgrade(bot), bot.loom.clone());
            let audio = glib::base64_encode(&audio).to_string();
            glib::spawn_future_local(async move {
                let asked = json!({ "audio": audio, "name": name, "mime": mime });
                let result = gio::spawn_blocking(move || loom.call("transcribe", asked)).await.unwrap_or_else(|_| Err("the request stopped".into()));
                let Some(bot) = weak.upgrade() else { return };
                match result.and_then(|text| text.as_str().map(str::to_owned).ok_or_else(|| "no text came back".to_owned())) {
                    Ok(text) if text.is_empty() => bot.say("Heard nothing in it.", About::Nothing),
                    Ok(text) => {
                        bot.say(&format!("🎙 Heard: {text}"), About::Nothing);
                        let text = if caption.is_empty() { text } else { format!("{caption}\n\n{text}") };
                        bot.deliver(&message, &text, true);
                    }
                    Err(why) => bot.say(&format!("Speech to text failed: {why}"), About::Nothing),
                }
            });
        });
    }

    /// 👍 on the message: it reached the agent.
    fn react(&self, message: &Value) {
        if let Some(id) = message["message_id"].as_i64() {
            self.call(
                "setMessageReaction",
                json!({ "chat_id": self.user.get(), "message_id": id, "reaction": [{ "type": "emoji", "emoji": "👍" }] }),
                About::Nothing,
            );
        }
    }

    /// Answers a question from here: `answer` runs with "answered in loom" held off for it.
    /// True if it was answered (it is then no longer told).
    fn answer_here(&self, agent: &View, serial: u64, answer: impl FnOnce() -> bool) -> bool {
        self.answering.set(Some((agent.id, serial)));
        let answered = answer();
        self.answering.set(None);
        if answered {
            self.told.borrow_mut().remove(&(agent.id, serial));
        }
        answered
    }

    fn pressed(self: &Rc<Self>, query: &Value) {
        let data = query["data"].as_str().unwrap_or("");
        let message = query["message"]["message_id"].as_i64();
        let parts: Vec<&str> = data.split(':').collect();
        let number = |i: usize| parts.get(i).and_then(|p| p.parse::<u64>().ok());
        let agent = number(1).and_then(|id| self.agent(id as u32));
        let serial = number(2).unwrap_or(0);
        let mut answer = String::new();
        // The question's message, answered: its buttons go.
        let done = |agent: &View, answered: &str, asking: &Asking| {
            if let Some(message) = message {
                self.edit(message, &format!("{}\n\n{answered}", question_text(&agent.title(), asking)), None);
            }
        };
        match (parts.first().copied(), &agent) {
            (_, None) => answer = "That agent is closed.".to_owned(),
            // A button of the old inline list.
            (Some("a"), Some(agent)) => self.pick(agent),
            (Some("q"), Some(agent)) => {
                let option = number(3).unwrap_or(0) as usize;
                let asking = agent.asking(serial);
                let picked = asking.as_ref().and_then(|a| a.options.get(option)).cloned();
                match (asking, picked) {
                    (Some(asking), Some((name, kind))) if kind != "allow_always" => {
                        if self.answer_here(agent, serial, || self.answer(agent.id, serial, option)) {
                            done(agent, &format!("→ {name}"), &asking);
                            answer = name;
                        } else {
                            answer = "Already answered.".to_owned();
                        }
                    }
                    (None, _) => answer = "Already answered.".to_owned(),
                    _ => answer = "Not from Telegram.".to_owned(),
                }
            }
            (Some("f"), Some(agent)) => {
                let (field, option) = (number(3).unwrap_or(0) as usize, number(4).unwrap_or(0) as usize);
                if !self.form(agent.id, serial, json!({ "do": "pick", "field": field, "option": option })).0 {
                    answer = "Already answered.".to_owned();
                } else if let Some(asking) = self.agent(agent.id).and_then(|a| a.asking(serial)) {
                    // One question, one answer: no need for Send.
                    let one = asking.fields.len() == 1 && !asking.fields[0].2;
                    if one && self.answer_here(agent, serial, || self.form(agent.id, serial, json!({ "do": "send" })).0) {
                        done(agent, &asking.summary, &asking);
                    } else if let Some(message) = message {
                        self.edit(message, &question_text(&agent.title(), &asking), Some(keyboard(agent, serial, &asking)));
                    }
                }
            }
            (Some(what @ ("s" | "k")), Some(agent)) => match agent.asking(serial) {
                // Sent empty from a phone is a slip more often than meant: Skip says that.
                Some(asking) if what == "s" && asking.blank => answer = "Pick an answer first, or Skip.".to_owned(),
                Some(asking) => {
                    let mut problem = None;
                    let answered = self.answer_here(agent, serial, || {
                        let (done, why) = self.form(agent.id, serial, json!({ "do": if what == "s" { "send" } else { "skip" } }));
                        problem = why;
                        done
                    });
                    if answered {
                        done(agent, if what == "s" { &asking.summary } else { "→ Skipped" }, &asking);
                    } else {
                        answer = problem.unwrap_or_default();
                    }
                }
                None => answer = "Already answered.".to_owned(),
            },
            _ => {}
        }
        let mut body = json!({ "callback_query_id": query["id"] });
        if !answer.is_empty() {
            body["text"] = Value::String(clip(&answer, 190));
        }
        self.call("answerCallbackQuery", body, About::Nothing);
    }

    /// Agent question `serial`, with its options as buttons (once).
    fn tell(&self, agent: &View, serial: u64) {
        if self.told.borrow().contains_key(&(agent.id, serial)) {
            return;
        }
        let Some(asking) = agent.asking(serial) else { return };
        let text = question_text(&agent.title(), &asking);
        self.told.borrow_mut().insert((agent.id, serial), (None, text.clone()));
        self.ask(&text, keyboard(agent, serial, &asking), About::Question(agent.id, serial));
    }

    /// An event from loom. `looking`: the user has that agent in front of them in loom (then
    /// the phone stays quiet).
    fn event(self: &Rc<Self>, e: &Value) {
        match e["event"].as_str().unwrap_or("") {
            "settings" => return self.settings(&e["settings"]),
            "closed" => {
                let id = e["agent"].as_u64().unwrap_or(0) as u32;
                self.agents.borrow_mut().retain(|a| a.id != id);
                return self.forget(id);
            }
            _ => {}
        }
        self.seen(&e["agent"]);
        let Some(agent) = e["agent"]["id"].as_u64().and_then(|id| self.agent(id as u32)) else { return };
        if !self.on() {
            return;
        }
        let looking = e["looking"].as_bool().unwrap_or(false);
        match e["event"].as_str().unwrap_or("") {
            "asked" if !looking => {
                for serial in agent.open_questions() {
                    self.tell(&agent, serial);
                }
            }
            "done" if !looking => {
                if self.typing.get() == Some(agent.id) {
                    self.stop_typing();
                }
                // Stopped: from here it was said already; from loom, said now.
                let here = self.stopped_here.take().is_some_and(|(id, at)| id == agent.id && at.elapsed().as_secs() < 120);
                if e["cancelled"].as_bool() == Some(true) {
                    if !here {
                        self.say(&format!("⏹ {} was stopped.", agent.title()), About::Agent(agent.id));
                    }
                    return;
                }
                if let Some(error) = e["error"].as_str() {
                    self.say(&format!("⚠️ {}: the turn ended on an error: {error}", agent.title()), About::Agent(agent.id));
                    return;
                }
                let header = format!("✅ {} · {} — done in {}", agent.project, agent.title(), e["took"].as_str().unwrap_or(""));
                match e["reply"].as_str() {
                    Some(reply) => self.say_markdown(&header, reply, About::Agent(agent.id)),
                    None => self.say(&header, About::Agent(agent.id)),
                }
            }
            "unsent" => {
                let count = e["count"].as_u64().unwrap_or(1);
                let what = if count == 1 { "A queued message".to_owned() } else { format!("{count} queued messages") };
                self.say(&format!("⚠️ {what} to {} never went out (the turn stopped first): back in loom's input.", agent.title()), About::Agent(agent.id));
            }
            "failed" if !looking => self.say(&format!("⚠️ {} stopped: {}", agent.title(), e["why"].as_str().unwrap_or("")), About::Agent(agent.id)),
            "agent" => {
                // Questions answered in loom: their buttons go.
                let open = agent.open_questions();
                let here = self.answering.get();
                let gone: Vec<(u64, i64, String)> = self
                    .told
                    .borrow()
                    .iter()
                    .filter(|(key, _)| key.0 == agent.id && !open.contains(&key.1) && Some(**key) != here)
                    // Still on its way (no message yet): the edit happens when it is sent.
                    .filter_map(|((_, serial), (message, text))| Some((*serial, (*message)?, text.clone())))
                    .collect();
                // Still at it: answered in loom. Not any more: dropped as the agent stopped.
                let outcome = if agent.is_busy() { "→ Answered in loom" } else { "→ Not answered: the agent stopped" };
                for (serial, message, text) in gone {
                    self.told.borrow_mut().remove(&(agent.id, serial));
                    self.edit(message, &format!("{text}\n\n{outcome}"), None);
                }
            }
            _ => {}
        }
    }

    /// An agent closed: what was told about it goes.
    fn forget(&self, agent: u32) {
        self.labels.borrow_mut().retain(|_, id| *id != agent);
        self.told.borrow_mut().retain(|(id, _), _| *id != agent);
        self.about.borrow_mut().retain(|_, id| *id != agent);
        if self.talking_to.get() == Some(agent) {
            self.talking_to.set(None);
        }
    }
}

const HELP: &str = "🧭 Agents: pick the agent you write to. A reply to one of its messages goes to it too.\n\
🟠 Needs me: every question waiting for you.\n\
⏹ Stop and 📄 Last reply: for the agent you write to.\n\
Questions and permission requests come with buttons; \"Always allow\" is only offered in loom.";

fn describe(agent: &View) -> (&'static str, &'static str) {
    match agent.status() {
        Status::Blocked => ("🟠", "needs you"),
        Status::Working => ("🔵", "working"),
        Status::Failed => ("🔴", "stopped"),
        Status::Starting => ("🔵", "starting"),
        Status::Idle if agent.unseen => ("🟢", "finished"),
        Status::Idle => ("⚪", "idle"),
        Status::Suspended => ("⚫", "suspended"),
    }
}

fn question_text(agent: &str, asking: &Asking) -> String {
    let mut text = if asking.is_form { format!("❓ {agent} asks:\n{}", asking.title) } else { format!("🟠 {agent} wants to: {}", asking.title) };
    if let Some(detail) = asking.detail.as_deref().filter(|d| !d.trim().is_empty() && d.trim() != asking.title.trim()) {
        text.push_str(&format!("\n\n{}", clip(detail.trim(), 2500)));
    }
    for (question, options, multi, _) in &asking.fields {
        let options = options.iter().enumerate().map(|(i, o)| format!("{}. {o}", i + 1)).collect::<Vec<_>>().join("\n");
        text.push_str(&format!("\n\n{question}{}\n{options}", if *multi { " (any)" } else { "" }));
    }
    if asking.writable {
        text.push_str("\n\nOr reply to this message with your own answer.");
    }
    text
}

fn keyboard(agent: &View, serial: u64, asking: &Asking) -> Value {
    let id = agent.id;
    let mut rows = Vec::new();
    if asking.is_form {
        let several = asking.fields.len() > 1;
        for (f, (question, options, _, picked)) in asking.fields.iter().enumerate() {
            for (o, option) in options.iter().enumerate() {
                let tick = if picked.get(o).copied().unwrap_or(false) { "✓ " } else { "" };
                let label = if several { format!("{tick}{}: {option}", clip(question, 20)) } else { format!("{tick}{option}") };
                rows.push(json!([{ "text": clip(&label, 60), "callback_data": format!("f:{id}:{serial}:{f}:{o}") }]));
            }
        }
        rows.push(json!([
            { "text": "Send", "callback_data": format!("s:{id}:{serial}") },
            { "text": "Skip", "callback_data": format!("k:{id}:{serial}") },
        ]));
    } else {
        // "Always" stays a decision made at the desk.
        let buttons: Vec<Value> = asking
            .options
            .iter()
            .enumerate()
            .filter(|(_, (_, kind))| kind != "allow_always")
            .map(|(i, (name, _))| json!({ "text": clip(name, 40), "callback_data": format!("q:{id}:{serial}:{i}") }))
            .collect();
        for pair in buttons.chunks(2) {
            rows.push(Value::Array(pair.to_vec()));
        }
    }
    Value::Array(rows)
}

fn clip(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_owned();
    }
    text.chars().take(most.saturating_sub(1)).collect::<String>() + "…"
}

/// A long text in parts that Telegram takes, cut at line ends where it can.
fn split(text: &str) -> Vec<String> {
    split_text(text, false)
}

/// Markdown in parts Telegram takes: a code block cut in two is closed at the end of one part
/// and opened again at the start of the next, so each part is still code.
fn split_markdown(text: &str) -> Vec<String> {
    split_text(text, true)
}

fn split_text(text: &str, markdown: bool) -> Vec<String> {
    // Room kept for a fence closed and opened again.
    let most = if markdown { PART - 64 } else { PART };
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut fence: Option<String> = None;
    let close = |part: &mut String, fence: &Option<String>| {
        if let Some(open) = fence {
            if !part.ends_with('\n') {
                part.push('\n');
            }
            part.push_str(&open.chars().take_while(|c| *c == '`' || *c == '~').collect::<String>());
            part.push('\n');
        }
    };
    for line in text.split_inclusive('\n') {
        if part.chars().count() + line.chars().count() > most && !part.trim().is_empty() {
            close(&mut part, &fence);
            parts.push(std::mem::take(&mut part));
            if let Some(open) = &fence {
                part.push_str(open);
                part.push('\n');
            }
        }
        let trimmed = line.trim_start();
        if markdown && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            fence = if fence.is_some() { None } else { Some(line.trim_end().to_owned()) };
        }
        if line.chars().count() > most {
            // One line too long for a part: cut, each piece code again if it was code.
            if !part.trim().is_empty() {
                close(&mut part, &fence);
                parts.push(std::mem::take(&mut part));
            }
            part.clear();
            let chars: Vec<char> = line.chars().collect();
            for chunk in chars.chunks(most) {
                let mut piece = String::new();
                if let Some(open) = &fence {
                    piece.push_str(open);
                    piece.push('\n');
                }
                piece.extend(chunk);
                close(&mut piece, &fence);
                parts.push(piece);
            }
            if let Some(open) = &fence {
                part.push_str(open);
                part.push('\n');
            }
        } else {
            part.push_str(line);
        }
    }
    parts.push(part);
    // Nothing but fences (a block cut right at its end): no message of its own.
    parts.retain(|p| p.lines().any(|l| !l.trim().is_empty() && !(markdown && (l.trim_start().starts_with("```") || l.trim_start().starts_with("~~~")))));
    if parts.is_empty() {
        parts.push(String::new());
    }
    parts
}

/// A file Telegram keeps (`file_id`), fetched whole.
fn download(token: &str, file_id: &str) -> Result<Vec<u8>, String> {
    let client = client(120);
    let file = request(&client, token, "getFile", &json!({ "file_id": file_id })).map_err(|f| f.why)?;
    let path = file["file_path"].as_str().ok_or("Telegram gave no path for it")?;
    let url = format!("{}/file/bot{token}/{path}", api());
    let mut response = client.get(&url).call().map_err(|e| e.to_string().replace(token, "…"))?;
    if response.status().as_u16() != 200 {
        return Err(format!("Telegram answered {}", response.status().as_u16()));
    }
    response.body_mut().with_config().limit(25 * 1024 * 1024).read_to_vec().map_err(|e| e.to_string().replace(token, "…"))
}

/// The file extension the services expect for an audio type.
fn audio_extension(mime: &str) -> &'static str {
    match mime {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" | "audio/aac" => "m4a",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/webm" => "webm",
        "audio/flac" | "audio/x-flac" => "flac",
        _ => "ogg",
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Claude's Markdown as the HTML Telegram takes (b, i, s, code, pre, a, blockquote): lists
/// as bullets, tables as rows of cells.
fn html(markdown: &str) -> String {
    use pulldown_cmark::{CodeBlockKind, Event as Md, Options, Parser, Tag, TagEnd};
    let mut out = String::new();
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut code_language = false;
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(markdown, options) {
        match event {
            Md::Start(tag) => match tag {
                Tag::Strong | Tag::Heading { .. } => out.push_str("<b>"),
                Tag::Emphasis => out.push_str("<i>"),
                Tag::Strikethrough => out.push_str("<s>"),
                Tag::BlockQuote(_) => out.push_str("<blockquote>"),
                Tag::CodeBlock(kind) => {
                    let language = match &kind {
                        CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_owned(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    code_language = !language.is_empty();
                    if code_language {
                        out.push_str(&format!("<pre><code class=\"language-{}\">", escape(&language)));
                    } else {
                        out.push_str("<pre>");
                    }
                }
                Tag::List(start) => {
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    lists.push(start);
                }
                Tag::Item => {
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&"   ".repeat(lists.len().saturating_sub(1)));
                    match lists.last_mut() {
                        Some(Some(n)) => {
                            out.push_str(&format!("{n}. "));
                            *n += 1;
                        }
                        _ => out.push_str("• "),
                    }
                }
                Tag::Link { dest_url, .. } => out.push_str(&format!("<a href=\"{}\">", escape(&dest_url).replace('"', "&quot;"))),
                _ => {}
            },
            Md::End(tag) => match tag {
                TagEnd::Strong => out.push_str("</b>"),
                TagEnd::Heading(_) => out.push_str("</b>\n\n"),
                TagEnd::Emphasis => out.push_str("</i>"),
                TagEnd::Strikethrough => out.push_str("</s>"),
                TagEnd::Paragraph => out.push_str(if lists.is_empty() { "\n\n" } else { "\n" }),
                TagEnd::BlockQuote(_) => {
                    let trimmed = out.trim_end().len();
                    out.truncate(trimmed);
                    out.push_str("</blockquote>\n\n");
                }
                TagEnd::CodeBlock => {
                    if out.ends_with('\n') {
                        out.pop();
                    }
                    out.push_str(if code_language { "</code></pre>\n\n" } else { "</pre>\n\n" });
                }
                TagEnd::List(_) => {
                    lists.pop();
                    if lists.is_empty() {
                        out.push('\n');
                    }
                }
                TagEnd::Item => {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
                TagEnd::Link => out.push_str("</a>"),
                TagEnd::TableCell => out.push_str(" │ "),
                TagEnd::TableHead | TagEnd::TableRow => {
                    if out.ends_with(" │ ") {
                        out.truncate(out.len() - " │ ".len());
                    }
                    out.push('\n');
                }
                TagEnd::Table => out.push('\n'),
                _ => {}
            },
            Md::Text(text) => out.push_str(&escape(&text)),
            Md::Code(code) => out.push_str(&format!("<code>{}</code>", escape(&code))),
            Md::SoftBreak | Md::HardBreak => out.push('\n'),
            Md::Rule => out.push_str("———\n\n"),
            Md::TaskListMarker(done) => out.push_str(if done { "☑ " } else { "☐ " }),
            Md::Html(text) | Md::InlineHtml(text) => out.push_str(&escape(&text)),
            _ => {}
        }
    }
    out.trim_end().to_owned()
}

// ---- the process -----------------------------------------------------------------------------

/// `loom bridge telegram`: started by loom (never by hand), until loom stops it or closes.
pub fn run() -> i32 {
    let (loom, app_id) = match Client::from_stdin() {
        Ok(started) => started,
        Err(why) => {
            eprintln!("loom bridge telegram: {why}");
            return 2;
        }
    };
    crate::log::name_for(&app_id);
    let context = glib::MainContext::default();
    let Ok(_owner) = context.acquire() else { return 1 };
    // Told first, asked second: nothing happens in between unheard.
    let events = match loom.subscribe() {
        Ok(events) => events,
        Err(why) => {
            eprintln!("loom bridge telegram: {why}");
            return 1;
        }
    };
    let hello = match loom.call("hello", json!({})) {
        Ok(hello) => hello,
        Err(why) => {
            eprintln!("loom bridge telegram: {why}");
            return 1;
        }
    };
    let bot = Bot::new(loom);
    for agent in hello["agents"].as_array().into_iter().flatten() {
        bot.seen(agent);
    }
    bot.settings(&hello["settings"]);
    let weak = Rc::downgrade(&bot);
    glib::spawn_future_local(async move {
        while let Ok(event) = events.recv().await {
            let Some(bot) = weak.upgrade() else { break };
            bot.event(&event);
        }
    });
    glib::MainLoop::new(Some(&context), false).run();
    0
}

// ---- the threads ---------------------------------------------------------------------------

fn api() -> String {
    // A test instance may talk to a stand-in server.
    match (std::env::var_os("LOOM_CONTROL"), std::env::var("LOOM_TELEGRAM_API")) {
        (Some(_), Ok(url)) => url,
        _ => "https://api.telegram.org".to_owned(),
    }
}

fn client(timeout: u64) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(timeout))).http_status_as_error(false).build().into()
}

/// Why a call failed (never with the token in it).
struct Failure {
    why: String,
    /// A wrong token: asking again won't help.
    fatal: bool,
    /// Not delivered for sure (no connection made): safe to send again.
    unsent: bool,
    /// Another poll is still running (one being stopped): wait it out.
    conflict: bool,
    /// Too many messages: Telegram says how many seconds to wait.
    retry_after: Option<u64>,
}

/// One Bot API call: its result, or what went wrong.
fn request(client: &ureq::Agent, token: &str, method: &str, body: &Value) -> Result<Value, Failure> {
    let url = format!("{}/bot{token}/{method}", api());
    let failure = |why: String, unsent: bool| Failure { why: why.replace(token, "…"), fatal: false, unsent, conflict: false, retry_after: None };
    let mut response = client.post(&url).header("content-type", "application/json").send(body.to_string()).map_err(|e| {
        let unsent = matches!(e, ureq::Error::ConnectionFailed | ureq::Error::HostNotFound);
        failure(format!("Can't reach Telegram: {e}"), unsent)
    })?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().map_err(|e| failure(format!("Can't reach Telegram: {e}"), false))?;
    let value: Value = serde_json::from_str(&text).map_err(|_| failure(format!("Telegram answered {status}."), false))?;
    if value["ok"].as_bool() == Some(true) {
        return Ok(value["result"].clone());
    }
    let why = value["description"].as_str().unwrap_or("refused");
    Err(Failure {
        fatal: status == 401 || status == 404,
        conflict: status == 409,
        retry_after: value["parameters"]["retry_after"].as_u64().filter(|_| status == 429),
        ..failure(format!("Telegram: {why}"), false)
    })
}

/// Waits for updates. `next`: the update to ask for, shared with the poll before and after
/// this one (a restart neither skips nor repeats any).
fn poll(token: &str, generation: &AtomicU64, mine: u64, next: &AtomicI64, to_main: &async_channel::Sender<In>) {
    let live = || generation.load(Ordering::SeqCst) == mine;
    let client = client(40);
    let mut offset: Option<i64> = None;
    let (mut name, mut down) = (String::new(), false);
    while live() {
        if offset.is_none() {
            let me = request(&client, token, "getMe", &json!({})).and_then(|me| {
                let known = next.load(Ordering::SeqCst);
                if known > 0 {
                    return Ok((me, known));
                }
                // The first start: past what was sent while loom was closed, so an old answer
                // or message is not acted on hours later.
                let last = request(&client, token, "getUpdates", &json!({ "offset": -1, "timeout": 0 }))?;
                let first = last.as_array().and_then(|u| u.last()).and_then(|u| u["update_id"].as_i64()).map_or(0, |id| id + 1);
                Ok((me, first))
            });
            match me {
                Ok((me, first)) => {
                    offset = Some(first);
                    name = me["username"].as_str().unwrap_or("bot").to_owned();
                    let _ = to_main.send_blocking(In::Online(name.clone()));
                }
                Err(failure) if failure.conflict => pause(&live, 3),
                Err(failure) => {
                    let _ = to_main.send_blocking(In::Problem(failure.why));
                    if failure.fatal {
                        return;
                    }
                    pause(&live, 15);
                }
            }
            continue;
        }
        let body = json!({ "offset": offset, "timeout": 25, "allowed_updates": ["message", "callback_query"] });
        match request(&client, token, "getUpdates", &body) {
            Ok(updates) => {
                // Stopped meanwhile: these are left for the poll after this one.
                if !live() {
                    return;
                }
                if std::mem::take(&mut down) {
                    let _ = to_main.send_blocking(In::Online(name.clone()));
                }
                for update in updates.as_array().into_iter().flatten() {
                    if let Some(id) = update["update_id"].as_i64() {
                        offset = Some(id + 1);
                        next.store(id + 1, Ordering::SeqCst);
                    }
                    if to_main.send_blocking(In::Update(update.clone())).is_err() {
                        return;
                    }
                }
            }
            // The poll before this one, not stopped yet: it gives up within seconds.
            Err(failure) if failure.conflict => pause(&live, 3),
            Err(failure) => {
                let _ = to_main.send_blocking(In::Problem(failure.why));
                if failure.fatal {
                    return;
                }
                pause(&live, 10);
                down = true;
            }
        }
    }
}

/// Waits `seconds`, or less if it is stopped.
fn pause(live: &dyn Fn() -> bool, seconds: u64) {
    for _ in 0..seconds * 4 {
        if !live() {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn send(token: &str, generation: &AtomicU64, mine: u64, calls: &async_channel::Receiver<Call>, to_main: &async_channel::Sender<In>) {
    let client = client(20);
    while let Ok(call) = calls.recv_blocking() {
        if generation.load(Ordering::SeqCst) != mine {
            return;
        }
        // One more try when it surely did not arrive, or when twice does no harm (all but a
        // new message); Telegram saying no is final.
        let mut result = request(&client, token, call.method, &call.body);
        if let Err(failure) = &result
            && failure.why.starts_with("Can't reach")
            && (failure.unsent || call.method != "sendMessage")
        {
            std::thread::sleep(Duration::from_secs(2));
            result = request(&client, token, call.method, &call.body);
        }
        // Too many at once (a long reply in many parts): as long as Telegram asks, then again.
        for _ in 0..3 {
            let Err(Failure { retry_after: Some(seconds), .. }) = &result else { break };
            std::thread::sleep(Duration::from_secs((*seconds).clamp(1, 60)));
            if generation.load(Ordering::SeqCst) != mine {
                return;
            }
            result = request(&client, token, call.method, &call.body);
        }
        if let (Err(failure), Some(plain)) = (&result, &call.plain)
            && failure.why.contains("can't parse entities")
        {
            let mut body = call.body.clone();
            body["text"] = Value::String(plain.clone());
            if let Some(body) = body.as_object_mut() {
                body.remove("parse_mode");
            }
            result = request(&client, token, call.method, &body);
        }
        match result {
            Ok(result) => {
                if let Some(id) = result["message_id"].as_i64() {
                    let _ = to_main.send_blocking(In::Sent(call.about, id));
                }
            }
            // An edit that changes nothing, or of a message too old: not worth a word.
            Err(failure) if failure.why.contains("message is not modified") || failure.why.contains("message to edit not found") => {}
            Err(failure) => {
                let _ = to_main.send_blocking(In::Unsent(failure.why, call.about));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_goes_in_parts_cut_at_lines() {
        let line = "x".repeat(1000) + "\n";
        let parts = split(&line.repeat(8));
        assert!(parts.len() >= 3);
        assert!(parts.iter().all(|p| p.chars().count() <= PART));
        assert!(parts.iter().all(|p| p.ends_with('\n')));
        assert_eq!(parts.concat(), line.repeat(8));
    }

    #[test]
    fn markdown_becomes_telegram_html() {
        let out = html("# Title\n\nSome **bold**, `a<b>` and [link](https://x.y/?a=1&b=2).\n\n- one\n- two\n\n```rust\nfn main() {}\n```");
        assert!(out.starts_with("<b>Title</b>"));
        assert!(out.contains("<b>bold</b>"));
        assert!(out.contains("<code>a&lt;b&gt;</code>"));
        assert!(out.contains("<a href=\"https://x.y/?a=1&amp;b=2\">link</a>"));
        assert!(out.contains("• one\n• two"));
        assert!(out.ends_with("<pre><code class=\"language-rust\">fn main() {}</code></pre>"));
    }

    #[test]
    fn a_code_block_cut_in_two_stays_code() {
        let code = "x\n".repeat(PART);
        let parts = split_markdown(&format!("```sh\n{code}```\n"));
        assert!(parts.len() >= 2);
        assert!(parts.iter().all(|p| p.starts_with("```sh") && p.trim_end().ends_with("```")));
    }

    #[test]
    fn a_huge_line_of_code_stays_code_in_every_part() {
        let parts = split_markdown(&format!("```json\n{}\n```\n", "*_#".repeat(PART)));
        assert!(parts.len() >= 3);
        assert!(parts.iter().all(|p| p.starts_with("```json\n") && p.trim_end().ends_with("```")));
    }

    #[test]
    fn no_part_is_only_fences() {
        // The block's last line fills a part: its closing fence must not become a message.
        let code = "y".repeat(PART - 80) + "\n";
        let parts = split_markdown(&format!("```sh\n{code}```\nafter\n"));
        assert!(parts.iter().all(|p| p.lines().any(|l| !l.trim().is_empty() && !l.starts_with("```"))));
    }

    #[test]
    fn one_huge_line_is_cut_anyway() {
        let parts = split(&"y".repeat(PART * 2 + 5));
        assert_eq!(parts.len(), 3);
        assert_eq!(split("hi"), vec!["hi".to_owned()]);
    }
}
