//! The Notes view's right panel: Topics (each topic with its notes, newest first; its
//! description and a chat about it a click away; the other notes after them) and History
//! (Notes' own panel: Scribe's questions, what waits for your OK, what was written).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::{glib, pango, prelude::*};

use crate::topics::Topic;

/// Notes listed under a topic before "Show all".
const FIRST: usize = 5;

/// Where "Show all" opens the notes of no topic.
const OTHER: &str = "";

type Callback<T> = RefCell<Option<Rc<dyn Fn(T)>>>;

/// What a row of the list does.
enum Row {
    Topic,
    Note(PathBuf),
    /// "Show all" (or fewer) of a topic's notes.
    More(String),
    /// Two versions of a note: (its conflict copy, the note); no copy: the note holds both.
    Conflict(String, String),
}

pub struct NotesPanel {
    pub root: gtk::Box,
    topics_tab: gtk::ToggleButton,
    history_tab: gtk::ToggleButton,
    /// Where Notes' panel goes while the view shows (it is the Notes tab's otherwise).
    pub history: gtk::Box,
    filter: gtk::SearchEntry,
    list: gtk::ListBox,
    note: gtk::Label,
    rows: RefCell<Vec<Row>>,
    topics: RefCell<Vec<Topic>>,
    /// Every note: (path in the vault, title, last change), newest first.
    notes: RefCell<Vec<(String, String, u64)>>,
    /// Topics shown with all their notes (by name; `OTHER` for the notes of none).
    expanded: RefCell<HashSet<String>>,
    /// Notes with a merge left undone in them (<<<<<<< lines), by path in the vault.
    marked: RefCell<Vec<String>>,
    /// The vault's folders watched while the view shows (a sync adding or changing notes).
    watches: RefCell<Vec<gtk::gio::FileMonitor>>,
    watch_pending: Cell<bool>,
    reading: Cell<bool>,
    /// Asked again while reading: read once more after.
    again: Cell<bool>,
    /// A note to open (its full path).
    pub on_open: Callback<PathBuf>,
    /// A chat about a topic: (its name, its folder).
    pub on_chat: Callback<(String, String)>,
    /// Two versions of a note to settle: (the conflict copy, or "" if the note holds both;
    /// the note), paths in the vault.
    pub on_conflict: Callback<(String, String)>,
}

impl NotesPanel {
    pub fn new() -> Rc<NotesPanel> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("files");
        root.add_css_class("notes-panel");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.add_css_class("panel-header");
        let topics_tab = gtk::ToggleButton::builder().label("Topics").active(true).tooltip_text("Your topics and their notes, newest first").build();
        let history_tab = gtk::ToggleButton::builder().label("History").group(&topics_tab).tooltip_text("Scribe's questions, changes waiting for your OK, what was written").build();
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabs.add_css_class("segmented");
        for tab in [&topics_tab, &history_tab] {
            tab.add_css_class("flat");
            tab.add_css_class("tab");
            tab.remove_css_class("text-button");
            tab.remove_css_class("toggle");
            tabs.append(tab);
        }
        header.append(&tabs);
        root.append(&header);

        let filter = gtk::SearchEntry::builder().placeholder_text("Filter the notes").build();
        filter.add_css_class("notes-filter");
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        list.add_css_class("notes-topics");
        let note = gtk::Label::builder().xalign(0.0).wrap(true).build();
        note.add_css_class("dim");
        note.add_css_class("small");
        let topics_view = gtk::Box::new(gtk::Orientation::Vertical, 6);
        topics_view.append(&filter);
        topics_view.append(&gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build());
        topics_view.append(&note);
        let history = gtk::Box::new(gtk::Orientation::Vertical, 0);
        history.set_vexpand(true);
        let views = gtk::Stack::new();
        views.set_vexpand(true);
        views.add_named(&topics_view, Some("topics"));
        views.add_named(&history, Some("history"));
        root.append(&views);

