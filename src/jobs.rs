//! The job tray: one chip per command along the bottom of a project. A chip peeks its
//! terminal in a popover; pinning docks the terminal above the tray.
//!
//! The commands themselves run in loom-core (`loom_core::jobs`); here they are drawn from
//! the core API (warp): the chips from `jobs`, each terminal from its `job/<id>` (the screen,
//! then the bytes the job prints), sending back the keys typed into it and its size. What is
//! done to them goes as commands (`run_job`, `job_input`…), to loomd too when the window is
//! its client.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use gtk::{glib, prelude::*};
use loom_core::api::{Body, Op, wire};
use serde_json::json;
use loom_core::jobs::{Id, Jobs};
use serde_json::Value;
use vte::prelude::*;

use crate::term;

pub struct Job {
    id: Id,
    core: Jobs,
    title: gtk::Label,
    term: vte::Terminal,
    chip: gtk::Button,
    dot: gtk::Box,
    popover: gtk::Popover,
    holder: gtk::Box,
    status: gtk::Label,
    /// The rows and columns last told to the job.
    size: Cell<(i64, i64)>,
    /// Counts the times it was shown: the size watch of an earlier showing stops.
    size_watch: Cell<u64>,
    docked: RefCell<Option<gtk::Box>>,
    tray: Weak<Tray>,
    /// Its `job/<id>` in the core, once followed, and the byte offset its terminal is at.
    output: RefCell<Option<crate::conn::Sub>>,
    offset: Cell<u64>,
    /// Its status in `jobs` as last drawn ("" before it was seen there).
    state: RefCell<String>,
}

pub struct Tray {
    /// The chips (and a hint) along the bottom.
    pub bar: gtk::Box,
    /// Pinned terminals, side by side (whoever holds it shows it only while `pinned`).
    pub dock: gtk::Box,
    chips: gtk::Box,
    hint: gtk::Label,
    jobs: RefCell<Vec<Rc<Job>>>,
    core: Jobs,
    /// Told when a job ends: its name and exit code.
    pub on_finish: RefCell<Option<Rc<dyn Fn(&str, Option<i64>)>>>,
    /// Told when a job is pinned or unpinned.
    pub on_dock: RefCell<Option<Rc<dyn Fn()>>>,
    /// The core's `jobs`, followed: what the chips draw.
    feed: RefCell<Option<crate::feed::Feed>>,
    /// Jobs removed while running: stopped, out of sight, taken out once they exit.
    removing: RefCell<Vec<Id>>,
    /// A job to peek once its chip comes (asked for before `jobs` had it).
    peek_due: Cell<Option<Id>>,
    /// The project's browser (a chip while it runs; its page docked when opened), and loomd's
    /// `browsers`, followed.
    browser: Rc<crate::browser::Browser>,
    browsers: RefCell<Option<crate::feed::Feed>>,
}

impl Tray {
    /// The folder its jobs run in.
    pub fn cwd(&self) -> &std::path::Path {
        self.core.cwd()
    }

