//! The Notes add-on in the window: when Scribe checks a conversation, what it found, and
//! the Notes tab of the right panel (one for all agents) with its questions, the changes
//! waiting for an OK, and what it did.
//!
//! A check runs when a conversation goes quiet, when it has grown long, when it is about to
//! be compacted or closed, after loom starts (for what is left), or on Check now. One runs at
//! a time, off the main thread; the others wait in line.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::{gio, glib, pango, prelude::*};
use serde_json::{Value, json};

use crate::scribe::{self, Question, Report};

/// Quiet this long after a turn: the conversation is checked.
const IDLE: Duration = Duration::from_secs(120);
/// This much unread (bytes of journal): checked at once, even while it goes on.
const LONG: u64 = 24_000;
/// How many of Scribe's own changes the panel lists (the history page has them all).
const RECENT: usize = 5;
/// The history page lists this many at most (the newest).
const PAGE: usize = 400;

/// Something that writes to the vault, done one at a time.
enum Job {
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
struct Store {
    /// Changes waiting for the user's OK (scribe::proposal_json + "id").
    pending: Vec<Value>,
    /// Questions: topic, question, options, excerpt, session, title, id.
    questions: Vec<Value>,
    /// What Scribe changed, newest first.
    recent: Vec<Value>,
    /// Changes the user said no to or undid, each with a box to say why (it becomes a rule):
    /// the change (proposal_json), "noteBefore", "stage" (ask, drafting, draft), "draft".
    teach: Vec<Value>,
    /// The day (YYYY-MM-DD) and the calls to Claude made on it.
    day: String,
    calls: u64,
    next_id: u64,
}

impl Store {
    fn load() -> Store {
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

    fn save(&self) {
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

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Calls left today (the count starts again each day).
    fn calls_left(&mut self) -> usize {
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
fn log_path() -> PathBuf {
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
fn history() -> Vec<Value> {
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
fn when(at: u64) -> String {
    let Ok(time) = glib::DateTime::from_unix_local(at as i64) else { return String::new() };
    let format = if day_of(at) == today() { "%H:%M" } else { "%e %b, %H:%M" };
    time.format(format).map(|t| t.to_string().trim().replace("  ", " ")).unwrap_or_default()
}

/// "12:40".
fn clock(at: u64) -> String {
    glib::DateTime::from_unix_local(at as i64).ok().and_then(|t| t.format("%H:%M").ok()).map(|t| t.to_string()).unwrap_or_default()
}

fn day_of(at: u64) -> String {
    glib::DateTime::from_unix_local(at as i64).ok().and_then(|d| d.format("%Y-%m-%d").ok()).map(|d| d.to_string()).unwrap_or_default()
}

/// A day's heading on the history page: "Today", "Yesterday", "Monday 26 October".
fn day_heading(at: u64) -> String {
    let day = day_of(at);
    let yesterday = glib::DateTime::now_local().ok().and_then(|d| d.add_days(-1).ok()).and_then(|d| d.format("%Y-%m-%d").ok()).map(|d| d.to_string());
    if day == today() {
        "TODAY".to_owned()
    } else if Some(&day) == yesterday.as_ref() {
        "YESTERDAY".to_owned()
    } else {
        glib::DateTime::from_unix_local(at as i64).ok().and_then(|d| d.format("%A %e %B").ok()).map(|d| d.to_string().replace("  ", " ").to_uppercase()).unwrap_or(day)
    }
}

/// The corrections the user explained, as tests for `loom scribe-eval` (private: they hold
/// the user's notes and words, so they stay in loom's state folder, never in the vault).
pub fn corrections_path() -> PathBuf {
    crate::journal::store_path().with_file_name("scribe-corrections.jsonl")
}

fn today() -> String {
    glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%d").ok()).map(|d| d.to_string()).unwrap_or_default()
}

pub struct Hub {
    pub panel: gtk::Box,
    store: RefCell<Store>,
    running: Cell<bool>,
    /// What waits its turn: one thing at a time touches the vault (a check, an answer, an OK).
    queue: RefCell<VecDeque<Job>>,
    timers: RefCell<HashMap<String, glib::SourceId>>,
    /// The conversation the window shows: (session, title).
    current: RefCell<Option<(String, String)>>,
    status: gtk::Label,
    this_line: gtk::Label,
    check: gtk::Button,
    body: gtk::Box,
    /// Something new for the user (a question, a change to review): the app shows it.
    pub on_attention: RefCell<Option<Box<dyn Fn()>>>,
    /// The app shows a conversation (a question's "from").
    pub on_open: RefCell<Option<Box<dyn Fn(&str)>>>,
    /// The counts changed (sidebar, overview).
    pub on_changed: RefCell<Option<Box<dyn Fn()>>>,
    /// The history page (in the main area, where the chats are).
    pub page: gtk::Box,
    page_list: gtk::Box,
    page_search: gtk::SearchEntry,
    page_topic: gtk::DropDown,
    page_dropped: gtk::CheckButton,
    page_status: gtk::Label,
    /// The topics the topic filter offers ("All topics" first).
    page_topics: RefCell<Vec<String>>,
    /// The history as listed (the notes before and after left out), and the log's length
    /// when read: read again only when it grew.
    listed: RefCell<(u64, Rc<Vec<Value>>)>,
    /// The page is being filled (choosing the topic list fills it again otherwise).
    filling: Cell<bool>,
    /// Hides the page's status line a little after it was said.
    page_status_timer: RefCell<Option<glib::SourceId>>,
    /// The app shows the history page.
    pub on_history: RefCell<Option<Box<dyn Fn()>>>,
}

impl Hub {
    pub fn new() -> Rc<Hub> {
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 10);
        panel.add_css_class("hub");
        let head = gtk::Box::new(gtk::Orientation::Vertical, 6);
        head.add_css_class("hub-head");
        let status = gtk::Label::builder().xalign(0.0).wrap(true).build();
        status.add_css_class("small");
        status.add_css_class("dim");
        let this_line = gtk::Label::builder().xalign(0.0).wrap(true).hexpand(true).build();
        this_line.add_css_class("small");
        let check = gtk::Button::with_label("Check now");
        check.add_css_class("prefs-button");
        check.set_valign(gtk::Align::Center);
        check.set_tooltip_text(Some("Read what this conversation said since the last check, and keep it in your notes"));
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.append(&this_line);
        line.append(&check);
        head.append(&line);
        head.append(&status);
        panel.append(&head);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let scroll = gtk::ScrolledWindow::builder().child(&body).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        panel.append(&scroll);
        // The history page: every change, newest first, by day; searched, filtered, undone.
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.add_css_class("chat");
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 12);
        inner.add_css_class("overview");
        inner.add_css_class("notes-history");
        inner.set_valign(gtk::Align::Start);
        let title = gtk::Label::builder().label("Notes history").xalign(0.0).build();
        title.add_css_class("overview-title");
        let about = gtk::Label::builder()
            .label("Everything Scribe wrote in your notes, newest first. Undo puts a note back as it was; if it changed since, only that change's lines are taken out.")
            .xalign(0.0)
            .wrap(true)
            .build();
        about.add_css_class("dim");
        let page_search = gtk::SearchEntry::builder().placeholder_text("Search notes, reasons and lines").hexpand(true).build();
        let page_topic = gtk::DropDown::from_strings(&["All topics"]);
        page_topic.set_tooltip_text(Some("Only this topic's changes"));
        let page_dropped = gtk::CheckButton::with_label("Show what you said no to");
        page_dropped.set_tooltip_text(Some("Also the changes you didn't let Scribe write"));
        let filters = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        filters.append(&page_search);
        filters.append(&page_topic);
        filters.append(&page_dropped);
        let page_status = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
        page_status.add_css_class("small");
        let page_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
        inner.append(&title);
        inner.append(&about);
        inner.append(&filters);
        inner.append(&page_status);
        inner.append(&page_list);
        let page_scroll = gtk::ScrolledWindow::builder().child(&inner).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        page.append(&page_scroll);
        let hub = Rc::new(Hub {
            page,
            page_list,
            page_search: page_search.clone(),
            page_topic: page_topic.clone(),
            page_dropped: page_dropped.clone(),
            page_status,
            page_topics: RefCell::new(vec![String::new()]),
            page_status_timer: RefCell::default(),
            listed: RefCell::default(),
            filling: Cell::new(false),
            on_history: RefCell::default(),
            panel,
            store: RefCell::new(Store::load()),
            running: Cell::new(false),
            queue: RefCell::default(),
            timers: RefCell::default(),
            current: RefCell::default(),
            status,
            this_line,
            check: check.clone(),
            body,
            on_attention: RefCell::default(),
            on_open: RefCell::default(),
            on_changed: RefCell::default(),
        });
        // What the panel showed is seen once the user leaves it: folded from then on.
        let weak = Rc::downgrade(&hub);
        hub.panel.connect_unmap(move |_| {
            if let Some(hub) = weak.upgrade()
                && hub.mark_seen(None)
            {
                // Shown folded from the next look on (rebuilt once the switch is over).
                let weak = Rc::downgrade(&hub);
                glib::idle_add_local_once(move || {
                    if let Some(hub) = weak.upgrade() {
                        hub.refresh();
                    }
                });
            }
        });
        let weak = Rc::downgrade(&hub);
        page_search.connect_search_changed(move |_| {
            if let Some(hub) = weak.upgrade() {
                hub.filtered();
            }
        });
        let weak = Rc::downgrade(&hub);
        page_topic.connect_selected_notify(move |_| {
            if let Some(hub) = weak.upgrade() {
                hub.filtered();
            }
        });
        let weak = Rc::downgrade(&hub);
        page_dropped.connect_toggled(move |_| {
            if let Some(hub) = weak.upgrade() {
                hub.filtered();
            }
        });
        let weak = Rc::downgrade(&hub);
        check.connect_clicked(move |_| {
            if let Some(hub) = weak.upgrade() {
                let session = hub.current.borrow().as_ref().map(|(s, _)| s.clone());
                let Some(session) = session else { return };
                let busy = hub.running.get();
                hub.check.set_sensitive(false);
                hub.check.set_label(if busy { "Waiting…" } else { "Checking…" });
                if busy {
                    hub.status.set_label("This conversation is read as soon as the check running now ends…");
                }
                hub.check_soon(&session, false, Duration::ZERO);
                // Nothing started: there was nothing new to read.
                if !busy && !hub.running.get() {
                    hub.status.set_label("Nothing new to read since the last check.");
                    hub.refresh_head();
                }
            }
        });
        hub.refresh();
        // What was left unread when loom closed: caught up a little after it starts.
        let weak = Rc::downgrade(&hub);
        glib::timeout_add_local_once(Duration::from_secs(20), move || {
            if let Some(hub) = weak.upgrade() {
                // Nothing is mid-turn yet: what is left is read whole (a last message whose
                // answer never came included).
                for session in crate::journal::waiting() {
                    hub.check_soon(&session, false, Duration::ZERO);
                }
            }
        });
        // The "since the last check" line ages.
        let weak = Rc::downgrade(&hub);
        glib::timeout_add_seconds_local(30, move || match weak.upgrade() {
            Some(hub) => {
                hub.refresh_head();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        hub
    }

    fn on(&self) -> Option<PathBuf> {
        crate::vault::current()
    }

    /// The window shows this conversation now.
    pub fn set_current(&self, session: Option<String>, title: &str) {
        *self.current.borrow_mut() = session.map(|s| (s, title.to_owned()));
        self.refresh_head();
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
        let id = glib::timeout_add_local_once(after, move || {
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
        self.status.set_label(&format!("Reading “{title}”…"));
        self.refresh_head();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let checked = gio::spawn_blocking(move || scribe::check(&vault, &session, &entries, calls_left)).await;
            let Some(hub) = weak.upgrade() else { return };
            hub.running.set(false);
            match checked {
                Ok(report) => hub.take(report),
                Err(_) => hub.status.set_label("The check stopped unexpectedly."),
            }
            hub.run_next();
        });
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
        self.status.set_label(&match &report.failed {
            Some(why) => format!("Stopped: {why}"),
            None if did.is_empty() => format!("Checked “{}”: nothing to keep.", report.title),
            None => format!("Checked “{}”: {}.", report.title, did.join(", ")),
        });
        self.refresh();
        if news {
            if let Some(attention) = self.on_attention.borrow().as_ref() {
                attention();
            }
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

    fn refresh_head(&self) {
        let current = self.current.borrow().clone();
        match (self.on(), current) {
            (None, _) => {
                self.this_line.set_label("Notes is off (Settings → Notes).");
                self.check.set_visible(false);
            }
            (Some(_), None) => {
                self.this_line.set_label("Open an agent to see what Scribe has left to read of it.");
                self.check.set_visible(false);
            }
            (Some(_), Some((session, _))) => {
                let unread = crate::journal::unread_summary(&session);
                self.this_line.set_label(&match unread {
                    None => "This conversation is in your notes up to its last message.".to_owned(),
                    Some((bytes, at)) => format!(
                        "~{}k characters not read yet · since {}",
                        bytes.div_ceil(1000),
                        crate::overview::ago(crate::state::now().saturating_sub(at))
                    ),
                });
                // While a check runs it says so (one started by hand or by itself).
                let running = self.running.get();
                self.check.set_visible(unread.is_some() || running);
                self.check.set_sensitive(!running);
                self.check.set_label(if running { "Checking…" } else { "Check now" });
            }
        }
        if !self.running.get() && self.status.label().is_empty() {
            self.status.set_label("Scribe keeps what you tell your agents in your notes.");
        }
    }

    /// Builds the panel's lists again.
    pub fn refresh(self: &Rc<Self>) {
        self.refresh_head();
        while let Some(child) = self.body.first_child() {
            self.body.remove(&child);
        }
        let store = self.store.borrow();
        if store.questions.is_empty() && store.pending.is_empty() && store.recent.is_empty() && store.teach.is_empty() {
            let empty = gtk::Label::builder()
                .label("Questions Scribe has, changes waiting for your OK, and what it wrote in your notes show up here.")
                .wrap(true)
                .justify(gtk::Justification::Center)
                .max_width_chars(30)
                .margin_top(24)
                .build();
            empty.add_css_class("dim");
            self.body.append(&empty);
        }
        if !store.teach.is_empty() {
            self.body.append(&heading("WHY NOT?"));
            for t in &store.teach {
                self.body.append(&self.teach_card(t));
            }
        }
        if !store.questions.is_empty() {
            self.body.append(&heading("QUESTIONS"));
            for q in &store.questions {
                self.body.append(&self.question_card(q));
            }
        }
        if !store.pending.is_empty() {
            self.body.append(&heading("WAITING FOR YOUR OK"));
            for p in &store.pending {
                self.body.append(&self.change_card(p, true));
            }
        }
        if !store.recent.is_empty() {
            self.body.append(&heading("WRITTEN"));
            let rows = gtk::Box::new(gtk::Orientation::Vertical, 4);
            for p in store.recent.iter().take(RECENT) {
                rows.append(&self.change_row(p, true));
            }
            self.body.append(&rows);
        }
        if log_path().exists() {
            let all = gtk::Button::builder().label("Full history").halign(gtk::Align::Start).build();
            all.add_css_class("prefs-button");
            all.set_tooltip_text(Some("Everything Scribe wrote, searched and undone, in the main area"));
            let weak = Rc::downgrade(self);
            all.connect_clicked(move |_| {
                if let Some(hub) = weak.upgrade()
                    && let Some(show) = hub.on_history.borrow().as_ref()
                {
                    show();
                }
            });
            self.body.append(&all);
        }
        drop(store);
        if let Some(changed) = self.on_changed.borrow().as_ref() {
            changed();
        }
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

    fn teach_card(self: &Rc<Self>, t: &Value) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("hub-card");
        card.add_css_class("ask");
        let id = t["id"].as_u64().unwrap_or(0);
        let Some(proposal) = scribe::proposal_from(t) else { return card };
        card.append(&from_line(self, t));
        let what = gtk::Label::builder().label(format!("{} · {}", proposal.change.note(), proposal.change.summary())).xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).build();
        what.add_css_class("hub-note");
        card.append(&what);
        let diff = gtk::Label::builder().label(proposal.change.diff()).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build();
        diff.add_css_class("hub-diff");
        diff.add_css_class("mono");
        card.append(&diff);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let weak = Rc::downgrade(self);
        match t["stage"].as_str() {
            Some("drafting") => {
                let busy = gtk::Label::builder().label("Drafting a rule…").xalign(0.0).build();
                busy.add_css_class("dim");
                card.append(&busy);
                return card;
            }
            Some("draft") => {
                let says = gtk::Label::builder().label(format!("A rule for {}, kept in its topic file (change it as you like):", proposal.topic)).xalign(0.0).wrap(true).build();
                says.add_css_class("small");
                card.append(&says);
                let rule = gtk::Entry::builder().text(t["draft"].as_str().unwrap_or("")).hexpand(true).build();
                let keep = gtk::Button::with_label("Keep it");
                keep.add_css_class("suggested-action");
                let (entry, chosen) = (rule.clone(), card.downgrade());
                let kept = move |hub: &Rc<Hub>| {
                    let text = entry.text().trim().to_owned();
                    if !text.is_empty() {
                        if let Some(card) = chosen.upgrade() {
                            card.set_sensitive(false);
                        }
                        hub.keep_rule(id, &text);
                    }
                };
                let kept = Rc::new(kept);
                let (w, k) = (weak.clone(), kept.clone());
                rule.connect_activate(move |_| {
                    if let Some(hub) = w.upgrade() {
                        k(&hub);
                    }
                });
                let w = weak.clone();
                keep.connect_clicked(move |_| {
                    if let Some(hub) = w.upgrade() {
                        kept(&hub);
                    }
                });
                card.append(&rule);
                line.append(&keep);
            }
            _ => {
                let says = gtk::Label::builder()
                    .label(format!("Tell Scribe why, and it keeps it as a rule for {} (and as a test it must pass). Optional.", proposal.topic))
                    .xalign(0.0)
                    .wrap(true)
                    .build();
                says.add_css_class("small");
                card.append(&says);
                let why = gtk::Entry::builder().placeholder_text("Why not? Enter").hexpand(true).build();
                let (w, chosen) = (weak.clone(), card.downgrade());
                why.connect_activate(move |entry| {
                    let text = entry.text().trim().to_owned();
                    if let (Some(hub), false) = (w.upgrade(), text.is_empty()) {
                        if let Some(card) = chosen.upgrade() {
                            card.set_sensitive(false);
                        }
                        hub.teach(id, &text);
                    }
                });
                line.append(&why);
            }
        }
        let skip = gtk::Button::with_label(if t["stage"].as_str() == Some("draft") { "Drop" } else { "Skip" });
        skip.add_css_class("flat");
        skip.connect_clicked(move |_| {
            if let Some(hub) = weak.upgrade() {
                hub.forget_teaching(id);
            }
        });
        line.append(&skip);
        card.append(&line);
        card
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

    fn forget_teaching(self: &Rc<Self>, id: u64) {
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
    fn teach(self: &Rc<Self>, id: u64, said: &str) {
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
        glib::spawn_future_local(async move {
            let drafted = gio::spawn_blocking(move || scribe::rule_from(&vault, &proposal.topic, &proposal.change, &proposal.reason, &said)).await;
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

    fn keep_rule(self: &Rc<Self>, id: u64, rule: &str) {
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

    fn question_card(self: &Rc<Self>, q: &Value) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("hub-card");
        card.add_css_class("ask");
        let id = q["id"].as_u64().unwrap_or(0);
        card.append(&from_line(self, q));
        let text = gtk::Label::builder().label(q["question"].as_str().unwrap_or("")).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build();
        text.add_css_class("hub-question");
        card.append(&text);
        let options = gtk::Box::new(gtk::Orientation::Vertical, 4);
        for option in q["options"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let button = gtk::Button::with_label(option);
            button.add_css_class("prefs-button");
            let (weak, option, chosen) = (Rc::downgrade(self), option.to_owned(), card.downgrade());
            button.connect_clicked(move |button| {
                if let Some(hub) = weak.upgrade() {
                    button.add_css_class("chosen");
                    if let Some(card) = chosen.upgrade() {
                        card.set_sensitive(false);
                    }
                    hub.answer(id, &option);
                }
            });
            if let Some(label) = button.child().and_downcast::<gtk::Label>() {
                label.set_wrap(true);
                label.set_xalign(0.0);
            }
            options.append(&button);
        }
        // An answer of one's own (another currency, a date…).
        let own = gtk::Entry::builder().placeholder_text("Or your own answer, Enter").hexpand(true).build();
        let (weak, chosen) = (Rc::downgrade(self), card.downgrade());
        own.connect_activate(move |entry| {
            let text = entry.text().trim().to_owned();
            if let (Some(hub), false) = (weak.upgrade(), text.is_empty()) {
                if let Some(card) = chosen.upgrade() {
                    card.set_sensitive(false);
                }
                hub.answer(id, &text);
            }
        });
        let skip = gtk::Button::with_label("Skip");
        skip.add_css_class("flat");
        let weak = Rc::downgrade(self);
        skip.connect_clicked(move |_| {
            if let Some(hub) = weak.upgrade() {
                hub.drop_question(id);
            }
        });
        card.append(&options);
        let last = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        last.append(&own);
        last.append(&skip);
        card.append(&last);
        card
    }

    /// A written (or dropped) change, folded to one line: the note, what was done, when.
    /// Unfolded: where it came from, why, the lines, and Undo.
    /// `dated`: the day said too (not under a day's heading).
    fn change_row(self: &Rc<Self>, v: &Value, dated: bool) -> gtk::Expander {
        let row = gtk::Expander::new(None);
        row.add_css_class("hub-row");
        let Some(proposal) = scribe::proposal_from(v) else { return row };
        let (undone, dropped) = (v["undone"].as_bool() == Some(true), v["kind"].as_str() == Some("dropped"));
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let note = gtk::Label::builder().label(proposal.change.note()).xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).build();
        note.add_css_class("hub-note");
        let mut said = vec![proposal.change.summary()];
        if dropped {
            said.push("not written".to_owned());
        } else if undone {
            said.push("undone".to_owned());
        }
        let at = v["at"].as_u64().unwrap_or(0);
        said.push(if dated { when(at) } else { clock(at) });
        let what = gtk::Label::new(Some(&said.join(" · ")));
        what.add_css_class("small");
        what.add_css_class("dim");
        line.append(&note);
        line.append(&what);
        row.set_label_widget(Some(&line));
        if undone || dropped {
            row.add_css_class("gone");
        }
        // Its card is made when it is first unfolded (a long history lists many).
        let (weak, v) = (Rc::downgrade(self), v.clone());
        let unseen = v["unseen"].as_bool() == Some(true);
        row.connect_expanded_notify(move |row| {
            // Folded by hand: seen.
            if !row.is_expanded() {
                if let (Some(hub), Some(id)) = (weak.upgrade(), v["id"].as_u64()) {
                    hub.mark_seen(Some(id));
                }
                return;
            }
            if row.child().is_some() {
                return;
            }
            let Some(hub) = weak.upgrade() else { return };
            let card = hub.change_card(&v, false);
            // Undo: only a change written, not undone yet, and kept with what it did.
            if !undone && !dropped && (v["after"].is_string() || v["undoable"].as_bool() == Some(true)) {
                let undo = gtk::Button::builder().label("Undo").halign(gtk::Align::Start).build();
                undo.add_css_class("prefs-button");
                undo.set_tooltip_text(Some("Put the note back as it was before this change"));
                let (weak, id) = (Rc::downgrade(&hub), v["id"].as_u64().unwrap_or(0));
                undo.connect_clicked(move |button| {
                    if let Some(hub) = weak.upgrade() {
                        button.set_sensitive(false);
                        button.set_label("Undoing…");
                        hub.undo(id);
                    }
                });
                card.append(&undo);
                // A comment on it: Scribe changes the note as the user says.
                let comment = gtk::Entry::builder().placeholder_text("Ask Scribe to change it… Enter").hexpand(true).build();
                comment.set_tooltip_text(Some("What's wrong or missing in this change (\"it's net, not gross\"): Scribe changes the note that way at once"));
                let (weak, row_data, held) = (Rc::downgrade(&hub), v.clone(), card.downgrade());
                comment.connect_activate(move |entry| {
                    let text = entry.text().trim().to_owned();
                    if let (Some(hub), false) = (weak.upgrade(), text.is_empty()) {
                        if let Some(card) = held.upgrade() {
                            card.set_sensitive(false);
                        }
                        hub.fix(row_data.clone(), text);
                    }
                });
                card.append(&comment);
            }
            row.set_child(Some(&card));
        });
        // Not seen yet: open (its card made now).
        if unseen {
            row.set_expanded(true);
        }
        row
    }

    /// Seen: one written change (by id), or all the panel shows (None).
    fn mark_seen(&self, id: Option<u64>) -> bool {
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
    fn fix(self: &Rc<Self>, v: Value, comment: String) {
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
        self.status.set_label(&format!("Changing {} as you say…", done.change.note()));
        let (session, title) = (v["session"].as_str().unwrap_or("").to_owned(), v["title"].as_str().unwrap_or("").to_owned());
        let (comment, note) = (comment.to_owned(), done.change.note().to_owned());
        // A row of the history page is kept without its conversation: the history has it.
        let look_up = v["id"].as_u64().filter(|_| v.get("excerpt").is_none());
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let report = gio::spawn_blocking(move || {
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
    fn listed(&self) -> Rc<Vec<Value>> {
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

    /// The history page, filled again (as searched and filtered).
    pub fn fill_page(self: &Rc<Self>) {
        if self.filling.replace(true) {
            return;
        }
        self.fill_page_now();
        self.filling.set(false);
    }

    fn fill_page_now(self: &Rc<Self>) {
        let entries = self.listed();
        // The topic filter offers the topics there are changes for.
        let mut topics: Vec<String> = entries.iter().filter_map(|v| v["topic"].as_str().map(str::to_owned)).collect();
        topics.sort();
        topics.dedup();
        topics.insert(0, String::new());
        if *self.page_topics.borrow() != topics {
            let chosen = self.page_topics.borrow().get(self.page_topic.selected() as usize).cloned().unwrap_or_default();
            *self.page_topics.borrow_mut() = topics.clone();
            let names: Vec<&str> = topics.iter().map(|t| if t.is_empty() { "All topics" } else { t.as_str() }).collect();
            // Setting the model selects its first entry: the page is filled once more then.
            self.page_topic.set_model(Some(&gtk::StringList::new(&names)));
            self.page_topic.set_selected(topics.iter().position(|t| *t == chosen).unwrap_or(0) as u32);
        }
        let topic = self.page_topics.borrow().get(self.page_topic.selected() as usize).cloned().unwrap_or_default();
        let search = self.page_search.text().to_lowercase();
        let dropped = self.page_dropped.is_active();
        while let Some(child) = self.page_list.first_child() {
            self.page_list.remove(&child);
        }
        let mut day = String::new();
        let mut shown = 0;
        for v in entries.iter() {
            if !dropped && v["kind"].as_str() == Some("dropped") {
                continue;
            }
            if !topic.is_empty() && v["topic"].as_str() != Some(topic.as_str()) {
                continue;
            }
            if !search.is_empty() {
                let Some(p) = scribe::proposal_from(v) else { continue };
                let haystack = format!("{} {} {} {}", p.change.note(), p.reason, p.change.diff(), v["title"].as_str().unwrap_or("")).to_lowercase();
                if !haystack.contains(&search) {
                    continue;
                }
            }
            let at = v["at"].as_u64().unwrap_or(0);
            if day_of(at) != day {
                day = day_of(at);
                self.page_list.append(&heading(&day_heading(at)));
            }
            self.page_list.append(&self.change_row(v, false));
            shown += 1;
            if shown == PAGE {
                break;
            }
        }
        if shown == 0 {
            let empty = gtk::Label::builder()
                .label(if entries.is_empty() { "Nothing written yet." } else { "No change matches." })
                .xalign(0.0)
                .margin_top(12)
                .build();
            empty.add_css_class("dim");
            self.page_list.append(&empty);
        }
    }

    pub fn focus_page(&self) {
        self.page_search.grab_focus();
    }

    /// A written change undone (after the check running now, if one is).
    fn undo(self: &Rc<Self>, id: u64) {
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
        glib::spawn_future_local(async move {
            let undone = gio::spawn_blocking(move || {
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
                    hub.offer_teaching(&v, before);
                    let mut store = hub.store.borrow_mut();
                    store.recent.retain(|r| r["id"].as_u64() != Some(id));
                    store.save();
                    drop(store);
                    hub.say(&format!("Undone: {note}. Tell Scribe why in the Notes panel, and it keeps it as a rule."), false);
                }
                Err(why) => hub.say(&why, true),
            }
            hub.refresh();
            if hub.page.is_mapped() {
                hub.fill_page();
            }
            hub.run_next();
        });
    }

    /// Said in the panel and on the history page (there for a few seconds; an error until
    /// the next search or filter).
    fn say(self: &Rc<Self>, text: &str, error: bool) {
        self.status.set_label(text);
        self.page_status.set_label(text);
        self.page_status.set_visible(true);
        if let Some(timer) = self.page_status_timer.take() {
            timer.remove();
        }
        if error {
            self.page_status.add_css_class("error");
        } else {
            self.page_status.remove_css_class("error");
            let weak = Rc::downgrade(self);
            let timer = glib::timeout_add_local_once(Duration::from_secs(6), move || {
                if let Some(hub) = weak.upgrade() {
                    hub.page_status_timer.take();
                    hub.page_status.set_visible(false);
                }
            });
            *self.page_status_timer.borrow_mut() = Some(timer);
        }
    }

    /// Searched or filtered: the page's status line is old news.
    fn filtered(self: &Rc<Self>) {
        if self.filling.get() {
            return;
        }
        if let Some(timer) = self.page_status_timer.take() {
            timer.remove();
        }
        self.page_status.set_visible(false);
        self.fill_page();
    }

    fn change_card(self: &Rc<Self>, p: &Value, waiting: bool) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("hub-card");
        let Some(proposal) = scribe::proposal_from(p) else { return card };
        card.append(&from_line(self, p));
        // A written one is unfolded from a row that names its note already.
        if waiting {
            let what = gtk::Label::builder()
                .label(proposal.change.note())
                .xalign(0.0)
                .ellipsize(pango::EllipsizeMode::Middle)
                .build();
            what.add_css_class("hub-note");
            card.append(&what);
        }
        if !proposal.reason.is_empty() {
            let reason = gtk::Label::builder().label(&proposal.reason).xalign(0.0).wrap(true).build();
            reason.add_css_class("small");
            card.append(&reason);
        }
        let diff = gtk::Label::builder().label(proposal.change.diff()).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).selectable(true).build();
        diff.add_css_class("hub-diff");
        diff.add_css_class("mono");
        card.append(&diff);
        if waiting {
            if let Some(why) = p["why"].as_str().filter(|w| !w.is_empty()) {
                let why = gtk::Label::builder().label(why).xalign(0.0).wrap(true).build();
                why.add_css_class("small");
                why.add_css_class(if p["failed"].as_bool() == Some(true) { "error" } else { "dim" });
                card.append(&why);
            }
            let id = p["id"].as_u64().unwrap_or(0);
            let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let approve = gtk::Button::with_label("Write it");
            approve.add_css_class("suggested-action");
            let reject = gtk::Button::with_label("Don't");
            reject.add_css_class("prefs-button");
            let weak = Rc::downgrade(self);
            // Pressed: said at once (it may wait for a check that is running).
            let pressed = buttons.clone();
            approve.connect_clicked(move |button| {
                if let Some(hub) = weak.upgrade() {
                    pressed.set_sensitive(false);
                    button.set_label("Writing…");
                    hub.decide(id, true);
                }
            });
            let weak = Rc::downgrade(self);
            let pressed = buttons.clone();
            reject.connect_clicked(move |button| {
                if let Some(hub) = weak.upgrade() {
                    pressed.set_sensitive(false);
                    button.set_label("Not writing…");
                    hub.decide(id, false);
                }
            });
            buttons.append(&approve);
            buttons.append(&reject);
            card.append(&buttons);
        } else {
            card.add_css_class("written");
        }
        card
    }

    /// A held change written (approved) or dropped.
    fn decide(self: &Rc<Self>, id: u64, write: bool) {
        if self.running.get() {
            self.status.set_label("Done as soon as the check that is running ends…");
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
        self.status.set_label(&format!("Writing {}…", proposal.change.note()));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let note = proposal.change.note().to_owned();
            let written = gio::spawn_blocking(move || scribe::approve(&vault, &proposal)).await.unwrap_or_else(|_| Err("it stopped unexpectedly".to_owned()));
            let Some(hub) = weak.upgrade() else { return };
            hub.running.set(false);
            match written {
                Ok(done) => {
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
                    hub.status.set_label(&format!("Written: {note}"));
                }
                Err(why) => {
                    hub.status.set_label(&format!("Not written: {why}"));
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

    fn drop_question(self: &Rc<Self>, id: u64) {
        self.store.borrow_mut().questions.retain(|q| q["id"].as_u64() != Some(id));
        self.store.borrow().save();
        self.refresh();
    }

    /// An answer: the change Scribe was unsure of, made with it (off the main thread).
    fn answer(self: &Rc<Self>, id: u64, answer: &str) {
        if self.running.get() {
            self.status.set_label("Written down as soon as the check that is running ends…");
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
            self.status.set_label("Today's limit of note checks is reached (Settings → Notes); the question waits.");
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
        self.status.set_label("Writing your answer down…");
        let (session, title) = (q["session"].as_str().unwrap_or("").to_owned(), q["title"].as_str().unwrap_or("").to_owned());
        let answer = answer.to_owned();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let done = gio::spawn_blocking(move || scribe::answer(&vault, &question, &answer)).await;
            let Some(hub) = weak.upgrade() else { return };
            hub.running.set(false);
            match done {
                Ok(mut report) => {
                    if report.failed.is_none() {
                        hub.store.borrow_mut().questions.retain(|q| q["id"].as_u64() != Some(id));
                    }
                    report.session = session;
                    report.title = title;
                    hub.take(report);
                }
                Err(_) => {
                    hub.status.set_label("Writing the answer stopped unexpectedly; the question waits.");
                    hub.refresh();
                }
            }
            hub.run_next();
        });
    }
}

fn heading(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    label.add_css_class("hub-heading");
    label
}

/// "Finances · from Forecast endpoint", the conversation's name a link to it.
fn from_line(hub: &Rc<Hub>, v: &Value) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let topic = gtk::Label::new(Some(v["topic"].as_str().unwrap_or("")));
    topic.add_css_class("hub-topic");
    line.append(&topic);
    let title = v["title"].as_str().unwrap_or("").to_owned();
    if !title.is_empty() {
        let from = gtk::Label::new(Some("· from"));
        from.add_css_class("small");
        from.add_css_class("dim");
        line.append(&from);
        let link = gtk::Button::builder().label(&title).build();
        link.add_css_class("flat");
        link.add_css_class("hub-from");
        if let Some(label) = link.child().and_downcast::<gtk::Label>() {
            label.set_ellipsize(pango::EllipsizeMode::End);
        }
        let (weak, session) = (Rc::downgrade(hub), v["session"].as_str().unwrap_or("").to_owned());
        link.connect_clicked(move |_| {
            if let Some(hub) = weak.upgrade() {
                if let Some(open) = hub.on_open.borrow().as_ref() {
                    open(&session);
                }
            }
        });
        line.append(&link);
    }
    line
}
