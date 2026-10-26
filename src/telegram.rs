//! The Telegram bot, off unless switched on in the settings. It lists the agents, tells their
//! finished replies and their questions, and takes messages and answers for them: from the
//! one Telegram account the settings allow, and nobody else.
//!
//! Two threads talk to Telegram (one waits for updates, one sends); everything else happens
//! here, on the main loop. Nothing listens on this machine: the bot asks Telegram.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use gtk::glib;
use serde_json::{Value, json};

use crate::agent::{Agent, Asking, Event, Status};
use crate::app::App;
use crate::settings::{self, Settings};

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
    /// A message that could not be sent (the bot is still connected).
    Unsent(String),
}

struct Call {
    method: &'static str,
    body: Value,
    about: About,
}

pub struct Bot {
    app: RefCell<Weak<App>>,
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
    /// Wrong guesses at the code (a new one after too many).
    guesses: Cell<u32>,
    /// Just linked: say so once connected.
    greet: Cell<bool>,
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
}

thread_local! {
    /// The bot's state in a line, for the settings page.
    static STATE: RefCell<String> = RefCell::new("Off.".to_owned());
    static WATCHERS: RefCell<Vec<Box<dyn Fn(&str) -> bool>>> = RefCell::new(Vec::new());
}

/// Calls `f` with the bot's state now and on every change, while it returns true.
pub fn watch(f: impl Fn(&str) -> bool + 'static) {
    if STATE.with(|s| f(&s.borrow())) {
        WATCHERS.with(|w| w.borrow_mut().push(Box::new(f)));
    }
}

fn set_state(line: &str) {
    STATE.with(|s| *s.borrow_mut() = line.to_owned());
    let watchers = WATCHERS.with(|w| std::mem::take(&mut *w.borrow_mut()));
    let kept: Vec<_> = watchers.into_iter().filter(|f| f(line)).collect();
    WATCHERS.with(|w| {
        let mut w = w.borrow_mut();
        let added = std::mem::take(&mut *w);
        *w = kept;
        w.extend(added);
    });
}

/// A code nobody can guess, to link the account that sends it.
fn new_code() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 8];
    let random = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if random.is_err() {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()) ^ u128::from(std::process::id());
        bytes = (seed as u64).to_le_bytes();
    }
    const LETTERS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    bytes.iter().map(|b| LETTERS[*b as usize % LETTERS.len()] as char).collect()
}

impl Bot {
    pub fn new() -> Rc<Bot> {
        Rc::new(Bot {
            app: RefCell::new(Weak::new()),
            generation: Arc::new(AtomicU64::new(0)),
            offset: Arc::new(AtomicI64::new(0)),
            calls: RefCell::default(),
            running: RefCell::default(),
            token: RefCell::default(),
            user: Cell::new(0),
            code: RefCell::default(),
            guesses: Cell::new(0),
            greet: Cell::new(false),
            online: RefCell::default(),
            talking_to: Cell::new(None),
            about: RefCell::default(),
            told: RefCell::default(),
            answering: Cell::new(None),
        })
    }

    pub fn attach(&self, app: &Rc<App>) {
        *self.app.borrow_mut() = Rc::downgrade(app);
    }

    /// Starts, stops or restarts it as the settings say.
    pub fn apply(self: &Rc<Self>, s: &Settings) {
        let token = s.telegram_token.trim().to_owned();
        let wanted = (s.telegram_enabled && !token.is_empty()).then(|| (token.clone(), s.telegram_user));
        if *self.running.borrow() == wanted {
            return;
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.calls.borrow_mut().take();
        *self.running.borrow_mut() = wanted.clone();
        let Some((token, user)) = wanted else {
            set_state(if s.telegram_enabled { "Needs the bot's token." } else { "Off." });
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
        if user != self.user.get() {
            self.talking_to.set(None);
        }
        self.user.set(user);
        *self.code.borrow_mut() = (user == 0).then(new_code);
        self.guesses.set(0);
        self.online.borrow_mut().take();
        set_state("Connecting…");
        let generation = self.generation.load(Ordering::SeqCst);
        let (to_main, from_threads) = async_channel::unbounded::<In>();
        let (calls, to_send) = async_channel::unbounded::<Call>();
        *self.calls.borrow_mut() = Some(calls);
        let (flag, offset) = (self.generation.clone(), self.offset.clone());
        let (tx, key) = (to_main.clone(), token.clone());
        std::thread::spawn(move || poll(&key, &flag, generation, &offset, &tx));
        let flag = self.generation.clone();
        std::thread::spawn(move || send(&token, &flag, generation, &to_send, &to_main));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(incoming) = from_threads.recv().await {
                let Some(bot) = weak.upgrade() else { break };
                if bot.generation.load(Ordering::SeqCst) != generation {
                    break;
                }
                bot.incoming(incoming);
            }
        });
    }

