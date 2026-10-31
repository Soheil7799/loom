//! A workflow's card (what is read from its files: `loom_core::workflow`).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::{glib, pango, prelude::*};
use serde_json::{Value, json};

pub use loom_core::workflow::*;

/// A workflow's card in the conversation, kept up to date while it runs.
pub struct Run {
    pub tool_id: String,
    pub name: String,
    pub task: String,
    pub root: gtk::Box,
    status: gtk::Label,
    stop: gtk::Button,
    progress_label: gtk::Label,
    finished: gtk::Label,
    agents: gtk::Expander,
    list: gtk::ListBox,
    /// Where it stands (read from its files, in loom-core).
    progress: RefCell<Progress>,
    /// What the strip was last told: (done, all, over).
    told: Cell<(usize, usize, bool)>,
    /// The members the list shows (it is filled again only when they change).
    shown: RefCell<Vec<Member>>,
    /// The agent of the conversation still runs (a workflow ends with it).
    alive: Box<dyn Fn() -> bool>,
    /// An agent of it opened (its transcript file and label).
    pub on_open: RefCell<Option<Rc<dyn Fn(PathBuf, String)>>>,
    /// Its progress changed (the strip under the conversation says it).
    pub on_changed: RefCell<Option<Rc<dyn Fn()>>>,
    /// Its Stop pressed.
    pub on_stop: RefCell<Option<Rc<dyn Fn()>>>,
}

