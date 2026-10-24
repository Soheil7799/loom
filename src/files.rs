//! The files side panel: what the focused agent created or changed (an .env to fill in, a
//! document it wrote) or the whole project as a tree, and an editor to open them in without
//! leaving the app.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use gtk::{gio, glib, pango, prelude::*};
use sourceview::prelude::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mark {
    Created,
    Changed,
}

pub struct Files {
    pub root: gtk::Box,
    list: gtk::ListBox,
    empty: gtk::Label,
    editor_box: gtk::Box,
    name: gtk::Label,
    dirty: gtk::Label,
    view: sourceview::View,
    buffer: sourceview::Buffer,
    message: gtk::Label,
    open: RefCell<Option<PathBuf>>,
    opened_at: Cell<Option<SystemTime>>,
    /// The on-disk version (mtime) the user was warned about: a second Save overwrites it,
    /// a newer one warns again.
    warned: Cell<Option<SystemTime>>,
    /// Where Open… starts.
    base: RefCell<PathBuf>,
    shown: RefCell<Vec<(PathBuf, Mark)>>,
    views: gtk::Stack,
    project_tab: gtk::ToggleButton,
    /// Changes: the repo picked, its branch line, and its changed files.
    changes_tab: gtk::ToggleButton,
    repo_pick: gtk::DropDown,
    repo_line: gtk::Label,
    changes: gtk::ListBox,
    /// The repos offered: the agent's (latest first), then the others in the project.
    repos: RefCell<Vec<PathBuf>>,
    agent_repos: RefCell<Vec<PathBuf>>,
    shown_changes: RefCell<Vec<PathBuf>>,
    filling: Cell<bool>,
    /// Bumped by each Changes fill: an answer from git for an older one is dropped.
    changes_round: Cell<u64>,
    tree: gtk::ListView,
    /// The folder the tree shows.
    project: RefCell<PathBuf>,
}

impl Files {
    pub fn new() -> Rc<Files> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("files");

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.add_css_class("panel-header");
        let agent_tab = gtk::ToggleButton::builder().label("Agent").active(true).tooltip_text("Files the agent wrote").build();
        let project_tab = gtk::ToggleButton::builder().label("Project").group(&agent_tab).tooltip_text("The project's files").build();
        let changes_tab = gtk::ToggleButton::builder().label("Changes").group(&agent_tab).tooltip_text("Git status of the repos in this project").build();
        for tab in [&agent_tab, &project_tab, &changes_tab] {
            tab.add_css_class("flat");
            tab.add_css_class("tab");
        }
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let open_button = gtk::Button::from_icon_name("document-open-symbolic");
        open_button.add_css_class("flat");
        open_button.set_tooltip_text(Some("Open any file (Ctrl+O; Ctrl+P to find one by name)"));
        header.append(&agent_tab);
        header.append(&project_tab);
        header.append(&changes_tab);
        header.append(&spacer);
        header.append(&open_button);
        root.append(&header);

        let list = gtk::ListBox::new();
        list.add_css_class("file-list");
        let empty = gtk::Label::builder()
            .label("Files the agent writes show up here.")
            .wrap(true)
            .margin_top(12)
            .margin_bottom(12)
            .build();
        empty.add_css_class("dim");
        let list_scroller = gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        let agent_view = gtk::Box::new(gtk::Orientation::Vertical, 0);
        agent_view.append(&empty);
        agent_view.append(&list_scroller);

        let tree = gtk::ListView::builder().factory(&tree_factory()).build();
        tree.add_css_class("tree");
        let tree_scroller = gtk::ScrolledWindow::builder().child(&tree).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        let repo_pick = gtk::DropDown::from_strings(&[]);
        repo_pick.set_hexpand(true);
        let refresh = gtk::Button::with_label("↻");
        refresh.add_css_class("flat");
        refresh.set_tooltip_text(Some("Read git status again"));
        let pick_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        pick_line.append(&repo_pick);
        pick_line.append(&refresh);
        let repo_line = gtk::Label::builder().xalign(0.0).build();
        repo_line.add_css_class("repo-line");
        let changes = gtk::ListBox::new();
        changes.add_css_class("file-list");
        let changes_scroller = gtk::ScrolledWindow::builder().child(&changes).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        let changes_view = gtk::Box::new(gtk::Orientation::Vertical, 6);
        changes_view.add_css_class("changes-view");
        changes_view.append(&pick_line);
        changes_view.append(&repo_line);
        changes_view.append(&changes_scroller);
        let views = gtk::Stack::new();
        views.add_named(&agent_view, Some("agent"));
        views.add_named(&tree_scroller, Some("project"));
        views.add_named(&changes_view, Some("changes"));
        views.set_size_request(-1, 120);

