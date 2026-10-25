//! One agent's conversation: a header, the messages and tool calls, the plan, and the input.
//!
//! Widgets only; what happens on submit, stop or a permission answer is the agent's business
//! (see agent.rs), passed in as callbacks.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::{gdk, gio, glib, pango, prelude::*};
use serde_json::Value;

use crate::markdown::{self, Segment};

type Callback<T> = RefCell<Option<Rc<dyn Fn(T)>>>;

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
    /// Pickers for the session's settings (model, effort…).
    options: gtk::Box,
    show_thinking: Cell<bool>,
    placeholder: gtk::Label,
    send: gtk::Button,
    working: gtk::Label,
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
    /// Where the next tool call goes, until something else is said.
    group: RefCell<Option<Rc<ToolGroup>>>,
    stick: Rc<Cell<bool>>,
    commands: RefCell<Vec<(String, String)>>,
    completion: gtk::Popover,
    completion_list: gtk::ListBox,
    /// What was typed, and the images attached (PNG).
    pub on_submit: Callback<(String, Vec<glib::Bytes>)>,
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
}

struct ToolCard {
    root: gtk::Box,
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
        let state = gtk::Label::new(None);
        state.add_css_class("dim");
        let mode = gtk::Button::with_label("Mode");
        mode.add_css_class("mode");
        mode.add_css_class("flat");
        mode.set_tooltip_text(Some("Permission mode (Shift+Tab in the input)"));
        mode.set_valign(gtk::Align::Center);
        let usage = gtk::Label::new(None);
        usage.add_css_class("dim");
        usage.add_css_class("small");
        usage.set_tooltip_text(Some("Context window in use"));
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
        header.append(&state);
        header.append(&context);
        header.append(&usage);
        root.append(&header);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
        list.add_css_class("messages");
        let starting = gtk::Label::builder().visible(false).halign(gtk::Align::Center).build();
        starting.add_css_class("starting-note");
        list.append(&starting);
        let clamp = gtk::Box::new(gtk::Orientation::Vertical, 0);
        clamp.append(&list);
        clamp.set_layout_manager(Some(crate::column::ColumnLayout::new(880)));
        let scroller = gtk::ScrolledWindow::builder()
            .child(&clamp)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        // "Jump to latest", while the reader is scrolled up.
        let latest = gtk::Button::with_label("↓  Latest");
        latest.add_css_class("latest");
        latest.set_halign(gtk::Align::Center);
        latest.set_valign(gtk::Align::End);
        latest.set_margin_bottom(14);
        latest.set_visible(false);
        let scroll_area = gtk::Overlay::builder().child(&scroller).vexpand(true).build();
        scroll_area.add_overlay(&latest);
        root.append(&scroll_area);