impl Run {
    /// `live`: launched now (a conversation read back has only workflows that are over).
    pub fn new(tool_id: &str, launch: Launch, live: bool, alive: impl Fn() -> bool + 'static) -> Rc<Run> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("workflow-card");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::builder().label(format!("Workflow · {}", launch.name)).xalign(0.0).ellipsize(pango::EllipsizeMode::End).hexpand(true).build();
        title.add_css_class("workflow-title");
        let status = gtk::Label::new(Some("running"));
        status.add_css_class("workflow-status");
        let stop = gtk::Button::with_label("Stop");
        stop.add_css_class("flat");
        stop.add_css_class("workflow-stop");
        stop.set_valign(gtk::Align::Center);
        stop.set_tooltip_text(Some("Stop this workflow (its agents' answers so far stay)"));
        head.append(&title);
        head.append(&stop);
        head.append(&status);
        root.append(&head);
        if !launch.summary.is_empty() {
            let summary = gtk::Label::builder().label(&launch.summary).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build();
            summary.add_css_class("dim");
            summary.add_css_class("small");
            root.append(&summary);
        }
        let progress = gtk::Label::builder().xalign(0.0).wrap(true).build();
        progress.add_css_class("workflow-progress");
        root.append(&progress);
        let finished = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
        finished.add_css_class("small");
        root.append(&finished);
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        list.add_css_class("workflow-agents");
        let agents = gtk::Expander::builder().label("Agents").child(&list).build();
        agents.set_tooltip_text(Some("Every agent of the workflow: what it does and its answer; one opens its whole conversation"));
        root.append(&agents);
        let run = Rc::new(Run {
            tool_id: tool_id.to_owned(),
            name: launch.name.clone(),
            progress: RefCell::new(Progress::new(&launch, live)),
            task: launch.task,
            root,
            status,
            stop: stop.clone(),
            progress_label: progress,
            finished,
            agents: agents.clone(),
            list: list.clone(),
            told: Cell::new((0, 0, false)),
            shown: RefCell::default(),
            alive: Box::new(alive),
            on_open: RefCell::default(),
            on_changed: RefCell::default(),
            on_stop: RefCell::default(),
        });
        let weak = Rc::downgrade(&run);
        stop.connect_clicked(move |stop| {
            let Some(run) = weak.upgrade() else { return };
            let on_stop = run.on_stop.borrow().clone();
            if let Some(on_stop) = on_stop {
                stop.set_sensitive(false);
                stop.set_label("Stopping…");
                on_stop();
            }
        });
        // Opening the list fills it (a long one is only made when wanted).
        let weak = Rc::downgrade(&run);
        agents.connect_expanded_notify(move |_| {
            if let Some(run) = weak.upgrade() {
                run.fill_list();
            }
        });
        let weak = Rc::downgrade(&run);
        list.connect_row_activated(move |_, row| {
            let Some(run) = weak.upgrade() else { return };
            let member = run.shown.borrow().get(row.index() as usize).cloned();
            let open = run.on_open.borrow().clone();
            if let (Some(member), Some(open)) = (member, open) {
                open(run.progress.borrow().transcript(&member.id), member.label);
            }
        });
        run.poll();
        // While it runs: read what is new every second and a half.
        if !run.done() {
            let weak = Rc::downgrade(&run);
            glib::timeout_add_local(Duration::from_millis(1500), move || match weak.upgrade() {
                Some(run) if !run.done() => {
                    run.poll();
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            });
        }
        run
    }

    /// Over (done, failed, stopped).
    pub fn done(&self) -> bool {
        self.progress.borrow().done()
    }

    /// (done, all) agents, for the strip.
    pub fn counts(&self) -> (usize, usize) {
        self.progress.borrow().counts()
    }

    /// Reads what its files have new, and shows it.
    fn poll(&self) {
        let alive = (self.alive)();
        self.progress.borrow_mut().poll(alive);
        self.show();
    }

    /// Where it stands, drawn; the strip told of a change.
    fn show(&self) {
        // Its agents' answers can be long: read where they are, not copied on every poll.
        let (state, text) = {
            let progress = self.progress.borrow();
            (progress.state.clone(), phases(&progress.members))
        };
        for class in ["done", "stopped", "failed"] {
            self.status.remove_css_class(class);
        }
        let (word, class) = match &state {
            State::Running => ("running", None),
            State::Done(_) => ("done", Some("done")),
            State::Ended => ("ended", Some("done")),
            State::Failed => ("failed", Some("failed")),
            State::Stopped => ("stopped", Some("stopped")),
        };
        self.status.set_label(word);
        if let Some(class) = class {
            self.status.add_css_class(class);
        }
        if let State::Done(text) = &state {
            self.finished.set_label(text);
            self.finished.set_visible(true);
        }
        let over = state != State::Running;
        self.stop.set_visible(!over);
        let (done, all) = self.counts();
        self.progress_label.set_label(if all == 0 { "Starting…" } else { &text });
        self.agents.set_label(Some(&format!("Agents ({done} of {all} done)")));
        if self.agents.is_expanded() {
            self.fill_list();
        }
        // The strip is told only of a change.
        if self.told.replace((done, all, over)) != (done, all, over)
            && let Some(changed) = self.on_changed.borrow().clone()
        {
            changed();
        }
    }

    /// Its Stop was asked for and nothing stopped it (the request was taken back, or Claude
    /// did not stop it): Stop can be pressed again.
    pub fn stop_unanswered(&self) {
        if !self.done() {
            self.stop.set_sensitive(true);
            self.stop.set_label("Stop");
        }
    }

    /// Its end, as the provider tells it: done (its output in `output`), failed, or stopped.
    pub fn ended(&self, status: &str, output: Option<PathBuf>) {
        let alive = (self.alive)();
        self.progress.borrow_mut().ended(status, output, alive);
        self.show();
    }

    /// Stopped from loom: over, whatever its files say later.
    pub fn stopped(&self) {
        self.progress.borrow_mut().state = State::Stopped;
        self.show();
    }

    fn fill_list(&self) {
        if *self.shown.borrow() == self.progress.borrow().members {
            return;
        }
        let members = self.progress.borrow().members.clone();
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        for m in &members {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let glyph = gtk::Label::new(Some(if m.result.is_some() { "✓" } else { "◌" }));
            glyph.add_css_class(if m.result.is_some() { "done" } else { "running" });
            let label = gtk::Label::builder().label(&m.label).xalign(0.0).build();
            label.add_css_class("workflow-agent");
            let preview = m.result.as_deref().map(|r| r.trim().lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim_start_matches(['#', '*', ' ']).to_owned()).unwrap_or_else(|| "working…".to_owned());
            let said = gtk::Label::builder().label(preview).xalign(0.0).ellipsize(pango::EllipsizeMode::End).hexpand(true).build();
            said.add_css_class("dim");
            said.add_css_class("small");
            line.append(&glyph);
            line.append(&label);
            line.append(&said);
            let row = gtk::ListBoxRow::builder().child(&line).build();
            row.set_tooltip_text(Some("Open its conversation"));
            self.list.append(&row);
        }
        *self.shown.borrow_mut() = members;
    }
}

/// A workflow agent's conversation (its transcript file) into a read-only chat: what it was
/// asked, what it said, and its tool calls.
pub fn fill_transcript(chat: &crate::chat::Chat, path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        chat.notice("Its conversation is not there (Claude Code keeps it only while the conversation's files are kept).", "dim");
        return;
    };
    let mut results: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    for v in &lines {
        for block in v["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] == "tool_result"
                && let Some(id) = block["tool_use_id"].as_str()
            {
                let text = match &block["content"] {
                    Value::String(s) => s.clone(),
                    Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
                    _ => String::new(),
                };
                results.insert(id.to_owned(), text);
            }
        }
    }
    for v in &lines {
        let content = &v["message"]["content"];
        match v["type"].as_str() {
            Some("user") => {
                let said = match content {
                    Value::String(s) => s.clone(),
                    Value::Array(parts) => parts.iter().filter(|p| p["type"] == "text").filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n\n"),
                    _ => String::new(),
                };
                if !said.trim().is_empty() {
                    chat.user(said.trim());
                }
            }
            Some("assistant") => {
                for block in content.as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => chat.agent_text_with(block["text"].as_str().unwrap_or(""), None),
                        Some("tool_use") => {
                            let (id, name) = (block["id"].as_str().unwrap_or(""), block["name"].as_str().unwrap_or("Tool"));
                            let input = &block["input"];
                            let what = ["command", "file_path", "pattern", "url", "query", "description"].iter().find_map(|k| input[*k].as_str());
                            let title = what.map_or_else(|| name.to_owned(), |w| format!("{name} {w}"));
                            let kind = match name {
                                "Bash" => "execute",
                                "Read" => "read",
                                "Edit" | "Write" | "MultiEdit" => "edit",
                                "Grep" | "Glob" => "search",
                                "WebFetch" | "WebSearch" => "fetch",
                                _ => "other",
                            };
                            let output = results.get(id).cloned().unwrap_or_default();
                            chat.tool(&json!({
                                "toolCallId": format!("wf-{id}"),
                                "title": title,
                                "kind": kind,
                                "status": "completed",
                                "rawInput": input,
                                "content": [{ "type": "content", "content": { "type": "text", "text": output } }],
                            }));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}
