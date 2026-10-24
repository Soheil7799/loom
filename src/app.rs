//! The window: agents down the left (grouped by project), the focused agent's chat in the
//! middle, its files on the right, the project's job tray along the bottom, and the
//! scratchpad and the pickers floating over it all.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::{gdk, gio, glib, pango, prelude::*};

use crate::agent::{Agent, Event, Status};
use crate::files::Files;
use crate::jobs::Tray;
use crate::scratch::Scratch;
use crate::state::{self, Saved};

/// Idle agents that nobody looks at stop after this long (LOOM_IDLE_MINUTES overrides).
const IDLE_MINUTES: u64 = 15;

pub struct App {
    app: gtk::Application,
    app_id: String,
    pub window: gtk::ApplicationWindow,
    agents: RefCell<Vec<Rc<Agent>>>,
    focused: RefCell<Option<Rc<Agent>>>,
    next_id: Cell<u32>,
    sidebar: gtk::Box,
    list: gtk::ListBox,
    rows: RefCell<Vec<Option<Rc<Agent>>>>,
    inbox: gtk::MenuButton,
    inbox_list: gtk::Box,
    chat_stack: gtk::Stack,
    /// The second agent shown beside the focused one (Ctrl+\), and where it sits.
    pinned: RefCell<Option<Rc<Agent>>>,
    pinned_box: gtk::Box,
    previous: RefCell<Option<Rc<Agent>>>,
    dock_stack: gtk::Stack,
    bar_stack: gtk::Stack,
    trays: RefCell<HashMap<PathBuf, Rc<Tray>>>,
    scratch: Rc<Scratch>,
    files: Rc<Files>,
    switcher: gtk::Popover,
    switcher_entry: gtk::SearchEntry,
    switcher_list: gtk::ListBox,
    run: gtk::Popover,
    run_entry: gtk::Entry,
    help: gtk::Popover,
    quitting: Cell<bool>,
}

