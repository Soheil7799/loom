//! A project's page: start a new conversation, or pick up a past one.
//!
//! Past conversations come from the adapter (ACP `session/list`, the same list and titles
//! Claude Code shows), asked by a short-lived adapter process that stops once it answered.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::{gio, glib, pango, prelude::*};

use loom_core::provider::Conn;

pub use loom_core::history::*;

thread_local! {
    /// Every kept conversation, as last listed.
    static LAST: RefCell<Option<Vec<Past>>> = const { RefCell::new(None) };
}

type Callback<T> = RefCell<Option<Rc<dyn Fn(T)>>>;

/// The page: the project's name, "New conversation", and its past conversations.
pub struct Page {
    pub root: gtk::Box,
    title: gtk::Label,
    place: gtk::Label,
    list: gtk::ListBox,
    note: gtk::Label,
    /// Where a new conversation starts, and the project whose past ones are listed.
    cwd: RefCell<PathBuf>,
    project: RefCell<PathBuf>,
    /// What each row (after "New conversation") resumes.
    rows: RefCell<Vec<Past>>,
    /// Answers for an earlier project are dropped (and its listing stopped).
    round: Cell<u64>,
    filter: gtk::SearchEntry,
    me: RefCell<std::rc::Weak<Page>>,
    listing: RefCell<Option<std::rc::Weak<Conn>>>,
    pub on_new: Callback<PathBuf>,
    pub on_resume: Callback<Past>,
    /// Is this conversation open in loom already? (shown as such; picking it goes there)
    pub is_open: RefCell<Option<Rc<dyn Fn(&str) -> bool>>>,
}

impl Page {
    pub fn new() -> Rc<Page> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("chat");
        root.add_css_class("project-page");
        let column = gtk::Box::new(gtk::Orientation::Vertical, 14);
        column.add_css_class("project-column");

