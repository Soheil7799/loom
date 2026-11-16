//! The window: projects and their agents down the left, the focused agent's chat in the
//! middle (a second one beside it on demand), its files on the right, the project's job
//! tray along the bottom, and the scratchpad and pickers floating over it all.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

/// Untouched this long, a window in front counts as away (`set_looking`).
const IDLE_AWAY: Duration = Duration::from_secs(180);

thread_local! {
    /// The user's last key, click or scroll in the window.
    static LAST_INPUT: Cell<std::time::Instant> = Cell::new(std::time::Instant::now());
}

use gtk::{gdk, gio, glib, pango, prelude::*};
use serde_json::{Value, json};

use crate::feed::{Feed, SideAgent, SideProject};
use crate::agent::Status;
use crate::files::Files;
use crate::jobs::Tray;
use crate::picker::{Choice, Picker};
use crate::schedule::{self, Schedule};
use crate::scratch::Scratch;
use crate::settings;
use crate::state::{self};

/// What a sidebar row stands for.
#[derive(Clone)]
enum Row {
    Project(PathBuf),
    /// One of loomd's agents (the window as its client).
    Remote(u32),
}

pub struct App {
    app: gtk::Application,
    app_id: String,
    pub window: gtk::ApplicationWindow,
    /// Itself (for what is called with only a reference).
    me: RefCell<std::rc::Weak<App>>,
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
    board: Rc<crate::board::BoardView>,
    /// Every agent on one page, and the scheduled prompts.
    overview: Rc<crate::overview::Overview>,
    pinned_box: gtk::Box,
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
    quitting: Cell<bool>,
    /// The Notes view: its chats (the sidebar's page and the middle), its panel on the right,
    /// its button beside Overview (a dot on it: a chat waits for you), and the way back.
    notes_chat: Rc<crate::notes_chat::NotesChat>,
    notes_panel: Rc<crate::notes_panel::NotesPanel>,
    notes_button: gtk::ToggleButton,
    notes_dot: gtk::Box,
    notes_back: gtk::Button,
    notes_mode: Cell<bool>,
    /// The agents' side as the Notes view found it (put back when leaving), and the note open
    /// in the Notes view (back when it is shown again).
    agents_side: RefCell<AgentsSide>,
    notes_file: RefCell<(Option<PathBuf>, bool)>,
    notes_tabs: RefCell<Vec<PathBuf>>,
    /// The sidebar's list: the agents', or the Notes chats'.
    side_stack: gtk::Stack,
    split: gtk::Paned,
    /// Ctrl+B was pressed: the next key is a command (herdr's prefix).
    prefix: Cell<bool>,
    prefix_hint: gtk::Label,
    prefix_timeout: RefCell<Option<glib::SourceId>>,
    files_were_open: Cell<bool>,
    editor_was_open: Cell<bool>,
    /// The chat text size (from the settings).
    scale_css: gtk::CssProvider,
    /// "<agent> needs you", over the window while that agent is out of view.
    toast: gtk::Revealer,
    toast_label: gtk::Label,
    toast_go: gtk::Button,
    /// The toast is a question (it goes once answered).
    toast_question: Cell<bool>,
    toast_timeout: RefCell<Option<glib::SourceId>>,
    /// An overview refresh is due (several changes at once make one).
    overview_due: Cell<bool>,
    /// The files panel was open when the overview was shown (it opens again with an agent).
    files_hidden_for_overview: Cell<bool>,
    /// The Notes add-on: Scribe's checks and the Notes tab.
    pub hub: Rc<crate::hub::Hub>,
    /// The running agents in the Running list's order (ids): one that starts running joins at
    /// the end; dragging reorders.
    running: RefCell<Vec<u32>>,
    /// The Running list's rows, in order (for its drop lines).
    running_rows: RefCell<Vec<gtk::ListBoxRow>>,
    /// The Running section's list (its rows in `running`'s order) and the sections' parts.
    run_list: gtk::ListBox,
    run_scroller: gtk::ScrolledWindow,
    list_scroller: gtk::ScrolledWindow,
    run_head: SectionHead,
    proj_head: SectionHead,
    sections_filler: gtk::Box,
    /// Running's tallest while Projects is open (half the sidebar), in pixels.
    run_cap: Cell<i32>,
    /// What the sidebar draws: its copies of the core's `agents` and `projects`.
    sidebar_feeds: RefCell<Option<(Feed, Feed)>>,
    /// Something new waits in them: drawn once the window is idle.
    sidebar_due: Cell<bool>,
    /// Project icons drawn, by project and dark or light.
    icons: RefCell<HashMap<(PathBuf, bool), PathBuf>>,
    /// A redraw is due for project icons still looked for.
    icons_due: Cell<bool>,
    /// What the sidebar's rows were last built from: `agents` changing only in what they
    /// don't show (a file edited, the options) doesn't rebuild them under the pointer.
    sidebar_drawn: RefCell<Option<(Vec<SideAgent>, Vec<SideProject>, Vec<(u32, String)>)>>,
    /// Each project's first and last sidebar rows (where a drop line shows).
    project_edges: RefCell<HashMap<PathBuf, (gtk::ListBoxRow, gtk::ListBoxRow)>>,
    /// Open projects' cards (name, first and last agent row): their names drawn up the left.
    cards: RefCell<Vec<(PathBuf, String, gtk::ListBoxRow, gtk::ListBoxRow)>>,
    /// Over the agent list: each open card's name (a button that folds it).
    card_layer: gtk::Overlay,
    card_spots: CardSpots,
    /// loomd's agents opened in the window (its client), and the one in view.
    remote_agents: RefCell<HashMap<u32, Rc<crate::remote_agent::RemoteAgent>>>,
    remote_focused: Cell<Option<u32>>,
    /// loomd's agents' statuses as last drawn (a change may be worth a toast), and the agent
    /// of loomd's the toast is about.
    remote_status: RefCell<HashMap<u32, String>>,
    toast_remote: Cell<Option<u32>>,
    /// loomd's `asks`, followed: the ones acted on, whether the first came, and the edit the
    /// files panel waits on (its number).
    asks_feed: RefCell<Option<Feed>>,
    /// Jobs an agent brings forward for the user (a login): each `n` seen, by agent.
    shown_feed: RefCell<Option<Feed>>,
    shown_seen: RefCell<HashMap<String, u64>>,
    shown_started: Cell<bool>,
    /// Web pages an agent's MCP server asked to open: each `n` seen, by agent.
    uris_feed: RefCell<Option<Feed>>,
    uris_seen: RefCell<HashMap<String, u64>>,
    uris_started: Cell<bool>,
    /// loomd's settings: this window's copy follows them (a change made in Settings, here or
    /// elsewhere, reaches what the window draws).
    settings_feed: RefCell<Option<Feed>>,
    asks_seen: RefCell<std::collections::HashSet<String>>,
    asks_started: Cell<bool>,
    asking_edit: Cell<Option<u64>>,
    /// loomd's `overview` (cards' questions and replies) and `schedules`, followed.
    overview_feeds: RefCell<Option<(Feed, Feed)>>,
    /// The footer's sign of loomd (the window as its client): the button, its dot, its words.
    loomd_sign: (gtk::Button, gtk::Box, gtk::Label),
    /// loomd's `closed` (Ctrl+Shift+T reopens the newest).
    closed_feed: RefCell<Option<Feed>>,
    /// loomd's agent shown beside the one in view (Ctrl+\\), and the one in view before.
    pinned_remote: Cell<Option<u32>>,
    previous_remote: Cell<Option<u32>>,
    /// The agent the project page was opened from (its back button).
    back_to: Cell<Option<u32>>,
    /// The first view after start was chosen (the overview or the last agent).
    started: Cell<bool>,
    /// The agent whose files the files panel shows, and how many it had then.
    fed_files: Cell<(u32, usize)>,
    /// A copy was asked of loomd (rewind, fork): the next new agent it lists is shown.
    expect_copy: Cell<bool>,
}

/// What the agents' side showed when the Notes view opened.
#[derive(Default)]
struct AgentsSide {
    /// An agent was in view (else the overview or a page).
    agent: bool,
    files: bool,
    pinned: bool,
    bar: bool,
    dock: bool,
    /// The file open, and whether the editor showed; the editor's tabs.
    file: (Option<PathBuf>, bool),
    tabs: Vec<PathBuf>,
}

/// loomd at `socket` stopped now, before the window goes (a blocking call: one sent the
/// usual way could still wait to be written when the process ends).
fn stop_loomd_now(socket: &Path) -> Result<(), String> {
    let mut connection = loom_core::api::cli::Connection::open(socket).map_err(|e| e.message)?;
    let asks = connection.hello["capabilities"]["commands"].as_array().is_some_and(|c| c.iter().any(|c| c == "loomd_stop"));
    if asks {
        return connection.command(&loom_core::rt::uuid(), "loomd_stop", json!({})).map(|_| ()).map_err(|e| e.message);
    }
    let pid = loom_core::api::socket::server_pid(socket).ok_or("loomd's process not found")?;
    stop_old_loomd(pid)
}

/// An option's button in the inbox: a long one ("Yes, and don't ask again for …") cut, whole
/// in its tooltip (the popover stays within the window).
fn inbox_option(name: &str) -> gtk::Button {
    let label = gtk::Label::builder().label(name).ellipsize(pango::EllipsizeMode::End).max_width_chars(28).build();
    gtk::Button::builder().child(&label).tooltip_text(name).build()
}

