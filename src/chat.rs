//! One agent's conversation: a header, the messages and tool calls, the plan, and the input.
//!
//! Widgets only; what happens on submit, stop or a permission answer is the agent's business
//! (see agent.rs), passed in as callbacks.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::{gdk, gdk_pixbuf, gio, glib, pango, prelude::*};
use serde_json::Value;

use crate::markdown::{self, Segment};

type Callback<T> = RefCell<Option<Rc<dyn Fn(T)>>>;

thread_local! {
    /// Opens a file link ("loom-file:src/app.rs:42", or a relative or file:// link) clicked
    /// in a chat (the widget it was in, to know whose folder): true if it did.
    pub static OPEN_LINK: RefCell<Option<Rc<dyn Fn(&str, &gtk::Widget) -> bool>>> = const { RefCell::new(None) };
}

/// A link clicked in chat text: files go to the app, the web to the browser (GTK's default).
fn link_clicked(label: &gtk::Label, uri: &str) -> glib::Propagation {
    let web = ["http://", "https://", "mailto:"].iter().any(|p| uri.starts_with(p));
    if web {
        return glib::Propagation::Proceed;
    }
    let handler = OPEN_LINK.with(|h| h.borrow().clone());
    match handler {
        Some(open) if open(uri, label.upcast_ref()) => glib::Propagation::Stop,
        _ => glib::Propagation::Proceed,
    }
}

pub struct Chat {
    pub root: gtk::Box,
    title: gtk::Label,
    place: gtk::Label,
    dot: gtk::Box,
    context: gtk::LevelBar,
    starting: gtk::Label,
    state: gtk::Label,
    mode: gtk::Button,
    usage: gtk::Label,
    /// Shown once the context is fuller than the user's setting: sends /compact.
    compact: gtk::Button,
    /// The conversation's cost and the account's limit, from the agent.
    spend: gtk::Label,
    /// Claude Code is not the version loom was built for (the note, None: it is).
    version_note: Rc<RefCell<Option<String>>>,
    /// What the usage popover shows of this conversation: (used, size) tokens, its cost, the
    /// account limit the agent last reported.
    spent: RefCell<(Option<(u64, u64)>, Option<String>, Option<String>)>,
    /// What fills the context (name, tokens), biggest first, and where it compacts by itself.
    context_parts: RefCell<(Vec<(String, u64)>, Option<u64>)>,
    usage_body: gtk::Box,
    /// Sent messages, newest last (Up/Down in the input), where Up has got to, and what
    /// was typed before.
    history: RefCell<Vec<String>>,
    history_at: Cell<Option<usize>>,
    history_draft: RefCell<String>,
    /// Pickers for the session's settings (model, effort…).
    options: gtk::Box,
    show_thinking: Cell<bool>,
    placeholder: gtk::Label,
    /// The input's Markdown marks are due (they follow typing, after a pause).
    markdown_due: Cell<bool>,
    /// What Claude Code predicts you will send next (shown in the empty input; → or Tab
    /// fills it in).
    suggestion: RefCell<Option<String>>,
    send: gtk::Button,
    /// Speech to text: a click records, another transcribes into the input.
    mic: gtk::Button,
    recording: RefCell<Option<crate::stt::Recording>>,
    working: gtk::Label,
    /// The input box, and its "Shell mode" badge (text starting with !).
    composer: gtk::Box,
    shell_badge: gtk::Label,
    working_since: Cell<Option<std::time::Instant>>,
    latest: gtk::Button,
    /// The agent's folder, and its files once listed (for @ mentions).
    cwd: RefCell<std::path::PathBuf>,
    project_files: RefCell<Option<Vec<String>>>,
    /// What the open completion list offers: (text to insert, shown name, detail).
    offered: RefCell<Vec<(String, String, String)>>,
    thoughts: RefCell<Vec<glib::WeakRef<gtk::Expander>>>,
    scroller: gtk::ScrolledWindow,
    list: gtk::Box,
    plan: gtk::Box,
    plan_revealer: gtk::Revealer,
    pub input: gtk::TextView,
    last: RefCell<Last>,
    tools: RefCell<HashMap<String, Rc<ToolCard>>>,
    /// Subagents at work this turn: their task's tool call, what they last did, when.
    crew: RefCell<Vec<Crew>>,
    /// What the agent's workflows are at (see set_workflow_note).
    workflow_note: RefCell<Option<String>>,
    crew_list: gtk::ListBox,
    crew_revealer: gtk::Revealer,
    /// A row of the strip clicked: its key (a subagent session, or a task's tool call).
    pub on_crew_open: Callback<String>,
    /// Stop pressed on a running subagent's row.
    pub on_crew_stop: Callback<String>,
    /// A queued message taken back (its place in the queue).
    pub on_unqueue: Callback<usize>,
    /// Ctrl+Enter: the queued messages go into the running turn now.
    pub on_send_now: Callback<()>,
    /// Messages waiting for the turn to end, over the input.
    queued_box: gtk::Box,
    /// The main conversation, or one subagent's ("aside").
    views: gtk::Stack,
    aside: gtk::Box,
    aside_title: gtk::Label,
    pub header: gtk::Box,
    input_column: gtk::Box,
    /// Where the next tool call goes, until something else is said.
    group: RefCell<Option<Rc<ToolGroup>>>,
    stick: Rc<Cell<bool>>,
    /// A follow is due once the layout settles.
    follow_pending: Cell<bool>,
    /// Where the main conversation was read when a subagent's was shown over it: (scroll
    /// position, following the end).
    main_place: Cell<Option<(f64, bool)>>,
    commands: RefCell<Vec<(String, String)>>,
    completion: gtk::Popover,
    completion_list: gtk::ListBox,
    /// The conversation's messages in order (export, rewind).
    messages: Rc<RefCell<Vec<Rc<Message>>>>,
    /// "Rewind to here" on a message you sent: the id of the agent's message before it (None:
    /// from the start) and the text, to fork the conversation there.
    pub on_rewind: Callback<(Option<String>, bool, String)>,
    /// What was typed, and the images attached (PNG).
    pub on_submit: Callback<(String, Vec<glib::Bytes>)>,
    /// Find in the conversation (Ctrl+F): the bar, what is typed, how many, which one.
    find_bar: gtk::Revealer,
    find_entry: gtk::SearchEntry,
    find_count: gtk::Label,
    find_at: Cell<Option<usize>>,
    /// Images pasted or dropped, waiting to go with the next message.
    attachments: RefCell<Vec<glib::Bytes>>,
    attach_box: gtk::Box,
    pub on_cancel: Callback<()>,
    pub on_cycle_mode: Callback<()>,
    /// A session setting picked: (its id, the value).
    pub on_option: Callback<(String, String)>,
    /// A digit typed into an empty input: answers the oldest permission question, if any
    /// (true when it did).
    pub on_answer: RefCell<Option<Rc<dyn Fn(usize) -> bool>>>,
}

enum Last {
    None,
    Agent(Rc<Message>),
    Thought(Rc<Message>),
    User(Rc<Message>),
}

/// Tool calls in a row, between two messages. Once three or more have all finished well (and
/// none is an edit worth seeing), they fold into one "✓ 5 tool calls" line, and stay folded:
/// later calls join it (an edit opens it again). One the user has opened a card of never folds.
struct ToolGroup {
    root: gtk::Box,
    summary: gtk::Button,
    summary_label: gtk::Label,
    cards_box: gtk::Box,
    cards: RefCell<Vec<Rc<ToolCard>>>,
    folded: Cell<bool>,
    /// The fold opened by a click.
    open: Cell<bool>,
    /// A card in it was opened by hand: it stays as it is.
    touched: Cell<bool>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Agent,
    Thought,
    User,
}

struct Message {
    kind: Kind,
    text: RefCell<String>,
    body: gtk::Box,
    scheduled: Cell<bool>,
    /// The agent's id for it (to fork the conversation from here).
    id: RefCell<Option<String>>,
}

struct ToolCard {
    root: gtk::Box,
    /// A task's own steps (a subagent at work), under it.
    children: gtk::Box,
    glyph: gtk::Label,
    spinner: gtk::Spinner,
    kind: gtk::Label,
    title: gtk::Label,
    actions: gtk::Box,
    detail: gtk::Box,
    revealer: gtk::Revealer,
    content: RefCell<Vec<Value>>,
    /// A command's output (its last 200 KB, as the terminal got it) and how it ended.
    output: RefCell<String>,
    exit: Cell<Option<Option<i64>>>,
    output_button: RefCell<Option<gtk::Button>>,
    rendering: Cell<bool>,
    has_open: Cell<bool>,
    /// Opened by itself once (a small diff shows without a click).
    auto_opened: Cell<bool>,
    status: RefCell<String>,
    has_diff: Cell<bool>,
    group: RefCell<std::rc::Weak<ToolGroup>>,
}

#[derive(Clone)]
pub struct PermissionCard {
    buttons: gtk::Widget,
    answer: gtk::Label,
}

/// A question form (Claude Code's AskUserQuestion, or an MCP server asking for input).
pub struct FormCard {
    pub card: PermissionCard,
    fields: Vec<Field>,
    /// The fields that must be answered.
    required: Vec<String>,
    /// Why it can't be sent yet.
    problem: gtk::Label,
}

enum Field {
    /// One of the options (`multi`: any of them); each button with the value it stands for
    /// and its name.
    Choice { key: String, title: String, multi: bool, buttons: Vec<(gtk::CheckButton, Value, String)> },
    Text { key: String, title: String, entry: gtk::Entry, number: Option<bool> },
    Flag { key: String, title: String, check: gtk::CheckButton },
}

impl Chat {
    pub fn new() -> Rc<Chat> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("chat");

