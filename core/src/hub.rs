//! Scribe at work: when it checks a conversation (quiet after a turn, grown long, about to be
//! compacted or closed, left unread when loom closed, or asked), one thing at a time off the
//! main thread; what it found (questions, changes waiting for an OK, what it wrote), kept; and
//! what the user does with it (answers, OKs, undos, comments, why-nots). A UI shows it through
//! a HubView.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use serde_json::{Value, json};

use crate::scribe::{self, Question, Report};

/// What the hub shows through.
pub trait HubView {
    /// Everything shown again (the counts changed, or a list).
    fn refresh(&self);
    /// The line on what is checked now, and since when.
    fn refresh_head(&self);
    /// What Scribe is doing, in the status line.
    fn status(&self, text: &str);
    /// Said where the user looks (an error stays).
    fn say(&self, text: &str, error: bool);
    /// Something new for the user (a question, a change to review).
    fn attention(&self);
    /// The history changed (an undo): its page, if shown, again.
    fn history_changed(&self);
}

/// Quiet this long after a turn: the conversation is checked.
const IDLE: Duration = Duration::from_secs(120);
/// This much unread (bytes of journal): checked at once, even while it goes on.
const LONG: u64 = 24_000;
/// How many of Scribe's own changes the panel lists (the history page has them all).
pub const RECENT: usize = 5;
/// The history page lists this many at most (the newest).
pub const PAGE: usize = 400;

/// Something that writes to the vault, done one at a time.
pub enum Job {
    /// Read a conversation (finished turns only, or all of it).
    Check(String, bool),
    /// A question answered.
    Answer(u64, String),
    /// A held change written (true) or dropped.
    Decide(u64, bool),
    /// A written change undone.
    Undo(u64),
    /// A written change (its row's data) changed again as the user says.
    Fix(Value, String),
}

#[derive(Default)]
pub struct Store {
    /// Changes waiting for the user's OK (scribe::proposal_json + "id").
    pub pending: Vec<Value>,
    /// Questions: topic, question, options, excerpt, session, title, id.
    pub questions: Vec<Value>,
    /// What Scribe changed, newest first.
    pub recent: Vec<Value>,
    /// Changes the user said no to or undid, each with a box to say why (it becomes a rule):
    /// the change (proposal_json), "noteBefore", "stage" (ask, drafting, draft), "draft".
    pub teach: Vec<Value>,
    /// The day (YYYY-MM-DD) and the calls to Claude made on it.
    pub day: String,
    pub calls: u64,
    pub next_id: u64,
}

impl Store {
    pub fn load() -> Store {
        let Ok(text) = std::fs::read_to_string(crate::journal::store_path()) else { return Store::default() };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { return Store::default() };
        let list = |k: &str| v[k].as_array().cloned().unwrap_or_default();
        let mut recent = list("recent");
        if !log_path().exists() {
            for v in recent.iter().rev() {
                log_change(v, "written", None);
            }
        }
        recent.truncate(RECENT);
        Store {
            pending: list("pending"),
            questions: list("questions"),
            recent,
            teach: list("teach")
                .into_iter()
                .map(|mut t| {
                    if t["stage"].as_str() == Some("drafting") {
                        t["stage"] = "ask".into();
                    }
                    t
                })
                .collect(),
            day: v["day"].as_str().unwrap_or("").to_owned(),
            calls: v["calls"].as_u64().unwrap_or(0),
            next_id: v["nextId"].as_u64().unwrap_or(1),
        }
    }

    pub fn save(&self) {
        use std::os::unix::fs::OpenOptionsExt;
        let path = crate::journal::store_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = json!({ "pending": self.pending, "questions": self.questions, "recent": self.recent, "teach": self.teach, "day": self.day, "calls": self.calls, "nextId": self.next_id }).to_string();
        let written = std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&path).and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()));
        if let Err(err) = written {
            crate::log::write("scribe", &format!("{}: {err}", path.display()));
        }
    }

    pub fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Calls left today (the count starts again each day).
    pub fn calls_left(&mut self) -> usize {
        let today = today();
        if self.day != today {
            self.day = today;
            self.calls = 0;
        }
        crate::settings::get().notes_daily_calls.saturating_sub(self.calls) as usize
    }
}

/// Every change Scribe wrote and every one the user dropped, one JSON line each (with the
/// note before and after, to undo it), and the undos: `{"undo": id}`.
pub fn log_path() -> PathBuf {
    crate::journal::store_path().with_file_name("scribe-history.jsonl")
}

fn log_line(v: &Value) {
    append_line(&log_path(), v);
}

/// One JSON line more at the end of `path` (made owner-only).
fn append_line(path: &std::path::Path, v: &Value) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let written = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path).and_then(|mut f| writeln!(f, "{v}"));
    if let Err(err) = written {
        crate::log::write("scribe", &format!("{}: {err}", path.display()));
    }
}

/// A change into the history: "written" (with what it did) or "dropped".
fn log_change(v: &Value, kind: &str, done: Option<&scribe::Written>) {
    let mut v = v.clone();
    v["kind"] = kind.into();
    if let Some(done) = done {
        v["before"] = done.before.clone().into();
        v["after"] = done.after.clone().into();
    }
    log_line(&v);
}

/// The history, newest first: the undone changes marked `"undone": true`.
pub fn history() -> Vec<Value> {
    let text = std::fs::read_to_string(log_path()).unwrap_or_default();
    let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let undone: Vec<u64> = lines.iter().filter_map(|v| v["undo"].as_u64()).collect();
    let mut changes: Vec<Value> = lines
        .into_iter()
        .filter(|v| v["undo"].is_null())
        .map(|mut v| {
            if v["id"].as_u64().is_some_and(|id| undone.contains(&id)) {
                v["undone"] = true.into();
            }
            v
        })
        .collect();
    changes.reverse();
    changes
}

