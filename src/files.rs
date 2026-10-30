//! The files side panel: what the focused agent created or changed (an .env to fill in, a
//! document it wrote) or the whole project as a tree, and an editor to open them in without
//! leaving the app.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use gtk::{gdk, gio, glib, pango, prelude::*};
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
    /// The open file: shown beside the chat (the app places it), not in this panel.
    pub editor_box: gtk::Box,
    /// Markdown read as rendered (Read), or as text (Edit).
    editor_stack: gtk::Stack,
    /// A picture, shown as itself.
    picture: gtk::Picture,
    /// Anything else not text: what it is, and the app that opens it.
    other_label: gtk::Label,
    other_open: gtk::Button,
    /// The open file is shown, not edited (a picture, a PDF…): nothing to save.
    view_only: Cell<bool>,
    reader: gtk::Box,
    read_toggle: gtk::ToggleButton,
    name: gtk::Label,
    /// The open file's full path, under its name (a click copies it).
    path_label: gtk::Label,
    file_icon: gtk::Image,
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
    /// Graph: the commits (hash per row, None for a line of the graph alone).
    graph: gtk::ListBox,
    graph_tab: gtk::ToggleButton,
    /// Notes (the add-on, when on): Scribe's questions, changes to review, what it wrote.
    notes_tab: gtk::ToggleButton,
    /// When the user last clicked, typed or scrolled in this panel.
    touched: Cell<std::time::Instant>,
    graph_commits: RefCell<Vec<Option<String>>>,
    /// Bumped by each graph fill (an older answer is dropped), and the repo shown.
    graph_round: Cell<u64>,
    graph_repo: RefCell<Option<PathBuf>>,
    /// A commit is running (one at a time).
    committing: Cell<bool>,
    repo_bar: gtk::Box,
    repo_pick: gtk::DropDown,
    repo_line: gtk::Label,
    changes: gtk::ListBox,
    /// The repos offered: the agent's (latest first), then the others in the project.
    repos: RefCell<Vec<PathBuf>>,
    agent_repos: RefCell<Vec<PathBuf>>,
    shown_changes: RefCell<Vec<PathBuf>>,
    /// The Changes rows for their menu: (repo, file as git names it, staged, unstaged, can be
    /// reverted).
    change_info: RefCell<Vec<(PathBuf, String, bool, bool, bool)>>,
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
    /// An agent waits for the user to fill a file in (loom_edit): its id, the file, and the
    /// bar over the editor that says so, with Done and Cancel.
    request: RefCell<Option<(u64, PathBuf)>>,
    request_bar: gtk::Box,
    request_text: gtk::Label,
    /// Told when the user finishes (true: Done, saved) or cancels a request.
    pub on_request: RefCell<Option<Rc<dyn Fn(u64, bool)>>>,
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
        let graph_tab = gtk::ToggleButton::builder().label("Graph").group(&agent_tab).tooltip_text("The repo's commits and where each branch is").build();
        let notes_tab = gtk::ToggleButton::builder().label("Notes").group(&agent_tab).tooltip_text("Your notes: Scribe's questions, changes waiting for your OK, what it wrote").visible(false).build();
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabs.add_css_class("segmented");
        for tab in [&agent_tab, &project_tab, &changes_tab, &graph_tab, &notes_tab] {
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
        let open_button = gtk::Button::from_icon_name("folder-open-symbolic");
        open_button.add_css_class("flat");
        open_button.set_tooltip_text(Some("Open the project folder in the file manager (Ctrl+O opens a file here)"));
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
        // Dropped below the last row: into the project's own folder.
        tree.add_controller(drop_into(|| Some(TREE_ROOT.with(|r| r.borrow().clone()))));
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
        pick_line.add_css_class("repo-bar");
        pick_line.set_visible(false);
        root.append(&pick_line);
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
        let graph = gtk::ListBox::new();
        graph.add_css_class("graph");
        graph.set_selection_mode(gtk::SelectionMode::Browse);
        let graph_scroller = gtk::ScrolledWindow::builder().child(&graph).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        views.add_named(&graph_scroller, Some("graph"));
        views.set_size_request(-1, 120);

        let editor_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        editor_box.set_visible(false);
        editor_box.set_vexpand(true);
        // The header, one line: the file (its name over its path) and what can be done with it.
        let editor_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        editor_header.add_css_class("editor-title");
        let file_icon = gtk::Image::new();
        file_icon.add_css_class("file-icon");
        file_icon.set_valign(gtk::Align::Center);
        // Never squeezed out by the buttons: about ten characters at least.
        let name = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).width_chars(10).build();
        name.add_css_class("editor-name");
        let path_label = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::Start).build();
        path_label.add_css_class("editor-path");
        path_label.set_tooltip_text(Some("Click to copy the path"));
        let dirty = gtk::Label::new(Some("●"));
        dirty.add_css_class("dirty");
        dirty.set_visible(false);
        dirty.set_tooltip_text(Some("Unsaved (Ctrl+S)"));
        let name_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        name_line.append(&name);
        name_line.append(&dirty);
        let names = gtk::Box::new(gtk::Orientation::Vertical, 1);
        names.set_hexpand(true);
        names.set_valign(gtk::Align::Center);
        names.append(&name_line);
        names.append(&path_label);
        let button = |label: &str, tip: &str| {
            let button = gtk::Button::with_label(label);
            button.add_css_class("prefs-button");
            button.set_valign(gtk::Align::Center);
            button.set_tooltip_text(Some(tip));
            button
        };
        let save = button("Save", "Save the changes (Ctrl+S)");
        save.set_visible(false);
        let open_app = button("Open", "Open it in the app your system opens it with");
        let ask = button("Ask Claude", "Mention this file (or the selected lines) in the chat");
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.add_css_class("editor-close");
        close.set_valign(gtk::Align::Center);
        close.set_tooltip_text(Some("Close the file"));
        let find = gtk::SearchEntry::builder().placeholder_text("Find").width_chars(14).visible(false).build();
        let read_toggle = gtk::ToggleButton::builder().label("Read").tooltip_text("Markdown as it reads, or as text to edit").visible(false).build();
        read_toggle.add_css_class("prefs-button");
        read_toggle.set_valign(gtk::Align::Center);
        editor_header.append(&file_icon);
        editor_header.append(&names);
        editor_header.append(&find);
        editor_header.append(&save);
        editor_header.append(&read_toggle);
        editor_header.append(&ask);
        editor_header.append(&open_app);
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
        // Markdown, read: rendered as in the chat, in a column of a comfortable width.
        let reader = gtk::Box::new(gtk::Orientation::Vertical, 8);
        reader.add_css_class("reader");
        let reader_clamp = gtk::Box::new(gtk::Orientation::Vertical, 0);
        reader_clamp.set_layout_manager(Some(crate::column::ColumnLayout::new(760)));
        reader_clamp.append(&reader);
        let reader_scroller = gtk::ScrolledWindow::builder().child(&reader_clamp).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        let editor_stack = gtk::Stack::new();
        editor_stack.add_named(&view_scroller, Some("edit"));
        editor_stack.add_named(&reader_scroller, Some("read"));
        // Not text: a picture as itself; anything else said plainly, with the app that opens it.
        let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::Contain).can_shrink(true).build();
        picture.add_css_class("file-picture");
        editor_stack.add_named(&picture, Some("picture"));
        let other = gtk::Box::new(gtk::Orientation::Vertical, 12);
        other.set_valign(gtk::Align::Center);
        other.set_halign(gtk::Align::Center);
        other.add_css_class("file-other");
        let other_label = gtk::Label::builder().wrap(true).justify(gtk::Justification::Center).max_width_chars(40).build();
        other_label.add_css_class("dim");
        let other_open = gtk::Button::with_label("Open");
        other_open.add_css_class("prefs-button");
        other_open.set_halign(gtk::Align::Center);
        other.append(&other_label);
        other.append(&other_open);
        editor_stack.add_named(&other, Some("other"));

        editor_box.set_size_request(-1, 200);
        editor_box.append(&editor_header);
        // An agent waiting for this file to be filled in (loom_edit).
        let request_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        request_bar.add_css_class("edit-request");
        request_bar.set_visible(false);
        let request_text = gtk::Label::builder().xalign(0.0).wrap(true).hexpand(true).build();
        let request_done = gtk::Button::with_label("Done");
        request_done.add_css_class("suggested-action");
        request_done.set_valign(gtk::Align::Center);
        request_done.set_tooltip_text(Some("Save the file and let the agent go on"));
        let request_cancel = gtk::Button::with_label("Cancel");
        request_cancel.add_css_class("prefs-button");
        request_cancel.set_valign(gtk::Align::Center);
        request_bar.append(&request_text);
        request_bar.append(&request_cancel);
        request_bar.append(&request_done);
        editor_box.append(&request_bar);
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tabs.add_css_class("editor-tabs");
        tabs.set_visible(false);
        let tabs_scroller = gtk::ScrolledWindow::builder().child(&tabs).vscrollbar_policy(gtk::PolicyType::Never).hscrollbar_policy(gtk::PolicyType::External).build();
        editor_box.append(&tabs_scroller);
        let message = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
        message.add_css_class("editor-message");
        editor_box.append(&message);
        editor_box.append(&editor_stack);
        editor_box.add_css_class("editor-pane");
        views.set_vexpand(true);
        root.append(&views);

        let files = Rc::new(Files {
            root,
            list,
            empty,
            editor_box,
            name,
            path_label: path_label.clone(),
            file_icon,
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
            change_info: RefCell::default(),
            filling: Cell::new(false),
            changes_round: Cell::new(0),
            tree,
            project: RefCell::default(),
            me: RefCell::default(),
            tabs,
            recent: RefCell::default(),
            on_ask: RefCell::default(),
            request: RefCell::default(),
            request_bar: request_bar.clone(),
            request_text: request_text.clone(),
            on_request: RefCell::default(),
            commit_entry,
            changes_repo: RefCell::default(),
            graph,
            graph_tab: graph_tab.clone(),
            notes_tab: notes_tab.clone(),
            touched: Cell::new(std::time::Instant::now() - std::time::Duration::from_secs(3600)),
            graph_commits: RefCell::default(),
            graph_round: Cell::new(0),
            graph_repo: RefCell::default(),
            committing: Cell::new(false),
            repo_bar: pick_line.clone(),
            editor_stack,
            picture,
            other_label,
            other_open: other_open.clone(),
            view_only: Cell::new(false),
            reader,
            read_toggle,
        });
        let views = files.views.clone();
        agent_tab.connect_toggled(move |tab| {
            if tab.is_active() {
                views.set_visible_child_name("agent");
            }
        });
        let weak = Rc::downgrade(&files);
        request_done.connect_clicked(move |_| {
            if let Some(files) = weak.upgrade() {
                files.finish_request(true);
            }
        });
        let weak = Rc::downgrade(&files);
        request_cancel.connect_clicked(move |_| {
            if let Some(files) = weak.upgrade() {
                files.finish_request(false);
            }
        });
        let views = files.views.clone();
        notes_tab.connect_toggled(move |tab| {
            if tab.is_active() {
                views.set_visible_child_name("notes");
                tab.remove_css_class("news");
            }
        });
        // The user working in the panel: Scribe's news does not switch its tab then.
        let weak = Rc::downgrade(&files);
        let touch = move || {
            if let Some(files) = weak.upgrade() {
                files.touched.set(std::time::Instant::now());
            }
        };
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let touched = touch.clone();
        click.connect_pressed(move |_, _, _, _| touched());
        files.root.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let touched = touch.clone();
        keys.connect_key_pressed(move |_, _, _, _| {
            touched();
            glib::Propagation::Proceed
        });
        files.root.add_controller(keys);
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        scroll.connect_scroll(move |_, _, _| {
            touch();
            glib::Propagation::Proceed
        });
        files.root.add_controller(scroll);
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
            if let Some(files) = weak.upgrade() {
                files.repo_bar.set_visible(tab.is_active() || files.views.visible_child_name().as_deref() == Some("graph"));
                if tab.is_active() {
                    files.views.set_visible_child_name("changes");
                    files.fill_repos();
                }
            }
        });
        let weak = Rc::downgrade(&files);
        graph_tab.connect_toggled(move |tab| {
            if let Some(files) = weak.upgrade() {
                files.repo_bar.set_visible(tab.is_active() || files.changes_tab.is_active());
                if tab.is_active() {
                    files.views.set_visible_child_name("graph");
                    files.fill_repos();
                }
            }
        });
        let weak = Rc::downgrade(&files);
        files.graph.connect_row_activated(move |_, row| {
            let Some(files) = weak.upgrade() else { return };
            let hash = files.graph_commits.borrow().get(row.index() as usize).cloned().flatten();
            if let Some(hash) = hash {
                files.show_commit(&hash);
            }
        });
        let weak = Rc::downgrade(&files);
        files.repo_pick.connect_selected_notify(move |_| {
            if let Some(files) = weak.upgrade()
                && !files.filling.get()
            {
                files.fill_repo_view();
            }
        });
        let weak = Rc::downgrade(&files);
        refresh.connect_clicked(move |_| {
            if let Some(files) = weak.upgrade() {
                files.fill_repo_view();
            }
        });
        // Right-click menus: a file in the tree, in the agent's files, in the Changes.
        let weak = Rc::downgrade(&files);
        crate::menu::on_right_click(&files.tree, move |x, y| {
            let Some(files) = weak.upgrade() else { return };
            let picked = files.tree.pick(x, y, gtk::PickFlags::DEFAULT);
            let expander = picked.and_then(|w| if w.is::<gtk::TreeExpander>() { Some(w) } else { w.ancestor(gtk::TreeExpander::static_type()) });
            let Some(row) = expander.and_downcast::<gtk::TreeExpander>().and_then(|e| e.list_row()) else { return };
            let Some(info) = row.item().and_downcast::<gio::FileInfo>() else { return };
            let Some(path) = file_of(&info).and_then(|f| f.path()) else { return };
            let is_dir = info.file_type() == gio::FileType::Directory;
            let menu = if is_dir {
                let row = row.clone();
                crate::menu::Menu::new().item(if row.is_expanded() { "Fold" } else { "Unfold" }, move || row.set_expanded(!row.is_expanded())).section()
            } else {
                files.open_item(&path)
            };
            files.file_items(menu, &path, is_dir).popup(&files.tree, x, y);
        });
        let weak = Rc::downgrade(&files);
        crate::menu::on_right_click(&files.list, move |x, y| {
            let Some(files) = weak.upgrade() else { return };
            let Some(row) = files.list.row_at_y(y as i32) else { return };
            let path = files.shown.borrow().get(row.index() as usize).map(|(path, _)| path.clone());
            let Some(path) = path else { return };
            let menu = files.open_item(&path);
            files.file_items(menu, &path, false).popup(&files.list, x, y);
        });
        let weak = Rc::downgrade(&files);
        crate::menu::on_right_click(&files.changes, move |x, y| {
            let Some(files) = weak.upgrade() else { return };
            let Some(row) = files.changes.row_at_y(y as i32) else { return };
            let at = row.index() as usize;
            let (Some(path), Some((repo, file, staged, unstaged, revertable))) =
                (files.shown_changes.borrow().get(at).cloned(), files.change_info.borrow().get(at).cloned())
            else {
                return;
            };
            let me = files.me.borrow().clone();
            let git = move |f: Box<dyn Fn(&Files)>| {
                let me = me.clone();
                move || {
                    if let Some(files) = me.upgrade() {
                        f(&files);
                    }
                }
            };
            let (r1, f1, r2, f2, r3, f3, r4, f4) = (repo.clone(), file.clone(), repo.clone(), file.clone(), repo.clone(), file.clone(), repo, file);
            let mut menu = if path.is_file() { files.open_item(&path) } else { crate::menu::Menu::new() };
            menu = menu
                .item("What changed in it", git(Box::new(move |files| files.show_diff(&r1, &f1))))
                .item("Stage", git(Box::new(move |files| files.git_then_refresh(&r2, vec!["add".into(), "--".into(), f2.clone()]))))
                .enabled(unstaged)
                .item("Unstage", git(Box::new(move |files| files.git_then_refresh(&r3, vec!["restore".into(), "--staged".into(), "--".into(), f3.clone()]))))
                .enabled(staged)
                .section();
            if path.exists() {
                menu = files.file_items(menu, &path, path.is_dir());
            }
            menu.section()
                .item("Throw its changes away…", git(Box::new(move |files| files.revert(&r4, &f4))))
                .enabled(revertable)
                .danger()
                .popup(&files.changes, x, y);
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
        open_button.connect_clicked(move |button| {
            if let Some(files) = weak.upgrade() {
                files.open_folder(button);
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
        files.read_toggle.connect_toggled(move |toggle| {
            if let Some(files) = weak.upgrade() {
                files.set_reading(toggle.is_active());
            }
        });
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
        close.connect_clicked(move |_| {
            if let Some(files) = weak.upgrade() {
                files.close();
            }
        });
        let (dirty, unsaved) = (files.dirty.clone(), save.clone());
        files.buffer.connect_modified_changed(move |buffer| {
            dirty.set_visible(buffer.is_modified());
            unsaved.set_visible(buffer.is_modified());
        });
        // Open: in the app the system opens this kind of file with.
        for button in [open_app.clone(), other_open.clone()] {
            let weak = Rc::downgrade(&files);
            button.connect_clicked(move |button| {
                let Some(files) = weak.upgrade() else { return };
                files.open_in_app(button);
            });
        }
        // The path: a click copies it (and says so for a moment).
        let copy = gtk::GestureClick::new();
        let shown = path_label.clone();
        copy.connect_released(move |gesture, _, _, _| {
            let full = shown.label().to_string();
            if full.is_empty() || full == "Copied" {
                return;
            }
            if let Some(widget) = gesture.widget() {
                widget.clipboard().set_text(&full);
            }
            shown.set_label("Copied");
            let back = shown.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(1200), move || back.set_label(&full));
        });
        path_label.add_controller(copy);

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
            // Dragged out like the tree's files (to the chat as @path, or to another app).
            let source = gtk::DragSource::builder().actions(gdk::DragAction::COPY).build();
            let dragged = gio::File::for_path(path);
            source.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&gdk::FileList::from_array(&[dragged.clone()]).to_value())));
            row.add_controller(source);
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

    /// An agent waits for the user to fill `path` in: opened with a bar saying what is asked,
    /// Done and Cancel. Err if another agent already waits for the user, or the file can't
    /// be opened.
    /// The user is filling in a file for an agent now.
    pub fn requesting(&self) -> bool {
        self.request.borrow().is_some()
    }

    pub fn request_edit(&self, id: u64, path: &Path, text: &str) -> Result<(), String> {
        if self.request.borrow().is_some() {
            return Err("the user is filling in another file for an agent; ask again when that is done".into());
        }
        self.open_for_agent(path)?;
        *self.request.borrow_mut() = Some((id, path.to_owned()));
        self.request_text.set_label(text);
        self.request_bar.set_visible(true);
        Ok(())
    }

    /// The request ends: saved and Done (true), or Cancel.
    fn finish_request(&self, done: bool) {
        let Some((id, path)) = self.request.borrow().clone() else { return };
        if done && self.open.borrow().as_deref() == Some(path.as_path()) && self.buffer.is_modified() && !self.save() {
            return;
        }
        self.end_request(id);
        if let Some(told) = self.on_request.borrow().clone() {
            told(id, done);
        }
    }

    /// The bar goes (the request answered, or the agent stopped waiting).
    pub fn end_request(&self, id: u64) {
        let ours = self.request.borrow().as_ref().is_some_and(|(r, _)| *r == id);
        if ours {
            self.request.borrow_mut().take();
            self.request_bar.set_visible(false);
        }
    }

    /// Reads `path` into the editor; false if it could not be read as text.
    fn load(&self, path: &Path) -> bool {
        // The waiting bar shows only over the file it is about.
        let asked = self.request.borrow().as_ref().is_some_and(|(_, p)| p == path);
        self.request_bar.set_visible(asked);
        let text = match std::fs::read(path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(err) => return self.show_other(path, &err.into_bytes()),
            },
            // Can't be read (gone, no permission): said, and nothing else changes.
            Err(err) => {
                self.notice(&format!("Could not open {}: {err}", path.display()));
                return false;
            }
        };
        self.view_only.set(false);
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
        // The name, and under it where it is (a click copies that).
        self.name.set_label(&path.file_name().map_or_else(|| path.to_string_lossy(), |n| n.to_string_lossy()));
        self.path_label.set_label(&path.to_string_lossy());
        self.file_icon.set_from_gicon(&file_icon(path, false));
        self.notice("");
        self.warned.set(None);
        let same_file = self.open.borrow().as_deref() == Some(path);
        *self.open.borrow_mut() = Some(path.to_owned());
        self.opened_at.set(modified_time(path));
        self.remember(path);
        let markdown = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
        self.read_toggle.set_visible(markdown);
        let read = if same_file { markdown && self.read_toggle.is_active() } else { markdown };
        if self.read_toggle.is_active() != read {
            self.read_toggle.set_active(read);
        } else {
            self.set_reading(read);
        }
        self.editor_box.set_visible(true);
        self.view.grab_focus();
        true
    }

    /// A file that is not text: a picture shown as itself, anything else said plainly with
    /// the app that opens it. Its name and path in the header; nothing to edit or save.
    fn show_other(&self, path: &Path, bytes: &[u8]) -> bool {
        if self.buffer.is_modified() && !self.save_quietly() {
            return false;
        }
        let (kind, _) = gio::content_type_guess(Some(path), &bytes[..bytes.len().min(4096)]);
        if kind.starts_with("image/") {
            self.picture.set_filename(Some(path));
            self.editor_stack.set_visible_child_name("picture");
        } else {
            let what = gio::content_type_get_description(&kind);
            let app = gio::AppInfo::default_for_type(&kind, false);
            self.other_label.set_label(&format!("Loom can't show this file ({what})."));
            match &app {
                Some(app) => {
                    self.other_open.set_label(&format!("Open in {}", app.display_name()));
                    self.other_open.set_sensitive(true);
                }
                None => {
                    self.other_open.set_label("No app opens this kind of file");
                    self.other_open.set_sensitive(false);
                }
            }
            self.editor_stack.set_visible_child_name("other");
        }
        self.name.set_label(&path.file_name().map_or_else(|| path.to_string_lossy(), |n| n.to_string_lossy()));
        self.path_label.set_label(&path.to_string_lossy());
        self.file_icon.set_from_gicon(&file_icon(path, false));
        self.notice("");
        self.view_only.set(true);
        self.read_toggle.set_visible(false);
        self.buffer.set_modified(false);
        *self.open.borrow_mut() = Some(path.to_owned());
        self.opened_at.set(modified_time(path));
        self.remember(path);
        self.editor_box.set_visible(true);
        true
    }

    /// A file's first menu items: open it here, or in its own app.
    fn open_item(&self, path: &Path) -> crate::menu::Menu {
        let me = self.me.borrow().clone();
        let (p1, p2, at) = (path.to_owned(), path.to_owned(), self.root.clone());
        crate::menu::Menu::new()
            .item("Open", move || {
                if let Some(files) = me.upgrade() {
                    files.open_file(&p1);
                }
            })
            .item("Open in its app", move || crate::menu::launch(&at, &p2))
            .section()
    }

    /// The items every file and folder has: where it is, its path, the chat, the trash.
    fn file_items(&self, menu: crate::menu::Menu, path: &Path, is_dir: bool) -> crate::menu::Menu {
        let base = self.base.borrow().clone();
        let relative = path.strip_prefix(&base).map(Path::to_path_buf).unwrap_or_else(|_| path.to_owned());
        let at = self.root.clone();
        let (p1, p2, p3) = (path.to_owned(), path.to_owned(), path.to_owned());
        let (a1, a2, a3) = (at.clone(), at.clone(), at.clone());
        let rel = relative.to_string_lossy().into_owned();
        let rel2 = rel.clone();
        let ask = self.on_ask.borrow().clone();
        let me = self.me.borrow().clone();
        let menu = if is_dir {
            menu.item("Open in the file manager", move || crate::menu::launch(&a1, &p1))
        } else {
            menu.item("Show in the file manager", move || crate::menu::reveal(&a1, &p1))
        };
        menu.item("Mention in the chat", move || {
            if let Some(ask) = &ask {
                ask(format!("@{rel} "));
            }
        })
        .section()
        .item("Copy its path", move || crate::menu::copy(&a2, &p2.to_string_lossy()))
        .item("Copy its path in the project", move || crate::menu::copy(&a3, &rel2))
        .section()
        .item("Move to the trash", move || {
            let Some(files) = me.upgrade() else { return };
            let name = p3.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match gio::File::for_path(&p3).trash(None::<&gio::Cancellable>) {
                Ok(()) => {
                    if files.open.borrow().as_deref() == Some(p3.as_path()) {
                        files.close();
                    }
                    files.notice(&format!("{name} is in the trash (restore it from the file manager's Trash)."));
                }
                Err(err) => files.notice(&format!("Could not move {name} to the trash: {err}")),
            }
        })
        .danger()
    }

    /// The project folder in the file manager.
    fn open_folder(&self, button: &gtk::Button) {
        let folder = self.project.borrow().clone();
        if folder.as_os_str().is_empty() {
            return;
        }
        let window = button.root().and_downcast::<gtk::Window>();
        let files = self.me.borrow().clone();
        gtk::FileLauncher::new(Some(&gio::File::for_path(&folder))).launch(window.as_ref(), None::<&gio::Cancellable>, move |result| {
            if let (Err(err), Some(files)) = (result, files.upgrade()) {
                files.notice(&format!("Could not open the folder: {err}"));
            }
        });
    }

    /// The open file in the app the system opens its kind with.
    fn open_in_app(&self, button: &gtk::Button) {
        let Some(path) = self.open.borrow().clone() else { return };
        let window = button.root().and_downcast::<gtk::Window>();
        let files = self.me.borrow().clone();
        gtk::FileLauncher::new(Some(&gio::File::for_path(&path))).launch(window.as_ref(), None::<&gio::Cancellable>, move |result| {
            if let (Err(err), Some(files)) = (result, files.upgrade()) {
                files.notice(&format!("Could not open it: {err}"));
            }
        });
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
        // A picture or the like is shown, not edited: never written over with text.
        if self.view_only.get() {
            return true;
        }
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
                // A file of loom's Claude folder: running sessions read it again.
                if path.starts_with(crate::plugin::dir()) {
                    crate::plugin::changed();
                }
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
        if self.view_only.get() {
            return;
        }
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
        let project = self.project.borrow().clone();
        TREE_ROOT.with(|r| *r.borrow_mut() = project.clone());
        IGNORED.with(|i| i.borrow_mut().clear());
        self.tree.set_model(Some(&tree_model(&project)));
        // What git ignores (target/, node_modules/…) is dimmed: asked off the main thread,
        // then the tree is shown again with it (nothing is open in it yet).
        let (tree, asked) = (self.tree.clone(), project.clone());
        glib::spawn_future_local(async move {
            let ignored = gio::spawn_blocking(move || ignored_in(&asked)).await.unwrap_or_default();
            if ignored.is_empty() || TREE_ROOT.with(|r| *r.borrow() != project) {
                return;
            }
            IGNORED.with(|i| *i.borrow_mut() = ignored);
            dim_ignored(tree.upcast_ref());
        });
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
        self.fill_repo_view();
    }

    /// Changes or Graph, whichever is in view, for the repo picked.
    fn fill_repo_view(self: &Rc<Self>) {
        if self.views.visible_child_name().as_deref() == Some("graph") {
            self.fill_graph();
        } else {
            self.fill_changes();
        }
    }

    /// The repo's commits as a graph (all branches, newest first), with each branch's head.
    fn fill_graph(self: &Rc<Self>) {
        let repo = self.repos.borrow().get(self.repo_pick.selected() as usize).cloned();
        let round = self.graph_round.get() + 1;
        self.graph_round.set(round);
        // The old repo's commits go at once (a click must not open them under the new name).
        self.clear_graph();
        *self.graph_repo.borrow_mut() = None;
        let Some(repo) = repo else { return };
        let loading = gtk::Label::builder().label("Reading the history…").xalign(0.0).build();
        loading.add_css_class("dim");
        loading.set_margin_start(10);
        self.graph.append(&loading);
        self.graph_commits.borrow_mut().push(None);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let reading = repo.clone();
            let found = gio::spawn_blocking(move || {
                let log = Files::git(
                    &reading,
                    &["-c", "core.quotePath=false", "log", "--graph", "--all", "--date-order", "--color=never", "-n", "400", "--format=%x1f%h%x1f%D%x1f%s%x1f%an%x1f%ar"],
                )?;
                let remotes: Vec<String> = Files::git(&reading, &["remote"]).unwrap_or_default().lines().map(str::to_owned).collect();
                Ok::<_, String>((log, remotes))
            })
            .await
            .unwrap_or_else(|_| Err("git did not finish".to_owned()));
            let Some(files) = weak.upgrade() else { return };
            if files.graph_round.get() != round {
                return;
            }
            files.clear_graph();
            *files.graph_repo.borrow_mut() = Some(repo.clone());
            match found {
                Ok((log, remotes)) => files.show_graph(&log, &remotes),
                Err(err) => {
                    let note = gtk::Label::builder().label(format!("git: {err}")).xalign(0.0).wrap(true).build();
                    note.add_css_class("dim");
                    files.graph.append(&note);
                }
            }
        });
    }

    fn show_graph(&self, log: &str, remotes: &[String]) {
        let mut commits = Vec::new();
        for line in log.lines() {
            let (lanes, rest) = line.split_once('\x1f').unwrap_or((line, ""));
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("graph-row");
            let drawn = gtk::Label::builder().use_markup(true).xalign(0.0).build();
            drawn.set_markup(&graph_markup(lanes.trim_end()));
            drawn.add_css_class("graph-lanes");
            row.append(&drawn);
            let fields: Vec<&str> = rest.split('\x1f').collect();
            if let [hash, refs, subject, author, age] = fields[..] {
                let hash_label = gtk::Label::new(Some(hash));
                hash_label.add_css_class("graph-hash");
                row.append(&hash_label);
                for (name, kind) in ref_badges(refs, remotes) {
                    let badge = gtk::Label::new(Some(&name));
                    badge.add_css_class("ref");
                    badge.add_css_class(kind);
                    row.append(&badge);
                }
                let text = gtk::Label::builder().label(subject).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
                row.append(&text);
                // The age, short ("4 min"); who wrote it is in the tooltip.
                let short = age.replace(" minutes", " min").replace(" minute", " min").replace(" hours", " h").replace(" hour", " h").replace(" days", " d").replace(" day", " d").replace(" weeks", " w").replace(" week", " w").replace(" months", " mo").replace(" month", " mo").replace(" years", " y").replace(" year", " y").replace(" ago", "");
                let who = gtk::Label::new(Some(&short));
                who.add_css_class("dim");
                who.add_css_class("small");
                row.append(&who);
                row.set_tooltip_text(Some(&format!("{hash} {subject}\n{author}, {age}")));
                commits.push(Some(hash.to_owned()));
            } else {
                commits.push(None);
            }
            let list_row = gtk::ListBoxRow::builder().child(&row).activatable(!rest.is_empty()).selectable(!rest.is_empty()).build();
            self.graph.append(&list_row);
        }
        *self.graph_commits.borrow_mut() = commits;
    }

    fn clear_graph(&self) {
        while let Some(row) = self.graph.row_at_index(0) {
            self.graph.remove(&row);
        }
        self.graph_commits.borrow_mut().clear();
    }

    /// A commit in full (message, files, changes), in a window.
    fn show_commit(&self, hash: &str) {
        // The repo the graph shows (the picker may have moved on meanwhile).
        let Some(repo) = self.graph_repo.borrow().clone() else { return };
        let (me, hash) = (self.me.borrow().clone(), hash.to_owned());
        glib::spawn_future_local(async move {
            let showing = hash.clone();
            let text = gio::spawn_blocking(move || Files::git(&repo, &["-c", "core.quotePath=false", "show", "--stat", "--patch", "--format=fuller", &showing]).unwrap_or_else(|e| e))
                .await
                .unwrap_or_default();
            if let Some(files) = me.upgrade() {
                files.diff_window(&format!("commit {hash}"), &text);
            }
        });
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
        let mut info = Vec::new();
        *self.changes_repo.borrow_mut() = Some(repo.to_owned());
        for (raw, file) in &status.changes {
            let code = raw.trim();
            // git's two columns: the index (staged) and the work tree (not yet).
            let staged = !raw.starts_with(' ') && !raw.starts_with('?');
            let unstaged = raw.ends_with(|c: char| c != ' ');
            // Not new files (nothing to go back to) nor renames (the old name too would be
            // involved): those are for git itself.
            let revertable = raw != "??" && !raw.starts_with('R') && !raw.starts_with('C');
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
            if unstaged {
                let (r, p) = (repo.to_owned(), file.clone());
                add("stage", "Put its changes in the next commit", Box::new(move |files| files.git_then_refresh(&r, vec!["add".into(), "--".into(), p.clone()])));
            }
            if staged {
                let (r, p) = (repo.to_owned(), file.clone());
                add("unstage", "Leave it out of the next commit", Box::new(move |files| files.git_then_refresh(&r, vec!["restore".into(), "--staged".into(), "--".into(), p.clone()])));
            }
            if revertable {
                let (r, p) = (repo.to_owned(), file.clone());
                add("revert", "Throw its changes away (asks first)", Box::new(move |files| files.revert(&r, &p)));
            }
            row.append(&actions);
            self.changes.append(&row);
            paths.push(repo.join(file));
            info.push((repo.to_owned(), file.clone(), staged, unstaged, revertable));
        }
        *self.shown_changes.borrow_mut() = paths;
        *self.change_info.borrow_mut() = info;
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

    /// Runs git in the background (a commit hook or a signing prompt may take a while), then
    /// shows the Changes again, or what went wrong.
    fn git_then_refresh(&self, repo: &Path, args: Vec<String>) {
        self.git_then(repo, args, |_, _| {});
    }

    /// Runs git in the background, then `done` (with whether it worked) and, if it did, the
    /// Changes again.
    fn git_then(&self, repo: &Path, args: Vec<String>, done: impl FnOnce(&Files, bool) + 'static) {
        let me = self.me.borrow().clone();
        let repo = repo.to_owned();
        self.repo_line.set_label("git…");
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                Files::git(&repo, &args)
            })
            .await
            .unwrap_or_else(|_| Err("git did not finish".to_owned()));
            let Some(files) = me.upgrade() else { return };
            if let Err(err) = result {
                files.repo_line.set_label(&format!("git: {}", err.lines().last().unwrap_or(&err)));
                done(&files, false);
                return;
            }
            done(&files, true);
            files.fill_changes();
        });
    }

    /// A file's changes against the last commit (a new file: all of it), in a window.
    fn show_diff(&self, repo: &Path, file: &str) {
        let (me, repo, file) = (self.me.borrow().clone(), repo.to_owned(), file.to_owned());
        glib::spawn_future_local(async move {
            let (reading, name) = (repo.clone(), file.clone());
            let diff = gio::spawn_blocking(move || {
                Files::git(&reading, &["-c", "core.quotePath=false", "diff", "HEAD", "--", &name])
                    .ok()
                    .filter(|d| !d.trim().is_empty())
                    .or_else(|| {
                        // Untracked: the whole file is new.
                        std::fs::read_to_string(reading.join(&name)).ok().map(|text| text.lines().map(|l| format!("+{l}\n")).collect())
                    })
                    .unwrap_or_else(|| "No changes to show.".to_owned())
            })
            .await
            .unwrap_or_default();
            if let Some(files) = me.upgrade() {
                files.diff_window(&format!("{file} — changes"), &diff);
            }
        });
    }

    fn diff_window(&self, file: &str, diff: &str) {
        let buffer = sourceview::Buffer::new(None);
        buffer.set_language(sourceview::LanguageManager::default().language("diff").as_ref());
        buffer.set_text(diff);
        if let Some(scheme) = self.buffer.style_scheme() {
            buffer.set_style_scheme(Some(&scheme));
        }
        let view = sourceview::View::with_buffer(&buffer);
        view.set_editable(false);
        view.set_monospace(true);
        view.add_css_class("editor");
        let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
        let window = gtk::Window::builder().title(file.to_owned()).default_width(900).default_height(640).child(&scroller).build();
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
                files.git_then_refresh(&repo, vec!["restore".into(), "--staged".into(), "--worktree".into(), "--source=HEAD".into(), "--".into(), file.clone()]);
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
        // One commit at a time; the message goes once it is in (and only if not retyped).
        if self.committing.replace(true) {
            return;
        }
        self.commit_entry.set_sensitive(false);
        let sent = message.clone();
        self.git_then(&repo, vec!["commit".into(), "-m".into(), message], move |files, worked| {
            files.committing.set(false);
            files.commit_entry.set_sensitive(true);
            if worked && files.commit_entry.text().trim() == sent {
                files.commit_entry.set_text("");
            }
        });
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

    /// The editor as a side left it: `path` open (shown or not), else nothing open. A file
    /// that cannot be opened any more is not shown instead of another.
    pub fn restore(&self, path: Option<&Path>, shown: bool) {
        if let Some(path) = path {
            self.open_file(path);
        }
        if path.is_none() || self.open.borrow().as_deref() != path {
            self.close();
        } else if !shown {
            self.editor_box.set_visible(false);
        }
    }

    /// The editor's tabs become `tabs` (the Notes view keeps its own); the ones it had back.
    pub fn swap_tabs(&self, tabs: Vec<PathBuf>) -> Vec<PathBuf> {
        let old = self.recent.replace(tabs);
        self.show_tabs();
        old
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
        if self.read_toggle.is_active() {
            let selected = selected_text(self.reader.upcast_ref());
            let text = match selected {
                Some(quote) => format!("@{}\n> {}\n", shown.display(), quote.trim().replace('\n', "\n> ")),
                None => format!("@{} ", shown.display()),
            };
            callback(text);
            return;
        }
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

    /// Markdown shown rendered (`read`) or as text to edit.
    fn set_reading(&self, read: bool) {
        self.read_toggle.set_label(if read { "Edit" } else { "Read" });
        if read {
            self.render_reader();
            self.editor_stack.set_visible_child_name("read");
        } else {
            self.editor_stack.set_visible_child_name("edit");
            self.view.grab_focus();
        }
    }

    /// The file's text, rendered as in the chat (headings, lists, tables, code).
    fn render_reader(&self) {
        while let Some(child) = self.reader.first_child() {
            self.reader.remove(&child);
        }
        let text = self.buffer.text(&self.buffer.start_iter(), &self.buffer.end_iter(), false);
        for segment in crate::markdown::segments(&text) {
            match segment {
                crate::markdown::Segment::Text(markup) => self.reader.append(&crate::chat::text_label(&markup)),
                crate::markdown::Segment::Code { code, .. } => self.reader.append(&crate::chat::code_label(&code)),
                crate::markdown::Segment::Table(rows) => self.reader.append(&crate::chat::table_view(&rows)),
                crate::markdown::Segment::Rule => self.reader.append(&crate::chat::rule_view()),
            }
        }
    }

    /// The open file (links in its Read view resolve from its folder).
    pub fn open_path(&self) -> Option<PathBuf> {
        self.open.borrow().clone()
    }

    /// Switches to the Changes view.
    pub fn show_changes(&self) {
        self.root.set_visible(true);
        self.changes_tab.set_active(true);
    }

    /// Switches to the commit graph (devhooks).
    pub fn show_graph_view(&self) {
        self.root.set_visible(true);
        self.graph_tab.set_active(true);
    }

    /// Switches to the project tree (devhooks).
    pub fn show_project(&self) {
        self.root.set_visible(true);
        self.project_tab.set_active(true);
    }

    /// The Notes tab's content (the add-on's panel), and whether the tab is offered.
    pub fn add_notes(&self, panel: &gtk::Box) {
        self.views.add_named(panel, Some("notes"));
    }

    /// Notes' panel goes to the Notes view (the tab left, if it showed)…
    pub fn lend_notes(&self, panel: &gtk::Box) {
        if self.notes_tab.is_active() {
            self.project_tab.set_active(true);
        }
        self.views.remove(panel);
    }

    /// … and comes back.
    pub fn take_back_notes(&self, panel: &gtk::Box) {
        self.views.add_named(panel, Some("notes"));
    }

    pub fn set_notes_available(&self, on: bool) {
        self.notes_tab.set_visible(on);
        if !on && self.notes_tab.is_active() {
            self.project_tab.set_active(true);
        }
    }

    pub fn show_notes(&self) {
        self.notes_tab.set_active(true);
    }

    /// Something new in Notes: its tab marked (until it is looked at).
    pub fn set_notes_news(&self, news: bool) {
        if news && !self.notes_tab.is_active() {
            self.notes_tab.add_css_class("news");
        } else {
            self.notes_tab.remove_css_class("news");
        }
    }

    /// The user is working in this panel now (focus in it, or a click, key or scroll in it
    /// in the last little while).
    pub fn in_use(&self) -> bool {
        let focused = self.root.root().and_then(|r| r.focus()).is_some_and(|f| f.is_ancestor(&self.root));
        focused || self.touched.get().elapsed() < std::time::Duration::from_secs(20)
    }

    /// Hands the open file to the other editor of the settings, or the system's default app
    /// (saved first, so it shows what was typed here). It stays open here too.
    pub fn open_elsewhere(&self) {
        let Some(path) = self.open.borrow().clone() else { return };
        if self.buffer.is_modified() && !self.save() {
            return;
        }
        let command = crate::settings::get().editor;
        let mut words = command.split_whitespace();
        let Some(program) = words.next() else {
            let files = self.me.borrow().clone();
            let window = self.view.root().and_downcast::<gtk::Window>();
            gtk::FileLauncher::new(Some(&gio::File::for_path(&path))).launch(window.as_ref(), None::<&gio::Cancellable>, move |result| {
                if let (Err(err), Some(files)) = (result, files.upgrade()) {
                    files.notice(&format!("Could not open it: {err}"));
                }
            });
            return;
        };
        if let Err(err) = std::process::Command::new(program).args(words).arg(&path).spawn() {
            self.notice(&format!("Could not start {program}: {err}"));
        }
    }

    /// Ctrl+F in the editor: its find field, matches marked as typed, Enter for the next.
    pub fn find(&self) -> bool {
        if !self.editor_box.is_visible() {
            return false;
        }
        if self.read_toggle.is_active() {
            self.read_toggle.set_active(false);
        }
        let Some(entry) = self.find_entry() else { return false };
        entry.set_visible(true);
        entry.grab_focus();
        true
    }

    /// The editor's find field (in its header rows).
    fn find_entry(&self) -> Option<gtk::SearchEntry> {
        let mut row = self.editor_box.first_child();
        while let Some(line) = row {
            let mut child = line.first_child();
            while let Some(widget) = child {
                if let Ok(entry) = widget.clone().downcast::<gtk::SearchEntry>() {
                    return Some(entry);
                }
                child = widget.next_sibling();
            }
            row = line.next_sibling();
        }
        None
    }

    /// The cursor on line `line` (from 1) of the open file, in view.
    pub fn go_to_line(&self, line: u32) {
        if self.read_toggle.is_active() {
            self.read_toggle.set_active(false);
        }
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

thread_local! {
    /// The folder the tree shows.
    static TREE_ROOT: std::cell::RefCell<PathBuf> = std::cell::RefCell::default();
    /// What git ignores in the tree's project (files, and folders as a whole).
    static IGNORED: std::cell::RefCell<std::collections::HashSet<PathBuf>> = std::cell::RefCell::default();
}

/// A tree row's file, and whether it is a folder.
fn entry_of(item: &gtk::ListItem) -> Option<(gio::File, bool)> {
    let info = item.item().and_downcast::<gtk::TreeListRow>()?.item().and_downcast::<gio::FileInfo>()?;
    Some((file_of(&info)?, info.file_type() == gio::FileType::Directory))
}

/// Files dropped on the tree go into the folder `into` gives: moved if they come from the
/// tree's own project, copied from anywhere else. A name already there gets a number.
fn drop_into(into: impl Fn() -> Option<PathBuf> + 'static) -> gtk::DropTarget {
    let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY | gdk::DragAction::MOVE);
    target.connect_drop(move |_, value, _, _| {
        let (Ok(files), Some(dir)) = (value.get::<gdk::FileList>(), into()) else { return false };
        let root = TREE_ROOT.with(|r| r.borrow().clone());
        for file in files.files() {
            let Some(path) = file.path() else { continue };
            // Onto itself, or into a folder inside itself: nothing to do.
            if path.parent() == Some(dir.as_path()) || dir.starts_with(&path) {
                continue;
            }
            let Some(name) = path.file_name() else { continue };
            let destination = free_name(&dir.join(name));
            let moved = !root.as_os_str().is_empty() && path.starts_with(&root);
            let result = if moved { std::fs::rename(&path, &destination) } else { copy_all(&path, &destination) };
            if let Err(err) = result {
                crate::log::write("files", &format!("could not {} {} to {}: {err}", if moved { "move" } else { "copy" }, path.display(), destination.display()));
            }
        }
        true
    });
    target
}

/// `wanted`, or "name (2).ext", "name (3).ext"… if it is taken.
pub fn free_name(wanted: &Path) -> PathBuf {
    if !wanted.exists() {
        return wanted.to_owned();
    }
    let stem = wanted.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = wanted.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..1000).map(|n| wanted.with_file_name(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap_or_else(|| wanted.to_owned())
}

/// A file, or a folder with all it holds.
fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// The file a DirectoryList entry stands for.
fn file_of(info: &gio::FileInfo) -> Option<gio::File> {
    info.attribute_object("standard::file").and_downcast::<gio::File>()
}

/// The tree of `project`, read as folders open.
fn tree_model(project: &Path) -> gtk::SingleSelection {
    let model = gtk::TreeListModel::new(folder(&gio::File::for_path(project)), false, false, |item| {
        let info = item.downcast_ref::<gio::FileInfo>()?;
        (info.file_type() == gio::FileType::Directory).then(|| file_of(info).map(|f| folder(&f)))?
    });
    gtk::SingleSelection::new(Some(model))
}

/// The files and folders git ignores in `project` (none if it is no repository).
fn ignored_in(project: &Path) -> std::collections::HashSet<PathBuf> {
    let Ok(out) = std::process::Command::new("git").arg("-C").arg(project).args(["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"]).output() else {
        return Default::default();
    };
    if !out.status.success() {
        return Default::default();
    }
    String::from_utf8_lossy(&out.stdout).split('\0').filter(|p| !p.is_empty()).map(|p| project.join(p.trim_end_matches('/'))).collect()
}

/// The tree's rows shown now, dimmed where git ignores them (the rows made later are, as
/// they are made).
fn dim_ignored(widget: &gtk::Widget) {
    if let Some(expander) = widget.downcast_ref::<gtk::TreeExpander>() {
        let info = expander.list_row().and_then(|r| r.item()).and_downcast::<gio::FileInfo>();
        if let (Some(info), Some(line)) = (info, expander.child()) {
            let ignored = file_of(&info).and_then(|f| f.path()).is_some_and(|p| is_ignored(&p));
            if ignored {
                line.add_css_class("ignored");
                line.set_tooltip_text(Some("Ignored by git"));
            }
        }
        return;
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        dim_ignored(&w);
        child = w.next_sibling();
    }
}

/// Whether git ignores `path` (or a folder it is in).
fn is_ignored(path: &Path) -> bool {
    IGNORED.with(|ignored| {
        let ignored = ignored.borrow();
        !ignored.is_empty() && path.ancestors().any(|p| ignored.contains(p))
    })
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
        // Dragged out: the file itself (to the chat, as @path; to a file manager or editor).
        let source = gtk::DragSource::builder().actions(gdk::DragAction::COPY | gdk::DragAction::MOVE).build();
        let held = item.downgrade();
        source.connect_prepare(move |_, _, _| {
            let file = held.upgrade().and_then(|i| entry_of(&i)).map(|(file, _)| file)?;
            Some(gdk::ContentProvider::for_value(&gdk::FileList::from_array(&[file]).to_value()))
        });
        expander.add_controller(source);
        // Dropped on: into that folder, or the folder of that file.
        let held = item.downgrade();
        expander.add_controller(drop_into(move || {
            let (file, is_dir) = held.upgrade().and_then(|i| entry_of(&i))?;
            let path = file.path()?;
            if is_dir { Some(path) } else { path.parent().map(Path::to_owned) }
        }));
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
        if file_of(&info).and_then(|f| f.path()).is_some_and(|p| is_ignored(&p)) {
            line.add_css_class("ignored");
            line.set_tooltip_text(Some("Ignored by git"));
        } else {
            line.remove_css_class("ignored");
            line.set_tooltip_text(None);
        }
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

/// The lanes of `git log --graph` in colour: each column its own, commits as dots.
fn graph_markup(lanes: &str) -> String {
    const COLOURS: &[&str] = &["#7aa2f7", "#9ece6a", "#e0af68", "#f7768e", "#bb9af7", "#7dcfff", "#ff9e64"];
    let mut out = String::new();
    for (i, c) in lanes.chars().enumerate() {
        let colour = COLOURS[(i / 2) % COLOURS.len()];
        let shown = match c {
            '*' => "●".to_owned(),
            '|' => "│".to_owned(),
            ' ' => {
                out.push(' ');
                continue;
            }
            other => glib::markup_escape_text(&other.to_string()).to_string(),
        };
        out.push_str(&format!("<span foreground=\"{colour}\">{shown}</span>"));
    }
    out
}

/// A commit's refs as badges: (name, kind) with kind head / branch / remote / tag.
fn ref_badges(refs: &str, remotes: &[String]) -> Vec<(String, &'static str)> {
    let mut badges = Vec::new();
    for part in refs.split(", ").map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(branch) = part.strip_prefix("HEAD -> ") {
            badges.push(("HEAD".to_owned(), "head"));
            badges.push((branch.to_owned(), "branch"));
        } else if part == "HEAD" {
            badges.push(("HEAD (detached)".to_owned(), "head"));
        } else if let Some(tag) = part.strip_prefix("tag: ") {
            badges.push((tag.to_owned(), "tag"));
        } else if remotes.iter().any(|r| part.starts_with(&format!("{r}/"))) {
            badges.push((part.to_owned(), "remote"));
        } else {
            badges.push((part.to_owned(), "branch"));
        }
    }
    badges
}

/// The text selected in a label under `widget` (the Read view), if any.
pub fn selected_text(widget: &gtk::Widget) -> Option<String> {
    if let Some(label) = widget.downcast_ref::<gtk::Label>()
        && let Some((a, b)) = label.selection_bounds()
        && a != b
    {
        let (a, b) = (a.min(b) as usize, a.max(b) as usize);
        return Some(label.text().chars().skip(a).take(b - a).collect());
    }
    let mut child = widget.first_child();
    while let Some(next) = child {
        if let Some(text) = selected_text(&next) {
            return Some(text);
        }
        child = next.next_sibling();
    }
    None
}

/// A file's icon by its kind (from its name), or a folder's.
fn file_icon(path: &Path, is_dir: bool) -> gio::Icon {
    crate::icons::for_file(path, is_dir)
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

#[cfg(test)]
mod tests {
    #[test]
    fn refs_read_as_badges() {
        let remotes = vec!["origin".to_owned()];
        assert_eq!(
            super::ref_badges("HEAD -> main, origin/main, tag: v1.0, feature/x", &remotes),
            vec![
                ("HEAD".to_owned(), "head"),
                ("main".to_owned(), "branch"),
                ("origin/main".to_owned(), "remote"),
                ("v1.0".to_owned(), "tag"),
                ("feature/x".to_owned(), "branch"),
            ]
        );
    }
}

#[cfg(test)]
mod drop_tests {
    use super::*;

    #[test]
    fn a_dropped_folder_is_copied_whole_and_names_never_clash() {
        let base = std::env::temp_dir().join(format!("loom-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src/inner")).unwrap();
        std::fs::write(base.join("src/a.txt"), "a").unwrap();
        std::fs::write(base.join("src/inner/b.txt"), "b").unwrap();
        std::fs::create_dir_all(base.join("into")).unwrap();
        copy_all(&base.join("src"), &free_name(&base.join("into/src"))).unwrap();
        assert_eq!(std::fs::read_to_string(base.join("into/src/inner/b.txt")).unwrap(), "b");
        // Again: a second copy beside the first, not over it.
        let second = free_name(&base.join("into/src"));
        assert_eq!(second, base.join("into/src (2)"));
        std::fs::write(base.join("into/note.md"), "x").unwrap();
        assert_eq!(free_name(&base.join("into/note.md")), base.join("into/note (2).md"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