        // Header: title and place on the left, state, mode and context use on the right.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        header.add_css_class("header");
        let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
        names.set_hexpand(true);
        let title = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        title.add_css_class("title");
        let place = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).build();
        place.add_css_class("place");
        let title_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("dot");
        dot.set_valign(gtk::Align::Center);
        title_line.append(&dot);
        title_line.append(&title);
        names.append(&title_line);
        names.append(&place);
        // The state as a pill right after the title, where the eye already is.
        let state = gtk::Label::builder().valign(gtk::Align::Center).visible(false).build();
        state.add_css_class("state-pill");
        title_line.append(&state);
        let mode = gtk::Button::with_label("Mode");
        mode.add_css_class("mode");
        mode.add_css_class("flat");
        mode.set_tooltip_text(Some("Permission mode (Shift+Tab in the input)"));
        mode.set_valign(gtk::Align::Center);
        let usage = gtk::Label::new(None);
        usage.add_css_class("dim");
        usage.add_css_class("small");
        let context = gtk::LevelBar::builder().min_value(0.0).max_value(1.0).valign(gtk::Align::Center).visible(false).build();
        context.add_css_class("context-bar");
        let options = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        options.set_valign(gtk::Align::Center);
        context.set_mode(gtk::LevelBarMode::Continuous);
        // LevelBar's own offsets colour it; loom's CSS does that instead.
        for name in ["low", "high", "full"] {
            context.remove_offset_value(Some(name));
        }
        header.append(&names);
        let spend = gtk::Label::new(None);
        spend.add_css_class("dim");
        spend.add_css_class("small");
        spend.set_visible(false);
        // Context, cost and limit in one button: clicked, the details and the account's limits.
        let meter = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        meter.append(&context);
        meter.append(&usage);
        meter.append(&spend);
        // Claude Code is not the version loom was built for: a mark here, the note in the popover.
        let version_warn = gtk::Label::new(Some("⚠"));
        version_warn.add_css_class("version-warn");
        version_warn.set_visible(false);
        meter.append(&version_warn);
        let version_note: Rc<RefCell<Option<String>>> = Rc::default();
        {
            let (warn, note) = (version_warn.downgrade(), version_note.clone());
            glib::spawn_future_local(async move {
                let found = gio::spawn_blocking(crate::provider::version_note).await.ok().flatten();
                let Some(warn) = warn.upgrade() else { return };
                warn.set_tooltip_text(found.as_deref());
                warn.set_visible(found.is_some());
                *note.borrow_mut() = found;
            });
        }
        let usage_body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        usage_body.add_css_class("usage-pop");
        let usage_pop = gtk::Popover::builder().child(&usage_body).build();
        let usage_button = gtk::MenuButton::builder().child(&meter).popover(&usage_pop).valign(gtk::Align::Center).build();
        usage_button.add_css_class("flat");
        usage_button.add_css_class("usage-button");
        usage_button.set_tooltip_text(Some("Context and usage limits"));
        let compact = gtk::Button::with_label("Compact");
        compact.add_css_class("compact-alert");
        compact.set_valign(gtk::Align::Center);
        compact.set_visible(false);
        header.append(&compact);
        header.append(&usage_button);
        root.append(&header);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 16);
        list.add_css_class("messages");
        let starting = gtk::Label::builder().visible(false).halign(gtk::Align::Center).build();
        starting.add_css_class("starting-note");
        list.append(&starting);
        let clamp = gtk::Box::new(gtk::Orientation::Vertical, 0);
        clamp.append(&list);
        // About 75 characters a line: long lines are hard to follow back.
        clamp.set_layout_manager(Some(crate::column::ColumnLayout::new(800)));
        let scroller = gtk::ScrolledWindow::builder()
            .child(&clamp)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        // "Jump to latest", while the reader is scrolled up.
        let find_entry = gtk::SearchEntry::builder().placeholder_text("Find in the conversation").width_chars(28).build();
        let find_count = gtk::Label::new(None);
        find_count.add_css_class("dim");
        find_count.add_css_class("small");
        let find_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        find_line.add_css_class("find-bar");
        find_line.append(&find_entry);
        find_line.append(&find_count);
        let find_bar = gtk::Revealer::builder().child(&find_line).halign(gtk::Align::End).valign(gtk::Align::Start).margin_top(8).margin_end(16).build();
        let latest = gtk::Button::with_label("↓  Latest");
        latest.add_css_class("latest");
        latest.set_halign(gtk::Align::Center);
        latest.set_valign(gtk::Align::End);
        latest.set_margin_bottom(14);
        latest.set_visible(false);
        let scroll_area = gtk::Overlay::builder().child(&scroller).vexpand(true).build();
        scroll_area.add_overlay(&find_bar);
        scroll_area.add_overlay(&latest);
        // The main conversation, or (aside) a subagent's, read-only, with the way back.
        let views = gtk::Stack::builder().vexpand(true).build();
        views.add_named(&scroll_area, Some("main"));
        let aside = gtk::Box::new(gtk::Orientation::Vertical, 0);
        aside.add_css_class("aside");
        let aside_bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        aside_bar.add_css_class("aside-bar");
        let back = gtk::Button::with_label("← Main conversation");
        back.add_css_class("flat");
        back.add_css_class("small");
        back.set_tooltip_text(Some("Back to the main conversation (Esc)"));
        let aside_title = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
        aside_title.add_css_class("aside-title");
        aside_bar.append(&back);
        aside_bar.append(&aside_title);
        aside.append(&aside_bar);
        views.add_named(&aside, Some("aside"));
        let v = views.clone();
        back.connect_clicked(move |_| v.set_visible_child_name("main"));
        root.append(&views);

        let plan = gtk::Box::new(gtk::Orientation::Vertical, 2);
        plan.add_css_class("plan");
        let plan_revealer = gtk::Revealer::builder().child(&plan).reveal_child(false).build();
        root.append(&plan_revealer);
        // Subagents working (in the background too): what each last did; a click goes to it.
        let crew_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        crew_list.add_css_class("crew");
        let crew_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        crew_column.append(&crew_list);
        crew_column.set_layout_manager(Some(crate::column::ColumnLayout::new(800)));
        let crew_revealer = gtk::Revealer::builder().child(&crew_column).reveal_child(false).build();
        root.append(&crew_revealer);

        let input = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(10)
            .right_margin(10)
            .build();
        input.add_css_class("input");
        let input_scroller = gtk::ScrolledWindow::builder()
            .child(&input)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(240)
            .build();
        // The hint shown while the input is empty (a text view has none of its own).
        let placeholder = gtk::Label::builder()
            .label("Ask Claude…   @ file · / commands · ! run · Ctrl+V pastes images")
            .xalign(0.0)
            .valign(gtk::Align::Start)
            .margin_start(14)
            .margin_top(10)
            .margin_end(14)
            .ellipsize(pango::EllipsizeMode::End)
            .can_target(false)
            .build();
        placeholder.add_css_class("placeholder");
        let field = gtk::Overlay::builder().child(&input_scroller).build();
        field.add_overlay(&placeholder);

        // Under the text: attach, the session's pickers and mode, what is happening, send.
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.add_css_class("composer-bar");
        let attach = gtk::Button::with_label("@");
        attach.add_css_class("flat");
        attach.add_css_class("round");
        attach.set_tooltip_text(Some("Mention a file (or drop files here)"));
        attach.update_property(&[gtk::accessible::Property::Label("Mention a file")]);
        let working = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).width_chars(4).build();
        working.add_css_class("working");
        let mic = gtk::Button::from_icon_name("audio-input-microphone-symbolic");
        mic.add_css_class("flat");
        mic.add_css_class("mic");
        mic.set_tooltip_text(Some("Speak: click to record, click again to write it here"));
        mic.update_property(&[gtk::accessible::Property::Label("Speak")]);
        mic.set_visible(crate::stt::configured());
        let send = gtk::Button::with_label("↑");
        send.add_css_class("send");
        send.set_tooltip_text(Some("Send (Enter)"));
        send.update_property(&[gtk::accessible::Property::Label("Send")]);
        let pickers = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        // The mode first: it decides what Claude may do without asking.
        pickers.append(&mode);
        pickers.append(&options);
        let pickers_scroller = gtk::ScrolledWindow::builder()
            .child(&pickers)
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_width(true)
            .min_content_width(60)
            .build();
        toolbar.append(&attach);
        toolbar.append(&pickers_scroller);
        let shell_badge = gtk::Label::builder().label("Shell mode").visible(false).valign(gtk::Align::Center).build();
        shell_badge.add_css_class("shell-badge");
        shell_badge.set_tooltip_text(Some("Runs here, as you typed it. What it prints goes to Claude when it ends."));
        toolbar.append(&shell_badge);
        toolbar.append(&working);
        toolbar.append(&mic);
        toolbar.append(&send);

        let attach_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        attach_box.add_css_class("attachments");
        attach_box.set_visible(false);
        let queued_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        queued_box.add_css_class("queued");
        queued_box.set_visible(false);
        let composer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        composer.add_css_class("composer");
        composer.append(&queued_box);
        composer.append(&attach_box);
        composer.append(&field);
        composer.append(&toolbar);
        let input_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        input_box.add_css_class("input-box");
        input_box.append(&composer);
        let input_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        input_column.append(&input_box);
        // The conversation's width (with its padding): the box's edges line up with the text.
        input_column.set_layout_manager(Some(crate::column::ColumnLayout::new(800)));
        root.append(&input_column);

        let completion_list = gtk::ListBox::new();
        completion_list.add_css_class("completion");
        let completion = gtk::Popover::builder()
            .child(&completion_list)
            .autohide(false)
            .has_arrow(false)
            .position(gtk::PositionType::Top)
            .can_focus(false)
            .build();
        completion.set_parent(&input_box);

        let chat = Rc::new(Chat {
            root,
            title,
            place,
            dot,
            context,
            starting,
            state,
            mode,
            usage,
            compact: compact.clone(),
            spend,
            version_note,
            spent: RefCell::default(),
            context_parts: RefCell::default(),
            usage_body: usage_body.clone(),
            history: RefCell::default(),
            history_at: Cell::new(None),
            history_draft: RefCell::default(),
            options,
            show_thinking: Cell::new(true),
            placeholder,
            suggestion: RefCell::default(),
            markdown_due: Cell::new(false),
            send: send.clone(),
            mic: mic.clone(),
            recording: RefCell::default(),
            working,
            composer: composer.clone(),
            shell_badge,
            working_since: Cell::new(None),
            latest: latest.clone(),
            cwd: RefCell::default(),
            project_files: RefCell::default(),
            offered: RefCell::default(),
            thoughts: RefCell::default(),
            scroller,
            list,
            plan,
            plan_revealer,
            input,
            last: RefCell::new(Last::None),
            group: RefCell::default(),
            tools: RefCell::default(),
            crew: RefCell::default(),
            workflow_note: RefCell::default(),
            crew_list,
            crew_revealer,
            on_crew_open: RefCell::default(),
            on_crew_stop: RefCell::default(),
            on_unqueue: RefCell::default(),
            on_send_now: RefCell::default(),
            queued_box,
            views: views.clone(),
            aside: aside.clone(),
            aside_title,
            header: header.clone(),
            input_column: input_column.clone(),
            stick: Rc::new(Cell::new(true)),
            follow_pending: Cell::new(false),
            main_place: Cell::new(None),
            commands: RefCell::default(),
            completion,
            completion_list,
            on_submit: RefCell::default(),
            attachments: RefCell::default(),
            messages: Rc::default(),
            on_rewind: RefCell::default(),
            find_bar,
            find_entry,
            find_count,
            find_at: Cell::new(None),
            attach_box,
            on_cancel: RefCell::default(),
            on_cycle_mode: RefCell::default(),
            on_option: RefCell::default(),
            on_answer: RefCell::default(),
        });
        chat.wire();
        // /compact sent on its own: what is being typed, and images attached, stay for the next
        // message. Only between turns (the button is not shown during one).
        let weak = Rc::downgrade(&chat);
        compact.connect_clicked(move |_| {
            let Some(chat) = weak.upgrade() else { return };
            if chat.working_since.get().is_some() {
                return;
            }
            chat.compact.set_visible(false);
            chat.call(&chat.on_submit, ("/compact".to_owned(), Vec::new()));
        });
        let weak = Rc::downgrade(&chat);
        // Until the chat goes (a viewer's goes once it is closed).
        crate::settings::on_change_while(move |_| {
            let Some(chat) = weak.upgrade() else { return false };
            chat.show_compact();
            true
        });
        let weak = Rc::downgrade(&chat);
        send.connect_clicked(move |_| {
            if let Some(chat) = weak.upgrade() {
                if chat.working_since.get().is_some() && chat.input.buffer().char_count() == 0 {
                    chat.call(&chat.on_cancel, ());
                } else {
                    chat.submit();
                }
            }
        });
        let weak = Rc::downgrade(&chat);
        attach.connect_clicked(move |_| {
            if let Some(chat) = weak.upgrade() {
                chat.input.buffer().insert_at_cursor("@");
                chat.input.grab_focus();
            }
        });
        let weak = Rc::downgrade(&chat);
        chat.find_entry.connect_search_changed(move |_| {
            if let Some(chat) = weak.upgrade() {
                chat.find_at.set(None);
                chat.find_next(false);
            }
        });
        let weak = Rc::downgrade(&chat);
        chat.find_entry.connect_activate(move |_| {
            if let Some(chat) = weak.upgrade() {
                chat.find_next(false);
            }
        });
        let weak = Rc::downgrade(&chat);
        chat.find_entry.connect_stop_search(move |_| {
            if let Some(chat) = weak.upgrade() {
                chat.close_find();
            }
        });
        let find_keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&chat);
        find_keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(chat) = weak.upgrade() else { return glib::Propagation::Proceed };
            let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
            match key {
                gdk::Key::Return | gdk::Key::KP_Enter => chat.find_next(shift),
                gdk::Key::Up => chat.find_next(true),
                gdk::Key::Down => chat.find_next(false),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        chat.find_entry.add_controller(find_keys);
        let weak = Rc::downgrade(&chat);
        latest.connect_clicked(move |_| {
            if let Some(chat) = weak.upgrade() {
                chat.scroll_to_latest();
                chat.input.grab_focus();
            }
        });
        chat
    }

    /// The messages, all of them (for pictures in tests).
    pub fn messages(&self) -> gtk::Widget {
        self.list.clone().upcast()
    }

    /// Copies the text selected in the chat (a message, a code block, a tool's output) to the
    /// clipboard. False if nothing is selected.
    pub fn copy_selection(&self) -> bool {
        fn find(widget: &gtk::Widget) -> Option<String> {
            if let Some(label) = widget.downcast_ref::<gtk::Label>()
                && label.is_selectable()
                && let Some((start, end)) = label.selection_bounds()
                && start != end
            {
                let (start, end) = (start.min(end) as usize, start.max(end) as usize);
                return Some(label.text().chars().skip(start).take(end - start).collect());
            }
            let mut child = widget.first_child();
            while let Some(widget) = child {
                if let Some(text) = find(&widget) {
                    return Some(text);
                }
                child = widget.next_sibling();
            }
            None
        }
        let Some(text) = find(self.list.upcast_ref()) else { return false };
        // Without the direction marks loom adds to mixed-direction lines.
        self.list.clipboard().set_text(&text.replace(['\u{200E}', '\u{200F}'], ""));
        true
    }

    /// Ctrl+F: the find bar over the conversation.
    pub fn find(self: &Rc<Self>) {
        self.find_bar.set_reveal_child(true);
        self.find_entry.grab_focus();
        self.find_entry.select_region(0, -1);
    }

    fn close_find(&self) {
        self.find_bar.set_reveal_child(false);
        self.find_at.set(None);
        self.find_count.set_label("");
        self.input.grab_focus();
    }

    /// The next (or previous) place the find text is in: selected, and scrolled to.
    fn find_next(&self, back: bool) {
        let needle = self.find_entry.text().to_lowercase();
        if needle.is_empty() {
            self.find_count.set_label("");
            return;
        }
        // Every (label, char offset) the text is at, top to bottom.
        let mut hits: Vec<(gtk::Label, usize)> = Vec::new();
        fn walk(widget: &gtk::Widget, needle: &str, hits: &mut Vec<(gtk::Label, usize)>) {
            if let Some(label) = widget.downcast_ref::<gtk::Label>()
                && label.is_selectable()
                && label.is_drawable()
            {
                let text = label.text().to_lowercase();
                let mut from = 0;
                while let Some(at) = text[from..].find(needle) {
                    let byte = from + at;
                    hits.push((label.clone(), text[..byte].chars().count()));
                    from = byte + needle.len().max(1);
                }
            }
            let mut child = widget.first_child();
            while let Some(widget) = child {
                walk(&widget, needle, hits);
                child = widget.next_sibling();
            }
        }
        walk(self.list.upcast_ref(), &needle, &mut hits);
        if hits.is_empty() {
            self.find_count.set_label("no match");
            self.find_at.set(None);
            return;
        }
        let at = match (self.find_at.get(), back) {
            (None, false) => 0,
            (None, true) => hits.len() - 1,
            (Some(i), false) => (i + 1) % hits.len(),
            (Some(i), true) => (i + hits.len() - 1) % hits.len(),
        };
        self.find_at.set(Some(at));
        self.find_count.set_label(&format!("{} of {}", at + 1, hits.len()));
        let (label, offset) = &hits[at];
        let length = needle.chars().count();
        label.select_region(*offset as i32, (*offset + length) as i32);
        if let Some(point) = label.compute_point(&self.list, &gtk::graphene::Point::new(0.0, 0.0)) {
            let adj = self.scroller.vadjustment();
            self.stick.set(false);
            let y = f64::from(point.y()) - adj.page_size() / 3.0;
            adj.set_value(y.clamp(0.0, (adj.upper() - adj.page_size()).max(0.0)));
        }
    }

    /// The scroll state, for tests: value, page, upper, following, the list's height and
    /// the height its column measures for its width.
    pub fn scroll_info(&self) -> String {
        let adj = self.scroller.vadjustment();
        let clamp = self.list.parent();
        let measured = clamp.as_ref().map(|c| c.measure(gtk::Orientation::Vertical, c.width()).1).unwrap_or(-1);
        format!(
            "value={:.0} page={:.0} upper={:.0} stick={} list_h={} clamp_w={} clamp_h={} measured_h={}",
            adj.value(),
            adj.page_size(),
            adj.upper(),
            self.stick.get(),
            self.list.height(),
            clamp.as_ref().map_or(-1, |c| c.width()),
            clamp.as_ref().map_or(-1, |c| c.height()),
            measured
        )
    }

    /// Moves the view by `dy` as a reader would (for tests).
    pub fn scroll_by_for_tests(&self, dy: f64) {
        let adj = self.scroller.vadjustment();
        adj.set_value((adj.value() + dy).clamp(0.0, (adj.upper() - adj.page_size()).max(0.0)));
    }

    /// To the end if following (now, and again once the layout has settled).
    ///
    /// Never at once: the conversation's new height is told in the middle of laying the
    /// window out, and a move made then is not laid out (GTK drops a re-layout asked for
    /// during one). The view would stay drawn where it was while the scroller believes it is
    /// at the end: half a screen short, scrolling down doing nothing, until any scroll lays it
    /// out again. So the move waits for the start of the next frame, before its layout.
    fn follow(self: &Rc<Self>) {
        // Not shown, no frames: when it is shown again, its map follows.
        if !self.stick.get() || !self.scroller.is_mapped() || self.follow_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        self.scroller.add_tick_callback(move |_, _| {
            if let Some(chat) = weak.upgrade() {
                chat.follow_pending.set(false);
                if chat.stick.get() {
                    let adj = chat.scroller.vadjustment();
                    adj.set_value(adj.upper() - adj.page_size());
                }
            }
            glib::ControlFlow::Break
        });
    }

    /// To the bottom, and following it again.
    pub fn scroll_to_latest(&self) {
        self.stick.set(true);
        let adj = self.scroller.vadjustment();
        adj.set_value(adj.upper() - adj.page_size());
        self.latest.set_visible(false);
    }

    fn wire(self: &Rc<Self>) {
        // Follow the bottom unless the reader moved away from it. Who moved it: the reader
        // (wheel, touchpad, scrollbar, keys, touch) moves it up with the content's size
        // unchanged; loom (a run of tool calls folding, a chat hidden and shown again, the
        // input growing) changes the size, which drags the position along. So only an upward
        // move at an unchanged size lets go, and reaching the end catches it again.
        let adjustment = self.scroller.vadjustment();
        let stick = self.stick.clone();
        let latest = self.latest.clone();
        let seen = Rc::new(Cell::new((0.0f64, 0.0f64, 0.0f64)));
        // The view now follows a frame later, so a reader's scroll can come in between: the
        // grown height is noted at once, or that scroll would count as a size change and not
        // let go. (A shrink is not: its clamp of the position must not count as the reader's.)
        let grown = seen.clone();
        adjustment.connect_upper_notify(move |adj| {
            let (value, upper, page) = grown.get();
            if adj.upper() > upper {
                grown.set((value, adj.upper(), page));
            }
        });
        adjustment.connect_value_changed(move |adj| {
            let (value, upper, page) = (adj.value(), adj.upper(), adj.page_size());
            let (was, was_upper, was_page) = seen.replace((value, upper, page));
            let at_bottom = value >= upper - page - 40.0;
            let same_size = (upper - was_upper).abs() < 0.5 && (page - was_page).abs() < 0.5;
            if at_bottom {
                stick.set(true);
            } else if same_size && value < was - 0.5 {
                stick.set(false);
            }
            latest.set_visible(!stick.get() && !at_bottom && upper > page * 1.2);
        });
        // Following: now, and once more after the layout settles (a size can change twice in
        // one frame, the second time after this ran).
        let weak = Rc::downgrade(self);
        let follow = move |_: &gtk::Adjustment| {
            if let Some(chat) = weak.upgrade() {
                chat.follow();
            }
        };
        let also = follow.clone();
        adjustment.connect_upper_notify(move |adj| follow(adj));
        adjustment.connect_page_size_notify(move |adj| also(adj));
        // Shown again (another agent was in view): where it was, at the end if following.
        let weak = Rc::downgrade(self);
        self.root.connect_map(move |_| {
            if let Some(chat) = weak.upgrade() {
                chat.follow();
            }
        });
        // A tick asked for just before it was hidden never comes: the next follow asks again.
        let weak = Rc::downgrade(self);
        self.root.connect_unmap(move |_| {
            if let Some(chat) = weak.upgrade() {
                chat.follow_pending.set(false);
            }
        });

        // Filled each time the usage popover opens.
        if let Some(popover) = self.usage_body.ancestor(gtk::Popover::static_type()) {
            let weak = Rc::downgrade(self);
            popover.connect_show(move |_| {
                if let Some(chat) = weak.upgrade() {
                    chat.fill_usage();
                }
            });
        }

        let weak = Rc::downgrade(self);
        self.crew_list.connect_row_activated(move |_, row| {
            let Some(chat) = weak.upgrade() else { return };
            let key = chat.crew.borrow().get(row.index() as usize).map(|c| c.key.clone());
            let Some(key) = key else { return };
            let open = chat.on_crew_open.borrow().clone();
            match open {
                Some(open) => open(key),
                None => chat.go_to_tool(&key),
            }
        });
        // The strip's "4s ago" kept current.
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(5, move || {
            let Some(chat) = weak.upgrade() else { return glib::ControlFlow::Break };
            if !chat.crew.borrow().is_empty() {
                chat.show_crew();
            }
            glib::ControlFlow::Continue
        });

        let weak = Rc::downgrade(self);
        self.mode.connect_clicked(move |_| {
            if let Some(chat) = weak.upgrade() {
                chat.call(&chat.on_cycle_mode, ());
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, keycode, modifiers| {
            let Some(chat) = weak.upgrade() else { return glib::Propagation::Proceed };
            if chat.paste_image(keycode, modifiers) {
                return glib::Propagation::Stop;
            }
            chat.key(key, modifiers)
        });
        self.input.add_controller(keys);

        let drop = gtk::DropTarget::new(gtk::gdk::FileList::static_type(), gtk::gdk::DragAction::COPY);
        let weak = Rc::downgrade(self);
        drop.connect_drop(move |_, value, _, _| {
            let Some(chat) = weak.upgrade() else { return false };
            let Ok(files) = value.get::<gtk::gdk::FileList>() else { return false };
            let mut mentions = String::new();
            for file in files.files() {
                let Some(path) = file.path() else { continue };
                let name = path.to_string_lossy();
                let (kind, _) = gio::content_type_guess(Some(name.as_ref()), None);
                // Pictures go as pictures (Claude sees them); anything else is mentioned.
                if kind.starts_with("image/")
                    && let Ok(texture) = gdk::Texture::from_file(&file)
                {
                    chat.attach(&texture);
                } else {
                    mentions.push_str(&format!("@{} ", path.display()));
                }
            }
            chat.input.buffer().insert_at_cursor(&mentions);
            chat.input.grab_focus();
            true
        });
        self.input.add_controller(drop);

        let weak = Rc::downgrade(self);
        self.mic.connect_clicked(move |_| {
            if let Some(chat) = weak.upgrade() {
                chat.toggle_recording();
            }
        });

        let weak = Rc::downgrade(self);
        markdown_tags(&self.input);
        // The theme's colours change live: the tags' tints with them.
        let view = self.input.downgrade();
        crate::theme::on_change(move || {
            if let Some(view) = view.upgrade() {
                markdown_tags(&view);
            }
        });
        self.input.connect_map(markdown_tags);
        self.input.buffer().connect_changed(move |buffer| {
            if let Some(chat) = weak.upgrade() {
                // Marked once typing pauses for a moment (a long paste is not re-marked per key).
                if !chat.markdown_due.replace(true) {
                    let (weak, buffer) = (Rc::downgrade(&chat), buffer.clone());
                    glib::timeout_add_local_once(Duration::from_millis(120), move || {
                        if let Some(chat) = weak.upgrade() {
                            chat.markdown_due.set(false);
                            mark_markdown(&buffer);
                        }
                    });
                }
                chat.placeholder.set_visible(buffer.char_count() == 0);
                // Typed something of one's own: the prediction is not wanted.
                if buffer.char_count() > 0 && chat.suggestion.borrow().as_deref().is_some_and(|s| buffer.text(&buffer.start_iter(), &buffer.end_iter(), false) != s) {
                    chat.set_suggestion(None);
                }
                chat.update_completion();
                // "!" first: shell mode, as in Claude Code (a purple box that says so).
                let shell = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).trim_start().starts_with('!');
                chat.shell_badge.set_visible(shell);
                if shell {
                    chat.composer.add_css_class("shell-mode");
                } else {
                    chat.composer.remove_css_class("shell-mode");
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.completion_list.connect_row_activated(move |_, row| {
            if let Some(chat) = weak.upgrade() {
                chat.complete(row.index());
            }
        });
    }

    /// Ctrl+V with an image on the clipboard (a screenshot, a copied picture): attached
    /// instead of pasted as text. Any layout: the key is read on the first one.
    fn paste_image(self: &Rc<Self>, keycode: u32, modifiers: gdk::ModifierType) -> bool {
        let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let other = modifiers.intersects(gdk::ModifierType::ALT_MASK | gdk::ModifierType::SHIFT_MASK);
        if !ctrl || other {
            return false;
        }
        let base = WidgetExt::display(&self.input).translate_key(keycode, gdk::ModifierType::empty(), 0).map(|(k, ..)| k.to_lower());
        if base != Some(gdk::Key::v) {
            return false;
        }
        let clipboard = self.input.clipboard();
        if !clipboard.formats().contains_type(gdk::Texture::static_type()) {
            return false;
        }
        let weak = Rc::downgrade(self);
        clipboard.read_texture_async(None::<&gio::Cancellable>, move |result| {
            if let (Some(chat), Ok(Some(texture))) = (weak.upgrade(), result) {
                chat.attach(&texture);
            }
        });
        true
    }

    fn key(self: &Rc<Self>, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
        let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let completing = self.completion.is_visible();
        match key {
            gdk::Key::Return | gdk::Key::KP_Enter if completing && !shift => {
                self.complete(self.completion_list.selected_row().map_or(0, |row| row.index()));
            }
            // Ctrl+Enter: what is typed joins the queue, and the queue goes in now.
            gdk::Key::Return | gdk::Key::KP_Enter if ctrl && !shift => {
                self.submit();
                self.call(&self.on_send_now, ());
            }
            gdk::Key::Return | gdk::Key::KP_Enter if !shift => self.submit(),
            // The prediction in the empty input: in, not sent.
            gdk::Key::Right | gdk::Key::Tab if !completing && self.input.buffer().char_count() == 0 && self.suggestion.borrow().is_some() => {
                let text = self.suggestion.borrow().clone().unwrap_or_default();
                self.input.buffer().set_text(&text);
                self.input.buffer().place_cursor(&self.input.buffer().end_iter());
            }
            gdk::Key::Tab if completing => {
                self.complete(self.completion_list.selected_row().map_or(0, |row| row.index()));
            }
            gdk::Key::Down | gdk::Key::Up if completing => {
                let index = self.completion_list.selected_row().map_or(-1, |row| row.index());
                let next = if key == gdk::Key::Down { index + 1 } else { (index - 1).max(0) };
                if let Some(row) = self.completion_list.row_at_index(next) {
                    self.completion_list.select_row(Some(&row));
                }
            }
            // Reading back without the mouse: a page, the ends, message to message.
            gdk::Key::Page_Up | gdk::Key::Page_Down => self.scroll_page(key == gdk::Key::Page_Down),
            gdk::Key::Home | gdk::Key::End if ctrl => {
                if key == gdk::Key::End {
                    self.scroll_to_latest();
                } else {
                    self.stick.set(false);
                    self.scroller.vadjustment().set_value(0.0);
                }
            }
            gdk::Key::Up | gdk::Key::Down if ctrl => self.jump_message(key == gdk::Key::Down),
            // Earlier messages, from the first line up (and back down from the last).
            gdk::Key::Up if self.cursor_on_edge(true) && self.recall(true) => {}
            gdk::Key::Down if self.history_at.get().is_some() && self.cursor_on_edge(false) && self.recall(false) => {}
            _ if !completing && self.input.buffer().char_count() == 0 && digit(key).is_some() => {
                let answer = self.on_answer.borrow().clone();
                let answered = answer.is_some_and(|answer| answer(digit(key).unwrap_or(1) - 1));
                if !answered {
                    return glib::Propagation::Proceed;
                }
            }
            gdk::Key::ISO_Left_Tab => self.call(&self.on_cycle_mode, ()),
            gdk::Key::Escape if completing => self.completion.popdown(),
            // Esc on a subagent's conversation goes back to the main one first.
            gdk::Key::Escape if self.show_main() => {}
            gdk::Key::Escape => self.call(&self.on_cancel, ()),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    /// Whether the input's cursor is on its first line (`top`) or its last.
    fn cursor_on_edge(&self, top: bool) -> bool {
        let buffer = self.input.buffer();
        let cursor = buffer.iter_at_mark(&buffer.get_insert());
        if top { cursor.line() == 0 } else { cursor.line() == buffer.end_iter().line() }
    }

    /// Up (`older`) or Down through the sent messages; past the newest, what was being typed.
    fn recall(&self, older: bool) -> bool {
        let history = self.history.borrow();
        if history.is_empty() {
            return false;
        }
        let buffer = self.input.buffer();
        let next = match (self.history_at.get(), older) {
            (None, true) => {
                *self.history_draft.borrow_mut() = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
                Some(history.len() - 1)
            }
            (None, false) => return false,
            (Some(0), true) => Some(0),
            (Some(i), true) => Some(i - 1),
            (Some(i), false) if i + 1 < history.len() => Some(i + 1),
            (Some(_), false) => None,
        };
        self.history_at.set(next);
        let text = next.map_or_else(|| self.history_draft.borrow().clone(), |i| history[i].clone());
        buffer.set_text(&text);
        buffer.place_cursor(&if older { buffer.start_iter() } else { buffer.end_iter() });
        true
    }

    /// A page up or down through the conversation.
    fn scroll_page(&self, down: bool) {
        if !down {
            self.stick.set(false);
        }
        let adj = self.scroller.vadjustment();
        let step = adj.page_size() * 0.85;
        let value = if down { adj.value() + step } else { adj.value() - step };
        adj.set_value(value.clamp(0.0, (adj.upper() - adj.page_size()).max(0.0)));
    }

    /// To the previous (or next) message you sent.
    fn jump_message(&self, down: bool) {
        let adj = self.scroller.vadjustment();
        let now = adj.value();
        let mut tops = Vec::new();
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            if widget.has_css_class("msg-wrap-user")
                && let Some(point) = widget.compute_point(&self.list, &gtk::graphene::Point::new(0.0, 0.0))
            {
                tops.push(f64::from(point.y()) - 12.0);
            }
            child = widget.next_sibling();
        }
        let target = if down { tops.into_iter().find(|y| *y > now + 4.0) } else { tops.into_iter().rev().find(|y| *y < now - 4.0) };
        if let Some(y) = target {
            self.stick.set(false);
            adj.set_value(y.clamp(0.0, (adj.upper() - adj.page_size()).max(0.0)));
        } else if down {
            self.scroll_to_latest();
        }
    }

    /// The messages waiting for the turn to end (an empty one: a `!command`'s output), each
    /// with a × to take it back.
    pub fn show_queued(self: &Rc<Self>, messages: &[String]) {
        while let Some(child) = self.queued_box.first_child() {
            self.queued_box.remove(&child);
        }
        for (i, text) in messages.iter().enumerate() {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            line.add_css_class("queued-row");
            let shown = if text.is_empty() { "The output of your ! command".to_owned() } else { text.lines().next().unwrap_or("").to_owned() };
            let label = gtk::Label::builder().label(&shown).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).single_line_mode(true).build();
            label.set_tooltip_text(Some(text));
            let mark = gtk::Label::new(Some("Queued"));
            mark.add_css_class("queued-mark");
            let remove = gtk::Button::with_label("×");
            remove.add_css_class("flat");
            remove.set_tooltip_text(Some("Take it back into the input"));
            let weak = Rc::downgrade(self);
            remove.connect_clicked(move |_| {
                if let Some(chat) = weak.upgrade() {
                    chat.call(&chat.on_unqueue, i);
                }
            });
            line.append(&mark);
            line.append(&label);
            line.append(&remove);
            self.queued_box.append(&line);
        }
        let hint = gtk::Label::builder().label("Sent when Claude finishes · Ctrl+Enter sends now").xalign(0.0).build();
        hint.add_css_class("queued-hint");
        self.queued_box.append(&hint);
        self.queued_box.set_visible(!messages.is_empty());
    }

    /// A queued message taken back: into the input (before what is typed there).
    pub fn unqueue(self: &Rc<Self>, text: &str, images: Vec<glib::Bytes>) {
        let buffer = self.input.buffer();
        let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
        let joined = [text, typed.trim()].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join("\n\n");
        buffer.set_text(&joined);
        self.attachments.borrow_mut().extend(images);
        self.show_attachments();
        self.input.grab_focus();
    }

    /// Messages that never went out (the turn was stopped first): back in the input, with
    /// their images, so nothing typed is lost.
    pub fn restore_unsent(self: &Rc<Self>, messages: Vec<(String, Vec<glib::Bytes>)>) {
        if messages.is_empty() {
            return;
        }
        let buffer = self.input.buffer();
        let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
        let mut texts: Vec<String> = messages.iter().map(|(t, _)| t.clone()).filter(|t| !t.is_empty()).collect();
        if !typed.trim().is_empty() {
            texts.push(typed);
        }
        buffer.set_text(&texts.join("\n\n"));
        for (_, images) in messages {
            self.attachments.borrow_mut().extend(images);
        }
        self.show_attachments();
        self.notice("Not sent (the turn was stopped first): it is back in the input.", "dim");
    }

    /// The tokens in the context, as last reported.
    pub fn context_used(&self) -> u64 {
        self.spent.borrow().0.map_or(0, |(used, _)| used)
    }

    /// The draft in the input (kept across restarts).
    pub fn draft(&self) -> String {
        let buffer = self.input.buffer();
        buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
    }

    /// The usage popover: this conversation's context and cost, then the account's limits
    /// (from `ai-usagebar`, when installed; else the limit the agent last reported).
    fn fill_usage(self: &Rc<Self>) {
        let body = &self.usage_body;
        while let Some(child) = body.first_child() {
            body.remove(&child);
        }
        let (context, cost, limit) = self.spent.borrow().clone();
        let k = |n: u64| if n >= 1000 { format!("{}k", n / 1000) } else { n.to_string() };
        body.append(&usage_heading("This conversation"));
        match context {
            Some((used, size)) if size > 0 => {
                let share = used as f64 / size as f64;
                body.append(&usage_row(
                    "Context",
                    share,
                    &format!("{}%", (share * 100.0).round()),
                    &format!("{} of {} tokens · {} left", k(used), k(size), k(size.saturating_sub(used))),
                ));
            }
            Some((used, _)) => body.append(&usage_note(&format!("Context: {} tokens", k(used)))),
            None => body.append(&usage_note("Context: not reported yet")),
        }
        let (parts, compacts_at) = self.context_parts.borrow().clone();
        if !parts.is_empty() {
            body.append(&usage_parts(&parts, &k));
        }
        if let Some(at) = compacts_at {
            body.append(&usage_note(&format!("Compacts by itself at {} tokens.", k(at))));
        }
        if let Some(cost) = cost {
            body.append(&usage_note(&format!("Cost: {cost}")));
        }
        if let Some(note) = self.version_note.borrow().as_deref() {
            let warn = usage_note(&format!("⚠ {note}"));
            warn.add_css_class("version-warn");
            body.append(&warn);
        }
        let account = gtk::Box::new(gtk::Orientation::Vertical, 6);
        account.append(&usage_heading("Account"));
        account.append(&usage_note("Loading…"));
        body.append(&account);
        glib::spawn_future_local(async move {
            let found = gio::spawn_blocking(account_usage).await.ok().flatten();
            account.first_child().and_then(|h| h.next_sibling()).inspect(|loading| account.remove(loading));
            let Some(found) = found else {
                account.append(&usage_note(&limit.unwrap_or_else(|| "No limits reported yet (ai-usagebar not available).".into())));
                return;
            };
            if let Some(heading) = account.first_child().and_downcast::<gtk::Label>()
                && let Some(plan) = found["plan"].as_str()
            {
                heading.set_label(&format!("ACCOUNT · {}", plan.to_uppercase()));
            }
            for metric in found["metrics"].as_array().into_iter().flatten() {
                let percent = metric["percent"].as_f64().unwrap_or(0.0);
                account.append(&usage_row(
                    metric["label"].as_str().unwrap_or("Limit"),
                    percent / 100.0,
                    metric["value"].as_str().unwrap_or(""),
                    metric["detail"].as_str().unwrap_or(""),
                ));
            }
            let credits = &found["reset_credits"];
            if !credits.is_null() {
                let text = credits.as_str().map(str::to_owned).unwrap_or_else(|| credits.to_string());
                account.append(&usage_note(&format!("Usage resets left: {text}")));
            }
            if let Some(error) = found["error"].as_str() {
                account.append(&usage_note(error));
            }
        });
    }

    /// The usage popover's content, filled (for tests: a popover cannot open off screen).
    pub fn usage_content(self: &Rc<Self>, fill: bool) -> gtk::Widget {
        if fill {
            self.fill_usage();
        }
        self.usage_body.clone().upcast()
    }

    /// The conversation's cost and the account's limit ("$0.42 · 5h 38%").
    pub fn set_spend(&self, cost: Option<String>, limit: Option<(String, String)>) {
        {
            let mut spent = self.spent.borrow_mut();
            spent.1 = cost.clone();
            spent.2 = limit.as_ref().map(|(_, long)| long.clone());
        }
        let mut parts = Vec::new();
        if let Some(cost) = cost {
            parts.push(cost);
        }
        if let Some((short, _)) = limit {
            parts.push(short);
        }
        self.spend.set_label(&parts.join(" · "));
        self.spend.set_visible(!parts.is_empty());
    }

    fn call<T>(&self, callback: &Callback<T>, value: T) {
        let callback = callback.borrow().clone();
        if let Some(callback) = callback {
            callback(value);
        }
    }

    fn submit(self: &Rc<Self>) {
        let buffer = self.input.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).trim().to_owned();
        if text.is_empty() && self.attachments.borrow().is_empty() {
            return;
        }
        buffer.set_text("");
        if !text.is_empty() {
            let mut history = self.history.borrow_mut();
            if history.last() != Some(&text) {
                history.push(text.clone());
            }
        }
        self.history_at.set(None);
        let images = self.attachments.take();
        self.show_attachments();
        self.stick.set(true);
        self.set_suggestion(None);
        self.call(&self.on_submit, (text, images));
    }

    /// The microphone shown or not (speech to text set up or not).
    pub fn set_mic_visible(&self, shown: bool) {
        self.mic.set_visible(shown);
        // Switched off while recording: the recording stops (and is thrown away).
        if !shown && self.recording.borrow_mut().take().is_some() {
            self.mic.remove_css_class("recording");
        }
    }

    /// The microphone: recording starts; clicked again, what was said is written into the
    /// input (not sent: read it, then send).
    fn toggle_recording(self: &Rc<Self>) {
        let running = self.recording.borrow_mut().take();
        match running {
            None => match crate::stt::record() {
                Ok(recording) => {
                    *self.recording.borrow_mut() = Some(recording);
                    self.mic.add_css_class("recording");
                    self.mic.set_tooltip_text(Some("Recording: click to stop and write it here"));
                }
                Err(why) => self.notice(&format!("Can't record: {why}"), "error"),
            },
            Some(recording) => {
                self.mic.remove_css_class("recording");
                self.mic.add_css_class("busy");
                self.mic.set_sensitive(false);
                self.mic.set_tooltip_text(Some("Writing down what you said…"));
                let audio = move || recording.finish();
                let weak = Rc::downgrade(self);
                let done = move |result: Result<String, String>| {
                    let Some(chat) = weak.upgrade() else { return };
                    chat.mic.remove_css_class("busy");
                    chat.mic.set_sensitive(true);
                    chat.mic.set_tooltip_text(Some("Speak: click to record, click again to write it here"));
                    match result {
                        Ok(text) if text.is_empty() => chat.notice("Heard nothing.", "dim"),
                        Ok(text) => {
                            let buffer = chat.input.buffer();
                            let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                            let joiner = if typed.is_empty() || typed.ends_with(char::is_whitespace) { "" } else { " " };
                            buffer.insert(&mut buffer.end_iter(), &format!("{joiner}{text}"));
                            chat.input.grab_focus();
                        }
                        Err(why) => chat.notice(&format!("Speech to text: {why}"), "error"),
                    }
                };
                // Stopping waits on the recorder: not on the main loop.
                glib::spawn_future_local(async move {
                    let audio = gio::spawn_blocking(move || audio()).await.unwrap_or_else(|_| Err("the recorder did not stop".to_owned()));
                    match audio {
                        Ok(audio) => crate::stt::transcribe(audio, "voice.wav", "audio/wav", done),
                        Err(why) => done(Err(why)),
                    }
                });
            }
        }
    }

    /// The next prompt Claude Code predicts (None: none, the hint again).
    pub fn set_suggestion(&self, text: Option<String>) {
        let text = text.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        match &text {
            Some(text) => {
                let first = text.lines().next().unwrap_or("");
                self.placeholder.set_markup(&format!(
                    "{}   <span size=\"small\" alpha=\"60%\">Tab fills it in</span>",
                    glib::markup_escape_text(first)
                ));
                self.placeholder.set_tooltip_text(Some(text));
                self.placeholder.add_css_class("suggesting");
            }
            None => {
                self.placeholder.set_label("Ask Claude…   @ file · / commands · ! run · Ctrl+V pastes images");
                self.placeholder.set_tooltip_text(None);
                self.placeholder.remove_css_class("suggesting");
            }
        }
        *self.suggestion.borrow_mut() = text;
    }

    /// Sends `text` as the Compact button does: never through the input (test hook).
    pub fn submit_direct(&self, text: &str) {
        self.call(&self.on_submit, (text.to_owned(), Vec::new()));
    }

    /// Sends `text` as if typed and sent (with any images attached).
    pub fn send_text(self: &Rc<Self>, text: &str) {
        self.input.buffer().set_text(text);
        self.submit();
    }

    /// An image to send with the next message (pasted or dropped), as PNG.
    pub fn attach(self: &Rc<Self>, texture: &gdk::Texture) {
        self.attachments.borrow_mut().push(texture.save_to_png_bytes());
        self.show_attachments();
        self.input.grab_focus();
    }

    /// The attached images over the input, each with a × to take it off.
    fn show_attachments(self: &Rc<Self>) {
        while let Some(child) = self.attach_box.first_child() {
            self.attach_box.remove(&child);
        }
        let images = self.attachments.borrow().clone();
        for (i, png) in images.iter().enumerate() {
            let Some(picture) = thumbnail(png, 96, 64) else { continue };
            picture.set_valign(gtk::Align::Center);
            picture.add_css_class("attachment");
            let frame = gtk::Overlay::builder().child(&picture).build();
            let remove = gtk::Button::with_label("×");
            remove.add_css_class("attachment-remove");
            remove.set_halign(gtk::Align::End);
            remove.set_valign(gtk::Align::Start);
            remove.set_tooltip_text(Some("Take this image off"));
            let weak = Rc::downgrade(self);
            remove.connect_clicked(move |_| {
                if let Some(chat) = weak.upgrade() {
                    if i < chat.attachments.borrow().len() {
                        chat.attachments.borrow_mut().remove(i);
                    }
                    chat.show_attachments();
                }
            });
            frame.add_overlay(&remove);
            self.attach_box.append(&frame);
        }
        self.attach_box.set_visible(!images.is_empty());
    }

    /// Images sent with a message, under it.
    pub fn user_images(&self, pngs: &[glib::Bytes]) {
        if pngs.is_empty() {
            return;
        }
        // They wrap onto more lines: in one row, several would hold the chat wider than the window.
        let row = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(8)
            .row_spacing(8)
            .max_children_per_line(16)
            .can_focus(false)
            .build();
        row.add_css_class("msg-images");
        for png in pngs {
            let Some(picture) = thumbnail(png, 360, 220) else { continue };
            picture.add_css_class("msg-image");
            picture.set_halign(gtk::Align::Start);
            row.insert(&picture, -1);
            if let Some(cell) = picture.parent() {
                cell.set_can_focus(false);
            }
        }
        self.list.append(&row);
        *self.last.borrow_mut() = Last::None;
    }

    /// An image from a resumed conversation (base64, as the agent replays it).
    pub fn user_image_data(&self, base64: &str) {
        let bytes = glib::base64_decode(base64);
        self.user_images(&[glib::Bytes::from_owned(bytes)]);
    }

    // ---- slash commands -------------------------------------------------------------------

    pub fn set_commands(&self, commands: Vec<(String, String)>) {
        *self.commands.borrow_mut() = commands;
    }

    /// Where the agent works (for @ file mentions).
    pub fn set_cwd(&self, cwd: &std::path::Path) {
        *self.cwd.borrow_mut() = cwd.to_owned();
        self.project_files.borrow_mut().take();
    }

    /// What the text before the cursor asks to complete: a "/command" (the whole input) or
    /// an "@file" (the word at the cursor). Returns the offered (insert, name, detail) and
    /// the character range the choice replaces.
    fn matches(&self) -> (Vec<(String, String, String)>, (i32, i32)) {
        let buffer = self.input.buffer();
        let cursor = buffer.iter_at_mark(&buffer.get_insert());
        let before = buffer.text(&buffer.start_iter(), &cursor, false).to_string();
        let none = (Vec::new(), (0, 0));
        // "/word" as the whole input so far.
        if let Some(word) = before.strip_prefix('/')
            && !word.contains(char::is_whitespace)
        {
            let commands = self.commands.borrow();
            let mut found: Vec<&(String, String)> = commands.iter().filter(|(name, _)| name.starts_with(word)).collect();
            found.extend(commands.iter().filter(|(name, _)| !name.starts_with(word) && name.contains(word)));
            let offered = found
                .into_iter()
                .take(8)
                .map(|(name, description)| (format!("/{name} "), format!("/{name}"), description.lines().next().unwrap_or("").to_owned()))
                .collect();
            return (offered, (0, cursor.offset()));
        }
        // "@part" just before the cursor.
        let Some(at) = before.rfind('@') else { return none };
        let word = &before[at + 1..];
        if word.contains(char::is_whitespace) || (at > 0 && !before[..at].ends_with(char::is_whitespace)) {
            return none;
        }
        if self.project_files.borrow().is_none() {
            let files = crate::quickopen::list_files(&self.cwd.borrow());
            *self.project_files.borrow_mut() = Some(files);
        }
        let files = self.project_files.borrow();
        let mut ranked: Vec<(i64, &String)> =
            files.iter().flatten().filter_map(|f| crate::picker::score(word, f).map(|s| (s, f))).collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
        let offered = ranked
            .into_iter()
            .take(8)
            .map(|(_, f)| {
                let (dir, name) = f.rsplit_once('/').map_or(("", f.as_str()), |(d, n)| (d, n));
                (format!("@{f} "), name.to_owned(), dir.to_owned())
            })
            .collect();
        let start = before[..at].chars().count() as i32;
        (offered, (start, cursor.offset()))
    }

    fn update_completion(&self) {
        let (found, _) = self.matches();
        while let Some(row) = self.completion_list.row_at_index(0) {
            self.completion_list.remove(&row);
        }
        if found.is_empty() {
            self.completion.popdown();
            *self.offered.borrow_mut() = found;
            return;
        }
        for (_, name, description) in &found {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let name = gtk::Label::new(Some(name));
            name.add_css_class("mono");
            let description = gtk::Label::builder()
                .label(description)
                .ellipsize(pango::EllipsizeMode::End)
                .max_width_chars(60)
                .xalign(0.0)
                .build();
            description.add_css_class("dim");
            row.append(&name);
            row.append(&description);
            self.completion_list.append(&row);
        }
        *self.offered.borrow_mut() = found;
        if let Some(first) = self.completion_list.row_at_index(0) {
            self.completion_list.select_row(Some(&first));
        }
        self.completion.popup();
    }

    fn complete(&self, index: i32) {
        let (found, (start, end)) = self.matches();
        if let Some((insert, _, _)) = found.get(index.max(0) as usize) {
            let buffer = self.input.buffer();
            let mut from = buffer.iter_at_offset(start);
            let mut to = buffer.iter_at_offset(end);
            buffer.delete(&mut from, &mut to);
            let mut at = buffer.iter_at_offset(start);
            buffer.insert(&mut at, insert);
        }
        self.completion.popdown();
        self.input.grab_focus();
    }

    // ---- header ---------------------------------------------------------------------------

    /// `place` is shown; `full` (the whole path) is its tooltip.
    pub fn set_title(&self, title: &str, place: &str, full: &str) {
        self.title.set_label(title);
        self.place.set_label(place);
        self.place.set_tooltip_text(Some(full));
    }

    /// `text` beside the title, and the status dot (a css class: working, blocked…).
    pub fn set_state(&self, text: &str, class: &str) {
        self.state.set_label(text);
        self.state.set_visible(!text.is_empty());
        for c in ["working", "starting", "blocked", "done", "failed", "suspended", "idle"] {
            self.dot.remove_css_class(c);
            self.state.remove_css_class(c);
        }
        self.dot.add_css_class(class);
        self.state.add_css_class(class);
    }

    /// While the agent starts, the empty conversation says so.
    pub fn set_starting(&self, text: Option<&str>) {
        self.starting.set_label(text.unwrap_or(""));
        // Only an empty conversation needs it (a resumed one fills in by itself).
        let empty = self.list.first_child().and_then(|c| c.next_sibling()).is_none();
        self.starting.set_visible(text.is_some() && empty);
    }

    pub fn set_mode(&self, mode: &str) {
        let name = match mode {
            "default" => "Ask first",
            "acceptEdits" => "Accept edits",
            "plan" => "Plan",
            "auto" => "Auto",
            "bypassPermissions" => "Bypass",
            other => other,
        };
        self.mode.set_label(&format!("Mode · {name}"));
        for class in ["plan-mode", "accept-mode", "bypass-mode"] {
            self.mode.remove_css_class(class);
        }
        match mode {
            "plan" => self.mode.add_css_class("plan-mode"),
            "acceptEdits" | "auto" => self.mode.add_css_class("accept-mode"),
            "bypassPermissions" => self.mode.add_css_class("bypass-mode"),
            _ => {}
        }
    }

    /// Session settings as chips, each with its menu: "Model · Opus 5.5", "Effort · High · Fast"
    /// (effort and speed, one menu), "Agent · …". All shown (nothing behind a "⋯"); the menus
    /// open upwards, the box being at the window's bottom. The mode has its own pill.
    pub fn set_options(&self, options: &Value) {
        while let Some(child) = self.options.first_child() {
            self.options.remove(&child);
        }
        let all: Vec<&Value> = options.as_array().into_iter().flatten().filter(|o| o["id"].as_str().is_some_and(|id| id != "mode")).collect();
        // Speed goes in effort's chip (and menu) when there is an effort to go with.
        let choices = |o: &&Value| !flat_values(&o["options"]).is_empty();
        let effort = all.iter().find(|o| (o["id"] == "effort" || o["category"] == "thought_level") && choices(o)).copied();
        let fast = all
            .iter()
            .find(|o| (o["id"] == "fast" || o["name"].as_str().is_some_and(|n| n.starts_with("Fast"))) && choices(o))
            .copied();
        let joined = effort.is_some() && fast.is_some();
        for option in &all {
            let id = option["id"].as_str().unwrap_or("");
            if joined && fast.is_some_and(|f| std::ptr::eq(*option, f)) {
                continue;
            }
            let values = flat_values(&option["options"]);
            if values.is_empty() {
                continue;
            }
            let name = option["name"].as_str().unwrap_or(id);
            let current = option["currentValue"].as_str().unwrap_or("");
            // "Fast mode · Off" reads as "Fast · Off": the chip is short, the menu says the rest.
            let short = name.strip_suffix(" mode").unwrap_or(name);
            let mut label = format!("{short} · {}", chip_value(&values, current));
            let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
            list.add_css_class("option-list");
            let popover = gtk::Popover::builder().child(&list).position(gtk::PositionType::Top).build();
            self.option_items(&list, &popover, id, name, &values, current);
            let mut tip = option["description"].as_str().unwrap_or(name).to_owned();
            if joined && effort.is_some_and(|e| std::ptr::eq(*option, e))
                && let Some(fast) = fast
            {
                let fast_values = flat_values(&fast["options"]);
                let fast_current = fast["currentValue"].as_str().unwrap_or("");
                let fast_id = fast["id"].as_str().unwrap_or("fast");
                let fast_name = fast["name"].as_str().unwrap_or("Fast mode");
                self.option_items(&list, &popover, fast_id, fast_name, &fast_values, fast_current);
                if !at_default(&fast_values, fast_current) {
                    label.push_str(" · Fast");
                }
                tip = format!("{tip} · {fast_name}: {}", chip_value(&fast_values, fast_current));
            }
            let button = gtk::MenuButton::builder().label(&label).popover(&popover).build();
            button.add_css_class("chip-picker");
            button.add_css_class("flat");
            button.set_tooltip_text(Some(&tip));
            self.options.append(&button);
        }
    }

    /// One setting's choices under its heading, in `list` (inside `popover`).
    fn option_items(&self, list: &gtk::Box, popover: &gtk::Popover, id: &str, name: &str, values: &[(String, String, String)], current: &str) {
        let heading = gtk::Label::builder().label(name).xalign(0.0).build();
        heading.add_css_class("option-heading");
        list.append(&heading);
        for (value, name, description) in values {
            let line = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let title = gtk::Label::builder().label(if value == current { format!("✓  {name}") } else { format!("    {name}") }).xalign(0.0).build();
            line.append(&title);
            if !description.is_empty() {
                let detail = gtk::Label::builder().label(description).xalign(0.0).wrap(true).max_width_chars(44).build();
                detail.add_css_class("dim");
                detail.add_css_class("small");
                detail.set_margin_start(22);
                line.append(&detail);
            }
            let item = gtk::Button::builder().child(&line).build();
            item.add_css_class("flat");
            item.add_css_class("menu-item");
            let callback = self.on_option.borrow().clone();
            let (id, value, popover) = (id.to_owned(), value.clone(), popover.clone());
            item.connect_clicked(move |_| {
                popover.popdown();
                if let Some(callback) = &callback {
                    callback((id.clone(), value.clone()));
                }
            });
            list.append(&item);
        }
    }

    /// A turn runs (Some: since when) or not: the composer says so, and send becomes stop.
    pub fn set_working(self: &Rc<Self>, since: Option<std::time::Instant>) {
        let was = self.working_since.replace(since);
        self.show_compact();
        self.send.set_label(if since.is_some() { "■" } else { "↑" });
        self.send.set_tooltip_text(Some(if since.is_some() { "Stop (Esc)" } else { "Send (Enter)" }));
        if since.is_some() {
            self.send.add_css_class("stop");
        } else {
            self.send.remove_css_class("stop");
            self.working.set_label("");
            self.crew.borrow_mut().retain(|c| !c.guessed && c.ended.is_none());
            self.show_crew();
        }
        if since.is_some() && was.is_none() {
            self.tick_working();
            let weak = Rc::downgrade(self);
            glib::timeout_add_seconds_local(1, move || {
                let Some(chat) = weak.upgrade() else { return glib::ControlFlow::Break };
                if chat.working_since.get().is_none() {
                    return glib::ControlFlow::Break;
                }
                chat.tick_working();
                glib::ControlFlow::Continue
            });
        }
    }

    fn tick_working(&self) {
        if let Some(since) = self.working_since.get() {
            self.working.set_label(&format!("Working {}", elapsed(since.elapsed().as_secs())));
            self.working.set_tooltip_text(Some("Esc stops it"));
        }
    }

    /// A quiet line under a finished turn: how long it took.
    pub fn turn_done(&self, seconds: u64) {
        self.crew.borrow_mut().retain(|c| !c.guessed && c.ended.is_none());
        self.show_crew();
        self.group.take();
        *self.last.borrow_mut() = Last::None;
        // Files written meanwhile can be mentioned next.
        self.project_files.borrow_mut().take();
        let time = glib::DateTime::now_local().ok().and_then(|t| t.format("%H:%M").ok()).map(|t| t.to_string()).unwrap_or_default();
        let text = if seconds < 3 { time } else { format!("{time} · Worked for {}", elapsed(seconds)) };
        let label = gtk::Label::builder().label(text).xalign(0.0).build();
        label.add_css_class("turn-footer");
        self.list.append(&label);
        *self.last.borrow_mut() = Last::None;
    }

    pub fn set_show_thinking(&self, show: bool) {
        self.show_thinking.set(show);
        self.thoughts.borrow_mut().retain(|weak| match weak.upgrade() {
            Some(expander) => {
                expander.set_visible(show);
                true
            }
            None => false,
        });
    }

    /// What fills the context, for the usage popover.
    pub fn set_context_parts(&self, parts: Vec<(String, u64)>, compacts_at: Option<u64>) {
        *self.context_parts.borrow_mut() = (parts, compacts_at);
    }

    pub fn set_usage(&self, used: u64, size: u64) {
        let k = |n: u64| if n >= 1000 { format!("{}k", n / 1000) } else { n.to_string() };
        self.spent.borrow_mut().0 = Some((used, size));
        if size == 0 {
            self.usage.set_label(&k(used));
            self.context.set_visible(false);
            return;
        }
        let share = used as f64 / size as f64;
        self.usage.set_label(&format!("{}%", (share * 100.0).round() as u32));
        self.context.set_value(share.min(1.0));
        self.context.set_visible(true);
        if share > 0.8 {
            self.context.add_css_class("full");
        } else {
            self.context.remove_css_class("full");
        }
        self.show_compact();
    }

    /// The Compact button, when the setting is on and the context is at least that full.
    fn show_compact(&self) {
        let s = crate::settings::get();
        let share = match self.spent.borrow().0 {
            Some((used, size)) if size > 0 => used as f64 / size as f64,
            _ => 0.0,
        };
        let percent = (share * 100.0).round() as u64;
        let show = s.compact_alert && percent >= s.compact_at && self.working_since.get().is_none();
        if show {
            self.compact.set_tooltip_text(Some(&format!("The context is {percent}% full: summarise the conversation so far to make room (/compact)")));
        }
        self.compact.set_visible(show);
    }

    // ---- messages -------------------------------------------------------------------------

    pub fn clear(&self) {
        // The guess was about the conversation that went.
        self.set_suggestion(None);
        // Everything but the "starting" note, which stays first.
        while let Some(child) = self.starting.next_sibling() {
            self.list.remove(&child);
        }
        *self.last.borrow_mut() = Last::None;
        self.group.take();
        self.messages.borrow_mut().clear();
        self.tools.borrow_mut().clear();
        self.set_plan(&Value::Null);
    }

    pub fn user(&self, text: &str) {
        self.scroll_to_latest();
        let message = self.message(Kind::User);
        message.text.borrow_mut().push_str(text);
        message.render_soon();
        *self.last.borrow_mut() = Last::None;
    }

    /// A piece of a message streaming in (`kind`: who speaks).
    fn chunk(&self, kind: Kind, text: &str) {
        let current = match (&*self.last.borrow(), kind) {
            (Last::Agent(m), Kind::Agent) | (Last::Thought(m), Kind::Thought) | (Last::User(m), Kind::User) => {
                Some(m.clone())
            }
            _ => None,
        };
        let message = current.unwrap_or_else(|| {
            let message = self.message(kind);
            *self.last.borrow_mut() = match kind {
                Kind::Agent => Last::Agent(message.clone()),
                Kind::Thought => Last::Thought(message.clone()),
                Kind::User => Last::User(message.clone()),
            };
            message
        });
        message.text.borrow_mut().push_str(text);
        message.render_soon();
    }

    /// A piece of the agent's message, with the agent's id for the message.
    pub fn agent_text_with(&self, text: &str, id: Option<&str>) {
        // Another message of the agent's in the same bubble (a read-back conversation has no
        // tool call between two replies): a paragraph of its own, not run into the last line.
        let another = match (&*self.last.borrow(), id) {
            (Last::Agent(message), Some(id)) => message.id.borrow().as_deref().is_some_and(|old| old != id) && !message.text.borrow().ends_with("\n\n"),
            _ => false,
        };
        if another {
            self.chunk(Kind::Agent, "\n\n");
        }
        self.chunk(Kind::Agent, text);
        if let (Some(id), Last::Agent(message)) = (id, &*self.last.borrow()) {
            // Messages run together in one bubble: the latest one's id (forking there keeps all).
            *message.id.borrow_mut() = Some(id.to_owned());
        }
    }

    /// Whether anything is shown yet (a resume can skip the replay then).
    pub fn has_messages(&self) -> bool {
        !self.messages.borrow().is_empty()
    }

    /// What Claude said last (for the overview).
    pub fn last_reply(&self) -> Option<String> {
        let messages = self.messages.borrow();
        let last = messages.iter().rev().find(|m| m.kind == Kind::Agent)?;
        let text = last.text.borrow().trim().to_owned();
        (!text.is_empty()).then_some(text)
    }

    /// The conversation as Markdown: who said what.
    pub fn transcript(&self) -> String {
        let mut out = String::new();
        for message in self.messages.borrow().iter() {
            let text = message.text.borrow();
            match message.kind {
                Kind::User => out.push_str(&format!("## You\n\n{}\n\n", text.trim())),
                Kind::Agent => out.push_str(&format!("## Claude\n\n{}\n\n", text.trim())),
                Kind::Thought => {}
            }
        }
        out
    }

    pub fn thought(&self, text: &str) {
        self.chunk(Kind::Thought, text);
    }

    pub fn user_chunk(&self, text: &str) {
        // The summary a compacted conversation goes on from: folded, its own kind of block.
        let starts = !matches!(&*self.last.borrow(), Last::User(_));
        if starts && text.trim_start().starts_with(SUMMARY_START) {
            let message = self.summary();
            message.text.borrow_mut().push_str(text);
            message.render_soon();
            // A replay gives a message in one piece: the next one is the user's own words.
            *self.last.borrow_mut() = Last::None;
            return;
        }
        self.chunk(Kind::User, text);
    }

    /// A compaction's summary: one line, opened to read it (as Markdown). Not one of the
    /// conversation's messages (export, the last reply, rewind).
    fn summary(&self) -> Rc<Message> {
        self.group.take();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        body.add_css_class("msg-agent");
        let expander = gtk::Expander::new(Some("Summary of the conversation before: what Claude carries on from"));
        expander.add_css_class("thought");
        expander.add_css_class("summary");
        expander.set_child(Some(&body));
        self.list.append(&expander);
        Rc::new(Message { kind: Kind::Agent, text: RefCell::default(), body, scheduled: Cell::new(false), id: RefCell::default() })
    }

    fn message(&self, kind: Kind) -> Rc<Message> {
        let message = self.make_message(kind);
        self.messages.borrow_mut().push(message.clone());
        message
    }

    fn make_message(&self, kind: Kind) -> Rc<Message> {
        // Hidden thinking between tool calls doesn't part them.
        if kind != Kind::Thought || self.show_thinking.get() {
            self.group.take();
        }
        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        match kind {
            Kind::Agent => {
                body.add_css_class("msg-agent");
                let wrap = gtk::Overlay::builder().child(&body).build();
                wrap.add_css_class("msg-wrap");
                let copy = gtk::Button::with_label("copy");
                copy.add_css_class("copy");
                copy.add_css_class("flat");
                copy.set_halign(gtk::Align::End);
                copy.set_valign(gtk::Align::Start);
                copy.set_tooltip_text(Some("Copy the message (Markdown)"));
                wrap.add_overlay(&copy);
                self.list.append(&wrap);
                let message = Rc::new(Message { kind, text: RefCell::default(), body, scheduled: Cell::new(false), id: RefCell::default() });
                message_menu(&wrap, &message, &self.input, vec![("Copy the whole message", copy.clone())]);
                let weak = Rc::downgrade(&message);
                copy.connect_clicked(move |button| {
                    if let Some(message) = weak.upgrade() {
                        button.clipboard().set_text(message.text.borrow().trim());
                        button.set_label("copied");
                        let button = button.clone();
                        glib::timeout_add_local_once(Duration::from_millis(1200), move || button.set_label("copy"));
                    }
                });
                return message;
            }
            Kind::User => {
                body.add_css_class("msg-user");
                let wrap = gtk::Overlay::builder().child(&body).build();
                wrap.add_css_class("msg-wrap");
                wrap.add_css_class("msg-wrap-user");
                let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
                actions.set_halign(gtk::Align::End);
                actions.set_valign(gtk::Align::Start);
                let copy = gtk::Button::with_label("copy");
                let edit = gtk::Button::with_label("edit");
                edit.set_tooltip_text(Some("Put it back in the input to change and send again"));
                let rewind = gtk::Button::with_label("rewind");
                rewind.set_tooltip_text(Some("A copy of the conversation from just before this message, with it in the input to change (this one is kept)"));
                for button in [&copy, &edit, &rewind] {
                    button.add_css_class("copy");
                    button.add_css_class("flat");
                    actions.append(button);
                }
                wrap.add_overlay(&actions);
                self.list.append(&wrap);
                let message = Rc::new(Message { kind, text: RefCell::default(), body, scheduled: Cell::new(false), id: RefCell::default() });
                message_menu(
                    &wrap,
                    &message,
                    &self.input,
                    vec![("Copy the whole message", copy.clone()), ("Edit and send again", edit.clone()), ("Rewind to here", rewind.clone())],
                );
                let weak = Rc::downgrade(&message);
                copy.connect_clicked(move |button| {
                    if let Some(message) = weak.upgrade() {
                        button.clipboard().set_text(message.text.borrow().trim());
                    }
                });
                let (weak, callback) = (Rc::downgrade(&message), self.on_rewind.borrow().clone());
                let messages = Rc::downgrade(&self.messages);
                rewind.connect_clicked(move |_| {
                    let (Some(message), Some(callback), Some(messages)) = (weak.upgrade(), callback.clone(), messages.upgrade()) else { return };
                    let messages = messages.borrow();
                    let position = messages.iter().position(|m| Rc::ptr_eq(m, &message)).unwrap_or(0);
                    let previous = messages[..position].iter().rev().find(|m| m.kind == Kind::Agent);
                    let before = previous.and_then(|m| m.id.borrow().clone());
                    let (_, rest) = split_shell_runs(&message.text.borrow());
                    callback((before, previous.is_some(), rest.trim().to_owned()));
                });
                let (weak, input) = (Rc::downgrade(&message), self.input.clone());
                edit.connect_clicked(move |_| {
                    if let Some(message) = weak.upgrade() {
                        let (_, rest) = split_shell_runs(&message.text.borrow());
                        input.buffer().set_text(rest.trim());
                        input.grab_focus();
                    }
                });
                return message;
            }
            Kind::Thought => {
                let expander = gtk::Expander::new(Some("Thinking"));
                expander.add_css_class("thought");
                expander.set_child(Some(&body));
                expander.set_visible(self.show_thinking.get());
                self.thoughts.borrow_mut().push(expander.downgrade());
                self.list.append(&expander);
            }
        }
        Rc::new(Message { kind, text: RefCell::default(), body, scheduled: Cell::new(false), id: RefCell::default() })
    }

    /// A block of its own in the conversation, after what is there (a workflow's card).
    pub fn block(&self, widget: &impl IsA<gtk::Widget>) {
        self.group.take();
        self.list.append(widget);
        *self.last.borrow_mut() = Last::None;
    }

    /// What the agent's workflows are at ("Workflow council: 120 of 180 agents done"), said
    /// where "Working…" is between turns. None: none runs.
    pub fn set_workflow_note(&self, note: Option<String>) {
        *self.workflow_note.borrow_mut() = note;
        self.background_note();
    }

    /// A notice with a button that does something about it.
    pub fn notice_action(&self, text: &str, button: &str, action: Rc<dyn Fn()>) {
        // A retry that failed again replaces its card rather than stacking another.
        if let Some(last) = self.list.last_child()
            && last.has_css_class("notice")
            && last.first_child().and_downcast::<gtk::Label>().is_some_and(|l| l.text().replace(['\u{200E}', '\u{200F}'], "") == text)
        {
            self.list.remove(&last);
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.add_css_class("notice");
        line.add_css_class("error");
        let label = text_label(&glib::markup_escape_text(text));
        label.set_hexpand(true);
        let go = gtk::Button::with_label(button);
        go.add_css_class("suggested");
        go.set_valign(gtk::Align::Center);
        go.connect_clicked(move |button| {
            button.set_sensitive(false);
            action();
        });
        line.append(&label);
        line.append(&go);
        self.list.append(&line);
        *self.last.borrow_mut() = Last::None;
        self.group.take();
    }

    pub fn notice(&self, text: &str, class: &str) {
        // The same notice again right after itself (a second failed start) says nothing new.
        if let Some(last) = self.list.last_child().and_downcast::<gtk::Label>()
            && last.has_css_class("notice")
            && last.text().replace(['\u{200E}', '\u{200F}'], "") == text
        {
            return;
        }
        let label = text_label(&glib::markup_escape_text(text));
        label.add_css_class("notice");
        label.add_css_class(class);
        self.group.take();
        self.list.append(&label);
        *self.last.borrow_mut() = Last::None;
    }

    // ---- tool calls -----------------------------------------------------------------------

    /// A tool call starts, or changes: `update` is ACP's tool_call or tool_call_update.
    pub fn tool(&self, update: &Value) {
        let Some(id) = update["toolCallId"].as_str() else { return };
        let existing = self.tools.borrow().get(id).cloned();
        let card = existing.unwrap_or_else(|| {
            let card = ToolCard::new();
            // A subagent's step goes under the task that started it, not in the main run.
            let parent = update["_meta"]["claudeCode"]["parentToolUseId"].as_str().and_then(|p| self.tools.borrow().get(p).cloned());
            if let Some(parent) = parent {
                parent.children.append(&card.root);
                parent.children.set_visible(true);
                self.tools.borrow_mut().insert(id.to_owned(), card.clone());
                card.update(update);
                return card;
            }
            *self.last.borrow_mut() = Last::None;
            let group = self.group.borrow().clone();
            let group = group.unwrap_or_else(|| {
                let group = ToolGroup::new();
                self.list.append(&group.root);
                *self.group.borrow_mut() = Some(group.clone());
                group
            });
            group.cards_box.append(&card.root);
            group.cards.borrow_mut().push(card.clone());
            *card.group.borrow_mut() = Rc::downgrade(&group);
            self.tools.borrow_mut().insert(id.to_owned(), card.clone());
            card
        });
        card.update(update);
        let group = card.group.borrow().upgrade();
        if let Some(group) = group {
            group.refresh();
        }
        // A task that ended takes its subagent off the strip; one sent to the background ends
        // its call at once ("launched"), so it stays until the turn ends.
        if matches!(update["status"].as_str(), Some("completed" | "failed"))
            && self.crew.borrow().iter().any(|c| c.key == id)
        {
            let background = card.content.borrow().iter().any(|c| c.to_string().to_lowercase().contains("background"));
            if !background {
                self.crew.borrow_mut().retain(|c| c.key != id);
                self.show_crew();
            }
        }
        // Only live work: a conversation read back replays its subagents too.
        if self.working_since.get().is_some()
            && let Some(parent) = update["_meta"]["claudeCode"]["parentToolUseId"].as_str()
        {
            self.crew_step(parent, update["title"].as_str().map(tool_title));
        }
    }

    /// A subagent did something (`step`: its tool call's title, when it has one). Only for
    /// subagents reported inside the main conversation (no sessions of their own).
    fn crew_step(&self, parent: &str, step: Option<String>) {
        {
            let name = self.tools.borrow().get(parent).map(|c| c.title.label().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| "Subagent".into());
            let mut crew = self.crew.borrow_mut();
            match crew.iter_mut().find(|c| c.key == parent) {
                Some(entry) => {
                    if let Some(step) = step {
                        entry.step = step;
                    }
                    entry.name = name;
                    entry.at = Instant::now();
                }
                None => crew.push(Crew { key: parent.to_owned(), name, step: step.unwrap_or_default(), at: Instant::now(), started: Instant::now(), state: Some("running".into()), ended: None, guessed: true }),
            }
        }
        self.show_crew();
    }

    /// No subagents any more (the agent stopped, failed or started over).
    pub fn crew_clear(&self) {
        self.crew.borrow_mut().clear();
        self.show_crew();
    }

    /// A row of the strip gone (a background command that ended: its card stays).
    pub fn crew_remove(&self, key: &str) {
        self.crew.borrow_mut().retain(|c| c.key != key);
        self.show_crew();
    }

    /// A subagent with a session of its own, as it now stands.
    pub fn crew_set(&self, entry: Crew) {
        {
            let mut crew = self.crew.borrow_mut();
            match crew.iter_mut().find(|c| c.key == entry.key) {
                Some(old) => *old = entry,
                None => crew.push(entry),
            }
        }
        self.show_crew();
    }

    /// Between turns, while subagents Claude left running work on: "✻ Waiting for 2
    /// background agents to finish", where "Working…" was (as Claude Code says it).
    fn background_note(&self) {
        if self.working_since.get().is_some() {
            return;
        }
        let (commands, running): (Vec<bool>, Vec<bool>) = self
            .crew
            .borrow()
            .iter()
            .filter(|c| c.ended.is_none() && c.state.as_deref().unwrap_or("running") == "running" && (!c.guessed || c.at.elapsed().as_secs() < 90))
            .map(|c| c.key.starts_with("task:"))
            .partition(|command| *command);
        let (commands, running) = (commands.len(), running.len());
        let workflow = self.workflow_note.borrow().clone();
        if running == 0 && commands == 0 && workflow.is_none() {
            self.working.set_label("");
            self.working.set_tooltip_text(None);
        } else if running == 0 && commands == 0 {
            self.working.set_markup(&format!("✻ {}", glib::markup_escape_text(&workflow.unwrap_or_default())));
            self.working.set_tooltip_text(Some("Claude goes on when it is done; its card in the conversation shows each agent."));
        } else {
            let mut what = Vec::new();
            if running > 0 {
                what.push(format!("<b>{running}</b> background {}", if running == 1 { "agent" } else { "agents" }));
            }
            if commands > 0 {
                what.push(format!("<b>{commands}</b> {}", if commands == 1 { "command" } else { "commands" }));
            }
            let also = workflow.map(|w| format!(" · {}", glib::markup_escape_text(&w))).unwrap_or_default();
            self.working.set_markup(&format!("✻ Waiting for {} to finish{also}", what.join(" and ")));
            self.working.set_tooltip_text(Some("Claude goes on when they are done; the strip above shows each."));
        }
    }

    /// The subagents strip, like the terminal's agent list: "◌ code-review · Read app.rs ·
    /// 6m 05s", what each is doing and for how long; Stop on a running one.
    fn show_crew(&self) {
        self.background_note();
        while let Some(row) = self.crew_list.row_at_index(0) {
            self.crew_list.remove(&row);
        }
        let crew = self.crew.borrow();
        for entry in crew.iter() {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let quiet = entry.at.elapsed().as_secs();
            let state = entry.state.as_deref().unwrap_or("running");
            // Reported states are known; a guessed one goes quiet after 90s without a step.
            let running = state == "running" && (!entry.guessed || quiet < 90);
            let glyph: gtk::Widget = match state {
                _ if running => gtk::Spinner::builder().spinning(true).build().upcast(),
                "requires_action" => gtk::Label::new(Some("⚑")).upcast(),
                "idle" if entry.ended.is_some() => gtk::Label::new(Some("✓")).upcast(),
                _ => gtk::Label::new(Some("○")).upcast(),
            };
            glyph.add_css_class("crew-glyph");
            glyph.add_css_class(state);
            let who = gtk::Label::builder().label(&entry.name).xalign(0.0).ellipsize(pango::EllipsizeMode::End).max_width_chars(36).build();
            who.add_css_class("crew-name");
            let doing = if state == "requires_action" { "waiting for your answer".to_owned() } else { entry.step.clone() };
            let what = gtk::Label::builder().label(doing).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
            what.add_css_class("dim");
            let time = if entry.guessed {
                if quiet < 5 { "now".to_owned() } else { format!("{} ago", elapsed(quiet)) }
            } else {
                let until = entry.ended.unwrap_or_else(Instant::now);
                elapsed(until.duration_since(entry.started).as_secs())
            };
            let when = gtk::Label::new(Some(&time));
            when.add_css_class("dim");
            when.add_css_class("small");
            line.append(&glyph);
            line.append(&who);
            line.append(&what);
            line.append(&when);
            if !entry.guessed && entry.ended.is_none() {
                let stop = gtk::Button::with_label("Stop");
                stop.add_css_class("flat");
                stop.add_css_class("small");
                stop.set_tooltip_text(Some("Stop this subagent (the main conversation goes on)"));
                let key = entry.key.clone();
                let handler = self.on_crew_stop.borrow().clone();
                stop.connect_clicked(move |button| {
                    button.set_sensitive(false);
                    if let Some(handler) = &handler {
                        handler(key.clone());
                    }
                });
                line.append(&stop);
            }
            let row = gtk::ListBoxRow::builder().child(&line).build();
            row.set_tooltip_text(Some(if entry.guessed { "Go to this subagent's work" } else { "Open this subagent's conversation" }));
            self.crew_list.append(&row);
        }
        self.crew_revealer.set_reveal_child(!crew.is_empty());
    }

    /// Shows `view` (a subagent's conversation) in place of the main one, titled `title`.
    pub fn show_aside(&self, view: &gtk::Widget, title: &str) {
        if let Some(old) = self.aside.first_child().and_then(|bar| bar.next_sibling()) {
            self.aside.remove(&old);
        }
        if let Some(parent) = view.parent().and_downcast::<gtk::Box>() {
            parent.remove(view);
        }
        view.set_vexpand(true);
        self.aside.append(view);
        self.aside_title.set_label(title);
        // Hidden, the conversation loses its place (its scroller shrinks): noted to go back to.
        if self.views.visible_child_name().as_deref() != Some("aside") {
            self.main_place.set(Some((self.scroller.vadjustment().value(), self.stick.get())));
            crate::log::debug!("chat", "subagent view over the main one; its place {:?}", self.main_place.get());
        }
        self.views.set_visible_child_name("aside");
    }

    /// Back to the main conversation; true if a subagent's was shown.
    pub fn show_main(self: &Rc<Self>) -> bool {
        let aside = self.views.visible_child_name().as_deref() == Some("aside");
        self.views.set_visible_child_name("main");
        // Back where it was read: at the end it follows again, else the same place, once laid
        // out (a position set before that is clamped to the shrunk view).
        let place = self.main_place.take();
        crate::log::debug!("chat", "back to the main conversation: place {place:?}");
        if let Some((value, stick)) = place {
            self.stick.set(stick);
            if stick {
                self.follow();
            } else {
                let (weak, frames) = (Rc::downgrade(self), Cell::new(0));
                self.scroller.add_tick_callback(move |_, _| {
                    let Some(chat) = weak.upgrade() else { return glib::ControlFlow::Break };
                    let adj = chat.scroller.vadjustment();
                    // Not as tall as before yet: a frame later (a few at most).
                    frames.set(frames.get() + 1);
                    if adj.upper() - adj.page_size() < value && frames.get() < 10 {
                        return glib::ControlFlow::Continue;
                    }
                    adj.set_value(value);
                    chat.stick.set(false);
                    glib::ControlFlow::Break
                });
            }
        }
        aside
    }

    /// A read-only conversation (a subagent's): no header, no input.
    pub fn viewer() -> Rc<Chat> {
        let chat = Chat::new();
        chat.header.set_visible(false);
        chat.input_column.set_visible(false);
        chat
    }

    /// Puts a button on a tool card (beside "open", "output").
    pub fn tool_button(&self, id: &str, label: &str, tip: &str, action: Rc<dyn Fn()>) {
        let Some(card) = self.tools.borrow().get(id).cloned() else { return };
        let button = gtk::Button::with_label(label);
        button.add_css_class("flat");
        button.add_css_class("small");
        button.set_tooltip_text(Some(tip));
        button.connect_clicked(move |_| action());
        card.actions.append(&button);
    }

    /// Opens a tool call (its group unfolded) and scrolls to it.
    pub fn go_to_tool(self: &Rc<Self>, id: &str) {
        let Some(card) = self.tools.borrow().get(id).cloned() else { return };
        self.open_tool(id);
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            let Some(chat) = weak.upgrade() else { return };
            if let Some(point) = card.root.compute_point(&chat.list, &gtk::graphene::Point::new(0.0, 0.0)) {
                let adj = chat.scroller.vadjustment();
                chat.stick.set(false);
                adj.set_value((f64::from(point.y()) - 24.0).clamp(0.0, (adj.upper() - adj.page_size()).max(0.0)));
            }
        });
    }

    /// Adds a button to a tool card that opens its terminal.
    /// A command's output as it comes (kept with its tool call, not in the tray: the tray is
    /// for jobs Claude means you to watch). An "output" button opens it for a look.
    pub fn tool_output(&self, id: &str, data: &str) {
        let Some(card) = self.tools.borrow().get(id).cloned() else { return };
        {
            let mut output = card.output.borrow_mut();
            output.push_str(data);
            if output.len() > 200_000 {
                let mut cut = output.len() - 150_000;
                while !output.is_char_boundary(cut) {
                    cut += 1;
                }
                output.drain(..cut);
            }
        }
        card.output_button();
        if card.revealer.reveals_child() {
            card.render_soon();
        }
    }

    /// Opens a tool card on its detail (and keeps its run of calls from folding).
    pub fn open_tool(&self, id: &str) {
        let Some(card) = self.tools.borrow().get(id).cloned() else { return };
        if let Some(group) = card.group.borrow().upgrade() {
            group.touched.set(true);
            group.refresh();
        }
        card.render_detail();
        card.revealer.set_reveal_child(true);
    }

    /// The command ended (`code`: its exit code, if known).
    pub fn tool_exit(&self, id: &str, code: Option<i64>) {
        let Some(card) = self.tools.borrow().get(id).cloned() else { return };
        card.exit.set(Some(code));
        let button = card.output_button();
        match code {
            Some(0) | None => button.set_label("output"),
            Some(code) => {
                button.set_label(&format!("output · exit {code}"));
                button.add_css_class("failed");
            }
        }
        if card.revealer.reveals_child() {
            card.render_detail();
        }
    }

    pub fn permission(&self, question: &str, detail: Option<&str>, markdown: bool, options: &[(String, String, String)], choose: Rc<dyn Fn(String)>) -> PermissionCard {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("permission-card");
        let title = text_label(&format!("<b>{}</b>", glib::markup_escape_text(question)));
        root.append(&title);
        // A plan's comments, by part: (what the part says, the comment).
        let comments: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
        let request_changes = gtk::Button::with_label("Request changes");
        if let Some(detail) = detail.filter(|d| !d.is_empty()) {
            if markdown {
                // A plan: read as written, headings and lists and code; each part can take a
                // comment (💬), and Request changes sends them back together.
                let plan = gtk::Box::new(gtk::Orientation::Vertical, 6);
                plan.add_css_class("plan-detail");
                for segment in markdown::segments(detail) {
                    let (widget, quote): (gtk::Widget, String) = match segment {
                        Segment::Text(markup) => {
                            let quote = pango::parse_markup(&markup, '\0').map(|(_, text, _)| text.to_string()).unwrap_or(markup.clone());
                            (text_label(&markup).upcast(), quote)
                        }
                        Segment::Code { code, .. } => (code_label(&code).upcast(), code.clone()),
                        Segment::Table(rows) => (table_view(&rows).upcast(), rows.iter().map(|r| r.join(" | ")).collect::<Vec<_>>().join("\n")),
                        Segment::Rule => (rule_view().upcast(), "---".to_owned()),
                    };
                    let index = comments.borrow().len();
                    comments.borrow_mut().push((quote, String::new()));
                    widget.set_hexpand(true);
                    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    line.add_css_class("plan-part");
                    line.append(&widget);
                    let add = gtk::Button::with_label("💬");
                    add.add_css_class("flat");
                    add.add_css_class("plan-comment-button");
                    add.set_valign(gtk::Align::Start);
                    add.set_tooltip_text(Some("Comment on this part"));
                    line.append(&add);
                    let entry = gtk::Entry::builder().placeholder_text("Your comment on this part (Enter keeps it)").build();
                    entry.add_css_class("plan-comment");
                    let reveal = gtk::Revealer::builder().child(&entry).reveal_child(false).build();
                    let (r, e) = (reveal.clone(), entry.clone());
                    add.connect_clicked(move |_| {
                        r.set_reveal_child(true);
                        e.grab_focus();
                    });
                    let (all, rc) = (comments.clone(), request_changes.clone());
                    entry.connect_changed(move |entry| {
                        all.borrow_mut()[index].1 = entry.text().trim().to_owned();
                        let n = all.borrow().iter().filter(|(_, c)| !c.is_empty()).count();
                        rc.set_label(&if n == 0 { "Request changes".to_owned() } else { format!("Request changes ({n})") });
                    });
                    plan.append(&line);
                    plan.append(&reveal);
                }
                root.append(&plan);
            } else {
                let detail = gtk::Label::builder().label(detail).xalign(0.0).wrap(true).selectable(true).build();
                detail.set_wrap_mode(pango::WrapMode::WordChar);
                detail.add_css_class("code-block");
                root.append(&detail);
            }
        }
        // Options wrap onto more lines rather than widen the chat.
        let buttons = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(6)
            .row_spacing(6)
            .homogeneous(false)
            .max_children_per_line(8)
            .build();
        for (i, (id, name, kind)) in options.iter().enumerate() {
            let button = gtk::Button::with_label(&format!("{}  {name}", i + 1));
            button.add_css_class(if kind.starts_with("allow") { "suggested" } else { "plain" });
            let choose = choose.clone();
            let id = id.clone();
            button.connect_clicked(move |_| choose(id.clone()));
            buttons.append(&button);
        }
        // A plan: declined with the comments, sent as one message quoting what each is about.
        let decline = options.iter().find(|(_, _, kind)| kind.starts_with("reject")).map(|(id, _, _)| id.clone());
        if markdown && let Some(decline) = decline {
            request_changes.add_css_class("plain");
            request_changes.set_tooltip_text(Some("Keep planning, with your comments (💬 on a part adds one)"));
            let submit = self.on_submit.borrow().clone();
            let choose = choose.clone();
            request_changes.connect_clicked(move |_| {
                let notes: Vec<String> = comments
                    .borrow()
                    .iter()
                    .filter(|(_, c)| !c.is_empty())
                    .map(|(quote, comment)| {
                        let quote: String = quote.trim().chars().take(240).collect();
                        format!("> {}\n\n{comment}", quote.replace('\n', "\n> "))
                    })
                    .collect();
                choose(decline.clone());
                if let Some(submit) = &submit
                    && !notes.is_empty()
                {
                    submit((format!("Changes to the plan:\n\n{}", notes.join("\n\n")), Vec::new()));
                }
            });
            buttons.append(&request_changes);
        }
        let controls = gtk::Box::new(gtk::Orientation::Vertical, 6);
        controls.append(&buttons);
        let n = options.len().min(9);
        if n > 0 {
            let mut keys = vec![if n == 1 { "1 or Alt+1".to_owned() } else { format!("1–{n} (Alt+1–{n} while typing)") }];
            if options.iter().any(|(_, _, kind)| kind.starts_with("allow")) {
                keys.push("Alt+Y allow".to_owned());
            }
            if options.iter().any(|(_, _, kind)| kind.starts_with("reject")) {
                keys.push("Alt+N deny".to_owned());
            }
            keys.push("Esc stop".to_owned());
            let hint = gtk::Label::builder().label(keys.join(" · ")).xalign(0.0).build();
            hint.add_css_class("dim");
            hint.add_css_class("small");
            controls.append(&hint);
        }
        root.append(&controls);
        let answer = gtk::Label::builder().xalign(0.0).visible(false).wrap(true).build();
        answer.set_wrap_mode(pango::WrapMode::WordChar);
        answer.add_css_class("dim");
        root.append(&answer);
        self.list.append(&root);
        *self.last.borrow_mut() = Last::None;
        self.group.take();
        PermissionCard { buttons: controls.upcast(), answer }
    }

    /// A form to fill in: `schema` is the request's JSON schema (an object of fields, in the
    /// order they are asked). `answer` gets the filled-in content, or None when skipped.
    pub fn form(&self, message: &str, schema: &Value, answer: Rc<dyn Fn(Option<Value>)>) -> Rc<FormCard> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        root.add_css_class("permission-card");
        root.add_css_class("question-card");
        root.append(&text_label(&format!("<b>{}</b>", glib::markup_escape_text(message))));
        let controls = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let mut fields = Vec::new();
        let properties = schema["properties"].as_object().cloned().unwrap_or_default();
        // Digits pick in the first question with options: its options are numbered.
        let mut numbered = false;
        for (key, field) in &properties {
            let title = field["title"].as_str().unwrap_or(key).to_owned();
            let description = field["description"].as_str().unwrap_or("");
            let choices = choices_with_previews(field);
            let box_ = gtk::Box::new(gtk::Orientation::Vertical, 4);
            // Claude Code's per-question "Other" box sits under its options without a heading.
            let other = choices.is_empty() && title == "Other";
            if !other {
                let heading = if description.is_empty() { title.clone() } else if title == *key { description.to_owned() } else { format!("{title} · {description}") };
                let label = text_label(&glib::markup_escape_text(&heading));
                label.add_css_class("question-heading");
                box_.append(&label);
            }
            if !choices.is_empty() {
                let multi = field["type"].as_str() == Some("array");
                let number_these = !numbered;
                numbered = true;
                let defaults: Vec<Value> = match &field["default"] {
                    Value::Array(all) => all.clone(),
                    Value::Null => Vec::new(),
                    one => vec![one.clone()],
                };
                let mut buttons = Vec::new();
                let mut group: Option<gtk::CheckButton> = None;
                for (i, (value, name, about, preview)) in choices.into_iter().enumerate() {
                    let line = gtk::Box::new(gtk::Orientation::Vertical, 1);
                    let shown = if number_these && i < 9 { format!("{}  {name}", i + 1) } else { name.clone() };
                    line.append(&gtk::Label::builder().label(shown).xalign(0.0).wrap(true).build());
                    if !about.is_empty() {
                        let about = gtk::Label::builder().label(&about).xalign(0.0).wrap(true).build();
                        about.add_css_class("dim");
                        about.add_css_class("small");
                        line.append(&about);
                    }
                    if !preview.is_empty() {
                        let shown = gtk::Label::builder().label(&preview).xalign(0.0).wrap(true).selectable(true).build();
                        shown.set_wrap_mode(pango::WrapMode::Char);
                        shown.add_css_class("code-block");
                        shown.add_css_class("option-preview");
                        line.append(&shown);
                    }
                    let check = gtk::CheckButton::new();
                    check.set_child(Some(&line));
                    if !multi {
                        match &group {
                            Some(first) => check.set_group(Some(first)),
                            None => group = Some(check.clone()),
                        }
                    }
                    check.set_active(defaults.contains(&value));
                    box_.append(&check);
                    buttons.push((check, value, name));
                }
                fields.push(Field::Choice { key: key.clone(), title: title.clone(), multi, buttons });
            } else if field["type"].as_str() == Some("boolean") {
                let check = gtk::CheckButton::with_label(&title);
                check.set_active(field["default"].as_bool().unwrap_or(false));
                box_.append(&check);
                fields.push(Field::Flag { key: key.clone(), title, check });
            } else {
                let kind = match field["type"].as_str() {
                    Some("integer") => Some(true),
                    Some("number") => Some(false),
                    _ => None,
                };
                let entry = gtk::Entry::builder().hexpand(true).build();
                if other {
                    entry.set_placeholder_text(Some("Other: your own answer (optional)"));
                }
                match &field["default"] {
                    Value::String(text) => entry.set_text(text),
                    Value::Number(n) => entry.set_text(&n.to_string()),
                    _ => {}
                }
                box_.append(&entry);
                fields.push(Field::Text { key: key.clone(), title, entry, number: kind });
            }
            controls.append(&box_);
        }
        let problem = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
        problem.add_css_class("form-problem");
        controls.append(&problem);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let send = gtk::Button::with_label("Answer  Alt+Enter");
        send.add_css_class("suggested");
        let skip = gtk::Button::with_label("Skip");
        skip.add_css_class("plain");
        let hint_text = if numbered { "1–9 pick (Alt+1–9 while typing) · Esc stops the turn" } else { "Esc stops the turn" };
        let hint = gtk::Label::builder().label(hint_text).xalign(0.0).hexpand(true).build();
        hint.add_css_class("dim");
        hint.add_css_class("small");
        buttons.append(&send);
        buttons.append(&skip);
        buttons.append(&hint);
        controls.append(&buttons);
        root.append(&controls);
        let answered = gtk::Label::builder().xalign(0.0).visible(false).wrap(true).build();
        answered.set_wrap_mode(pango::WrapMode::WordChar);
        answered.add_css_class("dim");
        root.append(&answered);
        self.list.append(&root);
        *self.last.borrow_mut() = Last::None;
        self.group.take();
        let required = schema["required"].as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_owned)).collect();
        let form = Rc::new(FormCard { card: PermissionCard { buttons: controls.upcast(), answer: answered }, fields, required, problem });
        let (weak, reply) = (Rc::downgrade(&form), answer.clone());
        send.connect_clicked(move |_| {
            if let Some(form) = weak.upgrade()
                && let Some(content) = form.checked()
            {
                reply(Some(content));
            }
        });
        skip.connect_clicked(move |_| answer(None));
        form
    }

    // ---- plan -----------------------------------------------------------------------------

    pub fn set_plan(&self, entries: &Value) {
        while let Some(child) = self.plan.first_child() {
            self.plan.remove(&child);
        }
        let entries = entries.as_array().cloned().unwrap_or_default();
        let open = entries.iter().any(|e| e["status"].as_str() != Some("completed"));
        for entry in &entries {
            let (mark, class) = match entry["status"].as_str() {
                Some("completed") => ("✓", "done"),
                Some("in_progress") => ("▸", "current"),
                _ => ("○", "todo"),
            };
            let label = gtk::Label::builder()
                .label(format!("{mark}  {}", entry["content"].as_str().unwrap_or("")))
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(pango::WrapMode::WordChar)
                .build();
            label.add_css_class(class);
            self.plan.append(&label);
        }
        self.plan_revealer.set_reveal_child(!entries.is_empty() && open);
    }
}

