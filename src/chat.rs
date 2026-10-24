//! One agent's conversation: a header, the messages and tool calls, the plan, and the input.
//!
//! Widgets only; what happens on submit, stop or a permission answer is the agent's business
//! (see agent.rs), passed in as callbacks.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::{gdk, glib, pango, prelude::*};
use serde_json::Value;

use crate::markdown::{self, Segment};

type Callback<T> = RefCell<Option<Rc<dyn Fn(T)>>>;

pub struct Chat {
    pub root: gtk::Box,
    title: gtk::Label,
    place: gtk::Label,
    state: gtk::Label,
    mode: gtk::Button,
    usage: gtk::Label,
    scroller: gtk::ScrolledWindow,
    list: gtk::Box,
    plan: gtk::Box,
    plan_revealer: gtk::Revealer,
    pub input: gtk::TextView,
    last: RefCell<Last>,
    tools: RefCell<HashMap<String, Rc<ToolCard>>>,
    stick: Rc<Cell<bool>>,
    commands: RefCell<Vec<(String, String)>>,
    completion: gtk::Popover,
    completion_list: gtk::ListBox,
    pub on_submit: Callback<String>,
    pub on_cancel: Callback<()>,
    pub on_cycle_mode: Callback<()>,
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
    has_terminal_button: Cell<bool>,
}

