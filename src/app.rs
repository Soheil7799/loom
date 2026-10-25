//! The window: projects and their agents down the left, the focused agent's chat in the
//! middle (a second one beside it on demand), its files on the right, the project's job
//! tray along the bottom, and the scratchpad and pickers floating over it all.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::{gdk, gio, glib, pango, prelude::*};
use serde_json::{Value, json};

use crate::agent::{Agent, Event, Status};
use crate::files::Files;
use crate::jobs::Tray;
use crate::picker::{Choice, Picker};
use crate::scratch::Scratch;
use crate::settings;
use crate::state::{self, Project, Saved};

/// What a sidebar row stands for.
enum Row {
    Project(PathBuf),
    Agent(Rc<Agent>),
    /// "No agents" under an empty project.
    Empty(PathBuf),
}

pub struct App {
    app: gtk::Application,
    app_id: String,
    pub window: gtk::ApplicationWindow,
    projects: RefCell<Vec<Project>>,
    agents: RefCell<Vec<Rc<Agent>>>,
    closed: RefCell<Vec<Saved>>,
    focused: RefCell<Option<Rc<Agent>>>,
    next_id: Cell<u32>,
    sidebar: gtk::Box,
    list: gtk::ListBox,
    rows: RefCell<Vec<Row>>,
    menu: gtk::Popover,
    inbox: gtk::MenuButton,
    inbox_list: gtk::Box,
    chat_stack: gtk::Stack,
    welcome: gtk::Box,
    /// A project's page: new conversation, or a past one.
    history: Rc<crate::history::Page>,
    /// The second agent shown beside the focused one (Ctrl+\), and where it sits.
    pinned: RefCell<Option<Rc<Agent>>>,
    pinned_box: gtk::Box,
    previous: RefCell<Option<Rc<Agent>>>,
    dock_stack: gtk::Stack,
    bar_stack: gtk::Stack,
    trays: RefCell<HashMap<PathBuf, Rc<Tray>>>,
    scratch: Rc<Scratch>,
    files: Rc<Files>,
    picker: Rc<Picker>,
    run: gtk::Popover,
    run_entry: gtk::Entry,
    rename: gtk::Popover,
    rename_entry: gtk::Entry,
    /// The project being renamed (None: the focused agent).
    renaming: RefCell<Option<PathBuf>>,
    help: gtk::Popover,
    quitting: Cell<bool>,
    /// Ctrl+B was pressed: the next key is a command (herdr's prefix).
    prefix: Cell<bool>,
    prefix_hint: gtk::Label,
    prefix_timeout: RefCell<Option<glib::SourceId>>,
    files_were_open: Cell<bool>,
    /// The chat text size (from the settings).
    scale_css: gtk::CssProvider,
    /// "<agent> needs you", over the window while that agent is out of view.
    toast: gtk::Revealer,
    toast_label: gtk::Label,
    toast_agent: RefCell<std::rc::Weak<Agent>>,
    /// The toast is a question (it goes once answered).
    toast_question: Cell<bool>,
    toast_timeout: RefCell<Option<glib::SourceId>>,
}