        let panel = Rc::new(NotesPanel {
            root,
            topics_tab: topics_tab.clone(),
            history_tab: history_tab.clone(),
            history,
            filter,
            list,
            note,
            rows: RefCell::default(),
            topics: RefCell::default(),
            notes: RefCell::default(),
            expanded: RefCell::default(),
            marked: RefCell::default(),
            watches: RefCell::default(),
            watch_pending: Cell::new(false),
            reading: Cell::new(false),
            again: Cell::new(false),
            on_open: RefCell::default(),
            on_chat: RefCell::default(),
            on_conflict: RefCell::default(),
        });
        let (stack, weak) = (views.clone(), Rc::downgrade(&panel));
        topics_tab.connect_toggled(move |tab| {
            if tab.is_active() {
                stack.set_visible_child_name("topics");
                if let Some(panel) = weak.upgrade() {
                    panel.refresh();
                }
            }
        });
        let stack = views.clone();
        history_tab.connect_toggled(move |tab| {
            if tab.is_active() {
                stack.set_visible_child_name("history");
                tab.remove_css_class("news");
            }
        });
        let weak = Rc::downgrade(&panel);
        panel.filter.connect_search_changed(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.fill();
            }
        });
        let weak = Rc::downgrade(&panel);
        panel.list.connect_row_activated(move |_, row| {
            if let Some(panel) = weak.upgrade() {
                panel.activate(row.index() as usize);
            }
        });
        panel
    }

    /// The topics and notes, read again (off the main loop).
    pub fn refresh(self: &Rc<Self>) {
        let Some(vault) = crate::vault::current() else {
            self.note.set_label("Notes is off: switch it on in Settings → Add-ons → Notes.");
            return;
        };
        if self.reading.replace(true) {
            self.again.set(true);
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let read = gtk::gio::spawn_blocking(move || {
                let notes = crate::vault::all_notes(&vault);
                let marked = notes
                    .iter()
                    .filter(|n| crate::vault::conflict_of(&n.0).is_none())
                    .filter(|n| std::fs::read_to_string(vault.join(&n.0)).is_ok_and(|t| crate::vault::has_conflict_marks(&t)))
                    .map(|n| n.0.clone())
                    .collect::<Vec<_>>();
                (crate::topics::list(&vault), notes, marked)
            })
            .await;
            let Some(panel) = weak.upgrade() else { return };
            panel.reading.set(false);
            if let Ok((topics, notes, marked)) = read {
                *panel.topics.borrow_mut() = topics;
                *panel.notes.borrow_mut() = notes;
                *panel.marked.borrow_mut() = marked;
                panel.fill();
            }
            if panel.again.take() {
                panel.refresh();
            }
        });
    }

    /// While the view shows (`on`), the vault's folders are watched: a note added, changed or
    /// gone (a sync, another app) reads the panel again once things settle. Off: not watched.
    pub fn watch(self: &Rc<Self>, on: bool) {
        for monitor in self.watches.take() {
            monitor.cancel();
        }
        let Some(vault) = crate::vault::current().filter(|_| on) else { return };
        // Its folders, not loom's, git's or Obsidian's (hidden), a few hundred at most.
        let mut folders = vec![vault];
        let mut at = 0;
        while at < folders.len() && folders.len() < 400 {
            if let Ok(entries) = std::fs::read_dir(&folders[at]) {
                for entry in entries.flatten() {
                    if !entry.file_name().to_string_lossy().starts_with('.') && entry.file_type().is_ok_and(|k| k.is_dir()) {
                        folders.push(entry.path());
                    }
                }
            }
            at += 1;
        }
        let mut watches = Vec::new();
        for folder in folders {
            let Ok(monitor) = gtk::gio::File::for_path(&folder).monitor_directory(gtk::gio::FileMonitorFlags::WATCH_MOVES, None::<&gtk::gio::Cancellable>) else { continue };
            let weak = Rc::downgrade(self);
            monitor.connect_changed(move |_, file, _, _| {
                if file.basename().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                    return;
                }
                let Some(panel) = weak.upgrade() else { return };
                if panel.watch_pending.replace(true) {
                    return;
                }
                let weak = weak.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                    let Some(panel) = weak.upgrade() else { return };
                    panel.watch_pending.set(false);
                    if panel.watches.borrow().is_empty() {
                        return;
                    }
                    // A new folder is watched too.
                    panel.watch(true);
                    panel.refresh();
                });
            });
            watches.push(monitor);
        }
        *self.watches.borrow_mut() = watches;
    }

    /// Something new in History: its tab marked, until looked at.
    pub fn set_news(&self) {
        if !self.history_tab.is_active() {
            self.history_tab.add_css_class("news");
        }
    }

    pub fn show_topics(&self) {
        self.topics_tab.set_active(true);
    }

    /// Test hook: the first note with two versions, opened to settle.
    pub fn settle_for_tests(&self) {
        let first = self.rows.borrow().iter().position(|r| matches!(r, Row::Conflict(..)));
        if let Some(index) = first {
            self.activate(index);
        }
    }

    pub fn focus_filter(&self) {
        self.show_topics();
        self.filter.grab_focus();
    }

    fn fill(&self) {
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let mut rows = Vec::new();
        let filter = self.filter.text().to_lowercase();
        let notes = self.notes.borrow();
        let matches = |n: &&(String, String, u64)| crate::vault::conflict_of(&n.0).is_none() && (filter.is_empty() || n.0.to_lowercase().contains(&filter));
        let in_folder = |folder: &str, path: &str| !folder.is_empty() && path.starts_with(&format!("{}/", folder.trim_end_matches('/')));
        let topics = self.topics.borrow();
        let mut groups: Vec<(Option<&Topic>, String, Vec<&(String, String, u64)>)> = topics
            .iter()
            .map(|t| (Some(t), t.name.clone(), notes.iter().filter(|n| in_folder(&t.folder, &n.0)).filter(matches).collect()))
            .collect();
        let other: Vec<&(String, String, u64)> = notes.iter().filter(|n| !topics.iter().any(|t| in_folder(&t.folder, &n.0))).filter(matches).collect();
        groups.push((None, OTHER.to_owned(), other));
        let vault = crate::vault::current().unwrap_or_default();
        // Two versions of a note (a sync tool's conflict copy, or a merge left undone): first.
        let mut conflicts: Vec<(String, String)> = notes.iter().filter_map(|n| crate::vault::conflict_of(&n.0).map(|of| (n.0.clone(), of))).collect();
        conflicts.extend(self.marked.borrow().iter().map(|path| (String::new(), path.clone())));
        if !conflicts.is_empty() {
            self.list.append(&conflicts_heading(conflicts.len()));
            rows.push(Row::Topic);
            for (copy, note) in conflicts {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let title = note.rsplit('/').next().unwrap_or(&note).trim_end_matches(".md").to_owned();
                let label = gtk::Label::builder().label(title).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).tooltip_text(note.as_str()).build();
                let what = gtk::Label::new(Some(if copy.is_empty() { "two versions in it" } else { "and another version" }));
                what.add_css_class("dim");
                what.add_css_class("small");
                line.append(&label);
                line.append(&what);
                let row = gtk::ListBoxRow::builder().child(&line).tooltip_text("Two versions of this note: compare them and keep one, both, or a merge").build();
                row.add_css_class("notes-note");
                row.add_css_class("notes-conflict");
                self.list.append(&row);
                rows.push(Row::Conflict(copy, note));
            }
        }
        for (topic, key, found) in groups {
            // A filter shows only what matches it.
            let name_matches = topic.is_some_and(|t| !filter.is_empty() && t.name.to_lowercase().contains(&filter));
            if !filter.is_empty() && found.is_empty() && !name_matches {
                continue;
            }
            if topic.is_none() && found.is_empty() {
                continue;
            }
            self.list.append(&self.topic_row(topic, found.len()));
            rows.push(Row::Topic);
            let all = !filter.is_empty() || self.expanded.borrow().contains(&key);
            for (path, title, when) in found.iter().take(if all { usize::MAX } else { FIRST }) {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let label = gtk::Label::builder().label(title.as_str()).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).tooltip_text(path.as_str()).build();
                let ago = gtk::Label::new(Some(&crate::vault::ago(*when)));
                ago.add_css_class("dim");
                ago.add_css_class("small");
                line.append(&label);
                line.append(&ago);
                let row = gtk::ListBoxRow::builder().child(&line).build();
                row.add_css_class("notes-note");
                self.list.append(&row);
                rows.push(Row::Note(vault.join(path)));
            }
            if found.len() > FIRST && filter.is_empty() {
                let label = gtk::Label::builder().label(if all { "Show fewer".to_owned() } else { format!("Show all {}", found.len()) }).xalign(0.0).build();
                label.add_css_class("dim");
                label.add_css_class("small");
                let row = gtk::ListBoxRow::builder().child(&label).build();
                row.add_css_class("notes-more");
                self.list.append(&row);
                rows.push(Row::More(key));
            }
        }
        *self.rows.borrow_mut() = rows;
        self.note.set_label(match (self.topics.borrow().is_empty(), notes.is_empty()) {
            (_, true) => "No notes yet: ask the chat to keep something, or Scribe will.",
            (true, false) => "No topics yet: Settings → Add-ons → Notes makes them.",
            _ => "",
        });
    }

    /// A topic's heading: its name and count, its description and a chat about it.
    fn topic_row(&self, topic: Option<&Topic>, count: usize) -> gtk::ListBoxRow {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let name = gtk::Label::builder()
            .label(topic.map_or("Other notes", |t| t.name.as_str()))
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        name.add_css_class("notes-topic-name");
        let count = gtk::Label::new(Some(&count.to_string()));
        count.add_css_class("dim");
        count.add_css_class("small");
        line.append(&name);
        line.append(&count);
        if let Some(topic) = topic {
            if !topic.enabled {
                name.add_css_class("dim");
                name.set_tooltip_text(Some("Off: Scribe does not write here"));
            }
            let about = gtk::Button::from_icon_name("help-about-symbolic");
            about.add_css_class("flat");
            about.add_css_class("icon");
            about.set_tooltip_text(Some("About this topic: what it keeps (its description, editable)"));
            about.update_property(&[gtk::accessible::Property::Label("About this topic")]);
            let (open, path) = (self.on_open.borrow().clone(), topic.path.clone());
            about.connect_clicked(move |_| {
                if let Some(open) = &open {
                    open(path.clone());
                }
            });
            let chat = gtk::Button::from_icon_name("mail-reply-sender-symbolic");
            chat.add_css_class("flat");
            chat.add_css_class("icon");
            chat.set_tooltip_text(Some("Chat about this topic"));
            chat.update_property(&[gtk::accessible::Property::Label("Chat about this topic")]);
            let (on_chat, scope) = (self.on_chat.borrow().clone(), (topic.name.clone(), topic.folder.clone()));
            chat.connect_clicked(move |_| {
                if let Some(on_chat) = &on_chat {
                    on_chat(scope.clone());
                }
            });
            line.append(&about);
            line.append(&chat);
        }
        let row = gtk::ListBoxRow::builder().child(&line).activatable(false).build();
        row.add_css_class("notes-topic");
        row
    }

    fn activate(&self, index: usize) {
        let rows = self.rows.borrow();
        match rows.get(index) {
            Some(Row::Note(path)) => {
                if let Some(open) = self.on_open.borrow().clone() {
                    open(path.clone());
                }
            }
            Some(Row::Conflict(copy, note)) => {
                if let Some(settle) = self.on_conflict.borrow().clone() {
                    settle((copy.clone(), note.clone()));
                }
            }
            Some(Row::More(key)) => {
                let key = key.clone();
                drop(rows);
                let mut expanded = self.expanded.borrow_mut();
                if !expanded.remove(&key) {
                    expanded.insert(key);
                }
                drop(expanded);
                self.fill();
            }
            _ => {}
        }
    }
}

/// "Sync conflicts", over the notes that have two versions.
fn conflicts_heading(count: usize) -> gtk::ListBoxRow {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let name = gtk::Label::builder().label("Sync conflicts").xalign(0.0).hexpand(true).build();
    name.add_css_class("notes-topic-name");
    let number = gtk::Label::new(Some(&count.to_string()));
    number.add_css_class("dim");
    number.add_css_class("small");
    line.append(&name);
    line.append(&number);
    let row = gtk::ListBoxRow::builder().child(&line).activatable(false).build();
    row.add_css_class("notes-topic");
    row.add_css_class("notes-conflicts");
    row
}
