//! The scratchpad: a small shell that slides down over the window's top-right corner, one
//! per project, kept alive between toggles so its history and `cd`s stay.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::{gio, glib, prelude::*};
use vte::prelude::*;

use crate::term;

pub struct Scratch {
    pub revealer: gtk::Revealer,
    stack: gtk::Stack,
    title: gtk::Label,
    terms: RefCell<HashMap<PathBuf, vte::Terminal>>,
}

impl Scratch {
    pub fn new() -> Rc<Scratch> {
        let stack = gtk::Stack::new();
        stack.set_vexpand(false);
        stack.set_hexpand(false);
        let title = gtk::Label::builder().xalign(0.0).build();
        title.add_css_class("dim");
        title.add_css_class("small");
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 4);
        frame.add_css_class("scratch");
        frame.append(&title);
        frame.append(&stack);
        let revealer = gtk::Revealer::builder()
            .child(&frame)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(140)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(56)
            .margin_end(18)
            .build();
        // Hidden, it must not swallow clicks meant for the chat underneath.
        revealer.set_can_target(false);
        Rc::new(Scratch { revealer, stack, title, terms: RefCell::default() })
    }

    pub fn is_open(&self) -> bool {
        self.revealer.reveals_child()
    }

    /// Shows the project's shell (starting one if needed), or hides it.
    pub fn toggle(self: &Rc<Self>, project: &Path) -> bool {
        if self.is_open() {
            self.hide();
            return false;
        }
        let term = self.term_for(project);
        self.stack.set_visible_child(&term);
        self.title.set_label(&format!("scratch · {}", project.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
        self.revealer.set_can_target(true);
        self.revealer.set_reveal_child(true);
        term.grab_focus();
        true
    }

    pub fn hide(&self) {
        self.revealer.set_reveal_child(false);
        self.revealer.set_can_target(false);
    }

    fn term_for(self: &Rc<Self>, project: &Path) -> vte::Terminal {
        if let Some(term) = self.terms.borrow().get(project) {
            return term.clone();
        }
        let term = term::new(&self.stack);
        term.set_vexpand(false);
        term.set_hexpand(false);
        let leave = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        leave.connect_leave(move |_| {
            if let Some(scratch) = weak.upgrade()
                && crate::settings::get().scratch_autohide
            {
                scratch.hide();
            }
        });
        term.add_controller(leave);
        // VTE asks for 80x24 cells otherwise: a note, not a window.
        term.set_size(72, 12);
        self.stack.add_child(&term);
        spawn_shell(&term, project);
        // The shell exited (Ctrl+D): close, and start a fresh one next time.
        let project_owned = project.to_owned();
        let weak = Rc::downgrade(self);
        term.connect_child_exited(move |term, _| {
            let Some(scratch) = weak.upgrade() else { return };
            scratch.terms.borrow_mut().remove(&project_owned);
            if scratch.stack.visible_child().as_ref() == Some(term.upcast_ref()) {
                scratch.hide();
            }
            scratch.stack.remove(term);
        });
        self.terms.borrow_mut().insert(project.to_owned(), term.clone());
        term
    }
}

fn spawn_shell(term: &vte::Terminal, cwd: &Path) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned());
    let cwd = cwd.to_string_lossy().into_owned();
    term.spawn_async(
        vte::PtyFlags::DEFAULT,
        Some(&cwd),
        &[&shell],
        &[],
        glib::SpawnFlags::DEFAULT,
        || {},
        -1,
        None::<&gio::Cancellable>,
        |result| {
            if let Err(err) = result {
                eprintln!("loom: scratch shell did not start: {err}");
            }
        },
    );
}