        let editor_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        editor_box.set_visible(false);
        editor_box.set_vexpand(true);
        let editor_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        editor_header.add_css_class("panel-header");
        let name = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::Start).build();
        name.add_css_class("mono");
        let dirty = gtk::Label::new(Some("●"));
        dirty.add_css_class("dirty");
        dirty.set_visible(false);
        dirty.set_tooltip_text(Some("Unsaved (Ctrl+S)"));
        let save = gtk::Button::with_label("Save");
        save.add_css_class("flat");
        let close = gtk::Button::with_label("Close");
        close.add_css_class("flat");
        let in_nevis = gtk::Button::with_label("nevis");
        in_nevis.add_css_class("flat");
        in_nevis.set_tooltip_text(Some("Open in nevis, with Live Preview (Ctrl+Shift+O)"));
        in_nevis.set_visible(glib::find_program_in_path("nevis").is_some());
        editor_header.append(&name);
        editor_header.append(&dirty);
        editor_header.append(&in_nevis);
        editor_header.append(&save);
        editor_header.append(&close);
        let buffer = sourceview::Buffer::new(None);
        buffer.set_highlight_syntax(true);
        let view = sourceview::View::with_buffer(&buffer);
        view.set_monospace(true);
        view.set_show_line_numbers(true);
        view.set_tab_width(4);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_top_margin(6);
        view.set_left_margin(6);
        view.add_css_class("editor");
        let view_scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
        editor_box.set_size_request(-1, 200);
        editor_box.append(&editor_header);
        let message = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
        message.add_css_class("editor-message");
        editor_box.append(&message);
        editor_box.append(&view_scroller);
        // The lists above, the editor below; with no file open the lists take it all.
        let split = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&views)
            .end_child(&editor_box)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .vexpand(true)
            .position(220)
            .build();
        root.append(&split);

        let files = Rc::new(Files {
            root,
            list,
            empty,
            editor_box,
            name,
            dirty,
            view,
            buffer,
            message,
            open: RefCell::default(),
            opened_at: Cell::new(None),
            warned: Cell::new(None),
            base: RefCell::new(glib::home_dir()),
            shown: RefCell::default(),
            views,
            project_tab: project_tab.clone(),
            changes_tab: changes_tab.clone(),
            repo_pick,
            repo_line,
            changes,
            repos: RefCell::default(),
            agent_repos: RefCell::default(),
            shown_changes: RefCell::default(),
            filling: Cell::new(false),
            changes_round: Cell::new(0),
            tree,
            project: RefCell::default(),
        });
        let views = files.views.clone();
        agent_tab.connect_toggled(move |tab| {
            if tab.is_active() {
                views.set_visible_child_name("agent");
            }
        });
        let weak = Rc::downgrade(&files);
        project_tab.connect_toggled(move |tab| {
            if let Some(files) = weak.upgrade()
                && tab.is_active()
            {
                files.views.set_visible_child_name("project");
                files.fill_tree();
            }
        });
        let weak = Rc::downgrade(&files);
        changes_tab.connect_toggled(move |tab| {
            if let Some(files) = weak.upgrade()
                && tab.is_active()
            {
                files.views.set_visible_child_name("changes");
                files.fill_repos();
            }
        });
        let weak = Rc::downgrade(&files);
        files.repo_pick.connect_selected_notify(move |_| {
            if let Some(files) = weak.upgrade()
                && !files.filling.get()
            {
                files.fill_changes();
            }
        });
        let weak = Rc::downgrade(&files);
        refresh.connect_clicked(move |_| {
            if let Some(files) = weak.upgrade() {
                files.fill_changes();
            }
        });
        let weak = Rc::downgrade(&files);
        files.changes.connect_row_activated(move |_, row| {
            let Some(files) = weak.upgrade() else { return };
            let path = files.shown_changes.borrow().get(row.index() as usize).cloned();
            if let Some(path) = path.filter(|p| p.is_file()) {
                files.open_file(&path);
            }
        });
        let weak = Rc::downgrade(&files);
        files.tree.connect_activate(move |tree, position| {
            let Some(files) = weak.upgrade() else { return };
            let Some(row) = tree.model().and_then(|m| m.item(position)).and_downcast::<gtk::TreeListRow>() else { return };
            let Some(info) = row.item().and_downcast::<gio::FileInfo>() else { return };
            if info.file_type() == gio::FileType::Directory {
                row.set_expanded(!row.is_expanded());
            } else if let Some(path) = file_of(&info).and_then(|f| f.path()) {
                files.open_file(&path);
            }
        });

        let weak = Rc::downgrade(&files);
        files.list.connect_row_activated(move |_, row| {
            let Some(files) = weak.upgrade() else { return };
            let path = files.shown.borrow().get(row.index() as usize).map(|(path, _)| path.clone());
            if let Some(path) = path {
                files.open_file(&path);
            }
        });
        let weak = Rc::downgrade(&files);
        open_button.connect_clicked(move |_| {
            if let Some(files) = weak.upgrade() {
                files.choose();
            }
        });
        let weak = Rc::downgrade(&files);
        save.connect_clicked(move |_| {
            if let Some(files) = weak.upgrade() {
                files.save();
            }
        });
        let weak = Rc::downgrade(&files);
        in_nevis.connect_clicked(move |_| {
            if let Some(files) = weak.upgrade() {
                files.open_in_nevis();
            }
        });
        let weak = Rc::downgrade(&files);
        close.connect_clicked(move |_| {
            if let Some(files) = weak.upgrade() {
                files.close();
            }
        });
        let dirty = files.dirty.clone();
        files.buffer.connect_modified_changed(move |buffer| dirty.set_visible(buffer.is_modified()));

        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&files);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(files) = weak.upgrade() else { return glib::Propagation::Proceed };
            if modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK) && key.to_lower() == gtk::gdk::Key::s {
                files.save();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        files.view.add_controller(keys);
        files
    }

    /// Shows `entries` (the focused agent's files), where Open… starts, and the project
    /// the tree shows.
    pub fn show(self: &Rc<Self>, base: &Path, project: &Path, entries: &[(PathBuf, Mark)]) {
        *self.base.borrow_mut() = base.to_owned();
        if *self.project.borrow() != project {
            *self.project.borrow_mut() = project.to_owned();
            // Another project: the tree and the repos are its, read again when shown.
            self.tree.set_model(None::<&gtk::SelectionModel>);
            match self.views.visible_child_name().as_deref() {
                Some("project") => self.fill_tree(),
                Some("changes") => self.fill_repos(),
                _ => {}
            }
        }
        *self.shown.borrow_mut() = entries.to_vec();
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        for (path, mark) in entries {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("file-row");
            let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
            names.set_hexpand(true);
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let name = gtk::Label::builder().label(&file).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            let shown_dir = path.parent().map(|dir| relative(dir, base)).unwrap_or_default();
            let dir = gtk::Label::builder().label(&shown_dir).xalign(0.0).ellipsize(pango::EllipsizeMode::Start).build();
            dir.add_css_class("dim");
            dir.add_css_class("small");
            names.append(&name);
            if !shown_dir.is_empty() {
                names.append(&dir);
            }
            let badge = gtk::Label::new(Some(if *mark == Mark::Created { "new" } else { "edited" }));
            badge.add_css_class("badge");
            badge.add_css_class(if *mark == Mark::Created { "new" } else { "edited" });
            row.append(&names);
            row.append(&badge);
            row.set_tooltip_text(Some(&path.to_string_lossy()));
            self.list.append(&row);
        }
        self.empty.set_visible(entries.is_empty());
        self.reload_if_changed();
    }

    pub fn open_file(&self, path: &Path) {
        let path = if path.is_absolute() { path.to_owned() } else { self.base.borrow().join(path) };
        let path = path.as_path();
        if self.open.borrow().as_deref() == Some(path) {
            // Already open: show it as it is (reading it again would drop unsaved typing).
            self.reload_if_changed();
            self.editor_box.set_visible(true);
            self.view.grab_focus();
            return;
        }
        // One file at a time: keep what was typed, and stay if it cannot be kept.
        if self.buffer.is_modified() && !self.save_quietly() {
            return;
        }
        self.load(path);
    }

    /// An agent shows the user a file: never at the cost of their unsaved typing, and it
    /// hears whether it worked.
    pub fn open_for_agent(&self, path: &Path) -> Result<(), String> {
        let current = self.open.borrow().clone();
        if current.as_deref() == Some(path) {
            self.reload_if_changed();
            self.root.set_visible(true);
            self.editor_box.set_visible(true);
            return Ok(());
        }
        if self.buffer.is_modified() {
            return Err("the user has unsaved changes in another file in loom's editor; mention the file instead".into());
        }
        if !self.load(path) {
            return Err(format!("{} could not be opened (not UTF-8 text, or unreadable)", path.display()));
        }
        self.root.set_visible(true);
        Ok(())
    }

    /// Reads `path` into the editor; false if it could not be read as text.
    fn load(&self, path: &Path) -> bool {
        let text = match std::fs::read(path).map(String::from_utf8) {
            Ok(Ok(text)) => text,
            Ok(Err(_)) => {
                self.notice(&format!("{} is not UTF-8 text; open it elsewhere.", path.display()));
                return false;
            }
            Err(err) => {
                self.notice(&format!("Could not open {}: {err}", path.display()));
                return false;
            }
        };
        let languages = sourceview::LanguageManager::default();
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let language = languages.guess_language(file_name.as_deref(), None).or_else(|| {
            // .env and friends: shell syntax colours them well enough.
            file_name.as_deref().filter(|n| n.starts_with(".env")).and_then(|_| languages.language("sh"))
        });
        self.buffer.set_language(language.as_ref());
        self.apply_scheme();
        self.buffer.begin_irreversible_action();
        self.buffer.set_text(&text);
        self.buffer.end_irreversible_action();
        self.buffer.set_modified(false);
        self.buffer.place_cursor(&self.buffer.start_iter());
        self.name.set_label(&path.to_string_lossy());
        self.notice("");
        self.warned.set(None);
        *self.open.borrow_mut() = Some(path.to_owned());
        self.opened_at.set(modified_time(path));
        self.editor_box.set_visible(true);
        self.view.grab_focus();
        true
    }

    /// Light or dark scheme to suit the window.
    fn apply_scheme(&self) {
        let schemes = sourceview::StyleSchemeManager::default();
        let dark = gtk::Settings::default().is_some_and(|s| s.is_gtk_application_prefer_dark_theme())
            || is_dark(&self.view);
        let id = if dark { "Adwaita-dark" } else { "Adwaita" };
        self.buffer.set_style_scheme(schemes.scheme(id).as_ref());
    }

    /// Save (Ctrl+S, the button): writes the buffer out; false if it was not saved. A file
    /// changed on disk since it was opened (the agent rewrote it) is overwritten only by a
    /// second Save of that same version.
    pub fn save(&self) -> bool {
        self.write(true)
    }

    /// Saving on the way to something else (opening another file, closing): never
    /// overwrites a file changed on disk.
    fn save_quietly(&self) -> bool {
        self.write(false)
    }

    fn write(&self, explicit: bool) -> bool {
        let Some(path) = self.open.borrow().clone() else { return true };
        let on_disk = modified_time(&path);
        if on_disk != self.opened_at.get() && !(explicit && on_disk.is_some() && self.warned.get() == on_disk) {
            if explicit {
                self.warned.set(on_disk);
                self.notice("Changed on disk since it was opened. Save again to overwrite it.");
            } else {
                self.notice("Changed on disk since it was opened: Save (twice) to overwrite it, or Close after undoing your edits.");
            }
            return false;
        }
        let text = self.buffer.text(&self.buffer.start_iter(), &self.buffer.end_iter(), false);
        match std::fs::write(&path, text.as_bytes()) {
            Ok(()) => {
                self.buffer.set_modified(false);
                self.opened_at.set(modified_time(&path));
                self.warned.set(None);
                self.notice("");
                true
            }
            Err(err) => {
                self.notice(&format!("Could not save: {err}"));
                false
            }
        }
    }

    /// A line under the editor's header ("" hides it).
    fn notice(&self, text: &str) {
        self.message.set_label(text);
        self.message.set_visible(!text.is_empty());
        if !text.is_empty() {
            self.editor_box.set_visible(true);
        }
    }

    fn close(&self) {
        if self.buffer.is_modified() && !self.save_quietly() {
            return;
        }
        *self.open.borrow_mut() = None;
        self.editor_box.set_visible(false);
    }

    /// The agent may have rewritten the open file: show its new text unless edits are pending.
    pub fn reload_if_changed(&self) {
        let Some(path) = self.open.borrow().clone() else { return };
        if self.buffer.is_modified() || modified_time(&path) == self.opened_at.get() {
            return;
        }
        let window = self.view.root().and_then(|root| root.downcast::<gtk::Window>().ok());
        let focus = window.as_ref().and_then(|w| GtkWindowExt::focus(w));
        let offset = self.buffer.cursor_position();
        if !self.load(&path) {
            return;
        }
        self.buffer.place_cursor(&self.buffer.iter_at_offset(offset));
        // Loading grabbed focus; a reload gives it back (to the chat, say).
        if let Some(widget) = focus {
            widget.grab_focus();
        }
    }

    pub fn choose(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title("Open a file").modal(true).build();
        dialog.set_initial_folder(Some(&gio::File::for_path(&*self.base.borrow())));
        let window = self.root.root().and_then(|root| root.downcast::<gtk::Window>().ok());
        let weak = Rc::downgrade(self);
        dialog.open(window.as_ref(), None::<&gio::Cancellable>, move |result| {
            if let (Some(files), Ok(file)) = (weak.upgrade(), result)
                && let Some(path) = file.path()
            {
                files.open_file(&path);
            }
        });
    }

    /// The project's folders and files, read as folders open (only when shown).
    fn fill_tree(&self) {
        if self.tree.model().is_some() {
            return;
        }
        let root = gio::File::for_path(&*self.project.borrow());
        let model = gtk::TreeListModel::new(folder(&root), false, false, |item| {
            let info = item.downcast_ref::<gio::FileInfo>()?;
            (info.file_type() == gio::FileType::Directory).then(|| file_of(info).map(|f| folder(&f)))?
        });
        self.tree.set_model(Some(&gtk::SingleSelection::new(Some(model))));
    }

    /// The focused agent's repos (latest first): the Changes tab starts on the first.
    pub fn set_repos(self: &Rc<Self>, repos: Vec<PathBuf>) {
        if *self.agent_repos.borrow() == repos {
            return;
        }
        *self.agent_repos.borrow_mut() = repos;
        if self.views.visible_child_name().as_deref() == Some("changes") {
            self.fill_repos();
        }
    }

    /// Offers the agent's repos, then every other repo in the project (found off the main
    /// thread: a big project takes a moment).
    fn fill_repos(self: &Rc<Self>) {
        let round = self.changes_round.get() + 1;
        self.changes_round.set(round);
        let project = self.project.borrow().clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let walked = project.clone();
            let found = gio::spawn_blocking(move || crate::repos::under(&walked)).await.unwrap_or_default();
            let Some(files) = weak.upgrade() else { return };
            if files.changes_round.get() == round {
                files.offer_repos(&project, found);
            }
        });
    }

    fn offer_repos(self: &Rc<Self>, project: &Path, found: Vec<PathBuf>) {
        let mut repos = self.agent_repos.borrow().clone();
        for repo in found {
            if !repos.contains(&repo) {
                repos.push(repo);
            }
        }
        let names: Vec<String> = repos
            .iter()
            .map(|r| match r.strip_prefix(project) {
                Ok(rest) if !rest.as_os_str().is_empty() => rest.to_string_lossy().into_owned(),
                _ => r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            })
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        self.filling.set(true);
        self.repo_pick.set_model(Some(&gtk::StringList::new(&names)));
        self.repo_pick.set_selected(0);
        self.filling.set(false);
        *self.repos.borrow_mut() = repos;
        self.fill_changes();
    }

    fn fill_changes(self: &Rc<Self>) {
        let repo = self.repos.borrow().get(self.repo_pick.selected() as usize).cloned();
        let Some(repo) = repo else {
            while let Some(row) = self.changes.row_at_index(0) {
                self.changes.remove(&row);
            }
            self.repo_line.set_label("No git repository in this project.");
            self.shown_changes.borrow_mut().clear();
            return;
        };
        let round = self.changes_round.get() + 1;
        self.changes_round.set(round);
        self.repo_line.set_label("Reading git status…");
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let reading = repo.clone();
            let status = gio::spawn_blocking(move || crate::repos::status(&reading)).await.ok().flatten();
            let Some(files) = weak.upgrade() else { return };
            if files.changes_round.get() == round {
                files.show_changes_of(&repo, status);
            }
        });
    }

    fn show_changes_of(&self, repo: &Path, status: Option<crate::repos::Repo>) {
        while let Some(row) = self.changes.row_at_index(0) {
            self.changes.remove(&row);
        }
        let Some(status) = status else {
            self.repo_line.set_label("git could not read this repository.");
            self.shown_changes.borrow_mut().clear();
            return;
        };
        let mut line = format!("⎇ {}", status.branch);
        if status.ahead > 0 {
            line.push_str(&format!("   ↑{} to push", status.ahead));
        }
        if status.behind > 0 {
            line.push_str(&format!("   ↓{} to pull", status.behind));
        }
        line.push_str(&if status.changes.is_empty() { "   · clean".to_owned() } else { format!("   · {} changed", status.changes.len()) });
        self.repo_line.set_label(&line);
        let mut paths = Vec::new();
        for (code, file) in &status.changes {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("file-row");
            let badge = gtk::Label::new(Some(code));
            badge.add_css_class("git-code");
            badge.add_css_class(match code.chars().next() {
                Some('?') | Some('A') => "added",
                Some('D') => "deleted",
                _ => "modified",
            });
            let name = gtk::Label::builder().label(file).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::Start).build();
            row.append(&badge);
            row.append(&name);
            self.changes.append(&row);
            paths.push(repo.join(file));
        }
        *self.shown_changes.borrow_mut() = paths;
    }

    /// Switches to the Changes view.
    pub fn show_changes(&self) {
        self.root.set_visible(true);
        self.changes_tab.set_active(true);
    }

    /// Switches to the project tree (devhooks).
    pub fn show_project(&self) {
        self.root.set_visible(true);
        self.project_tab.set_active(true);
    }

    /// Hands the open file to nevis (saved first, so nevis shows what was typed here).
    /// nevis opens it in a window of its own; it stays open here too.
    pub fn open_in_nevis(&self) {
        let Some(path) = self.open.borrow().clone() else { return };
        if self.buffer.is_modified() && !self.save() {
            return;
        }
        if let Err(err) = std::process::Command::new("nevis").arg(&path).spawn() {
            self.notice(&format!("Could not start nevis: {err}"));
        }
    }

    pub fn has_unsaved(&self) -> bool {
        self.buffer.is_modified()
    }
}