/// "12:40" today, "26 Oct, 12:40" before.
pub fn when(at: u64) -> String {
    let format = if day_of(at) == today() { "%H:%M" } else { "%e %b, %H:%M" };
    local(at, format).trim().replace("  ", " ")
}

/// "12:40".
pub fn clock(at: u64) -> String {
    local(at, "%H:%M")
}

fn local(at: u64, format: &str) -> String {
    crate::schedule::local(at as i64, format)
}

pub fn day_of(at: u64) -> String {
    local(at, "%Y-%m-%d")
}

/// A day's heading on the history page: "Today", "Yesterday", "Monday 26 October".
pub fn day_heading(at: u64) -> String {
    let day = day_of(at);
    let yesterday = jiff::Zoned::now().date().yesterday().map(|d| d.to_string()).unwrap_or_default();
    if day == today() {
        "TODAY".to_owned()
    } else if day == yesterday {
        "YESTERDAY".to_owned()
    } else {
        local(at, "%A %e %B").replace("  ", " ").to_uppercase()
    }
}

/// The corrections the user explained, as tests for `loom scribe-eval` (private: they hold
/// the user's notes and words, so they stay in loom's state folder, never in the vault).
pub fn corrections_path() -> PathBuf {
    crate::journal::store_path().with_file_name("scribe-corrections.jsonl")
}

fn today() -> String {
    crate::paths::local_now("%Y-%m-%d")
}

pub struct Hub {
    pub store: RefCell<Store>,
    pub running: Cell<bool>,
    /// What waits its turn: one thing at a time touches the vault (a check, an answer, an OK).
    queue: RefCell<VecDeque<Job>>,
    timers: RefCell<HashMap<String, crate::rt::Timer>>,
    /// The history as listed (the notes before and after left out), and the log's length
    /// when read: read again only when it grew.
    listed: RefCell<(u64, Rc<Vec<Value>>)>,
    view: RefCell<Option<Weak<dyn HubView>>>,
    /// What Scribe is doing, as last said (for the core API's `scribe`).
    doing: RefCell<String>,
}

impl Hub {
    pub fn new() -> Rc<Hub> {
        Rc::new(Hub {
            store: RefCell::new(Store::load()),
            running: Cell::new(false),
            queue: RefCell::default(),
            timers: RefCell::default(),
            listed: RefCell::default(),
            view: RefCell::default(),
            doing: RefCell::default(),
        })
    }