    pub fn new(cwd: PathBuf) -> Rc<Tray> {
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("tray");
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&chips)
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .build();
        let hint = gtk::Label::new(Some("Ctrl+J run · Ctrl+Shift+J peek · Ctrl+` scratch"));
        hint.add_css_class("hint");
        let browser = crate::browser::Browser::new(cwd.clone());
        bar.append(&scroller);
        bar.append(&browser.chip);
        bar.append(&hint);
        let dock = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        dock.set_homogeneous(true);
        dock.add_css_class("dock");
        let (core, events) = Jobs::new(cwd);
        // The core API publishes its jobs; the tray draws from what it publishes.
        drop(loom_core::jobs_api::published(&core, events));
        let tray = Rc::new(Tray { bar, dock, chips, hint, jobs: RefCell::default(), core, on_finish: RefCell::default(), on_dock: RefCell::default(), feed: RefCell::default(), removing: RefCell::default(), peek_due: Cell::new(None), browser, browsers: RefCell::default() });
        let weak = Rc::downgrade(&tray);
        *tray.browser.on_dock.borrow_mut() = Some(Rc::new(move |open: bool| {
            let Some(tray) = weak.upgrade() else { return };
            let frame = &tray.browser.frame;
            match (open, frame.parent().is_some()) {
                (true, false) => tray.dock.append(frame),
                (false, true) => tray.dock.remove(frame),
                _ => return,
            }
            tray.docked_changed();
        }));
        let weak = Rc::downgrade(&tray);
        *tray.browsers.borrow_mut() = crate::feed::Feed::new("browsers", move || {
            if let Some(tray) = weak.upgrade() {
                tray.sync_browser();
            }
        });
        tray.sync_browser();
        let weak = Rc::downgrade(&tray);
        *tray.feed.borrow_mut() = crate::feed::Feed::new("jobs", move || {
            if let Some(tray) = weak.upgrade() {
                tray.sync();
            }
        });
        tray.sync();
        tray
    }

    /// The chips drawn as the core's `jobs` has this project's jobs now.
    fn sync(self: &Rc<Self>) {
        let items: Vec<Value> = {
            let feed = self.feed.borrow();
            let Some(feed) = feed.as_ref() else { return };
            feed.update();
            feed.items().clone()
        };
        let cwd = self.core.cwd().to_string_lossy().into_owned();
        let mine: Vec<&Value> = items.iter().filter(|item| item["project_id"].as_str() == Some(cwd.as_str())).collect();
        for item in &mine {
            let Some(id) = item["id"].as_str().and_then(|id| id.parse::<Id>().ok()) else { continue };
            if self.removing.borrow().contains(&id) {
                if item["status"] == "exited" {
                    self.removing.borrow_mut().retain(|r| *r != id);
                    crate::conn::command("jobs", "remove_job", json!({ "job": id.to_string() }));
                }
                continue;
            }
            let job = match self.by_id(id) {
                Some(job) => job,
                None => self.add(id, item["name"].as_str().unwrap_or_default(), item["command"].as_str().unwrap_or_default()),
            };
            job.show(item);
            if self.peek_due.get() == Some(id) {
                self.peek_due.set(None);
                self.peek(&job);
            }
        }
        // Removed elsewhere (seen there once, gone now): its chip goes too.
        let gone: Vec<Rc<Job>> = self.jobs.borrow().iter().filter(|job| !job.state.borrow().is_empty() && !mine.iter().any(|item| item["id"].as_str() == Some(job.id.to_string().as_str()))).cloned().collect();
        for job in gone {
            self.drop_chip(&job);
        }
    }

    /// The browser's chip and view as loomd's `browsers` has this project's now.
    fn sync_browser(&self) {
        let item = {
            let feed = self.browsers.borrow();
            let Some(feed) = feed.as_ref() else { return };
            feed.update();
            // Not before its snapshot: the first one seen sets what was asked before.
            if !feed.synced() {
                return;
            }
            let cwd = self.core.cwd().to_string_lossy();
            feed.items().iter().find(|i| i["id"].as_str() == Some(&cwd)).cloned()
        };
        self.browser.show(item.as_ref());
    }

    /// Opens the project's browser (started by loomd if it doesn't run) above the tray.
    pub fn open_browser(&self) {
        let browser = Rc::downgrade(&self.browser);
        crate::conn::command_then("browser", "browser_open", json!({ "project": self.core.cwd() }), move |answer| {
            if let (Some(browser), Some(answer)) = (browser.upgrade(), answer) {
                browser.show(Some(&answer));
                browser.dock(true);
            }
        });
    }

    fn by_id(&self, id: Id) -> Option<Rc<Job>> {
        self.jobs.borrow().iter().find(|j| j.id == id).cloned()
    }

    /// Runs `line` in the project ("name: command" names the chip).
    pub fn run(self: &Rc<Self>, line: &str) {
        let (name, command) = match line.split_once(": ") {
            Some((name, command)) if !name.contains(' ') => (name.to_owned(), command.to_owned()),
            _ => (short_name(line), line.to_owned()),
        };
        let weak = Rc::downgrade(self);
        let params = json!({ "project": self.cwd(), "name": name, "command": command });
        crate::conn::command_then("jobs", "run_job", params, move |answer| {
            let Some(tray) = weak.upgrade() else { return };
            let Some(id) = answer.and_then(|a| a["job"].as_str().and_then(|id| id.parse::<Id>().ok())) else { return };
            let job = tray.by_id(id).unwrap_or_else(|| tray.add(id, &name, &command));
            tray.peek(&job);
        });
    }

    /// How many jobs are running.
    pub fn running(&self) -> usize {
        self.jobs.borrow().iter().filter(|j| j.is_running()).count()
    }

    fn add(self: &Rc<Self>, id: Id, name: &str, command: &str) -> Rc<Job> {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("dot");
        dot.add_css_class("working");
        dot.set_valign(gtk::Align::Center);
        let label = gtk::Label::new(Some(&ellipsize(name, 28)));
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&dot);
        content.append(&label);
        let chip = gtk::Button::builder().child(&content).tooltip_text(command).build();
        chip.add_css_class("chip");
        chip.add_css_class("flat");

        let term = term::new(&chip);
        term.set_size_request(760, 320);
        // It only draws: what is typed in it goes to the job.
        term.connect_commit(move |_, text, _| crate::conn::command("jobs", "job_input", json!({ "job": id.to_string(), "text": text })));
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let status = gtk::Label::new(Some("running"));
        status.add_css_class("dim");
        let popover = gtk::Popover::new();
        popover.add_css_class("peek");
        popover.set_parent(&chip);

        let title = gtk::Label::new(Some(name));
        let job = Rc::new(Job {
            id,
            core: self.core.clone(),
            title,
            term,
            chip: chip.clone(),
            dot,
            popover: popover.clone(),
            holder: holder.clone(),
            status: status.clone(),
            size: Cell::new((0, 0)),
            size_watch: Cell::new(0),
            docked: RefCell::default(),
            tray: Rc::downgrade(self),
            output: RefCell::default(),
            offset: Cell::new(0),
            state: RefCell::default(),
        });
        // And its size: VTE has no signal for it, so it is looked at twice a second while it is
        // shown (not on every frame, which would keep the window drawing all the time).
        let weak = Rc::downgrade(&job);
        job.term.connect_map(move |_| {
            let Some(job) = weak.upgrade() else { return };
            let watch = job.size_watch.get() + 1;
            job.size_watch.set(watch);
            job.send_size();
            let weak = Rc::downgrade(&job);
            glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                let Some(job) = weak.upgrade() else { return glib::ControlFlow::Break };
                // Hidden, or shown again since (a newer watch looks).
                if !job.term.is_mapped() || job.size_watch.get() != watch {
                    return glib::ControlFlow::Break;
                }
                job.send_size();
                glib::ControlFlow::Continue
            });
        });
        popover.set_child(Some(&job.frame(false)));
        holder.append(&job.term);

        let weak = Rc::downgrade(&job);
        chip.connect_clicked(move |_| {
            if let Some(job) = weak.upgrade()
                && let Some(tray) = job.tray.upgrade()
            {
                tray.peek(&job);
            }
        });
        // Right-click: what the peek's buttons do, and the command and output to copy.
        let weak = Rc::downgrade(&job);
        crate::menu::on_right_click(&chip, move |x, y| {
            let Some(job) = weak.upgrade() else { return };
            let act = |f: fn(&Tray, &Rc<Job>)| {
                let weak = Rc::downgrade(&job);
                move || {
                    if let Some(job) = weak.upgrade()
                        && let Some(tray) = job.tray.upgrade()
                    {
                        f(&tray, &job);
                    }
                }
            };
            let command = job.chip.tooltip_text().map(|c| c.to_string()).unwrap_or_default();
            let docked = job.docked.borrow().is_some();
            let menu = crate::menu::Menu::new()
                .item("Show its output", act(|tray, job| tray.peek(job)))
                .item("Run again", act(|_, job| job.restart()))
                .item("Stop", act(|_, job| job.stop()))
                .enabled(job.is_running())
                .item(
                    if docked { "Unpin" } else { "Pin above the tray" },
                    act(|tray, job| if job.docked.borrow().is_some() { tray.unpin(job) } else { tray.pin(job) }),
                );
            let (chip, chip2) = (job.chip.clone(), job.chip.clone());
            let output = Rc::downgrade(&job);
            menu.section()
                .item("Copy the command", move || crate::menu::copy(&chip, &command))
                .item("Copy its output", move || {
                    if let Some(job) = output.upgrade() {
                        crate::menu::copy(&chip2, &job.output(10_000));
                    }
                })
                .section()
                .item("Remove from the tray", act(|tray, job| tray.remove(job)))
                .danger()
                .popup(&job.chip, x, y);
        });
        self.chips.append(&chip);
        flash(&chip, "fresh", 300);
        self.hint.set_visible(false);
        self.jobs.borrow_mut().push(job.clone());
        // Its output, if the core has told of it yet (else once `jobs` lists it).
        job.follow();
        job
    }

    pub fn peek(&self, job: &Rc<Job>) {
        if job.docked.borrow().is_some() {
            job.term.grab_focus();
            return;
        }
        job.popover.popup();
        job.term.grab_focus();
    }

    /// Peeks job `id` (an agent's that waits for the user to type).
    pub fn peek_id(&self, id: Id) {
        let job = self.jobs.borrow().iter().find(|j| j.id == id).cloned();
        match job {
            Some(job) => self.peek(&job),
            None => self.peek_due.set(Some(id)),
        }
    }

    /// Peeks the newest job.
    pub fn peek_latest(&self) {
        let latest = self.jobs.borrow().last().cloned();
        if let Some(job) = latest {
            self.peek(&job);
        }
    }

    /// Pins the newest job above the tray, or back (the test hooks).
    /// Keys typed into the newest job's terminal (tests: as the terminal hands them on).
    pub fn keys_to_latest(&self, text: &str) {
        if let Some(job) = self.jobs.borrow().last() {
            job.term.emit_by_name::<()>("commit", &[&text, &(text.len() as u32)]);
        }
    }

    pub fn pin_latest(&self, pin: bool) {
        let latest = self.jobs.borrow().last().cloned();
        match latest {
            Some(job) if pin => self.pin(&job),
            Some(job) => self.unpin(&job),
            None => {}
        }
    }

    fn pin(&self, job: &Rc<Job>) {
        if job.docked.borrow().is_some() {
            return;
        }
        job.popover.popdown();
        job.holder.remove(&job.term);
        let frame = job.frame(true);
        frame.append(&job.term);
        self.dock.append(&frame);
        *job.docked.borrow_mut() = Some(frame);
        self.docked_changed();
    }

    fn unpin(&self, job: &Rc<Job>) {
        let Some(frame) = job.docked.borrow_mut().take() else { return };
        frame.remove(&job.term);
        self.dock.remove(&frame);
        job.holder.append(&job.term);
        self.docked_changed();
    }

    /// Whether anything is pinned (the dock worth showing).
    pub fn pinned(&self) -> bool {
        self.dock.first_child().is_some()
    }

    fn docked_changed(&self) {
        let told = self.on_dock.borrow().clone();
        if let Some(told) = told {
            told();
        }
    }

    fn remove(&self, job: &Rc<Job>) {
        if job.is_running() {
            self.removing.borrow_mut().push(job.id);
            job.stop();
        } else {
            job.tell("remove_job", json!({}));
        }
        self.drop_chip(job);
    }

    /// Its chip and terminal gone from the tray.
    fn drop_chip(&self, job: &Rc<Job>) {
        if !self.jobs.borrow().iter().any(|other| Rc::ptr_eq(other, job)) {
            return;
        }
        self.unpin(job);
        job.popover.popdown();
        job.output.borrow_mut().take();
        job.popover.unparent();
        self.chips.remove(&job.chip);
        self.jobs.borrow_mut().retain(|other| !Rc::ptr_eq(other, job));
        self.hint.set_visible(self.jobs.borrow().is_empty());
    }

    pub fn clear_finished(&self) {
        let done: Vec<Rc<Job>> =
            self.jobs.borrow().iter().filter(|job| !job.is_running() && job.docked.borrow().is_none()).cloned().collect();
        for job in done {
            self.remove(&job);
        }
    }

}

