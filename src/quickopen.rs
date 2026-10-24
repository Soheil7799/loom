//! Ctrl+P: find a file in the project by typing parts of its path, and open it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::cell::RefCell;

use gtk::{gdk, glib, pango, prelude::*};

/// Directories never worth listing.
pub const SKIP: &[&str] = &[".git", "target", "node_modules", "__pycache__", ".venv", ".cache", ".next", "dist"];
const MAX_FILES: usize = 20_000;
const SHOWN: usize = 40;

pub struct QuickOpen {
    popover: gtk::Popover,
    entry: gtk::SearchEntry,
    list: gtk::ListBox,
    root: RefCell<PathBuf>,
    files: RefCell<Vec<String>>,
    shown: RefCell<Vec<String>>,
    on_pick: RefCell<Option<Rc<dyn Fn(PathBuf)>>>,
}

impl QuickOpen {
    pub fn new(parent: &impl IsA<gtk::Widget>) -> Rc<QuickOpen> {
        let entry = gtk::SearchEntry::builder().placeholder_text("Open file…").build();
        let list = gtk::ListBox::new();
        list.add_css_class("switcher-list");
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(420)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.set_size_request(560, -1);
        content.append(&entry);
        content.append(&scroller);
        let popover = gtk::Popover::builder().child(&content).has_arrow(false).position(gtk::PositionType::Bottom).build();
        popover.set_parent(parent);
        let quick = Rc::new(QuickOpen {
            popover,
            entry,
            list,
            root: RefCell::default(),
            files: RefCell::default(),
            shown: RefCell::default(),
            on_pick: RefCell::default(),
        });
        let weak = Rc::downgrade(&quick);
        quick.entry.connect_search_changed(move |_| {
            if let Some(quick) = weak.upgrade() {
                quick.fill();
            }
        });
        let weak = Rc::downgrade(&quick);
        quick.entry.connect_activate(move |_| {
            if let Some(quick) = weak.upgrade() {
                let index = quick.list.selected_row().map_or(0, |r| r.index());
                quick.pick(index);
            }
        });
        let weak = Rc::downgrade(&quick);
        quick.list.connect_row_activated(move |_, row| {
            if let Some(quick) = weak.upgrade() {
                quick.pick(row.index());
            }
        });
        let keys = gtk::EventControllerKey::new();
        let list = quick.list.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let index = list.selected_row().map_or(-1, |r| r.index());
            let next = match key {
                gdk::Key::Down => index + 1,
                gdk::Key::Up => (index - 1).max(0),
                _ => return glib::Propagation::Proceed,
            };
            if let Some(row) = list.row_at_index(next) {
                list.select_row(Some(&row));
            }
            glib::Propagation::Stop
        });
        quick.entry.add_controller(keys);
        quick
    }

    /// Lists `root`'s files and shows the picker; `on_pick` gets the chosen one.
    pub fn open(&self, root: &Path, on_pick: impl Fn(PathBuf) + 'static) {
        *self.root.borrow_mut() = root.to_owned();
        *self.files.borrow_mut() = list_files(root);
        *self.on_pick.borrow_mut() = Some(Rc::new(on_pick));
        self.entry.set_text("");
        self.fill();
        if let Some(parent) = self.popover.parent() {
            self.popover.set_pointing_to(Some(&gdk::Rectangle::new(parent.width() / 2, 12, 1, 1)));
        }
        self.popover.popup();
        self.entry.grab_focus();
    }

    fn fill(&self) {
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let query = self.entry.text().to_lowercase();
        let files = self.files.borrow();
        let mut ranked: Vec<(i64, &String)> = files.iter().filter_map(|f| score(&query, f).map(|s| (s, f))).collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
        let shown: Vec<String> = ranked.into_iter().take(SHOWN).map(|(_, f)| f.clone()).collect();
        drop(files);
        for file in &shown {
            let (dir, name) = match file.rsplit_once('/') {
                Some((dir, name)) => (dir, name),
                None => ("", file.as_str()),
            };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let name = gtk::Label::new(Some(name));
            let dir = gtk::Label::builder().label(dir).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::Start).build();
            dir.add_css_class("dim");
            dir.add_css_class("small");
            row.append(&name);
            row.append(&dir);
            self.list.append(&row);
        }
        *self.shown.borrow_mut() = shown;
        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
    }

    fn pick(&self, index: i32) {
        let Some(file) = self.shown.borrow().get(index.max(0) as usize).cloned() else { return };
        self.popover.popdown();
        let path = self.root.borrow().join(file);
        let on_pick = self.on_pick.borrow().clone();
        if let Some(on_pick) = on_pick {
            on_pick(path);
        }
    }
}

/// The project's files, relative: what git tracks (and untracked, unignored), else a walk.
pub fn list_files(root: &Path) -> Vec<String> {
    let git = Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(root)
        .output();
    if let Ok(output) = git
        && output.status.success()
    {
        return String::from_utf8_lossy(&output.stdout).lines().take(MAX_FILES).map(str::to_owned).collect();
    }
    let mut files = Vec::new();
    walk(root, root, 0, &mut files);
    files
}

fn walk(root: &Path, dir: &Path, depth: usize, files: &mut Vec<String>) {
    if depth > 8 || files.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {
                if !SKIP.contains(&name.as_str()) {
                    walk(root, &path, depth + 1, files);
                }
            }
            Ok(_) => {
                if let Ok(relative) = path.strip_prefix(root) {
                    files.push(relative.to_string_lossy().into_owned());
                }
            }
            Err(_) => {}
        }
    }
}

/// How well `query` matches `path`: its characters in order (None if not all are there).
/// Matches in the file name, at word starts and in runs score higher.
pub fn score(query: &str, path: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let lower = path.to_lowercase();
    let name_start = lower.rfind('/').map_or(0, |i| i + 1);
    let chars: Vec<(usize, char)> = lower.char_indices().collect();
    let mut score = 0i64;
    let mut at = 0usize;
    let mut previous: Option<usize> = None;
    for wanted in query.chars().filter(|c| !c.is_whitespace()) {
        let found = chars[at..].iter().position(|(_, c)| *c == wanted)?;
        let index = at + found;
        let (byte, _) = chars[index];
        score += 1;
        if byte >= name_start {
            score += 4;
        }
        if index == 0 || matches!(chars[index - 1].1, '/' | '_' | '-' | '.' | ' ') {
            score += 3;
        }
        if previous == Some(index.wrapping_sub(1)) {
            score += 2;
        }
        previous = Some(index);
        at = index + 1;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn letters_in_order_match() {
        assert!(score("agrs", "src/agent.rs").is_some());
        assert!(score("zz", "src/agent.rs").is_none());
    }

    #[test]
    fn the_file_name_beats_the_folder() {
        assert!(score("app", "src/app.rs") > score("app", "apps/x/main.rs"));
    }

    #[test]
    fn dotfiles_can_be_found() {
        assert!(score(".env", "config/.env").is_some());
    }
}