    fn on(&self) -> bool {
        self.calls.borrow().is_some() && self.user.get() != 0
    }

    fn call(&self, method: &'static str, body: Value, about: About) {
        if let Some(calls) = self.calls.borrow().as_ref() {
            let _ = calls.send_blocking(Call { method, body, about });
        }
    }

    fn say(&self, text: &str, about: About) {
        for part in split(text) {
            self.call("sendMessage", json!({ "chat_id": self.user.get(), "text": part, "link_preview_options": { "is_disabled": true } }), about);
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

    fn connected_line(&self, name: &str) -> String {
        match self.code.borrow().as_deref() {
            Some(code) => format!("Connected as @{name}. To link your Telegram account, send the bot this code: {code}"),
            None => format!("Connected as @{name}."),
        }
    }

    fn incoming(self: &Rc<Self>, incoming: In) {
        match incoming {
            In::Online(name) => {
                let line = self.connected_line(&name);
                set_state(&line);
                *self.online.borrow_mut() = Some(name);
                if self.greet.take() && self.user.get() != 0 {
                    self.say("Linked. loom now answers only this account. /agents lists your agents.", About::Nothing);
                }
            }
            In::Problem(problem) => {
                self.online.borrow_mut().take();
                set_state(&problem);
            }
            In::Unsent(why) => {
                let line = match self.online.borrow().as_deref() {
                    Some(name) => format!("{} A message could not be sent: {why}", self.connected_line(name)),
                    None => why,
                };
                set_state(&line);
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
            self.link(id, item["text"].as_str().unwrap_or(""));
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
    fn link(&self, id: i64, text: &str) {
        let Some(code) = self.code.borrow().clone() else { return };
        // "/start <code>" (a t.me link) or the code alone.
        if text.split_whitespace().last() == Some(code.as_str()) {
            self.code.borrow_mut().take();
            self.greet.set(true);
            settings::update(|s| s.telegram_user = id);
            return;
        }
        // Too many wrong tries: another code (the settings show it).
        self.guesses.set(self.guesses.get() + 1);
        if self.guesses.get() >= 20 {
            self.guesses.set(0);
            *self.code.borrow_mut() = Some(new_code());
            if let Some(name) = self.online.borrow().clone() {
                set_state(&self.connected_line(&name));
            }
        }
    }

    fn agent(&self, id: u32) -> Option<Rc<Agent>> {
        self.app.borrow().upgrade()?.agents_listed().into_iter().find(|a| a.id == id)
    }

    fn message(self: &Rc<Self>, message: &Value) {
        let Some(text) = message["text"].as_str().map(str::trim) else {
            self.say("Only text messages for now.", About::Nothing);
            return;
        };
        let command = text.split_whitespace().next().unwrap_or("").split('@').next().unwrap_or("");
        if matches!(command, "/start" | "/agents" | "/list") {
            self.list();
            return;
        }
        if command == "/help" {
            self.say(HELP, About::Nothing);
            return;
        }
        let replied = message["reply_to_message"]["message_id"].as_i64();
        // A reply to a question: that question, of that agent.
        let question = replied.and_then(|m| self.told.borrow().iter().find(|(_, (told, _))| *told == Some(m)).map(|(key, _)| *key));
        if let Some((id, serial)) = question
            && let Some(agent) = self.agent(id)
            && agent.asking(serial).is_some_and(|a| a.is_form)
        {
            if agent.write_in_form(serial, text) {
                self.react(message);
                self.say("Written in. Press Send on the question to answer.", About::Nothing);
            } else {
                self.say("This question takes no answer of your own here: pick an option (or answer it in loom).", About::Nothing);
            }
            return;
        }
        let target = question.map(|(id, _)| id).or_else(|| replied.and_then(|m| self.about.borrow().get(&m).copied())).or(self.talking_to.get());
        let Some(agent) = target.and_then(|id| self.agent(id)) else {
            self.list();
            return;
        };
        // A shell command would run here unasked: only from the window.
        if text.starts_with('!') {
            self.say("! commands run only from loom itself.", About::Nothing);
            return;
        }
        self.talking_to.set(Some(agent.id));
        agent.submit_with(text.to_owned(), Vec::new());
        self.react(message);
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

    fn list(&self) {
        let Some(app) = self.app.borrow().upgrade() else { return };
        let agents = app.agents_listed();
        if agents.is_empty() {
            self.say("No agents open in loom.", About::Nothing);
            return;
        }
        let mut lines = vec!["Your agents (tap one to talk to it):".to_owned()];
        let mut keyboard = Vec::new();
        for agent in &agents {
            let (mark, state) = describe(agent);
            lines.push(format!("{mark} {} · {} · {state}", agent.title(), app.project_label(&agent.project)));
            let current = if self.talking_to.get() == Some(agent.id) { " ←" } else { "" };
            keyboard.push(json!([{ "text": format!("{mark} {}{current}", clip(&agent.title(), 40)), "callback_data": format!("a:{}", agent.id) }]));
        }
        self.ask(&lines.join("\n"), Value::Array(keyboard), About::Nothing);
    }

    /// Answers a question from here: `answer` runs with "answered in loom" held off for it.
    /// True if it was answered (it is then no longer told).
    fn answer_here(&self, agent: &Agent, serial: u64, answer: impl FnOnce() -> bool) -> bool {
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
        let done = |agent: &Agent, answered: &str, asking: &Asking| {
            if let Some(message) = message {
                self.edit(message, &format!("{}\n\n{answered}", question_text(&agent.title(), asking)), None);
            }
        };
        match (parts.first().copied(), &agent) {
            (_, None) => answer = "That agent is closed.".to_owned(),
            (Some("a"), Some(agent)) => {
                self.talking_to.set(Some(agent.id));
                let (mark, state) = describe(agent);
                let mut text = format!("{mark} Talking to {} ({state}). Your messages go to it.", agent.title());
                if let Some(last) = agent.chat.last_reply() {
                    text.push_str(&format!("\n\nLast reply:\n{last}"));
                }
                self.say(&text, About::Agent(agent.id));
                // Questions not told yet (those told keep their message).
                for serial in agent.open_questions() {
                    self.tell(agent, serial);
                }
            }
            (Some("q"), Some(agent)) => {
                let option = number(3).unwrap_or(0) as usize;
                let asking = agent.asking(serial);
                let picked = asking.as_ref().and_then(|a| a.options.get(option)).cloned();
                match (asking, picked) {
                    (Some(asking), Some((name, kind))) if kind != "allow_always" => {
                        if self.answer_here(agent, serial, || agent.answer_serial(serial, option)) {
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
                if !agent.pick_in_form(serial, field, option) {
                    answer = "Already answered.".to_owned();
                } else if let Some(asking) = agent.asking(serial) {
                    // One question, one answer: no need for Send.
                    let one = asking.fields.len() == 1 && !asking.fields[0].2;
                    if one && self.answer_here(agent, serial, || agent.send_form_serial(serial).is_ok()) {
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
                        let result = if what == "s" { agent.send_form_serial(serial) } else { agent.skip_form(serial) };
                        result.map_err(|p| problem = Some(p)).is_ok()
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
    fn tell(&self, agent: &Rc<Agent>, serial: u64) {
        if self.told.borrow().contains_key(&(agent.id, serial)) {
            return;
        }
        let Some(asking) = agent.asking(serial) else { return };
        let text = question_text(&agent.title(), &asking);
        self.told.borrow_mut().insert((agent.id, serial), (None, text.clone()));
        self.ask(&text, keyboard(agent, serial, &asking), About::Question(agent.id, serial));
    }

    /// What happened to an agent, from the app. `looking`: the user has it in front of them
    /// in loom (then the phone stays quiet).
    pub fn agent_event(&self, agent: &Rc<Agent>, event: &Event, looking: bool) {
        if !self.on() {
            return;
        }
        match event {
            Event::Asked(_) if !looking => {
                for serial in agent.open_questions() {
                    self.tell(agent, serial);
                }
            }
            Event::Done if !looking => {
                let reply = agent.chat.last_reply().unwrap_or_else(|| "(no reply text)".to_owned());
                self.say(&format!("✅ {} finished\n\n{reply}", agent.title()), About::Agent(agent.id));
            }
            Event::Failed(why) if !looking => self.say(&format!("⚠️ {} stopped: {why}", agent.title()), About::Agent(agent.id)),
            Event::Changed => {
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
                for (serial, message, text) in gone {
                    self.told.borrow_mut().remove(&(agent.id, serial));
                    self.edit(message, &format!("{text}\n\n→ Answered in loom"), None);
                }
            }
            _ => {}
        }
    }

    /// An agent closed: what was told about it goes.
    pub fn forget(&self, agent: u32) {
        self.told.borrow_mut().retain(|(id, _), _| *id != agent);
        self.about.borrow_mut().retain(|_, id| *id != agent);
        if self.talking_to.get() == Some(agent) {
            self.talking_to.set(None);
        }
    }
}

const HELP: &str = "/agents: your agents; tap one to talk to it.\n\
Write a message: it goes to the agent you picked, or to the one whose message you reply to.\n\
Questions and permission requests come with buttons. \"Always allow\" is only offered in loom.";

fn describe(agent: &Agent) -> (&'static str, &'static str) {
    match agent.status() {
        Status::Blocked => ("🟠", "needs you"),
        Status::Working => ("🔵", "working"),
        Status::Failed => ("🔴", "stopped"),
        Status::Starting => ("🔵", "starting"),
        Status::Idle if agent.unseen.get() => ("🟢", "finished"),
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

fn keyboard(agent: &Agent, serial: u64, asking: &Asking) -> Value {
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
    let mut parts = Vec::new();
    let mut part = String::new();
    for line in text.split_inclusive('\n') {
        if part.chars().count() + line.chars().count() > PART && !part.is_empty() {
            parts.push(std::mem::take(&mut part));
        }
        if line.chars().count() > PART {
            let chars: Vec<char> = line.chars().collect();
            for chunk in chars.chunks(PART) {
                parts.push(chunk.iter().collect());
            }
        } else {
            part.push_str(line);
        }
    }
    if !part.trim().is_empty() || parts.is_empty() {
        parts.push(part);
    }
    parts
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
}

/// One Bot API call: its result, or what went wrong.
fn request(client: &ureq::Agent, token: &str, method: &str, body: &Value) -> Result<Value, Failure> {
    let url = format!("{}/bot{token}/{method}", api());
    let failure = |why: String, unsent: bool| Failure { why: why.replace(token, "…"), fatal: false, unsent, conflict: false };
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
    Err(Failure { fatal: status == 401 || status == 404, conflict: status == 409, ..failure(format!("Telegram: {why}"), false) })
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
        match result {
            Ok(result) => {
                if let Some(id) = result["message_id"].as_i64() {
                    let _ = to_main.send_blocking(In::Sent(call.about, id));
                }
            }
            // An edit that changes nothing, or of a message too old: not worth a word.
            Err(failure) if failure.why.contains("message is not modified") || failure.why.contains("message to edit not found") => {}
            Err(failure) => {
                let _ = to_main.send_blocking(In::Unsent(failure.why));
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
    fn one_huge_line_is_cut_anyway() {
        let parts = split(&"y".repeat(PART * 2 + 5));
        assert_eq!(parts.len(), 3);
        assert_eq!(split("hi"), vec!["hi".to_owned()]);
    }
}