impl Job {
    /// Its item in `jobs` drawn: its status (and the tray told when it ends), its command.
    fn show(self: &Rc<Self>, item: &Value) {
        if self.output.borrow().is_none() {
            self.follow();
        }
        if let Some(command) = item["command"].as_str() {
            self.chip.set_tooltip_text(Some(command));
        }
        let status = item["status"].as_str().unwrap_or("running").to_owned();
        // A run's own: a rerun that started and ended between two looks is still a new one.
        let state = format!("{status}:{}", item["runs"].as_u64().unwrap_or(0));
        let before = self.state.replace(state.clone());
        if before == state {
            return;
        }
        match status.as_str() {
            "exited" => {
                let code = item["exit_code"].as_i64();
                self.finish(code);
                // Told of an end seen happen, or of one just over when it came (a quick job's);
                // not of one long over (a tray opened later). An agent's job is not: the agent
                // reads its end itself, and its own turn's end is the word worth having.
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
                let just = loom_core::api::seconds(&item["ended"]).is_some_and(|at| now - at < 10);
                if (!before.is_empty() || just)
                    && item["agent"].is_null()
                    && let Some(told) = self.tray.upgrade().and_then(|tray| tray.on_finish.borrow().clone())
                {
                    told(&self.title.label(), code);
                }
            }
            // Its old output goes now, so nobody reads it as the new run's.
            "restarting" => {
                self.term.reset(true, true);
                self.status.set_label("restarting");
            }
            _ => {
                for class in ["done", "failed"] {
                    self.dot.remove_css_class(class);
                }
                self.dot.add_css_class("working");
                self.status.set_label("running");
            }
        }
    }

