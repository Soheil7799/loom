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
/// How many of Scribe's own changes are listed.
const RECENT: usize = 40;

/// Something that writes to the vault, done one at a time.
enum Job {
    /// Read a conversation (finished turns only, or all of it).
    Check(String, bool),
    /// A question answered.
    Answer(u64, String),
    /// A held change written (true) or dropped.
    Decide(u64, bool),
}

#[derive(Default)]
struct Store {
    /// Changes waiting for the user's OK (scribe::proposal_json + "id").
    pending: Vec<Value>,
    /// Questions: topic, question, options, excerpt, session, title, id.
    questions: Vec<Value>,
    /// What Scribe changed, newest first.
    recent: Vec<Value>,
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
        Store {
            pending: list("pending"),
            questions: list("questions"),
            recent: list("recent"),
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
        let text = json!({ "pending": self.pending, "questions": self.questions, "recent": self.recent, "day": self.day, "calls": self.calls, "nextId": self.next_id }).to_string();
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
        let hub = Rc::new(Hub {
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
            for p in &report.applied {
                let v = scribe::proposal_json(p, "", &report.session, &report.title);
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
        if store.questions.is_empty() && store.pending.is_empty() && store.recent.is_empty() {
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
            for p in &store.recent {
                self.body.append(&self.change_card(p, false));
            }
        }
        drop(store);
        if let Some(changed) = self.on_changed.borrow().as_ref() {
            changed();
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

    fn change_card(self: &Rc<Self>, p: &Value, waiting: bool) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("hub-card");
        let Some(proposal) = scribe::proposal_from(p) else { return card };
        card.append(&from_line(self, p));
        let what = gtk::Label::builder()
            .label(proposal.change.note())
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::Middle)
            .build();
        what.add_css_class("hub-note");
        card.append(&what);
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
                    button.set_label("Dropping…");
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
        if write {
            let Some(proposal) = scribe::proposal_from(&p) else { return };
            match scribe::approve(&vault, &proposal) {
                Ok(()) => {
                    let mut store = self.store.borrow_mut();
                    store.recent.insert(0, p.clone());
                    store.recent.truncate(RECENT);
                    self.status.set_label(&format!("Written: {}", proposal.change.note()));
                }
                Err(why) => {
                    self.status.set_label(&format!("Not written: {why}"));
                    let mut p = p;
                    p["why"] = format!("Not written: {why}").into();
                    p["failed"] = true.into();
                    self.store.borrow_mut().pending.insert(0, p);
                }
            }
        }
        self.store.borrow().save();
        self.refresh();
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