/// loomd process `pid` (one older than `loomd_stop`) told to stop, as SIGTERM does: only if
/// it is a loomd (the socket's other end could be loom's own window, which isn't stopped so).
fn stop_old_loomd(pid: u32) -> Result<(), String> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|e| format!("loomd's process: {e}"))?;
    if !exe.file_name().is_some_and(|n| n.to_string_lossy().starts_with("loomd")) {
        return Err(format!("{} answers there, not loomd: close it first", exe.display()));
    }
    // SAFETY: a signal to a process of this user's, checked above to be loomd.
    if unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0 {
        return Err(format!("loomd ({pid}) couldn't be stopped: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// loomd's `hello` against this window (version `mine`): None if they match; else how many of
/// the commands this window uses loomd lacks (0: only the version differs).
fn loomd_differs(hello: &Value, mine: &str) -> Option<usize> {
    let has: Vec<&str> = hello["capabilities"]["commands"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let missing = loom_core::api::spec::COMMANDS.iter().filter(|c| !has.contains(&c.name)).count();
    (hello["server"]["version"].as_str() != Some(mine) || missing > 0).then_some(missing)
}

impl App {
    pub fn new(app: &gtk::Application) -> Rc<App> {
        let app_id = app.application_id().map(|id| id.to_string()).unwrap_or_default();
        // As it was left (its size, maximized); the first time, loom's own size.
        let kept = window_state(&app_id);
        let (width, height) = match (kept["width"].as_i64(), kept["height"].as_i64()) {
            (Some(w), Some(h)) if w > 200 && h > 200 => (w as i32, h as i32),
            _ => (1360, 860),
        };
        let window = gtk::ApplicationWindow::builder().application(app).title("loom").default_width(width).default_height(height).build();
        if kept["maximized"] == true {
            window.maximize();
        }
        window.add_css_class("loom");

        // Left: projects and their agents.
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar.add_css_class("sidebar");
        sidebar.set_size_request(290, -1);
        // Children that expand must not widen it.
        sidebar.set_hexpand(false);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        top.add_css_class("sidebar-top");
        // The lockup: the wordmark over who made it; across the row up to the buttons. It
        // opens About.
        let lockup = gtk::Box::new(gtk::Orientation::Vertical, 0);
        lockup.set_valign(gtk::Align::Center);
        let wordmark = crate::logo::wordmark(40);
        wordmark.set_halign(gtk::Align::Start);
        let by = gtk::Label::builder().label("by shirvani.dev").xalign(0.0).build();
        by.add_css_class("brand-by");
        lockup.append(&wordmark);
        lockup.append(&by);
        let brand = gtk::Button::builder().child(&lockup).tooltip_text("About Loom").hexpand(true).focus_on_click(false).build();
        brand.update_property(&[gtk::accessible::Property::Label("About Loom")]);
        brand.add_css_class("flat");
        brand.add_css_class("brand-button");
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
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
        let overview_button = gtk::Button::from_icon_name("view-grid-symbolic");
        overview_button.update_property(&[gtk::accessible::Property::Label("Overview")]);
        overview_button.add_css_class("flat");
        overview_button.add_css_class("overview-button");
        overview_button.set_tooltip_text(Some("Overview: every agent, and scheduled prompts (Ctrl+Shift+A)"));
        // The Notes view (while Notes is on): a dot on it when a chat waits for you.
        let notes_button = gtk::ToggleButton::builder().icon_name("accessories-text-editor-symbolic").tooltip_text("Notes: chat with your notes, their topics and history (Ctrl+Shift+M)").build();
        notes_button.update_property(&[gtk::accessible::Property::Label("Notes")]);
        notes_button.add_css_class("flat");
        notes_button.add_css_class("overview-button");
        let notes_dot = gtk::Box::builder().halign(gtk::Align::End).valign(gtk::Align::Start).can_target(false).visible(false).build();
        notes_dot.add_css_class("notes-dot");
        let notes_place = gtk::Overlay::builder().child(&notes_button).valign(gtk::Align::Center).visible(crate::vault::current().is_some()).build();
        notes_place.add_overlay(&notes_dot);
        top.append(&brand);
        top.append(&spacer);
        // Waiting for you: a band under the lockup (shown only then); it opens the questions.
        top.append(&notes_place);
        top.append(&overview_button);
        sidebar.append(&top);
        inbox.set_hexpand(true);
        inbox.add_css_class("inbox-banner");
        sidebar.append(&inbox);
        // Away mode on: a band saying until when; a click ends it.
        let away = gtk::Button::builder().visible(false).build();
        away.add_css_class("inbox-banner");
        away.add_css_class("away-banner");
        away.set_tooltip_text(Some("Claude's own questions are answered \"decide yourself, note it on the board\". Permissions still wait for you. Click to end."));
        away.connect_clicked(|_| crate::conn::command("window", "set_settings", json!({ "changes": { "away_until": 0 } })));
        sidebar.append(&away);
        let show_away = move |s: &settings::Settings| {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            away.set_visible(s.away_until > now);
            away.set_label(&format!("Away until {}: agents decide · End", settings::clock(s.away_until)));
        };
        show_away(&settings::get());
        settings::on_change(show_away);
        brand.connect_clicked(|button| {
            if let Some(window) = button.root().and_downcast::<gtk::Window>() {
                crate::about::open(&window);
            }
        });
        let list = gtk::ListBox::new();
        list.add_css_class("agents");
        let menu = gtk::Popover::builder().has_arrow(false).position(gtk::PositionType::Bottom).build();
        menu.add_css_class("menu");
        menu.set_parent(&list);
        // An open project's name runs up its card's left edge, drawn over the list (its rows stay
        // the list's own: keys, menus and drag work as on any row).
        let with_names = gtk::Overlay::builder().child(&list).build();
        let card_spots: CardSpots = Rc::default();
        // each over its card's left edge, its whole height
        let (spots, under) = (card_spots.clone(), list.clone());
        with_names.connect_get_child_position(move |_, widget| {
            let spots = spots.borrow();
            let (_, first, last) = spots.iter().find(|s| s.0 == *widget)?;
            let (top, bottom) = (first.compute_bounds(&under)?, last.compute_bounds(&under)?);
            let height = bottom.y() + bottom.height() - top.y() - 12.0;
            Some(gdk::Rectangle::new(top.x() as i32 + 2, top.y() as i32 + 6, CARD_STRIP as i32, height.max(1.0) as i32))
        });
        let list_scroller = gtk::ScrolledWindow::builder().child(&with_names).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        // Two sections, folded from their headers (as VS Code's): Running, sized to its rows up
        // to half the sidebar, and the projects under it.
        let run_list = gtk::ListBox::new();
        run_list.add_css_class("agents");
        run_list.add_css_class("running-list");
        let none = gtk::Label::builder().label("Nothing running").xalign(0.0).valign(gtk::Align::Start).build();
        none.add_css_class("dim");
        none.add_css_class("small");
        none.add_css_class("running-none");
        run_list.set_placeholder(Some(&none));
        let run_scroller = gtk::ScrolledWindow::builder().child(&run_list).hscrollbar_policy(gtk::PolicyType::Never).propagate_natural_height(true).build();
        run_scroller.add_css_class("running-pane");
        let run_head = SectionHead::new("Running", true);
        let proj_head = SectionHead::new("Projects", false);
        let sections_filler = gtk::Box::builder().vexpand(true).visible(false).build();
        let sections = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sections.append(&run_head.root);
        sections.append(&run_scroller);
        sections.append(&proj_head.root);
        sections.append(&list_scroller);
        sections.append(&sections_filler);
        let notes_chat = crate::notes_chat::NotesChat::new();
        // Back to the agents, with how many work and wait meanwhile.
        let notes_back = gtk::Button::builder().child(&gtk::Label::builder().label("←  Agents").xalign(0.0).ellipsize(pango::EllipsizeMode::End).build()).tooltip_text("Back to the agents (Ctrl+Shift+M)").build();
        notes_back.add_css_class("flat");
        notes_back.add_css_class("notes-back");
        notes_chat.side.append(&notes_back);
        let side_stack = gtk::Stack::builder().vexpand(true).build();
        side_stack.add_named(&sections, Some("agents"));
        side_stack.add_named(&notes_chat.side, Some("notes"));
        sidebar.append(&side_stack);
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        bottom.add_css_class("sidebar-bottom");
        // Small buttons, each with its key in the tooltip: new project, the scratch terminal and
        // a command in the tray on the left; settings and the shortcuts on the right.
        let small = |label: &str, tip: &str, name: &str| {
            let button = gtk::Button::builder().label(label).tooltip_text(tip).build();
            button.update_property(&[gtk::accessible::Property::Label(name)]);
            button.add_css_class("flat");
            button.add_css_class("icon");
            button
        };
        let new_button = small("", "New project: an agent in another folder (Ctrl+Shift+N)", "New project");
        new_button.set_icon_name("list-add-symbolic");
        let scratch_button = gtk::Button::from_icon_name("utilities-terminal-symbolic");
        scratch_button.update_property(&[gtk::accessible::Property::Label("Scratch terminal")]);
        scratch_button.add_css_class("flat");
        scratch_button.add_css_class("icon");
        scratch_button.set_tooltip_text(Some("Scratch terminal (Ctrl+`)"));
        let run_button = small("", "Run a command in the tray (Ctrl+J)", "Run a command");
        run_button.set_icon_name("media-playback-start-symbolic");
        let spacer = gtk::Box::builder().hexpand(true).build();
        // loomd's client: a sign that loomd runs the agents (and how it is).
        let loomd_dot = gtk::Box::builder().valign(gtk::Align::Center).build();
        loomd_dot.add_css_class("dot");
        let loomd_label = gtk::Label::builder().label("loomd").ellipsize(pango::EllipsizeMode::End).max_width_chars(14).build();
        loomd_label.add_css_class("small");
        let loomd_inner = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        loomd_inner.append(&loomd_dot);
        loomd_inner.append(&loomd_label);
        let loomd_sign = gtk::Button::builder().child(&loomd_inner).build();
        loomd_sign.add_css_class("flat");
        loomd_sign.update_property(&[gtk::accessible::Property::Label("loomd")]);
        let settings_button = gtk::Button::from_icon_name("emblem-system-symbolic");
        settings_button.update_property(&[gtk::accessible::Property::Label("Settings")]);
        settings_button.add_css_class("flat");
        settings_button.add_css_class("icon");
        settings_button.set_tooltip_text(Some("Settings (Ctrl+,)"));
        // A "?": the icon themes draw keyboard shortcuts as a Mac command key.
        let help_button = gtk::Button::builder().label("?").tooltip_text("Keyboard shortcuts (Ctrl+/)").build();
        help_button.update_property(&[gtk::accessible::Property::Label("Keyboard shortcuts")]);
        help_button.add_css_class("flat");
        help_button.add_css_class("icon");
        // At the window's bottom edge a tooltip would open below it, under a panel there.
        for button in [&new_button, &scratch_button, &run_button, &loomd_sign, &settings_button, &help_button] {
            tip_above(button);
        }
        bottom.append(&new_button);
        bottom.append(&scratch_button);
        bottom.append(&run_button);
        bottom.append(&loomd_sign);
        bottom.append(&spacer);
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
        // A project's page: its board, and its conversations in a sidebar on the right.
        let board = crate::board::BoardView::new();
        board.root.set_hexpand(true);
        history.root.add_css_class("project-side");
        history.root.set_size_request(340, -1);
        history.root.set_hexpand(false);
        let project_page = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        project_page.add_css_class("project-page-split");
        project_page.append(&board.root);
        project_page.append(&history.root);
        chat_stack.add_named(&project_page, Some("project"));
        let overview = crate::overview::Overview::new();
        chat_stack.add_named(&overview.root, Some("overview"));
        let hub = crate::hub::Hub::new();
        chat_stack.add_named(&hub.page, Some("notes"));
        chat_stack.add_named(&notes_chat.stack, Some("notes-chat"));
        let notes_panel = crate::notes_panel::NotesPanel::new();
        notes_panel.root.set_size_request(340, -1);
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
        // The open file beside the chat(s), above the tray: easy to read next to the agent.
        let files = Files::new();
        files.editor_box.set_size_request(320, -1);
        files.add_notes(&hub.panel);
        files.set_notes_available(crate::vault::current().is_some());
        let reading = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&chats)
            .end_child(&files.editor_box)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .build();
        // The agent's view: a rounded panel, the sidebars and the tray around it in one colour.
        // The agent's view and an open file: rounded panels side by side, the sidebars' colour
        // around and between them (the gap between them is the handle that resizes them).
        reading.add_css_class("panels");
        for panel in [chats.upcast_ref::<gtk::Widget>(), files.editor_box.upcast_ref()] {
            panel.add_css_class("island");
            // Content clipped to the rounded corners (not on Broadway, the test display: its
            // server aborts on that clip).
            if std::env::var("GDK_BACKEND").as_deref() != Ok("broadway") {
                panel.set_overflow(gtk::Overflow::Hidden);
            }
        }
        // Opened: about 45% of the width for the file (kept as dragged after that).
        let (pane, placed) = (reading.clone(), Cell::new(false));
        files.editor_box.connect_visible_notify(move |editor| {
            if editor.is_visible() && !placed.get() {
                let width = pane.width();
                if width > 0 {
                    pane.set_position(width * 55 / 100);
                    placed.set(true);
                }
            }
        });
        let work = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&reading)
            .end_child(&dock_stack)
            // Never smaller than they can be: else the window measures the chat for a height
            // it cannot fit in (a GTK warning on every relayout, every second while working).
            .shrink_start_child(false)
            .resize_end_child(false)
            .shrink_end_child(false)
            .vexpand(true)
            .build();
        let middle = gtk::Box::new(gtk::Orientation::Vertical, 0);
        middle.add_css_class("middle");
        middle.append(&work);
        middle.append(&bar_stack);

        // Right: the files panel (its lists; the file itself opens beside the chat).
        files.root.set_size_request(340, -1);
        files.root.set_visible(false);
        let split = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&middle)
            .end_child(&files.root)
            .shrink_start_child(false)
            .resize_end_child(false)
            .shrink_end_child(false)
            .hexpand(true)
            .build();
        split.add_css_class("split");

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

        let scale_css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &scale_css, gtk::STYLE_PROVIDER_PRIORITY_USER + 3);
        }

        let this = Rc::new(App {
            app: app.clone(),
            app_id: app_id.clone(),
            window,
            me: RefCell::default(),
            sidebar,
            list,
            rows: RefCell::default(),
            menu,
            inbox,
            inbox_list,
            chat_stack,
            overview,
            welcome,
            history,
            board,
            pinned_box,
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
            quitting: Cell::new(false),
            notes_chat,
            notes_panel,
            notes_button,
            notes_dot,
            notes_back,
            notes_mode: Cell::new(false),
            agents_side: RefCell::default(),
            notes_file: RefCell::default(),
            notes_tabs: RefCell::default(),
            side_stack,
            split: split.clone(),
            prefix: Cell::new(false),
            prefix_hint,
            prefix_timeout: RefCell::default(),
            files_were_open: Cell::new(false),
            editor_was_open: Cell::new(false),
            scale_css,
            toast,
            toast_label,
            toast_go: toast_go.clone(),
            toast_question: Cell::new(false),
            toast_timeout: RefCell::default(),
            overview_due: Cell::new(false),
            files_hidden_for_overview: Cell::new(false),
            hub: hub.clone(),
            project_edges: RefCell::default(),
            cards: RefCell::default(),
            card_layer: with_names.clone(),
            card_spots,
            running: RefCell::default(),
            running_rows: RefCell::default(),
            run_list,
            run_scroller,
            list_scroller,
            run_head,
            proj_head,
            sections_filler,
            run_cap: Cell::new(300),
            sidebar_feeds: RefCell::default(),
            sidebar_due: Cell::new(false),
            icons: RefCell::default(),
            icons_due: Cell::new(false),
            sidebar_drawn: RefCell::default(),
            remote_agents: RefCell::default(),
            remote_focused: Cell::new(None),
            remote_status: RefCell::default(),
            toast_remote: Cell::new(None),
            asks_feed: RefCell::default(),
            shown_feed: RefCell::default(),
            shown_seen: RefCell::default(),
            shown_started: Cell::new(false),
            uris_feed: RefCell::default(),
            uris_seen: RefCell::default(),
            uris_started: Cell::new(false),
            settings_feed: RefCell::default(),
            asks_seen: RefCell::default(),
            asks_started: Cell::new(false),
            asking_edit: Cell::new(None),
            overview_feeds: RefCell::default(),
            loomd_sign: (loomd_sign, loomd_dot, loomd_label),
            closed_feed: RefCell::default(),
            pinned_remote: Cell::new(None),
            previous_remote: Cell::new(None),
            back_to: Cell::new(None),
            fed_files: Cell::new((0, 0)),
            started: Cell::new(false),
            expect_copy: Cell::new(false),
        });
        this.follow_sidebar();
        this.wire_toast(&toast_go, &toast_close);
        let weak = Rc::downgrade(&this);
        *this.board.on_agent.borrow_mut() = Some(Rc::new(move |id| {
            if let Some(app) = weak.upgrade() {
                app.focus_remote(id);
            }
        }));
        let weak = Rc::downgrade(&this);
        *this.board.running.borrow_mut() = Some(Rc::new(move |project| {
            let Some(app) = weak.upgrade() else { return Vec::new() };
            let project = project.to_string_lossy();
            app.remote_items()
                .iter()
                .filter(|a| a["project_id"].as_str() == Some(&*project) && a["status"] != "suspended")
                .filter_map(|a| Some((a["id"].as_str()?.parse().ok()?, a["title"].as_str().unwrap_or_default().to_owned())))
                .collect()
        }));
        let weak = Rc::downgrade(&this);
        crate::conn::on_refused(move |text| {
            if let Some(app) = weak.upgrade()
                && !app.toast_waits()
            {
                app.toast_remote.set(None);
                app.reveal_toast(text, false);
            }
        });
        this.wire_hub();
        let weak = Rc::downgrade(&this);
        *this.files.on_request.borrow_mut() = Some(Rc::new(move |id, done| {
            let Some(app) = weak.upgrade() else { return };
            // loomd's ask: answered there, then the next edit waiting, if any.
            app.asking_edit.set(None);
            crate::conn::command("asks", if done { "ask_done" } else { "ask_cancel" }, json!({ "id": format!("a{id}") }));
            app.asks_changed();
        }));
        this.wire_history();
        this.wire_links();
        // "Ask Claude" in the editor: into the input of the agent in view.
        let weak = Rc::downgrade(&this);
        *this.files.on_ask.borrow_mut() = Some(Rc::new(move |text: String| {
            let Some(app) = weak.upgrade() else { return };
            let Some(chat) = app.chat_in_view() else { return };
            let buffer = chat.input.buffer();
            buffer.place_cursor(&buffer.end_iter());
            buffer.insert_at_cursor(&text);
            chat.input.grab_focus();
        }));
        crate::theme::status_colors(&this.window);
        this.wire(&new_button, &settings_button);
        let (weak, place) = (Rc::downgrade(&this), notes_place.downgrade());
        settings::on_change(move |_| {
            let on = crate::vault::current().is_some();
            if let Some(place) = place.upgrade() {
                place.set_visible(on);
            }
            if let Some(app) = weak.upgrade()
                && !on
                && app.notes_mode.get()
            {
                app.show_agents();
            }
        });
        let weak = Rc::downgrade(&this);
        overview_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.show_overview();
            }
        });
        let weak = Rc::downgrade(&this);
        scratch_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.toggle_scratch();
            }
        });
        let weak = Rc::downgrade(&this);
        run_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.ask_command();
            }
        });
        let weak = Rc::downgrade(&this);
        help_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::cheatsheet::open(&app.window);
            }
        });
        this.apply_settings(&settings::get());
        *this.me.borrow_mut() = Rc::downgrade(&this);
        // loomd's client: the agents, schedules and conversations are loomd's.
        this.wire_overview();
        this.wire_remote();
        this
    }

    fn wire(self: &Rc<Self>, new_button: &gtk::Button, settings_button: &gtk::Button) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            let Some(app) = weak.upgrade() else { return };
            app.activate_row(row.index() as usize);
        });
        let weak = Rc::downgrade(self);
        self.board.back.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.go_back();
            }
        });
        // The mouse's back button, anywhere in the window: the same.
        let mouse_back = gtk::GestureClick::builder().button(8).propagation_phase(gtk::PropagationPhase::Capture).build();
        let weak = Rc::downgrade(self);
        mouse_back.connect_pressed(move |gesture, _, _, _| {
            if weak.upgrade().is_some_and(|app| app.go_back()) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        self.window.add_controller(mouse_back);
        // The Running section: its rows in `running`'s order.
        let weak = Rc::downgrade(self);
        self.run_list.connect_row_activated(move |_, row| {
            let Some(app) = weak.upgrade() else { return };
            let id = app.running.borrow().get(row.index() as usize).copied();
            if let Some(id) = id {
                app.focus_remote(id);
            }
        });
        let weak = Rc::downgrade(self);
        crate::menu::on_right_click(&self.run_list, move |x, y| {
            let Some(app) = weak.upgrade() else { return };
            let id = app.run_list.row_at_y(y as i32).and_then(|row| app.running.borrow().get(row.index() as usize).copied());
            if let Some(menu) = id.and_then(|id| app.remote_menu(id)) {
                menu.popup(&app.run_list, x, y);
            }
        });
        self.run_head.toggle.connect_clicked(|_| crate::prefs::update(|s| s.show_running = !s.show_running));
        self.proj_head.toggle.connect_clicked(|_| crate::prefs::update(|s| s.projects_open = !s.projects_open));
        for (tool, collapsed) in self.proj_head.tools.iter().zip([true, false]) {
            let weak = Rc::downgrade(self);
            tool.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.collapse_all(collapsed);
                }
            });
        }
        // Running's height: up to half the sidebar (the window's, less its top and bottom).
        let weak = Rc::downgrade(self);
        self.window.connect_realize(move |window| {
            let weak = weak.clone();
            let Some(surface) = window.surface() else { return };
            surface.connect_layout(move |_, _, height| {
                if let Some(app) = weak.upgrade() {
                    app.run_cap.set(((height - 140) / 2).max(120));
                    app.apply_sections();
                }
            });
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
        // Right-click (or the Menu key) on a project or agent: its menu.
        let weak = Rc::downgrade(self);
        crate::menu::on_right_click(&self.list, move |x, y| {
            let Some(app) = weak.upgrade() else { return };
            match app.list.row_at_y(y as i32) {
                Some(row) => app.row_menu(Some(row.index() as usize), x, y),
                None => app.row_menu(None, x, y),
            }
        });

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
            } else if let Some(id) = app.remote_in_view() {
                crate::conn::command("window", "rename", json!({ "agent": id.to_string(), "title": name }));
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
            app.window.present();
            app.focus_remote(id);
        });
        self.app.add_action(&focus_action);
        // For the settings window: the overview (where schedules are).
        let overview_action = gio::SimpleAction::new("show-overview", None);
        let weak = Rc::downgrade(self);
        overview_action.connect_activate(move |_, _| {
            if let Some(app) = weak.upgrade() {
                app.window.present();
                app.show_overview();
            }
        });
        self.app.add_action(&overview_action);
        // For the settings window: a file in loom's editor (Settings → Claude's Edit).
        let open_action = gio::SimpleAction::new("open-file", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        open_action.connect_activate(move |_, target| {
            let (Some(app), Some(path)) = (weak.upgrade(), target.and_then(|t| t.get::<String>())) else { return };
            app.window.present();
            app.files.root.set_visible(true);
            app.files.open_file(Path::new(&path));
        });
        self.app.add_action(&open_action);

        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            let Some(app) = weak.upgrade() else { return glib::Propagation::Proceed };
            app.keep_window_state();
            app.close_request()
        });

        // With the debug log on: memory every 3 minutes (read off the main loop).
        let (weak, last) = (Rc::downgrade(self), Rc::new(Cell::new(None::<u64>)));
        glib::timeout_add_seconds_local(180, move || {
            if weak.upgrade().is_none() {
                return glib::ControlFlow::Break;
            }
            if crate::log::on() {
                // (the window's own process: its agents are loomd's)
                let agents: Vec<crate::memory::AgentInfo> = Vec::new();
                let last = last.clone();
                glib::spawn_future_local(async move {
                    let previous = last.get();
                    if let Ok((text, total)) = gio::spawn_blocking(move || crate::memory::snapshot(&agents, previous)).await {
                        last.set(Some(total));
                        crate::log::write("memory", &text);
                    }
                });
            }
            glib::ControlFlow::Continue
        });

        let weak = Rc::downgrade(self);
        settings::on_change(move |s| {
            if let Some(app) = weak.upgrade() {
                app.apply_settings(s);
                app.refresh_sidebar();
            }
        });
        // New colours, dark or light: the project icons drawn again in them, once restyled.
        let weak = Rc::downgrade(self);
        crate::theme::on_change(move || {
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(app) = weak.upgrade() {
                    app.refresh_sidebar();
                    app.fill_remote_overview();
                }
            });
        });
    }

    fn apply_settings(&self, s: &settings::Settings) {
        crate::log::set(s.debug_log);
        self.scale_css.load_from_string(&format!(
            ".messages, .plan, textview.input {{ font-size: {}em; }}",
            (s.text_scale * 100.0).round() / 100.0
        ));
        let mic = crate::stt::configured();
        // loomd's agents' chats (loomd tells its agents of a change of suggestions itself).
        for agent in self.remote_agents.borrow().values() {
            agent.chat.set_show_thinking(s.show_thinking);
            agent.chat.set_mic_visible(mic);
        }
        crate::theme::refont(&s.terminal_font);
        crate::theme::apply(&s.theme, &s.theme_mode);
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
        // The keyboard's Back key: from a project's page to the agent it was opened from.
        if key == gdk::Key::Back && self.go_back() {
            return glib::Propagation::Stop;
        }
        // The browser's page takes every key (typed into the page, its shortcuts its own).
        if GtkWindowExt::focus(&self.window).is_some_and(|w| w.has_css_class("browser-page")) {
            return glib::Propagation::Proceed;
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
            (true, true, false, gdk::Key::m) => self.toggle_notes(),
            // In the Notes view: its chats.
            (false, false, true, gdk::Key::Up) if self.notes_mode.get() => self.notes_chat.step(-1),
            (false, false, true, gdk::Key::Down) if self.notes_mode.get() => self.notes_chat.step(1),
            (true, false, false, gdk::Key::n) if self.notes_mode.get() => self.new_notes_chat(),
            (false, true, true, gdk::Key::Up) => self.move_agent(-1),
            (false, true, true, gdk::Key::Down) => self.move_agent(1),
            (false, false, true, gdk::Key::Up) => self.step(-1),
            (false, false, true, gdk::Key::Down) => self.step(1),
            (false, false, true, gdk::Key::Left | gdk::Key::Right) => self.swap(),
            (true, false, false, gdk::Key::Tab) => self.cycle(1),
            (true, true, false, gdk::Key::Tab | gdk::Key::ISO_Left_Tab) => self.cycle(-1),
            (true, true, false, gdk::Key::p) => self.palette(),
            // Everything else belongs to a focused terminal (Ctrl+W, Ctrl+K… mean something there).
            _ if in_terminal => return glib::Propagation::Proceed,
            // Questions, answered while typing: Alt+1…9 an option, Alt+Y allow, Alt+N deny,
            // Alt+Enter sends a form.
            (false, false, true, k) if k.to_unicode().and_then(|c| c.to_digit(10)).is_some_and(|d| d >= 1) => {
                let option = k.to_unicode().and_then(|c| c.to_digit(10)).unwrap_or(1) as usize - 1;
                if !self.remote_view().is_some_and(|a| a.answer(option)) {
                    return glib::Propagation::Proceed;
                }
            }
            (false, false, true, gdk::Key::y) => {
                if !self.remote_view().is_some_and(|a| a.answer_kind("allow")) {
                    return glib::Propagation::Proceed;
                }
            }
            (false, false, true, gdk::Key::n) => {
                if !self.remote_view().is_some_and(|a| a.answer_kind("reject")) {
                    return glib::Propagation::Proceed;
                }
            }
            (false, false, true, gdk::Key::Return | gdk::Key::KP_Enter) => {
                if !self.remote_view().is_some_and(|a| a.send_form()) {
                    return glib::Propagation::Proceed;
                }
            }
            (true, false, false, gdk::Key::n) => self.new_agent(false),
            (true, true, false, gdk::Key::n) => self.new_agent(true),
            (true, true, false, gdk::Key::t) => self.reopen_closed(),
            (true, false, false, gdk::Key::c) => {
                // Chat text can be selected but never holds focus (the input keeps it): copy
                // the chat's selection unless the input has one of its own.
                // In a terminal, Ctrl+C is its own (an interrupt).
                let in_terminal = GtkWindowExt::focus(&self.window).is_some_and(|w| w.is::<vte::Terminal>() || w.ancestor(vte::Terminal::static_type()).is_some());
                let copied = !in_terminal
                    && self.chat_in_view().is_some_and(|chat| !chat.input.buffer().has_selection() && chat.copy_selection());
                if !copied {
                    return glib::Propagation::Proceed;
                }
            }
            (true, false, false, gdk::Key::f) => {
                // The editor when it has the keyboard, the page of a project, else the chat.
                let in_files = GtkWindowExt::focus(&self.window).is_some_and(|w| w.is_ancestor(&self.files.root) || w.is_ancestor(&self.files.editor_box));
                let in_notes = self.notes_mode.get() && GtkWindowExt::focus(&self.window).is_some_and(|w| w.is_ancestor(&self.notes_panel.root));
                if in_notes {
                    self.notes_panel.focus_filter();
                } else if !(in_files && self.files.find()) {
                    if self.showing_project().is_some() {
                        self.history.focus_filter();
                    } else if let Some(chat) = self.chat_in_view() {
                        chat.find();
                    }
                }
            }
            (true, false, false, gdk::Key::w) => self.close_focused(),
            (true, false, false, gdk::Key::k) => self.open_switcher(),
            (true, false, false, gdk::Key::i) => self.open_inbox(),
            (true, true, false, gdk::Key::a) => self.show_overview(),
            (true, false, false, gdk::Key::j) => self.ask_command(),
            (true, true, false, gdk::Key::j) => self.current_tray().peek_latest(),
            (true, true, false, gdk::Key::x) => self.current_tray().clear_finished(),
            (true, false, false, gdk::Key::e) => self.toggle_panel(),
            (true, false, false, gdk::Key::o) => {
                self.leave_for_files();
                self.files.root.set_visible(true);
                self.files.choose();
            }
            (true, true, false, gdk::Key::o) => self.files.open_elsewhere(),
            (true, false, false, gdk::Key::l) => self.focus_input(),
            (true, false, false, gdk::Key::p) => self.find_file(),
            (true, false, false, gdk::Key::backslash) => self.toggle_split(),
            (true, false, false, gdk::Key::slash) => crate::cheatsheet::open(&self.window),
            (true, false, false, gdk::Key::comma) => crate::prefs::open(&self.window),
            (true, _, false, gdk::Key::equal | gdk::Key::plus | gdk::Key::KP_Add) => zoom(0.1),
            (true, false, false, gdk::Key::minus | gdk::Key::KP_Subtract) => zoom(-0.1),
            (true, false, false, gdk::Key::_0 | gdk::Key::KP_0) => crate::prefs::update(|s| s.text_scale = 1.0),
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
        let Some(chat) = self.chat_in_view() else {
            // On a project's page, typing filters its conversations.
            let focus = GtkWindowExt::focus(&self.window);
            let typing = focus.as_ref().is_some_and(|w| w.is::<gtk::Text>() || w.is::<gtk::TextView>());
            if !typing && self.showing_project().is_some() && !c.is_ascii_digit() {
                self.history.type_filter(c);
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        };
        let focus = GtkWindowExt::focus(&self.window);
        let takes_text = focus.as_ref().is_some_and(|w| {
            w.is::<gtk::Text>() || w.is::<gtk::TextView>() || w.is::<vte::Terminal>() || w.ancestor(vte::Terminal::static_type()).is_some()
        });
        let in_input = focus.as_ref().is_some_and(|w| w == chat.input.upcast_ref::<gtk::Widget>());
        // A digit with a question open and nothing typed yet answers it: from the chat input or
        // anything that takes no text (never from a field being typed in, like a plan comment).
        if (in_input || !takes_text)
            && let Some(n) = crate::chat::any_digit(c).filter(|n| *n >= 1)
            && chat.input.buffer().char_count() == 0
            && self.remote_view().is_some_and(|a| a.answer(n as usize - 1))
        {
            return glib::Propagation::Stop;
        }
        if takes_text {
            return glib::Propagation::Proceed;
        }
        let input = &chat.input;
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
            (_, gdk::Key::d) => {
                self.remote_do("background", json!({}));
            }
            (_, gdk::Key::z) => self.zoom(),
            (_, gdk::Key::b) => self.sidebar.set_visible(!self.sidebar.is_visible()),
            (_, gdk::Key::f | gdk::Key::e) => self.toggle_panel(),
            (_, gdk::Key::m) => self.toggle_notes(),
            (_, gdk::Key::o) => {
                self.leave_for_files();
                self.files.root.set_visible(true);
                self.files.choose();
            }
            (_, gdk::Key::t | gdk::Key::grave) => self.toggle_scratch(),
            (_, gdk::Key::r) => self.ask_command(),
            (_, gdk::Key::s) => self.open_switcher(),
            (_, gdk::Key::g) => self.find_file(),
            (_, gdk::Key::i) => self.open_inbox(),
            (_, gdk::Key::a) => self.show_overview(),
            (_, gdk::Key::colon | gdk::Key::semicolon) => self.palette(),
            (_, gdk::Key::comma) => crate::prefs::open(&self.window),
            (_, gdk::Key::question | gdk::Key::slash) => crate::cheatsheet::open(&self.window),
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

    pub fn focus_nth(self: &Rc<Self>, index: usize) {
        // The Running list's numbers (folded or not); none running, the sidebar's order.
        let running = self.running_ids();
        let list = if running.is_empty() { self.ids_in_order() } else { running };
        if let Some(id) = list.get(index) {
            self.focus_id(*id);
        }
    }

    pub fn start_in(self: &Rc<Self>, cwd: PathBuf) {
        self.start_agent_in(cwd);
    }

    /// What a key does, by name.
    pub fn act(self: &Rc<Self>, name: &str, argument: &str) {
        match name {
            "scratch" => self.toggle_scratch(),
            "files" => self.toggle_panel(),
            "switcher" => self.open_switcher(),
            "palette" => self.palette(),
            "inbox" => self.open_inbox(),
            "help" => crate::cheatsheet::open(&self.window),
            "settings" => crate::prefs::open(&self.window),
            "notes-chat" | "notes" if crate::vault::current().is_some() => self.show_notes(),
            "agents" => self.show_agents(),
            "topic-chat" => {
                let topic = crate::vault::current().and_then(|v| crate::topics::list(&v).into_iter().find(|t| t.name == argument));
                if let Some(topic) = topic {
                    self.show_notes();
                    self.notes_chat.topic_chat(&topic.name, &topic.folder);
                }
            }
            "peek" => self.current_tray().peek_latest(),
            "pin" => self.current_tray().pin_latest(true),
            "unpin" => self.current_tray().pin_latest(false),
            "split" => self.toggle_split(),
            "find" => self.find_file(),
            "elsewhere" => self.files.open_elsewhere(),
            "tree" => {
                self.leave_for_files();
                self.files.show_project();
            }
            "graph" => {
                self.leave_for_files();
                self.files.show_graph_view();
            }
            "graphwin" => self.files.graph_window(),
            "diff" => self.files.diff_for_tests(argument),
            "fork" => {
                if self.remote_do("fork", json!({})) {
                    self.expect_copy.set(true);
                }
            }
            "changes" => {
                self.leave_for_files();
                self.files.show_changes();
            }
            "close" => self.close_focused(),
            "reopen" => self.reopen_closed(),
            "rename" => {
                self.remote_do("rename", json!({ "title": argument }));
            }
            "collapse" => self.toggle_collapsed(Path::new(argument)),
            "open" => {
                self.files.root.set_visible(true);
                self.files.open_file(Path::new(argument));
            }
            // Test hooks: typing in the editor, and the changed-on-disk bar's buttons.
            "edittype" => self.files.type_for_tests(argument),
            "disk" => self.files.disk_for_tests(argument),
            "settle" => self.notes_panel.settle_for_tests(),
            "mode" => {
                if let Some(agent) = self.remote_view() {
                    agent.cycle_mode();
                }
            }
            "run" => self.current_tray().run(argument),
            "loomdrestart" => self.restart_loomd(),
            "loomdclick" => self.loomd_clicked(),
            "swap" => self.swap(),
            "putproject" => {
                if let Some((from, onto)) = argument.split_once('=') {
                    self.put_project(Path::new(from), Path::new(onto), true);
                }
            }
            "notesstep" => self.notes_chat.step(argument.parse().unwrap_or(1)),
            "jobkeys" => self.current_tray().keys_to_latest(&argument.replace("\\n", "\r")),
            other => eprintln!("loom control: unknown command {other}"),
        }
    }

    // ---- small actions --------------------------------------------------------------------

    fn current_project(&self) -> PathBuf {
        let remote = self.remote_focused.get().and_then(|id| self.remote_agents.borrow().get(&id)?.project());
        remote.or_else(|| self.showing_project()).unwrap_or_else(glib::home_dir)
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
        // where the agent in view works (its worktree, its repo), else the project in view
        let place = self.remote_view().and_then(|a| a.cwd()).unwrap_or_else(|| self.current_project());
        if !self.scratch.toggle(&place) {
            self.focus_input();
        }
    }

    fn focus_input(&self) {
        if let Some(chat) = self.chat_in_view() {
            chat.input.grab_focus();
        }
    }

    /// F2: the focused agent, or on a project's page, the project.
    fn begin_rename(&self) {
        let remote = self.remote_in_view().and_then(|id| self.remote_item(id));
        let project = if remote.is_none() { self.showing_project() } else { None };
        let (text, placeholder) = match (&remote, &project) {
            (Some(item), _) => (item["title"].as_str().unwrap_or_default().to_owned(), "Agent name"),
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
        let choices = self.palette_choices();
        self.picker.open("Run a command, or go to an agent…", choices);
    }

    /// Test instances: runs the palette's entry titled `title` (its start), as picked there.
    pub fn palette_run(self: &Rc<Self>, title: &str) -> Result<(), String> {
        let choice = self.palette_choices().into_iter().find(|c| c.title.starts_with(title)).ok_or("no such entry")?;
        (choice.run)();
        Ok(())
    }

    /// The palette's entries: its agents (those waiting on you first), its commands, and the
    /// conversations closed lately.
    pub fn palette_choices(self: &Rc<Self>) -> Vec<Choice> {
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
            act("This project's page: its board and conversations", "", |a| {
                let project = a.current_project();
                a.show_project(&project, &project);
            }),
            act("Reopen the last closed agent", "Ctrl+Shift+T", |a| a.reopen_closed()),
            act("Rename this agent or project", "F2", |a| a.begin_rename()),
            act("Copy this conversation (fork)", "", |a| {
                if a.remote_do("fork", json!({})) {
                    a.expect_copy.set(true);
                }
            }),
            act("Clear: a fresh conversation here (/clear)", "", |a| {
                a.remote_do("clear", json!({}));
            }),
            // In the input, drafted, to edit and send (Enter) or leave.
            act("Compact this conversation…", "", |a| {
                if let Some(chat) = a.chat_in_view() {
                    chat.input.buffer().set_text(loom_core::agent::REST_COMPACT);
                    chat.input.grab_focus();
                }
            }),
            act("Let this agent work in another folder too…", "", |a| a.add_dir()),
            act("Save this conversation as Markdown…", "", |a| a.export()),
            act("Move this agent up", "Alt+Shift+↑", |a| a.move_agent(-1)),
            act("Move this agent down", "Alt+Shift+↓", |a| a.move_agent(1)),
            act("Move this project up", "", |a| a.move_project(-1)),
            act("Move this project down", "", |a| a.move_project(1)),
            act("Restart this agent", "", |a| {
                if let Some(id) = a.remote_in_view() {
                    a.unless_stopping_remote(id, "Restart", "The conversation goes on after it.", |_, id| {
                        crate::conn::command("window", "restart", json!({ "agent": id.to_string() }));
                    });
                }
            }),
            act("Stop the turn", "Esc", |a| {
                a.remote_do("stop", json!({}));
            }),
            // What the right-click menus have, here too.
            act("New agent in a worktree of this project…", "", |a| {
                let project = a.current_project();
                a.new_worktree_at(&project, 60.0, 120.0);
            }),
            act("Merge this agent's worktree branch back", "", |a| a.palette_git("worktree_merge")),
            act("Remove this agent's worktree", "", |a| a.palette_git("worktree_remove")),
            act("Suspend this agent (free its memory)", "", |a| {
                a.remote_do("suspend", json!({}));
            }),
            act("Keep this agent awake, or let it suspend when idle", "", |a| {
                let awake = a.remote_in_view().and_then(|id| a.remote_item(id)).is_some_and(|i| i["awake"] == true);
                a.remote_do("set_awake", json!({ "awake": !awake }));
            }),
            act("Icon colour of this project…", "", |a| {
                let project = a.current_project();
                a.pick_icon_colour(&project);
            }),
            act("Show every project's agents", "", |a| a.collapse_all(false)),
            act("Hide every project's agents", "", |a| a.collapse_all(true)),
            act("Next permission mode", "Shift+Tab", |a| {
                if let Some(agent) = a.remote_view() {
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
                a.leave_for_files();
                a.files.root.set_visible(true);
                a.files.choose();
            }),
            act("Project files (tree)", "", |a| {
                a.leave_for_files();
                a.files.show_project();
            }),
            act("Git changes in this project", "", |a| {
                a.leave_for_files();
                a.files.show_changes();
            }),
            act("Open the file in another editor", "Ctrl+Shift+O", |a| a.files.open_elsewhere()),
            act("Scratch terminal", "Ctrl+`", |a| a.toggle_scratch()),
            act("Browser: this project's, above the tray", "", |a| a.current_tray().open_browser()),
            act("Run a command in the tray…", "Ctrl+J", |a| a.ask_command()),
            act("Peek the newest job", "Ctrl+Shift+J", |a| a.current_tray().peek_latest()),
            act("Clear finished jobs", "Ctrl+Shift+X", |a| a.current_tray().clear_finished()),
            act("Questions waiting", "Ctrl+I", |a| a.open_inbox()),
            act("Overview of all agents", "Ctrl+Shift+A", |a| a.show_overview()),
            act("Notes: chat with your notes", "Ctrl+Shift+M", |a| a.show_notes()),
            act("Notes history", "", |a| a.show_notes_history()),
            act("Schedule a prompt for this agent…", "", |a| {
                if let Some(id) = a.remote_in_view() {
                    a.schedule_for_remote(id);
                }
            }),
            act("Away mode: agents decide their own questions until…", "", |a| a.choose_away()),
            act("Settings", "Ctrl+,", |a| crate::prefs::open(&a.window)),
            act("Keyboard shortcuts", "Ctrl+/", |a| crate::cheatsheet::open(&a.window)),
            act("About Loom", "", |a| crate::about::open(&a.window)),
        ];
        // Agents before commands: those waiting on you, then the last used.
        let mut agents = self.remote_items();
        agents.sort_by_key(|a| (a["status"] != "blocked", std::cmp::Reverse(a["used"].as_str().unwrap_or_default().to_owned())));
        let mut going: Vec<Choice> = Vec::new();
        for item in agents {
            let Some(id) = item["id"].as_str().and_then(|i| i.parse::<u32>().ok()) else { continue };
            let weak = weak.clone();
            let mut choice = Choice::new(format!("Go to {}", item["title"].as_str().unwrap_or_default()), item["project"].as_str().unwrap_or_default().to_owned(), move || {
                if let Some(app) = weak.upgrade() {
                    app.focus_remote(id);
                }
            });
            choice.dot = Some(Status::from_css(item["status"].as_str().unwrap_or_default()).css());
            going.push(choice);
        }
        going.append(&mut choices);
        let mut choices = going;
        let closed = self.closed_feed.borrow().as_ref().map(|f| {
            f.update();
            f.items().clone()
        });
        for item in closed.unwrap_or_default() {
            let Some(session) = item["id"].as_str().map(str::to_owned) else { continue };
            let weak = weak.clone();
            let title = item["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("an agent").to_owned();
            let folder = PathBuf::from(item["folder"].as_str().unwrap_or_default());
            choices.push(Choice::new(format!("Reopen {title}"), self.project_label(&project_of(&folder)), move || {
                if let Some(app) = weak.upgrade() {
                    app.reopen(&session);
                }
            }));
        }
        choices
    }

    /// Conversation `session`, closed lately, an agent again: opened once loomd made it.
    fn reopen(self: &Rc<Self>, session: &str) {
        let weak = Rc::downgrade(self);
        crate::conn::command_then("window", "reopen", json!({ "session": session }), move |answer| {
            let id = answer.and_then(|a| a["agent"].as_str().and_then(|a| a.parse::<u32>().ok()));
            if let (Some(app), Some(id)) = (weak.upgrade(), id) {
                app.focus_remote(id);
            }
        });
    }

    /// Ctrl+K: agents and projects.
    fn open_switcher(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let mut choices = Vec::new();
        // loomd's agents and projects (the window as its client).
        for item in self.remote_items() {
            let Some(id) = item["id"].as_str().and_then(|i| i.parse::<u32>().ok()) else { continue };
            let weak = weak.clone();
            let mut choice = Choice::new(item["title"].as_str().unwrap_or_default().to_owned(), item["project"].as_str().unwrap_or_default().to_owned(), move || {
                if let Some(app) = weak.upgrade() {
                    app.focus_remote(id);
                }
            });
            choice.dot = Some(Status::from_css(item["status"].as_str().unwrap_or_default()).css());
            choices.push(choice);
        }
        let projects = self.sidebar_feeds.borrow().as_ref().map(|(_, p)| p.items().clone()).unwrap_or_default();
        for project in projects {
            let (Some(path), name) = (project["id"].as_str().map(PathBuf::from), project["name"].as_str().unwrap_or_default().to_owned()) else { continue };
            let weak = weak.clone();
            choices.push(Choice::new(name, "project · conversations", move || {
                if let Some(app) = weak.upgrade() {
                    app.show_project(&path, &path);
                }
            }));
        }

        self.picker.open("Go to an agent or project…", choices);
    }

    // ---- projects and agents --------------------------------------------------------------

    fn tray_for(&self, project: &Path) -> Rc<Tray> {
        if let Some(tray) = self.trays.borrow().get(project) {
            return tray.clone();
        }
        let tray = Tray::new(project.to_owned());
        let name = project.to_string_lossy();
        self.dock_stack.add_named(&tray.dock, Some(&name));
        self.bar_stack.add_named(&tray.bar, Some(&name));
        // The dock pane shows only while something is pinned in the project in view. (The dock
        // itself stays visible: a stack never shows a hidden page.)
        let (dock_stack, weak) = (self.dock_stack.clone(), Rc::downgrade(&tray));
        *tray.on_dock.borrow_mut() = Some(Rc::new(move || {
            if let Some(tray) = weak.upgrade()
                && dock_stack.visible_child().as_ref() == Some(tray.dock.upcast_ref())
            {
                dock_stack.set_visible(tray.pinned());
            }
        }));
        // A job ending (a build, the tests) is worth a word when the window is elsewhere.
        let (app, project_name) = (self.window.clone(), self.project_label(project));
        let application = self.app.clone();
        let icon_of = project.to_owned();
        *tray.on_finish.borrow_mut() = Some(Rc::new(move |name: &str, code: Option<i64>| {
            if app.is_active() || !settings::get().notify_done {
                return;
            }
            let how = match code {
                Some(0) => "finished".to_owned(),
                Some(code) => format!("failed (exit {code})"),
                None => "ended".to_owned(),
            };
            let notification = gio::Notification::new(&format!("{name} {how}"));
            notification.set_body(Some(&format!("Job in {project_name}")));
            // In the app icon's colours: the dark fox, as it is now (its colour already given).
            if let Some(icon) = loom_core::project_icon::ready(&icon_of, &[], true) {
                notification.set_icon(&gio::FileIcon::new(&gio::File::for_path(icon)));
            }
            application.send_notification(Some(&format!("job-{name}")), &notification);
        }));
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
        self.dock_stack.set_visible(tray.pinned());
    }

    /// The chat in view: the focused agent's, or that of loomd's agent in view.
    pub fn chat_in_view(&self) -> Option<Rc<crate::chat::Chat>> {
        if self.notes_mode.get()
            && let Some(chat) = self.notes_chat.remote_chat()
        {
            return Some(chat);
        }
        self.remote_view().map(|a| a.chat.clone())
    }

    /// loomd's agent whose chat is in view, if one is.
    pub fn remote_view(&self) -> Option<Rc<crate::remote_agent::RemoteAgent>> {
        let id = self.remote_in_view()?;
        self.remote_agents.borrow().get(&id).cloned()
    }

    /// One of loomd's agents in view (the window as its client): its chat made the first time.
    fn focus_remote(self: &Rc<Self>, id: u32) {
        self.leave_notes();
        // The one beside, opened: the two change places.
        if self.pinned_remote.get() == Some(id) {
            let current = self.remote_in_view();
            self.unpin_remote();
            self.focus_remote(id);
            if let Some(current) = current.filter(|c| *c != id) {
                self.pin_remote(current);
            }
            return;
        }
        let agent = self.remote_agent(id);
        if let Some(old) = self.remote_focused.replace(Some(id)).filter(|old| *old != id) {
            self.previous_remote.set(Some(old));
            if let Some(old) = self.remote_agents.borrow().get(&old).cloned() {
                old.chat.left();
            }
        }
        agent.chat.returned();
        self.chat_stack.set_visible_child(&agent.chat.root);
        // The files panel back, if the overview closed it; then fed with this agent's files.
        if self.files_hidden_for_overview.take() {
            self.files.root.set_visible(true);
        }
        self.feed_files(id);
        self.tell_hub(id);
        self.mark_active();
        // kept at once (not only on a clean close): the agent in view for the next start
        self.keep_window_state();
        if self.toast_remote.get() == Some(id) {
            self.hide_toast();
        }
        // its header from the `agents` item, and the row marked
        self.refresh_sidebar();
        if let Some(project) = agent.project() {
            self.show_tray(&project, &self.tray_for(&project));
        }
        agent.chat.input.grab_focus();
        // A sidebar row takes the focus back on release: the input gets it again after.
        let input = agent.chat.input.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(input) = input.upgrade()
                && input.is_mapped()
            {
                input.grab_focus();
            }
        });
    }

    /// The files panel shows agent `id`'s: the files it created or changed (loomd's `changes`),
    /// its folder, its project's tree, and its repos first in Changes.
    fn feed_files(self: &Rc<Self>, id: u32) {
        let Some(item) = self.remote_item(id) else { return };
        let (cwd, project) = (PathBuf::from(item["cwd"].as_str().unwrap_or_default()), PathBuf::from(item["project_id"].as_str().unwrap_or_default()));
        let edited = item["edited"].as_u64().unwrap_or(0) as usize;
        // The first file it touches opens the panel (as when it was the window's own agent).
        let (was, before) = self.fed_files.replace((id, edited));
        if was == id && before == 0 && edited > 0 {
            self.files.root.set_visible(true);
        }
        if edited == 0 {
            self.files.show(&cwd, &project, &[]);
            self.files.set_repos(Vec::new());
            return;
        }
        let weak = Rc::downgrade(self);
        crate::conn::command_result("window", "changes", json!({ "agent": id.to_string() }), move |answer| {
            let Some(app) = weak.upgrade() else { return };
            if app.remote_in_view() != Some(id) {
                return;
            }
            let files = answer.ok().and_then(|a| a["files"].as_array().cloned()).unwrap_or_default();
            let entries: Vec<(PathBuf, loom_core::agent::Mark)> = files
                .iter()
                .filter_map(|f| {
                    let mark = if f["mark"] == "created" { loom_core::agent::Mark::Created } else { loom_core::agent::Mark::Changed };
                    Some((PathBuf::from(f["path"].as_str()?), mark))
                })
                .collect();
            let mut repos: Vec<PathBuf> = Vec::new();
            for repo in files.iter().filter_map(|f| f["repo"].as_str()).map(PathBuf::from) {
                if !repos.contains(&repo) {
                    repos.push(repo);
                }
            }
            app.files.show(&cwd, &project, &entries);
            app.files.set_repos(repos);
        });
    }

    /// A job an agent opened for the user (Claude's login): brought forward in its project's
    /// tray, once per time it asks (`n` grows); those there before the window started stay.
    fn jobs_shown(self: &Rc<Self>) {
        let items = {
            let held = self.shown_feed.borrow();
            let Some(feed) = held.as_ref() else { return };
            feed.update();
            feed.items().clone()
        };
        // the first copy: what was asked before this window started, not brought up again
        let first = !self.shown_started.replace(true);
        for item in items {
            let (Some(agent), Some(n)) = (item["id"].as_str(), item["n"].as_u64()) else { continue };
            let before = self.shown_seen.borrow_mut().insert(agent.to_owned(), n).unwrap_or(0);
            let job = item["job"].as_str().and_then(|j| j.parse().ok());
            let project = agent.parse().ok().and_then(|a| self.remote_project(a));
            if let (false, Some(job), Some(project)) = (first, job, project)
                && n > before
            {
                self.tray_for(&project).peek_id(job);
            }
        }
    }

    /// A web page an MCP server asked to open (the user chose "open"): opened in the browser,
    /// once per time (`n` grows); only http(s), and not those from before the window started.
    fn uris_shown(self: &Rc<Self>) {
        let items = {
            let held = self.uris_feed.borrow();
            let Some(feed) = held.as_ref() else { return };
            feed.update();
            feed.items().clone()
        };
        let first = !self.uris_started.replace(true);
        for item in items {
            let (Some(agent), Some(n), Some(uri)) = (item["id"].as_str(), item["n"].as_u64(), item["uri"].as_str()) else { continue };
            let before = self.uris_seen.borrow_mut().insert(agent.to_owned(), n).unwrap_or(0);
            if !first && n > before && (uri.starts_with("https://") || uri.starts_with("http://")) {
                gtk::UriLauncher::new(uri).launch(Some(&self.window), None::<&gio::Cancellable>, |_| {});
            }
        }
    }

    /// The Notes panel's "this conversation": the agent in view's (its session, once it has one).
    fn tell_hub(&self, id: u32) {
        let session = self.overview_items().into_iter().find(|o| o["id"].as_str() == Some(&*id.to_string())).and_then(|o| o["session"].as_str().map(str::to_owned));
        let title = self.remote_item(id).and_then(|i| i["title"].as_str().map(str::to_owned)).unwrap_or_default();
        self.hub.set_current(session, &title);
    }

    /// loomd's agent `id`'s chat here (made, in the stack, the first time).
    fn remote_agent(&self, id: u32) -> Rc<crate::remote_agent::RemoteAgent> {
        let agent = self.remote_agents.borrow().get(&id).cloned();
        agent.unwrap_or_else(|| {
            let agent = crate::remote_agent::RemoteAgent::new(id);
            let weak = self.me.borrow().clone();
            *agent.on_copy.borrow_mut() = Some(Rc::new(move || {
                if let Some(app) = weak.upgrade() {
                    app.expect_copy.set(true);
                }
            }));
            // The header's project name: its page.
            let (weak, of) = (self.me.borrow().clone(), Rc::downgrade(&agent));
            *agent.chat.on_project.borrow_mut() = Some(Rc::new(move |()| {
                if let (Some(app), Some(project)) = (weak.upgrade(), of.upgrade().and_then(|a| a.project())) {
                    app.show_project(&project, &project);
                }
            }));
            self.chat_stack.add_named(&agent.chat.root, Some(&format!("remote-{id}")));
            self.remote_agents.borrow_mut().insert(id, agent.clone());
            agent
        })
    }

    /// loomd's agent `id` shown beside the one in view (in place of any there before).
    fn pin_remote(self: &Rc<Self>, id: u32) {
        if self.remote_in_view() == Some(id) || self.pinned_remote.get() == Some(id) {
            return;
        }
        self.unpin_remote();
        let agent = self.remote_agent(id);
        self.chat_stack.remove(&agent.chat.root);
        self.pinned_box.append(&agent.chat.root);
        self.pinned_box.set_visible(true);
        self.pinned_remote.set(Some(id));
        self.mark_active();
        // its header from `agents`
        self.refresh_sidebar();
        self.tell_looking();
    }

    /// loomd's agent beside put back with the others; its id.
    fn unpin_remote(&self) -> Option<u32> {
        let id = self.pinned_remote.take()?;
        if let Some(agent) = self.remote_agents.borrow().get(&id).cloned() {
            self.pinned_box.remove(&agent.chat.root);
            self.chat_stack.add_named(&agent.chat.root, Some(&format!("remote-{id}")));
        }
        self.pinned_box.set_visible(false);
        self.mark_active();
        self.tell_looking();
        Some(id)
    }

    /// The window as it is (size, maximized, the agents in view by session and id), for its
    /// next start.
    fn keep_window_state(&self) {
        let session = |id: Option<u32>| {
            let id = id?.to_string();
            self.overview_items().into_iter().find(|o| o["id"].as_str() == Some(&*id))?["session"].as_str().map(str::to_owned)
        };
        let (width, height) = self.window.default_size();
        let state = json!({
            "width": width,
            "height": height,
            "maximized": self.window.is_maximized(),
            "agent": self.remote_in_view().map(|id| id.to_string()),
            "session": session(self.remote_in_view()),
            "pinned": session(self.pinned_remote.get()),
        });
        let path = window_state_path(&self.app_id);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, state.to_string());
    }

    /// loomd's agents came the first time: the overview, or the agent (and the one beside) in
    /// view when the window last closed, as the settings say.
    fn first_view(self: &Rc<Self>) {
        if self.started.get() || self.notes_mode.get() {
            return;
        }
        // once, on loomd's first whole list (none: nothing to choose, the welcome stays)
        let synced = self.overview_feeds.borrow().as_ref().is_some_and(|(o, _)| {
            o.update();
            o.synced()
        });
        if !synced {
            return;
        }
        self.started.set(true);
        let ids: Vec<(u32, Option<String>)> = self
            .overview_items()
            .iter()
            .filter_map(|o| Some((o["id"].as_str()?.parse().ok()?, o["session"].as_str().map(str::to_owned))))
            .collect();
        if ids.is_empty() {
            return;
        }
        let kept = window_state(&self.app_id);
        let find = |session: &Value, id: &Value| {
            let by_session = session.as_str().and_then(|s| ids.iter().find(|(_, own)| own.as_deref() == Some(s)));
            by_session.or_else(|| id.as_str().and_then(|i| i.parse::<u32>().ok()).and_then(|i| ids.iter().find(|(own, _)| *own == i))).map(|(id, _)| *id)
        };
        match find(&kept["session"], &kept["agent"]) {
            Some(id) if !settings::get().start_on_overview => {
                self.focus_remote(id);
                if let Some(beside) = find(&kept["pinned"], &Value::Null).filter(|b| *b != id) {
                    self.pin_remote(beside);
                }
            }
            _ => self.show_overview(),
        }
    }

    /// Side by side, the pane with the keyboard is marked (`active`), the other dimmed.
    fn mark_active(&self) {
        let (focused, pinned) = (self.remote_focused.get(), self.pinned_remote.get());
        for (id, agent) in self.remote_agents.borrow().iter() {
            let root = &agent.chat.root;
            root.remove_css_class("active");
            root.remove_css_class("inactive");
            if pinned.is_some() {
                root.add_css_class(if focused == Some(*id) { "active" } else { "inactive" });
            }
        }
    }

    /// loomd's agent in view, if the window is in use and shows one.
    fn remote_looked_at(&self) -> Option<u32> {
        let id = self.remote_focused.get()?;
        (self.window.is_active() && self.chat_stack.visible_child_name().as_deref() == Some(format!("remote-{id}").as_str())).then_some(id)
    }

    /// loomd is told what the window shows (it says nothing on the desktop about that, and
    /// leaves questions to the window while it is in use). Said again every 20 s: loomd
    /// forgets it after a minute (a window that crashed silences nothing for long).
    fn tell_looking(&self) {
        // In front but untouched a while: away (notices and the bridges speak again).
        let active = self.window.is_active() && LAST_INPUT.get().elapsed() < IDLE_AWAY;
        let beside = self.pinned_remote.get().filter(|_| active);
        let agents: Vec<String> = self.remote_looked_at().into_iter().chain(beside).map(|id| id.to_string()).collect();
        crate::conn::command_result("window", "set_looking", json!({ "active": active, "agents": agents }), |_| {});
    }

    /// Any key, click or scroll in the window: the user is there (back from
    /// idle: loomd told at once).
    fn watch_input(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let seen = Rc::new(move || {
            let idle = LAST_INPUT.replace(std::time::Instant::now()).elapsed() >= IDLE_AWAY;
            if idle && let Some(app) = weak.upgrade() {
                app.tell_looking();
            }
        });
        let keys = gtk::EventControllerKey::builder().propagation_phase(gtk::PropagationPhase::Capture).build();
        let s = seen.clone();
        keys.connect_key_pressed(move |_, _, _, _| {
            s();
            glib::Propagation::Proceed
        });
        // (not pointer motion: GTK makes some up when what is under the pointer changes, a
        // reply coming in, so it doesn't tell the user is there)
        let scroll = gtk::EventControllerScroll::builder().flags(gtk::EventControllerScrollFlags::BOTH_AXES).propagation_phase(gtk::PropagationPhase::Capture).build();
        let s = seen.clone();
        scroll.connect_scroll(move |_, _, _| {
            s();
            glib::Propagation::Proceed
        });
        let click = gtk::GestureClick::builder().button(0).propagation_phase(gtk::PropagationPhase::Capture).build();
        click.connect_pressed(move |_, _, _, _| seen());
        self.window.add_controller(keys);
        self.window.add_controller(scroll);
        self.window.add_controller(click);
    }

    /// The window as loomd's client: what it shows told to loomd, and loomd's `asks` acted on.
    fn wire_remote(self: &Rc<Self>) {
        self.watch_input();
        let weak = Rc::downgrade(self);
        self.window.connect_is_active_notify(move |_| {
            if let Some(app) = weak.upgrade() {
                app.tell_looking();
            }
        });
        let weak = Rc::downgrade(self);
        self.chat_stack.connect_visible_child_name_notify(move |_| {
            if let Some(app) = weak.upgrade() {
                app.tell_looking();
            }
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(20, move || match weak.upgrade() {
            Some(app) => {
                app.tell_looking();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        self.tell_looking();
        // The sign: as loomd connects and goes, and every 2 s (a refusal says nothing itself).
        if let Some(remote) = crate::conn::loomd() {
            let weak = Rc::downgrade(self);
            remote.on_state(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.draw_loomd_sign();
                }
            });
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(2, move || match weak.upgrade() {
            Some(app) => {
                app.draw_loomd_sign();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        let weak = Rc::downgrade(self);
        self.loomd_sign.0.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.loomd_clicked();
            }
        });
        self.draw_loomd_sign();
        let ready = || {
            let weak = Rc::downgrade(self);
            move || {
                if let Some(app) = weak.upgrade() {
                    app.refresh_overview();
                    app.refresh_inbox();
                    // the Running rows' second lines (its questions, its last words)
                    app.sidebar_soon();
                    app.first_view();
                }
            }
        };
        *self.overview_feeds.borrow_mut() = Feed::new("overview", ready()).zip(Feed::new("schedules", ready()));
        *self.closed_feed.borrow_mut() = Feed::new("closed", || {});
        let weak = Rc::downgrade(self);
        *self.uris_feed.borrow_mut() = Feed::new("uris_shown", move || {
            if let Some(app) = weak.upgrade() {
                app.uris_shown();
            }
        });
        let weak = Rc::downgrade(self);
        *self.shown_feed.borrow_mut() = Feed::new("jobs_shown", move || {
            if let Some(app) = weak.upgrade() {
                app.jobs_shown();
            }
        });
        let weak = Rc::downgrade(self);
        *self.asks_feed.borrow_mut() = Feed::new("asks", move || {
            if let Some(app) = weak.upgrade() {
                app.asks_changed();
            }
        });
        let weak = Rc::downgrade(self);
        let feed = Feed::new("settings", move || {
            if let Some(app) = weak.upgrade() {
                app.settings_changed();
            }
        });
        *self.settings_feed.borrow_mut() = feed;
        self.settings_changed();
    }

    /// loomd's settings taken as this window's (secrets, which the API never gives, kept).
    fn settings_changed(&self) {
        let item = {
            let held = self.settings_feed.borrow();
            let Some(feed) = held.as_ref() else { return };
            feed.update();
            feed.items().first().cloned()
        };
        if let Some(item) = item {
            settings::adopt(loom_core::settings::from_item(&item, &settings::get()));
        }
    }

    /// The questions loomd's agents wait on (`overview` has them): each agent's, answered
    /// here as in the window's own inbox (a form, in its chat).
    fn refresh_remote_inbox(self: &Rc<Self>) {
        let waiting: Vec<(u32, String, Vec<Value>)> = {
            let overview = self.overview_items();
            self.remote_items()
                .iter()
                .filter_map(|a| {
                    let id: u32 = a["id"].as_str()?.parse().ok()?;
                    let questions: Vec<Value> = overview.iter().find(|o| o["id"] == a["id"]).and_then(|o| o["questions"].as_array().cloned()).unwrap_or_default();
                    (!questions.is_empty()).then(|| (id, a["title"].as_str().unwrap_or_default().to_owned(), questions))
                })
                .collect()
        };
        let count: usize = waiting.iter().map(|(_, _, q)| q.len()).sum();
        self.inbox.set_label(&format!("⚑  {count} waiting for you  ·  Ctrl+I"));
        self.inbox.set_visible(count > 0);
        if count == 0 {
            self.inbox.popdown();
        }
        while let Some(child) = self.inbox_list.first_child() {
            self.inbox_list.remove(&child);
        }
        for (id, title, questions) in waiting {
            let go = gtk::Button::with_label(&title);
            go.add_css_class("flat");
            go.add_css_class("inbox-agent");
            let weak = Rc::downgrade(self);
            go.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.inbox.popdown();
                    app.focus_remote(id);
                }
            });
            self.inbox_list.append(&go);
            for question in questions {
                let label = gtk::Label::builder().label(question["title"].as_str().unwrap_or_default()).xalign(0.0).wrap(true).max_width_chars(50).build();
                self.inbox_list.append(&label);
                let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                if question["form"] == true {
                    let open = gtk::Button::with_label("Answer in the chat");
                    open.add_css_class("suggested");
                    let weak = Rc::downgrade(self);
                    open.connect_clicked(move |_| {
                        if let Some(app) = weak.upgrade() {
                            app.inbox.popdown();
                            app.focus_remote(id);
                        }
                    });
                    buttons.append(&open);
                }
                let entry = question["entry"].as_str().unwrap_or_default().to_owned();
                for option in question["options"].as_array().into_iter().flatten() {
                    let kind = option["kind"].as_str().unwrap_or_default();
                    let button = inbox_option(option["name"].as_str().unwrap_or_default());
                    button.add_css_class(if kind.starts_with("allow") { "suggested" } else { "plain" });
                    let params = json!({ "agent": id.to_string(), "entry": entry, "option": option["id"] });
                    button.connect_clicked(move |_| crate::conn::command("window", "answer", params.clone()));
                    buttons.append(&button);
                }
                self.inbox_list.append(&buttons);
            }
        }
    }

    /// loomd's agents no longer in its `agents` (closed there): their chats go; the one in
    /// view gives way to the overview.
    fn drop_closed_remote(self: &Rc<Self>) {
        // Only a whole list says one is gone (not one being read again after a reconnect).
        let whole = crate::conn::loomd().is_some_and(|r| r.connected()) && self.sidebar_feeds.borrow().as_ref().is_some_and(|(agents, _)| agents.synced());
        if !whole {
            return;
        }
        let live: Vec<u32> = self.ids_in_order();
        // Gone: listed before (its status seen), not now. One just opened (resumed, reopened)
        // may not be listed yet.
        let seen = self.remote_status.borrow().keys().copied().collect::<Vec<u32>>();
        let gone: Vec<u32> = self.remote_agents.borrow().keys().copied().filter(|id| seen.contains(id) && !live.contains(id)).collect();
        for id in gone {
            let in_view = self.remote_in_view() == Some(id);
            if self.pinned_remote.get() == Some(id) {
                self.unpin_remote();
            }
            if let Some(agent) = self.remote_agents.borrow_mut().remove(&id) {
                self.chat_stack.remove(&agent.chat.root);
            }
            self.remote_status.borrow_mut().remove(&id);
            if self.remote_focused.get() == Some(id) {
                self.remote_focused.set(None);
            }
            if in_view {
                self.show_overview();
            }
        }
    }

    /// What loomd is to this window: connected as the same version, older or newer (its
    /// version, or commands this window uses that it lacks), refusing it, or not reachable.
    fn loomd_state(&self) -> (&'static str, String, String) {
        let mine = env!("CARGO_PKG_VERSION");
        let Some(remote) = crate::conn::loomd() else { return ("", String::new(), String::new()) };
        let hello = remote.hello();
        if hello.is_null() {
            return match remote.refused() {
                Some(why) => ("failed", "loomd: refused".to_owned(), format!("loomd refused this window ({why}). Stop it and open loom again: this window starts its own version.")),
                None => ("suspended", "loomd…".to_owned(), "loomd isn't reachable now; the window tries again every second. Click to start it if it stopped. Your agents run in loomd.".to_owned()),
            };
        }
        let version = hello["server"]["version"].as_str().unwrap_or("?");
        if let Some(missing) = loomd_differs(&hello, mine) {
            let lacks = if missing > 0 { format!(", and lacks {missing} of the commands this window uses") } else { String::new() };
            return ("blocked", format!("loomd {version}"), format!("loomd is {version}, this window {mine}{lacks}. Click to restart it as this window's version."));
        }
        let closing = if settings::get().keep_working {
            "Closing this window leaves them working."
        } else if !crate::conn::started_loomd() {
            "It ran before this window (a service, or another window): closing this one leaves it running."
        } else {
            "Closing this window stops it (Settings → General: keep agents working)."
        };
        ("done", "loomd".to_owned(), format!("loomd {version} runs your agents, jobs and bridges. {closing}"))
    }

    fn draw_loomd_sign(&self) {
        let (button, dot, label) = &self.loomd_sign;
        // Restarting: it says so until that is over.
        if !button.is_sensitive() {
            return;
        }
        let (class, words, tip) = self.loomd_state();
        for c in ["done", "failed", "blocked", "suspended"] {
            dot.remove_css_class(c);
        }
        if !class.is_empty() {
            dot.add_css_class(class);
        }
        label.set_label(&words);
        button.set_tooltip_text(Some(&tip));
    }

    /// The sign clicked: an older or newer loomd restarted as this window's version (after
    /// asking: its working agents stop), a refusing one explained, a gone one started again.
    fn loomd_clicked(self: &Rc<Self>) {
        let (class, _, tip) = self.loomd_state();
        let detail = match class {
            // Not reachable: none answers, so one is started (nothing runs to stop).
            "suspended" if loom_core::api::socket::running(&loom_core::api::socket::socket_path(&self.app_id)).is_none() => return self.restart_loomd(),
            "blocked" => {
                let busy: Vec<String> = self.sidebar_feeds.borrow().as_ref().map(|(agents, _)| agents.items().iter().filter(|a| a["busy"] == true).filter_map(|a| a["title"].as_str().map(str::to_owned)).collect()).unwrap_or_default();
                let stops = if busy.is_empty() { String::new() } else { format!(" Working now, they stop (their conversations are kept and go on): {}.", busy.join(", ")) };
                format!("{tip}{stops}")
            }
            "failed" => {
                gtk::AlertDialog::builder().message("loomd refuses this window").detail(tip).buttons(["OK"]).build().show(Some(&self.window));
                return;
            }
            _ => return,
        };
        let dialog = gtk::AlertDialog::builder().message("Restart loomd?").detail(detail).buttons(["Cancel", "Restart"]).cancel_button(0).default_button(0).build();
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), None::<&gio::Cancellable>, move |choice| {
            if let (Some(app), Ok(1)) = (weak.upgrade(), choice) {
                app.restart_loomd();
            }
        });
    }

    /// loomd stopped, then started again as this window's version; the connection follows.
    fn restart_loomd(self: &Rc<Self>) {
        let (button, _, label) = &self.loomd_sign;
        button.set_sensitive(false);
        label.set_label("loomd…");
        button.set_tooltip_text(Some("loomd is starting as this window's version."));
        // Asked to stop; one older than loomd_stop is sent SIGTERM (as `systemctl stop` would).
        let hello = crate::conn::loomd().map(|r| r.hello()).unwrap_or_default();
        let asks = hello["capabilities"]["commands"].as_array().is_some_and(|c| c.iter().any(|c| c == "loomd_stop"));
        if asks {
            crate::conn::command("window", "loomd_stop", json!({}));
        }
        let (socket, app_id) = (loom_core::api::socket::socket_path(&self.app_id), self.app_id.clone());
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let started = gio::spawn_blocking(move || {
                if !asks
                    && let Some(pid) = loom_core::api::socket::server_pid(&socket)
                {
                    stop_old_loomd(pid)?;
                }
                // The old one gone first (it saves the workspace as it goes).
                let deadline = std::time::Instant::now() + Duration::from_secs(20);
                while loom_core::api::socket::running(&socket).is_some() {
                    if std::time::Instant::now() > deadline {
                        return Err("the old loomd didn't stop".to_owned());
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                crate::conn::start_loomd(&socket, &app_id)
            })
            .await
            .unwrap_or_else(|_| Err("restarting loomd failed".to_owned()));
            let Some(app) = weak.upgrade() else { return };
            app.loomd_sign.0.set_sensitive(true);
            if let Err(err) = started {
                crate::log::write("loomd", &err);
                app.loomd_sign.2.set_label("loomd: failed");
                app.loomd_sign.0.set_tooltip_text(Some(&err));
                return;
            }
            app.draw_loomd_sign();
        });
    }

    /// loomd's agent `id` went to `status`: a toast about it settled, or one shown (the window
    /// in use, the agent out of view; else loomd says it on the desktop).
    fn remote_changed(self: &Rc<Self>, id: u32, status: &str, item: &Value) {
        let looking = self.remote_looked_at() == Some(id);
        if self.toast_remote.get() == Some(id) && (looking || (self.toast_question.get() && status != "blocked")) {
            self.hide_toast();
        }
        if !self.window.is_active() || looking {
            return;
        }
        let title = item["title"].as_str().unwrap_or("An agent");
        match status {
            "blocked" if settings::get().notify_questions => self.show_remote_toast(id, &format!("{title} needs you"), true),
            "failed" => self.show_remote_toast(id, &format!("{title} stopped: {}", item["failed"]["why"].as_str().unwrap_or("it failed")), false),
            _ => {}
        }
    }

    /// loomd's `asks` changed: the new ones done here (a file opened or to fill in, a job's
    /// terminal shown, a toast), one edit at a time. Those there when the window came were
    /// for then, but an edit an agent still waits for.
    fn asks_changed(self: &Rc<Self>) {
        let items: Vec<Value> = {
            let feed = self.asks_feed.borrow();
            let Some(feed) = feed.as_ref() else { return };
            feed.update();
            feed.items().clone()
        };
        // Known by id and time: a loomd started again counts its asks from a1 again.
        let key = |item: &Value| format!("{}@{}", item["id"].as_str().unwrap_or_default(), item["at"]);
        let keys: std::collections::HashSet<String> = items.iter().map(key).collect();
        // The edit in the files panel, given up by its agent: its bar goes.
        if let Some(n) = self.asking_edit.get()
            && !items.iter().any(|i| i["id"].as_str() == Some(format!("a{n}").as_str()))
        {
            self.asking_edit.set(None);
            self.files.end_request(n);
        }
        let first = !self.asks_started.replace(true);
        let mut due: Vec<Value> = Vec::new();
        {
            let mut seen = self.asks_seen.borrow_mut();
            seen.retain(|k| keys.contains(k));
            for item in &items {
                let id = key(item);
                if seen.contains(&id) || item["state"] != "waiting" {
                    continue;
                }
                let edit = item["kind"] == "edit";
                // (a notice clicked shortly before this window opened still shows its agent)
                if first && !edit && item["kind"] != "show" {
                    seen.insert(id);
                    continue;
                }
                if edit && (self.files.requesting() || due.iter().any(|d| d["kind"] == "edit")) {
                    continue;
                }
                seen.insert(id);
                due.push(item.clone());
            }
        }
        for item in due {
            self.act_on_ask(&item);
        }
    }

    /// One of loomd's asks, done here.
    fn act_on_ask(self: &Rc<Self>, item: &Value) {
        let id = item["id"].as_str().unwrap_or_default().to_owned();
        let agent: Option<u32> = item["agent"].as_str().and_then(|a| a.parse().ok());
        let title = item["title"].as_str().unwrap_or("An agent");
        let message = item["message"].as_str().unwrap_or_default();
        let path = item["path"].as_str().map(PathBuf::from);
        let out_of_view = self.window.is_active() && agent.is_some() && self.remote_looked_at() != agent;
        let done = |id: &str| crate::conn::command("asks", "ask_done", json!({ "id": id }));
        // Shown without taking the keyboard from what the user is doing.
        let focus = GtkWindowExt::focus(&self.window);
        match item["kind"].as_str().unwrap_or_default() {
            "open" => {
                if let Some(path) = path
                    && let Err(err) = self.files.open_for_agent(&path)
                {
                    crate::log::write("asks", &format!("{id}: {err}"));
                }
                done(&id);
            }
            "edit" => {
                let (Some(path), Some(n)) = (path, id.strip_prefix('a').and_then(|n| n.parse::<u64>().ok())) else { return };
                match self.files.request_edit(n, &path, &format!("{title} is waiting: {message}")) {
                    Ok(()) => {
                        self.asking_edit.set(Some(n));
                        if out_of_view && let Some(agent) = agent {
                            self.show_remote_toast(agent, &format!("{title} waits for you to fill in a file"), true);
                        }
                    }
                    // Tried again when `asks` changes next (unsaved changes in the editor…).
                    Err(err) => {
                        crate::log::write("asks", &format!("{id}: {err}"));
                        self.asks_seen.borrow_mut().remove(&format!("{id}@{}", item["at"]));
                    }
                }
            }
            "notify" => {
                if out_of_view && let Some(agent) = agent {
                    self.show_remote_toast(agent, &format!("{title}: {message}"), false);
                }
                done(&id);
            }
            // A desktop notice about it clicked: the window comes up on that agent.
            "show" => {
                done(&id);
                if let Some(agent) = agent {
                    self.focus_remote(agent);
                    self.window.present();
                    return;
                }
            }
            "input" => {
                let job = item["job"].as_str().and_then(|j| j.parse().ok());
                let project = agent.and_then(|a| self.remote_project(a));
                if let (Some(job), Some(project)) = (job, project) {
                    self.tray_for(&project).peek_id(job);
                }
                done(&id);
            }
            _ => {}
        }
        if let Some(widget) = focus {
            widget.grab_focus();
        }
    }

    /// loomd's agent `id`'s project, as `agents` says.
    fn remote_project(&self, id: u32) -> Option<PathBuf> {
        let feeds = self.sidebar_feeds.borrow();
        let (agents, _) = feeds.as_ref()?;
        let id = id.to_string();
        agents.items().iter().find(|a| a["id"].as_str() == Some(id.as_str()))?["project_id"].as_str().map(PathBuf::from)
    }

    /// Shows the previous agent beside the focused one, or puts it back.
    fn toggle_split(self: &Rc<Self>) {
        // loomd's: the one before (else the next in the sidebar) beside the one in view.
        if self.unpin_remote().is_some() {
            return;
        }
        let Some(current) = self.remote_in_view() else { return };
        let ids = self.ids_in_order();
        let next = ids.iter().position(|i| *i == current).and_then(|at| ids.get((at + 1) % ids.len())).copied();
        let candidate = self.previous_remote.get().filter(|p| *p != current && ids.contains(p)).or(next).filter(|c| *c != current);
        if let Some(id) = candidate {
            self.pin_remote(id);
        }
    }

    /// The pinned agent and the focused one change places (focus moves across the split).
    fn swap(self: &Rc<Self>) {
        if let (Some(beside), Some(current)) = (self.pinned_remote.get(), self.remote_in_view()) {
            self.unpin_remote();
            self.focus_remote(beside);
            self.pin_remote(current);
        }
    }

    /// Hides the sidebar and the files panel (and brings them back).
    fn zoom(&self) {
        let panel = self.side_panel();
        let zoomed = !self.sidebar.is_visible() && !panel.is_visible();
        if zoomed {
            self.sidebar.set_visible(true);
            panel.set_visible(self.files_were_open.get());
            self.files.editor_box.set_visible(self.editor_was_open.get() && self.files.open_path().is_some());
        } else {
            self.files_were_open.set(panel.is_visible());
            self.editor_was_open.set(self.files.editor_box.is_visible());
            self.sidebar.set_visible(false);
            panel.set_visible(false);
            self.files.editor_box.set_visible(false);
        }
    }

    /// Ctrl+Tab: through the Running list (stopped ones skipped: passing must wake nothing), or
    /// the sidebar's order when that setting is off. Nothing running: the one last looked at.
    fn cycle(self: &Rc<Self>, by: i32) {
        if !settings::get().cycle_running {
            self.step(by);
            return;
        }
        let failed = |id: &u32| self.remote_item(*id).is_some_and(|a| a["status"] == "failed");
        let ids: Vec<u32> = self.running_ids().into_iter().filter(|id| !failed(id)).collect();
        if ids.is_empty() {
            let last = self.previous_remote.get().or_else(|| self.ids_in_order().first().copied());
            if let Some(id) = last {
                self.focus_id(id);
            }
            return;
        }
        // From where the focused one is; from outside the list, the first (or the last).
        let next = match self.focused_id().and_then(|f| ids.iter().position(|i| *i == f)) {
            Some(at) => (at as i32 + by).rem_euclid(ids.len() as i32) as usize,
            None if by > 0 => 0,
            None => ids.len() - 1,
        };
        self.focus_id(ids[next]);
    }

    /// Alt+↑/↓, Ctrl+B j/k: the next agent as the sidebar shows them.
    fn step(self: &Rc<Self>, by: i32) {
        let ids = self.ids_in_order();
        if ids.is_empty() {
            return;
        }
        let current = self.focused_id().and_then(|f| ids.iter().position(|i| *i == f)).unwrap_or(0) as i32;
        let next = (current + by).rem_euclid(ids.len() as i32) as usize;
        self.focus_id(ids[next]);
    }

    /// The agents' ids in sidebar order: the window's own, or loomd's (its `agents`' order).
    fn ids_in_order(&self) -> Vec<u32> {
        self.remote_items().iter().filter_map(|a| a["id"].as_str()?.parse().ok()).collect()
    }

    /// The running agents' ids in the Running list's order.
    fn running_ids(&self) -> Vec<u32> {
        let live = self.remote_items().iter().filter(|a| a["status"] != "suspended").filter_map(|a| a["id"].as_str()?.parse().ok()).collect();
        self.running_order(live)
    }

    /// The agent in view: the window's own, or loomd's.
    fn focused_id(&self) -> Option<u32> {
        self.remote_in_view()
    }

    /// loomd's agent whose chat is in view, if one is.
    fn remote_in_view(&self) -> Option<u32> {
        let id = self.remote_focused.get()?;
        (self.chat_stack.visible_child_name().as_deref() == Some(format!("remote-{id}").as_str())).then_some(id)
    }

    /// Agent `id` in view: the window's own, or loomd's.
    fn focus_id(self: &Rc<Self>, id: u32) {
        self.focus_remote(id);
    }

    /// loomd's agents' items in `agents`, as last read (its order: the sidebar's).
    fn remote_items(&self) -> Vec<Value> {
        self.sidebar_feeds.borrow().as_ref().map(|(agents, _)| agents.items().clone()).unwrap_or_default()
    }

    /// The conversations loomd's agents have open (`overview` says them).
    fn remote_sessions(&self) -> Vec<String> {
        self.overview_items().iter().filter_map(|o| o["session"].as_str().map(str::to_owned)).collect()
    }

    /// loomd's `overview` as it is now (what came applied first).
    fn overview_items(&self) -> Vec<Value> {
        let feeds = self.overview_feeds.borrow();
        let Some((overview, _)) = feeds.as_ref() else { return Vec::new() };
        overview.update();
        overview.items().clone()
    }

    /// Takes loomd's project at `path` out of the sidebar, closing its agents there (asks
    /// first if any works).
    fn remove_remote_project(self: &Rc<Self>, path: &Path) {
        let mine: Vec<Value> = self.remote_items().into_iter().filter(|a| a["project_id"].as_str() == Some(&*path.to_string_lossy())).collect();
        let busy = mine.iter().filter(|a| a["busy"] == true).count();
        let ids: Vec<String> = mine.iter().filter_map(|a| a["id"].as_str().map(str::to_owned)).collect();
        let project = path.to_owned();
        let go = move || {
            // Its agents closed first (remove_project refuses while one works).
            for id in &ids {
                crate::conn::command("window", "close_agent", json!({ "agent": id }));
            }
            crate::conn::command("window", "remove_project", json!({ "project": project }));
        };
        if busy == 0 {
            return go();
        }
        let dialog = gtk::AlertDialog::builder()
            .message(format!("{busy} of its agents {} working", if busy == 1 { "is" } else { "are" }))
            .detail("Removing the project closes its agents and stops what they are doing. Their conversations are kept.")
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

    /// loomd's agent `id`'s item in `agents`.
    fn remote_item(&self, id: u32) -> Option<Value> {
        let id = id.to_string();
        self.remote_items().into_iter().find(|a| a["id"].as_str() == Some(id.as_str()))
    }

    /// A new agent starts from its project's page (where a past conversation can be picked
    /// up instead): this one's (`choose`: a folder asked for).
    fn new_agent(self: &Rc<Self>, choose: bool) {
        // The project in view: its page, or the agent's in view (Ctrl+N: its page, to start one).
        let in_view = self.remote_in_view().and_then(|id| self.remote_item(id)).and_then(|i| i["project_id"].as_str().map(PathBuf::from));
        let here = self.showing_project().or(in_view).map(|p| (p.clone(), p));
        match here {
            Some((project, cwd)) if !choose => self.show_project(&project, &cwd),
            _ => {
                let here = here.map(|(_, cwd)| cwd);
                let dialog = gtk::FileDialog::builder().title("Open a project folder").modal(true).build();
                // Always a folder to start in: GTK's chooser otherwise opens on "Recent", reading
                // the whole recent-files list first (seconds, with a long one).
                let start = here.map(|cwd| project_of(&cwd).parent().unwrap_or(&cwd).to_owned()).unwrap_or_else(glib::home_dir);
                dialog.set_initial_folder(Some(&gio::File::for_path(start)));
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

    /// A message passed on from the agent in view: pick another agent, and it lands in that
    /// one's input (quoted, with where it came from), to read over and send.
    pub fn send_elsewhere(self: &Rc<Self>, text: String) {
        let from = self.remote_in_view();
        let from_title = from.and_then(|id| self.remote_item(id)).and_then(|i| i["title"].as_str().map(str::to_owned)).unwrap_or_else(|| "another agent".to_owned());
        let weak = Rc::downgrade(self);
        let mut choices = Vec::new();
        for item in self.remote_items() {
            let Some(id) = item["id"].as_str().and_then(|i| i.parse::<u32>().ok()).filter(|id| Some(*id) != from) else { continue };
            let (weak, text, from_title) = (weak.clone(), text.clone(), from_title.clone());
            let mut choice = Choice::new(format!("Send to {}", item["title"].as_str().unwrap_or_default()), item["project"].as_str().unwrap_or_default().to_owned(), move || {
                let Some(app) = weak.upgrade() else { return };
                app.focus_remote(id);
                let agent = app.remote_agent(id);
                let buffer = agent.chat.input.buffer();
                let note = format!("From {from_title}:\n\n> {}\n\n", text.trim().replace('\n', "\n> "));
                buffer.place_cursor(&buffer.end_iter());
                if buffer.char_count() > 0 {
                    buffer.insert_at_cursor("\n");
                }
                buffer.insert_at_cursor(&note);
                agent.chat.input.grab_focus();
            });
            choice.dot = Some(Status::from_css(item["status"].as_str().unwrap_or_default()).css());
            choices.push(choice);
        }
        if choices.is_empty() {
            crate::conn::tell("No other agent to send it to.");
            return;
        }
        self.picker.open("Send it to which agent?", choices);
    }

    /// File links in chats ("loom-file:src/app.rs:42", relative links, file://) open in
    /// loom's editor, from the folder of the agent whose chat it is.
    fn wire_links(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        crate::chat::SEND_ELSEWHERE.with(|h| {
            let weak = weak.clone();
            *h.borrow_mut() = Some(Rc::new(move |text: String| {
                if let Some(app) = weak.upgrade() {
                    app.send_elsewhere(text);
                }
            }));
        });
        crate::chat::OPEN_LINK.with(|h| {
            *h.borrow_mut() = Some(Rc::new(move |uri: &str, widget: &gtk::Widget| {
                let Some(app) = weak.upgrade() else { return false };
                let agent = app.remote_agents.borrow().values().find(|a| widget.is_ancestor(&a.chat.root)).cloned().or_else(|| app.remote_view());
                let in_file = widget.is_ancestor(&app.files.editor_box);
                let file_dir = app.files.open_path().and_then(|p| p.parent().map(Path::to_path_buf)).filter(|_| in_file);
                let base = file_dir.unwrap_or_else(|| agent.as_ref().and_then(|a| a.cwd()).unwrap_or_else(glib::home_dir));
                let (path, line) = if let Some(rest) = uri.strip_prefix("file://") {
                    // A file URI: the line split off the path, then percent-escapes decoded
                    // (spaces, Persian names).
                    let (bare, line) = split_line(rest);
                    (gio::File::for_uri(&format!("file://{bare}")).path().unwrap_or_default(), line)
                } else {
                    let (path, line) = split_line(uri.strip_prefix("loom-file:").unwrap_or(uri));
                    (state::expand(Path::new(path)), line)
                };
                let path = if path.is_absolute() { path } else { base.join(path) };
                if path.is_dir() {
                    app.show_project(&project_of(&path), &path);
                    return true;
                }
                if !path.is_file() {
                    if let Some(agent) = agent {
                        agent.chat.notice(&format!("No such file here: {}", path.display()), "dim");
                    }
                    return true;
                }
                app.files.open_file(&path);
                if let Some(line) = line {
                    app.files.go_to_line(line);
                }
                true
            }));
        });
    }

    /// Saves the conversation in view as a Markdown file (asks where).
    fn export(self: &Rc<Self>) {
        // loomd's agent in view: its title, folder and chat.
        let (title, cwd, chat) = match self.remote_view() {
            Some(agent) => {
                let item = self.remote_item(agent.id).unwrap_or_default();
                (item["title"].as_str().unwrap_or_default().to_owned(), PathBuf::from(item["cwd"].as_str().unwrap_or_default()), agent.chat.clone())
            }
            None => return,
        };
        let name: String = title.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
        let dialog = gtk::FileDialog::builder().title("Save the conversation").initial_name(format!("{}.md", name.trim())).modal(true).build();
        dialog.set_initial_folder(Some(&gio::File::for_path(&cwd)));
        dialog.save(Some(&self.window), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                let text = format!("# {title}\n\n{}", chat.transcript());
                match std::fs::write(&path, text) {
                    Ok(()) => chat.notice(&format!("Saved to {}", path.display()), "dim"),
                    Err(err) => chat.notice(&format!("Could not save it: {err}"), "error"),
                }
            }
        });
    }

    /// Command `method` for loomd's agent in view (`extra`: its other params); false if none
    /// is in view.
    /// Away mode: for how long (or ended). While on, Claude's own questions are answered
    /// "decide yourself, note it on the board"; permissions still wait.
    fn choose_away(self: &Rc<Self>) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let set = |until: u64| move || crate::conn::command("window", "set_settings", json!({ "changes": { "away_until": until } }));
        let mut choices = vec![Choice::new("For 1 hour", "", set(now + 3600)), Choice::new("For 3 hours", "", set(now + 3 * 3600))];
        for hour in [7, 9] {
            if let Some(at) = settings::next_clock(hour) {
                choices.push(Choice::new(format!("Until {hour}:00"), "", set(at)));
            }
        }
        if settings::get().away_until > now {
            choices.insert(0, Choice::new("I'm back: end away mode", "", set(0)));
        }
        self.picker.open("Away: agents decide their own questions (permissions still wait). Until…", choices);
    }

    /// A worktree command on the agent in view, from the palette.
    fn palette_git(self: &Rc<Self>, method: &str) {
        if let Some(id) = self.remote_in_view() {
            self.worktree_git(id, method);
        }
    }

    /// A worktree command on agent `id`: what git said, in a toast. A merge that didn't go
    /// (a conflict) is handed to the agent: a message drafted in its input, to send.
    fn worktree_git(self: &Rc<Self>, id: u32, method: &str) {
        let weak = Rc::downgrade(self);
        crate::conn::command_result("window", method, json!({ "agent": id.to_string() }), move |answer| match answer {
            Ok(answer) => crate::conn::tell(answer["said"].as_str().unwrap_or("Done.")),
            Err(err) => {
                let said = err.to_string();
                crate::conn::tell(&said);
                let base = said.split(" didn't merge into ").nth(1).and_then(|rest| rest.split(" (nothing changed)").next());
                let (Some(app), Some(base)) = (weak.upgrade(), base) else { return };
                app.focus_remote(id);
                let agent = app.remote_agent(id);
                agent.chat.input.buffer().set_text(&format!(
                    "Merging your branch back into {base} conflicts (loom undid the merge). Here in your worktree, merge {base} into your branch, resolve the conflicts, check it builds and its tests pass, and commit. Then tell me it is ready to merge back."
                ));
                agent.chat.input.grab_focus();
            }
        });
    }

    fn remote_do(&self, method: &str, mut extra: Value) -> bool {
        let Some(id) = self.remote_in_view() else { return false };
        extra["agent"] = json!(id.to_string());
        crate::conn::command("window", method, extra);
        true
    }

    /// Moves the focused agent up or down among its project's agents.
    fn move_agent(self: &Rc<Self>, by: i32) {
        if let Some(id) = self.remote_in_view() {
            crate::conn::command("window", "move_agent", json!({ "agent": id.to_string(), "by": by }));
        }
    }

    /// Moves the project in view up or down the sidebar.
    fn move_project(self: &Rc<Self>, by: i32) {
        let project = self.remote_in_view().and_then(|id| self.remote_item(id)).and_then(|i| i["project_id"].as_str().map(PathBuf::from)).or_else(|| self.showing_project());
        if let Some(project) = project {
            crate::conn::command("window", "move_project", json!({ "project": project, "by": by }));
        }
    }

    /// Asks for a folder the focused agent may work in besides its own (another repo).
    fn add_dir(self: &Rc<Self>) {
        // loomd's in view: the folder asked for here, given to it there.
        if let Some(id) = self.remote_in_view() {
            let project = self.remote_item(id).and_then(|i| i["project_id"].as_str().map(PathBuf::from)).unwrap_or_default();
            let dialog = gtk::FileDialog::builder().title("Another folder this agent may work in").modal(true).build();
            dialog.set_initial_folder(Some(&gio::File::for_path(project.parent().unwrap_or(&project))));
            dialog.select_folder(Some(&self.window), None::<&gio::Cancellable>, move |result| {
                if let Ok(folder) = result
                    && let Some(path) = folder.path()
                {
                    crate::conn::command("window", "add_folder", json!({ "agent": id.to_string(), "folder": path }));
                }
            });
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
            weak.upgrade().is_some_and(|app| app.remote_sessions().iter().any(|s| s == id))
        }));
    }

    /// The project whose page is showing, if it is.
    fn showing_project(&self) -> Option<PathBuf> {
        let showing = self.chat_stack.visible_child_name().as_deref() == Some("project");
        showing.then(|| self.history.project())
    }

    /// Picks row `n` of the project page (0: new conversation).
    pub fn pick_history(&self, n: i32) {
        self.history.activate(n);
    }

    /// The project's name: given by hand, else its folder's.
    pub fn project_label(&self, path: &Path) -> String {
        // loomd's: its `projects` name it (empty: the folder's name).
        let named = self.remote_projects().into_iter().find(|p| p["id"].as_str() == Some(&*path.to_string_lossy())).and_then(|p| p["name"].as_str().filter(|n| !n.is_empty()).map(str::to_owned));
        named.unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
    }

    /// loomd's `projects` items, in the sidebar's order.
    fn remote_projects(&self) -> Vec<Value> {
        self.sidebar_feeds.borrow().as_ref().map(|(_, projects)| projects.items().clone()).unwrap_or_default()
    }

    /// Names a project (empty: back to its folder's name).
    fn rename_project(self: &Rc<Self>, path: &Path, name: &str) {
        crate::conn::command("window", "rename_project", json!({ "project": path, "name": name }));
        if self.showing_project().is_some_and(|p| p == path) {
            let shown = if name.is_empty() { path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { name.to_owned() };
            self.history.set_name(&shown);
            self.board.set_name(&shown);
        }
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

    /// A project's icon file in the window's colours; `None` while its logo is still looked
    /// for (the sidebar and overview are drawn again soon).
    fn icon_file(self: &Rc<Self>, path: &Path) -> Option<PathBuf> {
        let key = (path.to_owned(), crate::files::is_dark(&self.list));
        if let Some(file) = self.icons.borrow().get(&key) {
            return Some(file.clone());
        }
        let others: Vec<PathBuf> = self.remote_projects().iter().filter_map(|p| p["id"].as_str().map(PathBuf::from)).collect();
        let file = loom_core::project_icon::ready(path, &others, key.1);
        match &file {
            Some(file) => {
                self.icons.borrow_mut().insert(key, file.clone());
            }
            None if !self.icons_due.replace(true) => {
                let weak = Rc::downgrade(self);
                glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                    if let Some(app) = weak.upgrade() {
                        app.icons_due.set(false);
                        app.refresh_sidebar();
                        app.fill_remote_overview();
                    }
                });
            }
            None => {}
        }
        file
    }

    /// A project's icon, `size` pixels square (blank while it is looked for).
    fn project_icon(self: &Rc<Self>, path: &Path, size: i32) -> gtk::Image {
        let image = self.icon_file(path).map_or_else(gtk::Image::new, gtk::Image::from_file);
        image.set_pixel_size(size);
        image.set_size_request(size, size);
        image.add_css_class("project-icon");
        image
    }

    /// Changes a project's icon (`change` writes it), then draws the sidebar again.
    fn set_icon(self: &Rc<Self>, path: &Path, change: impl FnOnce(&Path) -> std::io::Result<()>) {
        if let Err(e) = change(path) {
            eprintln!("loom: the icon of {} was not changed: {e}", path.display());
        }
        self.icons.borrow_mut().retain(|(p, _), _| p != path);
        self.refresh_sidebar();
        self.fill_remote_overview();
    }

    /// The fox's colour picked by hand; its tile follows.
    fn pick_icon_colour(self: &Rc<Self>, path: &Path) {
        let dialog = gtk::ColorDialog::builder().title("The icon's colour").with_alpha(false).modal(true).build();
        let (weak, path) = (Rc::downgrade(self), path.to_owned());
        dialog.choose_rgba(Some(&self.window), None, None::<&gio::Cancellable>, move |chosen| {
            let (Some(app), Ok(rgba)) = (weak.upgrade(), chosen) else { return };
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            let hex = format!("#{:02x}{:02x}{:02x}", byte(rgba.red()), byte(rgba.green()), byte(rgba.blue()));
            app.set_icon(&path, |p| loom_core::project_icon::pick(p, Some(&hex)));
        });
    }

    /// A picture (SVG or PNG) as the project's icon, copied into its `.loom` folder.
    fn pick_icon_picture(self: &Rc<Self>, path: &Path) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("SVG or PNG"));
        filter.add_mime_type("image/svg+xml");
        filter.add_mime_type("image/png");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title("The project's icon").filters(&filters).modal(true).build();
        dialog.set_initial_folder(Some(&gio::File::for_path(path)));
        let (weak, path) = (Rc::downgrade(self), path.to_owned());
        dialog.open(Some(&self.window), None::<&gio::Cancellable>, move |chosen| {
            let (Some(app), Ok(file)) = (weak.upgrade(), chosen) else { return };
            if let Some(picture) = file.path() {
                app.set_icon(&path, |p| loom_core::project_icon::choose(p, Some(&picture)));
            }
        });
    }

    /// A branch's name asked for in place, then an agent in a worktree of its own on it.
    fn new_worktree_at(self: &Rc<Self>, project: &Path, x: f64, y: f64) {
        let entry = gtk::Entry::builder().width_chars(30).placeholder_text("The new branch's name").build();
        entry.set_tooltip_text(Some("Made from what the project has checked out, in a folder beside it"));
        let weak = Rc::downgrade(self);
        let project = project.to_owned();
        entry.connect_activate(move |entry| {
            let Some(app) = weak.upgrade() else { return };
            let branch = entry.text().trim().to_owned();
            if branch.is_empty() {
                return;
            }
            entry.set_sensitive(false);
            let (weak, entry) = (Rc::downgrade(&app), entry.clone());
            crate::conn::command_result("window", "new_worktree_agent", json!({ "project": project, "branch": branch }), move |answer| {
                entry.set_sensitive(true);
                let Some(app) = weak.upgrade() else { return };
                match answer {
                    Ok(answer) => {
                        app.menu.popdown();
                        if let Some(id) = answer["agent"].as_str().and_then(|a| a.parse().ok()) {
                            app.focus_remote(id);
                        }
                    }
                    // said, and the name typed stays to be fixed
                    Err(err) => crate::conn::tell(&err.to_string()),
                }
            });
        });
        self.menu.set_child(Some(&entry));
        self.menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        self.menu.popup();
        entry.grab_focus();
    }

    // ---- overview and schedules -----------------------------------------------------------

    fn showing_overview(&self) -> bool {
        self.chat_stack.visible_child_name().as_deref() == Some("overview")
    }

    /// Every agent on one page (no agent has the keyboard meanwhile).
    pub fn show_overview(self: &Rc<Self>) {
        self.leave_notes();
        self.chat_stack.set_visible_child_name("overview");
        // The files panel is an agent's: closed here, back as it was when an agent is opened.
        if self.files.root.is_visible() {
            self.files_hidden_for_overview.set(true);
            self.files.root.set_visible(false);
        }
        self.fill_overview();
        self.refresh_sidebar();
        self.overview.grab_focus();
    }

    // ---- the Notes view --------------------------------------------------------------------

    /// The Notes view: its chats in the sidebar, a chat in the middle, its topics and history
    /// on the right. The agents' side is kept as it was, for the way back.
    pub fn show_notes(self: &Rc<Self>) {
        if crate::vault::current().is_none() {
            self.notes_button.set_active(false);
            return;
        }
        if !self.notes_mode.replace(true) {
            *self.agents_side.borrow_mut() = AgentsSide {
                agent: self.remote_in_view().is_some(),
                files: self.files.root.is_visible(),
                pinned: self.pinned_box.is_visible(),
                bar: self.bar_stack.is_visible(),
                dock: self.dock_stack.is_visible(),
                file: (self.files.open_path(), self.files.editor_box.is_visible()),
                tabs: self.files.swap_tabs(self.notes_tabs.take()),
            };
            self.side_stack.set_visible_child_name("notes");
            self.split.set_end_child(Some(&self.notes_panel.root));
            self.files.lend_notes(&self.hub.panel);
            self.notes_panel.history.append(&self.hub.panel);
            self.pinned_box.set_visible(false);
            self.bar_stack.set_visible(false);
            self.dock_stack.set_visible(false);
            let (file, open) = self.notes_file.borrow().clone();
            self.files.restore(file.as_deref(), open);
            self.notes_dot.set_visible(false);
            self.notes_button.set_active(true);
            self.notes_button.add_css_class("on");
            self.notes_panel.refresh();
            self.notes_panel.watch(true);
        }
        self.chat_stack.set_visible_child_name("notes-chat");
        self.notes_chat.open();
        self.refresh_sidebar();
        self.notes_chat.focus();
    }

    /// Out of the Notes view, the agents' side put back as it was (the view kept as it is, for
    /// next time). False: not in it.
    fn leave_notes(self: &Rc<Self>) -> bool {
        if !self.notes_mode.replace(false) {
            return false;
        }
        let side = self.agents_side.take();
        *self.notes_file.borrow_mut() = (self.files.open_path(), self.files.editor_box.is_visible());
        *self.notes_tabs.borrow_mut() = self.files.swap_tabs(side.tabs);
        self.notes_panel.watch(false);
        self.side_stack.set_visible_child_name("agents");
        self.notes_panel.history.remove(&self.hub.panel);
        self.files.take_back_notes(&self.hub.panel);
        self.split.set_end_child(Some(&self.files.root));
        self.files.root.set_visible(side.files);
        self.pinned_box.set_visible(side.pinned);
        self.bar_stack.set_visible(side.bar);
        self.dock_stack.set_visible(side.dock);
        self.files.restore(side.file.0.as_deref(), side.file.1);
        // A chat still waiting for an answer: the button says so.
        self.notes_dot.set_visible(self.notes_chat.waiting());
        self.notes_button.set_active(false);
        self.notes_button.remove_css_class("on");
        self.refresh_sidebar();
        true
    }

    /// Back to the agents: the one that was in view, else the overview.
    pub fn show_agents(self: &Rc<Self>) {
        let had = self.agents_side.borrow().agent;
        if !self.leave_notes() {
            return;
        }
        match self.remote_focused.get().filter(|_| had) {
            Some(id) => self.focus_remote(id),
            None => self.show_overview(),
        }
    }

    fn toggle_notes(self: &Rc<Self>) {
        if self.notes_mode.get() { self.show_agents() } else { self.show_notes() }
    }

    fn new_notes_chat(self: &Rc<Self>) {
        self.chat_stack.set_visible_child_name("notes-chat");
        self.notes_chat.new_chat();
    }

    /// The right panel on screen: the files panel, or in the Notes view its own.
    fn side_panel(&self) -> gtk::Widget {
        if self.notes_mode.get() { self.notes_panel.root.clone().upcast() } else { self.files.root.clone().upcast() }
    }

    /// Ctrl+E: the right panel on screen, shown or hidden.
    fn toggle_panel(&self) {
        let panel = self.side_panel();
        panel.set_visible(!panel.is_visible());
    }

    /// The files panel (a project's files, its changes…) is the agents' side's.
    fn leave_for_files(self: &Rc<Self>) {
        if self.notes_mode.get() {
            self.show_agents();
        }
    }

    /// Everything Scribe wrote in the notes, in the main area (no agent has the keyboard).
    pub fn show_notes_history(self: &Rc<Self>) {
        self.chat_stack.set_visible_child_name("notes");
        self.hub.fill_page();
        self.refresh_sidebar();
        self.hub.focus_page();
    }

    fn fill_overview(self: &Rc<Self>) {
        self.fill_remote_overview()
    }

    /// The overview of loomd's agents and schedules (the window as its client).
    fn fill_remote_overview(self: &Rc<Self>) {
        let read = |feed: &Feed| {
            feed.update();
            feed.items().clone()
        };
        let agents = self.sidebar_feeds.borrow().as_ref().map(|(agents, _)| read(agents)).unwrap_or_default();
        let (extra, schedules) = self.overview_feeds.borrow().as_ref().map(|(o, s)| (read(o), read(s))).unwrap_or_default();
        let time = |v: &Value| loom_core::api::seconds(v);
        let schedules: Vec<Schedule> = schedules
            .iter()
            .filter_map(|s| {
                Some(Schedule {
                    id: s["id"].as_str()?.parse().ok()?,
                    cwd: PathBuf::from(s["folder"].as_str().unwrap_or_default()),
                    session_id: s["session"].as_str().map(str::to_owned),
                    title: s["title"].as_str().unwrap_or_default().to_owned(),
                    prompt: s["prompt"].as_str().unwrap_or_default().to_owned(),
                    when: s["when"].as_str().unwrap_or_default().to_owned(),
                    next: time(&s["next"]),
                    paused: s["paused"].as_bool() == Some(true),
                    last_run: time(&s["last_run"]),
                })
            })
            .collect();
        let cards = agents
            .iter()
            .filter_map(|a| {
                let id: u32 = a["id"].as_str()?.parse().ok()?;
                let more = extra.iter().find(|e| e["id"] == a["id"]);
                let session = more.and_then(|m| m["session"].as_str());
                Some(crate::overview::Card {
                    id,
                    title: a["title"].as_str().unwrap_or_default().to_owned(),
                    project: a["project"].as_str().unwrap_or_default().to_owned(),
                    icon: a["project_id"].as_str().and_then(|p| self.icon_file(Path::new(p))),
                    place: a["place"].as_str().unwrap_or_default().to_owned(),
                    status: Status::from_css(a["status"].as_str().unwrap_or_default()),
                    unseen: a["unseen"] == true,
                    waiting: more.and_then(|m| m["waiting"].as_array()).into_iter().flatten().filter_map(|w| w.as_str().map(str::to_owned)).collect(),
                    reply: more.and_then(|m| m["reply"].as_str()).map(str::to_owned),
                    used: time(&a["used"]).map_or(0, |t| t.max(0) as u64),
                    scheduled: schedules.iter().filter(|s| s.session_id.is_some() && s.session_id.as_deref() == session).count(),
                })
            })
            .collect();
        self.overview.fill(cards, &schedules);
    }

    /// The overview again, if shown, keeping the card that had the keyboard.
    fn refresh_overview(self: &Rc<Self>) {
        if !self.showing_overview() || self.overview_due.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(400), move || {
            let Some(app) = weak.upgrade() else { return };
            app.overview_due.set(false);
            if app.showing_overview() {
                let focused = app.overview.focused_id();
                app.fill_overview();
                app.overview.focus_id(focused);
            }
        });
    }

    /// The overview's cards and scheduled prompts, wired to what they do.
    fn wire_overview(self: &Rc<Self>) {
        // A card's right-click: its agent's menu, as in the sidebar. The cards are rebuilt as
        // agents change: the menu hangs off the page.
        let weak = Rc::downgrade(self);
        *self.overview.on_menu.borrow_mut() = Some(Rc::new(move |(id, card, x, y): (u32, gtk::Widget, f64, f64)| {
            let Some(app) = weak.upgrade() else { return };
            let page = app.overview.root.clone();
            let (x, y) = card.compute_point(&page, &gtk::graphene::Point::new(x as f32, y as f32)).map_or((x, y), |p| (f64::from(p.x()), f64::from(p.y())));
            if let Some(menu) = app.remote_menu(id) {
                menu.popup(&page, x, y);
            }
        }));
        // Scribe's line: the Notes tab, beside the agent last looked at.
        let weak = Rc::downgrade(self);
        *self.overview.on_scribe.borrow_mut() = Some(Rc::new(move |()| {
            let Some(app) = weak.upgrade() else { return };
            let last = app.previous_remote.get().or_else(|| app.ids_in_order().first().copied());
            if let Some(id) = last {
                app.focus_remote(id);
            }
            app.files.root.set_visible(true);
            app.files.show_notes();
        }));
        let weak = Rc::downgrade(self);
        *self.overview.on_open.borrow_mut() = Some(Rc::new(move |id| {
            let Some(app) = weak.upgrade() else { return };
            app.focus_remote(id);
        }));
        let weak = Rc::downgrade(self);
        *self.overview.on_schedule_for.borrow_mut() = Some(Rc::new(move |id| {
            let Some(app) = weak.upgrade() else { return };
            app.schedule_for_remote(id);
        }));
        *self.overview.on_schedule.borrow_mut() = Some(Rc::new(move |(id, act)| {
            // loomd's schedules: acted on there.
            let act = match act {
                crate::overview::Act::RunNow => "run_now",
                crate::overview::Act::Pause => "pause",
                crate::overview::Act::Remove => "remove",
            };
            crate::conn::command("window", "schedule_act", json!({ "id": id.to_string(), "act": act }));
        }));
    }

    /// Asks for a prompt and when to send it to loomd's agent `id` (the window as its client).
    pub fn schedule_for_remote(self: &Rc<Self>, id: u32) {
        let feeds = self.sidebar_feeds.borrow();
        let Some((agents, _)) = feeds.as_ref() else { return };
        let Some(item) = agents.items().iter().find(|a| a["id"].as_str() == Some(id.to_string().as_str())).cloned() else { return };
        drop(feeds);
        let has_conversation = self.overview_items().iter().any(|e| e["id"] == item["id"] && e["session"].is_string());
        schedule::ask(self.window.upcast_ref(), item["title"].as_str().unwrap_or_default(), has_conversation, move |prompt, when, same| {
            crate::conn::command("window", "schedule_new", json!({ "agent": id.to_string(), "prompt": prompt, "when": when, "same": same }));
        });
    }

    /// Opens `project`'s page: nothing starts until a conversation is picked there.
    pub fn show_project(self: &Rc<Self>, project: &Path, cwd: &Path) {
        // Opened from an agent: the page's back button goes to it (from another project's
        // page, to the same one as before).
        match self.remote_in_view() {
            Some(id) => self.back_to.set(Some(id)),
            None if self.showing_project().is_none() => self.back_to.set(None),
            None => {}
        }
        let back = self.back_to.get().and_then(|id| self.remote_item(id));
        self.board.set_back(back.as_ref().and_then(|item| item["title"].as_str()));
        self.leave_notes();
        self.chat_stack.set_visible_child_name("project");
        self.history.show(project, cwd, &self.project_label(project));
        self.board.show(project, &self.project_label(project));
        let tray = self.tray_for(project);
        self.show_tray(project, &tray);
        self.files.show(cwd, project, &[]);
        // No agent here: the repos offered are the project's own, not the last agent's.
        self.files.set_repos(Vec::new());
        self.refresh_sidebar();
        self.history.grab_focus();
    }

    /// Picks up a past conversation (or goes to it, when it is open already).
    fn resume(self: &Rc<Self>, past: crate::history::Past) {
        // loomd's: picked up there (the one open, if it is), then shown here.
        let weak = Rc::downgrade(self);
        let params = json!({ "session": past.session_id, "folder": past.cwd, "title": past.title.unwrap_or_default() });
        crate::conn::command_then("window", "resume", params, move |answer| {
            let id = answer.and_then(|a| a["agent"].as_str().and_then(|a| a.parse::<u32>().ok()));
            if let (Some(app), Some(id)) = (weak.upgrade(), id) {
                app.focus_remote(id);
            }
        });
    }

    fn start_agent_in(self: &Rc<Self>, cwd: PathBuf) {
        // loomd's client: loomd starts it (it shows in the sidebar from `agents`), then it opens.
        let weak = Rc::downgrade(self);
        crate::conn::command_then("window", "new_agent", serde_json::json!({ "folder": cwd }), move |answer| {
            let id = answer.as_ref().and_then(|a| a["agent"].as_str()?.parse().ok());
            if let (Some(app), Some(id)) = (weak.upgrade(), id) {
                app.focus_remote(id);
            }
        });
    }

    fn reopen_closed(self: &Rc<Self>) {
        // loomd's: the newest of its `closed`, reopened there (it shows in `agents`).
        let newest = self.closed_feed.borrow().as_ref().and_then(|f| {
            f.update();
            f.items().first().cloned()
        });
        if let Some(session) = newest.and_then(|c| c["id"].as_str().map(str::to_owned)) {
            self.reopen(&session);
        }
    }

    fn close_focused(self: &Rc<Self>) {
        if let Some(id) = self.remote_in_view() {
            self.close_remote(id);
        }
    }

    /// loomd's agent `id` closed there (after asking, if it works); its chat goes once
    /// `agents` no longer has it.
    fn close_remote(self: &Rc<Self>, id: u32) {
        self.unless_stopping_remote(id, "Close", "The conversation itself is kept: Ctrl+Shift+T reopens it.", |_, id| {
            crate::conn::command("window", "close_agent", json!({ "agent": id.to_string() }));
        });
    }

    /// `then` at once, or after asking if loomd's agent `id` works (it would stop).
    fn unless_stopping_remote(self: &Rc<Self>, id: u32, action: &str, kept: &str, then: impl FnOnce(&Rc<App>, u32) + 'static) {
        let Some(item) = self.remote_item(id) else { return };
        if item["busy"] != true {
            then(self, id);
            return;
        }
        let dialog = gtk::AlertDialog::builder()
            .message(format!("“{}” is working", item["title"].as_str().unwrap_or_default()))
            .detail(format!("{action} stops what it is doing. {kept}"))
            .buttons(["Keep it", action])
            .cancel_button(0)
            .default_button(0)
            .build();
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), None::<&gio::Cancellable>, move |choice| {
            if let (Some(app), Ok(1)) = (weak.upgrade(), choice) {
                then(&app, id);
            }
        });
    }

    fn toggle_collapsed(self: &Rc<Self>, path: &Path) {
        let collapsed = self.remote_projects().iter().any(|p| p["id"].as_str() == Some(&*path.to_string_lossy()) && p["collapsed"] == true);
        crate::conn::command("window", "set_collapsed", json!({ "project": path, "collapsed": !collapsed }));
    }

    /// Takes a project out of the sidebar, closing its agents (asks first if any is busy).
    fn remove_project(self: &Rc<Self>, path: &Path) {
        self.remove_remote_project(path)
    }

    /// Scribe's news shown: the Notes tab marked, and switched to when the user isn't working
    /// in the panel; a question's conversation opened from it.
    fn wire_hub(self: &Rc<Self>) {
        let (weak, dot) = (Rc::downgrade(self), self.notes_dot.downgrade());
        *self.notes_chat.on_attention.borrow_mut() = Some(Rc::new(move |waiting: bool| {
            if let (Some(app), Some(dot)) = (weak.upgrade(), dot.upgrade()) {
                dot.set_visible(waiting && !app.notes_mode.get());
            }
        }));
        // A chat picked in the Notes view shows in the middle (History's page may be there).
        let weak = Rc::downgrade(self);
        *self.notes_chat.on_shown.borrow_mut() = Some(Rc::new(move || {
            if let Some(app) = weak.upgrade()
                && app.notes_mode.get()
            {
                app.chat_stack.set_visible_child_name("notes-chat");
            }
        }));
        let weak = Rc::downgrade(self);
        self.notes_button.connect_toggled(move |button| {
            let Some(app) = weak.upgrade() else { return };
            if button.is_active() != app.notes_mode.get() {
                if button.is_active() { app.show_notes() } else { app.show_agents() }
            }
        });
        let weak = Rc::downgrade(self);
        self.notes_back.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.show_agents();
            }
        });
        let weak = Rc::downgrade(self);
        *self.notes_panel.on_open.borrow_mut() = Some(Rc::new(move |path: PathBuf| {
            if let Some(app) = weak.upgrade() {
                app.files.open_file(&path);
            }
        }));
        let weak = Rc::downgrade(self);
        *self.notes_panel.on_conflict.borrow_mut() = Some(Rc::new(move |(copy, note): (String, String)| {
            let (Some(app), Some(vault)) = (weak.upgrade(), crate::vault::current()) else { return };
            let w = Rc::downgrade(&app);
            let merge: Rc<dyn Fn(String)> = Rc::new(move |text| {
                if let Some(app) = w.upgrade() {
                    app.chat_stack.set_visible_child_name("notes-chat");
                    app.notes_chat.new_chat_saying(&text);
                }
            });
            let w = Rc::downgrade(&app);
            let done: Rc<dyn Fn()> = Rc::new(move || {
                if let Some(app) = w.upgrade() {
                    app.notes_panel.refresh();
                }
            });
            app.files.conflict_window(&vault, &copy, &note, merge, done);
        }));
        let weak = Rc::downgrade(self);
        *self.notes_panel.on_chat.borrow_mut() = Some(Rc::new(move |(name, folder): (String, String)| {
            if let Some(app) = weak.upgrade() {
                app.chat_stack.set_visible_child_name("notes-chat");
                app.notes_chat.topic_chat(&name, &folder);
            }
        }));
        let weak = Rc::downgrade(self);
        *self.hub.on_attention.borrow_mut() = Some(Box::new(move || {
            let Some(app) = weak.upgrade() else { return };
            if app.notes_mode.get() {
                app.notes_panel.set_news();
                app.files.set_notes_news(true);
            } else if app.files.root.is_visible() && !app.files.in_use() {
                app.files.show_notes();
            } else {
                app.files.set_notes_news(true);
            }
        }));
        let weak = Rc::downgrade(self);
        *self.hub.on_history.borrow_mut() = Some(Box::new(move || {
            if let Some(app) = weak.upgrade() {
                app.show_notes_history();
            }
        }));
        let weak = Rc::downgrade(self);
        *self.hub.on_open.borrow_mut() = Some(Box::new(move |session: &str| {
            let Some(app) = weak.upgrade() else { return };
            // loomd's agent with that conversation open (`overview` says which)
            let id = app.overview_items().iter().find(|o| o["session"].as_str() == Some(session)).and_then(|o| o["id"].as_str()?.parse::<u32>().ok());
            if let Some(id) = id {
                app.focus_remote(id);
            }
        }));
        let weak = Rc::downgrade(self);
        *self.hub.on_changed.borrow_mut() = Some(Box::new(move || {
            if let Some(app) = weak.upgrade() {
                let (questions, waiting) = app.hub.counts();
                app.overview.set_scribe(questions, waiting);
                let weak = Rc::downgrade(&app);
                // After the panel is rebuilt (not inside it).
                glib::idle_add_local_once(move || {
                    if let Some(app) = weak.upgrade() {
                        app.refresh_sidebar();
                    }
                });
            }
        }));
        settings::on_change({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(app) = weak.upgrade() {
                    app.files.set_notes_available(crate::vault::current().is_some());
                    app.hub.refresh();
                }
            }
        });
    }

    fn wire_toast(self: &Rc<Self>, go: &gtk::Button, close: &gtk::Button) {
        let weak = Rc::downgrade(self);
        go.connect_clicked(move |_| {
            let Some(app) = weak.upgrade() else { return };
            let remote = app.toast_remote.get();
            app.hide_toast();
            if let Some(id) = remote {
                app.focus_remote(id);
            }
        });
        let weak = Rc::downgrade(self);
        close.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.hide_toast();
            }
        });
    }

    /// The same, about one of loomd's agents (the window as its client).
    pub fn show_remote_toast(self: &Rc<Self>, id: u32, text: &str, question: bool) {
        if self.toast_waits() && !question {
            return;
        }
        self.toast_remote.set(Some(id));
        self.reveal_toast(text, question);
    }

    /// The toast shows a question still waiting (a lesser word doesn't replace it).
    fn toast_waits(&self) -> bool {
        let remote = self.toast_remote.get().is_some_and(|id| self.remote_status.borrow().get(&id).is_some_and(|s| s == "blocked"));
        self.toast_question.get() && self.toast.reveals_child() && remote
    }

    fn reveal_toast(self: &Rc<Self>, text: &str, question: bool) {
        self.toast_label.set_label(text);
        // "Show" only where there is an agent to show
        self.toast_go.set_visible(self.toast_remote.get().is_some());
        self.toast_question.set(question);
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
        self.toast_remote.set(None);
        if let Some(source) = self.toast_timeout.take() {
            source.remove();
        }
    }

    fn close_request(self: &Rc<Self>) -> glib::Propagation {
        // loomd's client: closing stops the loomd it started, unless its agents are to keep
        // working (one that ran before, a service, has its own reasons to run).
        let stops_loomd = !settings::get().keep_working && crate::conn::started_loomd();
        // Only a turn in flight is lost by quitting (an agent still starting just restarts).
        let mut busy: Vec<String> = Vec::new();
        if stops_loomd {
            busy.extend(self.sidebar_feeds.borrow().as_ref().map(|(agents, _)| agents.items().iter().filter(|a| a["busy"] == true || a["status"] == "blocked").filter_map(|a| a["title"].as_str().map(str::to_owned)).collect::<Vec<_>>()).unwrap_or_default());
        }
        // loomd's jobs go on with it, unless it stops.
        let jobs: usize = if !stops_loomd { 0 } else { self.trays.borrow().values().map(|t| t.running()).sum() };
        let ask = ((!busy.is_empty() || jobs > 0) && settings::get().confirm_quit) || self.files.has_unsaved();
        if !self.quitting.get() && ask {
            let mut what = Vec::new();
            if !busy.is_empty() {
                what.push(format!("{} agent{} working: {}", busy.len(), if busy.len() == 1 { " is" } else { "s are" }, busy.join(", ")));
            }
            if jobs > 0 {
                what.push(format!("{jobs} job{} running in the tray", if jobs == 1 { " is" } else { "s are" }));
            }
            if self.files.has_unsaved() {
                what.push("a file has unsaved changes".to_owned());
            }
            let what = what.join("; ");
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
        // loomd's client: loomd stopped (its agents with it), or told at once that nobody
        // looks (it says things on the desktop again; else a minute after the last word).
        if stops_loomd {
            if let Err(err) = stop_loomd_now(&loom_core::api::socket::socket_path(&self.app_id)) {
                crate::log::write("loomd", &err);
            }
        } else {
            crate::conn::command_result("window", "set_looking", json!({ "active": false, "agents": [] }), |_| {});
        }
        glib::Propagation::Proceed
    }

    // ---- what agents ask of loom (its MCP tools) ------------------------------------------

    // ---- sidebar, menus, inbox, welcome ---------------------------------------------------

    pub fn activate_row(self: &Rc<Self>, index: usize) {
        enum Do {
            Remote(u32),
            Start(PathBuf),
        }
        let what = match self.rows.borrow().get(index) {
            Some(Row::Project(path)) => Do::Start(path.clone()),
            Some(Row::Remote(id)) => Do::Remote(*id),
            None => return,
        };
        match what {
            Do::Remote(id) => self.focus_remote(id),
            // A folded project opens into its card; one with no agents, its page (to start one).
            Do::Start(path) if self.remote_items().iter().any(|a| a["project_id"].as_str() == Some(&*path.to_string_lossy())) => self.toggle_collapsed(&path),
            Do::Start(path) => self.show_project(&path, &path),
        }
    }

    /// The right-click menu of row `index` (None: the empty part of the list), at (x, y) in
    /// the list.
    /// Project `path`'s menu at (x, y) in the list (an open project has no row of its own).
    fn project_menu(self: &Rc<Self>, path: &Path, x: f64, y: f64) {
        self.row_menu_for(Some(Row::Project(path.to_owned())), x, y);
    }

    fn row_menu(self: &Rc<Self>, index: Option<usize>, x: f64, y: f64) {
        let row = index.and_then(|i| self.rows.borrow().get(i).cloned());
        self.row_menu_for(row, x, y);
    }

    fn row_menu_for(self: &Rc<Self>, row: Option<Row>, x: f64, y: f64) {
        use crate::menu::Menu;
        let weak = Rc::downgrade(self);
        // An action on the app, run if it is still there.
        let on = |f: Box<dyn Fn(&Rc<App>)>| {
            let weak = weak.clone();
            move || {
                if let Some(app) = weak.upgrade() {
                    f(&app);
                }
            }
        };
        let remote = match &row {
            Some(Row::Remote(id)) => Some(*id),
            _ => None,
        };
        let row = match row {
            Some(Row::Project(path)) => Some(path),
            _ => None,
        };
        if let Some(id) = remote {
            if let Some(menu) = self.remote_menu(id) {
                menu.popup(&self.list, x, y);
            }
            return;
        }
        let menu = match row {
            Some(path) => {
                let p = || path.clone();
                let collapsed = self.remote_projects().iter().any(|p| p["id"].as_str() == Some(&*path.to_string_lossy()) && p["collapsed"] == true);
                let (p1, p2, p3, p4, p5, p6, p7) = (p(), p(), p(), p(), p(), p(), p());
                let list = self.list.clone();
                Menu::new()
                    .item("New agent here", on(Box::new(move |app| app.start_agent_in(p1.clone()))))
                    .item("New agent in a worktree…", {
                        let (weak, at) = (Rc::downgrade(self), path.clone());
                        move || {
                            if let Some(app) = weak.upgrade() {
                                app.new_worktree_at(&at, x, y);
                            }
                        }
                    })
                    .item("Board and conversations…", on(Box::new(move |app| app.show_project(&p2, &p2))))
                    .section()
                    .item("Open in the file manager", move || crate::menu::launch(&list, &p3))
                    .item("Copy its path", {
                        let list = self.list.clone();
                        move || crate::menu::copy(&list, &p4.to_string_lossy())
                    })
                    .item("Scratch terminal here", on(Box::new(move |app| {
                        app.scratch.hide();
                        app.scratch.toggle(&p5);
                    })))
                    .section()
                    .item("Rename…", on(Box::new(move |app| app.rename_project_at(&p6, x, y))))
                    .item("Icon colour…", { let at = path.clone(); on(Box::new(move |app| app.pick_icon_colour(&at))) })
                    .item("Icon picture…", { let at = path.clone(); on(Box::new(move |app| app.pick_icon_picture(&at))) })
                    .item("Automatic icon", {
                        let at = path.clone();
                        on(Box::new(move |app| app.set_icon(&at, |p| loom_core::project_icon::choose(p, None).and_then(|()| loom_core::project_icon::pick(p, None)))))
                    })
                    .enabled(loom_core::project_icon::custom(&path))
                    .item(if collapsed { "Show its agents" } else { "Hide its agents" }, on(Box::new(move |app| app.toggle_collapsed(&p7))))
                    .section()
                    .item("Remove from the sidebar", on(Box::new(move |app| app.remove_project(&path))))
                    .danger()
            }
            // The empty part of the list.
            _ => {
                let projects = self.remote_projects();
                let (any_shut, any_open) = (projects.iter().any(|p| p["collapsed"] == true), projects.iter().any(|p| p["collapsed"] != true));
                Menu::new()
                    .item("New agent…", on(Box::new(|app| app.new_agent(false))))
                    .hint("Ctrl+N")
                    .item("Open a project folder…", on(Box::new(|app| app.new_agent(true))))
                    .hint("Ctrl+Shift+N")
                    .section()
                    .item("Show every project's agents", on(Box::new(|app| app.collapse_all(false))))
                    .enabled(any_shut)
                    .item("Hide every project's agents", on(Box::new(|app| app.collapse_all(true))))
                    .enabled(any_open)
            }
        };
        menu.popup(&self.list, x, y);
    }

    pub fn row_menu_for_tests(self: &Rc<Self>, index: Option<usize>) {
        let y = index.and_then(|i| self.list.row_at_index(i as i32)).and_then(|r| r.compute_bounds(&self.list)).map_or(400.0, |b| f64::from(b.y() + 10.0));
        self.row_menu(index, 60.0, y);
    }

    /// An agent's right-click menu (its sidebar row, its overview card).
    /// The right-click menu of loomd's agent `id`: what the window's own agents' has, done
    /// as commands to loomd.
    fn remote_menu(self: &Rc<Self>, id: u32) -> Option<crate::menu::Menu> {
        use crate::menu::Menu;
        let item = self.remote_item(id)?;
        let weak = Rc::downgrade(self);
        let with = |f: fn(&Rc<App>, u32)| {
            let weak = weak.clone();
            move || {
                if let Some(app) = weak.upgrade() {
                    f(&app, id);
                }
            }
        };
        let say = |method: &'static str, extra: Value| {
            move || {
                let mut params = extra.clone();
                params["agent"] = json!(id.to_string());
                crate::conn::command("window", method, params);
            }
        };
        let focused = self.remote_in_view() == Some(id);
        let suspended = item["status"] == "suspended";
        let awake = item["awake"] == true;
        let cwd = PathBuf::from(item["cwd"].as_str().or(item["project_id"].as_str()).unwrap_or_default());
        let (list, list2, cwd2) = (self.list.clone(), self.list.clone(), cwd.clone());
        // In a worktree loom made for it: merged back, then removed (what git said, in a toast).
        let worktree = loom_core::worktrees::made_from(&cwd).is_some();
        let git = |method: &'static str| {
            let weak = weak.clone();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.worktree_git(id, method);
                }
            }
        };
        let menu = Menu::new()
                .item("Open", with(|app, id| app.focus_remote(id)))
                .enabled(!focused)
                .item("Show beside the focused one", with(|app, id| app.pin_remote(id)))
                .enabled(!focused)
                .item(if awake { "Allow suspending when idle" } else { "Keep awake" }, say("set_awake", json!({ "awake": !awake })))
                .section()
                .item("Rename…", with(|app, id| {
                    app.focus_remote(id);
                    app.begin_rename();
                }))
                .hint("F2")
                .item("Open its folder in the file manager", move || crate::menu::launch(&list, &cwd))
                .item("Copy its folder's path", move || crate::menu::copy(&list2, &cwd2.to_string_lossy()))
                .item("Schedule a prompt…", with(|app, id| app.schedule_for_remote(id)))
                .item("Fold its project", {
                    let (weak, project) = (Rc::downgrade(self), PathBuf::from(item["project_id"].as_str().unwrap_or_default()));
                    move || {
                        if let Some(app) = weak.upgrade() {
                            app.toggle_collapsed(&project);
                        }
                    }
                });
        let menu = if worktree {
            menu.section()
                .item("Merge its branch back", git("worktree_merge"))
                .item("Remove its worktree", git("worktree_remove"))
                .enabled(item["busy"] != true)
        } else {
            menu
        };
        Some(
            menu.section()
                .item("Suspend (free its memory)", say("suspend", json!({})))
                .enabled(!suspended && item["busy"] != true)
                .item("Restart", with(|app, id| {
                    app.unless_stopping_remote(id, "Restart", "The conversation goes on after it.", |_, id| {
                        crate::conn::command("window", "restart", json!({ "agent": id.to_string() }));
                    });
                }))
                .item("Close", with(|app, id| app.close_remote(id)))
                .hint("Ctrl+W")
                .danger(),
        )
    }

    /// "agent — project · loom", for the window list and the taskbar.
    fn refresh_title(&self) {
        let item = self.remote_in_view().and_then(|id| self.remote_item(id));
        let title = match (item, self.showing_project()) {
            _ if self.notes_mode.get() => "Notes · loom".to_owned(),
            (Some(item), _) => format!("{} — {} · loom", item["title"].as_str().unwrap_or_default(), item["project"].as_str().unwrap_or_default()),
            (None, Some(project)) => format!("{} · loom", self.project_label(&project)),
            _ => "loom".to_owned(),
        };
        // Agents waiting for you, first: the window list and task switcher show it.
        let waiting = self.remote_items().iter().filter(|a| a["status"] == "blocked").count();
        let title = if waiting > 0 { format!("({waiting}) needs you · {title}") } else { title };
        self.window.set_title(Some(&title));
    }

    /// The sidebar subscribes to the core's `agents` and `projects` (warp): what comes is
    /// drawn once the window is idle.
    fn follow_sidebar(self: &Rc<Self>) {
        let ready = || {
            let weak = Rc::downgrade(self);
            move || {
                if let Some(app) = weak.upgrade() {
                    app.sidebar_soon();
                }
            }
        };
        let agents = Feed::new("agents", ready());
        let projects = Feed::new("projects", ready());
        *self.sidebar_feeds.borrow_mut() = agents.zip(projects);
    }

    /// The sidebar drawn once the window is idle (what changed since drawn together).
    fn sidebar_soon(self: &Rc<Self>) {
        if !self.sidebar_due.replace(true) {
            let weak = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                if let Some(app) = weak.upgrade()
                    && app.sidebar_due.get()
                {
                    app.draw_sidebar();
                }
            });
        }
    }

    /// The sidebar drawn now (from what loomd last sent).
    fn refresh_sidebar(self: &Rc<Self>) {
        self.sidebar_drawn.take();
        self.draw_sidebar();
    }

    /// The sidebar drawn from its copies of `agents` and `projects`.
    fn draw_sidebar(self: &Rc<Self>) {
        self.sidebar_due.set(false);
        // loomd's agents whose status changed (the window as its client), and a copy asked for
        let mut changed = Vec::new();
        let mut copy = None;
        let (agents, projects): (Vec<SideAgent>, Vec<SideProject>) = {
            let feeds = self.sidebar_feeds.borrow();
            let Some((agents, projects)) = feeds.as_ref() else { return };
            agents.update();
            projects.update();
            for item in agents.items().iter() {
                // its session settings: what Settings' defaults offer
                crate::prefs::seen_options(&item["options"]);
                let Some(id) = item["id"].as_str().and_then(|id| id.parse::<u32>().ok()) else { continue };
                if let Some(agent) = self.remote_agents.borrow().get(&id).cloned() {
                    agent.update(item);
                }
                let status = item["status"].as_str().unwrap_or_default().to_owned();
                match self.remote_status.borrow_mut().insert(id, status.clone()) {
                    Some(before) if before != status => changed.push((id, status, item.clone())),
                    // New in the list: the copy asked for, if one was.
                    None if self.expect_copy.replace(false) => copy = Some(id),
                    _ => {}
                }

            }
            (agents.items().iter().filter_map(SideAgent::from).collect(), projects.items().iter().filter_map(SideProject::from).collect())
        };
        for (id, status, item) in changed {
            self.remote_changed(id, &status, &item);
        }
        self.drop_closed_remote();
        self.refresh_inbox();

        if let Some(id) = copy {
            self.focus_remote(id);
        }
        self.refresh_title();
        // The agent in view: its files again when it touched more, and its session for Notes.
        if let Some(id) = self.remote_in_view() {
            let edited = agents.iter().find(|a| a.id == id).map_or(0, |a| a.edited);
            if self.fed_files.get() != (id, edited) {
                self.feed_files(id);
            }
            self.tell_hub(id);
        }
        if self.notes_mode.get() {
            let (mut working, mut waiting) = (0, 0);
            for agent in &agents {
                match agent.status {
                    Status::Working => working += 1,
                    Status::Blocked => waiting += 1,
                    _ => {}
                }
            }
            let mut text = "←  Agents".to_owned();
            if working > 0 {
                text.push_str(&format!(" · {working} working"));
            }
            if waiting > 0 {
                text.push_str(&format!(" · {waiting} waiting"));
            }
            if let Some(label) = self.notes_back.child().and_downcast::<gtk::Label>() {
                label.set_label(&text);
            }
        }
        let overview = self.overview_items();
        // Each running row's second line: what it waits on, why it failed, or its last words.
        let said: Vec<(u32, String)> = agents.iter().filter(|a| a.status != Status::Suspended).map(|a| (a.id, said_line(a, &overview))).collect();
        let built_from = Some((agents.clone(), projects.clone(), said.clone()));
        if *self.sidebar_drawn.borrow() == built_from {
            return;
        }
        *self.sidebar_drawn.borrow_mut() = built_from;
        self.apply_sections();
        // Rebuilt in place: each list stays scrolled where it was.
        let scrolls: Vec<(gtk::Adjustment, f64)> = [&self.run_scroller, &self.list_scroller].iter().map(|s| (s.vadjustment(), s.vadjustment().value())).collect();
        for list in [&self.list, &self.run_list] {
            while let Some(row) = list.row_at_index(0) {
                list.remove(&row);
            }
        }
        let mut rows = Vec::new();
        let mut edges = HashMap::new();
        let focused = self.remote_focused.get();
        // Conversations Scribe has a question about, by agent (`overview` says whose is whose).
        let asked: Vec<u32> = overview.iter().filter(|o| o["session"].as_str().is_some_and(|s| self.hub.asks_about(s))).filter_map(|o| o["id"].as_str()?.parse().ok()).collect();
        // The running agents, numbered (Ctrl+1…9) in the order dragged, each with its ☕. On its
        // project's card a running one is a dim line: one full row each.
        let order = self.running_order(agents.iter().filter(|a| a.status != Status::Suspended).map(|a| a.id).collect());
        let running: Vec<&SideAgent> = order.iter().filter_map(|id| agents.iter().find(|a| a.id == *id)).collect();
        let mut running_rows = Vec::new();
        for (i, item) in running.iter().enumerate() {
            let line = said.iter().find(|(id, _)| *id == item.id).map_or("", |(_, line)| line.as_str());
            let row = self.running_row(item, i, line);
            if focused == Some(item.id) {
                row.add_css_class("current");
            }
            self.run_list.append(&row);
            running_rows.push(row);
        }
        *self.running_rows.borrow_mut() = running_rows;
        self.run_head.title.set_label(&format!("Running · {}", running.len()));
        let count = |f: &dyn Fn(&SideAgent) -> bool| running.iter().filter(|a| f(a)).count();
        let counts = [
            (count(&|a| a.status == Status::Blocked), "waiting"),
            (count(&|a| a.status == Status::Failed), "failed"),
            (count(&|a| a.status == Status::Idle && a.unseen), "done"),
        ];
        for (label, (n, word)) in self.run_head.counts.iter().zip(counts) {
            label.set_label(&format!("● {n} {word}"));
            label.set_visible(n > 0);
        }
        self.proj_head.title.set_label(&format!("Projects · {}", projects.len()));
        let mut cards = Vec::new();
        for project in &projects {
            let mine: Vec<&SideAgent> = agents.iter().filter(|a| a.project == project.path).collect();
            // Closed (or with no agents): one pill, its name. Open: a card of its agents, its
            // name up the left edge (drawn by `draw_card_names`; a click there opens its page).
            if project.collapsed || mine.is_empty() {
                let header = self.project_row(project, &mine);
                header.add_css_class("alone");
                self.project_drop_zone(&header, &project.path, true);
                self.list.append(&header);
                rows.push(Row::Project(project.path.clone()));
                edges.insert(project.path.clone(), (header.clone(), header));
                continue;
            }
            let count = mine.len();
            let alike = |agent: &SideAgent| mine.iter().filter(|a| a.title == agent.title).count() > 1;
            let alike: Vec<bool> = mine.iter().map(|a| alike(a)).collect();
            let mut first = None;
            let mut last = None;
            for (i, item) in mine.into_iter().enumerate() {
                let row = agent_row(item, alike[i], asked.contains(&item.id));
                row.add_css_class("in-card");
                if let Some(at) = order.iter().position(|id| *id == item.id) {
                    row.add_css_class("in-running");
                    let key = if at < 9 { format!(" (Ctrl+{})", at + 1) } else { String::new() };
                    row.set_tooltip_text(Some(&format!("Running: its row is in Running above{key}")));
                }
                if i == 0 {
                    row.add_css_class("first");
                    first = Some(row.clone());
                }
                if i + 1 == count {
                    row.add_css_class("last");
                    last = Some(row.clone());
                }
                self.project_drop_zone(&row, &project.path, i == 0);
                self.list.append(&row);
                if focused == Some(item.id) {
                    self.list.select_row(Some(&row));
                }
                rows.push(Row::Remote(item.id));
            }
            let (Some(first), Some(last)) = (first, last) else { continue };
            cards.push((project.path.clone(), project.name.clone(), first.clone(), last.clone()));
            edges.insert(project.path.clone(), (first, last));
        }
        *self.project_edges.borrow_mut() = edges;
        *self.cards.borrow_mut() = cards;
        self.card_buttons();
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
        for (adjustment, value) in scrolls {
            adjustment.set_value(value);
        }
    }

    /// The open cards' name strips, made again with the list (real widgets: in the
    /// focus order and named for screen readers, as drawn text never is).
    // shortcut: they come after the list in Tab order; one focus order with the rows if that bites
    fn card_buttons(self: &Rc<Self>) {
        for (widget, ..) in self.card_spots.take() {
            self.card_layer.remove_overlay(&widget);
        }
        let mut spots = Vec::new();
        for (path, name, first, last) in self.cards.borrow().iter() {
            let label = name.clone();
            // As tall as its agents: a longer name is cut short ("…"; in full in the tooltip).
            let spine = crate::tube::Tube::new();
            spine.set_draw(move |tube, snapshot| {
                // Under the icon a one-agent card has no room for it: the icon's letter says it.
                if tube.height() < 48 {
                    return;
                }
                let layout = name_layout(tube, &label, tube.height().max(10));
                let (w, h) = layout.pixel_size();
                snapshot.save();
                snapshot.translate(&gtk::graphene::Point::new((tube.width() - h) as f32 / 2.0, (tube.height() + w) as f32 / 2.0));
                snapshot.rotate(-90.0);
                snapshot.append_layout(&layout, &tube.color());
                snapshot.restore();
            });
            // Its page is the project's name in the chat's header now: this folds the card.
            spine.set_vexpand(true);
            let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
            column.append(&self.project_icon(path, 32));
            column.append(&spine);
            let strip = gtk::Button::builder().child(&column).tooltip_text(format!("Fold {name} (its board and conversations: its name in the header)")).build();
            strip.add_css_class("card-strip");
            strip.set_cursor_from_name(Some("pointer"));
            strip.update_property(&[gtk::accessible::Property::Label(&format!("Fold {name}"))]);
            let (weak, open) = (Rc::downgrade(self), path.clone());
            strip.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.toggle_collapsed(&open);
                }
            });
            // dragged onto another project: it goes there (as a closed one's pill)
            let source = gtk::DragSource::builder().actions(gdk::DragAction::MOVE).build();
            let dragged = path.to_string_lossy().into_owned();
            source.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&format!("loom-project:{dragged}").to_value())));
            let shown = first.clone();
            source.connect_drag_begin(move |source, _| source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&shown))), 12, 12));
            strip.add_controller(source);
            let (weak, menu, at) = (Rc::downgrade(self), path.clone(), strip.clone());
            crate::menu::on_right_click(&strip, move |x, y| {
                let Some(app) = weak.upgrade() else { return };
                let point = at.compute_point(&app.list, &gtk::graphene::Point::new(x as f32, y as f32)).map_or((x, y), |p| (f64::from(p.x()), f64::from(p.y())));
                app.project_menu(&menu, point.0, point.1);
            });
            strip.update_state(&[gtk::accessible::State::Expanded(Some(true))]);
            self.card_layer.add_overlay(&strip);
            spots.push((strip.upcast(), first.clone(), last.clone()));
        }
        *self.card_spots.borrow_mut() = spots;
    }

    /// From a project's page back to the agent it was opened from (false: not on one, or
    /// that agent is gone).
    fn go_back(self: &Rc<Self>) -> bool {
        let Some(id) = self.back_to.get().filter(|_| self.showing_project().is_some()) else { return false };
        if self.remote_item(id).is_none() {
            return false;
        }
        self.focus_remote(id);
        true
    }

    /// The sections open or folded as the settings say. Projects folded: Running takes the
    /// height and Projects' header sits at the bottom (as VS Code's folded sections).
    fn apply_sections(&self) {
        let s = settings::get();
        let (run, projects) = (s.show_running, s.projects_open);
        self.run_head.set_open(run);
        self.proj_head.set_open(projects);
        self.run_scroller.set_visible(run);
        self.list_scroller.set_visible(projects);
        self.run_scroller.set_vexpand(run && !projects);
        self.run_scroller.set_max_content_height(if projects { self.run_cap.get() } else { -1 });
        self.sections_filler.set_visible(!run && !projects);
    }

    /// A project's header: open/closed arrow, name, what its agents are up to, and +.
    fn project_row(self: &Rc<Self>, project: &SideProject, agents: &[&SideAgent]) -> gtk::ListBoxRow {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.append(&self.project_icon(&project.path, 32));
        let name = gtk::Label::builder().label(&project.name).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
        name.add_css_class("project-name");
        line.append(&name);
        let waiting = agents.iter().filter(|a| a.status == Status::Blocked).count();
        let working = agents.iter().filter(|a| matches!(a.status, Status::Working | Status::Starting)).count();
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
        add.set_tooltip_text(Some(&format!("New or past conversation in {}", project.name)));
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
        // Dragged by its header onto another project's: it goes there (the order is kept).
        let source = gtk::DragSource::builder().actions(gdk::DragAction::MOVE).build();
        let dragged = project.path.to_string_lossy().into_owned();
        source.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&format!("loom-project:{dragged}").to_value())));
        let header = row.clone();
        source.connect_drag_begin(move |source, _| {
            source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&header))), 12, 12);
        });
        row.add_controller(source);
        row
    }

    /// A row of project `path` takes a dragged project: over the top half of its header, it
    /// goes before this project; anywhere lower in it, after. A line shows where.
    fn project_drop_zone(self: &Rc<Self>, row: &gtk::ListBoxRow, path: &Path, header: bool) {
        let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        target.set_preload(true);
        let before = |row: &gtk::ListBoxRow, y: f64, header: bool| header && y < f64::from(row.height()) / 2.0;
        let (weak, onto, zone) = (Rc::downgrade(self), path.to_owned(), row.clone());
        target.connect_motion(move |target, _, y| {
            let project = target.value().and_then(|v| v.get::<String>().ok()).is_some_and(|v| v.starts_with("loom-project:"));
            if let Some(app) = weak.upgrade()
                && project
            {
                app.show_drop_line(&onto, before(&zone, y, header));
                return gdk::DragAction::MOVE;
            }
            gdk::DragAction::empty()
        });
        let weak = Rc::downgrade(self);
        target.connect_leave(move |_| {
            if let Some(app) = weak.upgrade() {
                app.clear_drop_line();
            }
        });
        let (weak, onto, zone) = (Rc::downgrade(self), path.to_owned(), row.clone());
        target.connect_drop(move |_, value, _, y| {
            let Some(app) = weak.upgrade() else { return false };
            app.clear_drop_line();
            let Some(from) = value.get::<String>().ok().and_then(|v| v.strip_prefix("loom-project:").map(PathBuf::from)) else { return false };
            app.put_project(&from, &onto, before(&zone, y, header));
            true
        });
        row.add_controller(target);
    }

    /// The Running list's order, given the agents running now (`live`).
    fn running_order(&self, live: Vec<u32>) -> Vec<u32> {
        let mut order = self.running.borrow_mut();
        order.retain(|id| live.contains(id));
        for id in live {
            if !order.contains(&id) {
                order.push(id);
            }
        }
        order.clone()
    }

    /// A row of the Running list: its number (Ctrl+1…9), dot, name, project, and the ☕ that
    /// keeps it awake. Dragged, it moves within the list.
    fn running_row(self: &Rc<Self>, agent: &SideAgent, index: usize, said: &str) -> gtk::ListBoxRow {
        // Two lines on one grid: bead, title and its project's chip; then what it does, its
        // Ctrl number and ☕ (each item on its line's own middle).
        let line = gtk::Grid::builder().column_spacing(8).row_spacing(2).build();
        let number = gtk::Label::new(Some(&if index < 9 { (index + 1).to_string() } else { String::new() }));
        number.add_css_class("running-number");
        number.set_width_chars(1);
        let dot = gtk::Box::builder().valign(gtk::Align::Center).build();
        dot.add_css_class("dot");
        let status = agent.status;
        dot.add_css_class(if status == Status::Idle && agent.unseen { "done" } else { status.css() });
        bead_named(&dot, agent);
        let title = gtk::Label::builder().label(&agent.title).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        title.add_css_class("agent-title");
        title.set_hexpand(true);
        // under its name: what it waits on, why it failed or what it said last, and when
        let sub = gtk::Label::builder().label(said).xalign(0.0).ellipsize(pango::EllipsizeMode::End).tooltip_text(said).build();
        sub.add_css_class("dim");
        sub.add_css_class("small");
        sub.add_css_class("agent-sub");
        // Its project, a chip: clicked, the project's page (as its name in the header).
        let open = format!("Open {}: its board and conversations", agent.project_name);
        let project = gtk::Button::builder()
            .child(&gtk::Label::builder().label(&agent.project_name).max_width_chars(10).ellipsize(pango::EllipsizeMode::End).build())
            .focus_on_click(false)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::End)
            .tooltip_text(&open)
            .build();
        project.add_css_class("flat");
        project.add_css_class("running-project");
        project.set_cursor_from_name(Some("pointer"));
        project.update_property(&[gtk::accessible::Property::Label(&open)]);
        let (weak, path) = (Rc::downgrade(self), agent.project.clone());
        project.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.show_project(&path, &path);
            }
        });
        // Not a focus stop: clicked, the sidebar is rebuilt, and focus left on it would land on
        // another row (scrolling the list away).
        let awake = gtk::Button::builder().child(&crate::logo::cup(14, agent.awake)).focus_on_click(false).build();
        awake.update_property(&[gtk::accessible::Property::Label("Keep awake")]);
        awake.add_css_class("flat");
        awake.add_css_class("awake");
        if agent.awake {
            awake.add_css_class("on");
        }
        awake.set_tooltip_text(Some(if agent.awake { "Kept awake: never suspended while idle (click to allow it)" } else { "Keep it awake: never suspended while idle" }));
        let (weak, target) = (Rc::downgrade(self), agent.id);
        let was_awake = agent.awake;
        awake.connect_clicked(move |_| {
            if weak.upgrade().is_some() {
                crate::conn::command("window", "set_awake", json!({ "agent": target.to_string(), "awake": !was_awake }));
            }
        });
        let end = gtk::Box::builder().spacing(4).halign(gtk::Align::End).valign(gtk::Align::Center).build();
        end.append(&number);
        end.append(&awake);
        let icon = self.project_icon(&agent.project, 32);
        icon.set_valign(gtk::Align::Center);
        line.attach(&icon_with_bead(&icon, &dot), 0, 0, 1, 2);
        line.attach(&title, 1, 0, 1, 1);
        line.attach(&project, 2, 0, 1, 1);
        line.attach(&sub, 1, 1, 1, 1);
        line.attach(&end, 2, 1, 1, 1);
        let row = gtk::ListBoxRow::builder().child(&line).build();
        row.add_css_class("running-row");
        // Reordered by dragging: before or after the row it is dropped on (a line shows where).
        let source = gtk::DragSource::builder().actions(gdk::DragAction::MOVE).build();
        let id = agent.id;
        source.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&format!("loom-running:{id}").to_value())));
        let shown = row.clone();
        source.connect_drag_begin(move |source, _| source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&shown))), 12, 12));
        row.add_controller(source);
        let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        target.set_preload(true);
        let ours = |target: &gtk::DropTarget| target.value().and_then(|v| v.get::<String>().ok()).is_some_and(|v| v.starts_with("loom-running:"));
        let (weak, zone) = (Rc::downgrade(self), row.clone());
        target.connect_motion(move |target, _, y| {
            let Some(app) = weak.upgrade() else { return gdk::DragAction::empty() };
            app.clear_running_line();
            if !ours(target) {
                return gdk::DragAction::empty();
            }
            zone.add_css_class(if y < f64::from(zone.height()) / 2.0 { "drop-before" } else { "drop-after" });
            gdk::DragAction::MOVE
        });
        let weak = Rc::downgrade(self);
        target.connect_leave(move |_| {
            if let Some(app) = weak.upgrade() {
                app.clear_running_line();
            }
        });
        let (weak, zone, onto) = (Rc::downgrade(self), row.clone(), agent.id);
        target.connect_drop(move |_, value, _, y| {
            let Some(app) = weak.upgrade() else { return false };
            app.clear_running_line();
            let Some(from) = value.get::<String>().ok().and_then(|v| v.strip_prefix("loom-running:").and_then(|id| id.parse::<u32>().ok())) else {
                return false;
            };
            let before = y < f64::from(zone.height()) / 2.0;
            {
                let mut order = app.running.borrow_mut();
                if from == onto {
                    return true;
                }
                order.retain(|id| *id != from);
                let at = order.iter().position(|id| *id == onto).unwrap_or(order.len());
                order.insert(if before { at } else { at + 1 }, from);
            }
            app.refresh_sidebar();
            true
        });
        row.add_controller(target);
        row
    }

    fn clear_running_line(&self) {
        for row in self.running_rows.borrow().iter() {
            row.remove_css_class("drop-before");
            row.remove_css_class("drop-after");
        }
    }

    /// Every project card closed, or every one open.
    fn collapse_all(self: &Rc<Self>, collapsed: bool) {
        for project in self.remote_projects() {
            crate::conn::command("window", "set_collapsed", json!({ "project": project["id"], "collapsed": collapsed }));
        }
    }

    /// The line where a dragged project would go: above project `path`'s card, or below it.
    fn show_drop_line(&self, path: &Path, before: bool) {
        self.clear_drop_line();
        let edges = self.project_edges.borrow().get(path).cloned();
        if let Some((first, last)) = edges {
            if before {
                first.add_css_class("drop-before");
            } else {
                last.add_css_class("drop-after");
            }
        }
    }

    pub fn drop_line_for_tests(&self, index: usize, before: bool) {
        let path = self.remote_projects().get(index).and_then(|p| p["id"].as_str().map(PathBuf::from));
        if let Some(path) = path {
            self.show_drop_line(&path, before);
        }
    }

    fn clear_drop_line(&self) {
        for (first, last) in self.project_edges.borrow().values() {
            first.remove_css_class("drop-before");
            last.remove_css_class("drop-after");
        }
    }

    /// Project `from` moved just before or after project `onto`.
    fn put_project(self: &Rc<Self>, from: &Path, onto: &Path, before: bool) {
        if from == onto {
            return;
        }
        // loomd's: moved there a step at a time (move_project goes by one).
        let order: Vec<PathBuf> = self.remote_projects().iter().filter_map(|p| p["id"].as_str().map(PathBuf::from)).collect();
        let (Some(at), Some(to)) = (order.iter().position(|p| p == from), order.iter().position(|p| p == onto)) else { return };
        // Where it lands once taken out of the list.
        let target = if to > at { if before { to - 1 } else { to } } else if before { to } else { to + 1 };
        let by: i64 = if target > at { 1 } else { -1 };
        for _ in 0..target.abs_diff(at) {
            crate::conn::command("window", "move_project", json!({ "project": from, "by": by }));
        }
    }

    fn refresh_inbox(self: &Rc<Self>) {
        self.refresh_remote_inbox()
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
        mark.set_margin_bottom(4);
        middle.append(&mark);
        let lockup = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(Some("Loom"));
        title.add_css_class("welcome-title");
        lockup.append(&title);
        lockup.append(&crate::about::byline());
        middle.append(&lockup);
        let tagline = gtk::Label::new(Some(crate::about::TAGLINE));
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
        if self.closed_feed.borrow().as_ref().is_some_and(|f| !f.items().is_empty()) {
            action("Reopen the last closed agent", "Ctrl+Shift+T", Box::new(|a| a.reopen_closed()));
        }
        action("Command palette", "Ctrl+Shift+P", Box::new(|a| a.palette()));
        action("Settings", "Ctrl+,", Box::new(|a| crate::prefs::open(&a.window)));
        middle.append(&actions);

        let projects: Vec<PathBuf> = self.remote_projects().iter().filter_map(|p| p["id"].as_str().map(PathBuf::from)).collect();
        if !projects.is_empty() {
            let heading = gtk::Label::builder().label("Projects").xalign(0.0).build();
            heading.add_css_class("welcome-heading");
            middle.append(&heading);
            for project in projects.iter().take(6) {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                let name = gtk::Label::builder().label(self.project_label(project)).xalign(0.0).build();
                let place = gtk::Label::builder()
                    .label(project.to_string_lossy().replace(&*glib::home_dir().to_string_lossy(), "~"))
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
                let path = project.clone();
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

/// A sidebar section's header: ▾/▸ and its name (a button that folds it), then, for
/// Running, how many need you (shown folded too: nothing waiting is out of sight), or the
/// projects' fold-all and open-all.
struct SectionHead {
    root: gtk::Box,
    toggle: gtk::Button,
    chevron: gtk::Image,
    title: gtk::Label,
    /// Waiting, failed and done (Running only), each hidden at zero.
    counts: [gtk::Label; 3],
    /// Fold every project and open every one (Projects only).
    tools: [gtk::Button; 2],
}

impl SectionHead {
    fn new(name: &str, counted: bool) -> SectionHead {
        let chevron = gtk::Image::from_icon_name("pan-down-symbolic");
        chevron.add_css_class("section-chevron");
        let title = gtk::Label::builder().label(name).xalign(0.0).build();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.set_halign(gtk::Align::Fill);
        line.append(&chevron);
        line.append(&title);
        let counts: [gtk::Label; 3] = ["blocked", "failed", "done"].map(|class| {
            let label = gtk::Label::builder().visible(false).build();
            label.add_css_class("section-count");
            label.add_css_class(class);
            label
        });
        let toggle = gtk::Button::builder().child(&line).hexpand(true).build();
        toggle.add_css_class("flat");
        toggle.add_css_class("section-toggle");
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        root.add_css_class("section-head");
        root.append(&toggle);
        let tools = [("pan-up-symbolic", "Fold every project"), ("pan-down-symbolic", "Open every project")].map(|(icon, tip)| {
            let button = gtk::Button::from_icon_name(icon);
            button.add_css_class("flat");
            button.add_css_class("heading-button");
            button.set_tooltip_text(Some(tip));
            button.update_property(&[gtk::accessible::Property::Label(tip)]);
            button
        });
        if counted {
            let shown = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            shown.set_can_target(false);
            for label in &counts {
                shown.append(label);
            }
            line.append(&gtk::Box::builder().hexpand(true).build());
            line.append(&shown);
        } else {
            for button in &tools {
                root.append(button);
            }
        }
        SectionHead { root, toggle, chevron, title, counts, tools }
    }

    fn set_open(&self, open: bool) {
        self.chevron.set_icon_name(Some(if open { "pan-down-symbolic" } else { "pan-end-symbolic" }));
        self.toggle.update_state(&[gtk::accessible::State::Expanded(Some(open))]);
        for tool in &self.tools {
            tool.set_visible(open);
        }
    }
}

/// A project's icon with an agent's status bead on its top corner: which project at a glance,
/// and how its agent is.
pub(crate) fn icon_with_bead(icon: &gtk::Image, dot: &gtk::Box) -> gtk::Overlay {
    let overlay = gtk::Overlay::builder().child(icon).valign(gtk::Align::Center).build();
    let holder = gtk::Box::builder().halign(gtk::Align::End).valign(gtk::Align::Start).build();
    holder.add_css_class("icon-bead");
    dot.set_valign(gtk::Align::Center);
    holder.append(dot);
    overlay.add_overlay(&holder);
    overlay
}

/// A bead says its state to a screen reader and on hover, not by its look alone.
fn bead_named(dot: &gtk::Box, agent: &SideAgent) {
    let state = if agent.status == Status::Idle && agent.unseen { "done" } else { agent.status.describe() };
    dot.update_property(&[gtk::accessible::Property::Label(state)]);
    dot.set_tooltip_text(Some(state));
}

/// `widget`'s tooltip (its tooltip text, as it is when shown) above it, in a popover: GTK puts
/// tooltips below, which at the bottom of the screen means under a panel or off it.
fn tip_above(widget: &impl IsA<gtk::Widget>) {
    let widget = widget.as_ref().clone();
    let label = gtk::Label::builder().wrap(true).max_width_chars(48).xalign(0.0).build();
    let tip = gtk::Popover::builder().child(&label).position(gtk::PositionType::Top).autohide(false).has_arrow(false).can_target(false).build();
    tip.add_css_class("tip");
    tip.set_parent(&widget);
    // GTK's own stays off (setting the text turns it on again); the text is still there to read
    widget.set_has_tooltip(false);
    widget.connect_has_tooltip_notify(|w| {
        if w.has_tooltip() {
            w.set_has_tooltip(false);
        }
    });
    let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let motion = gtk::EventControllerMotion::new();
    let (shown, waiting) = (tip.clone(), timer.clone());
    motion.connect_enter(move |m, _, _| {
        let Some(widget) = m.widget() else { return };
        let (tip, label, slot) = (shown.clone(), label.clone(), waiting.clone());
        let source = glib::timeout_add_local_once(Duration::from_millis(500), move || {
            slot.borrow_mut().take();
            if let Some(text) = widget.tooltip_text().filter(|t| !t.is_empty()) {
                label.set_text(&text);
                tip.popup();
            }
        });
        if let Some(old) = waiting.borrow_mut().replace(source) {
            old.remove();
        }
    });
    let (hidden, waiting) = (tip.clone(), timer.clone());
    motion.connect_leave(move |_| {
        if let Some(source) = waiting.borrow_mut().take() {
            source.remove();
        }
        hidden.popdown();
    });
    widget.add_controller(motion);
    let click = gtk::GestureClick::new();
    // before the button's own gesture claims the press
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed(move |_, _, _, _| {
        if let Some(source) = timer.borrow_mut().take() {
            source.remove();
        }
        tip.popdown();
    });
    widget.add_controller(click);
}

/// How wide an open project's name strip is, at its card's left (a click folds the card).
const CARD_STRIP: f32 = 32.0;

/// The open cards' name strips over the list: each widget and its card's first and last rows.
type CardSpots = Rc<RefCell<Vec<(gtk::Widget, gtk::ListBoxRow, gtk::ListBoxRow)>>>;

/// An open project's name as its card's edge shows it (`width`: the most it may take).
fn name_layout(widget: &impl IsA<gtk::Widget>, text: &str, width: i32) -> pango::Layout {
    let layout = widget.create_pango_layout(Some(text));
    let mut font = pango::FontDescription::new();
    font.set_weight(pango::Weight::Medium);
    font.set_size(10 * pango::SCALE);
    layout.set_font_description(Some(&font));
    layout.set_width(width * pango::SCALE);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout
}

/// A running agent's second line in the sidebar: "waiting: <its question>", "failed: <why>",
/// "done: <its last words>", or what it works on, then how long ago it was used.
fn said_line(agent: &SideAgent, overview: &[Value]) -> String {
    let card = overview.iter().find(|o| o["id"].as_str() == Some(&*agent.id.to_string()));
    // its first line of text, without markdown's marks
    let first_line = |v: &Value| {
        let line = v.as_str().and_then(|t| t.lines().map(|l| l.trim().trim_start_matches(['#', '*', '>', '-', ' '])).find(|l| !l.is_empty()));
        line.unwrap_or_default().replace(['`', '*'], "")
    };
    let reply = card.map(|c| first_line(&c["reply"])).unwrap_or_default();
    let what = match agent.status {
        Status::Blocked => format!("waiting: {}", card.map(|c| first_line(&c["waiting"][0])).filter(|q| !q.is_empty()).unwrap_or_else(|| "your answer".to_owned())),
        Status::Failed if !agent.failed.is_empty() => format!("failed: {}", agent.failed),
        Status::Idle if agent.unseen && !reply.is_empty() => format!("done: {reply}"),
        Status::Working | Status::Starting => [agent.status.describe().to_owned(), agent.place.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · "),
        _ if !reply.is_empty() => reply,
        other => other.describe().to_owned(),
    };
    match agent.used {
        0 => what,
        used => format!("{what} · {}", crate::overview::ago(crate::state::now().saturating_sub(used))),
    }
}

fn agent_row(agent: &SideAgent, alike: bool, scribe_asks: bool) -> gtk::ListBoxRow {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("dot");
    let status = agent.status;
    dot.add_css_class(if status == Status::Idle && agent.unseen { "done" } else { status.css() });
    bead_named(&dot, agent);
    dot.set_valign(gtk::Align::Center);
    let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
    names.set_hexpand(true);
    let title = gtk::Label::builder().label(&agent.title).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
    title.add_css_class("agent-title");
    if agent.status == Status::Suspended {
        title.add_css_class("resting");
    }
    // Another of the same name in this project: when each was last used tells them apart.
    if alike && agent.used > 0 {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        title.set_hexpand(false);
        line.append(&title);
        let when = gtk::Label::new(Some(&format!("· {}", crate::overview::ago(crate::state::now().saturating_sub(agent.used)))));
        when.add_css_class("dup");
        when.add_css_class("small");
        line.append(&when);
        names.append(&line);
    } else {
        names.append(&title);
    }
    let state = match status {
        Status::Idle if agent.unseen => "done".to_owned(),
        Status::Blocked => "needs you".to_owned(),
        // The resting states say nothing (most agents are in one): the dot is enough.
        Status::Idle | Status::Suspended => String::new(),
        other => other.describe().to_owned(),
    };
    // Where it works: the repo it last touched (name when it isn't the project itself), its
    // branch and changes; else the subfolder it runs in.
    let place = agent.place.clone();
    // Files it wrote or edited in this conversation.
    let edited = match agent.edited {
        0 => String::new(),
        n => format!("{n} edited"),
    };
    let sub = [state, place, edited].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    if !sub.is_empty() {
        let sub = gtk::Label::builder().label(sub).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        sub.add_css_class("dim");
        sub.add_css_class("small");
        sub.add_css_class("agent-sub");
        names.append(&sub);
    }
    line.append(&dot);
    line.append(&names);
    if scribe_asks {
        let mark = gtk::Box::builder().valign(gtk::Align::Center).tooltip_text("Scribe has a question about this conversation (the Notes tab)").build();
        mark.add_css_class("scribe-mark");
        line.append(&mark);
    }
    let row = gtk::ListBoxRow::builder().child(&line).build();
    row.add_css_class("agent-row");
    // the pill's tint: what it is doing (as its bead)
    row.add_css_class(&format!("state-{}", if status == Status::Idle && agent.unseen { "done" } else { status.css() }));
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
    loom_core::repos::project_of(cwd)
}

/// "src/app.rs:42" → ("src/app.rs", Some(42)); also "#L42".
fn split_line(target: &str) -> (&str, Option<u32>) {
    if let Some((path, line)) = target.rsplit_once("#L")
        && let Ok(line) = line.parse()
    {
        return (path, Some(line));
    }
    let mut parts = target.splitn(3, ':');
    let path = parts.next().unwrap_or(target);
    (path, parts.next().and_then(|l| l.parse().ok()))
}

/// Chat text a step bigger or smaller (Ctrl+= / Ctrl+-).
fn zoom(step: f64) {
    crate::prefs::update(|s| s.text_scale = ((s.text_scale + step) * 10.0).round().clamp(7.0, 16.0) / 10.0);
}

impl App {
}

#[cfg(test)]
mod loomd_tests {
    use super::*;

    #[test]
    fn an_older_or_newer_loomd_is_told_apart() {
        let all: Vec<&str> = loom_core::api::spec::COMMANDS.iter().map(|c| c.name).collect();
        let hello = |version: &str, commands: &[&str]| json!({ "server": { "version": version }, "capabilities": { "commands": commands } });
        assert_eq!(loomd_differs(&hello("0.5.0", &all), "0.5.0"), None);
        assert_eq!(loomd_differs(&hello("0.4.0", &all), "0.5.0"), Some(0), "another version");
        assert_eq!(loomd_differs(&hello("0.5.0", &all[1..]), "0.5.0"), Some(1), "the same version, a command short");
    }
}

/// The window's own state file (its size, the agents it showed): the window's, not loomd's.
fn window_state_path(app_id: &str) -> PathBuf {
    loom_core::state::dir().join(format!("window-{}.json", app_id.rsplit('.').next().unwrap_or("loom")))
}

fn window_state(app_id: &str) -> Value {
    std::fs::read_to_string(window_state_path(app_id)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}