impl FormCard {
    /// What was filled in, as the schema's object, or why it can't be sent (a required
    /// field left empty, a number that isn't one).
    pub fn content(&self) -> Result<Value, String> {
        let mut content = serde_json::Map::new();
        for field in &self.fields {
            match field {
                Field::Choice { key, multi, buttons, .. } => {
                    let picked: Vec<Value> = buttons.iter().filter(|(b, _, _)| b.is_active()).map(|(_, v, _)| v.clone()).collect();
                    if *multi {
                        if !picked.is_empty() {
                            content.insert(key.clone(), Value::Array(picked));
                        }
                    } else if let Some(one) = picked.into_iter().next() {
                        content.insert(key.clone(), one);
                    }
                }
                Field::Text { key, title, entry, number } => {
                    let text = entry.text().trim().to_owned();
                    if text.is_empty() {
                        continue;
                    }
                    let value = match number {
                        Some(true) => Value::from(text.parse::<i64>().map_err(|_| format!("“{title}” needs a whole number."))?),
                        Some(false) => Value::from(text.parse::<f64>().map_err(|_| format!("“{title}” needs a number."))?),
                        None => Value::String(text),
                    };
                    content.insert(key.clone(), value);
                }
                Field::Flag { key, check, .. } => {
                    content.insert(key.clone(), Value::Bool(check.is_active()));
                }
            }
        }
        if let Some(missing) = self.required.iter().find(|k| !content.contains_key(k.as_str())) {
            let title = self.fields.iter().find(|f| f.key() == missing).map_or(missing.as_str(), Field::title);
            return Err(format!("“{title}” needs an answer."));
        }
        Ok(Value::Object(content))
    }

