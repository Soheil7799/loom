//! A project's page: start a new conversation, or pick up a past one.
//!
//! Past conversations come from the adapter (ACP `session/list`, the same list and titles
//! Claude Code shows), asked by a short-lived adapter process that stops once it answered.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::{glib, pango, prelude::*};
use serde_json::{Value, json};

use crate::acp::Conn;

/// A conversation Claude Code kept.
#[derive(Clone, Debug)]
pub struct Past {
    pub session_id: String,
    pub cwd: PathBuf,
    /// None: Claude Code has none for it.
    pub title: Option<String>,
    /// ISO 8601, as the adapter gives it.
    pub updated: String,
}

type Done = Box<dyn FnOnce(Result<Vec<Past>, String>)>;

/// Every kept conversation, newest first (a few pages at most), or why not. Returns the
/// listing process, to stop it early when its answer is no longer wanted.
pub fn list(done: impl FnOnce(Result<Vec<Past>, String>) + 'static) -> Option<std::rc::Weak<Conn>> {
    let done: Rc<RefCell<Option<Done>>> = Rc::new(RefCell::new(Some(Box::new(done))));
    let found: Rc<RefCell<Vec<Past>>> = Rc::default();
    let finish = {
        let done = done.clone();
        Rc::new(move |result: Result<Vec<Past>, String>| {
            if let Some(done) = done.borrow_mut().take() {
                done(result);
            }
        })
    };
    let on_exit = finish.clone();
    let conn = match Conn::spawn(&glib::home_dir(), move |incoming| {
        if let crate::acp::Incoming::Exited = incoming {
            on_exit(Err("the agent program stopped".to_owned()));
        }
    }) {
        Ok(conn) => conn,
        Err(err) => {
            finish(Err(format!("the agent program did not start: {err}")));
            return None;
        }
    };
    // Never left running: answered or not, it stops after 20 s (keeping the pages it gave).
    let weak = Rc::downgrade(&conn);
    let (late, partial) = (finish.clone(), found.clone());
    glib::timeout_add_local_once(Duration::from_secs(20), move || {
        let partial = newest_first(&partial);
        late(if partial.is_empty() { Err("the agent program did not answer".to_owned()) } else { Ok(partial) });
        if let Some(conn) = weak.upgrade() {
            conn.stop();
        }
    });
    let params = json!({
        "protocolVersion": 1,
        "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
        "clientInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") }
    });
    let next = conn.clone();
    conn.request("initialize", params, move |result| match result {
        Ok(_) => page(next, None, found, finish),
        Err(err) => {
            next.stop();
            finish(Err(message(&err)));
        }
    });
    Some(Rc::downgrade(&conn))
}

fn newest_first(found: &RefCell<Vec<Past>>) -> Vec<Past> {
    let mut all = found.borrow().clone();
    all.sort_by(|a, b| b.updated.cmp(&a.updated));
    all
}

/// Asks for one page (after `cursor`); the next, or the end.
fn page(conn: Rc<Conn>, cursor: Option<String>, found: Rc<RefCell<Vec<Past>>>, finish: Rc<impl Fn(Result<Vec<Past>, String>) + 'static>) {
    let params = match &cursor {
        Some(cursor) => json!({ "cursor": cursor }),
        None => json!({}),
    };
    let next = conn.clone();
    conn.request("session/list", params, move |result| {
        let value = match result {
            Ok(value) => value,
            Err(err) => {
                next.stop();
                return finish(Err(message(&err)));
            }
        };
        found.borrow_mut().extend(parse(&value));
        let count = found.borrow().len();
        match value["nextCursor"].as_str() {
            // Ten thousand is plenty to find a project's recent ones.
            Some(cursor) if count < 10_000 => page(next, Some(cursor.to_owned()), found, finish),
            _ => {
                next.stop();
                finish(Ok(newest_first(&found)));
            }
        }
    });
}

/// The folder with symlinks resolved (as it is, if it is gone).
pub fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

fn parse(value: &Value) -> Vec<Past> {
    value["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(Past {
                session_id: s["sessionId"].as_str()?.to_owned(),
                cwd: PathBuf::from(s["cwd"].as_str()?),
                title: s["title"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned),
                updated: s["updatedAt"].as_str().unwrap_or("").to_owned(),
            })
        })
        .collect()
}

fn message(err: &Value) -> String {
    err["message"].as_str().unwrap_or("unknown error").to_owned()
}