    /// Its terminal follows `job/<id>` from now on.
    fn follow(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let Some(sub) = crate::conn::subscribe(&format!("job/{}", self.id), move || {
            if let Some(job) = weak.upgrade() {
                job.draw();
            }
        }) else {
            return;
        };
        *self.output.borrow_mut() = Some(sub);
        self.draw();
    }

    /// What came for its terminal, fed to it: a screen (it starts over), or bytes (what it
    /// lacks of them; bytes past where it is mean some were missed: a fresh screen is asked for).
    fn draw(&self) {
        let messages = match self.output.borrow().as_ref() {
            Some(sub) => sub.drain(),
            None => return,
        };
        let mut items = Vec::new();
        let mut missed = false;
        for message in messages {
            match message.body {
                Body::Page { items: page } => items.extend(page.into_iter().map(Op::from_item)),
                Body::Changes { ops } => items.extend(ops),
                _ => {}
            }
        }
        for op in items {
            match op {
                Op::Upsert { item } if item["id"] == "output" => {
                    self.term.reset(true, true);
                    self.term.feed(&wire::bytes_of(item["screen"].as_str().unwrap_or_default()));
                    self.offset.set(item["offset"].as_u64().unwrap_or(0));
                    missed = false;
                }
                Op::Bytes { offset, data, .. } if !missed => {
                    // What its screen already holds is dropped; past where it is, some were missed.
                    let Some(bytes) = wire::unseen(self.offset.get(), offset, &data) else {
                        missed = true;
                        continue;
                    };
                    self.term.feed(&bytes);
                    self.offset.set(self.offset.get() + bytes.len() as u64);
                }
                _ => {}
            }
        }
        if missed && let Some(sub) = self.output.borrow().as_ref() {
            sub.resync();
        }
    }