    /// The content if it can be sent; else the card says what is missing.
    pub fn checked(&self) -> Option<Value> {
        match self.content() {
            Ok(content) => {
                self.problem.set_visible(false);
                Some(content)
            }
            Err(problem) => {
                self.problem.set_label(&problem);
                self.problem.set_visible(true);
                None
            }
        }
    }

    /// "Approach: Small steps · Other: keep the API", for the card once answered.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        for field in &self.fields {
            match field {
                Field::Choice { title, buttons, .. } => {
                    let picked: Vec<&str> = buttons.iter().filter(|(b, _, _)| b.is_active()).map(|(_, _, name)| name.as_str()).collect();
                    if !picked.is_empty() {
                        parts.push(format!("{title}: {}", picked.join(", ")));
                    }
                }
                Field::Text { title, entry, .. } if !entry.text().trim().is_empty() => parts.push(format!("{title}: {}", entry.text().trim())),
                Field::Flag { title, check, .. } => parts.push(format!("{title}: {}", if check.is_active() { "yes" } else { "no" })),
                _ => {}
            }
        }
        if parts.is_empty() { "→ (nothing picked)".to_owned() } else { format!("→ {}", parts.join(" · ")) }
    }

    /// Its choices: (question, option names, several allowed, which are picked).
    pub fn choices(&self) -> Vec<(String, Vec<String>, bool, Vec<bool>)> {
        self.fields
            .iter()
            .filter_map(|f| match f {
                Field::Choice { title, multi, buttons, .. } => Some((
                    title.clone(),
                    buttons.iter().map(|(_, _, name)| name.clone()).collect(),
                    *multi,
                    buttons.iter().map(|(b, _, _)| b.is_active()).collect(),
                )),
                _ => None,
            })
            .collect()
    }

    /// Option `option` of choice `field` (counted among the choices): picked, or toggled in a
    /// multiple choice.
    pub fn pick_in(&self, field: usize, option: usize) -> bool {
        let Some(Field::Choice { buttons, multi, .. }) = self.fields.iter().filter(|f| matches!(f, Field::Choice { .. })).nth(field) else {
            return false;
        };
        let Some((button, _, _)) = buttons.get(option) else { return false };
        button.set_active(if *multi { !button.is_active() } else { true });
        true
    }

    /// One text field (more, and a written answer could not say which it is for).
    pub fn writable(&self) -> bool {
        self.fields.iter().filter(|f| matches!(f, Field::Text { .. })).count() == 1
    }

    /// `text` in its text field (false unless it has exactly one).
    pub fn write(&self, text: &str) -> bool {
        if !self.writable() {
            return false;
        }
        let Some(Field::Text { entry, .. }) = self.fields.iter().find(|f| matches!(f, Field::Text { .. })) else { return false };
        entry.set_text(text);
        true
    }

    /// Option `n` (from 0) of the first question with options: picked (a multiple choice:
    /// toggled). Focus stays where it is, so the next digit picks again.
    pub fn pick(&self, n: usize) -> bool {
        let Some(Field::Choice { buttons, multi, .. }) = self.fields.iter().find(|f| matches!(f, Field::Choice { .. })) else { return false };
        let Some((button, _, _)) = buttons.get(n) else { return false };
        button.set_active(if *multi { !button.is_active() } else { true });
        true
    }
}