/// A folder's entries, folders first then by name, without the ones never worth showing
/// (dotfiles stay: an .env is often the point).
fn folder(dir: &gio::File) -> gio::ListModel {
    let list = gtk::DirectoryList::new(Some("standard::name,standard::display-name,standard::type"), Some(dir));
    let filter = gtk::CustomFilter::new(|item| {
        item.downcast_ref::<gio::FileInfo>()
            .is_some_and(|info| !crate::quickopen::SKIP.contains(&info.name().to_string_lossy().as_ref()))
    });
    let sorter = gtk::CustomSorter::new(|a, b| {
        let (Some(a), Some(b)) = (a.downcast_ref::<gio::FileInfo>(), b.downcast_ref::<gio::FileInfo>()) else {
            return gtk::Ordering::Equal;
        };
        let folder = |i: &gio::FileInfo| i.file_type() != gio::FileType::Directory;
        let key = |i: &gio::FileInfo| (folder(i), i.display_name().to_lowercase());
        key(a).cmp(&key(b)).into()
    });
    let filtered = gtk::FilterListModel::new(Some(list), Some(filter));
    gtk::SortListModel::new(Some(filtered), Some(sorter)).upcast()
}

/// The file a DirectoryList entry stands for.
fn file_of(info: &gio::FileInfo) -> Option<gio::File> {
    info.attribute_object("standard::file").and_downcast::<gio::File>()
}

/// Tree rows: an expander arrow, then the name.
fn tree_factory() -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
        let label = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        let expander = gtk::TreeExpander::builder().child(&label).build();
        item.set_child(Some(&expander));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
        let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else { return };
        let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else { return };
        expander.set_list_row(Some(&row));
        let Some(info) = row.item().and_downcast::<gio::FileInfo>() else { return };
        if let Some(label) = expander.child().and_downcast::<gtk::Label>() {
            label.set_label(&info.display_name());
            if info.file_type() == gio::FileType::Directory {
                label.add_css_class("folder");
            } else {
                label.remove_css_class("folder");
            }
        }
    });
    factory
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn relative(dir: &Path, base: &Path) -> String {
    match dir.strip_prefix(base) {
        Ok(rest) => rest.to_string_lossy().into_owned(),
        Err(_) => dir.to_string_lossy().into_owned(),
    }
}

/// Whether the widget's text colour is light, i.e. the theme is dark.
fn is_dark(widget: &impl IsA<gtk::Widget>) -> bool {
    let fg = widget.as_ref().color();
    0.299 * fg.red() + 0.587 * fg.green() + 0.114 * fg.blue() > 0.5
}