impl App {
    pub fn new(app: &gtk::Application) -> Rc<App> {
        let app_id = app.application_id().map(|id| id.to_string()).unwrap_or_default();
        let window = gtk::ApplicationWindow::builder().application(app).title("loom").default_width(1360).default_height(860).build();
        window.add_css_class("loom");

        // Left: projects and their agents.
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar.add_css_class("sidebar");
        sidebar.set_size_request(264, -1);
        // Children that expand must not widen it.
        sidebar.set_hexpand(false);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        top.add_css_class("sidebar-top");
        let mark = crate::logo::mark(20);
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
        top.append(&mark);
        top.append(&name);
        top.append(&inbox);
        sidebar.append(&top);
        let list = gtk::ListBox::new();
        list.add_css_class("agents");
        let menu = gtk::Popover::builder().has_arrow(false).position(gtk::PositionType::Bottom).build();
        menu.add_css_class("menu");
        menu.set_parent(&list);
        let list_scroller = gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        sidebar.append(&list_scroller);
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        bottom.add_css_class("sidebar-bottom");
        let new_button = gtk::Button::with_label("+  New project");
        new_button.add_css_class("flat");
        new_button.set_hexpand(true);
        new_button.set_tooltip_text(Some("Start an agent in another folder (Ctrl+Shift+N)"));
        let settings_button = gtk::Button::from_icon_name("emblem-system-symbolic");
        settings_button.add_css_class("flat");
        settings_button.add_css_class("icon");
        settings_button.set_tooltip_text(Some("Settings (Ctrl+,)"));
        let help_button = gtk::MenuButton::builder().icon_name("help-about-symbolic").always_show_arrow(false).tooltip_text("Keys (Ctrl+/)").build();
        help_button.add_css_class("flat");
        help_button.add_css_class("icon");
        bottom.append(&new_button);
        bottom.append(&settings_button);
        bottom.append(&help_button);
        sidebar.append(&bottom);

        // Middle: chat (and the pinned one) over the dock, the tray at the bottom.
        // No transition: switching agents is instant (and a crossfade breaks when the outgoing
        // chat is moved to the split mid-fade).
        let chat_stack = gtk::Stack::builder().vexpand(true).build();
        let welcome = gtk::Box::new(gtk::Orientation::Vertical, 0);
        welcome.add_css_class("chat");
        chat_stack.add_named(&welcome, Some("empty"));
        let history = crate::history::Page::new();
        chat_stack.add_named(&history.root, Some("project"));
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
        files.root.set_size_request(400, -1);
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
        let prefix_hint = gtk::Label::builder()
            .label("Ctrl+B …   v split · h/l swap · j/k agents · c new · x close · z zoom · b sidebar · ? all")
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .margin_bottom(44)
            .visible(false)
            .can_target(false)
            .build();
        prefix_hint.add_css_class("prefix-hint");
        overlay.add_overlay(&prefix_hint);
        let toast_label = gtk::Label::builder().ellipsize(pango::EllipsizeMode::End).max_width_chars(60).build();
        let toast_dot = gtk::Box::builder().valign(gtk::Align::Center).build();
        toast_dot.add_css_class("dot");
        toast_dot.add_css_class("blocked");
        let toast_go = gtk::Button::with_label("Show");
        toast_go.add_css_class("flat");
        toast_go.add_css_class("toast-go");
        let toast_close = gtk::Button::from_icon_name("window-close-symbolic");
        toast_close.add_css_class("flat");
        toast_close.set_tooltip_text(Some("Dismiss"));
        let toast_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        toast_box.add_css_class("toast");
        toast_box.append(&toast_dot);
        toast_box.append(&toast_label);
        toast_box.append(&toast_go);
        toast_box.append(&toast_close);
        let toast = gtk::Revealer::builder()
            .child(&toast_box)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .margin_top(10)
            .build();
        overlay.add_overlay(&toast);
        window.set_child(Some(&overlay));

        // Floating bits.
        let picker = Picker::new(&chat_stack);
        let run_entry = gtk::Entry::builder().placeholder_text("Command to run here (name: command names the chip)").width_chars(56).build();
        run_entry.add_css_class("mono");
        let run = gtk::Popover::builder().child(&run_entry).position(gtk::PositionType::Top).build();
        run.set_parent(&bar_stack);
        let rename_entry = gtk::Entry::builder().width_chars(36).placeholder_text("Agent name").build();
        let rename = gtk::Popover::builder().child(&rename_entry).has_arrow(false).position(gtk::PositionType::Bottom).build();
        rename.set_parent(&chat_stack);
        let help = gtk::Popover::builder().child(&help_text()).build();
        help_button.set_popover(Some(&help));

        let scale_css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &scale_css, gtk::STYLE_PROVIDER_PRIORITY_USER + 3);
        }

        let this = Rc::new(App {
            app: app.clone(),
            app_id,
            window,
            projects: RefCell::default(),
            agents: RefCell::default(),
            closed: RefCell::default(),
            focused: RefCell::default(),
            next_id: Cell::new(1),
            sidebar,
            list,
            rows: RefCell::default(),
            menu,
            inbox,
            inbox_list,
            chat_stack,
            welcome,
            history,
            pinned: RefCell::default(),
            pinned_box,
            previous: RefCell::default(),
            dock_stack,
            bar_stack,
            trays: RefCell::default(),
            scratch,
            files,
            picker,
            run,
            run_entry,
            rename,
            rename_entry,
            renaming: RefCell::default(),
            help,
            quitting: Cell::new(false),
            prefix: Cell::new(false),
            prefix_hint,
            prefix_timeout: RefCell::default(),
            files_were_open: Cell::new(false),
            scale_css,
            toast,
            toast_label,
            toast_agent: RefCell::default(),
            toast_question: Cell::new(false),
            toast_timeout: RefCell::default(),
        });
        this.wire_toast(&toast_go, &toast_close);
        this.wire_history();
        crate::theme::status_colors(&this.window);
        this.wire(&new_button, &settings_button);
        this.apply_settings(&settings::get());
        this.restore();
        this.listen();
        this
    }

    fn wire(self: &Rc<Self>, new_button: &gtk::Button, settings_button: &gtk::Button) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            let Some(app) = weak.upgrade() else { return };
            app.activate_row(row.index() as usize);
        });
        let weak = Rc::downgrade(self);
        new_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.new_agent(true);
            }
        });
        let weak = Rc::downgrade(self);
        settings_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::prefs::open(&app.window);
            }
        });
        let weak = Rc::downgrade(self);
        self.inbox.popover().unwrap().connect_show(move |_| {
            if let Some(app) = weak.upgrade() {
                app.refresh_inbox();
            }
        });
        // Right-click on a project or agent: its menu.
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |_, _, x, y| {
            let Some(app) = weak.upgrade() else { return };
            if let Some(row) = app.list.row_at_y(y as i32) {
                app.row_menu(row.index() as usize, x, y);
            }
        });
        self.list.add_controller(click);

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
        let weak = Rc::downgrade(self);
        self.rename_entry.connect_activate(move |entry| {
            let Some(app) = weak.upgrade() else { return };
            app.rename.popdown();
            let name = entry.text().trim().to_owned();
            if let Some(project) = app.renaming.take() {
                app.rename_project(&project, &name);
            } else if let Some(agent) = app.focused_agent() {
                agent.rename(&name);
                app.refresh_sidebar();
                app.save();
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
            if let Some(agent) = app.agent_by_id(id) {
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

        let weak = Rc::downgrade(self);
        settings::on_change(move |s| {
            if let Some(app) = weak.upgrade() {
                app.apply_settings(s);
                app.refresh_sidebar();
            }
        });
    }

    /// Opens the control socket the agents' MCP servers talk to.
    fn listen(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        crate::control::serve(crate::control::socket_path(&self.app_id), move |agent, method, params| {
            let app = weak.upgrade().ok_or("loom is closing")?;
            app.control(agent, method, params)
        });
    }

    fn apply_settings(&self, s: &settings::Settings) {
        self.scale_css.load_from_string(&format!(
            ".messages, .plan, textview.input {{ font-size: {}em; }}",
            (s.text_scale * 100.0).round() / 100.0
        ));
        for agent in self.agents.borrow().iter() {
            agent.chat.set_show_thinking(s.show_thinking);
        }
        crate::theme::refont(&s.terminal_font);
    }

    // ---- keys -----------------------------------------------------------------------------

    fn key(self: &Rc<Self>, key: gdk::Key, keycode: u32, modifiers: gdk::ModifierType) -> glib::Propagation {
        let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
        // The key on the first layout, so shortcuts work with the Persian one active too.
        let base = WidgetExt::display(&self.window)
            .translate_key(keycode, gdk::ModifierType::empty(), 0)
            .map_or(key, |(k, ..)| k)
            .to_lower();
        if is_modifier(key) {
            return glib::Propagation::Proceed;
        }
        if self.prefix.get() {
            self.end_prefix();
            self.prefixed(base, shift);
            return glib::Propagation::Stop;
        }
        // Ctrl+B starts the prefix everywhere, terminals included (as in herdr).
        if ctrl && !shift && !alt && base == gdk::Key::b {
            self.start_prefix();
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::F2 && !ctrl && !alt {
            self.begin_rename();
            return glib::Propagation::Stop;
        }
        if !ctrl && !alt {
            return self.type_into_chat(key);
        }
        let in_terminal = GtkWindowExt::focus(&self.window).is_some_and(|w| w.is::<vte::Terminal>() || w.ancestor(vte::Terminal::static_type()).is_some());

        match (ctrl, shift, alt, base) {
            (true, false, false, gdk::Key::grave) => self.toggle_scratch(),
            (false, false, true, gdk::Key::Up) => self.step(-1),
            (false, false, true, gdk::Key::Down) => self.step(1),
            (false, false, true, gdk::Key::Left | gdk::Key::Right) => self.swap(),
            (true, false, false, gdk::Key::Tab) => self.step(1),
            (true, true, false, gdk::Key::Tab | gdk::Key::ISO_Left_Tab) => self.step(-1),
            (true, true, false, gdk::Key::p) => self.palette(),
            // Everything else belongs to a focused terminal (Ctrl+W, Ctrl+K… mean something there).
            _ if in_terminal => return glib::Propagation::Proceed,
            // Questions, answered while typing: Alt+1…9 an option, Alt+Y allow, Alt+N deny,
            // Alt+Enter sends a form.
            (false, false, true, k) if k.to_unicode().and_then(|c| c.to_digit(10)).is_some_and(|d| d >= 1) => {
                let option = k.to_unicode().and_then(|c| c.to_digit(10)).unwrap_or(1) as usize - 1;
                if !self.focused_agent().is_some_and(|a| a.answer(0, option)) {
                    return glib::Propagation::Proceed;
                }
            }
            (false, false, true, gdk::Key::y) => {
                if !self.focused_agent().is_some_and(|a| a.answer_kind("allow")) {
                    return glib::Propagation::Proceed;
                }
            }
            (false, false, true, gdk::Key::n) => {
                if !self.focused_agent().is_some_and(|a| a.answer_kind("reject")) {
                    return glib::Propagation::Proceed;
                }
            }
            (false, false, true, gdk::Key::Return | gdk::Key::KP_Enter) => {
                if !self.focused_agent().is_some_and(|a| a.send_form()) {
                    return glib::Propagation::Proceed;
                }
            }
            (true, false, false, gdk::Key::n) => self.new_agent(false),
            (true, true, false, gdk::Key::n) => self.new_agent(true),
            (true, true, false, gdk::Key::t) => self.reopen_closed(),
            (true, false, false, gdk::Key::w) => self.close_focused(),
            (true, false, false, gdk::Key::k) => self.open_switcher(),
            (true, false, false, gdk::Key::i) => self.open_inbox(),
            (true, false, false, gdk::Key::j) => self.ask_command(),
            (true, true, false, gdk::Key::j) => self.current_tray().peek_latest(),
            (true, true, false, gdk::Key::x) => self.current_tray().clear_finished(),
            (true, false, false, gdk::Key::e) => self.files.root.set_visible(!self.files.root.is_visible()),
            (true, false, false, gdk::Key::o) => {
                self.files.root.set_visible(true);
                self.files.choose();
            }
            (true, true, false, gdk::Key::o) => self.files.open_in_nevis(),
            (true, false, false, gdk::Key::l) => self.focus_input(),
            (true, false, false, gdk::Key::p) => self.find_file(),
            (true, false, false, gdk::Key::backslash) => self.toggle_split(),
            (true, false, false, gdk::Key::slash) => self.help.popup(),
            (true, false, false, gdk::Key::comma) => crate::prefs::open(&self.window),
            (true, false, false, k) if k.to_unicode().and_then(|c| c.to_digit(10)).is_some_and(|d| d >= 1) => {
                self.focus_nth(k.to_unicode().and_then(|c| c.to_digit(10)).unwrap_or(1) as usize - 1);
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    /// A character typed where nothing takes text (the sidebar, a button, a tool row) goes
    /// to the focused agent's input, so typing never needs the mouse first. Space and Enter
    /// still press what has focus.
    fn type_into_chat(&self, key: gdk::Key) -> glib::Propagation {
        let Some(c) = key.to_unicode().filter(|c| !c.is_control() && *c != ' ') else { return glib::Propagation::Proceed };
        let Some(agent) = self.focused_agent() else { return glib::Propagation::Proceed };
        // A digit with a question open and nothing typed yet answers it, wherever focus is.
        if let Some(n) = crate::chat::any_digit(c).filter(|n| *n >= 1)
            && agent.chat.input.buffer().char_count() == 0
            && agent.answer(0, n as usize - 1)
        {
            return glib::Propagation::Stop;
        }
        let focus = GtkWindowExt::focus(&self.window);
        let takes_text = focus.as_ref().is_some_and(|w| {
            w.is::<gtk::Text>() || w.is::<gtk::TextView>() || w.is::<vte::Terminal>() || w.ancestor(vte::Terminal::static_type()).is_some()
        });
        if takes_text {
            return glib::Propagation::Proceed;
        }
        let input = &agent.chat.input;
        let buffer = input.buffer();
        buffer.place_cursor(&buffer.end_iter());
        buffer.insert_at_cursor(&c.to_string());
        input.grab_focus();
        glib::Propagation::Stop
    }

    fn start_prefix(self: &Rc<Self>) {
        self.prefix.set(true);
        self.prefix_hint.set_visible(true);
        let weak = Rc::downgrade(self);
        let timeout = glib::timeout_add_local_once(Duration::from_millis(2500), move || {
            if let Some(app) = weak.upgrade() {
                app.prefix_timeout.borrow_mut().take();
                app.end_prefix();
            }
        });
        if let Some(old) = self.prefix_timeout.borrow_mut().replace(timeout) {
            old.remove();
        }
    }

    fn end_prefix(&self) {
        self.prefix.set(false);
        self.prefix_hint.set_visible(false);
        if let Some(timeout) = self.prefix_timeout.borrow_mut().take() {
            timeout.remove();
        }
    }

    /// The key after Ctrl+B: herdr's letters where it has one.
    fn prefixed(self: &Rc<Self>, key: gdk::Key, shift: bool) {
        match (shift, key) {
            (_, gdk::Key::v | gdk::Key::minus | gdk::Key::backslash) => self.toggle_split(),
            (_, gdk::Key::h | gdk::Key::l) => self.swap(),
            (false, gdk::Key::j | gdk::Key::n) => self.step(1),
            (false, gdk::Key::k | gdk::Key::p) => self.step(-1),
            (false, gdk::Key::c) => self.new_agent(false),
            (true, gdk::Key::n | gdk::Key::c) => self.new_agent(true),
            (_, gdk::Key::x) => self.close_focused(),
            (_, gdk::Key::z) => self.zoom(),
            (_, gdk::Key::b) => self.sidebar.set_visible(!self.sidebar.is_visible()),
            (_, gdk::Key::f | gdk::Key::e) => self.files.root.set_visible(!self.files.root.is_visible()),
            (_, gdk::Key::o) => {
                self.files.root.set_visible(true);
                self.files.choose();
            }
            (_, gdk::Key::t | gdk::Key::grave) => self.toggle_scratch(),
            (_, gdk::Key::r) => self.ask_command(),
            (_, gdk::Key::s) => self.open_switcher(),
            (_, gdk::Key::g) => self.find_file(),
            (_, gdk::Key::i) => self.open_inbox(),
            (_, gdk::Key::colon | gdk::Key::semicolon) => self.palette(),
            (_, gdk::Key::comma) => crate::prefs::open(&self.window),
            (_, gdk::Key::question | gdk::Key::slash) => self.help.popup(),
            (_, k) if k.to_unicode().and_then(|c| c.to_digit(10)).is_some_and(|d| d >= 1) => {
                self.focus_nth(k.to_unicode().and_then(|c| c.to_digit(10)).unwrap_or(1) as usize - 1);
            }
            _ => {}
        }
    }

    /// Quits without asking: agents stopped, state saved (signals, session end).
    pub fn quit_now(&self) {
        self.quitting.set(true);
        self.window.close();
    }

    // ---- for devhooks ----------------------------------------------------------------------

    pub fn prefix_key(self: &Rc<Self>, name: &str) {
        let shift = name.chars().next().is_some_and(char::is_uppercase);
        if let Some(key) = gdk::Key::from_name(name.to_lowercase()) {
            self.prefixed(key, shift);
        }
    }

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
            "palette" => self.palette(),
            "inbox" => self.open_inbox(),
            "help" => self.help.popup(),
            "settings" => crate::prefs::open(&self.window),
            "peek" => self.current_tray().peek_latest(),
            "split" => self.toggle_split(),
            "find" => self.find_file(),
            "nevis" => self.files.open_in_nevis(),
            "tree" => self.files.show_project(),
            "changes" => self.files.show_changes(),
            "close" => self.close_focused(),
            "reopen" => self.reopen_closed(),
            "rename" => {
                if let Some(agent) = self.focused_agent() {
                    agent.rename(argument);
                    self.refresh_sidebar();
                    self.save();
                }
            }
            "collapse" => self.toggle_collapsed(Path::new(argument)),
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

    // ---- small actions --------------------------------------------------------------------

    fn current_project(&self) -> PathBuf {
        let focused = self.focused.borrow().as_ref().map(|a| a.project.clone());
        focused.or_else(|| self.showing_project()).unwrap_or_else(glib::home_dir)
    }

    fn ask_command(&self) {
        self.run.popup();
        self.run_entry.grab_focus();
    }

    fn open_inbox(&self) {
        if self.inbox.is_visible() {
            self.inbox.popup();
        }
    }

    /// Ctrl+P: pick a file of the project in view and open it in the editor.
    fn find_file(self: &Rc<Self>) {
        let project = self.current_project();
        let choices = crate::quickopen::list_files(&project)
            .into_iter()
            .map(|file| {
                let (dir, name) = match file.rsplit_once('/') {
                    Some((dir, name)) => (dir.to_owned(), name.to_owned()),
                    None => (String::new(), file.clone()),
                };
                let files = self.files.clone();
                let path = project.join(&file);
                Choice::new(name, dir, move || {
                    files.root.set_visible(true);
                    files.open_file(&path);
                })
            })
            .collect();
        self.picker.open("Open a file in this project…", choices);
    }

    /// The scratch terminal opens where the agent works: its latest repo, else the project.
    fn toggle_scratch(self: &Rc<Self>) {
        let place = self.focused_agent().and_then(|a| a.repos().first().cloned()).unwrap_or_else(|| self.current_project());
        if !self.scratch.toggle(&place) {
            self.focus_input();
        }
    }

    fn focus_input(&self) {
        if let Some(agent) = self.focused.borrow().as_ref() {
            agent.chat.input.grab_focus();
        }
    }

    /// F2: the focused agent, or on a project's page, the project.
    fn begin_rename(&self) {
        let agent = self.focused_agent();
        let project = if agent.is_none() { self.showing_project() } else { None };
        let (text, placeholder) = match (&agent, &project) {
            (Some(agent), _) => (agent.title(), "Agent name"),
            (None, Some(project)) => (self.project_label(project), "Project name (empty: the folder's)"),
            (None, None) => return,
        };
        *self.renaming.borrow_mut() = project;
        self.rename_entry.set_placeholder_text(Some(placeholder));
        self.rename_entry.set_text(&text);
        self.rename.set_pointing_to(Some(&gdk::Rectangle::new(24, 30, 1, 1)));
        self.rename.popup();
        self.rename_entry.grab_focus();
        self.rename_entry.select_region(0, -1);
    }

    // ---- palette and switcher -------------------------------------------------------------

    /// Ctrl+Shift+P: every action by name, and every agent and project to go to.
    fn palette(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let act = |title: &str, keys: &str, f: fn(&Rc<App>)| {
            let weak = weak.clone();
            Choice::new(title, keys, move || {
                if let Some(app) = weak.upgrade() {
                    f(&app);
                }
            })
        };
        let mut choices = vec![
            act("New agent… (or resume one)", "Ctrl+N", |a| a.new_agent(false)),
            act("Open a project folder…", "Ctrl+Shift+N", |a| a.new_agent(true)),
            act("Reopen the last closed agent", "Ctrl+Shift+T", |a| a.reopen_closed()),
            act("Rename this agent or project", "F2", |a| a.begin_rename()),
            act("Restart this agent", "", |a| {
                if let Some(agent) = a.focused_agent() {
                    agent.restart();
                }
            }),
            act("Stop the turn", "Esc", |a| {
                if let Some(agent) = a.focused_agent() {
                    agent.cancel();
                }
            }),
            act("Next permission mode", "Shift+Tab", |a| {
                if let Some(agent) = a.focused_agent() {
                    agent.cycle_mode();
                }
            }),
            act("Close this agent", "Ctrl+W", |a| a.close_focused()),
            act("Show the previous agent beside", "Ctrl+\\", |a| a.toggle_split()),
            act("Swap the two agents", "Alt+←", |a| a.swap()),
            act("Zoom (hide the side panels)", "Ctrl+B z", |a| a.zoom()),
            act("Toggle the sidebar", "Ctrl+B b", |a| a.sidebar.set_visible(!a.sidebar.is_visible())),
            act("Toggle the files panel", "Ctrl+E", |a| a.files.root.set_visible(!a.files.root.is_visible())),
            act("Find a file in the project", "Ctrl+P", |a| a.find_file()),
            act("Open a file…", "Ctrl+O", |a| {
                a.files.root.set_visible(true);
                a.files.choose();
            }),
            act("Project files (tree)", "", |a| a.files.show_project()),
            act("Git changes in this project", "", |a| a.files.show_changes()),
            act("Open the file in nevis", "Ctrl+Shift+O", |a| a.files.open_in_nevis()),
            act("Scratch terminal", "Ctrl+`", |a| a.toggle_scratch()),
            act("Run a command in the tray…", "Ctrl+J", |a| a.ask_command()),
            act("Peek the newest job", "Ctrl+Shift+J", |a| a.current_tray().peek_latest()),
            act("Clear finished jobs", "Ctrl+Shift+X", |a| a.current_tray().clear_finished()),
            act("Questions waiting", "Ctrl+I", |a| a.open_inbox()),
            act("Settings", "Ctrl+,", |a| crate::prefs::open(&a.window)),
            act("Keyboard shortcuts", "Ctrl+/", |a| a.help.popup()),
        ];
        for agent in self.agents_in_order() {
            let weak = weak.clone();
            let target = Rc::downgrade(&agent);
            let mut choice = Choice::new(format!("Go to {}", agent.title()), self.project_label(&agent.project), move || {
                if let (Some(app), Some(target)) = (weak.upgrade(), target.upgrade()) {
                    app.focus(&target);
                }
            });
            choice.dot = Some(agent.status().css());
            choices.push(choice);
        }
        for saved in self.closed.borrow().iter() {
            let weak = weak.clone();
            let saved = saved.clone();
            let title = if saved.title.is_empty() { "an agent".to_owned() } else { saved.title.clone() };
            choices.push(Choice::new(format!("Reopen {title}"), self.project_label(&state_project(&saved.cwd)), move || {
                if let Some(app) = weak.upgrade() {
                    app.reopen(saved.clone());
                }
            }));
        }
        self.picker.open("Run a command, or go to an agent…", choices);
    }

    /// Ctrl+K: agents and projects.
    fn open_switcher(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let mut choices = Vec::new();
        for agent in self.agents_in_order() {
            let weak = weak.clone();
            let target = Rc::downgrade(&agent);
            let mut choice = Choice::new(agent.title(), self.project_label(&agent.project), move || {
                if let (Some(app), Some(target)) = (weak.upgrade(), target.upgrade()) {
                    app.focus(&target);
                }
            });
            choice.dot = Some(agent.status().css());
            choices.push(choice);
        }
        for project in self.projects.borrow().iter() {
            let weak = weak.clone();
            let path = project.path.clone();
            choices.push(Choice::new(self.project_label(&project.path), "project · conversations", move || {
                if let Some(app) = weak.upgrade() {
                    app.show_project(&path, &path);
                }
            }));
        }
        self.picker.open("Go to an agent or project…", choices);
    }

    // ---- projects and agents --------------------------------------------------------------

    fn restore(self: &Rc<Self>) {
        let layout = state::load(&self.app_id);
        *self.projects.borrow_mut() = layout.projects;
        *self.closed.borrow_mut() = layout.closed;
        for saved in layout.agents {
            self.add_agent(saved);
        }
        let agents = self.agents_in_order();
        if let Some(agent) = agents.get(layout.focused).or(agents.first()) {
            self.focus(agent);
            if let Some(pinned) = layout.pinned.and_then(|p| agents.get(p)).filter(|p| !Rc::ptr_eq(p, agent)) {
                self.pin(pinned);
                self.save();
            }
        }
        self.refresh_sidebar();
    }

    fn ensure_project(&self, path: &Path) {
        let mut projects = self.projects.borrow_mut();
        if !projects.iter().any(|p| p.path == path) {
            projects.push(Project { path: path.to_owned(), collapsed: false, name: String::new() });
        }
    }

    fn add_agent(self: &Rc<Self>, saved: Saved) -> Rc<Agent> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let project = project_of(&saved.cwd);
        self.ensure_project(&project);
        let tray = self.tray_for(&project);
        let agent = Agent::new(id, saved, tray);
        agent.chat.set_show_thinking(settings::get().show_thinking);
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

    fn agent_by_id(&self, id: u32) -> Option<Rc<Agent>> {
        self.agents.borrow().iter().find(|a| a.id == id).cloned()
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
        let project = self.current_project();
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
        self.toast_settled(agent);
        if self.is_pinned(agent) && self.chat_stack.visible_child_name().as_deref() == Some("project") {
            self.unpin();
        }
        if !self.is_pinned(agent) {
            self.chat_stack.set_visible_child(&agent.chat.root);
        }
        // A collapsed project opens to show what was picked.
        if let Some(project) = self.projects.borrow_mut().iter_mut().find(|p| p.path == agent.project) {
            project.collapsed = false;
        }
        self.show_tray(&agent.project, &agent.tray);
        self.show_files(&agent);
        if matches!(agent.status(), Status::Suspended | Status::Failed) {
            agent.start();
        }
        agent.chat.input.grab_focus();
        // Again once the click (or key) that chose it is done: a sidebar row takes focus
        // on release.
        let input = agent.chat.input.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(input) = input.upgrade() {
                input.grab_focus();
            }
        });
        self.refresh_sidebar();
        self.save();
    }

    fn show_files(&self, agent: &Rc<Agent>) {
        self.files.show(&agent.cwd, &agent.project, &agent.files());
        self.files.set_repos(agent.repos());
    }

    fn is_pinned(&self, agent: &Rc<Agent>) -> bool {
        self.pinned.borrow().as_ref().is_some_and(|p| Rc::ptr_eq(p, agent))
    }

    /// Shows the previous agent beside the focused one, or puts it back.
    fn toggle_split(self: &Rc<Self>) {
        if self.unpin().is_some() {
            self.save();
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
        self.pin(&agent);
        self.save();
    }

    /// Shows `agent` beside the focused one (in place of any pinned before).
    fn pin(self: &Rc<Self>, agent: &Rc<Agent>) {
        if self.is_pinned(agent) || self.focused.borrow().as_ref().is_some_and(|f| Rc::ptr_eq(f, agent)) {
            return;
        }
        self.unpin();
        self.chat_stack.remove(&agent.chat.root);
        self.pinned_box.append(&agent.chat.root);
        self.pinned_box.set_visible(true);
        agent.last_active.set(std::time::Instant::now());
        agent.unseen.set(false);
        if matches!(agent.status(), Status::Suspended | Status::Failed) {
            agent.start();
        }
        *self.pinned.borrow_mut() = Some(agent.clone());
        self.toast_settled(agent);
        self.mark_active();
    }

    /// Puts the pinned agent back with the others; returns it.
    fn unpin(&self) -> Option<Rc<Agent>> {
        let agent = self.pinned.borrow_mut().take()?;
        self.pinned_box.remove(&agent.chat.root);
        self.pinned_box.set_visible(false);
        self.chat_stack.add_named(&agent.chat.root, Some(&format!("agent-{}", agent.id)));
        if agent.focused.get() {
            self.chat_stack.set_visible_child(&agent.chat.root);
            agent.chat.input.grab_focus();
        }
        self.mark_active();
        Some(agent)
    }

    /// The pinned agent and the focused one change places (focus moves across the split).
    fn swap(self: &Rc<Self>) {
        let Some(other) = self.unpin() else { return };
        let Some(current) = self.focused.borrow().clone() else { return };
        self.focus(&other);
        self.pin(&current);
        self.save();
    }

    /// Hides the sidebar and the files panel (and brings them back).
    fn zoom(&self) {
        let zoomed = !self.sidebar.is_visible() && !self.files.root.is_visible();
        if zoomed {
            self.sidebar.set_visible(true);
            self.files.root.set_visible(self.files_were_open.get());
        } else {
            self.files_were_open.set(self.files.root.is_visible());
            self.sidebar.set_visible(false);
            self.files.root.set_visible(false);
        }
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

    /// Agents in sidebar order: by project (in the sidebar's order), then as opened.
    /// The projects as the sidebar shows them: with sorting on, those with an agent waiting
    /// for the user (or failed) come first, the rest keep their place.
    fn projects_in_order(&self) -> Vec<Project> {
        let mut projects = self.projects.borrow().clone();
        if settings::get().sort_by_urgency {
            let agents = self.agents.borrow();
            let needs_you = |p: &Project| agents.iter().any(|a| a.project == p.path && a.status().urgency(false) <= 1);
            projects.sort_by_key(|p| !needs_you(p));
        }
        projects
    }

    /// Agents in sidebar order (number keys follow it too): by project, and within one, by
    /// urgency when sorting is on (a stable sort, so equals keep the order they started in).
    fn agents_in_order(&self) -> Vec<Rc<Agent>> {
        let projects = self.projects_in_order();
        let agents = self.agents.borrow();
        let urgent = settings::get().sort_by_urgency;
        let mut ordered: Vec<Rc<Agent>> = Vec::new();
        for project in &projects {
            let mut mine: Vec<Rc<Agent>> = agents.iter().filter(|a| a.project == project.path).cloned().collect();
            if urgent {
                mine.sort_by_key(|a| a.status().urgency(a.unseen.get()));
            }
            ordered.extend(mine);
        }
        // Any agent whose project is missing from the list still counts.
        ordered.extend(agents.iter().filter(|a| !projects.iter().any(|p| p.path == a.project)).cloned());
        ordered
    }

    /// A new agent starts from its project's page (where a past conversation can be picked
    /// up instead): this one's (`choose`: a folder asked for).
    fn new_agent(self: &Rc<Self>, choose: bool) {
        let here = self.focused.borrow().as_ref().map(|a| (a.project.clone(), a.cwd.clone()));
        let here = here.or_else(|| self.showing_project().map(|p| (p.clone(), p)));
        match here {
            Some((project, cwd)) if !choose => self.show_project(&project, &cwd),
            _ => {
                let here = here.map(|(_, cwd)| cwd);
                let dialog = gtk::FileDialog::builder().title("Open a project folder").modal(true).build();
                if let Some(cwd) = here {
                    dialog.set_initial_folder(Some(&gio::File::for_path(project_of(&cwd).parent().unwrap_or(&cwd))));
                }
                let weak = Rc::downgrade(self);
                dialog.select_folder(Some(&self.window), None::<&gio::Cancellable>, move |result| {
                    if let (Some(app), Ok(folder)) = (weak.upgrade(), result)
                        && let Some(path) = folder.path()
                    {
                        app.show_project(&project_of(&path), &path);
                    }
                });
            }
        }
    }

    fn wire_history(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        *self.history.on_new.borrow_mut() = Some(Rc::new(move |cwd| {
            if let Some(app) = weak.upgrade() {
                app.start_agent_in(cwd);
            }
        }));
        let weak = Rc::downgrade(self);
        *self.history.on_resume.borrow_mut() = Some(Rc::new(move |past| {
            if let Some(app) = weak.upgrade() {
                app.resume(past);
            }
        }));
        let weak = Rc::downgrade(self);
        *self.history.is_open.borrow_mut() = Some(Rc::new(move |id: &str| {
            weak.upgrade().is_some_and(|app| app.agent_with_session(id).is_some())
        }));
    }

    /// The project whose page is showing, if it is.
    fn showing_project(&self) -> Option<PathBuf> {
        let showing = self.focused.borrow().is_none() && self.chat_stack.visible_child_name().as_deref() == Some("project");
        showing.then(|| self.history.project())
    }

    /// Picks row `n` of the project page (0: new conversation).
    pub fn pick_history(&self, n: i32) {
        self.history.activate(n);
    }

    /// The project's name: given by hand, else its folder's.
    fn project_label(&self, path: &Path) -> String {
        let projects = self.projects.borrow();
        let named = projects.iter().find(|p| p.path == path).map(|p| p.name.trim()).filter(|n| !n.is_empty());
        named.map_or_else(|| project_name(path), str::to_owned)
    }

    /// Names a project (empty: back to its folder's name).
    fn rename_project(self: &Rc<Self>, path: &Path, name: &str) {
        if let Some(project) = self.projects.borrow_mut().iter_mut().find(|p| p.path == path) {
            project.name = name.to_owned();
        }
        if self.showing_project().is_some_and(|p| p == path) {
            self.history.set_name(&self.project_label(path));
        }
        self.refresh_sidebar();
        self.save();
    }

    pub fn name_project(self: &Rc<Self>, path: &Path, name: &str) {
        self.rename_project(path, name);
    }

    /// The right-click menu turns into the name field, where it was opened.
    fn rename_project_at(self: &Rc<Self>, path: &Path, x: f64, y: f64) {
        let entry = gtk::Entry::builder().width_chars(30).placeholder_text("Project name (empty: the folder's)").build();
        entry.set_text(&self.project_label(path));
        let weak = Rc::downgrade(self);
        let path = path.to_owned();
        entry.connect_activate(move |entry| {
            if let Some(app) = weak.upgrade() {
                app.menu.popdown();
                app.rename_project(&path, entry.text().trim());
            }
        });
        self.menu.set_child(Some(&entry));
        self.menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        self.menu.popup();
        entry.grab_focus();
        entry.select_region(0, -1);
    }

    fn agent_with_session(&self, id: &str) -> Option<Rc<Agent>> {
        self.agents.borrow().iter().find(|a| a.session_id().as_deref() == Some(id)).cloned()
    }

    /// Opens `project`'s page: nothing starts until a conversation is picked there.
    pub fn show_project(self: &Rc<Self>, project: &Path, cwd: &Path) {
        self.ensure_project(project);
        if let Some(entry) = self.projects.borrow_mut().iter_mut().find(|p| p.path == project) {
            entry.collapsed = false;
        }
        // No agent has the keyboard while the page shows (the last one is "previous").
        if let Some(old) = self.focused.borrow_mut().take() {
            old.focused.set(false);
            *self.previous.borrow_mut() = Some(old);
        }
        self.chat_stack.set_visible_child_name("project");
        self.history.show(project, cwd, &self.project_label(project));
        let tray = self.tray_for(project);
        self.show_tray(project, &tray);
        self.files.show(cwd, project, &[]);
        self.mark_active();
        self.refresh_sidebar();
        self.save();
        self.history.grab_focus();
    }

    /// Picks up a past conversation (or goes to it, when it is open already).
    fn resume(self: &Rc<Self>, past: crate::history::Past) {
        if let Some(agent) = self.agent_with_session(&past.session_id) {
            self.focus(&agent);
            return;
        }
        self.closed.borrow_mut().retain(|c| c.session_id.as_deref() != Some(past.session_id.as_str()));
        // A folder deleted since resumes in its project (or home): the adapter can't start in it.
        let cwd = [past.cwd.clone(), project_of(&past.cwd), glib::home_dir()].into_iter().find(|d| d.is_dir()).unwrap_or(past.cwd);
        let saved = Saved { cwd, session_id: Some(past.session_id), title: past.title.unwrap_or_default(), named: false };
        let agent = self.add_agent(saved);
        self.focus(&agent);
    }

    fn start_agent_in(self: &Rc<Self>, cwd: PathBuf) {
        let agent = self.add_agent(Saved::new(cwd));
        self.focus(&agent);
    }

    fn reopen(self: &Rc<Self>, saved: Saved) {
        self.closed.borrow_mut().retain(|c| *c != saved);
        let agent = self.add_agent(saved);
        self.focus(&agent);
    }

    fn reopen_closed(self: &Rc<Self>) {
        let last = self.closed.borrow().first().cloned();
        if let Some(saved) = last {
            self.reopen(saved);
        }
    }

    fn close_focused(self: &Rc<Self>) {
        if let Some(agent) = self.focused_agent() {
            self.close_agent(&agent);
        }
    }

    fn close_agent(self: &Rc<Self>, agent: &Rc<Agent>) {
        if !agent.is_busy() {
            self.remove_agent(agent);
            return;
        }
        let dialog = gtk::AlertDialog::builder()
            .message(format!("“{}” is working", agent.title()))
            .detail("Closing stops what it is doing. The conversation itself is kept: Ctrl+Shift+T reopens it.")
            .buttons(["Keep it", "Close"])
            .cancel_button(0)
            .default_button(0)
            .build();
        let weak = Rc::downgrade(self);
        let agent = agent.clone();
        dialog.choose(Some(&self.window), None::<&gio::Cancellable>, move |choice| {
            if let (Some(app), Ok(1)) = (weak.upgrade(), choice) {
                app.remove_agent(&agent);
            }
        });
    }

    fn remove_agent(self: &Rc<Self>, agent: &Rc<Agent>) {
        self.remove_agent_then(agent, true);
    }

    /// Closes `agent`; `refocus` moves focus on to the next one (not when several close at
    /// once: each would be started only to be closed).
    fn remove_agent_then(self: &Rc<Self>, agent: &Rc<Agent>, refocus: bool) {
        let order = self.agents_in_order();
        let position = order.iter().position(|a| Rc::ptr_eq(a, agent)).unwrap_or(0);
        let focused_left = self.focused.borrow().as_ref().is_some_and(|f| Rc::ptr_eq(f, agent));
        if self.is_pinned(agent) || (focused_left && self.pinned.borrow().is_some()) {
            self.unpin();
        }
        if self.previous.borrow().as_ref().is_some_and(|p| Rc::ptr_eq(p, agent)) {
            *self.previous.borrow_mut() = None;
        }
        if self.toast_agent.borrow().upgrade().is_some_and(|a| Rc::ptr_eq(&a, agent)) {
            self.hide_toast();
        }
        agent.stop();
        // Remembered to reopen (a conversation that never started has nothing to reopen).
        let saved = agent.saved();
        if saved.session_id.is_some() {
            let mut closed = self.closed.borrow_mut();
            closed.retain(|c| c.session_id != saved.session_id);
            closed.insert(0, saved);
            closed.truncate(state::CLOSED_KEPT);
        }
        self.chat_stack.remove(&agent.chat.root);
        self.agents.borrow_mut().retain(|a| !Rc::ptr_eq(a, agent));
        if focused_left {
            *self.focused.borrow_mut() = None;
        }
        if focused_left && refocus {
            let order = self.agents_in_order();
            match order.get(position.min(order.len().saturating_sub(1))) {
                Some(next) => self.focus(next),
                None => self.show_welcome(),
            }
        }
        self.refresh_sidebar();
        self.save();
    }

    fn show_welcome(self: &Rc<Self>) {
        self.fill_welcome();
        self.chat_stack.set_visible_child_name("empty");
        self.files.show(&glib::home_dir(), &glib::home_dir(), &[]);
    }

    fn toggle_collapsed(self: &Rc<Self>, path: &Path) {
        if let Some(project) = self.projects.borrow_mut().iter_mut().find(|p| p.path == path) {
            project.collapsed = !project.collapsed;
        }
        self.refresh_sidebar();
        self.save();
    }

    /// Takes a project out of the sidebar, closing its agents (asks first if any is busy).
    fn remove_project(self: &Rc<Self>, path: &Path) {
        let agents: Vec<Rc<Agent>> = self.agents.borrow().iter().filter(|a| a.project == path).cloned().collect();
        let busy = agents.iter().filter(|a| a.is_busy()).count();
        let path = path.to_owned();
        let weak = Rc::downgrade(self);
        let go = move || {
            let Some(app) = weak.upgrade() else { return };
            let had_focus = agents.iter().any(|a| a.focused.get());
            for agent in &agents {
                app.remove_agent_then(agent, false);
            }
            app.projects.borrow_mut().retain(|p| p.path != path);
            if app.showing_project().is_some_and(|p| p == path) {
                app.show_welcome();
            }
            // Focus moves to what is left (only now, so no closing agent is started first).
            let next = if had_focus { app.agents_in_order().into_iter().next() } else { None };
            match next {
                Some(next) => app.focus(&next),
                None if app.agents.borrow().is_empty() => app.show_welcome(),
                None => {}
            }
            app.refresh_sidebar();
            app.save();
        };
        if busy == 0 {
            go();
            return;
        }
        let dialog = gtk::AlertDialog::builder()
            .message("Remove this project?")
            .detail(format!("{busy} of its agents {} working; removing stops them. Conversations are kept.", if busy == 1 { "is" } else { "are" }))
            .buttons(["Keep it", "Remove"])
            .cancel_button(0)
            .default_button(0)
            .build();
        dialog.choose(Some(&self.window), None::<&gio::Cancellable>, move |choice| {
            if let Ok(1) = choice {
                go();
            }
        });
    }

    fn agent_event(self: &Rc<Self>, agent: &Rc<Agent>, event: Event) {
        let looking = self.window.is_active() && (agent.focused.get() || self.is_pinned(agent));
        match event {
            Event::Changed => {
                self.toast_settled(agent);
                self.refresh_sidebar();
                self.refresh_inbox();
                if agent.focused.get() {
                    self.files.set_repos(agent.repos());
                }
            }
            Event::Session => self.save(),
            Event::Files => {
                if agent.focused.get() {
                    self.show_files(&agent);
                    if !self.files.root.is_visible() && !agent.files().is_empty() {
                        self.files.root.set_visible(true);
                    }
                }
            }
            Event::Asked(title) => {
                self.refresh_inbox();
                if !looking && settings::get().notify_questions {
                    if self.window.is_active() {
                        self.show_toast(agent, &format!("{} wants to: {title}", agent.title()), true);
                    } else {
                        self.notify(agent, &format!("Wants to: {title}"));
                    }
                }
            }
            Event::Done => {
                if !looking && settings::get().notify_done {
                    self.notify(agent, "Finished");
                }
                self.refresh_sidebar();
            }
        }
    }

    fn wire_toast(self: &Rc<Self>, go: &gtk::Button, close: &gtk::Button) {
        let weak = Rc::downgrade(self);
        go.connect_clicked(move |_| {
            let Some(app) = weak.upgrade() else { return };
            let agent = app.toast_agent.borrow().upgrade();
            app.hide_toast();
            if let Some(agent) = agent {
                app.focus(&agent);
            }
        });
        let weak = Rc::downgrade(self);
        close.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.hide_toast();
            }
        });
    }

    /// The window is in use but `agent` is out of view: say it is waiting, here.
    pub fn show_toast(self: &Rc<Self>, agent: &Rc<Agent>, text: &str, question: bool) {
        let waiting = self.toast_question.get()
            && self.toast.reveals_child()
            && self.toast_agent.borrow().upgrade().is_some_and(|a| a.status() == Status::Blocked);
        if waiting && !question {
            return;
        }
        self.toast_label.set_label(text);
        self.toast_question.set(question);
        *self.toast_agent.borrow_mut() = Rc::downgrade(agent);
        self.toast.set_reveal_child(true);
        if let Some(old) = self.toast_timeout.take() {
            old.remove();
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_seconds_local_once(12, move || {
            if let Some(app) = weak.upgrade() {
                app.toast_timeout.take();
                app.hide_toast();
            }
        });
        *self.toast_timeout.borrow_mut() = Some(source);
    }

    fn hide_toast(&self) {
        self.toast.set_reveal_child(false);
        *self.toast_agent.borrow_mut() = std::rc::Weak::new();
        if let Some(source) = self.toast_timeout.take() {
            source.remove();
        }
    }

    /// The toast is about `agent` and no longer true (answered, or now in view).
    fn toast_settled(&self, agent: &Rc<Agent>) {
        let about = self.toast_agent.borrow().upgrade().is_some_and(|a| Rc::ptr_eq(&a, agent));
        let answered = self.toast_question.get() && agent.status() != Status::Blocked;
        if about && (answered || agent.focused.get() || self.is_pinned(agent)) {
            self.hide_toast();
        }
    }

    fn notify(&self, agent: &Agent, body: &str) {
        let notification = gio::Notification::new(&agent.title());
        notification.set_body(Some(body));
        notification.set_default_action_and_target_value("app.focus-agent", Some(&agent.id.to_variant()));
        self.app.send_notification(Some(&format!("agent-{}", agent.id)), &notification);
    }

    fn suspend_idle(&self) {
        let minutes = std::env::var("LOOM_IDLE_MINUTES").ok().and_then(|m| m.parse().ok()).unwrap_or(settings::get().idle_minutes);
        if minutes == 0 {
            return;
        }
        let limit = Duration::from_secs(minutes * 60);
        let agents = self.agents.borrow().clone();
        for agent in agents {
            let in_view = agent.focused.get() || self.is_pinned(&agent);
            if agent.status() == Status::Idle && !in_view && agent.last_active.get().elapsed() > limit {
                agent.suspend();
            }
        }
    }

    fn save(&self) {
        let order = self.agents_in_order();
        // On a project's page, the agent that had focus before it.
        let focused = self.focused.borrow().clone().or_else(|| self.previous.borrow().clone());
        let focused = focused.and_then(|f| order.iter().position(|a| Rc::ptr_eq(a, &f))).unwrap_or(0);
        let pinned = self.pinned.borrow().as_ref().and_then(|p| order.iter().position(|a| Rc::ptr_eq(a, p)));
        state::save(
            &self.app_id,
            &state::Layout {
                projects: self.projects.borrow().clone(),
                agents: order.iter().map(|a| a.saved()).collect(),
                focused,
                pinned,
                closed: self.closed.borrow().clone(),
            },
        );
    }

    fn close_request(self: &Rc<Self>) -> glib::Propagation {
        // Only a turn in flight is lost by quitting (an agent still starting just restarts).
        let busy: Vec<String> =
            self.agents.borrow().iter().filter(|a| matches!(a.status(), Status::Working | Status::Blocked)).map(|a| a.title()).collect();
        let ask = (!busy.is_empty() && settings::get().confirm_quit) || self.files.has_unsaved();
        if !self.quitting.get() && ask {
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
        if let Some(socket) = crate::control::current() {
            let _ = std::fs::remove_file(socket);
        }
        glib::Propagation::Proceed
    }

    // ---- what agents ask of loom (its MCP tools) ------------------------------------------

    fn control(self: &Rc<Self>, id: u32, method: &str, params: &Value) -> Result<Value, String> {
        let agent = self.agent_by_id(id);
        let tray = || agent.as_ref().map(|a| a.tray.clone()).ok_or_else(|| "this agent is no longer open in loom".to_owned());
        let name = || params["name"].as_str().filter(|n| !n.is_empty()).map(str::to_owned).ok_or("a job name is needed");
        let job = |name: &str| tray().and_then(|t| t.named(name).ok_or_else(|| format!("no job named {name}")));
        match method {
            "run_job" => {
                let name = name()?;
                let command = params["command"].as_str().filter(|c| !c.trim().is_empty()).ok_or("a command is needed")?;
                // A folder of its own: a repo inside the project, say (relative to the agent's).
                let cwd = match params["cwd"].as_str().filter(|c| !c.is_empty()) {
                    Some(dir) => {
                        let base = agent.as_ref().map(|a| a.cwd.clone()).unwrap_or_default();
                        let dir = base.join(crate::state::expand(Path::new(dir)));
                        if !dir.is_dir() {
                            return Err(format!("{} is not a folder", dir.display()));
                        }
                        Some(dir)
                    }
                    None => None,
                };
                tray()?.run_named(&name, command, params["restart"].as_bool().unwrap_or(false), cwd)?;
                Ok(json!({ "name": name }))
            }
            "job_output" => {
                let name = name()?;
                let job = job(&name)?;
                let lines = params["lines"].as_u64().unwrap_or(60).clamp(1, 2000) as usize;
                Ok(json!({ "name": name, "running": job.is_running(), "exit_code": job.exit_code(), "output": job.output(lines) }))
            }
            "stop_job" => {
                job(&name()?)?.stop();
                Ok(json!({}))
            }
            "list_jobs" => Ok(json!(
                tray()?.list().into_iter().map(|(n, r, c)| json!({ "name": n, "running": r, "exit_code": c })).collect::<Vec<_>>()
            )),
            "open_file" => {
                let agent = agent.ok_or("this agent is no longer open in loom")?;
                let path = Path::new(params["path"].as_str().ok_or("a path is needed")?);
                let path = &crate::state::expand(path);
                let path = if path.is_absolute() { path.to_owned() } else { agent.cwd.join(path) };
                if !path.is_file() {
                    return Err(format!("{} is not a file", path.display()));
                }
                // Shown without taking the keyboard from what the user is doing.
                let focus = GtkWindowExt::focus(&self.window);
                let opened = self.files.open_for_agent(&path);
                if let Some(widget) = focus {
                    widget.grab_focus();
                }
                opened.map(|()| json!({}))
            }
            "notify" => {
                let agent = agent.ok_or("this agent is no longer open in loom")?;
                let message = params["message"].as_str().unwrap_or("Needs you");
                let looking = self.window.is_active() && (agent.focused.get() || self.is_pinned(&agent));
                if !self.window.is_active() {
                    self.notify(&agent, message);
                } else if !looking {
                    self.show_toast(&agent, &format!("{}: {message}", agent.title()), false);
                }
                Ok(json!({}))
            }
            "list_agents" => Ok(json!(
                self.agents_in_order()
                    .iter()
                    .map(|a| json!({ "title": a.title(), "project": a.project.to_string_lossy(), "status": a.status().css(), "you": a.id == id }))
                    .collect::<Vec<_>>()
            )),
            other => Err(format!("unknown request {other}")),
        }
    }

    // ---- sidebar, menus, inbox, welcome ---------------------------------------------------

    /// The pane with the keyboard shows it; with two side by side, the other one recedes.
    fn mark_active(&self) {
        let focused = self.focused.borrow().clone();
        let pinned = self.pinned.borrow().clone();
        for agent in self.agents.borrow().iter() {
            let root = &agent.chat.root;
            root.remove_css_class("active");
            root.remove_css_class("inactive");
            if pinned.is_some() {
                let is_focused = focused.as_ref().is_some_and(|f| Rc::ptr_eq(f, agent));
                root.add_css_class(if is_focused { "active" } else { "inactive" });
            }
        }
    }

    fn activate_row(self: &Rc<Self>, index: usize) {
        enum Do {
            Focus(Rc<Agent>),
            Start(PathBuf),
        }
        let what = match self.rows.borrow().get(index) {
            Some(Row::Project(path)) => Do::Start(path.clone()),
            Some(Row::Agent(agent)) => Do::Focus(agent.clone()),
            Some(Row::Empty(path)) => Do::Start(path.clone()),
            None => return,
        };
        match what {
            Do::Focus(agent) => self.focus(&agent),
            Do::Start(path) => self.show_project(&path, &path),
        }
    }

    /// The right-click menu of row `index`, at (x, y) in the list.
    fn row_menu(self: &Rc<Self>, index: usize, x: f64, y: f64) {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let weak = Rc::downgrade(self);
        let item = |label: &str, f: Box<dyn Fn(&Rc<App>)>| {
            let button = gtk::Button::with_label(label);
            button.add_css_class("flat");
            button.add_css_class("menu-item");
            if let Some(child) = button.child().and_downcast::<gtk::Label>() {
                child.set_xalign(0.0);
            }
            let weak = weak.clone();
            button.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.menu.popdown();
                    f(&app);
                }
            });
            content.append(&button);
        };
        match self.rows.borrow().get(index) {
            Some(Row::Project(path)) | Some(Row::Empty(path)) => {
                let (p1, p2, p3, p4, p5) = (path.clone(), path.clone(), path.clone(), path.clone(), path.clone());
                item("Conversations (new or past)", Box::new(move |app| app.show_project(&p1, &p1)));
                item("Rename…", Box::new(move |app| app.rename_project_at(&p5, x, y)));
                item("Scratch terminal here", Box::new(move |app| {
                    app.scratch.hide();
                    app.scratch.toggle(&p2);
                }));
                item("Collapse / expand", Box::new(move |app| app.toggle_collapsed(&p3)));
                item("Remove from the sidebar", Box::new(move |app| app.remove_project(&p4)));
            }
            Some(Row::Agent(agent)) => {
                let target = Rc::downgrade(agent);
                let with = |f: fn(&Rc<App>, &Rc<Agent>)| -> Box<dyn Fn(&Rc<App>)> {
                    let target = target.clone();
                    Box::new(move |app| {
                        if let Some(agent) = target.upgrade() {
                            f(app, &agent);
                        }
                    })
                };
                item("Rename…", with(|app, agent| {
                    app.focus(agent);
                    app.begin_rename();
                }));
                item("Show beside the focused one", with(|app, agent| {
                    app.pin(agent);
                    app.save();
                }));
                item("Restart", with(|_, agent| agent.restart()));
                item("Suspend (free its memory)", with(|_, agent| {
                    agent.suspend();
                }));
                item("Close", with(|app, agent| app.close_agent(agent)));
            }
            None => return,
        }
        self.menu.set_child(Some(&content));
        self.menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        self.menu.popup();
    }

    fn refresh_sidebar(self: &Rc<Self>) {
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let mut rows = Vec::new();
        let focused = self.focused.borrow().clone();
        let projects = self.projects_in_order();
        let agents = self.agents_in_order();
        for project in &projects {
            let mine: Vec<&Rc<Agent>> = agents.iter().filter(|a| a.project == project.path).collect();
            self.list.append(&self.project_row(project, &mine));
            rows.push(Row::Project(project.path.clone()));
            if project.collapsed {
                continue;
            }
            if mine.is_empty() {
                let empty = gtk::Label::builder().label("No agents open · Enter: new or past conversation").xalign(0.0).build();
                empty.add_css_class("dim");
                empty.add_css_class("small");
                let row = gtk::ListBoxRow::builder().child(&empty).selectable(false).build();
                row.add_css_class("empty-row");
                self.list.append(&row);
                rows.push(Row::Empty(project.path.clone()));
            }
            for agent in mine {
                let row = agent_row(agent);
                self.list.append(&row);
                if focused.as_ref().is_some_and(|f| Rc::ptr_eq(f, agent)) {
                    self.list.select_row(Some(&row));
                }
                rows.push(Row::Agent(agent.clone()));
            }
        }
        if projects.is_empty() {
            let hint = gtk::Label::builder()
                .label("No projects yet.\nCtrl+Shift+N starts an agent in a folder.")
                .xalign(0.0)
                .wrap(true)
                .build();
            hint.add_css_class("dim");
            hint.add_css_class("small");
            hint.add_css_class("sidebar-empty");
            let row = gtk::ListBoxRow::builder().child(&hint).selectable(false).activatable(false).build();
            self.list.append(&row);
        }
        *self.rows.borrow_mut() = rows;
        if agents.is_empty() {
            self.fill_welcome();
        }
        self.mark_active();
    }

    /// A project's header: open/closed arrow, name, what its agents are up to, and +.
    fn project_row(self: &Rc<Self>, project: &Project, agents: &[&Rc<Agent>]) -> gtk::ListBoxRow {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let arrow = gtk::Button::with_label(if project.collapsed { "▸" } else { "▾" });
        arrow.add_css_class("flat");
        arrow.add_css_class("arrow");
        arrow.set_tooltip_text(Some(if project.collapsed { "Show its agents" } else { "Hide its agents" }));
        let weak = Rc::downgrade(self);
        let path = project.path.clone();
        arrow.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.toggle_collapsed(&path);
            }
        });
        let name = gtk::Label::builder().label(self.project_label(&project.path)).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
        name.add_css_class("project-name");
        line.append(&arrow);
        line.append(&name);
        let waiting = agents.iter().filter(|a| a.status() == Status::Blocked).count();
        let working = agents.iter().filter(|a| matches!(a.status(), Status::Working | Status::Starting)).count();
        if waiting > 0 {
            let badge = gtk::Label::new(Some(&format!("⚑ {waiting}")));
            badge.add_css_class("badge-wait");
            badge.set_tooltip_text(Some("waiting for you"));
            line.append(&badge);
        }
        if working > 0 {
            let badge = gtk::Label::new(Some(&format!("● {working}")));
            badge.add_css_class("badge-busy");
            badge.set_tooltip_text(Some("working"));
            line.append(&badge);
        }
        if project.collapsed && !agents.is_empty() {
            let count = gtk::Label::new(Some(&format!("({})", agents.len())));
            count.add_css_class("dim");
            count.add_css_class("small");
            count.set_tooltip_text(Some("agents in it"));
            line.append(&count);
        }
        let add = gtk::Button::with_label("+");
        add.add_css_class("flat");
        add.add_css_class("add");
        add.set_tooltip_text(Some(&format!("New or past conversation in {}", self.project_label(&project.path))));
        let weak = Rc::downgrade(self);
        let path = project.path.clone();
        add.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.show_project(&path, &path);
            }
        });
        line.append(&add);
        let row = gtk::ListBoxRow::builder().child(&line).selectable(false).build();
        row.add_css_class("project-row");
        row.set_tooltip_text(Some(&project.path.to_string_lossy()));
        row
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
            let target = Rc::downgrade(&agent);
            go.connect_clicked(move |_| {
                if let (Some(app), Some(target)) = (weak.upgrade(), target.upgrade()) {
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
                // A question form is answered in the chat.
                if agent.is_form(serial) {
                    let open = gtk::Button::with_label("Answer in the chat");
                    open.add_css_class("suggested");
                    let target = Rc::downgrade(&agent);
                    let weak = Rc::downgrade(self);
                    open.connect_clicked(move |_| {
                        if let (Some(app), Some(target)) = (weak.upgrade(), target.upgrade()) {
                            app.inbox.popdown();
                            app.focus(&target);
                        }
                    });
                    buttons.append(&open);
                }
                for (oi, (name, kind)) in options.iter().enumerate() {
                    let button = gtk::Button::with_label(name);
                    button.add_css_class(if kind.starts_with("allow") { "suggested" } else { "plain" });
                    let target = Rc::downgrade(&agent);
                    let weak = Rc::downgrade(self);
                    button.connect_clicked(move |_| {
                        if let Some(target) = target.upgrade() {
                            target.answer_serial(serial, oi);
                        }
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

    /// The page shown with no agent open: the mark, the main actions, recent projects.
    fn fill_welcome(self: &Rc<Self>) {
        while let Some(child) = self.welcome.first_child() {
            self.welcome.remove(&child);
        }
        let middle = gtk::Box::new(gtk::Orientation::Vertical, 10);
        middle.set_valign(gtk::Align::Center);
        middle.set_halign(gtk::Align::Center);
        middle.set_vexpand(true);
        middle.add_css_class("welcome");
        let mark = crate::logo::mark(84);
        mark.set_halign(gtk::Align::Center);
        middle.append(&mark);
        let title = gtk::Label::new(Some("loom"));
        title.add_css_class("welcome-title");
        middle.append(&title);
        let tagline = gtk::Label::new(Some("Your Claude Code agents, side by side."));
        tagline.add_css_class("dim");
        middle.append(&tagline);

        let actions = gtk::Box::new(gtk::Orientation::Vertical, 6);
        actions.add_css_class("welcome-actions");
        let weak = Rc::downgrade(self);
        let action = |label: &str, keys: &str, f: Box<dyn Fn(&Rc<App>)>| {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let text = gtk::Label::builder().label(label).xalign(0.0).hexpand(true).build();
            let keys = gtk::Label::new(Some(keys));
            keys.add_css_class("keycap");
            line.append(&text);
            line.append(&keys);
            let button = gtk::Button::builder().child(&line).build();
            button.add_css_class("welcome-action");
            let weak = weak.clone();
            button.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    f(&app);
                }
            });
            actions.append(&button);
        };
        action("Open a project folder…", "Ctrl+Shift+N", Box::new(|a| a.new_agent(true)));
        if !self.closed.borrow().is_empty() {
            action("Reopen the last closed agent", "Ctrl+Shift+T", Box::new(|a| a.reopen_closed()));
        }
        action("Command palette", "Ctrl+Shift+P", Box::new(|a| a.palette()));
        action("Settings", "Ctrl+,", Box::new(|a| crate::prefs::open(&a.window)));
        middle.append(&actions);

        let projects = self.projects.borrow().clone();
        if !projects.is_empty() {
            let heading = gtk::Label::builder().label("Projects").xalign(0.0).build();
            heading.add_css_class("welcome-heading");
            middle.append(&heading);
            for project in projects.iter().take(6) {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                let name = gtk::Label::builder().label(self.project_label(&project.path)).xalign(0.0).build();
                let place = gtk::Label::builder()
                    .label(project.path.to_string_lossy().replace(&*glib::home_dir().to_string_lossy(), "~"))
                    .xalign(1.0)
                    .hexpand(true)
                    .ellipsize(pango::EllipsizeMode::Start)
                    .build();
                place.add_css_class("dim");
                place.add_css_class("small");
                line.append(&name);
                line.append(&place);
                let button = gtk::Button::builder().child(&line).build();
                button.add_css_class("welcome-project");
                let weak = Rc::downgrade(self);
                let path = project.path.clone();
                button.connect_clicked(move |_| {
                    if let Some(app) = weak.upgrade() {
                        app.show_project(&path, &path);
                    }
                });
                middle.append(&button);
            }
        }
        self.welcome.append(&middle);
    }
}

fn agent_row(agent: &Rc<Agent>) -> gtk::ListBoxRow {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("dot");
    let status = agent.status();
    dot.add_css_class(if status == Status::Idle && agent.unseen.get() { "done" } else { status.css() });
    dot.set_valign(gtk::Align::Center);
    let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
    names.set_hexpand(true);
    let title = gtk::Label::builder().label(agent.title()).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
    title.add_css_class("agent-title");
    names.append(&title);
    let state = match status {
        Status::Idle if agent.unseen.get() => "done".to_owned(),
        Status::Blocked => "needs you".to_owned(),
        Status::Idle => String::new(),
        other => other.describe().to_owned(),
    };
    // Where it works: the repo it last touched (name when it isn't the project itself), its
    // branch and changes; else the subfolder it runs in.
    let place = match agent.repo() {
        Some(repo) if repo.path == agent.project => repo.summary(),
        Some(repo) => format!("{} · {}", repo.name(), repo.summary()),
        None => agent.cwd.strip_prefix(&agent.project).ok().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
    };
    let sub = [state, place].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    if !sub.is_empty() {
        let sub = gtk::Label::builder().label(sub).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        sub.add_css_class("dim");
        sub.add_css_class("small");
        names.append(&sub);
    }
    line.append(&dot);
    line.append(&names);
    let row = gtk::ListBoxRow::builder().child(&line).build();
    row.add_css_class("agent-row");
    row
}

fn is_modifier(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::ISO_Level3_Shift
    )
}

/// The repository a folder belongs to (the nearest parent with .git), else the folder.
pub fn project_of(cwd: &Path) -> PathBuf {
    cwd.ancestors().find(|dir| dir.join(".git").exists()).unwrap_or(cwd).to_owned()
}

fn state_project(cwd: &Path) -> PathBuf {
    project_of(cwd)
}

fn project_name(project: &Path) -> String {
    project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| project.to_string_lossy().into_owned())
}

fn help_text() -> gtk::Box {
    let grid = gtk::Grid::builder().column_spacing(18).row_spacing(4).build();
    grid.add_css_class("help");
    for (i, (key, what)) in crate::prefs::KEYS.iter().enumerate() {
        let key = gtk::Label::builder().label(*key).xalign(1.0).build();
        key.add_css_class("keycap");
        let what = gtk::Label::builder().label(*what).xalign(0.0).wrap(true).max_width_chars(48).build();
        grid.attach(&key, 0, i as i32, 1, 1);
        grid.attach(&what, 1, i as i32, 1, 1);
    }
    let scroller = gtk::ScrolledWindow::builder().child(&grid).propagate_natural_height(true).max_content_height(560).hscrollbar_policy(gtk::PolicyType::Never).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&scroller);
    content
}