    /// The core API's `scribe` (warp): `{id: "status", list: "status", running, doing,
    /// calls_left}`, then Scribe's questions (`list: "question"`), the changes waiting for an
    /// OK (`proposal`), what it wrote lately (`change`, newest first) and the changes said no
    /// to that wait for a why (`teach`), each with its own fields as kept, `id` being
    /// `<list>:<its number>` and `own_id` that number.
    /// Commands (loom's window and the CLI only: they write to the vault), `id` being the
    /// item's: `scribe_answer {id, answer}`, `scribe_decide {id, write}`, `scribe_undo {id}`,
    /// `scribe_drop {id}` (a question left unanswered), `scribe_teach {id, said}` (why not),
    /// `scribe_keep_rule {id, rule}`, `scribe_forget {id}` (a why-not dropped).
    pub fn serve(self: &Rc<Self>) {
        self.publish();
        let hub = crate::api::hub();
        let weak = Rc::downgrade(self);
        hub.handle("scribe_answer", move |who, params| {
            let (scribe, id) = scribe_target(&weak, who, params)?;
            let answer = params["answer"].as_str().filter(|a| !a.trim().is_empty()).ok_or_else(|| crate::api::Error::Invalid("an answer, please".into()))?;
            if !scribe.store.borrow().questions.iter().any(|q| q["id"].as_u64() == Some(id)) {
                return Err(crate::api::Error::Conflict("not asked any more (answered already?)".into()));
            }
            scribe.answer(id, answer);
            Ok(json!({}))
        });
        let weak = Rc::downgrade(self);
        hub.handle("scribe_decide", move |who, params| {
            let (scribe, id) = scribe_target(&weak, who, params)?;
            let write = params["write"].as_bool().ok_or_else(|| crate::api::Error::Invalid("write: true or false".into()))?;
            if !scribe.store.borrow().pending.iter().any(|p| p["id"].as_u64() == Some(id)) {
                return Err(crate::api::Error::Conflict("not waiting any more (decided already?)".into()));
            }
            scribe.decide(id, write);
            Ok(json!({}))
        });
        let weak = Rc::downgrade(self);
        hub.handle("scribe_undo", move |who, params| {
            let (scribe, id) = scribe_target(&weak, who, params)?;
            scribe.undo(id);
            Ok(json!({}))
        });
        let weak = Rc::downgrade(self);
        hub.handle("scribe_drop", move |who, params| {
            let (scribe, id) = scribe_target(&weak, who, params)?;
            scribe.drop_question(id);
            Ok(json!({}))
        });
        let weak = Rc::downgrade(self);
        hub.handle("scribe_teach", move |who, params| {
            let (scribe, id) = scribe_target(&weak, who, params)?;
            let said = params["said"].as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| crate::api::Error::Invalid("why not? (said)".into()))?;
            scribe.teach(id, said);
            Ok(json!({}))
        });
        let weak = Rc::downgrade(self);
        hub.handle("scribe_keep_rule", move |who, params| {
            let (scribe, id) = scribe_target(&weak, who, params)?;
            let rule = params["rule"].as_str().filter(|r| !r.trim().is_empty()).ok_or_else(|| crate::api::Error::Invalid("which rule?".into()))?;
            scribe.keep_rule(id, rule);
            Ok(json!({}))
        });
        // Everything Scribe wrote or was told no, newest first (the notes before and after
        // left out: `scribe_undo {id}` takes one back).
        hub.handle("scribe_history", move |who, params| {
            window_only(who)?;
            let limit = params["limit"].as_u64().unwrap_or(200).min(2000) as usize;
            let items: Vec<Value> = history()
                .into_iter()
                .take(limit)
                .map(|v| json!({ "id": v["id"].as_u64().map(|id| format!("change:{id}")), "kind": v["kind"], "topic": v["topic"], "reason": v["reason"], "change": v["change"], "title": v["title"], "at": v["at"], "undone": v["undone"].as_bool().unwrap_or(false) }))
                .collect();
            Ok(json!({ "changes": items }))
        });
        let weak = Rc::downgrade(self);
        hub.handle("scribe_forget", move |who, params| {
            let (scribe, id) = scribe_target(&weak, who, params)?;
            scribe.forget_teaching(id);
            Ok(json!({}))
        });
    }

    fn publish(&self) {
        let mut items = Vec::new();
        {
            // Asked while the store is being changed: the change's own refresh publishes it.
            let Ok(store) = self.store.try_borrow() else { return };
            let calls = if store.day == today() { store.calls } else { 0 };
            let calls_left = crate::settings::get().notes_daily_calls.saturating_sub(calls);
            items.push(json!({ "id": "status", "list": "status", "running": self.running.get(), "doing": *self.doing.borrow(), "calls_left": calls_left }));
            for (list, entries) in [("question", &store.questions), ("proposal", &store.pending), ("change", &store.recent), ("teach", &store.teach)] {
                for (i, entry) in entries.iter().enumerate() {
                    let mut item = entry.clone();
                    let Some(fields) = item.as_object_mut() else { continue };
                    let own = entry["id"].as_u64().map_or_else(|| format!("at{i}"), |id| id.to_string());
                    fields.insert("own_id".into(), entry["id"].clone());
                    fields.insert("id".into(), json!(format!("{list}:{own}")));
                    fields.insert("list".into(), json!(list));
                    items.push(item);
                }
            }
        }
        crate::api::hub().store("scribe").set(items);
    }

    pub fn set_view(self: &Rc<Self>, view: Weak<dyn HubView>) {
        *self.view.borrow_mut() = Some(view);
        // What was left unread when loom closed: caught up a little after it starts.
        let weak = Rc::downgrade(self);
        let _ = crate::rt::timeout_once(Duration::from_secs(20), move || {
            if let Some(hub) = weak.upgrade() {
                // Nothing is mid-turn yet: what is left is read whole (a last message whose
                // answer never came included).
                for session in crate::journal::waiting() {
                    hub.check_soon(&session, false, Duration::ZERO);
                }
            }
        });
    }

    fn view(&self) -> Option<Rc<dyn HubView>> {
        self.view.borrow().as_ref().and_then(Weak::upgrade)
    }

    fn refresh(&self) {
        self.publish();
        if let Some(view) = self.view() {
            view.refresh();
        }
    }

    fn refresh_head(&self) {
        self.publish();
        if let Some(view) = self.view() {
            view.refresh_head();
        }
    }

    fn status(&self, text: &str) {
        *self.doing.borrow_mut() = text.to_owned();
        self.publish();
        if let Some(view) = self.view() {
            view.status(text);
        }
    }

    fn say(&self, text: &str, error: bool) {
        if let Some(view) = self.view() {
            view.say(text, error);
        }
    }

    fn history_changed(&self) {
        if let Some(view) = self.view() {
            view.history_changed();
        }
    }

    pub fn on(&self) -> Option<PathBuf> {
        crate::vault::current()
    }

    /// A turn ended: checked when the conversation goes quiet, or now if it has grown long.
    pub fn turn_done(self: &Rc<Self>, session: &str) {
        if self.on().is_none() {
            return;
        }
        if crate::journal::unread_summary(session).is_some_and(|(bytes, _)| bytes >= LONG) {
            self.check_soon(session, true, Duration::ZERO);
        } else {
            self.check_soon(session, true, IDLE);
        }
        self.refresh_head();
    }

    /// About to be compacted, or closed: checked now (closed: its last words too).
    pub fn check_now(self: &Rc<Self>, session: &str, all: bool) {
        if self.on().is_some() {
            self.check_soon(session, !all, Duration::ZERO);
        }
    }

    fn check_soon(self: &Rc<Self>, session: &str, finished_only: bool, after: Duration) {
        if let Some(timer) = self.timers.borrow_mut().remove(session) {
            timer.remove();
        }
        if after.is_zero() {
            let mut queue = self.queue.borrow_mut();
            let queued = queue.iter_mut().find_map(|job| match job {
                Job::Check(s, finished) if s == session => Some(finished),
                _ => None,
            });
            match queued {
                // Already waiting: asked for all of it now (closed), it reads all of it.
                Some(finished) => *finished = *finished && finished_only,
                None => queue.push_back(Job::Check(session.to_owned(), finished_only)),
            }
            drop(queue);
            self.run_next();
            return;
        }
        let weak = Rc::downgrade(self);
        let key = session.to_owned();
        let id = crate::rt::timeout_once(after, move || {
            if let Some(hub) = weak.upgrade() {
                hub.timers.borrow_mut().remove(&key);
                hub.check_soon(&key, finished_only, Duration::ZERO);
            }
        });
        self.timers.borrow_mut().insert(session.to_owned(), id);
    }

    fn run_next(self: &Rc<Self>) {
        if self.running.get() {
            return;
        }
        let Some(vault) = self.on() else {
            self.queue.borrow_mut().clear();
            // What waited (an OK, an undo) shows as it was again.
            self.refresh();
            return;
        };
        let job = self.queue.borrow_mut().pop_front();
        let (session, finished_only) = match job {
            None => {
                self.refresh_head();
                return;
            }
            Some(Job::Answer(id, answer)) => {
                self.answer_now(id, &answer);
                return;
            }
            Some(Job::Decide(id, write)) => {
                self.decide_now(id, write);
                self.run_next();
                return;
            }
            Some(Job::Undo(id)) => {
                self.undo_now(id);
                self.run_next();
                return;
            }
            Some(Job::Fix(v, comment)) => {
                self.fix_now(&v, &comment);
                return;
            }
            Some(Job::Check(session, finished_only)) => (session, finished_only),
        };
        let entries = crate::journal::unread(&session, finished_only);
        if entries.is_empty() {
            self.run_next();
            return;
        }
        let calls_left = self.store.borrow_mut().calls_left();
        self.running.set(true);
        let title = entries.last().map(|e| e.title.clone()).unwrap_or_default();
        self.status(&format!("Reading “{title}”…"));
        self.refresh_head();
        let weak = Rc::downgrade(self);
        crate::rt::spawn_local(async move {
            let checked = crate::rt::spawn_blocking(move || scribe::check(&vault, &session, &entries, calls_left)).await;
            let Some(hub) = weak.upgrade() else { return };
            hub.running.set(false);
            match checked {
                Ok(report) => hub.take(report),
                Err(_) => hub.status("The check stopped unexpectedly."),
            }
            hub.run_next();
        });
    }

    /// Notes changed elsewhere (the Notes chat): each in the history beside Scribe's, with its
    /// Undo, and committed. `edits`: (note, relative to the vault; before, None if new; after).
    pub fn record_external(self: &Rc<Self>, edits: Vec<(String, Option<String>, String)>, session: &str, title: &str) {
        let Some(vault) = crate::vault::current() else { return };
        if edits.is_empty() {
            return;
        }
        let topics = crate::topics::list(&vault);
        {
            let mut store = self.store.borrow_mut();
            for (note, before, after) in &edits {
                // The topic whose folder holds it, else none.
                let topic = topics.iter().filter(|t| !t.folder.is_empty() && note.starts_with(&format!("{}/", t.folder.trim_end_matches('/')))).map(|t| t.name.clone()).next().unwrap_or_default();
                let change = match before {
                    None => json!({ "op": "create", "note": note, "text": after }),
                    Some(old) => json!({ "op": "replace", "note": note, "old": old, "new": after }),
                };
                let mut v = json!({ "topic": topic, "reason": "From the Notes chat", "change": change, "why": "", "session": session, "title": title, "at": crate::state::now(), "excerpt": "" });
                v["id"] = store.id().into();
                log_change(&v, "written", Some(&scribe::Written { before: before.clone(), after: after.clone() }));
                v["undoable"] = true.into();
                store.recent.insert(0, v);
            }
            store.recent.truncate(RECENT);
            store.save();
        }
        let files: Vec<String> = edits.iter().map(|(note, ..)| note.clone()).collect();
        let message = format!("loom: from the Notes chat ({title})");
        let committed = vault.clone();
        crate::rt::spawn_local(async move {
            let _ = crate::rt::spawn_blocking(move || scribe::commit(&committed, &files, &message)).await;
        });
        self.refresh();
    }

    /// A check's results: the journal marked as read, questions and held changes kept.
    fn take(self: &Rc<Self>, report: Report) {
        if report.upto > 0 {
            crate::journal::set_mark(&report.session, report.upto);
        }
        let news = !report.questions.is_empty() || !report.held.is_empty();
        {
            let mut store = self.store.borrow_mut();
            store.calls += report.calls as u64;
            for (proposal, why) in &report.held {
                let mut v = scribe::proposal_json(proposal, why, &report.session, &report.title);
                v["id"] = store.id().into();
                store.pending.push(v);
            }
            for q in &report.questions {
                let id = store.id();
                store.questions.push(json!({ "id": id, "topic": q.topic, "question": q.question, "options": q.options, "excerpt": q.excerpt, "session": report.session, "title": report.title, "at": crate::state::now() }));
            }
            for (p, done) in &report.applied {
                let mut v = scribe::proposal_json(p, "", &report.session, &report.title);
                v["id"] = store.id().into();
                log_change(&v, "written", Some(done));
                // What it did is in the history (whole notes): here only that it can be undone.
                v["undoable"] = true.into();
                v["unseen"] = true.into();
                store.recent.insert(0, v);
            }
            store.recent.truncate(RECENT);
            store.save();
        }
        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut did = Vec::new();
        if !report.applied.is_empty() {
            did.push(plural(report.applied.len(), "change written", "changes written"));
        }
        if !report.held.is_empty() {
            did.push(plural(report.held.len(), "change to review", "changes to review"));
        }
        if !report.questions.is_empty() {
            did.push(plural(report.questions.len(), "question", "questions"));
        }
        self.status(&match &report.failed {
            Some(why) => format!("Stopped: {why}"),
            None if did.is_empty() => format!("Checked “{}”: nothing to keep.", report.title),
            None => format!("Checked “{}”: {}.", report.title, did.join(", ")),
        });
        self.refresh();
        if news && let Some(view) = self.view() {
            view.attention();
        }
    }

    /// (questions, changes to review).
    pub fn counts(&self) -> (usize, usize) {
        let store = self.store.borrow();
        (store.questions.len(), store.pending.len())
    }

    /// Scribe has a question about this conversation.
    pub fn asks_about(&self, session: &str) -> bool {
        self.store.borrow().questions.iter().any(|q| q["session"].as_str() == Some(session))
    }

    /// A change said no to (or undone) waits for a word on why, if the user wants to give one.
    fn offer_teaching(&self, change: &Value, note_before: Option<String>) {
        let mut t = change.clone();
        // What a written change did is in the history; only the note before is wanted here.
        if let Some(fields) = t.as_object_mut() {
            fields.remove("before");
            fields.remove("after");
        }
        t["noteBefore"] = note_before.into();
        t["stage"] = "ask".into();
        let mut store = self.store.borrow_mut();
        t["id"] = store.id().into();
        store.teach.insert(0, t);
        // A few at most: older ones nobody answered go.
        store.teach.truncate(3);
        store.save();
    }

    /// Test hooks: the newest correction answered with `said`, or its drafted rule kept.
    pub fn teach_newest(self: &Rc<Self>, said: &str) {
        let id = self.store.borrow().teach.first().and_then(|t| t["id"].as_u64());
        if let Some(id) = id {
            self.teach(id, said);
        }
    }

    /// Test hook: a comment on the newest written change.
    pub fn fix_newest(self: &Rc<Self>, comment: &str) {
        let first = self.store.borrow().recent.first().cloned();
        if let Some(v) = first {
            self.fix(v, comment.to_owned());
        }
    }

    pub fn keep_newest_rule(self: &Rc<Self>) {
        let first = self.store.borrow().teach.first().cloned();
        if let Some(t) = first.filter(|t| t["stage"].as_str() == Some("draft")) {
            self.keep_rule(t["id"].as_u64().unwrap_or(0), t["draft"].as_str().unwrap_or(""));
        }
    }

    pub fn forget_teaching(self: &Rc<Self>, id: u64) {
        self.store.borrow_mut().teach.retain(|t| t["id"].as_u64() != Some(id));
        self.store.borrow().save();
        self.refresh();
    }

    fn set_teaching(&self, id: u64, f: impl FnOnce(&mut Value)) {
        let mut store = self.store.borrow_mut();
        if let Some(t) = store.teach.iter_mut().find(|t| t["id"].as_u64() == Some(id)) {
            f(t);
        }
        store.save();
    }

    /// The user said why: kept as a test, and a rule drafted from it (one call to Claude).
    pub fn teach(self: &Rc<Self>, id: u64, said: &str) {
        let Some(vault) = self.on() else {
            self.say("Not kept: the Notes add-on is off (Settings → Notes).", true);
            self.refresh();
            return;
        };
        let t = self.store.borrow().teach.iter().find(|t| t["id"].as_u64() == Some(id)).cloned();
        let Some((t, proposal)) = t.and_then(|t| scribe::proposal_from(&t).map(|p| (t, p))) else {
            self.refresh();
            return;
        };
        if t["caseSaved"].as_bool() != Some(true)
            && let Some(case) = scribe::correction_case(&proposal, t["noteBefore"].as_str(), said, &today())
        {
            append_line(&corrections_path(), &case);
            self.set_teaching(id, |t| t["caseSaved"] = true.into());
        }
        if self.store.borrow_mut().calls_left() == 0 {
            self.say("Kept as a test. Today's limit of note checks is reached (Settings → Notes), so no rule is drafted.", true);
            self.forget_teaching(id);
            return;
        }
        self.set_teaching(id, |t| t["stage"] = "drafting".into());
        self.store.borrow_mut().calls += 1;
        self.store.borrow().save();
        self.refresh();
        let said = said.to_owned();
        let weak = Rc::downgrade(self);
        crate::rt::spawn_local(async move {
            let drafted = crate::rt::spawn_blocking(move || scribe::rule_from(&vault, &proposal.topic, &proposal.change, &proposal.reason, &said)).await;
            let Some(hub) = weak.upgrade() else { return };
            match drafted {
                Ok(Ok(rule)) => hub.set_teaching(id, |t| {
                    t["stage"] = "draft".into();
                    t["draft"] = rule.into();
                }),
                Ok(Err(why)) => {
                    hub.set_teaching(id, |t| t["stage"] = "ask".into());
                    hub.say(&format!("No rule drafted: {why}"), true);
                }
                Err(_) => {
                    hub.set_teaching(id, |t| t["stage"] = "ask".into());
                    hub.say("Drafting the rule stopped unexpectedly.", true);
                }
            }
            hub.refresh();
        });
    }

    pub fn keep_rule(self: &Rc<Self>, id: u64, rule: &str) {
        let Some(vault) = self.on() else {
            self.say("Not kept: the Notes add-on is off (Settings → Notes).", true);
            self.refresh();
            return;
        };
        let Some(topic) = self.store.borrow().teach.iter().find(|t| t["id"].as_u64() == Some(id)).and_then(|t| t["topic"].as_str().map(str::to_owned)) else {
            self.refresh();
            return;
        };
        match scribe::add_rule(&vault, &topic, rule) {
            Ok(()) => {
                self.say(&format!("Kept as a rule for {topic} (in its topic file, under Rules learned)."), false);
                self.forget_teaching(id);
            }
            Err(why) => {
                self.say(&format!("Not kept: {why}"), true);
                self.refresh();
            }
        }
    }

    /// An answered question: a rule drafted from the answer (one call to Claude), offered to
    /// keep if it holds beyond that one case, so the question is asked once.
    fn offer_answer_rule(self: &Rc<Self>, q: Value, answer: String) {
        let Some(vault) = self.on() else { return };
        if self.store.borrow_mut().calls_left() == 0 {
            return;
        }
        self.store.borrow_mut().calls += 1;
        self.store.borrow().save();
        let (topic, question, said) = (q["topic"].as_str().unwrap_or("").to_owned(), q["question"].as_str().unwrap_or("").to_owned(), answer.clone());
        let weak = Rc::downgrade(self);
        crate::rt::spawn_local(async move {
            let drafted = crate::rt::spawn_blocking(move || scribe::rule_from_answer(&vault, &topic, &question, &said)).await;
            let Some(hub) = weak.upgrade() else { return };
            let rule = match drafted {
                Ok(Ok(Some(rule))) => rule,
                Ok(Ok(None)) => return,
                Ok(Err(why)) => return crate::log::write("scribe", &format!("no rule from an answer: {why}")),
                Err(_) => return,
            };
            let mut t = json!({ "kind": "answer", "topic": q["topic"], "question": q["question"], "answer": answer, "session": q["session"], "title": q["title"], "stage": "draft", "draft": rule });
            {
                let mut store = hub.store.borrow_mut();
                t["id"] = store.id().into();
                store.teach.insert(0, t);
                store.teach.truncate(3);
                store.save();
            }
            hub.refresh();
        });
    }

    /// Seen: one written change (by id), or all the panel shows (None).
    pub fn mark_seen(&self, id: Option<u64>) -> bool {
        let mut store = self.store.borrow_mut();
        let mut changed = false;
        for v in store.recent.iter_mut().filter(|v| id.is_none() || v["id"].as_u64() == id) {
            if let Some(fields) = v.as_object_mut() {
                changed |= fields.remove("unseen").is_some();
            }
        }
        if changed {
            store.save();
        }
        changed
    }

    /// A comment on a written change: the note changed as the user says (after the check
    /// running now, if one is).
    pub fn fix(self: &Rc<Self>, v: Value, comment: String) {
        if self.running.get() {
            self.say("Changed as you say as soon as the check that is running ends…", false);
            self.queue.borrow_mut().push_front(Job::Fix(v, comment));
            return;
        }
        self.fix_now(&v, &comment);
    }

    fn fix_now(self: &Rc<Self>, v: &Value, comment: &str) {
        let Some(vault) = self.on() else {
            self.say("Not changed: the Notes add-on is off (Settings → Notes).", true);
            self.refresh();
            return;
        };
        let Some(mut done) = scribe::proposal_from(v) else {
            self.refresh();
            self.run_next();
            return;
        };
        if self.store.borrow_mut().calls_left() == 0 {
            self.say("Today's limit of note checks is reached (Settings → Notes): your comment waits for tomorrow.", true);
            self.refresh();
            self.run_next();
            return;
        }
        self.running.set(true);
        self.status(&format!("Changing {} as you say…", done.change.note()));
        let (session, title) = (v["session"].as_str().unwrap_or("").to_owned(), v["title"].as_str().unwrap_or("").to_owned());
        let (comment, note) = (comment.to_owned(), done.change.note().to_owned());
        // A row of the history page is kept without its conversation: the history has it.
        let look_up = v["id"].as_u64().filter(|_| v.get("excerpt").is_none());
        let weak = Rc::downgrade(self);
        crate::rt::spawn_local(async move {
            let report = crate::rt::spawn_blocking(move || {
                if let Some(id) = look_up
                    && let Some(excerpt) = history().into_iter().find(|h| h["id"].as_u64() == Some(id)).and_then(|h| h["excerpt"].as_str().map(str::to_owned))
                {
                    done.excerpt = excerpt;
                }
                scribe::fix(&vault, &done, &comment)
            })
            .await;
            let Some(hub) = weak.upgrade() else { return };
            hub.running.set(false);
            match report {
                Ok(mut report) => {
                    let (failed, applied, held) = (report.failed.clone(), report.applied.len(), report.held.len());
                    report.session = session;
                    report.title = title;
                    hub.take(report);
                    if let Some(why) = failed {
                        hub.say(&format!("Not changed: {why}"), true);
                    } else if applied == 0 && held == 0 {
                        hub.say(&format!("Scribe found nothing to change in {note} for that."), true);
                    } else if applied == 0 {
                        hub.say(&format!("Scribe's change to {note} did not fit the note as it is: it waits for your OK in the Notes panel."), true);
                    } else if held > 0 {
                        hub.say(&format!("Changed {note} as you asked; {held} more waiting for your OK in the Notes panel."), true);
                    } else {
                        hub.say(&format!("Changed {note} as you asked."), false);
                    }
                }
                Err(_) => {
                    hub.say("Changing it stopped unexpectedly.", true);
                    hub.refresh();
                }
            }
            hub.run_next();
        });
    }

    /// The history to list: whether a change can be undone, not the notes themselves.
    pub fn listed(&self) -> Rc<Vec<Value>> {
        let length = std::fs::metadata(log_path()).map_or(0, |m| m.len());
        if self.listed.borrow().0 == length && length > 0 {
            return self.listed.borrow().1.clone();
        }
        let entries: Vec<Value> = history()
            .into_iter()
            .map(|mut v| {
                v["undoable"] = v["after"].is_string().into();
                if let Some(fields) = v.as_object_mut() {
                    fields.remove("before");
                    fields.remove("after");
                    fields.remove("excerpt");
                }
                v
            })
            .collect();
        let entries = Rc::new(entries);
        *self.listed.borrow_mut() = (length, entries.clone());
        entries
    }

    /// A written change undone (after the check running now, if one is).
    pub fn undo(self: &Rc<Self>, id: u64) {
        if self.running.get() {
            self.say("Undone as soon as the check that is running ends…", false);
            self.queue.borrow_mut().push_front(Job::Undo(id));
            return;
        }
        self.undo_now(id);
    }

    fn undo_now(self: &Rc<Self>, id: u64) {
        let Some(vault) = self.on() else {
            self.say("Not undone: the Notes add-on is off (Settings → Notes).", true);
            self.refresh();
            return;
        };
        // The history read and the note put back (a commit) off the main thread.
        self.running.set(true);
        let weak = Rc::downgrade(self);
        crate::rt::spawn_local(async move {
            let undone = crate::rt::spawn_blocking(move || {
                let entry = history().into_iter().find(|v| v["id"].as_u64() == Some(id) && v["kind"].as_str() == Some("written") && v["undone"].is_null() && v["after"].is_string());
                let Some((v, proposal)) = entry.and_then(|v| scribe::proposal_from(&v).map(|p| (v, p))) else {
                    return Err("Not undone: what this change did is not in the history.".to_owned());
                };
                let done = scribe::Written { before: v["before"].as_str().map(str::to_owned), after: v["after"].as_str().unwrap_or("").to_owned() };
                let what = format!("{}: {}", proposal.topic, if proposal.reason.is_empty() { proposal.change.summary() } else { proposal.reason.clone() });
                let note = proposal.change.note().to_owned();
                scribe::undo(&vault, &note, &done, &what).map_err(|why| format!("Not undone ({note}): {why}"))?;
                Ok((v, note, done.before))
            })
            .await
            .unwrap_or_else(|_| Err("Undoing it stopped unexpectedly.".to_owned()));
            let Some(hub) = weak.upgrade() else { return };
            hub.running.set(false);
            match undone {
                Ok((v, note, before)) => {
                    log_line(&json!({ "undo": id, "at": crate::state::now() }));
                    // A change the Notes chat made is not Scribe's to learn from.
                    if v["reason"] != "From the Notes chat" {
                        hub.offer_teaching(&v, before);
                    }
                    let mut store = hub.store.borrow_mut();
                    store.recent.retain(|r| r["id"].as_u64() != Some(id));
                    store.save();
                    drop(store);
                    hub.say(&format!("Undone: {note}. Tell Scribe why in the Notes panel, and it keeps it as a rule."), false);
                }
                Err(why) => hub.say(&why, true),
            }
            hub.refresh();
            hub.history_changed();
            hub.run_next();
        });
    }

    /// A held change written (approved) or dropped.
    pub fn decide(self: &Rc<Self>, id: u64, write: bool) {
        if self.running.get() {
            self.status("Done as soon as the check that is running ends…");
            self.queue.borrow_mut().push_front(Job::Decide(id, write));
            return;
        }
        self.decide_now(id, write);
    }

    fn decide_now(self: &Rc<Self>, id: u64, write: bool) {
        let Some(vault) = self.on() else { return };
        let taken = {
            let mut store = self.store.borrow_mut();
            let at = store.pending.iter().position(|p| p["id"].as_u64() == Some(id));
            at.map(|at| store.pending.remove(at))
        };
        let Some(p) = taken else { return };
        if !write {
            let mut dropped = p.clone();
            dropped["at"] = crate::state::now().into();
            log_change(&dropped, "dropped", None);
            let before = scribe::proposal_from(&p).and_then(|q| std::fs::read_to_string(vault.join(q.change.note())).ok());
            self.offer_teaching(&p, before);
        }
        // A change that no longer reads is dropped (and that is kept).
        let proposal = if write { scribe::proposal_from(&p) } else { None };
        let Some(proposal) = proposal else {
            self.store.borrow().save();
            self.refresh();
            return;
        };
        // Written (a commit) off the main thread.
        self.running.set(true);
        self.status(&format!("Writing {}…", proposal.change.note()));
        let weak = Rc::downgrade(self);
        // A line to replace that is gone (the note changed since: the user, a sync): Scribe looks
        // at it again, if today's calls allow, and asks again.
        let may_redo = self.store.borrow_mut().calls_left() > 0;
        crate::rt::spawn_local(async move {
            let note = proposal.change.note().to_owned();
            // (What came of it, and whether Claude was asked.)
            let (written, asked) = crate::rt::spawn_blocking(move || match scribe::approve(&vault, &proposal) {
                Ok(done) => (Ok(Ok(done)), false),
                Err(why) if may_redo && scribe::may_redo(&vault, &proposal) => {
                    (scribe::redo(&vault, &proposal).map(Err).map_err(|again| format!("{why} Looking at it again failed: {again}")), true)
                }
                Err(why) => (Err(why), false),
            })
            .await
            .unwrap_or_else(|_| (Err("it stopped unexpectedly".to_owned()), false));
            let Some(hub) = weak.upgrade() else { return };
            hub.running.set(false);
            if asked {
                hub.store.borrow_mut().calls += 1;
            }
            match written {
                Ok(Err(again)) => {
                    let mut dropped = p.clone();
                    dropped["at"] = crate::state::now().into();
                    log_change(&dropped, "dropped", None);
                    if again.is_empty() {
                        hub.status(&format!("{note} changed since, and needs this no more: nothing written."));
                    } else {
                        hub.status(&format!("{note} changed since: Scribe looked again, and asks again."));
                    }
                    let (session, title) = (p["session"].as_str().unwrap_or("").to_owned(), p["title"].as_str().unwrap_or("").to_owned());
                    let mut store = hub.store.borrow_mut();
                    for proposal in again {
                        let mut v = scribe::proposal_json(&proposal, "The note changed since you were asked: Scribe looked at it again.", &session, &title);
                        v["id"] = store.id().into();
                        store.pending.insert(0, v);
                    }
                }
                Ok(Ok(done)) => {
                    let mut p = p;
                    // Written now: its time is the writing's.
                    p["at"] = crate::state::now().into();
                    p["why"] = "".into();
                    log_change(&p, "written", Some(&done));
                    p["undoable"] = true.into();
                    p["unseen"] = true.into();
                    let mut store = hub.store.borrow_mut();
                    store.recent.insert(0, p);
                    store.recent.truncate(RECENT);
                    drop(store);
                    hub.status(&format!("Written: {note}"));
                }
                Err(why) => {
                    hub.status(&format!("Not written: {why}"));
                    let mut p = p;
                    p["why"] = format!("Not written: {why}").into();
                    p["failed"] = true.into();
                    hub.store.borrow_mut().pending.insert(0, p);
                }
            }
            hub.store.borrow().save();
            hub.refresh();
            hub.run_next();
        });
    }

    pub fn drop_question(self: &Rc<Self>, id: u64) {
        self.store.borrow_mut().questions.retain(|q| q["id"].as_u64() != Some(id));
        self.store.borrow().save();
        self.refresh();
    }

    /// An answer: the change Scribe was unsure of, made with it (off the main thread).
    pub fn answer(self: &Rc<Self>, id: u64, answer: &str) {
        if self.running.get() {
            self.status("Written down as soon as the check that is running ends…");
            self.queue.borrow_mut().push_front(Job::Answer(id, answer.to_owned()));
            return;
        }
        self.answer_now(id, answer);
    }

    /// The question stays until its answer is written: a call that fails leaves it to try
    /// again.
    fn answer_now(self: &Rc<Self>, id: u64, answer: &str) {
        let Some(vault) = self.on() else { return };
        let q = self.store.borrow().questions.iter().find(|q| q["id"].as_u64() == Some(id)).cloned();
        let Some(q) = q else {
            self.run_next();
            return;
        };
        if self.store.borrow_mut().calls_left() == 0 {
            self.status("Today's limit of note checks is reached (Settings → Notes); the question waits.");
            self.refresh();
            self.run_next();
            return;
        }
        let question = Question {
            topic: q["topic"].as_str().unwrap_or("").to_owned(),
            question: q["question"].as_str().unwrap_or("").to_owned(),
            options: Vec::new(),
            excerpt: q["excerpt"].as_str().unwrap_or("").to_owned(),
        };
        self.running.set(true);
        self.status("Writing your answer down…");
        let (session, title) = (q["session"].as_str().unwrap_or("").to_owned(), q["title"].as_str().unwrap_or("").to_owned());
        let answer = answer.to_owned();
        let (asked, said) = (q.clone(), answer.clone());
        let weak = Rc::downgrade(self);
        crate::rt::spawn_local(async move {
            let done = crate::rt::spawn_blocking(move || scribe::answer(&vault, &question, &answer)).await;
            let Some(hub) = weak.upgrade() else { return };
            hub.running.set(false);
            match done {
                Ok(mut report) => {
                    if report.failed.is_none() {
                        hub.store.borrow_mut().questions.retain(|q| q["id"].as_u64() != Some(id));
                        hub.offer_answer_rule(asked, said);
                    }
                    report.session = session;
                    report.title = title;
                    hub.take(report);
                }
                Err(_) => {
                    hub.status("Writing the answer stopped unexpectedly; the question waits.");
                    hub.refresh();
                }
            }
            hub.run_next();
        });
    }
}