    /// Name, status and buttons above the terminal; `docked` picks Unpin over Pin.
    fn frame(self: &Rc<Self>, docked: bool) -> gtk::Box {
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 4);
        frame.add_css_class(if docked { "docked" } else { "peek-frame" });
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let name = gtk::Label::new(None);
        self.title.bind_property("label", &name, "label").sync_create().build();
        name.add_css_class("mono");
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        name.set_xalign(0.0);
        name.set_hexpand(true);
        header.append(&name);
        if docked {
            let status = gtk::Label::new(None);
            status.add_css_class("dim");
            self.status.bind_property("label", &status, "label").sync_create().build();
            header.append(&status);
        } else {
            header.append(&self.status);
        }
        let button = |label: &str, tip: &str, act: fn(&Tray, &Rc<Job>)| {
            let b = gtk::Button::with_label(label);
            b.add_css_class("flat");
            b.add_css_class("small");
            b.set_tooltip_text(Some(tip));
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(job) = weak.upgrade()
                    && let Some(tray) = job.tray.upgrade()
                {
                    act(&tray, &job);
                }
            });
            b
        };
        header.append(&button("Rerun", "Run the command again", |_, job| job.restart()));
        header.append(&button("Stop", "Stop the command", |_, job| job.stop()));
        if docked {
            header.append(&button("Unpin", "Back to the tray", |tray, job| tray.unpin(job)));
        } else {
            header.append(&button("Pin", "Dock above the tray", |tray, job| tray.pin(job)));
        }
        header.append(&button("Remove", "Remove from the tray", |tray, job| tray.remove(job)));
        frame.append(&header);
        if !docked {
            frame.append(&self.holder);
        }
        frame
    }

    /// Its terminal's size, told to the job when it changed.
    fn send_size(&self) {
        let size = (self.term.row_count(), self.term.column_count());
        if size != self.size.replace(size) && size.0 > 0 && size.1 > 0 {
            self.tell("job_resize", json!({ "rows": size.0, "cols": size.1 }));
        }
    }

    /// The last `lines` lines it printed, as text.
    pub fn output(&self, lines: usize) -> String {
        self.core.output(self.id, lines)
    }

    /// Runs the command again: at once if nothing runs, else once the current run has
    /// exited (it is stopped).
    pub fn restart(&self) {
        self.tell("restart_job", json!({}));
    }

    /// Running, as `jobs` said last (one not seen there yet was just started).
    pub fn is_running(&self) -> bool {
        !self.state.borrow().starts_with("exited:")
    }

    /// Stops it (and any restart it was waiting for).
    pub fn stop(&self) {
        self.tell("stop_job", json!({}));
    }

    /// Command `method` done to it.
    fn tell(&self, method: &str, mut params: Value) {
        params["job"] = Value::String(self.id.to_string());
        crate::conn::command("jobs", method, params);
    }

    fn finish(&self, code: Option<i64>) {
        // (ended just now, not one listed ended already: its chip flashes its outcome once)
        let was_running = self.dot.has_css_class("working");
        for class in ["working", "done", "failed"] {
            self.dot.remove_css_class(class);
        }
        let (class, text) = match code {
            Some(0) => ("done", "exit 0".to_owned()),
            Some(code) => ("failed", format!("exit {code}")),
            None => ("done", "ended".to_owned()),
        };
        self.dot.add_css_class(class);
        self.status.set_label(&text);
        if was_running {
            flash(&self.chip, if class == "failed" { "just-failed" } else { "just-done" }, 700);
        }
    }
}

/// `class` on `widget` for `ms` (a one-shot CSS animation; taken off so it can play again).
fn flash(widget: &impl IsA<gtk::Widget>, class: &'static str, ms: u64) {
    widget.add_css_class(class);
    let widget = widget.clone().upcast::<gtk::Widget>();
    glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || widget.remove_css_class(class));
}

/// A chip name for a bare command: the program and its first argument.
fn short_name(line: &str) -> String {
    line.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(max - 1).collect::<String>())
    }
}
