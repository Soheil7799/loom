//! The Notes add-on in the window: when Scribe checks a conversation, what it found, and
//! the Notes tab of the right panel (one for all agents) with its questions, the changes
//! waiting for an OK, and what it did.
//!
//! A check runs when a conversation goes quiet, when it has grown long, when it is about to
//! be compacted or closed, after loom starts (for what is left), or on Check now. One runs at
//! a time, off the main thread; the others wait in line.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::{glib, pango, prelude::*};
use serde_json::{Value, json};

use loom_core::hub::{HubView, PAGE, RECENT, clock, day_heading, day_of, log_path, when};

use crate::scribe;

pub struct Hub {
    pub panel: gtk::Box,
    /// Scribe at work (in loom-core).
    pub core: Rc<loom_core::hub::Hub>,
    /// The core API's `scribe`, followed: what the panel's lists draw (warp).
    feed: RefCell<Option<crate::feed::Feed>>,
    /// Something new waits in it: drawn once the window is idle.
    due: Cell<bool>,
    /// loomd's Scribe (the window as its client): what it had for the user last (questions,
    /// proposals, changes not seen), to mark the Notes tab when that grows.
    news: Cell<Option<usize>>,
    /// Itself (for what the core calls with only a reference).
    me: RefCell<std::rc::Weak<Hub>>,
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
            .label("Everything Scribe and the Notes chat wrote in your notes, newest first. Undo puts a note back as it was; if it changed since, only that change's lines are taken out.")
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
            filling: Cell::new(false),
            on_history: RefCell::default(),
            panel,
            core: loom_core::hub::Hub::new(),
            feed: RefCell::default(),
            due: Cell::new(false),
            news: Cell::new(None),
            me: RefCell::default(),
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
                let busy = hub.running();
                hub.check.set_sensitive(false);
                hub.check.set_label(if busy { "Waiting…" } else { "Checking…" });
                if busy {
                    hub.status.set_label("This conversation is read as soon as the check running now ends…");
                }
                // loomd's Scribe: it says what it does in `scribe`.
                scribe("scribe_check", json!({ "session": session }));
            }
        });
        let view: std::rc::Weak<Hub> = Rc::downgrade(&hub);
        *hub.me.borrow_mut() = view.clone();
        // Scribe in the core API (warp): the panel's lists draw from it. loomd's client: Scribe
        // is loomd's alone (one here would write in the same vault).
        let weak = Rc::downgrade(&hub);
        *hub.feed.borrow_mut() = crate::feed::Feed::new("scribe", move || {
            let Some(hub) = weak.upgrade() else { return };
            if !hub.due.replace(true) {
                let weak = Rc::downgrade(&hub);
                glib::idle_add_local_once(move || {
                    if let Some(hub) = weak.upgrade()
                        && hub.due.get()
                    {
                        hub.refresh();
                    }
                });
            }
        });
        hub.refresh();
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

    /// The conversation the window shows (its session, once it has one; none: no agent).
    pub fn set_current(&self, session: Option<String>, title: &str) {
        let now = session.map(|s| (s, title.to_owned()));
        if *self.current.borrow() != now {
            *self.current.borrow_mut() = now;
            self.refresh_head();
        }
    }

    fn refresh_head(&self) {
        let current = self.current.borrow().clone();
        match (self.core.on(), current) {
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
                let running = self.running();
                self.check.set_visible(unread.is_some() || running);
                self.check.set_sensitive(!running);
                self.check.set_label(if running { "Checking…" } else { "Check now" });
            }
        }
        if !self.running() && self.status.label().is_empty() {
            self.status.set_label("Scribe keeps what you tell your agents in your notes.");
        }
    }

    /// Builds the panel's lists again.
    pub fn refresh(self: &Rc<Self>) {
        self.refresh_head();
        while let Some(child) = self.body.first_child() {
            self.body.remove(&child);
        }
        // What the core's `scribe` has now (each with Scribe's own number as its id, as the
        // cards act by it).
        self.due.set(false);
        let items: Vec<Value> = match self.feed.borrow().as_ref() {
            Some(feed) => {
                feed.update();
                feed.items().clone()
            }
            None => Vec::new(),
        };
        let list = |name: &str| -> Vec<Value> {
            items
                .iter()
                .filter(|i| i["list"] == name)
                .map(|i| {
                    let mut own = i.clone();
                    own["id"] = i["own_id"].clone();
                    own
                })
                .collect()
        };
        let (questions, pending, recent, teach) = (list("question"), list("proposal"), list("change"), list("teach"));
        self.remote_news(&items, questions.len() + pending.len() + recent.iter().filter(|c| c["unseen"] == true).count());

        if questions.is_empty() && pending.is_empty() && recent.is_empty() && teach.is_empty() {
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
        let answered = |t: &&Value| t["kind"].as_str() == Some("answer");
        if teach.iter().any(|t| !answered(&t)) {
            self.body.append(&heading("WHY NOT?"));
            for t in teach.iter().filter(|t| !answered(t)) {
                self.body.append(&self.teach_card(t));
            }
        }
        if teach.iter().any(|t| answered(&t)) {
            self.body.append(&heading("RULES FROM YOUR ANSWERS"));
            for t in teach.iter().filter(answered) {
                self.body.append(&self.teach_card(t));
            }
        }
        if !questions.is_empty() {
            self.body.append(&heading("QUESTIONS"));
            for q in &questions {
                self.body.append(&self.question_card(q));
            }
        }
        if !pending.is_empty() {
            self.body.append(&heading("WAITING FOR YOUR OK"));
            for p in &pending {
                self.body.append(&self.change_card(p, true));
            }
        }
        if !recent.is_empty() {
            self.body.append(&heading("WRITTEN"));
            let rows = gtk::Box::new(gtk::Orientation::Vertical, 4);
            for p in recent.iter().take(RECENT) {
                rows.append(&self.change_row(p, true));
            }
            self.body.append(&rows);
        }
        if log_path().exists() {
            let all = gtk::Button::builder().label("Full history").halign(gtk::Align::Start).build();
            all.add_css_class("prefs-button");
            all.set_tooltip_text(Some("Everything Scribe and the Notes chat wrote, searched and undone, in the main area"));
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
        if let Some(changed) = self.on_changed.borrow().as_ref() {
            changed();
        }
    }

    fn teach_card(self: &Rc<Self>, t: &Value) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("hub-card");
        card.add_css_class("ask");
        let id = t["id"].as_u64().unwrap_or(0);
        // An answer to a question that makes a rule, or a change said no to.
        let answered = t["kind"].as_str() == Some("answer");
        let proposal = scribe::proposal_from(t);
        if !answered && proposal.is_none() {
            return card;
        }
        let topic = t["topic"].as_str().unwrap_or("").to_owned();
        card.append(&from_line(self, t));
        if let Some(proposal) = proposal.filter(|_| !answered) {
            let what = gtk::Label::builder().label(format!("{} · {}", proposal.change.note(), proposal.change.summary())).xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).build();
            what.add_css_class("hub-note");
            card.append(&what);
            let diff = gtk::Label::builder().label(proposal.change.diff()).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build();
            diff.add_css_class("hub-diff");
            diff.add_css_class("mono");
            card.append(&diff);
        } else {
            let question = gtk::Label::builder().label(t["question"].as_str().unwrap_or("")).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build();
            question.add_css_class("hub-question");
            card.append(&question);
            let said = gtk::Label::builder().label(format!("You answered: {}", t["answer"].as_str().unwrap_or(""))).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build();
            said.add_css_class("small");
            said.add_css_class("dim");
            card.append(&said);
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        match t["stage"].as_str() {
            Some("drafting") => {
                let busy = gtk::Label::builder().label("Drafting a rule…").xalign(0.0).build();
                busy.add_css_class("dim");
                card.append(&busy);
                return card;
            }
            Some("draft") => {
                let says = if answered {
                    format!("Keep it as a rule for {topic}, so Scribe doesn't ask this again (change it as you like):")
                } else {
                    format!("A rule for {topic}, kept in its topic file (change it as you like):")
                };
                let says = gtk::Label::builder().label(says).xalign(0.0).wrap(true).build();
                says.add_css_class("small");
                card.append(&says);
                let rule = gtk::Entry::builder().text(t["draft"].as_str().unwrap_or("")).hexpand(true).build();
                let keep = gtk::Button::with_label("Keep it");
                keep.add_css_class("suggested-action");
                let (entry, chosen) = (rule.clone(), card.downgrade());
                let kept = move || {
                    let text = entry.text().trim().to_owned();
                    if !text.is_empty() {
                        if let Some(card) = chosen.upgrade() {
                            card.set_sensitive(false);
                        }
                        scribe("scribe_keep_rule", json!({ "id": format!("teach:{id}"), "rule": text }));
                    }
                };
                let kept = Rc::new(kept);
                let k = kept.clone();
                rule.connect_activate(move |_| k());
                keep.connect_clicked(move |_| kept());
                card.append(&rule);
                line.append(&keep);
            }
            _ => {
                let says = gtk::Label::builder()
                    .label(format!("Tell Scribe why, and it keeps it as a rule for {topic} (and as a test it must pass). Optional."))
                    .xalign(0.0)
                    .wrap(true)
                    .build();
                says.add_css_class("small");
                card.append(&says);
                let why = gtk::Entry::builder().placeholder_text("Why not? Enter").hexpand(true).build();
                let chosen = card.downgrade();
                why.connect_activate(move |entry| {
                    let text = entry.text().trim().to_owned();
                    if !text.is_empty() {
                        if let Some(card) = chosen.upgrade() {
                            card.set_sensitive(false);
                        }
                        scribe("scribe_teach", json!({ "id": format!("teach:{id}"), "said": text }));
                    }
                });
                line.append(&why);
            }
        }
        let skip = gtk::Button::with_label(if t["stage"].as_str() == Some("draft") { "Drop" } else { "Skip" });
        skip.add_css_class("flat");
        skip.connect_clicked(move |_| scribe("scribe_forget", json!({ "id": format!("teach:{id}") })));
        line.append(&skip);
        card.append(&line);
        card
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
            let (option, chosen) = (option.to_owned(), card.downgrade());
            button.connect_clicked(move |button| {
                button.add_css_class("chosen");
                if let Some(card) = chosen.upgrade() {
                    card.set_sensitive(false);
                }
                scribe("scribe_answer", json!({ "id": format!("question:{id}"), "answer": option }));
            });
            if let Some(label) = button.child().and_downcast::<gtk::Label>() {
                label.set_wrap(true);
                label.set_xalign(0.0);
            }
            options.append(&button);
        }
        // An answer of one's own (another currency, a date…).
        let own = gtk::Entry::builder().placeholder_text("Or your own answer, Enter").hexpand(true).build();
        let chosen = card.downgrade();
        own.connect_activate(move |entry| {
            let text = entry.text().trim().to_owned();
            if !text.is_empty() {
                if let Some(card) = chosen.upgrade() {
                    card.set_sensitive(false);
                }
                scribe("scribe_answer", json!({ "id": format!("question:{id}"), "answer": text }));
            }
        });
        let skip = gtk::Button::with_label("Skip");
        skip.add_css_class("flat");
        skip.connect_clicked(move |_| scribe("scribe_drop", json!({ "id": format!("question:{id}") })));
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
                let id = v["id"].as_u64().unwrap_or(0);
                undo.connect_clicked(move |button| {
                    button.set_sensitive(false);
                    button.set_label("Undoing…");
                    scribe("scribe_undo", json!({ "id": format!("change:{id}") }));
                });
                card.append(&undo);
                // A comment on it: Scribe changes the note as the user says.
                let comment = gtk::Entry::builder().placeholder_text("Ask Scribe to change it… Enter").hexpand(true).build();
                comment.set_tooltip_text(Some("What's wrong or missing in this change (\"it's net, not gross\"): Scribe changes the note that way at once"));
                let (row_data, held) = (v.clone(), card.downgrade());
                comment.connect_activate(move |entry| {
                    let text = entry.text().trim().to_owned();
                    if !text.is_empty() {
                        if let Some(card) = held.upgrade() {
                            card.set_sensitive(false);
                        }
                        scribe("scribe_fix", json!({ "id": format!("change:{}", row_data["id"]), "said": text }));
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

    /// The history page, filled again (as searched and filtered).
    pub fn fill_page(self: &Rc<Self>) {
        if self.filling.replace(true) {
            return;
        }
        self.fill_page_now();
        self.filling.set(false);
    }

    fn fill_page_now(self: &Rc<Self>) {
        let entries = self.core.listed();
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
            // Pressed: said at once (it may wait for a check that is running).
            let pressed = buttons.clone();
            approve.connect_clicked(move |button| {
                pressed.set_sensitive(false);
                button.set_label("Writing…");
                scribe("scribe_decide", json!({ "id": format!("proposal:{id}"), "write": true }));
            });
            let pressed = buttons.clone();
            reject.connect_clicked(move |button| {
                pressed.set_sensitive(false);
                button.set_label("Not writing…");
                scribe("scribe_decide", json!({ "id": format!("proposal:{id}"), "write": false }));
            });
            buttons.append(&approve);
            buttons.append(&reject);
            card.append(&buttons);
        } else {
            card.add_css_class("written");
        }
        card
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

impl Hub {
    fn me(&self) -> Option<Rc<Hub>> {
        self.me.borrow().upgrade()
    }

    // loomd's client: Scribe is loomd's, which hears of its agents itself; the core here
    // must not write Scribe's store or the vault beside it.
    pub fn counts(&self) -> (usize, usize) {
        let items = self.items();
        let count = |list: &str| items.iter().filter(|i| i["list"] == list).count();
        (count("question"), count("proposal"))
    }

    pub fn asks_about(&self, session: &str) -> bool {
        self.items().iter().any(|i| i["list"] == "question" && i["session"].as_str() == Some(session))
    }

    /// The `scribe` feed's items as last read.
    fn items(&self) -> Vec<Value> {
        self.feed.borrow().as_ref().map(|f| f.items().clone()).unwrap_or_default()
    }

    /// Scribe is checking a conversation now (here, or loomd's).
    fn running(&self) -> bool {
        self.items().iter().any(|i| i["id"] == "status" && i["running"] == true)
    }

    /// Changes seen (one, or all): folded from then on. True if that changed anything here
    /// (loomd's: its `scribe` says so when it did).
    fn mark_seen(&self, id: Option<u64>) -> bool {
        scribe("scribe_seen", json!({ "id": id.map(|id| format!("change:{id}")) }));
        false
    }

    /// loomd's Scribe as `scribe` says: what it does in the status line, and the Notes tab
    /// marked when it has more for the user than before (`news`: that count).
    fn remote_news(&self, items: &[Value], news: usize) {
        let status = items.iter().find(|i| i["id"] == "status");
        let doing = status.and_then(|s| s["doing"].as_str()).filter(|d| !d.is_empty());
        if let Some(doing) = doing {
            self.status.set_label(doing);
        }
        if self.news.replace(Some(news)).is_some_and(|before| news > before)
            && let Some(attention) = self.on_attention.borrow().as_ref()
        {
            attention();
        }
    }

    pub fn teach_newest(&self, said: &str) {
        self.core.teach_newest(said);
    }

    pub fn fix_newest(&self, comment: &str) {
        self.core.fix_newest(comment);
    }

    pub fn keep_newest_rule(&self) {
        self.core.keep_newest_rule();
    }
}

impl HubView for Hub {
    fn refresh(&self) {
        if let Some(hub) = self.me() {
            Hub::refresh(&hub);
        }
    }

    fn refresh_head(&self) {
        Hub::refresh_head(self);
    }

    fn status(&self, text: &str) {
        self.status.set_label(text);
    }

    fn say(&self, text: &str, error: bool) {
        if let Some(hub) = self.me() {
            Hub::say(&hub, text, error);
        }
    }

    fn attention(&self) {
        if let Some(attention) = self.on_attention.borrow().as_ref() {
            attention();
        }
    }

    fn history_changed(&self) {
        if let Some(hub) = self.me()
            && hub.page.is_mapped()
        {
            hub.fill_page();
        }
    }
}

/// Scribe told through the core API, as every client tells it (warp).
fn scribe(method: &str, params: Value) {
    crate::conn::command("scribe", method, params);
}
