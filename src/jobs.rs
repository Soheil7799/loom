//! The job tray: one chip per command along the bottom of a project. A chip peeks its
//! terminal in a popover; pinning docks the terminal above the tray.
//!
//! Two kinds of job: the commands an agent runs (their output arrives over ACP and is fed to
//! a read-only terminal), and commands run from here (a real process in the terminal).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use gtk::{gio, glib, prelude::*};
use vte::prelude::*;

use crate::term;

/// Finished agent jobs beyond this many go, oldest first (pinned ones stay).
const KEEP: usize = 12;

pub struct Job {
    pub name: RefCell<String>,
    title: gtk::Label,
    command: RefCell<Option<String>>,
    term: vte::Terminal,
    chip: gtk::Button,
    dot: gtk::Box,
    popover: gtk::Popover,
    holder: gtk::Box,
    status: gtk::Label,
    /// None while running; Some(exit code, if known) once done.
    outcome: Cell<Option<Option<i64>>>,
    pid: Cell<Option<glib::Pid>>,
    /// Asked to run again while still running: the exit starts it (one restart, however
    /// many times it was asked).
    restart_pending: Cell<bool>,
    /// SIGKILL, if SIGTERM was not enough.
    kill_timer: RefCell<Option<glib::SourceId>>,
    /// Where it runs, if not the project folder.
    cwd: RefCell<Option<PathBuf>>,
    /// Between asking VTE to spawn and hearing back.
    spawning: Cell<bool>,
    docked: RefCell<Option<gtk::Box>>,
    tray: Weak<Tray>,
}

pub struct Tray {
    /// The chips (and a hint) along the bottom.
    pub bar: gtk::Box,
    /// Pinned terminals, side by side; hidden when nothing is pinned.
    pub dock: gtk::Box,
    chips: gtk::Box,
    hint: gtk::Label,
    jobs: RefCell<Vec<Rc<Job>>>,
    cwd: PathBuf,
    /// Told when a job ends: its name and exit code.
    pub on_finish: RefCell<Option<Rc<dyn Fn(&str, Option<i64>)>>>,
}