        let plan = gtk::Box::new(gtk::Orientation::Vertical, 2);
        plan.add_css_class("plan");
        let plan_revealer = gtk::Revealer::builder().child(&plan).reveal_child(false).build();
        root.append(&plan_revealer);

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
        let working = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).width_chars(4).build();
        working.add_css_class("working");
        let send = gtk::Button::with_label("↑");
        send.add_css_class("send");
        send.set_tooltip_text(Some("Send (Enter)"));
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
        toolbar.append(&working);
        toolbar.append(&send);

        let attach_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        attach_box.add_css_class("attachments");
        attach_box.set_visible(false);
        let composer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        composer.add_css_class("composer");
        composer.append(&attach_box);
        composer.append(&field);
        composer.append(&toolbar);
        let input_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        input_box.add_css_class("input-box");
        input_box.append(&composer);
        let input_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        input_column.append(&input_box);
        input_column.set_layout_manager(Some(crate::column::ColumnLayout::new(920)));
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
            options,
            show_thinking: Cell::new(true),
            placeholder,
            send: send.clone(),
            working,
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
            stick: Rc::new(Cell::new(true)),
            commands: RefCell::default(),
            completion,
            completion_list,
            on_submit: RefCell::default(),
            attachments: RefCell::default(),
            attach_box,
            on_cancel: RefCell::default(),
            on_cycle_mode: RefCell::default(),
            on_option: RefCell::default(),
            on_answer: RefCell::default(),
        });
        chat.wire();
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
        self.list.clipboard().set_text(&text);
        true
    }

    /// To the bottom, and following it again.
    pub fn scroll_to_latest(&self) {
        self.stick.set(true);
        let adj = self.scroller.vadjustment();
        adj.set_value(adj.upper() - adj.page_size());
        self.latest.set_visible(false);
    }

    fn wire(self: &Rc<Self>) {
        // Follow the bottom while it is in view; leave the reader alone once they scroll up.
        // Only a move up lets go of the bottom: content growing (which can report a value
        // short of the new end before the end is followed) must not.
        let adjustment = self.scroller.vadjustment();
        let stick = self.stick.clone();
        let latest = self.latest.clone();
        let last = Rc::new(Cell::new(0.0));
        adjustment.connect_value_changed(move |adj| {
            let at_bottom = adj.value() >= adj.upper() - adj.page_size() - 40.0;
            if at_bottom {
                stick.set(true);
            } else if adj.value() < last.get() - 1.0 {
                stick.set(false);
            }
            last.set(adj.value());
            latest.set_visible(!stick.get() && adj.upper() > adj.page_size() * 1.2);
        });
        let follow = {
            let stick = self.stick.clone();
            move |adj: &gtk::Adjustment| {
                if stick.get() {
                    adj.set_value(adj.upper() - adj.page_size());
                }
            }
        };
        let also = follow.clone();
        adjustment.connect_upper_notify(move |adj| follow(adj));
        adjustment.connect_page_size_notify(move |adj| also(adj));

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
        self.input.buffer().connect_changed(move |buffer| {
            if let Some(chat) = weak.upgrade() {
                chat.placeholder.set_visible(buffer.char_count() == 0);
                chat.update_completion();
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
        let completing = self.completion.is_visible();
        match key {
            gdk::Key::Return | gdk::Key::KP_Enter if completing && !shift => {
                self.complete(self.completion_list.selected_row().map_or(0, |row| row.index()));
            }
            gdk::Key::Return | gdk::Key::KP_Enter if !shift => self.submit(),
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
            _ if !completing && self.input.buffer().char_count() == 0 && digit(key).is_some() => {
                let answer = self.on_answer.borrow().clone();
                let answered = answer.is_some_and(|answer| answer(digit(key).unwrap_or(1) - 1));
                if !answered {
                    return glib::Propagation::Proceed;
                }
            }
            gdk::Key::ISO_Left_Tab => self.call(&self.on_cycle_mode, ()),
            gdk::Key::Escape if completing => self.completion.popdown(),
            gdk::Key::Escape => self.call(&self.on_cancel, ()),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
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
        let images = self.attachments.take();
        self.show_attachments();
        self.stick.set(true);
        self.call(&self.on_submit, (text, images));
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
            let Ok(texture) = gdk::Texture::from_bytes(png) else { continue };
            let picture = gtk::Picture::for_paintable(&texture);
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.set_size_request(64, 64);
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
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("msg-images");
        row.set_halign(gtk::Align::Start);
        for png in pngs {
            let Ok(texture) = gdk::Texture::from_bytes(png) else { continue };
            let picture = gtk::Picture::for_paintable(&texture);
            picture.set_content_fit(gtk::ContentFit::Contain);
            picture.set_can_shrink(true);
            let (w, h) = (texture.width().max(1), texture.height().max(1));
            let height = h.min(220);
            picture.set_size_request((w * height / h).min(360), height);
            picture.add_css_class("msg-image");
            row.append(&picture);
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
        for c in ["working", "starting", "blocked", "done", "failed", "suspended", "idle"] {
            self.dot.remove_css_class(c);
        }
        self.dot.add_css_class(class);
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

    /// Session settings as pickers: each `{id, name, currentValue, options}` (options flat or
    /// in groups), shown like the mode: "Model · Opus 5.5", "Effort · High". The mode has its
    /// own pill, so it is left out here.
    pub fn set_options(&self, options: &Value) {
        while let Some(child) = self.options.first_child() {
            self.options.remove(&child);
        }
        for option in options.as_array().into_iter().flatten() {
            let Some(id) = option["id"].as_str().filter(|id| *id != "mode") else { continue };
            let values = flat_values(&option["options"]);
            if values.is_empty() {
                continue;
            }
            let name = option["name"].as_str().unwrap_or(id);
            let current = option["currentValue"].as_str().unwrap_or("");
            // "Fast mode · Off" reads as "Fast · Off": the chip is short, the menu says the rest.
            let short = name.strip_suffix(" mode").unwrap_or(name);
            let label = format!("{short} · {}", chip_value(&values, current));
            let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
            list.add_css_class("option-list");
            let popover = gtk::Popover::builder().child(&list).build();
            self.option_items(&list, &popover, id, name, &values, current);
            let button = gtk::MenuButton::builder().label(&label).popover(&popover).build();
            button.add_css_class("chip-picker");
            button.add_css_class("flat");
            button.set_tooltip_text(option["description"].as_str().or(Some(name)));
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
        self.send.set_label(if since.is_some() { "■" } else { "↑" });
        self.send.set_tooltip_text(Some(if since.is_some() { "Stop (Esc)" } else { "Send (Enter)" }));
        if since.is_some() {
            self.send.add_css_class("stop");
        } else {
            self.send.remove_css_class("stop");
            self.working.set_label("");
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
        self.group.take();
        *self.last.borrow_mut() = Last::None;
        if seconds < 3 {
            return;
        }
        let label = gtk::Label::builder().label(format!("Worked for {}", elapsed(seconds))).xalign(0.0).build();
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

    pub fn set_usage(&self, used: u64, size: u64) {
        let k = |n: u64| if n >= 1000 { format!("{}k", n / 1000) } else { n.to_string() };
        if size == 0 {
            self.usage.set_label(&k(used));
            self.context.set_visible(false);
            return;
        }
        let share = used as f64 / size as f64;
        self.usage.set_label(&format!("{}%", (share * 100.0).round() as u32));
        self.usage.set_tooltip_text(Some(&format!("Context: {} of {} tokens", k(used), k(size))));
        self.context.set_value(share.min(1.0));
        self.context.set_visible(true);
        if share > 0.8 {
            self.context.add_css_class("full");
        } else {
            self.context.remove_css_class("full");
        }
    }

    // ---- messages -------------------------------------------------------------------------

    pub fn clear(&self) {
        // Everything but the "starting" note, which stays first.
        while let Some(child) = self.starting.next_sibling() {
            self.list.remove(&child);
        }
        *self.last.borrow_mut() = Last::None;
        self.group.take();
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

    pub fn agent_text(&self, text: &str) {
        self.chunk(Kind::Agent, text);
    }

    pub fn thought(&self, text: &str) {
        self.chunk(Kind::Thought, text);
    }

    pub fn user_chunk(&self, text: &str) {
        self.chunk(Kind::User, text);
    }

    fn message(&self, kind: Kind) -> Rc<Message> {
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
                let message = Rc::new(Message { kind, text: RefCell::default(), body, scheduled: Cell::new(false) });
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
                self.list.append(&body);
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
        Rc::new(Message { kind, text: RefCell::default(), body, scheduled: Cell::new(false) })
    }

    pub fn notice(&self, text: &str, class: &str) {
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

    pub fn permission(&self, question: &str, detail: Option<&str>, options: &[(String, String, String)], choose: Rc<dyn Fn(String)>) -> PermissionCard {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("permission-card");
        let title = text_label(&format!("<b>{}</b>", glib::markup_escape_text(question)));
        root.append(&title);
        if let Some(detail) = detail.filter(|d| !d.is_empty()) {
            let detail = gtk::Label::builder().label(detail).xalign(0.0).wrap(true).selectable(true).build();
            detail.set_wrap_mode(pango::WrapMode::WordChar);
            detail.add_css_class("code-block");
            root.append(&detail);
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
            let choices = choices_of(field);
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
                for (i, (value, name, about)) in choices.into_iter().enumerate() {
                    let line = gtk::Box::new(gtk::Orientation::Vertical, 1);
                    let shown = if number_these && i < 9 { format!("{}  {name}", i + 1) } else { name.clone() };
                    line.append(&gtk::Label::builder().label(shown).xalign(0.0).wrap(true).build());
                    if !about.is_empty() {
                        let about = gtk::Label::builder().label(&about).xalign(0.0).wrap(true).build();
                        about.add_css_class("dim");
                        about.add_css_class("small");
                        line.append(&about);
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
fn choices_of(field: &Value) -> Vec<(Value, String, String)> {
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
                    (value, name, o["description"].as_str().unwrap_or("").to_owned())
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
            (v.clone(), name, String::new())
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
            if !rest.trim().is_empty() {
                self.body.append(&text_label(&glib::markup_escape_text(rest.trim())));
            }
            return;
        }
        for segment in markdown::segments(&text) {
            match segment {
                Segment::Text(markup) => self.body.append(&text_label(&markup)),
                Segment::Code { code, .. } => self.body.append(&code_label(&code)),
                Segment::Table(rows) => self.body.append(&table_view(&rows)),
            }
        }
    }
}

impl ToolGroup {
    fn new() -> Rc<ToolGroup> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("tool-group");
        let summary = gtk::Button::builder().visible(false).halign(gtk::Align::Start).build();
        summary.add_css_class("flat");
        summary.add_css_class("tool-toggle");
        summary.add_css_class("tool-summary");
        let cards_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&summary);
        root.append(&cards_box);
        let group = Rc::new(ToolGroup {
            root,
            summary,
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
        self.summary.set_label(&format!("{glyph}  {} tool calls  {arrow}", shown.len()));
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
        let title = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
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
        let card = Rc::new(ToolCard {
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
            let none = text_label("Nothing to show.");
            none.add_css_class("dim");
            self.detail.append(&none);
        }
    }
}

/// A tool's title for people: no Markdown ticks, and "mcp__server__tool" as "server · tool".
fn tool_title(title: &str) -> String {
    let title = title.replace('`', "");
    if let Some(rest) = title.strip_prefix("mcp__")
        && let Some((server, tool)) = rest.split_once("__")
    {
        let tool = tool.strip_prefix(&format!("{server}_")).unwrap_or(tool);
        return format!("{server} · {}", tool.replace('_', " "));
    }
    title
}

/// "42s", "3m 05s", "1h 02m".
fn elapsed(seconds: u64) -> String {
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

/// A Markdown table: a header row in small caps, hairlines between rows.
fn table_view(rows: &[Vec<String>]) -> gtk::Grid {
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
fn text_label(markup: &str) -> gtk::Label {
    let label = gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).use_markup(true).build();
    label.set_wrap_mode(pango::WrapMode::WordChar);
    label.set_markup(markup);
    label.set_focusable(false);
    label
}

/// A code block: monospace, selectable, with a copy button in its corner.
fn code_label(code: &str) -> gtk::Overlay {
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