impl Field {
    fn key(&self) -> &str {
        match self {
            Field::Choice { key, .. } | Field::Text { key, .. } | Field::Flag { key, .. } => key,
        }
    }

    fn title(&self) -> &str {
        match self {
            Field::Choice { title, .. } | Field::Text { title, .. } | Field::Flag { title, .. } => title,
        }
    }
}

/// A field's options as (value, name, description): `oneOf`/`anyOf` of `{const, title,
/// description}`, or a plain `enum` (also inside `items`, for a multiple choice).
#[cfg(test)]
fn choices_of(field: &Value) -> Vec<(Value, String, String)> {
    choices_with_previews(field).into_iter().map(|(v, n, d, _)| (v, n, d)).collect()
}

/// The options with the preview Claude may attach to each (a mock-up, a snippet to compare).
fn choices_with_previews(field: &Value) -> Vec<(Value, String, String, String)> {
    let source = if field["type"].as_str() == Some("array") { &field["items"] } else { field };
    let name_of = |v: &Value| v.as_str().map_or_else(|| v.to_string(), str::to_owned);
    for key in ["oneOf", "anyOf"] {
        if let Some(options) = source[key].as_array() {
            return options
                .iter()
                .filter(|o| !o["const"].is_null())
                .map(|o| {
                    let value = o["const"].clone();
                    let name = o["title"].as_str().map_or_else(|| name_of(&value), str::to_owned);
                    let preview = o["_meta"]["_claude/askUserQuestionOption"]["preview"].as_str().unwrap_or("").to_owned();
                    (value, name, o["description"].as_str().unwrap_or("").to_owned(), preview)
                })
                .collect();
        }
    }
    let names = source["enumNames"].as_array();
    source["enum"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, v)| {
            let name = names.and_then(|n| n.get(i)).and_then(Value::as_str).map_or_else(|| name_of(v), str::to_owned);
            (v.clone(), name, String::new(), String::new())
        })
        .collect()
}

