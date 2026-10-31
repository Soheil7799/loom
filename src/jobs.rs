//! The job tray: one chip per command along the bottom of a project. A chip peeks its
//! terminal in a popover; pinning docks the terminal above the tray.
//!
//! The commands themselves run in loom-core (`loom_core::jobs`); here they are drawn: each
//! terminal is fed what its job prints, and sends back the keys typed into it and its size.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use gtk::{glib, prelude::*};
use loom_core::jobs::{Event, Id, Jobs};
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
    docked: RefCell<Option<gtk::Box>>,
    tray: Weak<Tray>,
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
        bar.append(&scroller);
        bar.append(&hint);
        let dock = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        dock.set_homogeneous(true);
        dock.add_css_class("dock");
        let (core, events) = Jobs::new(cwd);
        let tray = Rc::new(Tray { bar, dock, chips, hint, jobs: RefCell::default(), core, on_finish: RefCell::default(), on_dock: RefCell::default() });
        let weak = Rc::downgrade(&tray);
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                let Some(tray) = weak.upgrade() else { return };
                tray.show(event);
            }
        });
        tray
    }

    /// What a job did, drawn.
    fn show(&self, event: Event) {
        let id = match &event {
            Event::Started(id) | Event::Restarting(id) | Event::Output(id, _) | Event::Exited(id, _) => *id,
        };
        let Some(job) = self.by_id(id) else { return };
        match event {
            Event::Started(_) => {
                job.term.reset(true, true);
                job.dot.remove_css_class("done");
                job.dot.remove_css_class("failed");
                job.dot.add_css_class("working");
                job.status.set_label("running");
            }
            // Its old output goes now, so nobody reads it as the new run's.
            Event::Restarting(_) => {
                job.term.reset(true, true);
                job.status.set_label("restarting");
            }
            Event::Output(_, bytes) => job.term.feed(&bytes),
            Event::Exited(_, code) => {
                job.finish(code);
                let told = self.on_finish.borrow().clone();
                if let Some(told) = told {
                    told(&job.title.label(), code);
                }
            }
        }
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
        let id = self.core.run(&name, &command, None);
        let job = self.add(id, &name, &command);
        self.peek(&job);
    }

    /// The job named `name`.
    pub fn named(&self, name: &str) -> Option<Rc<Job>> {
        self.core.named(name).and_then(|info| self.by_id(info.id))
    }

    /// Runs `command` as the job `name`: the same chip again if there is one (never two
    /// jobs of one name, like herdr's named panes). Err if it is still running and
    /// `restart` is false.
    pub fn run_named(self: &Rc<Self>, name: &str, command: &str, restart: bool, cwd: Option<PathBuf>) -> Result<Rc<Job>, String> {
        let id = self.core.run_named(name, command, restart, cwd)?;
        if let Some(job) = self.by_id(id) {
            job.chip.set_tooltip_text(Some(command));
            return Ok(job);
        }
        Ok(self.add(id, name, command))
    }

    /// How many jobs are running.
    pub fn running(&self) -> usize {
        self.core.list().iter().filter(|j| j.running).count()
    }

    /// Every job: (name, running, exit code).
    pub fn list(&self) -> Vec<(String, bool, Option<i64>)> {
        self.core.list().into_iter().map(|j| (j.name, j.running, j.exit_code)).collect()
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
        let core = self.core.clone();
        term.connect_commit(move |_, text, _| core.write(id, text.as_bytes()));
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
            docked: RefCell::default(),
            tray: Rc::downgrade(self),
        });
        // And its size: VTE has no signal for it, so it is checked on each frame drawn.
        let weak = Rc::downgrade(&job);
        job.term.add_tick_callback(move |term, _| {
            let Some(job) = weak.upgrade() else { return glib::ControlFlow::Break };
            let size = (term.row_count(), term.column_count());
            if size != job.size.replace(size) {
                job.core.resize(job.id, u16::try_from(size.0).unwrap_or(0), u16::try_from(size.1).unwrap_or(0));
            }
            glib::ControlFlow::Continue
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
            let command = job.core.info(job.id).map(|info| info.command).unwrap_or_default();
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
        self.hint.set_visible(false);
        self.jobs.borrow_mut().push(job.clone());
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

    /// Peeks the newest job.
    pub fn peek_latest(&self) {
        let latest = self.jobs.borrow().last().cloned();
        if let Some(job) = latest {
            self.peek(&job);
        }
    }

    /// Pins the newest job above the tray, or back (the test hooks).
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
        self.unpin(job);
        job.popover.popdown();
        self.core.remove(job.id);
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

    /// Stops the jobs this tray started (on quit).
    pub fn stop_all(&self) {
        self.core.stop_all();
    }
}

impl Job {
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

    /// The last `lines` lines it printed, as text.
    pub fn output(&self, lines: usize) -> String {
        self.core.output(self.id, lines)
    }

    /// Exit code once done (None while running or when unknown).
    pub fn exit_code(&self) -> Option<i64> {
        self.core.info(self.id).and_then(|info| info.exit_code)
    }

    /// Runs the command again: at once if nothing runs, else once the current run has
    /// exited (it is stopped).
    pub fn restart(&self) {
        self.core.restart(self.id);
    }

    pub fn is_running(&self) -> bool {
        self.core.info(self.id).is_some_and(|info| info.running)
    }

    /// Stops it (and any restart it was waiting for).
    pub fn stop(&self) {
        self.core.stop(self.id);
    }

    fn finish(&self, code: Option<i64>) {
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
    }
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