impl Tray {
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
        dock.set_visible(false);
        Rc::new(Tray { bar, dock, chips, hint, jobs: RefCell::default(), cwd, on_finish: RefCell::default() })
    }

    /// The job showing an agent's command (made on first sight of `terminal_id`).
    /// Runs `line` in the project ("name: command" names the chip).
    pub fn run(self: &Rc<Self>, line: &str) {
        let (name, command) = match line.split_once(": ") {
            Some((name, command)) if !name.contains(' ') => (name.to_owned(), command.to_owned()),
            _ => (short_name(line), line.to_owned()),
        };
        let job = self.add(&name, Some(command.clone()), &command);
        job.start();
        self.peek(&job);
    }

    /// The job named `name` that runs a command of its own (not an agent's).
    pub fn named(&self, name: &str) -> Option<Rc<Job>> {
        self.jobs.borrow().iter().rev().find(|j| j.command.borrow().is_some() && *j.name.borrow() == name).cloned()
    }

    /// Runs `command` as the job `name`: the same chip again if there is one (never two
    /// jobs of one name, like herdr's named panes). Err if it is still running and
    /// `restart` is false.
    pub fn run_named(self: &Rc<Self>, name: &str, command: &str, restart: bool, cwd: Option<PathBuf>) -> Result<Rc<Job>, String> {
        if let Some(job) = self.named(name) {
            if job.is_running() && !restart {
                return Err(format!("job {name} is still running (stop it, or pass restart)"));
            }
            *job.cwd.borrow_mut() = cwd;
            *job.command.borrow_mut() = Some(command.to_owned());
            job.chip.set_tooltip_text(Some(command));
            job.restart();
            return Ok(job);
        }
        let job = self.add(name, Some(command.to_owned()), command);
        *job.cwd.borrow_mut() = cwd;
        job.start();
        Ok(job)
    }

    /// How many jobs are running.
    pub fn running(&self) -> usize {
        self.jobs.borrow().iter().filter(|j| j.is_running()).count()
    }

    /// Every job: (name, running, exit code).
    pub fn list(&self) -> Vec<(String, bool, Option<i64>)> {
        self.jobs.borrow().iter().map(|j| (j.name.borrow().clone(), j.is_running(), j.outcome.get().flatten())).collect()
    }

    fn add(self: &Rc<Self>, name: &str, command: Option<String>, tooltip: &str) -> Rc<Job> {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("dot");
        dot.add_css_class("working");
        dot.set_valign(gtk::Align::Center);
        let label = gtk::Label::new(Some(&ellipsize(name, 28)));
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&dot);
        content.append(&label);
        let chip = gtk::Button::builder().child(&content).tooltip_text(tooltip).build();
        chip.add_css_class("chip");
        chip.add_css_class("flat");

        let term = term::new(&chip);
        term.set_size_request(760, 320);
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let status = gtk::Label::new(Some("running"));
        status.add_css_class("dim");
        let popover = gtk::Popover::new();
        popover.add_css_class("peek");
        popover.set_parent(&chip);

        let title = gtk::Label::new(Some(name));
        let job = Rc::new(Job {
            name: RefCell::new(name.to_owned()),
            title,
            command: RefCell::new(command),
            term,
            chip: chip.clone(),
            dot,
            popover: popover.clone(),
            holder: holder.clone(),
            status: status.clone(),
            outcome: Cell::new(None),
            pid: Cell::new(None),
            spawning: Cell::new(false),
            restart_pending: Cell::new(false),
            kill_timer: RefCell::default(),
            cwd: RefCell::default(),
            docked: RefCell::default(),
            tray: Rc::downgrade(self),
        });
        popover.set_child(Some(&job.frame(false)));
        holder.append(&job.term);
        if job.command.borrow().is_some() {
            let weak = Rc::downgrade(&job);
            job.term.connect_child_exited(move |_, status| {
                let Some(job) = weak.upgrade() else { return };
                job.pid.set(None);
                if let Some(timer) = job.kill_timer.borrow_mut().take() {
                    timer.remove();
                }
                // Asked to run again: this exit was the old run's.
                if job.restart_pending.take() {
                    job.start();
                    return;
                }
                // A wait status: the exit code in the second byte when the low 7 bits are 0.
                let code = if status & 0x7f == 0 { (status >> 8) & 0xff } else { 128 + (status & 0x7f) };
                job.finish(Some(i64::from(code)));
                let told = job.tray.upgrade().and_then(|tray| tray.on_finish.borrow().clone());
                if let Some(told) = told {
                    told(&job.name.borrow(), Some(i64::from(code)));
                }
            });
        }

        let weak = Rc::downgrade(&job);
        chip.connect_clicked(move |_| {
            if let Some(job) = weak.upgrade()
                && let Some(tray) = job.tray.upgrade()
            {
                tray.peek(&job);
            }
        });
        self.chips.append(&chip);
        self.hint.set_visible(false);
        self.jobs.borrow_mut().push(job.clone());
        self.trim();
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
        self.dock.set_visible(true);
    }

    fn unpin(&self, job: &Rc<Job>) {
        let Some(frame) = job.docked.borrow_mut().take() else { return };
        frame.remove(&job.term);
        self.dock.remove(&frame);
        job.holder.append(&job.term);
        self.dock.set_visible(self.dock.first_child().is_some());
    }

    fn remove(&self, job: &Rc<Job>) {
        self.unpin(job);
        job.popover.popdown();
        job.stop();
        job.popover.unparent();
        self.chips.remove(&job.chip);
        self.jobs.borrow_mut().retain(|other| !Rc::ptr_eq(other, job));
        self.hint.set_visible(self.jobs.borrow().is_empty());
    }

    /// Drops the oldest finished, unpinned jobs past KEEP.
    fn trim(&self) {
        loop {
            let old = {
                let jobs = self.jobs.borrow();
                if jobs.len() <= KEEP {
                    return;
                }
                jobs.iter()
                    .find(|job| job.command.borrow().is_none() && job.outcome.get().is_some() && job.docked.borrow().is_none())
                    .cloned()
            };
            match old {
                Some(job) => self.remove(&job),
                None => return,
            }
        }
    }

    pub fn clear_finished(&self) {
        let done: Vec<Rc<Job>> = self
            .jobs
            .borrow()
            .iter()
            .filter(|job| job.outcome.get().is_some() && job.docked.borrow().is_none())
            .cloned()
            .collect();
        for job in done {
            self.remove(&job);
        }
    }

    /// Stops the jobs this tray started (on quit).
    pub fn stop_all(&self) {
        for job in self.jobs.borrow().iter() {
            job.stop();
        }
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
        if self.command.borrow().is_some() {
            header.append(&button("Rerun", "Run the command again", |_, job| job.restart()));
            header.append(&button("Stop", "Stop the command", |_, job| job.stop()));
        }
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

    /// Names the job (an agent's command is known only after its first report).

    /// Output an agent's command printed.
    pub fn feed(&self, data: &str) {
        self.term.feed(&term::crlf(data));
    }

    /// The last `lines` lines it printed, as text.
    pub fn output(&self, lines: usize) -> String {
        let (_, row) = self.term.cursor_position();
        let start = (row - lines as libc::c_long).max(0);
        let (text, _) = self.term.text_range_format(vte::Format::Text, start, 0, row, self.term.column_count());
        text.map(|t| t.trim_end().to_owned()).unwrap_or_default()
    }

    /// Exit code once done (None while running or when unknown).
    pub fn exit_code(&self) -> Option<i64> {
        self.outcome.get().flatten()
    }

    /// Runs the command again: at once if nothing runs, else once the current run has
    /// exited (it is stopped). Its old output goes now, so nobody reads it as the new run's.
    pub fn restart(self: &Rc<Self>) {
        if self.pid.get().is_none() && !self.spawning.get() {
            self.start();
            return;
        }
        self.restart_pending.set(true);
        self.term.reset(true, true);
        self.outcome.set(None);
        self.status.set_label("restarting");
        self.terminate();
    }

    pub fn is_running(&self) -> bool {
        self.outcome.get().is_none()
    }

    pub fn finish(&self, code: Option<i64>) {
        self.outcome.set(Some(code));
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

    /// (Re)starts a command run from here.
    fn start(self: &Rc<Self>) {
        let Some(command) = self.command.borrow().clone() else { return };
        if self.spawning.get() || self.pid.get().is_some() {
            return;
        }
        let Some(tray) = self.tray.upgrade() else { return };
        self.spawning.set(true);
        self.term.reset(true, true);
        self.outcome.set(None);
        self.dot.remove_css_class("done");
        self.dot.remove_css_class("failed");
        self.dot.add_css_class("working");
        self.status.set_label("running");
        let weak = Rc::downgrade(self);
        let cwd = self.cwd.borrow().clone().unwrap_or_else(|| tray.cwd.clone()).to_string_lossy().into_owned();
        self.term.spawn_async(
            vte::PtyFlags::DEFAULT,
            Some(&cwd),
            &["/bin/bash", "-c", &command],
            &[],
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            None::<&gio::Cancellable>,
            move |result| {
                let Some(job) = weak.upgrade() else { return };
                job.spawning.set(false);
                match result {
                    Ok(pid) => {
                        job.pid.set(Some(pid));
                        // Asked to restart (or stop) while it was starting: that applies now.
                        if job.restart_pending.get() {
                            job.terminate();
                        }
                    }
                    Err(err) => {
                        job.feed(&format!("loom: could not start: {err}\n"));
                        job.finish(Some(127));
                    }
                }
            },
        );
    }

    /// Stops it (and any restart it was waiting for).
    pub fn stop(self: &Rc<Self>) {
        self.restart_pending.set(false);
        self.terminate();
    }

    /// SIGTERM to its process group, SIGKILL 3 s later if it is still there. The pid stays
    /// until the exit is seen, so it is never run twice.
    fn terminate(self: &Rc<Self>) {
        let Some(pid) = self.pid.get() else { return };
        // SAFETY: plain kill(2); the shell VTE started leads its own session and group.
        unsafe { libc::kill(-pid.0, libc::SIGTERM) };
        let group = pid.0;
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
            // Fired: the handle is spent (removing it later would be an error).
            if let Some(job) = weak.upgrade() {
                job.kill_timer.borrow_mut().take();
                if job.pid.get().is_none_or(|p| p.0 != group) {
                    return;
                }
            }
            // SAFETY: as above; a group already gone just gives ESRCH.
            unsafe { libc::kill(-group, libc::SIGKILL) };
        });
        if let Some(old) = self.kill_timer.borrow_mut().replace(timer) {
            old.remove();
        }
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
