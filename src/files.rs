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
    /// Itself, for the buttons made later (rows of Changes).
    me: RefCell<std::rc::Weak<Files>>,
    /// Files opened lately, as tabs over the editor.
    tabs: gtk::Box,
    recent: RefCell<Vec<PathBuf>>,
    /// "Ask Claude" about the file or the selection: the text for the chat's input.
    pub on_ask: RefCell<Option<Rc<dyn Fn(String)>>>,
    /// Committing in the repo the Changes view shows.
    commit_entry: gtk::Entry,
    changes_repo: RefCell<Option<PathBuf>>,
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
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabs.add_css_class("segmented");
        for tab in [&agent_tab, &project_tab, &changes_tab] {
            tab.add_css_class("flat");
            tab.add_css_class("tab");
            // The theme styles .text-button.toggle groups its own way (the first one tinted
            // whatever is chosen): these are drawn as one segmented control instead.
            tab.remove_css_class("text-button");
            tab.remove_css_class("toggle");
            tabs.append(tab);
        }
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let open_button = gtk::Button::from_icon_name("document-open-symbolic");
        open_button.add_css_class("flat");
        open_button.set_tooltip_text(Some("Open any file (Ctrl+O; Ctrl+P to find one by name)"));
        header.append(&tabs);
        header.append(&spacer);
        header.append(&open_button);
        root.append(&header);

        let list = gtk::ListBox::new();
        list.add_css_class("file-list");
        let empty = gtk::Label::builder()
            .label("Files the agent writes or edits show up here, to read or fill in without leaving loom.")
            .wrap(true)
            .justify(gtk::Justification::Center)
            .max_width_chars(28)
            .build();
        empty.add_css_class("dim");
        empty.add_css_class("files-empty");
        let list_scroller = gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        let agent_view = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let caption = gtk::Label::builder().label("Written by this agent").xalign(0.0).build();
        caption.add_css_class("panel-caption");
        agent_view.append(&caption);
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
        let commit_entry = gtk::Entry::builder().placeholder_text("Commit message (commits what is staged)").hexpand(true).build();
        let commit = gtk::Button::with_label("Commit");
        commit.add_css_class("suggested");
        let commit_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        commit_line.add_css_class("commit-line");
        commit_line.append(&commit_entry);
        commit_line.append(&commit);
        changes_view.append(&commit_line);
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
        let find = gtk::SearchEntry::builder().placeholder_text("Find").width_chars(14).visible(false).build();
        editor_header.append(&name);
        editor_header.append(&find);
        editor_header.append(&dirty);
        let ask = gtk::Button::with_label("Ask Claude");
        ask.add_css_class("flat");
        ask.set_tooltip_text(Some("Mention this file (or the selected lines) in the chat"));
        editor_header.append(&ask);
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
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tabs.add_css_class("editor-tabs");
        tabs.set_visible(false);
        let tabs_scroller = gtk::ScrolledWindow::builder().child(&tabs).vscrollbar_policy(gtk::PolicyType::Never).hscrollbar_policy(gtk::PolicyType::External).build();
        editor_box.append(&tabs_scroller);
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
            me: RefCell::default(),
            tabs,
            recent: RefCell::default(),
            on_ask: RefCell::default(),
            commit_entry,
            changes_repo: RefCell::default(),
        });
        let views = files.views.clone();
        agent_tab.connect_toggled(move |tab| {
            if tab.is_active() {
                views.set_visible_child_name("agent");
            }
        });
        // Find in the open file: every match marked, Enter goes to the next.
        if let Some(entry) = files.find_entry() {
            let settings = sourceview::SearchSettings::new();
            settings.set_wrap_around(true);
            let context = sourceview::SearchContext::new(&files.buffer, Some(&settings));
            context.set_highlight(true);
            entry.connect_search_changed(move |entry| settings.set_search_text(Some(entry.text().as_str()).filter(|t| !t.is_empty())));
            let (weak, found) = (Rc::downgrade(&files), context.clone());
            entry.connect_activate(move |_| {
                let Some(files) = weak.upgrade() else { return };
                let buffer = &files.buffer;
                let from = buffer.selection_bounds().map_or_else(|| buffer.iter_at_mark(&buffer.get_insert()), |(_, end)| end);
                if let Some((mut start, end, _)) = found.forward(&from) {
                    buffer.select_range(&start, &end);
                    files.view.scroll_to_iter(&mut start, 0.2, false, 0.0, 0.0);
                }
            });
            let weak = Rc::downgrade(&files);
            entry.connect_stop_search(move |entry| {
                entry.set_text("");
                entry.set_visible(false);
                if let Some(files) = weak.upgrade() {
                    files.view.grab_focus();
                }
            });
            // Kept alive with the entry.
            unsafe { entry.set_data("loom-search", context) };
        }
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
        *files.me.borrow_mut() = Rc::downgrade(&files);
        let weak = Rc::downgrade(&files);
        ask.connect_clicked(move |_| {
            if let Some(files) = weak.upgrade() {
                files.ask();
            }
        });
        let weak = Rc::downgrade(&files);
        commit.connect_clicked(move |_| {
            if let Some(files) = weak.upgrade() {
                files.commit();
            }
        });
        let weak = Rc::downgrade(&files);
        files.commit_entry.connect_activate(move |_| {
            if let Some(files) = weak.upgrade() {
                files.commit();
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
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            row.add_css_class("file-row");
            let icon = gtk::Image::from_gicon(&file_icon(path, false));
            icon.add_css_class("file-icon");
            icon.set_valign(gtk::Align::Center);
            row.append(&icon);
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
            badge.set_valign(gtk::Align::Center);
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
        self.remember(path);
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
        let mut parts = vec![format!("⎇ {}", status.branch)];
        if status.ahead > 0 {
            parts.push(format!("↑{} to push", status.ahead));
        }
        if status.behind > 0 {
            parts.push(format!("↓{} to pull", status.behind));
        }
        parts.push(if status.changes.is_empty() { "clean".to_owned() } else { format!("{} changed", status.changes.len()) });
        self.repo_line.set_label(&parts.join("  ·  "));
        let mut paths = Vec::new();
        *self.changes_repo.borrow_mut() = Some(repo.to_owned());
        for (raw, file) in &status.changes {
            let code = raw.trim();
            let staged = !raw.starts_with(' ') && !raw.starts_with('?');
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("file-row");
            row.add_css_class("change-row");
            let badge = gtk::Label::new(Some(code));
            if staged {
                badge.add_css_class("staged");
                badge.set_tooltip_text(Some("Staged: goes into the next commit"));
            }
            badge.add_css_class("git-code");
            badge.add_css_class(match code.chars().next() {
                Some('?') | Some('A') => "added",
                Some('D') => "deleted",
                _ => "modified",
            });
            let name = gtk::Label::builder().label(file).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::Start).build();
            row.append(&badge);
            row.append(&name);
            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            actions.add_css_class("change-actions");
            let add = |label: &str, tip: &str, act: Box<dyn Fn(&Files)>| {
                let button = gtk::Button::with_label(label);
                button.add_css_class("flat");
                button.add_css_class("small");
                button.set_tooltip_text(Some(tip));
                let me = self.me.borrow().clone();
                button.connect_clicked(move |_| {
                    if let Some(files) = me.upgrade() {
                        act(&files);
                    }
                });
                actions.append(&button);
            };
            let (r, p) = (repo.to_owned(), file.clone());
            add("diff", "What changed in it", Box::new(move |files| files.show_diff(&r, &p)));
            let (r, p) = (repo.to_owned(), file.clone());
            if staged {
                add("unstage", "Leave it out of the next commit", Box::new(move |files| files.git_then_refresh(&r, &["restore", "--staged", "--", &p])));
            } else {
                add("stage", "Put it in the next commit", Box::new(move |files| files.git_then_refresh(&r, &["add", "--", &p])));
            }
            if raw != "??" {
                let (r, p) = (repo.to_owned(), file.clone());
                add("revert", "Throw its changes away (asks first)", Box::new(move |files| files.revert(&r, &p)));
            }
            row.append(&actions);
            self.changes.append(&row);
            paths.push(repo.join(file));
        }
        *self.shown_changes.borrow_mut() = paths;
    }

    /// Runs git in `repo`: its output, or what it said went wrong.
    fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
        let output = std::process::Command::new("git").arg("-C").arg(repo).args(args).output().map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    }

    fn git_then_refresh(&self, repo: &Path, args: &[&str]) {
        if let Err(err) = Files::git(repo, args) {
            self.repo_line.set_label(&format!("git: {err}"));
            return;
        }
        if let Some(files) = self.me.borrow().upgrade() {
            files.fill_changes();
        }
    }

    /// A file's changes against the last commit (a new file: all of it), in a window.
    fn show_diff(&self, repo: &Path, file: &str) {
        let diff = Files::git(repo, &["-c", "core.quotePath=false", "diff", "HEAD", "--", file])
            .ok()
            .filter(|d| !d.trim().is_empty())
            .or_else(|| {
                // Untracked: the whole file is new.
                std::fs::read_to_string(repo.join(file)).ok().map(|text| text.lines().map(|l| format!("+{l}\n")).collect())
            })
            .unwrap_or_else(|| "No changes to show.".to_owned());
        let buffer = sourceview::Buffer::new(None);
        buffer.set_language(sourceview::LanguageManager::default().language("diff").as_ref());
        buffer.set_text(&diff);
        if let Some(scheme) = self.buffer.style_scheme() {
            buffer.set_style_scheme(Some(&scheme));
        }
        let view = sourceview::View::with_buffer(&buffer);
        view.set_editable(false);
        view.set_monospace(true);
        view.add_css_class("editor");
        let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
        let window = gtk::Window::builder().title(format!("{file} — changes")).default_width(900).default_height(640).child(&scroller).build();
        if let Some(parent) = self.root.root().and_downcast::<gtk::Window>() {
            window.set_transient_for(Some(&parent));
        }
        let keys = gtk::EventControllerKey::new();
        let weak = window.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape
                && let Some(window) = weak.upgrade()
            {
                window.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(keys);
        window.present();
    }

    /// Throws a file's changes away, after asking.
    fn revert(&self, repo: &Path, file: &str) {
        let dialog = gtk::AlertDialog::builder()
            .message(format!("Throw away the changes to {file}?"))
            .detail("It goes back to how it was at the last commit. This can't be undone.")
            .buttons(["Keep them", "Throw away"])
            .cancel_button(0)
            .default_button(0)
            .build();
        let (me, repo, file) = (self.me.borrow().clone(), repo.to_owned(), file.to_owned());
        let parent = self.root.root().and_downcast::<gtk::Window>();
        dialog.choose(parent.as_ref(), None::<&gio::Cancellable>, move |choice| {
            if let (Ok(1), Some(files)) = (choice, me.upgrade()) {
                files.git_then_refresh(&repo, &["restore", "--staged", "--worktree", "--source=HEAD", "--", &file]);
            }
        });
    }

    /// Commits what is staged in the repo shown, with the message typed.
    fn commit(&self) {
        let Some(repo) = self.changes_repo.borrow().clone() else { return };
        let message = self.commit_entry.text().trim().to_owned();
        if message.is_empty() {
            self.commit_entry.grab_focus();
            self.repo_line.set_label("Type a commit message first.");
            return;
        }
        match Files::git(&repo, &["commit", "-m", &message]) {
            Ok(_) => {
                self.commit_entry.set_text("");
                if let Some(files) = self.me.borrow().upgrade() {
                    files.fill_changes();
                }
            }
            Err(err) => self.repo_line.set_label(&format!("git: {}", err.lines().last().unwrap_or(&err))),
        }
    }

    /// The file opened goes first in the tabs (eight at most).
    fn remember(&self, path: &Path) {
        {
            let mut recent = self.recent.borrow_mut();
            recent.retain(|p| p != path);
            recent.insert(0, path.to_owned());
            recent.truncate(8);
        }
        self.show_tabs();
    }

    fn show_tabs(&self) {
        while let Some(child) = self.tabs.first_child() {
            self.tabs.remove(&child);
        }
        let open = self.open.borrow().clone();
        let recent = self.recent.borrow().clone();
        for path in &recent {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let tab = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            tab.add_css_class("editor-tab");
            if open.as_deref() == Some(path.as_path()) {
                tab.add_css_class("current");
            }
            let go = gtk::Button::with_label(&name);
            go.add_css_class("flat");
            go.set_tooltip_text(Some(&path.to_string_lossy()));
            let close = gtk::Button::with_label("×");
            close.add_css_class("flat");
            close.set_tooltip_text(Some("Take it off the tabs"));
            let (me, target) = (self.me.borrow().clone(), path.clone());
            go.connect_clicked(move |_| {
                if let Some(files) = me.upgrade() {
                    files.open_file(&target);
                }
            });
            let (me, target) = (self.me.borrow().clone(), path.clone());
            close.connect_clicked(move |_| {
                if let Some(files) = me.upgrade() {
                    files.recent.borrow_mut().retain(|p| *p != target);
                    files.show_tabs();
                }
            });
            tab.append(&go);
            tab.append(&close);
            self.tabs.append(&tab);
        }
        self.tabs.set_visible(recent.len() > 1);
    }

    /// "Ask Claude": the file, or the lines selected (quoted), into the chat's input.
    fn ask(&self) {
        let Some(path) = self.open.borrow().clone() else { return };
        let callback = self.on_ask.borrow().clone();
        let Some(callback) = callback else { return };
        let shown = path.strip_prefix(self.base.borrow().as_path()).map(Path::to_path_buf).unwrap_or(path.clone());
        let text = match self.buffer.selection_bounds() {
            Some((start, end)) => {
                let (from, to) = (start.line() + 1, end.line() + 1);
                let lines = if from == to { format!("#L{from}") } else { format!("#L{from}-{to}") };
                let selected = self.buffer.text(&start, &end, false);
                format!("@{}{lines}\n```\n{}\n```\n", shown.display(), selected.trim_end())
            }
            None => format!("@{} ", shown.display()),
        };
        callback(text);
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

    /// Ctrl+F in the editor: its find field, matches marked as typed, Enter for the next.
    pub fn find(&self) -> bool {
        if !self.editor_box.is_visible() {
            return false;
        }
        let Some(entry) = self.find_entry() else { return false };
        entry.set_visible(true);
        entry.grab_focus();
        true
    }

    fn find_entry(&self) -> Option<gtk::SearchEntry> {
        let header = self.editor_box.first_child()?;
        let mut child = header.first_child();
        while let Some(widget) = child {
            if let Ok(entry) = widget.clone().downcast::<gtk::SearchEntry>() {
                return Some(entry);
            }
            child = widget.next_sibling();
        }
        None
    }

    /// The cursor on line `line` (from 1) of the open file, in view.
    pub fn go_to_line(&self, line: u32) {
        let Some(mut iter) = self.buffer.iter_at_line(line.saturating_sub(1) as i32) else { return };
        self.buffer.place_cursor(&iter);
        self.view.scroll_to_iter(&mut iter, 0.2, true, 0.0, 0.3);
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
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let icon = gtk::Image::new();
        icon.add_css_class("file-icon");
        let label = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::End).hexpand(true).build();
        line.append(&icon);
        line.append(&label);
        let expander = gtk::TreeExpander::builder().child(&line).indent_for_icon(true).build();
        item.set_child(Some(&expander));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
        let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else { return };
        let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else { return };
        expander.set_list_row(Some(&row));
        let Some(info) = row.item().and_downcast::<gio::FileInfo>() else { return };
        let Some(line) = expander.child().and_downcast::<gtk::Box>() else { return };
        let is_dir = info.file_type() == gio::FileType::Directory;
        if let Some(icon) = line.first_child().and_downcast::<gtk::Image>() {
            icon.set_from_gicon(&file_icon(Path::new(info.name().as_path()), is_dir));
        }
        if let Some(label) = line.last_child().and_downcast::<gtk::Label>() {
            label.set_label(&info.display_name());
            if is_dir {
                label.add_css_class("folder");
            } else {
                label.remove_css_class("folder");
            }
        }
    });
    factory
}

/// The theme's symbolic icon for a file's kind (from its name), or a folder's.
fn file_icon(path: &Path, is_dir: bool) -> gio::Icon {
    if is_dir {
        return gio::ThemedIcon::new("folder-symbolic").upcast();
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (kind, _) = gio::content_type_guess(Some(name.as_str()), None);
    gio::content_type_get_symbolic_icon(&kind)
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