impl App {
    pub fn new(app: &gtk::Application) -> Rc<App> {
        let app_id = app.application_id().map(|id| id.to_string()).unwrap_or_default();
        let window = gtk::ApplicationWindow::builder().application(app).title("loom").default_width(1320).default_height(840).build();
        window.add_css_class("loom");

        // Left: the agents.
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar.add_css_class("sidebar");
        sidebar.set_size_request(250, -1);
        // Children that expand must not widen it.
        sidebar.set_hexpand(false);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        top.add_css_class("sidebar-top");
        let name = gtk::Label::builder().label("loom").xalign(0.0).hexpand(true).build();
        name.add_css_class("brand");
        let inbox_list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        inbox_list.add_css_class("inbox");
        let inbox = gtk::MenuButton::builder()
            .label("0")
            .popover(&gtk::Popover::builder().child(&inbox_list).build())
            .tooltip_text("Questions waiting (Ctrl+I)")
            .visible(false)
            .build();
        inbox.add_css_class("inbox-button");
        inbox.add_css_class("flat");
        top.append(&name);
        top.append(&inbox);
        sidebar.append(&top);
        let list = gtk::ListBox::new();
        list.add_css_class("agents");
        let list_scroller = gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        sidebar.append(&list_scroller);
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bottom.add_css_class("sidebar-bottom");
        let new_button = gtk::Button::with_label("+  New agent");
        new_button.add_css_class("flat");
        new_button.set_hexpand(true);
        new_button.set_tooltip_text(Some("Ctrl+N here · Ctrl+Shift+N in another folder"));
        let help_button = gtk::MenuButton::builder().label("?").tooltip_text("Keys (Ctrl+/)").build();
        help_button.add_css_class("flat");
        bottom.append(&new_button);
        bottom.append(&help_button);
        sidebar.append(&bottom);

        // Middle: chat over the dock, the tray at the bottom.
        let chat_stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).transition_duration(90).vexpand(true).build();
        chat_stack.add_named(&empty_page(), Some("empty"));
        let dock_stack = gtk::Stack::new();
        let bar_stack = gtk::Stack::new();
        bar_stack.add_css_class("tray-stack");
        let pinned_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pinned_box.add_css_class("pinned");
        pinned_box.set_visible(false);
        let chats = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&chat_stack)
            .end_child(&pinned_box)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .build();
        let work = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&chats)
            .end_child(&dock_stack)
            .resize_end_child(false)
            .shrink_end_child(false)
            .vexpand(true)
            .build();
        let middle = gtk::Box::new(gtk::Orientation::Vertical, 0);
        middle.append(&work);
        middle.append(&bar_stack);

        // Right: files.
        let files = Files::new();
        files.root.set_size_request(380, -1);
        files.root.set_visible(false);
        let split = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&middle)
            .end_child(&files.root)
            .resize_end_child(false)
            .shrink_end_child(false)
            .hexpand(true)
            .build();

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.append(&sidebar);
        body.append(&split);
        let overlay = gtk::Overlay::builder().child(&body).build();
        let scratch = Scratch::new();
        overlay.add_overlay(&scratch.revealer);
        window.set_child(Some(&overlay));

        // Pickers.
        let switcher_entry = gtk::SearchEntry::builder().placeholder_text("Go to agent or project…").build();
        let switcher_list = gtk::ListBox::new();
        switcher_list.add_css_class("switcher-list");
        let switcher_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        switcher_box.set_size_request(460, -1);
        switcher_box.append(&switcher_entry);
        switcher_box.append(&switcher_list);
        let switcher = gtk::Popover::builder().child(&switcher_box).has_arrow(false).position(gtk::PositionType::Bottom).build();
        switcher.set_parent(&chat_stack);
        switcher.set_pointing_to(Some(&gdk::Rectangle::new(0, 0, 1, 1)));
        let run_entry = gtk::Entry::builder().placeholder_text("Command to run here (name: command names the chip)").width_chars(56).build();
        run_entry.add_css_class("mono");
        let run = gtk::Popover::builder().child(&run_entry).position(gtk::PositionType::Top).build();
        run.set_parent(&bar_stack);
        let help = gtk::Popover::builder().child(&help_text()).build();
        help_button.set_popover(Some(&help));

        let this = Rc::new(App {
            app: app.clone(),
            app_id,
            window,
            agents: RefCell::default(),
            focused: RefCell::default(),
            next_id: Cell::new(1),
            sidebar,
            list,
            rows: RefCell::default(),
            inbox,
            inbox_list,
            chat_stack,
            pinned: RefCell::default(),
            pinned_box,
            previous: RefCell::default(),
            dock_stack,
            bar_stack,
            trays: RefCell::default(),
            scratch,
            files,
            switcher,
            switcher_entry,
            switcher_list,
            run,
            run_entry,
            help,
            quitting: Cell::new(false),
        });
        this.wire(&new_button);
        this.restore();
        this
    }

    fn wire(self: &Rc<Self>, new_button: &gtk::Button) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            let Some(app) = weak.upgrade() else { return };
            let agent = app.rows.borrow().get(row.index() as usize).cloned().flatten();
            if let Some(agent) = agent {
                app.focus(&agent);
            }
        });
        let weak = Rc::downgrade(self);
        new_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.new_agent(false);
            }
        });
        let weak = Rc::downgrade(self);
        self.inbox.popover().unwrap().connect_show(move |_| {
            if let Some(app) = weak.upgrade() {
                app.refresh_inbox();
            }
        });

        let weak = Rc::downgrade(self);
        self.switcher_entry.connect_search_changed(move |_| {
            if let Some(app) = weak.upgrade() {
                app.fill_switcher();
            }
        });
        let weak = Rc::downgrade(self);
        self.switcher_entry.connect_activate(move |_| {
            if let Some(app) = weak.upgrade() {
                let row = app.switcher_list.selected_row().or_else(|| app.switcher_list.row_at_index(0));
                if let Some(row) = row {
                    row.activate();
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        let list = self.switcher_list.clone();
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
        self.switcher_entry.add_controller(keys);

        let weak = Rc::downgrade(self);
        self.run_entry.connect_activate(move |entry| {
            let Some(app) = weak.upgrade() else { return };
            let line = entry.text().trim().to_owned();
            app.run.popdown();
            entry.set_text("");
            if !line.is_empty() {
                app.current_tray().run(&line);
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, keycode, modifiers| {
            weak.upgrade().map_or(glib::Propagation::Proceed, |app| app.key(key, keycode, modifiers))
        });
        self.window.add_controller(keys);

        let focus_action = gio::SimpleAction::new("focus-agent", Some(glib::VariantTy::UINT32));
        let weak = Rc::downgrade(self);
        focus_action.connect_activate(move |_, target| {
            let Some(app) = weak.upgrade() else { return };
            let Some(id) = target.and_then(|t| t.get::<u32>()) else { return };
            let agent = app.agents.borrow().iter().find(|a| a.id == id).cloned();
            if let Some(agent) = agent {
                app.window.present();
                app.focus(&agent);
            }
        });
        self.app.add_action(&focus_action);

        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            let Some(app) = weak.upgrade() else { return glib::Propagation::Proceed };
            app.close_request()
        });

        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(60, move || {
            let Some(app) = weak.upgrade() else { return glib::ControlFlow::Break };
            app.suspend_idle();
            glib::ControlFlow::Continue
        });
    }

    // ---- keys -----------------------------------------------------------------------------

    fn key(self: &Rc<Self>, key: gdk::Key, keycode: u32, modifiers: gdk::ModifierType) -> glib::Propagation {
        let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
        if !ctrl && !alt {
            return glib::Propagation::Proceed;
        }
        // The key on the first layout, so shortcuts work with the Persian one active too.
        let base = WidgetExt::display(&self.window)
            .translate_key(keycode, gdk::ModifierType::empty(), 0)
            .map_or(key, |(k, ..)| k)
            .to_lower();
        let in_terminal = GtkWindowExt::focus(&self.window).is_some_and(|w| w.is::<vte::Terminal>() || w.ancestor(vte::Terminal::static_type()).is_some());

        match (ctrl, shift, alt, base) {
            (true, false, false, gdk::Key::grave) => self.toggle_scratch(),
            (false, false, true, gdk::Key::Up) => self.step(-1),
            (false, false, true, gdk::Key::Down) => self.step(1),
            // Everything else belongs to a focused terminal (Ctrl+W, Ctrl+K… mean something there).
            _ if in_terminal => return glib::Propagation::Proceed,
            (true, false, false, gdk::Key::n) => self.new_agent(false),
            (true, true, false, gdk::Key::n) => self.new_agent(true),
            (true, false, false, gdk::Key::w) => self.close_focused(),
            (true, false, false, gdk::Key::k) => self.open_switcher(),
            (true, false, false, gdk::Key::i) => {
                if self.inbox.is_visible() {
                    self.inbox.popup();
                }
            }
            (true, false, false, gdk::Key::j) => {
                self.run.popup();
                self.run_entry.grab_focus();
            }
            (true, true, false, gdk::Key::j) => self.current_tray().peek_latest(),
            (true, true, false, gdk::Key::x) => self.current_tray().clear_finished(),
            (true, false, false, gdk::Key::b) => self.sidebar.set_visible(!self.sidebar.is_visible()),
            (true, false, false, gdk::Key::e) => self.files.root.set_visible(!self.files.root.is_visible()),
            (true, false, false, gdk::Key::o) => {
                self.files.root.set_visible(true);
                self.files.choose();
            }
            (true, false, false, gdk::Key::l) => self.focus_input(),
            (true, false, false, gdk::Key::backslash) => self.toggle_split(),
            (true, false, false, gdk::Key::slash) => self.help.popup(),
            (true, false, false, k) if k.to_unicode().and_then(|c| c.to_digit(10)).is_some_and(|d| d >= 1) => {
                let index = k.to_unicode().and_then(|c| c.to_digit(10)).unwrap_or(1) as usize - 1;
                let agent = self.agents_in_order().get(index).cloned();
                if let Some(agent) = agent {
                    self.focus(&agent);
                }
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    // ---- for devhooks ----------------------------------------------------------------------

    pub fn focused_agent(&self) -> Option<Rc<Agent>> {
        self.focused.borrow().clone()
    }

    pub fn focus_nth(self: &Rc<Self>, index: usize) {
        let agent = self.agents_in_order().get(index).cloned();
        if let Some(agent) = agent {
            self.focus(&agent);
        }
    }

    pub fn start_in(self: &Rc<Self>, cwd: PathBuf) {
        self.start_agent_in(cwd);
    }

    /// What a key does, by name.
    pub fn act(self: &Rc<Self>, name: &str, argument: &str) {
        match name {
            "scratch" => self.toggle_scratch(),
            "files" => self.files.root.set_visible(!self.files.root.is_visible()),
            "switcher" => self.open_switcher(),
            "inbox" => self.inbox.popup(),
            "help" => self.help.popup(),
            "peek" => self.current_tray().peek_latest(),
            "split" => self.toggle_split(),
            "open" => {
                self.files.root.set_visible(true);
                self.files.open_file(Path::new(argument));
            }
            "mode" => {
                if let Some(agent) = self.focused_agent() {
                    agent.cycle_mode();
                }
            }
            "run" => self.current_tray().run(argument),
            other => eprintln!("loom control: unknown command {other}"),
        }
    }

    fn toggle_scratch(self: &Rc<Self>) {
        let project = self.focused.borrow().as_ref().map(|a| project_of(&a.cwd)).unwrap_or_else(glib::home_dir);
        if !self.scratch.toggle(&project) {
            self.focus_input();
        }
    }

    fn focus_input(&self) {
        if let Some(agent) = self.focused.borrow().as_ref() {
            agent.chat.input.grab_focus();
        }
    }

    // ---- agents ---------------------------------------------------------------------------

    fn restore(self: &Rc<Self>) {
        let (saved, focused) = state::load(&self.app_id);
        for saved in saved {
            self.add_agent(saved);
        }
        let agents = self.agents.borrow().clone();
        if let Some(agent) = agents.get(focused).or(agents.first()) {
            self.focus(agent);
        }
        self.refresh_sidebar();
    }

    fn add_agent(self: &Rc<Self>, saved: Saved) -> Rc<Agent> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let tray = self.tray_for(&project_of(&saved.cwd));
        let agent = Agent::new(id, saved, tray);
        self.chat_stack.add_named(&agent.chat.root, Some(&format!("agent-{id}")));
        let weak = Rc::downgrade(self);
        agent.on_event(move |agent, event| {
            if let Some(app) = weak.upgrade() {
                app.agent_event(agent, event);
            }
        });
        self.agents.borrow_mut().push(agent.clone());
        agent
    }

    fn tray_for(&self, project: &Path) -> Rc<Tray> {
        if let Some(tray) = self.trays.borrow().get(project) {
            return tray.clone();
        }
        let tray = Tray::new(project.to_owned());
        let name = project.to_string_lossy();
        self.dock_stack.add_named(&tray.dock, Some(&name));
        self.bar_stack.add_named(&tray.bar, Some(&name));
        // The dock pane shows only while something is pinned in the project in view.
        let dock_stack = self.dock_stack.clone();
        tray.dock.connect_visible_notify(move |dock| {
            if dock_stack.visible_child().as_ref() == Some(dock.upcast_ref()) {
                dock_stack.set_visible(dock.is_visible());
            }
        });
        self.trays.borrow_mut().insert(project.to_owned(), tray.clone());
        tray
    }

    /// The tray of the project in view (home's when no agent is open).
    fn current_tray(&self) -> Rc<Tray> {
        let project = self.focused.borrow().as_ref().map(|a| project_of(&a.cwd)).unwrap_or_else(glib::home_dir);
        let tray = self.tray_for(&project);
        self.show_tray(&project, &tray);
        tray
    }

    fn show_tray(&self, project: &Path, tray: &Rc<Tray>) {
        let name = project.to_string_lossy();
        self.dock_stack.set_visible_child_name(&name);
        self.bar_stack.set_visible_child_name(&name);
        self.dock_stack.set_visible(tray.dock.is_visible());
    }

    pub fn focus(self: &Rc<Self>, agent: &Rc<Agent>) {
        let old = self.focused.borrow().clone();
        if let Some(old) = old.filter(|old| !Rc::ptr_eq(old, agent)) {
            old.focused.set(false);
            *self.previous.borrow_mut() = Some(old);
        }
        agent.focused.set(true);
        agent.unseen.set(false);
        agent.last_active.set(std::time::Instant::now());
        *self.focused.borrow_mut() = Some(agent.clone());
        if !self.is_pinned(agent) {
            self.chat_stack.set_visible_child(&agent.chat.root);
        }
        let project = project_of(&agent.cwd);
        self.show_tray(&project, &agent.tray);
        self.files.show(&agent.cwd, &agent.files());
        if agent.status() == Status::Suspended {
            agent.start();
        }
        agent.chat.input.grab_focus();
        self.refresh_sidebar();
        self.save();
    }

    fn is_pinned(&self, agent: &Rc<Agent>) -> bool {
        self.pinned.borrow().as_ref().is_some_and(|p| Rc::ptr_eq(p, agent))
    }

    /// Shows the previous agent beside the focused one, or puts it back.
    fn toggle_split(self: &Rc<Self>) {
        let pinned = self.pinned.borrow_mut().take();
        if let Some(agent) = pinned {
            self.pinned_box.remove(&agent.chat.root);
            self.pinned_box.set_visible(false);
            self.chat_stack.add_named(&agent.chat.root, Some(&format!("agent-{}", agent.id)));
            if agent.focused.get() {
                self.chat_stack.set_visible_child(&agent.chat.root);
            }
            return;
        }
        let focused = self.focused.borrow().clone();
        let candidate = self.previous.borrow().clone().filter(|p| !focused.as_ref().is_some_and(|f| Rc::ptr_eq(f, p)));
        let candidate = candidate.or_else(|| {
            // No previous one: the next agent in the sidebar.
            let order = self.agents_in_order();
            let at = focused.as_ref().and_then(|f| order.iter().position(|a| Rc::ptr_eq(a, f)))?;
            let focused = focused.as_ref()?;
            order.get((at + 1) % order.len()).cloned().filter(|a| !Rc::ptr_eq(a, focused))
        });
        let Some(agent) = candidate else { return };
        self.chat_stack.remove(&agent.chat.root);
        self.pinned_box.append(&agent.chat.root);
        self.pinned_box.set_visible(true);
        if agent.status() == Status::Suspended {
            agent.start();
        }
        *self.pinned.borrow_mut() = Some(agent);
    }

    fn step(self: &Rc<Self>, by: i32) {
        let agents = self.agents_in_order();
        if agents.is_empty() {
            return;
        }
        let current = self.focused.borrow().as_ref().and_then(|f| agents.iter().position(|a| Rc::ptr_eq(a, f))).unwrap_or(0) as i32;
        let next = (current + by).rem_euclid(agents.len() as i32) as usize;
        self.focus(&agents[next]);
    }

    /// Agents in sidebar order: grouped by project, in order of first appearance.
    fn agents_in_order(&self) -> Vec<Rc<Agent>> {
        let agents = self.agents.borrow();
        let mut projects: Vec<PathBuf> = Vec::new();
        for agent in agents.iter() {
            let project = project_of(&agent.cwd);
            if !projects.contains(&project) {
                projects.push(project);
            }
        }
        projects.iter().flat_map(|p| agents.iter().filter(move |a| project_of(&a.cwd) == *p).cloned()).collect()
    }

    fn new_agent(self: &Rc<Self>, choose: bool) {
        let here = self.focused.borrow().as_ref().map(|a| a.cwd.clone());
        match here {
            Some(cwd) if !choose => self.start_agent_in(cwd),
            _ => {
                let dialog = gtk::FileDialog::builder().title("Start an agent in…").modal(true).build();
                if let Some(cwd) = here {
                    dialog.set_initial_folder(Some(&gio::File::for_path(project_of(&cwd).parent().unwrap_or(&cwd))));
                }
                let weak = Rc::downgrade(self);
                dialog.select_folder(Some(&self.window), None::<&gio::Cancellable>, move |result| {
                    if let (Some(app), Ok(folder)) = (weak.upgrade(), result)
                        && let Some(path) = folder.path()
                    {
                        app.start_agent_in(path);
                    }
                });
            }
        }
    }

    fn start_agent_in(self: &Rc<Self>, cwd: PathBuf) {
        let agent = self.add_agent(Saved { cwd, session_id: None, title: String::new() });
        self.focus(&agent);
    }

    fn close_focused(self: &Rc<Self>) {
        let Some(agent) = self.focused.borrow().clone() else { return };
        if !agent.is_busy() {
            self.remove_agent(&agent);
            return;
        }
        let dialog = gtk::AlertDialog::builder()
            .message(format!("“{}” is working", agent.title()))
            .detail("Closing stops what it is doing. The conversation itself is kept by Claude Code.")
            .buttons(["Keep it", "Close"])
            .cancel_button(0)
            .default_button(0)
            .build();
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), None::<&gio::Cancellable>, move |choice| {
            if let (Some(app), Ok(1)) = (weak.upgrade(), choice) {
                app.remove_agent(&agent);
            }
        });
    }

    fn remove_agent(self: &Rc<Self>, agent: &Rc<Agent>) {
        let order = self.agents_in_order();
        let position = order.iter().position(|a| Rc::ptr_eq(a, agent)).unwrap_or(0);
        if self.is_pinned(agent) {
            self.toggle_split();
        }
        if self.previous.borrow().as_ref().is_some_and(|p| Rc::ptr_eq(p, agent)) {
            *self.previous.borrow_mut() = None;
        }
        agent.stop();
        self.chat_stack.remove(&agent.chat.root);
        self.agents.borrow_mut().retain(|a| !Rc::ptr_eq(a, agent));
        let was_focused = self.focused.borrow().as_ref().is_some_and(|f| Rc::ptr_eq(f, agent));
        if was_focused {
            *self.focused.borrow_mut() = None;
            let order = self.agents_in_order();
            match order.get(position.min(order.len().saturating_sub(1))) {
                Some(next) => self.focus(next),
                None => {
                    self.chat_stack.set_visible_child_name("empty");
                    self.files.show(&glib::home_dir(), &[]);
                }
            }
        }
        self.refresh_sidebar();
        self.save();
    }

    fn agent_event(self: &Rc<Self>, agent: &Rc<Agent>, event: Event) {
        let looking = self.window.is_active() && agent.focused.get();
        match event {
            Event::Changed => {
                self.refresh_sidebar();
                self.refresh_inbox();
            }
            Event::Session => self.save(),
            Event::Files => {
                if agent.focused.get() {
                    self.files.show(&agent.cwd, &agent.files());
                    if !self.files.root.is_visible() && !agent.files().is_empty() {
                        self.files.root.set_visible(true);
                    }
                }
            }
            Event::Asked(title) => {
                self.refresh_inbox();
                if !looking {
                    self.notify(agent, &format!("Wants to: {title}"));
                }
            }
            Event::Done => {
                if !looking {
                    self.notify(agent, "Finished");
                }
                self.refresh_sidebar();
            }
        }
    }

    fn notify(&self, agent: &Agent, body: &str) {
        let notification = gio::Notification::new(&agent.title());
        notification.set_body(Some(body));
        notification.set_default_action_and_target_value("app.focus-agent", Some(&agent.id.to_variant()));
        self.app.send_notification(Some(&format!("agent-{}", agent.id)), &notification);
    }

    fn suspend_idle(&self) {
        let minutes = std::env::var("LOOM_IDLE_MINUTES").ok().and_then(|m| m.parse().ok()).unwrap_or(IDLE_MINUTES);
        let limit = Duration::from_secs(minutes * 60);
        let agents = self.agents.borrow().clone();
        for agent in agents {
            if agent.status() == Status::Idle && !agent.focused.get() && agent.last_active.get().elapsed() > limit {
                agent.suspend();
            }
        }
    }

    fn save(&self) {
        let order = self.agents_in_order();
        let focused = self.focused.borrow().as_ref().and_then(|f| order.iter().position(|a| Rc::ptr_eq(a, f))).unwrap_or(0);
        let saved: Vec<Saved> = order.iter().map(|a| a.saved()).collect();
        state::save(&self.app_id, &saved, focused);
    }

    fn close_request(self: &Rc<Self>) -> glib::Propagation {
        let busy: Vec<String> = self.agents.borrow().iter().filter(|a| a.is_busy()).map(|a| a.title()).collect();
        if !self.quitting.get() && (!busy.is_empty() || self.files.has_unsaved()) {
            let what = if busy.is_empty() {
                "A file has unsaved changes".to_owned()
            } else {
                format!("{} agent{} working: {}", busy.len(), if busy.len() == 1 { " is" } else { "s are" }, busy.join(", "))
            };
            let dialog = gtk::AlertDialog::builder()
                .message("Quit loom?")
                .detail(format!("{what}. Quitting stops them; conversations are kept and resume next time."))
                .buttons(["Cancel", "Quit"])
                .cancel_button(0)
                .default_button(0)
                .build();
            let weak = Rc::downgrade(self);
            dialog.choose(Some(&self.window), None::<&gio::Cancellable>, move |choice| {
                if let (Some(app), Ok(1)) = (weak.upgrade(), choice) {
                    app.quitting.set(true);
                    app.window.close();
                }
            });
            return glib::Propagation::Stop;
        }
        self.save();
        for agent in self.agents.borrow().iter() {
            agent.stop();
        }
        for tray in self.trays.borrow().values() {
            tray.stop_all();
        }
        glib::Propagation::Proceed
    }

    // ---- sidebar, inbox, switcher ---------------------------------------------------------

    fn refresh_sidebar(&self) {
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let mut rows = Vec::new();
        let mut last_project: Option<PathBuf> = None;
        let focused = self.focused.borrow().clone();
        for agent in self.agents_in_order() {
            let project = project_of(&agent.cwd);
            if last_project.as_ref() != Some(&project) {
                let header = gtk::Label::builder().label(project_name(&project)).xalign(0.0).build();
                header.add_css_class("project-header");
                let row = gtk::ListBoxRow::builder().child(&header).activatable(false).selectable(false).build();
                self.list.append(&row);
                rows.push(None);
                last_project = Some(project);
            }
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.add_css_class("dot");
            let status = agent.status();
            dot.add_css_class(if status == Status::Idle && agent.unseen.get() { "done" } else { status.css() });
            dot.set_valign(gtk::Align::Center);
            let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let title = gtk::Label::builder().label(agent.title()).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            title.add_css_class("agent-title");
            names.append(&title);
            let sub = match status {
                Status::Idle if agent.unseen.get() => "done",
                Status::Blocked => "needs you",
                other => other.describe(),
            };
            if !sub.is_empty() {
                let sub = gtk::Label::builder().label(sub).xalign(0.0).build();
                sub.add_css_class("dim");
                sub.add_css_class("small");
                names.append(&sub);
            }
            line.append(&dot);
            line.append(&names);
            let row = gtk::ListBoxRow::builder().child(&line).build();
            row.add_css_class("agent-row");
            self.list.append(&row);
            if focused.as_ref().is_some_and(|f| Rc::ptr_eq(f, &agent)) {
                self.list.select_row(Some(&row));
            }
            rows.push(Some(agent));
        }
        *self.rows.borrow_mut() = rows;
    }

    fn refresh_inbox(self: &Rc<Self>) {
        let agents = self.agents.borrow().clone();
        let count: usize = agents.iter().map(|a| a.questions().len()).sum();
        self.inbox.set_label(&format!("⚑ {count}"));
        self.inbox.set_visible(count > 0);
        if count == 0 {
            self.inbox.popdown();
        }
        while let Some(child) = self.inbox_list.first_child() {
            self.inbox_list.remove(&child);
        }
        for agent in agents {
            let questions = agent.questions();
            if questions.is_empty() {
                continue;
            }
            let go = gtk::Button::with_label(&agent.title());
            go.add_css_class("flat");
            go.add_css_class("inbox-agent");
            let weak = Rc::downgrade(self);
            let target = agent.clone();
            go.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.inbox.popdown();
                    app.focus(&target);
                }
            });
            self.inbox_list.append(&go);
            for (serial, title, options) in &questions {
                let serial = *serial;
                let label = gtk::Label::builder().label(title).xalign(0.0).wrap(true).max_width_chars(50).build();
                self.inbox_list.append(&label);
                let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                for (oi, (name, kind)) in options.iter().enumerate() {
                    let button = gtk::Button::with_label(name);
                    button.add_css_class(if kind.starts_with("allow") { "suggested" } else { "plain" });
                    let target = agent.clone();
                    let weak = Rc::downgrade(self);
                    button.connect_clicked(move |_| {
                        target.answer_serial(serial, oi);
                        if let Some(app) = weak.upgrade() {
                            app.refresh_inbox();
                        }
                    });
                    buttons.append(&button);
                }
                self.inbox_list.append(&buttons);
            }
        }
    }

    fn open_switcher(self: &Rc<Self>) {
        self.switcher_entry.set_text("");
        self.fill_switcher();
        let width = self.chat_stack.width();
        self.switcher.set_pointing_to(Some(&gdk::Rectangle::new(width / 2, 12, 1, 1)));
        self.switcher.popup();
        self.switcher_entry.grab_focus();
    }

    fn fill_switcher(self: &Rc<Self>) {
        while let Some(row) = self.switcher_list.row_at_index(0) {
            self.switcher_list.remove(&row);
        }
        let query = self.switcher_entry.text().to_lowercase();
        for agent in self.agents_in_order() {
            let project = project_name(&project_of(&agent.cwd));
            let haystack = format!("{} {}", agent.title(), project).to_lowercase();
            if !query.split_whitespace().all(|word| haystack.contains(word)) {
                continue;
            }
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.add_css_class("dot");
            dot.add_css_class(agent.status().css());
            dot.set_valign(gtk::Align::Center);
            let title = gtk::Label::builder().label(agent.title()).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
            let project = gtk::Label::new(Some(&project));
            project.add_css_class("dim");
            line.append(&dot);
            line.append(&title);
            line.append(&project);
            let row = gtk::ListBoxRow::builder().child(&line).build();
            let weak = Rc::downgrade(self);
            row.connect_activate(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.switcher.popdown();
                    app.focus(&agent);
                }
            });
            self.switcher_list.append(&row);
        }
        if let Some(first) = self.switcher_list.row_at_index(0) {
            self.switcher_list.select_row(Some(&first));
        }
    }
}