/// The Scribe and the item's own id a command is for, if `who` may write to the vault: a
/// person, in loom's window (the CLI too in a test instance: whatever loom can't place under
/// an agent passes for the CLI, and Scribe's OKs and rules write the user's notes).
/// Scribe's commands are loom's window's (they write to the vault); a test instance's CLI too.
fn window_only(who: &crate::api::Principal) -> Result<(), crate::api::Error> {
    let cli = *who == crate::api::Principal::Cli && crate::api::test_instance();
    if *who != crate::api::Principal::Window && !cli {
        return Err(crate::api::Error::Forbidden("Scribe, but in loom's window".into()));
    }
    Ok(())
}

fn scribe_target(scribe: &Weak<Hub>, who: &crate::api::Principal, params: &Value) -> Result<(Rc<Hub>, u64), crate::api::Error> {
    window_only(who)?;
    let scribe = scribe.upgrade().ok_or_else(|| crate::api::Error::NotFound("Scribe".into()))?;
    let id = params["id"].as_str().and_then(|id| id.rsplit(':').next()).and_then(|id| id.parse().ok()).ok_or_else(|| crate::api::Error::Invalid("which item (its id)?".into()))?;
    Ok((scribe, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Error, Principal};

    #[test]
    fn scribe_shows_what_it_holds_and_only_the_window_acts_on_it() {
        let scribe = Hub::new();
        // What it holds, made up (nothing here writes to the vault).
        *scribe.store.borrow_mut() = Store {
            questions: vec![json!({ "id": 7, "topic": "Money", "question": "Which budget?", "options": ["2026"] })],
            pending: vec![json!({ "id": 9, "note": "Finances/Budget.md", "why": "a new figure" })],
            day: today(),
            calls: 2,
            ..Store::default()
        };
        scribe.serve();
        let items = crate::api::hub().store("scribe").items();
        let ids: Vec<&str> = items.iter().filter_map(|i| i["id"].as_str()).collect();
        assert_eq!(ids, ["status", "question:7", "proposal:9"]);
        assert_eq!(items[0]["calls_left"].as_u64(), Some(crate::settings::get().notes_daily_calls.saturating_sub(2)));
        assert_eq!((items[1]["list"].as_str(), items[1]["own_id"].as_u64(), items[1]["question"].as_str()), (Some("question"), Some(7), Some("Which budget?")));
        let phone = crate::api::hub().connect(Principal::Bridge("remote".into()));
        assert!(matches!(phone.command("c1", "scribe_decide", json!({ "id": "proposal:9", "write": true })), Err(Error::Forbidden(_))));
        let window = crate::api::hub().connect(Principal::Window);
        assert!(matches!(window.command("c2", "scribe_answer", json!({ "id": "question:8", "answer": "2026" })), Err(Error::Conflict(_))));
        assert!(matches!(window.command("c3", "scribe_decide", json!({ "id": "proposal:9" })), Err(Error::Invalid(_))));
    }
}