/// A message's `!command` runs (`<bash-input>…</bash-input>` and its output blocks, as
/// Claude Code stores them) taken out: the commands, and the rest of the text.
fn split_shell_runs(text: &str) -> (Vec<String>, String) {
    let mut commands = Vec::new();
    let mut rest = String::new();
    let mut left = text;
    while let Some(start) = left.find("<bash-input>") {
        rest.push_str(&left[..start]);
        let after = &left[start + "<bash-input>".len()..];
        let Some(end) = after.find("</bash-input>") else {
            left = &left[start..];
            break;
        };
        commands.push(after[..end].trim().to_owned());
        left = &after[end + "</bash-input>".len()..];
        // Its output blocks, if they follow.
        for tag in ["bash-stdout", "bash-stderr"] {
            let trimmed = left.trim_start();
            let open = format!("<{tag}>");
            let close = format!("</{tag}>");
            if trimmed.starts_with(&open)
                && let Some(end) = trimmed.find(&close)
            {
                left = &trimmed[end + close.len()..];
            }
        }
    }
    rest.push_str(left);
    (commands, rest)
}

/// The last `lines` lines of terminal output as plain text: colours and cursor codes
/// dropped, a line rewritten in place (progress bars, \r) shown as it ended.
fn plain_tail(output: &str, lines: usize) -> String {
    let mut text = String::with_capacity(output.len());
    let mut chars = output.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI: parameters up to a final byte in @..~
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: up to BEL or ESC \
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' if chars.peek() == Some(&'\n') => {}
            c if c.is_control() && c != '\n' && c != '\r' && c != '\t' => {}
            c => text.push(c),
        }
    }
    let shown: Vec<&str> = text.lines().map(|line| line.rsplit('\r').next().unwrap_or(line)).collect();
    let start = shown.len().saturating_sub(lines);
    shown[start..].join("\n").trim_end().to_owned()
}