        let title = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        title.add_css_class("project-title");
        let place = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::Start).build();
        place.add_css_class("dim");
        place.add_css_class("small");
        let names = gtk::Box::new(gtk::Orientation::Vertical, 2);
        names.set_hexpand(true);
        names.append(&title);
        names.append(&place);
        // The project's name heads its board beside this (board.rs): here, what this list is.
        names.set_visible(false);
        let side = gtk::Label::builder().label("Conversations").xalign(0.0).build();
        side.add_css_class("side-title");
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        heading.append(&names);
        heading.append(&side);
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Browse).build();
        list.add_css_class("history");
        let filter = gtk::SearchEntry::builder().placeholder_text("Filter conversations").build();
        filter.add_css_class("history-filter");
        let note = gtk::Label::builder().xalign(0.0).wrap(true).build();
        note.add_css_class("dim");
        note.add_css_class("small");
        note.add_css_class("history-note");
        column.append(&heading);
        column.append(&filter);
        column.append(&list);
        column.append(&note);
        let clamp = gtk::Box::new(gtk::Orientation::Vertical, 0);
        clamp.set_layout_manager(Some(crate::column::ColumnLayout::new(420)));
        clamp.append(&column);
        let scroller = gtk::ScrolledWindow::builder().child(&clamp).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        root.append(&scroller);
        let page = Rc::new(Page {
            root,
            title,
            place,
            list,
            note,
            cwd: RefCell::default(),
            project: RefCell::default(),
            rows: RefCell::default(),
            round: Cell::new(0),
            filter: filter.clone(),
            me: RefCell::default(),
            listing: RefCell::default(),
            on_new: RefCell::default(),
            on_resume: RefCell::default(),
            is_open: RefCell::default(),
        });
        *page.me.borrow_mut() = Rc::downgrade(&page);
        // Rows whose title has every word typed (the "New conversation" row always stays).
        let weak = Rc::downgrade(&page);
        page.list.set_filter_func(move |row| {
            let Some(page) = weak.upgrade() else { return true };
            let words = page.filter.text().to_lowercase();
            if row.index() == 0 || words.trim().is_empty() {
                return true;
            }
            let rows = page.rows.borrow();
            let Some(past) = rows.get(row.index() as usize - 1) else { return true };
            let title = format!("{} {}", past.title.as_deref().unwrap_or(""), past.cwd.display()).to_lowercase();
            words.split_whitespace().all(|w| title.contains(w))
        });
        let weak = Rc::downgrade(&page);
        filter.connect_search_changed(move |_| {
            if let Some(page) = weak.upgrade() {
                page.list.invalidate_filter();
            }
        });
        // Enter in the filter opens the first match; Down goes to the list.
        let weak = Rc::downgrade(&page);
        filter.connect_activate(move |_| {
            let Some(page) = weak.upgrade() else { return };
            let mut index = 1;
            while let Some(row) = page.list.row_at_index(index) {
                if row.is_child_visible() {
                    page.activate(index);
                    return;
                }
                index += 1;
            }
            page.activate(0);
        });
        let weak = Rc::downgrade(&page);
        page.list.connect_row_activated(move |_, row| {
            if let Some(page) = weak.upgrade() {
                page.activate(row.index());
            }
        });
        page
    }

    /// Shows `project` (a new conversation starts in `cwd`) and lists its past ones.
    pub fn show(self: &Rc<Self>, project: &Path, cwd: &Path, name: &str) {
        *self.project.borrow_mut() = project.to_owned();
        *self.cwd.borrow_mut() = cwd.to_owned();
        self.set_name(name);
        self.place.set_label(&project.to_string_lossy().replacen(&*glib::home_dir().to_string_lossy(), "~", 1));
        self.place.set_tooltip_text(Some(&project.to_string_lossy()));
        self.filter.set_text("");
        // The last listing at once (any project's: it lists them all), then the agent
        // program's fresh one, drawn only if it differs.
        let last = LAST.with_borrow(Clone::clone);
        match &last {
            Some(all) => self.show_mine(all.clone()),
            None => {
                self.rows.borrow_mut().clear();
                self.fill(&[]);
                self.note.set_label("Looking for past conversations…");
                self.note.set_visible(true);
            }
        }
        let round = self.round.get() + 1;
        self.round.set(round);
        if let Some(old) = self.listing.take().and_then(|w| w.upgrade()) {
            old.stop();
        }
        let weak = Rc::downgrade(self);
        let listing = list(move |result| {
            let Some(page) = weak.upgrade() else { return };
            match result {
                Ok(all) => {
                    let changed = LAST.with_borrow(|l| l.as_ref() != Some(&all));
                    LAST.with_borrow_mut(|l| *l = Some(all.clone()));
                    if page.round.get() == round && changed {
                        page.show_mine(all);
                    }
                }
                // Said only where nothing was shown instead.
                Err(err) if page.round.get() == round && last.is_none() => page.note.set_label(&format!("Past conversations could not be listed: {err}.")),
                Err(_) => {}
            }
        });
        *self.listing.borrow_mut() = listing;
    }

    /// Of `all` the conversations, this project's, listed.
    fn show_mine(&self, all: Vec<Past>) {
        // Compared as real folders: one opened through a symlink (~/Projects/hr →
        // faratamin/personnel) is recorded under the link's name.
        let project = real(&self.project.borrow());
        let mine: Vec<Past> = all
            .into_iter()
            .map(|mut p| {
                p.cwd = real(&p.cwd);
                p
            })
            .filter(|p| p.cwd.starts_with(&project) && real(&crate::app::project_of(&p.cwd)) == project)
            .collect();
        self.note.set_label(if mine.is_empty() { "No past conversations here yet." } else { "" });
        self.note.set_visible(mine.is_empty());
        self.fill(&mine);
        *self.rows.borrow_mut() = mine;
    }

    fn fill(&self, past: &[Past]) {
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let new = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let plus = gtk::Label::new(Some("+"));
        plus.add_css_class("history-plus");
        let text = gtk::Label::builder().label("New conversation").xalign(0.0).hexpand(true).build();
        let keys = gtk::Label::new(Some("Enter"));
        keys.add_css_class("keycap");
        new.append(&plus);
        new.append(&text);
        new.append(&keys);
        let row = gtk::ListBoxRow::builder().child(&new).build();
        row.add_css_class("history-new");
        self.list.append(&row);
        self.list.select_row(Some(&row));

        let project = self.project.borrow().clone();
        let is_open = self.is_open.borrow().clone();
        for past in past {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let names = gtk::Box::new(gtk::Orientation::Vertical, 1);
            names.set_hexpand(true);
            let title = gtk::Label::builder().label(past.title.as_deref().unwrap_or("Untitled")).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            names.append(&title);
            // Where it ran, when not the project's own folder.
            if let Ok(sub) = past.cwd.strip_prefix(&project)
                && !sub.as_os_str().is_empty()
            {
                let place = gtk::Label::builder().label(sub.to_string_lossy()).xalign(0.0).ellipsize(pango::EllipsizeMode::Start).build();
                place.add_css_class("dim");
                place.add_css_class("small");
                names.append(&place);
            }
            line.append(&names);
            if is_open.as_ref().is_some_and(|open| open(&past.session_id)) {
                let open = gtk::Label::new(Some("open"));
                open.add_css_class("history-open");
                open.set_tooltip_text(Some("Open in loom already: picking it goes there"));
                line.append(&open);
            }
            let when = gtk::Label::new(Some(&ago(&past.updated)));
            when.add_css_class("dim");
            when.add_css_class("small");
            line.append(&when);
            let remove = gtk::Button::with_label("delete");
            remove.add_css_class("flat");
            remove.add_css_class("small");
            remove.add_css_class("history-delete");
            remove.set_tooltip_text(Some("Delete this conversation for good (asks first)"));
            let (me, id, title) = (self.me.borrow().clone(), past.session_id.clone(), past.title.clone().unwrap_or_else(|| "Untitled".to_owned()));
            remove.connect_clicked(move |button| {
                if let Some(page) = me.upgrade() {
                    page.ask_delete(button, id.clone(), title.clone());
                }
            });
            line.append(&remove);
            let row = gtk::ListBoxRow::builder().child(&line).build();
            row.set_tooltip_text(Some(&past.cwd.to_string_lossy()));
            self.list.append(&row);
        }
    }

    pub fn activate(&self, index: i32) {
        if index == 0 {
            let callback = self.on_new.borrow().clone();
            if let Some(callback) = callback {
                callback(self.cwd.borrow().clone());
            }
            return;
        }
        let past = self.rows.borrow().get(index as usize - 1).cloned();
        let callback = self.on_resume.borrow().clone();
        if let (Some(past), Some(callback)) = (past, callback) {
            callback(past);
        }
    }

    pub fn set_name(&self, name: &str) {
        self.title.set_label(name);
    }

    pub fn project(&self) -> PathBuf {
        self.project.borrow().clone()
    }

    fn ask_delete(&self, button: &gtk::Button, session: String, title: String) {
        let open = self.is_open.borrow().clone();
        if open.is_some_and(|open| open(&session)) {
            self.note.set_label("That conversation is open in loom: close it first (Ctrl+W), then delete it.");
            self.note.set_visible(true);
            return;
        }
        let dialog = gtk::AlertDialog::builder()
            .message(format!("Delete “{title}”?"))
            .detail("The conversation is removed from Claude Code for good; it can't be resumed afterwards.")
            .buttons(["Keep it", "Delete"])
            .cancel_button(0)
            .default_button(0)
            .build();
        let me = self.me.borrow().clone();
        let parent = button.root().and_downcast::<gtk::Window>();
        dialog.choose(parent.as_ref(), None::<&gio::Cancellable>, move |choice| {
            if choice != Ok(1) {
                return;
            }
            delete(session.clone(), move |result| {
                let Some(page) = me.upgrade() else { return };
                match result {
                    Ok(()) => {
                        let project = page.project();
                        let cwd = page.cwd.borrow().clone();
                        let name = page.title.label().to_string();
                        page.show(&project, &cwd, &name);
                    }
                    Err(err) => page.note.set_label(&format!("Could not delete it: {err}.")),
                }
            });
        });
    }

    /// Typing on the page filters it (the text goes into the filter).
    pub fn type_filter(&self, c: char) {
        self.filter.grab_focus();
        let mut text = self.filter.text().to_string();
        text.push(c);
        self.filter.set_text(&text);
        self.filter.set_position(-1);
    }

    /// Ctrl+F on the page.
    pub fn focus_filter(&self) {
        self.filter.grab_focus();
    }

    /// Keyboard focus on the list ("New conversation" selected: Enter starts one).
    pub fn grab_focus(&self) {
        if let Some(row) = self.list.selected_row().or_else(|| self.list.row_at_index(0)) {
            row.grab_focus();
        }
    }
}