/// The repository a folder belongs to (the nearest parent with .git), else the folder.
pub fn project_of(cwd: &Path) -> PathBuf {
    cwd.ancestors().find(|dir| dir.join(".git").exists()).unwrap_or(cwd).to_owned()
}

fn project_name(project: &Path) -> String {
    project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| project.to_string_lossy().into_owned())
}

fn empty_page() -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.set_valign(gtk::Align::Center);
    page.set_halign(gtk::Align::Center);
    let title = gtk::Label::new(Some("No agents yet"));
    title.add_css_class("title");
    let hint = gtk::Label::new(Some("Ctrl+Shift+N starts one in a folder"));
    hint.add_css_class("dim");
    page.append(&title);
    page.append(&hint);
    page
}

fn help_text() -> gtk::Label {
    let keys = [
        ("Ctrl+N", "new agent here"),
        ("Ctrl+Shift+N", "new agent in a folder"),
        ("Ctrl+K", "go to agent"),
        ("Alt+↑ / Alt+↓", "previous / next agent"),
        ("Ctrl+1…9", "agent by position"),
        ("Ctrl+W", "close agent (conversation kept)"),
        ("Ctrl+\\", "previous agent beside this one"),
        ("Enter / Shift+Enter", "send / new line"),
        ("Esc", "stop the turn"),
        ("Shift+Tab", "next permission mode"),
        ("1…9 (empty input)", "answer the question"),
        ("Ctrl+I", "questions from all agents"),
        ("Ctrl+`", "scratch terminal"),
        ("Ctrl+J", "run a command in the tray"),
        ("Ctrl+Shift+J", "peek the newest job"),
        ("Ctrl+Shift+X", "clear finished jobs"),
        ("Ctrl+E / Ctrl+O", "files panel / open a file"),
        ("Ctrl+S", "save (in the editor)"),
        ("Ctrl+B", "hide the sidebar"),
        ("Ctrl+L", "back to the input"),
    ];
    let markup: Vec<String> = keys
        .iter()
        .map(|(key, what)| format!("<tt>{:<20}</tt> {}", glib::markup_escape_text(key), glib::markup_escape_text(what)))
        .collect();
    let label = gtk::Label::builder().use_markup(true).label(markup.join("\n")).xalign(0.0).build();
    label.add_css_class("help");
    label
}