impl PermissionCard {
    /// Replaces the buttons with what was answered.
    pub fn resolve(&self, answer: &str) {
        self.buttons.set_visible(false);
        self.answer.set_label(answer);
        self.answer.set_visible(true);
    }
}

impl Message {
    /// Re-renders while text streams in: every 60 ms for short text, less often as it grows
    /// (each render parses the whole message), up to every 600 ms.
    fn render_soon(self: &Rc<Self>) {
        if self.scheduled.replace(true) {
            return;
        }
        let delay = (60 + self.text.borrow().len() as u64 / 40).min(600);
        let message = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            message.scheduled.set(false);
            message.render();
        });
    }

    fn render(&self) {
        while let Some(child) = self.body.first_child() {
            self.body.remove(&child);
        }
        let text = self.text.borrow();
        if self.kind != Kind::Agent {
            // What people type, and thinking, are shown as written; `!command` runs a
            // resumed conversation carries come back as their command lines.
            let (commands, rest) = split_shell_runs(&text);
            for command in commands {
                let line = gtk::Label::builder().label(format!("! {command}")).xalign(0.0).wrap(true).build();
                line.set_wrap_mode(pango::WrapMode::WordChar);
                line.add_css_class("shell-line");
                self.body.append(&line);
            }
            if rest.trim().is_empty() {
                return;
            }
            // Thinking as written; what the user sent as Markdown (code boxes, lines, lists…),
            // their line breaks kept.
            if self.kind == Kind::User {
                for segment in markdown::typed_segments(rest.trim()) {
                    match segment {
                        Segment::Text(markup) => self.body.append(&text_label(&markup)),
                        Segment::Code { code, .. } => self.body.append(&code_label(&code)),
                        Segment::Table(rows) => self.body.append(&table_view(&rows)),
                        Segment::Rule => self.body.append(&rule_view()),
                    }
                }
            } else {
                self.body.append(&text_label(&glib::markup_escape_text(rest.trim())));
            }
            return;
        }
        for segment in markdown::segments(&text) {
            match segment {
                Segment::Text(markup) => self.body.append(&text_label(&markup)),
                Segment::Code { code, .. } => self.body.append(&code_label(&code)),
                Segment::Table(rows) => self.body.append(&table_view(&rows)),
                Segment::Rule => self.body.append(&rule_view()),
            }
        }
    }
}

impl ToolGroup {
    fn new() -> Rc<ToolGroup> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("tool-group");
        // Its label gives way ("…") rather than hold the chat wide.
        let summary_label = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        let summary = gtk::Button::builder().child(&summary_label).visible(false).halign(gtk::Align::Fill).build();
        summary.add_css_class("flat");
        summary.add_css_class("tool-toggle");
        summary.add_css_class("tool-summary");
        let cards_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&summary);
        root.append(&cards_box);
        let group = Rc::new(ToolGroup {
            root,
            summary,
            summary_label,
            cards_box,
            cards: RefCell::default(),
            folded: Cell::new(false),
            open: Cell::new(false),
            touched: Cell::new(false),
        });
        let weak = Rc::downgrade(&group);
        group.summary.connect_clicked(move |_| {
            if let Some(group) = weak.upgrade() {
                group.open.set(!group.open.get());
                group.refresh();
            }
        });
        group
    }

    fn refresh(&self) {
        let cards = self.cards.borrow();
        // Its own flag: is_visible() would also count the folded box around it.
        let shown: Vec<&Rc<ToolCard>> = cards.iter().filter(|c| c.root.get_visible()).collect();
        let edits = shown.iter().any(|c| c.has_diff.get());
        if edits || self.touched.get() {
            self.folded.set(false);
        } else if !self.folded.get() {
            self.folded.set(shown.len() >= 3 && shown.iter().all(|c| c.status.borrow().as_str() == "completed"));
        }
        let folded = self.folded.get();
        self.summary.set_visible(folded);
        self.cards_box.set_visible(!folded || self.open.get());
        let running = shown.iter().any(|c| c.status.borrow().as_str() != "completed");
        let arrow = if self.open.get() { "⌄" } else { "›" };
        let glyph = if running { "○" } else { "✓" };
        // What was done, not how many calls: "Read 3 · Ran 2 · last: app.rs".
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for card in &shown {
            let verb = match card.kind.label().as_str() {
                "read" => "Read",
                "edit" => "Edited",
                "run" => "Ran",
                "search" => "Searched",
                "web" => "Fetched",
                "delete" => "Deleted",
                "move" => "Moved",
                "think" => "Tasks",
                _ => "Other",
            };
            match counts.iter_mut().find(|(v, _)| *v == verb) {
                Some(entry) => entry.1 += 1,
                None => counts.push((verb, 1)),
            }
        }
        let what = counts.iter().map(|(verb, n)| format!("{verb} {n}")).collect::<Vec<_>>().join(" · ");
        let last = shown.last().map(|c| c.title.label().to_string()).unwrap_or_default();
        self.summary_label.set_label(&format!("{glyph}  {what}  ·  last: {last}  {arrow}"));
        self.summary.set_tooltip_text(Some(&format!("{} tool calls: {what}\nlast: {last}", shown.len())));
    }
}

impl ToolCard {
    /// The "output" button (made once): opens the card on the output, or closes it.
    fn output_button(self: &Rc<Self>) -> gtk::Button {
        if let Some(button) = self.output_button.borrow().as_ref() {
            return button.clone();
        }
        let button = gtk::Button::with_label("output");
        button.add_css_class("flat");
        button.add_css_class("small");
        button.add_css_class("output-button");
        button.set_tooltip_text(Some("What the command printed"));
        let weak = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(card) = weak.upgrade() {
                let open = !card.revealer.reveals_child();
                if open {
                    card.render_detail();
                    if let Some(group) = card.group.borrow().upgrade() {
                        group.touched.set(true);
                    }
                }
                card.revealer.set_reveal_child(open);
            }
        });
        self.actions.append(&button);
        *self.output_button.borrow_mut() = Some(button.clone());
        button
    }

    /// Output streaming into an open card: shown again at most every 250 ms.
    fn render_soon(self: &Rc<Self>) {
        if self.rendering.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(250), move || {
            if let Some(card) = weak.upgrade() {
                card.rendering.set(false);
                card.render_detail();
            }
        });
    }

    fn new() -> Rc<ToolCard> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("tool-card");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let glyph = gtk::Label::new(Some("○"));
        glyph.add_css_class("glyph");
        let spinner = gtk::Spinner::new();
        spinner.set_visible(false);
        let kind = gtk::Label::new(None);
        kind.add_css_class("kind");
        let title = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).single_line_mode(true).build();
        title.add_css_class("tool-title");
        let chevron = gtk::Label::new(Some("›"));
        chevron.add_css_class("chevron");
        row.append(&glyph);
        row.append(&spinner);
        row.append(&kind);
        row.append(&title);
        row.append(&chevron);
        let toggle = gtk::Button::builder().child(&row).hexpand(true).build();
        toggle.add_css_class("flat");
        toggle.add_css_class("tool-toggle");
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        actions.set_valign(gtk::Align::Center);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        top.append(&toggle);
        top.append(&actions);
        let detail = gtk::Box::new(gtk::Orientation::Vertical, 6);
        detail.add_css_class("tool-detail");
        let revealer = gtk::Revealer::builder().child(&detail).reveal_child(false).build();
        root.append(&top);
        root.append(&revealer);
        let children = gtk::Box::new(gtk::Orientation::Vertical, 0);
        children.add_css_class("tool-children");
        children.set_visible(false);
        root.append(&children);
        let card = Rc::new(ToolCard {
            children,
            root,
            glyph,
            spinner,
            kind,
            title,
            actions,
            detail,
            revealer,
            content: RefCell::default(),
            output: RefCell::default(),
            exit: Cell::new(None),
            output_button: RefCell::default(),
            rendering: Cell::new(false),
            has_open: Cell::new(false),
            auto_opened: Cell::new(false),
            status: RefCell::default(),
            has_diff: Cell::new(false),
            group: RefCell::default(),
        });
        let weak = Rc::downgrade(&card);
        toggle.connect_clicked(move |_| {
            if let Some(card) = weak.upgrade() {
                if let Some(group) = card.group.borrow().upgrade()
                    && !group.folded.get()
                {
                    group.touched.set(true);
                }
                let open = !card.revealer.reveals_child();
                if open {
                    card.render_detail();
                }
                card.revealer.set_reveal_child(open);
            }
        });
        card
    }

    fn update(&self, update: &Value) {
        if let Some(title) = update["title"].as_str() {
            self.title.set_label(&tool_title(title));
            self.title.set_tooltip_text(Some(title));
            // Claude Code's own plumbing (finding tools, its todo list, shown as the plan)
            // says nothing to the reader.
            let plumbing = matches!(title, "ToolSearch" | "TodoWrite");
            self.root.set_visible(!plumbing);
        }
        if update["status"].as_str() == Some("failed") {
            self.root.set_visible(true);
        }
        if let Some(kind) = update["kind"].as_str() {
            // Its kind as a class: edits, commands and reads each look their own.
            for class in self.root.css_classes() {
                if class.starts_with("kind-") {
                    self.root.remove_css_class(&class);
                }
            }
            self.root.add_css_class(&format!("kind-{kind}"));
            self.kind.set_label(match kind {
                "execute" => "run",
                "fetch" => "web",
                "think" => "think",
                "other" => "",
                other => other,
            });
            self.kind.set_visible(kind != "other");
        }
        if let Some(status) = update["status"].as_str() {
            *self.status.borrow_mut() = status.to_owned();
            let running = status == "in_progress";
            self.spinner.set_visible(running);
            self.spinner.set_spinning(running);
            self.glyph.set_visible(!running);
            for class in ["done", "failed"] {
                self.glyph.remove_css_class(class);
            }
            match status {
                "completed" => {
                    self.glyph.set_label("✓");
                    self.glyph.add_css_class("done");
                }
                "failed" => {
                    self.glyph.set_label("✗");
                    self.glyph.add_css_class("failed");
                }
                _ => self.glyph.set_label("○"),
            }
            // An open card says it is done (its "still running" goes).
            if self.revealer.reveals_child() {
                self.render_detail();
            }
        }
        if let Some(path) = update["locations"].as_array().and_then(|l| l.first()).and_then(|l| l["path"].as_str())
            && !self.has_open.replace(true)
        {
            let line = update["locations"][0]["line"].as_u64();
            let target = match line {
                Some(line) => format!("loom-file:{path}:{line}"),
                None => format!("loom-file:{path}"),
            };
            let open = gtk::Button::with_label("open");
            open.add_css_class("flat");
            open.add_css_class("small");
            open.set_tooltip_text(Some(&format!("Open {path} in loom's editor")));
            open.connect_clicked(move |button| {
                let handler = OPEN_LINK.with(|h| h.borrow().clone());
                if let Some(handler) = handler {
                    handler(&target, button.upcast_ref());
                }
            });
            self.actions.prepend(&open);
        }
        if let Some(content) = update["content"].as_array() {
            *self.content.borrow_mut() = content.clone();
            self.has_diff.set(content.iter().any(|c| c["type"] == "diff"));
            let small_diff = content.iter().any(|c| c["type"] == "diff")
                && content
                    .iter()
                    .filter(|c| c["type"] == "diff")
                    .map(|c| changed_lines(c["oldText"].as_str().unwrap_or(""), c["newText"].as_str().unwrap_or("")).len())
                    .sum::<usize>()
                    <= 40;
            if small_diff && !self.auto_opened.replace(true) {
                self.revealer.set_reveal_child(true);
            }
            if self.revealer.reveals_child() {
                self.render_detail();
            }
        }
    }

    fn render_detail(&self) {
        while let Some(child) = self.detail.first_child() {
            self.detail.remove(&child);
        }
        for item in self.content.borrow().iter() {
            match item["type"].as_str() {
                Some("diff") => self.detail.append(&diff_view(item)),
                Some("content") => {
                    if let Some(text) = item["content"]["text"].as_str() {
                        for segment in markdown::segments(text) {
                            match segment {
                                Segment::Text(markup) => self.detail.append(&text_label(&markup)),
                                Segment::Code { code, .. } => self.detail.append(&code_label(&code)),
                                Segment::Table(rows) => self.detail.append(&table_view(&rows)),
                                Segment::Rule => self.detail.append(&rule_view()),
                            }
                        }
                    }
                }
                Some("terminal") => {
                    let output = self.output.borrow();
                    let tail = plain_tail(&output, 60);
                    if tail.is_empty() {
                        let none = text_label(if self.exit.get().is_some() { "It printed nothing." } else { "No output yet." });
                        none.add_css_class("dim");
                        self.detail.append(&none);
                    } else {
                        let view = code_label(&tail);
                        view.add_css_class("command-output");
                        self.detail.append(&view);
                    }
                }
                _ => {}
            }
        }
        if self.detail.first_child().is_none() {
            // Still at it (a wait on a job, a long fetch): not "nothing" yet.
            let running = matches!(self.status.borrow().as_str(), "pending" | "in_progress");
            let none = text_label(if running { "Still running; what it returns shows here." } else { "It returned nothing to show." });
            none.add_css_class("dim");
            self.detail.append(&none);
        }
    }
}

/// A row of the subagents strip.
pub struct Crew {
    /// The subagent's session id, or (reported inside the main conversation) its task's tool call.
    pub key: String,
    pub name: String,
    /// What it did last.
    pub step: String,
    /// When it last did something.
    pub at: Instant,
    pub started: Instant,
    /// "running", "requires_action", "idle".
    pub state: Option<String>,
    pub ended: Option<Instant>,
    /// No session of its own: its state is a guess from its steps.
    pub guessed: bool,
}

/// A tool's title for people: no Markdown ticks, and "mcp__server__tool" as "server · tool".
/// One line: a multi-line command (a heredoc) shows its first line and "…"; the rest
/// would spill over the card around it.
pub(crate) fn tool_title(title: &str) -> String {
    let title = title.replace('`', "");
    let title = match title.trim().split_once('\n') {
        Some((first, _)) => format!("{} …", first.trim_end()),
        None => title.trim().to_owned(),
    };
    if let Some(rest) = title.strip_prefix("mcp__")
        && let Some((server, tool)) = rest.split_once("__")
    {
        let tool = tool.strip_prefix(&format!("{server}_")).unwrap_or(tool);
        return format!("{server} · {}", tool.replace('_', " "));
    }
    title
}

/// "42s", "3m 05s", "1h 02m".
pub fn elapsed(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// 1–9 for those keys.
fn digit(key: gdk::Key) -> Option<usize> {
    key.to_unicode().and_then(any_digit).filter(|d| *d >= 1).map(|d| d as usize)
}

/// The value of an ASCII, Persian (۱) or Arabic-Indic (١) digit.
pub fn any_digit(c: char) -> Option<u32> {
    match c as u32 {
        0x06F0..=0x06F9 => Some(c as u32 - 0x06F0),
        0x0660..=0x0669 => Some(c as u32 - 0x0660),
        _ => c.to_digit(10),
    }
}

/// A picture at most `max_w` × `max_h`, scaled down once. A full-size picture would ask for its
/// own size: the input would grow to hold it, and the chat would be laid out taller than shown.
fn thumbnail(png: &glib::Bytes, max_w: i32, max_h: i32) -> Option<gtk::Picture> {
    let full = gdk_pixbuf::Pixbuf::from_read(std::io::Cursor::new(png.to_vec())).ok()?;
    let (w, h) = (full.width().max(1), full.height().max(1));
    let scale = (f64::from(max_w) / f64::from(w)).min(f64::from(max_h) / f64::from(h)).min(1.0);
    let (width, height) = (((f64::from(w) * scale) as i32).max(1), ((f64::from(h) * scale) as i32).max(1));
    let small = full.scale_simple(width, height, gdk_pixbuf::InterpType::Bilinear)?;
    #[allow(deprecated)]
    let texture = gdk::Texture::for_pixbuf(&small);
    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_can_shrink(false);
    picture.set_size_request(width, height);
    Some(picture)
}

/// The Claude account's limits from `ai-usagebar` (its entry for Anthropic), if installed.
fn account_usage() -> Option<serde_json::Value> {
    let output = std::process::Command::new("ai-usagebar").args(["usage", "--json"]).output().ok()?;
    let all: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    all["entries"].as_array()?.iter().find(|e| e["id"] == "anthropic").cloned()
}

fn usage_heading(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text.to_uppercase()).xalign(0.0).build();
    label.add_css_class("usage-heading");
    label
}

/// What fills the context: a name and its tokens a line.
fn usage_parts(parts: &[(String, u64)], k: &dyn Fn(u64) -> String) -> gtk::Grid {
    let grid = gtk::Grid::builder().column_spacing(16).row_spacing(2).build();
    grid.add_css_class("context-parts");
    for (row, (name, tokens)) in parts.iter().enumerate() {
        let name = gtk::Label::builder().label(name).xalign(0.0).hexpand(true).build();
        let amount = gtk::Label::builder().label(k(*tokens)).xalign(1.0).build();
        for label in [&name, &amount] {
            label.add_css_class("dim");
            label.add_css_class("small");
        }
        amount.add_css_class("numeric");
        grid.attach(&name, 0, row as i32, 1, 1);
        grid.attach(&amount, 1, row as i32, 1, 1);
    }
    grid
}

fn usage_note(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).wrap(true).max_width_chars(44).build();
    label.add_css_class("dim");
    label.add_css_class("small");
    label
}

/// "Weekly (7d)   ▬▬▬▭▭  57%" over "Resets in 1d 5h".
fn usage_row(name: &str, share: f64, value: &str, detail: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let label = gtk::Label::builder().label(name).xalign(0.0).width_chars(14).build();
    let bar = gtk::LevelBar::builder().min_value(0.0).max_value(1.0).value(share.clamp(0.0, 1.0)).hexpand(true).valign(gtk::Align::Center).build();
    bar.set_mode(gtk::LevelBarMode::Continuous);
    for name in ["low", "high", "full"] {
        bar.remove_offset_value(Some(name));
    }
    bar.add_css_class("context-bar");
    if share > 0.8 {
        bar.add_css_class("full");
    }
    let amount = gtk::Label::builder().label(value).xalign(1.0).width_chars(5).build();
    amount.add_css_class("numeric");
    line.append(&label);
    line.append(&bar);
    line.append(&amount);
    row.append(&line);
    if !detail.is_empty() {
        row.append(&usage_note(detail));
    }
    row
}

/// A Markdown table: a header row in small caps, hairlines between rows.
pub(crate) fn table_view(rows: &[Vec<String>]) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.add_css_class("md-table");
    for (r, row) in rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let label = gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).use_markup(true).build();
            label.set_markup(cell);
            label.set_wrap_mode(pango::WrapMode::WordChar);
            label.set_focusable(false);
            label.add_css_class(if r == 0 { "table-head" } else { "table-cell" });
            label.set_hexpand(true);
            grid.attach(&label, c as i32, r as i32, 1, 1);
        }
    }
    // Cells wrap, so a wide table narrows rather than widening the chat.
    grid
}