pub struct PermissionCard {
    buttons: gtk::Box,
    answer: gtk::Label,
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
        place.add_css_class("dim");
        place.add_css_class("small");
        names.append(&title);
        names.append(&place);
        let state = gtk::Label::new(None);
        state.add_css_class("dim");
        let mode = gtk::Button::with_label("default");
        mode.add_css_class("pill");
        mode.add_css_class("flat");
        mode.set_tooltip_text(Some("Permission mode (Shift+Tab in the input)"));
        mode.set_valign(gtk::Align::Center);
        let usage = gtk::Label::new(None);
        usage.add_css_class("dim");
        usage.add_css_class("small");
        usage.set_tooltip_text(Some("Context window in use"));
        header.append(&names);
        header.append(&state);
        header.append(&usage);
        header.append(&mode);
        root.append(&header);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
        list.add_css_class("messages");
        let clamp = gtk::Box::new(gtk::Orientation::Vertical, 0);
        clamp.append(&list);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&clamp)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        root.append(&scroller);

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
            .max_content_height(220)
            .build();
        let input_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        input_box.add_css_class("input-box");
        input_box.append(&input_scroller);
        root.append(&input_box);

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
            state,
            mode,
            usage,
            scroller,
            list,
            plan,
            plan_revealer,
            input,
            last: RefCell::new(Last::None),
            tools: RefCell::default(),
            stick: Rc::new(Cell::new(true)),
            commands: RefCell::default(),
            completion,
            completion_list,
            on_submit: RefCell::default(),
            on_cancel: RefCell::default(),
            on_cycle_mode: RefCell::default(),
            on_answer: RefCell::default(),
        });
        chat.wire();
        chat
    }

    fn wire(self: &Rc<Self>) {
        // Follow the bottom while it is in view; leave the reader alone once they scroll up.
        let adjustment = self.scroller.vadjustment();
        let stick = self.stick.clone();
        adjustment.connect_value_changed(move |adj| {
            stick.set(adj.value() >= adj.upper() - adj.page_size() - 40.0);
        });
        let stick = self.stick.clone();
        adjustment.connect_upper_notify(move |adj| {
            if stick.get() {
                adj.set_value(adj.upper() - adj.page_size());
            }
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
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(chat) = weak.upgrade() else { return glib::Propagation::Proceed };
            chat.key(key, modifiers)
        });
        self.input.add_controller(keys);

        let weak = Rc::downgrade(self);
        self.input.buffer().connect_changed(move |_| {
            if let Some(chat) = weak.upgrade() {
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

    fn key(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
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

    fn submit(&self) {
        let buffer = self.input.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).trim().to_owned();
        if text.is_empty() {
            return;
        }
        buffer.set_text("");
        self.stick.set(true);
        self.call(&self.on_submit, text);
    }

    // ---- slash commands -------------------------------------------------------------------

    pub fn set_commands(&self, commands: Vec<(String, String)>) {
        *self.commands.borrow_mut() = commands;
    }

    /// The commands the input's "/word" matches, best first.
    fn matches(&self) -> Vec<(String, String)> {
        let buffer = self.input.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        let Some(word) = text.strip_prefix('/') else { return Vec::new() };
        if word.contains(char::is_whitespace) {
            return Vec::new();
        }
        let commands = self.commands.borrow();
        let mut found: Vec<&(String, String)> = commands.iter().filter(|(name, _)| name.starts_with(word)).collect();
        found.extend(commands.iter().filter(|(name, _)| !name.starts_with(word) && name.contains(word)));
        found.into_iter().take(8).cloned().collect()
    }

    fn update_completion(&self) {
        let found = self.matches();
        while let Some(row) = self.completion_list.row_at_index(0) {
            self.completion_list.remove(&row);
        }
        if found.is_empty() {
            self.completion.popdown();
            return;
        }
        for (name, description) in &found {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let name = gtk::Label::new(Some(&format!("/{name}")));
            name.add_css_class("mono");
            let description = gtk::Label::builder()
                .label(description.lines().next().unwrap_or(""))
                .ellipsize(pango::EllipsizeMode::End)
                .max_width_chars(60)
                .xalign(0.0)
                .build();
            description.add_css_class("dim");
            row.append(&name);
            row.append(&description);
            self.completion_list.append(&row);
        }
        if let Some(first) = self.completion_list.row_at_index(0) {
            self.completion_list.select_row(Some(&first));
        }
        self.completion.popup();
    }

    fn complete(&self, index: i32) {
        let found = self.matches();
        if let Some((name, _)) = found.get(index.max(0) as usize) {
            self.input.buffer().set_text(&format!("/{name} "));
        }
        self.completion.popdown();
        self.input.grab_focus();
    }

    // ---- header ---------------------------------------------------------------------------

    pub fn set_title(&self, title: &str, place: &str) {
        self.title.set_label(title);
        self.place.set_label(place);
    }

    pub fn set_state(&self, text: &str) {
        self.state.set_label(text);
    }

    pub fn set_mode(&self, mode: &str) {
        self.mode.set_label(mode);
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

    pub fn set_usage(&self, used: u64, size: u64) {
        let k = |n: u64| if n >= 1000 { format!("{}k", n / 1000) } else { n.to_string() };
        self.usage.set_label(&if size > 0 { format!("{} / {}", k(used), k(size)) } else { k(used) });
    }

    // ---- messages -------------------------------------------------------------------------

    pub fn clear(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        *self.last.borrow_mut() = Last::None;
        self.tools.borrow_mut().clear();
        self.set_plan(&Value::Null);
    }

    pub fn user(&self, text: &str) {
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
        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        match kind {
            Kind::Agent => {
                body.add_css_class("msg-agent");
                self.list.append(&body);
            }
            Kind::User => {
                body.add_css_class("msg-user");
                self.list.append(&body);
            }
            Kind::Thought => {
                let expander = gtk::Expander::new(Some("Thinking"));
                expander.add_css_class("thought");
                expander.set_child(Some(&body));
                self.list.append(&expander);
            }
        }
        Rc::new(Message { kind, text: RefCell::default(), body, scheduled: Cell::new(false) })
    }

    pub fn notice(&self, text: &str, class: &str) {
        let label = text_label(&glib::markup_escape_text(text));
        label.add_css_class("notice");
        label.add_css_class(class);
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
            self.list.append(&card.root);
            self.tools.borrow_mut().insert(id.to_owned(), card.clone());
            *self.last.borrow_mut() = Last::None;
            card
        });
        card.update(update);
    }

    /// Adds a button to a tool card that opens its terminal.
    pub fn tool_terminal(&self, id: &str, open: Rc<dyn Fn()>) {
        let Some(card) = self.tools.borrow().get(id).cloned() else { return };
        if card.has_terminal_button.replace(true) {
            return;
        }
        let button = gtk::Button::with_label("terminal");
        button.add_css_class("flat");
        button.add_css_class("small");
        button.set_tooltip_text(Some("Show this command's output"));
        button.connect_clicked(move |_| open());
        card.actions.append(&button);
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
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for (i, (id, name, kind)) in options.iter().enumerate() {
            let button = gtk::Button::with_label(&format!("{}  {name}", i + 1));
            button.add_css_class(if kind.starts_with("allow") { "suggested" } else { "plain" });
            let choose = choose.clone();
            let id = id.clone();
            button.connect_clicked(move |_| choose(id.clone()));
            buttons.append(&button);
        }
        root.append(&buttons);
        let answer = gtk::Label::builder().xalign(0.0).visible(false).build();
        answer.add_css_class("dim");
        root.append(&answer);
        self.list.append(&root);
        *self.last.borrow_mut() = Last::None;
        PermissionCard { buttons, answer }
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
                .build();
            label.add_css_class(class);
            self.plan.append(&label);
        }
        self.plan_revealer.set_reveal_child(!entries.is_empty() && open);
    }
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
    /// Re-renders at most every 60 ms while text streams in.
    fn render_soon(self: &Rc<Self>) {
        if self.scheduled.replace(true) {
            return;
        }
        let message = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(60), move || {
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
            // What people type, and thinking, are shown as written.
            self.body.append(&text_label(&glib::markup_escape_text(text.trim())));
            return;
        }
        for segment in markdown::segments(&text) {
            match segment {
                Segment::Text(markup) => self.body.append(&text_label(&markup)),
                Segment::Code { code, .. } => self.body.append(&code_label(&code)),
            }
        }
    }
}

impl ToolCard {
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
        row.append(&glyph);
        row.append(&spinner);
        row.append(&kind);
        row.append(&title);
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
            has_terminal_button: Cell::new(false),
        });
        let weak = Rc::downgrade(&card);
        toggle.connect_clicked(move |_| {
            if let Some(card) = weak.upgrade() {
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
            self.title.set_label(&title.replace('`', ""));
            self.title.set_tooltip_text(Some(title));
        }
        if let Some(kind) = update["kind"].as_str() {
            self.kind.set_label(match kind {
                "execute" => "run",
                "fetch" => "web",
                "think" => "think",
                "other" => "tool",
                other => other,
            });
        }
        if let Some(status) = update["status"].as_str() {
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
                            }
                        }
                    }
                }
                Some("terminal") => {
                    let hint = text_label("Output is in the job tray.");
                    hint.add_css_class("dim");
                    self.detail.append(&hint);
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

/// 1–9 for those keys.
fn digit(key: gdk::Key) -> Option<usize> {
    key.to_unicode().and_then(|c| c.to_digit(10)).filter(|d| *d >= 1).map(|d| d as usize)
}

/// A wrapping, selectable label for Pango markup.
fn text_label(markup: &str) -> gtk::Label {
    let label = gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).use_markup(true).build();
    label.set_wrap_mode(pango::WrapMode::WordChar);
    label.set_markup(markup);
    label.set_focusable(false);
    label
}

fn code_label(code: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(code).xalign(0.0).wrap(true).selectable(true).build();
    label.set_wrap_mode(pango::WrapMode::Char);
    label.add_css_class("code-block");
    label.set_focusable(false);
    label
}

/// The lines between what the old and new text share at either end, as removed and added.
fn diff_view(item: &Value) -> gtk::Box {
    let view = gtk::Box::new(gtk::Orientation::Vertical, 0);
    view.add_css_class("diff");
    let path = gtk::Label::builder().label(item["path"].as_str().unwrap_or("")).xalign(0.0).build();
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
    use super::changed_lines;

    #[test]
    fn only_the_changed_middle_is_shown() {
        let d = changed_lines("a\nb\nc\nd", "a\nB\nc\nd");
        assert_eq!(d, vec![('-', "b"), ('+', "B")]);
    }

    #[test]
    fn a_new_file_is_all_added() {
        assert_eq!(changed_lines("", "x\ny"), vec![('+', "x"), ('+', "y")]);
    }
}