/// "just now", "5 min ago", "3 h ago", "yesterday", "4 days ago", else the date.
pub fn ago(iso: &str, now: &glib::DateTime) -> String {
    let Ok(then) = glib::DateTime::from_iso8601(iso, None) else { return String::new() };
    let seconds = now.difference(&then).as_seconds();
    match seconds {
        ..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86400 => format!("{} h ago", seconds / 3600),
        86400..172800 => "yesterday".to_owned(),
        172800..604800 => format!("{} days ago", seconds / 86400),
        _ => then.to_local().and_then(|t| t.format("%e %b %Y")).map(|s| s.trim().to_owned()).unwrap_or_default(),
    }
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
        let heading = gtk::Box::new(gtk::Orientation::Vertical, 2);
        heading.append(&title);
        heading.append(&place);
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Browse).build();
        list.add_css_class("history");
        let note = gtk::Label::builder().xalign(0.0).wrap(true).build();
        note.add_css_class("dim");
        note.add_css_class("small");
        note.add_css_class("history-note");
        column.append(&heading);
        column.append(&list);
        column.append(&note);
        let clamp = gtk::Box::new(gtk::Orientation::Vertical, 0);
        clamp.set_layout_manager(Some(crate::column::ColumnLayout::new(720)));
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
            listing: RefCell::default(),
            on_new: RefCell::default(),
            on_resume: RefCell::default(),
            is_open: RefCell::default(),
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
    pub fn show(self: &Rc<Self>, project: &Path, cwd: &Path) {
        *self.project.borrow_mut() = project.to_owned();
        *self.cwd.borrow_mut() = cwd.to_owned();
        self.title.set_label(&project.file_name().map_or_else(|| project.to_string_lossy(), |n| n.to_string_lossy()));
        self.place.set_label(&project.to_string_lossy().replacen(&*glib::home_dir().to_string_lossy(), "~", 1));
        self.place.set_tooltip_text(Some(&project.to_string_lossy()));
        self.rows.borrow_mut().clear();
        self.fill(&[]);
        self.note.set_label("Looking for past conversations…");
        let round = self.round.get() + 1;
        self.round.set(round);
        if let Some(old) = self.listing.take().and_then(|w| w.upgrade()) {
            old.stop();
        }
        let weak = Rc::downgrade(self);
        let listing = list(move |result| {
            let Some(page) = weak.upgrade() else { return };
            if page.round.get() != round {
                return;
            }
            match result {
                Ok(all) => {
                    let project = page.project.borrow().clone();
                    // Compared as real folders: one opened through a symlink (~/Projects/hr →
                    // faratamin/personnel) is recorded under the link's name.
                    let project = real(&project);
                    let mine: Vec<Past> = all
                        .into_iter()
                        .map(|mut p| {
                            p.cwd = real(&p.cwd);
                            p
                        })
                        .filter(|p| p.cwd.starts_with(&project) && real(&crate::app::project_of(&p.cwd)) == project)
                        .collect();
                    page.note.set_label(if mine.is_empty() { "No past conversations here yet." } else { "" });
                    page.note.set_visible(mine.is_empty());
                    page.fill(&mine);
                    *page.rows.borrow_mut() = mine;
                }
                Err(err) => page.note.set_label(&format!("Past conversations could not be listed: {err}.")),
            }
        });
        *self.listing.borrow_mut() = listing;
        self.note.set_visible(true);
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

        let now = glib::DateTime::now_local().ok();
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
            if let Some(now) = &now {
                let when = gtk::Label::new(Some(&ago(&past.updated, now)));
                when.add_css_class("dim");
                when.add_css_class("small");
                line.append(&when);
            }
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

    pub fn project(&self) -> PathBuf {
        self.project.borrow().clone()
    }

    /// Keyboard focus on the list ("New conversation" selected: Enter starts one).
    pub fn grab_focus(&self) {
        if let Some(row) = self.list.selected_row().or_else(|| self.list.row_at_index(0)) {
            row.grab_focus();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_read_from_the_list() {
        let value = json!({ "sessions": [
            { "sessionId": "a", "cwd": "/p", "title": "  Fix it ", "updatedAt": "2026-10-25T09:00:00Z" },
            { "sessionId": "b", "cwd": "/p/web", "title": null, "updatedAt": "2026-10-24T09:00:00Z" },
            { "cwd": "/p" }
        ]});
        let past = parse(&value);
        assert_eq!(past.len(), 2);
        assert_eq!(past[0].title.as_deref(), Some("Fix it"));
        assert_eq!(past[1].title, None);
        assert_eq!(past[1].cwd, PathBuf::from("/p/web"));
    }

    #[test]
    fn times_read_as_ago() {
        let now = glib::DateTime::from_iso8601("2026-10-25T12:00:00Z", None).unwrap();
        assert_eq!(ago("2026-10-25T11:59:30Z", &now), "just now");
        assert_eq!(ago("2026-10-25T11:15:00Z", &now), "45 min ago");
        assert_eq!(ago("2026-10-25T09:00:00Z", &now), "3 h ago");
        assert_eq!(ago("2026-10-24T09:00:00Z", &now), "yesterday");
        assert_eq!(ago("2026-10-21T09:00:00Z", &now), "4 days ago");
        assert_eq!(ago("nonsense", &now), "");
    }
}