/// A wrapping, selectable label for Pango markup.
pub(crate) fn text_label(markup: &str) -> gtk::Label {
    let label = gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).use_markup(true).build();
    label.set_wrap_mode(pango::WrapMode::WordChar);
    label.set_markup(&one_direction(markup));
    label.set_focusable(false);
    label.connect_activate_link(link_clicked);
    label
}

/// Every line of a block in one direction: right-to-left when most of its letters are (a
/// Persian message), else left-to-right like the input (an English reply quoting a Persian
/// word). Left alone, each line takes the direction of its own first letter (Pango's default)
/// and a mixed block zigzags between the edges.
pub(crate) fn one_direction(markup: &str) -> String {
    // Only a block with right-to-left letters in it needs them.
    if !markup.chars().any(is_rtl) {
        return markup.to_owned();
    }
    let Some(rtl) = mostly_rtl(markup) else { return markup.to_owned() };
    let mark = if rtl { '\u{200F}' } else { '\u{200E}' };
    let mut out = String::with_capacity(markup.len() + 16);
    out.push(mark);
    for c in markup.chars() {
        out.push(c);
        if c == '\n' {
            out.push(mark);
        }
    }
    out
}

fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Whether most letters are right-to-left; None: no letter. Only words count: not markup,
/// not inline code, and not paths, URLs or identifiers (a Persian sentence naming a file is
/// still Persian).
fn mostly_rtl(markup: &str) -> Option<bool> {
    // The text outside tags, without what code tags (<tt>, monospace spans) hold.
    let mut text = String::new();
    let mut open: Vec<bool> = Vec::new();
    let mut rest = markup;
    while let Some(start) = rest.find('<') {
        if !open.contains(&true) {
            text.push_str(&rest[..start]);
        }
        let Some(end) = rest[start..].find('>') else { break };
        let tag = &rest[start + 1..start + end];
        if tag.starts_with('/') {
            open.pop();
        } else if !tag.ends_with('/') {
            open.push(tag.starts_with("tt") || tag.contains("font_family"));
        }
        rest = &rest[start + end + 1..];
    }
    if !open.contains(&true) {
        text.push_str(rest);
    }
    let (mut rtl, mut ltr) = (0usize, 0usize);
    for word in text.split_whitespace() {
        if word.contains(['/', '\\', '_', '=', '@']) || word.contains("::") || word.contains("&amp;") {
            continue;
        }
        for c in word.chars().filter(|c| c.is_alphabetic()) {
            if is_rtl(c) {
                rtl += 1;
            } else {
                ltr += 1;
            }
        }
    }
    (rtl + ltr > 0).then_some(rtl > ltr)
}

/// A code block: monospace, selectable, with a copy button in its corner.
pub(crate) fn code_label(code: &str) -> gtk::Overlay {
    let label = gtk::Label::builder().label(code).xalign(0.0).wrap(true).selectable(true).build();
    label.set_wrap_mode(pango::WrapMode::Char);
    label.add_css_class("code-block");
    label.set_focusable(false);
    let copy = gtk::Button::with_label("copy");
    copy.add_css_class("copy");
    copy.add_css_class("flat");
    copy.set_halign(gtk::Align::End);
    copy.set_valign(gtk::Align::Start);
    copy.set_tooltip_text(Some("Copy to the clipboard"));
    let text = code.to_owned();
    copy.connect_clicked(move |button| {
        button.clipboard().set_text(&text);
        button.set_label("copied");
        let button = button.clone();
        glib::timeout_add_local_once(Duration::from_millis(1200), move || button.set_label("copy"));
    });
    let overlay = gtk::Overlay::builder().child(&label).build();
    overlay.add_css_class("code-wrap");
    overlay.add_overlay(&copy);
    overlay
}

/// A line across, for `---`.
pub fn rule_view() -> gtk::Separator {
    let rule = gtk::Separator::new(gtk::Orientation::Horizontal);
    rule.add_css_class("md-rule");
    rule
}

/// The input's tags for Markdown as typed: code (inline and blocks), the marks themselves
/// (dim), bold, italics, headings, rules and quotes.
fn markdown_tags(view: &gtk::TextView) {
    let fg = view.color();
    let tint = |alpha: f32| gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), alpha);
    let table = view.buffer().tag_table();
    // Again (the theme changed): the tints only.
    if let Some(code) = table.lookup("md-code") {
        code.set_background_rgba(Some(&tint(0.09)));
        for (name, alpha) in [("md-mark", 0.4), ("md-quote", 0.7)] {
            if let Some(tag) = table.lookup(name) {
                tag.set_foreground_rgba(Some(&tint(alpha)));
            }
        }
        if let Some(block) = table.lookup("md-block") {
            block.set_paragraph_background_rgba(Some(&tint(0.07)));
        }
        return;
    }
    let add = |tag: gtk::TextTag| {
        table.add(&tag);
    };
    add(gtk::TextTag::builder().name("md-code").family("JetBrains Mono").background_rgba(&tint(0.09)).build());
    add(gtk::TextTag::builder().name("md-block").family("JetBrains Mono").paragraph_background_rgba(&tint(0.07)).build());
    add(gtk::TextTag::builder().name("md-mark").foreground_rgba(&tint(0.4)).build());
    add(gtk::TextTag::builder().name("md-bold").weight(700).build());
    add(gtk::TextTag::builder().name("md-italic").style(pango::Style::Italic).build());
    add(gtk::TextTag::builder().name("md-heading").weight(700).scale(1.15).build());
    add(gtk::TextTag::builder().name("md-quote").style(pango::Style::Italic).foreground_rgba(&tint(0.7)).build());
}

/// Marks what is typed as Markdown: fenced blocks, `code`, **bold**, *italics*, # headings,
/// --- rules and > quotes (the marks dim, as in an editor that knows it).
fn mark_markdown(buffer: &gtk::TextBuffer) {
    let (start, end) = buffer.bounds();
    for name in ["md-code", "md-block", "md-mark", "md-bold", "md-italic", "md-heading", "md-quote"] {
        buffer.remove_tag_by_name(name, &start, &end);
    }
    // Something pasted that long is not prose to mark (and would make typing slow).
    if buffer.char_count() > 20_000 {
        return;
    }
    let text = buffer.text(&start, &end, false).to_string();
    let apply = |name: &str, from: usize, to: usize| {
        if to > from {
            buffer.apply_tag_by_name(name, &buffer.iter_at_offset(from as i32), &buffer.iter_at_offset(to as i32));
        }
    };
    let mut at = 0usize;
    let mut fenced = false;
    for line in text.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        let n = chars.len();
        let lead = chars.iter().take_while(|c| c.is_whitespace()).count();
        let rest: String = chars[lead..].iter().collect();
        if rest.starts_with("```") {
            apply("md-block", at, at + n);
            apply("md-mark", at, at + n);
            fenced = !fenced;
        } else if fenced {
            apply("md-block", at, at + n);
        } else if rest.starts_with('#') && rest.trim_start_matches('#').starts_with(' ') {
            let hashes = rest.chars().take_while(|c| *c == '#').count();
            apply("md-mark", at + lead, at + lead + hashes);
            apply("md-heading", at + lead + hashes, at + n);
        } else if rest.len() >= 3 && (rest.chars().all(|c| c == '-') || rest.chars().all(|c| c == '*') || rest.chars().all(|c| c == '_')) {
            apply("md-mark", at, at + n);
        } else if rest.starts_with('>') {
            apply("md-mark", at + lead, at + lead + 1);
            apply("md-quote", at + lead + 1, at + n);
        } else {
            // Inline: `code` first (nothing inside it counts), then **bold**, then *italics*.
            let mut code = vec![false; n];
            let mut i = 0;
            while i < n {
                if chars[i] == '`'
                    && let Some(close) = (i + 1..n).find(|j| chars[*j] == '`')
                {
                    apply("md-mark", at + i, at + i + 1);
                    apply("md-code", at + i + 1, at + close);
                    apply("md-mark", at + close, at + close + 1);
                    code[i..=close].iter_mut().for_each(|c| *c = true);
                    i = close + 1;
                    continue;
                }
                i += 1;
            }
            let mut i = 0;
            while i + 1 < n {
                if !code[i] && chars[i] == '*' && chars[i + 1] == '*'
                    && let Some(close) = (i + 2..n.saturating_sub(1)).find(|j| !code[*j] && chars[*j] == '*' && chars[*j + 1] == '*')
                    && close > i + 2
                {
                    apply("md-mark", at + i, at + i + 2);
                    apply("md-bold", at + i + 2, at + close);
                    apply("md-mark", at + close, at + close + 2);
                    i = close + 2;
                    continue;
                }
                i += 1;
            }
            let mut i = 0;
            while i < n {
                let lone = |j: usize| chars[j] == '*' && (j == 0 || chars[j - 1] != '*') && (j + 1 >= n || chars[j + 1] != '*');
                if !code[i] && lone(i) && i + 1 < n && !chars[i + 1].is_whitespace()
                    && let Some(close) = (i + 2..n).find(|j| !code[*j] && lone(*j) && !chars[*j - 1].is_whitespace())
                {
                    apply("md-mark", at + i, at + i + 1);
                    apply("md-italic", at + i + 1, at + close);
                    apply("md-mark", at + close, at + close + 1);
                    i = close + 1;
                    continue;
                }
                i += 1;
            }
        }
        at += n + 1;
    }
}

/// Whether a setting is at its default: "default", "off", or a choice named "Default…".
fn at_default(values: &[(String, String, String)], current: &str) -> bool {
    let name = values.iter().find(|(v, _, _)| v == current).map_or("", |(_, n, _)| n.as_str());
    matches!(current.to_lowercase().as_str(), "default" | "off" | "false" | "") || name.starts_with("Default")
}

/// The values of a select option, groups flattened: (value, name, description).
/// The current value, short: "Default (recommended)" says what it stands for ("Opus 5.5"),
/// anything longer than a chip holds ends in "…".
fn chip_value(values: &[(String, String, String)], current: &str) -> String {
    let Some((_, name, description)) = values.iter().find(|(v, _, _)| v == current) else { return current.to_owned() };
    let name = if name.starts_with("Default") && !description.is_empty() && description.chars().count() <= 24 {
        description.clone()
    } else {
        name.split(" (").next().unwrap_or(name).to_owned()
    };
    if name.chars().count() > 18 { format!("{}…", name.chars().take(17).collect::<String>()) } else { name }
}

fn flat_values(options: &Value) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for entry in options.as_array().into_iter().flatten() {
        if let Some(group) = entry["options"].as_array() {
            out.extend(flat_values(&Value::Array(group.clone())));
        } else if let Some(value) = entry["value"].as_str() {
            out.push((
                value.to_owned(),
                entry["name"].as_str().unwrap_or(value).to_owned(),
                entry["description"].as_str().unwrap_or("").to_owned(),
            ));
        }
    }
    out
}

/// The lines between what the old and new text share at either end, as removed and added.
fn diff_view(item: &Value) -> gtk::Box {
    let view = gtk::Box::new(gtk::Orientation::Vertical, 0);
    view.add_css_class("diff");
    // A long path must not hold the chat wide: it gives way from the left.
    let full = item["path"].as_str().unwrap_or("");
    let path = gtk::Label::builder().label(full).xalign(0.0).ellipsize(pango::EllipsizeMode::Start).tooltip_text(full).build();
    path.add_css_class("mono");
    path.add_css_class("dim");
    view.append(&path);
    let old = item["oldText"].as_str().unwrap_or("");
    let new = item["newText"].as_str().unwrap_or("");
    for (sign, line) in changed_lines(old, new).into_iter().take(300) {
        let label = gtk::Label::builder().label(format!("{sign} {line}")).xalign(0.0).wrap(true).selectable(true).build();
        label.set_wrap_mode(pango::WrapMode::Char);
        label.add_css_class("mono");
        label.add_css_class(if sign == '+' { "diff-add" } else { "diff-del" });
        view.append(&label);
    }
    view
}

pub fn changed_lines<'a>(old: &'a str, new: &'a str) -> Vec<(char, &'a str)> {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    let removed = old[prefix..old.len() - suffix].iter().map(|l| ('-', *l));
    let added = new[prefix..new.len() - suffix].iter().map(|l| ('+', *l));
    removed.chain(added).collect()
}

#[cfg(test)]
mod tests {
    use super::{changed_lines, chip_value, choices_of, plain_tail, split_shell_runs};

    #[test]
    fn shell_runs_read_as_commands() {
        let text = "<bash-input>ls</bash-input>\n<bash-stdout>a\nb</bash-stdout><bash-stderr></bash-stderr>What did it print?";
        let (commands, rest) = split_shell_runs(text);
        assert_eq!(commands, vec!["ls".to_owned()]);
        assert_eq!(rest, "What did it print?");
        assert_eq!(split_shell_runs("plain"), (Vec::new(), "plain".to_owned()));
    }

    #[test]
    fn a_block_keeps_one_direction() {
        assert_eq!(super::one_direction("<b>which</b> is\nقیمت"), "\u{200E}<b>which</b> is\n\u{200E}قیمت");
        // An English reply quoting Persian stays left-to-right; Persian stays right-to-left.
        assert_eq!(super::one_direction("&laquo;قیمت&raquo; is the price of it"), "\u{200E}&laquo;قیمت&raquo; is the price of it");
        assert_eq!(super::one_direction("قیمت اخرین خرید for"), "\u{200F}قیمت اخرین خرید for");
        // A path in a Persian sentence does not make it English.
        assert!(super::one_direction("این فایل را ببین: /home/me/notes/todo.md").starts_with('\u{200F}'));
        assert!(super::one_direction("فایل <tt>src/app.rs</tt> و <tt>chat.rs</tt> را ببین").starts_with('\u{200F}'));
        assert_eq!(super::one_direction("123"), "123");
        assert_eq!(super::one_direction("cargo test\nnext"), "cargo test\nnext");
    }

    #[test]
    fn output_reads_as_plain_text() {
        let raw = "\u{1b}[32mok\u{1b}[0m\r\nstep 1\rstep 2\r\n\u{1b}]0;title\u{7}done\r\n";
        assert_eq!(plain_tail(raw, 10), "ok\nstep 2\ndone");
        assert_eq!(plain_tail("a\nb\nc\n", 2), "b\nc");
    }

    #[test]
    fn only_the_changed_middle_is_shown() {
        let d = changed_lines("a\nb\nc\nd", "a\nB\nc\nd");
        assert_eq!(d, vec![('-', "b"), ('+', "B")]);
    }

    #[test]
    fn mcp_tools_read_as_server_and_tool() {
        assert_eq!(super::tool_title("mcp__loom__loom_run"), "loom · run");
        assert_eq!(super::tool_title("mcp__github__create_issue"), "github · create issue");
        assert_eq!(super::tool_title("`ls -la`"), "ls -la");
        assert_eq!(super::tool_title("python3 - <<EOF\nprint(1)\nEOF"), "python3 - <<EOF …");
    }

    #[test]
    fn choices_come_from_one_of_and_enum() {
        let single = serde_json::json!({ "type": "string", "oneOf": [{ "const": "a", "title": "Apple", "description": "red" }] });
        assert_eq!(choices_of(&single), vec![("a".into(), "Apple".into(), "red".into())]);
        let multi = serde_json::json!({ "type": "array", "items": { "anyOf": [{ "const": "x", "title": "X" }] } });
        assert_eq!(choices_of(&multi), vec![("x".into(), "X".into(), String::new())]);
        let plain = serde_json::json!({ "type": "string", "enum": ["s", "m"], "enumNames": ["Small", "Medium"] });
        assert_eq!(choices_of(&plain)[1], ("m".into(), "Medium".into(), String::new()));
        let numbers = serde_json::json!({ "type": "integer", "enum": [1, 2] });
        assert_eq!(choices_of(&numbers)[1], (serde_json::json!(2), "2".into(), String::new()));
    }

    #[test]
    fn chips_say_what_default_means() {
        let model = vec![("default".into(), "Default (recommended)".into(), "Opus 5.5".into()), ("opus".into(), "Opus 5.5".into(), "For complex work".into())];
        assert_eq!(chip_value(&model, "default"), "Opus 5.5");
        let effort = vec![("default".into(), "Default".into(), String::new())];
        assert_eq!(chip_value(&effort, "default"), "Default");
        let long = vec![("x".into(), "A really very long agent name".into(), String::new())];
        assert_eq!(chip_value(&long, "x"), "A really very lon…");
    }

    #[test]
    fn persian_digits_count() {
        assert_eq!(super::any_digit('۱'), Some(1));
        assert_eq!(super::any_digit('٣'), Some(3));
        assert_eq!(super::any_digit('7'), Some(7));
        assert_eq!(super::any_digit('a'), None);
    }

    #[test]
    fn a_new_file_is_all_added() {
        assert_eq!(changed_lines("", "x\ny"), vec![('+', "x"), ('+', "y")]);
    }
}

/// How the summary of a compacted conversation starts (Claude Code's words).
const SUMMARY_START: &str = "This session is being continued from a previous conversation";

/// A message's right-click menu: what is selected in it, its own buttons' actions, and a quote
/// of it (or of the selection) into the input. Over the text's own menu (Capture); the Menu
/// key or Shift+F10 too.
fn message_menu(wrap: &gtk::Overlay, message: &Rc<Message>, input: &gtk::TextView, actions: Vec<(&'static str, gtk::Button)>) {
    let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).propagation_phase(gtk::PropagationPhase::Capture).build();
    let (at, message, input) = (wrap.downgrade(), Rc::downgrade(message), input.downgrade());
    click.connect_pressed(|gesture, _, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
    });
    let open = Rc::new(move |x: f64, y: f64| {
        let (Some(wrap), Some(message), Some(input)) = (at.upgrade(), message.upgrade(), input.upgrade()) else { return };
        let selected = crate::files::selected_text(message.body.upcast_ref()).filter(|t| !t.trim().is_empty());
        let (copied, there) = (selected.clone(), wrap.clone());
        let mut menu = crate::menu::Menu::new()
            .item("Copy", move || {
                if let Some(text) = &copied {
                    crate::menu::copy(&there, text);
                }
            })
            .hint("Ctrl+C")
            .enabled(selected.is_some());
        for (label, button) in &actions {
            let button = button.clone();
            menu = menu.item(label, move || button.emit_clicked());
        }
        let quoted = selected.unwrap_or_else(|| message.text.borrow().trim().to_owned());
        menu.section()
            .item("Quote in your reply", move || {
                let buffer = input.buffer();
                let quote = format!("> {}\n\n", quoted.trim().replace('\n', "\n> "));
                buffer.place_cursor(&buffer.end_iter());
                if buffer.char_count() > 0 {
                    buffer.insert_at_cursor("\n");
                }
                buffer.insert_at_cursor(&quote);
                input.grab_focus();
            })
            .popup(&wrap, x, y);
    });
    let opened = open.clone();
    click.connect_pressed(move |_, _, x, y| opened(x, y));
    wrap.add_controller(click);
    crate::menu::on_menu_key(wrap, move |x, y| open(x, y));
}
