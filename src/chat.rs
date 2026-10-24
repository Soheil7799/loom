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
    stick: Rc<Cell<bool>>,
    commands: RefCell<Vec<(String, String)>>,
    completion: gtk::Popover,
    completion_list: gtk::ListBox,
    pub on_submit: Callback<String>,
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
    /// Opened by itself once (a small diff shows without a click).
    auto_opened: Cell<bool>,
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
        let options = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        options.set_valign(gtk::Align::Center);
        header.append(&names);
        header.append(&state);
        header.append(&usage);
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
            .label("Message Claude…   @ a file · / a command · Shift+Enter new line")
            .xalign(0.0)
            .valign(gtk::Align::Start)
            .margin_start(14)
            .margin_top(10)
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
        let working = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
        working.add_css_class("working");
        let send = gtk::Button::with_label("↑");
        send.add_css_class("send");
        send.set_tooltip_text(Some("Send (Enter)"));
        toolbar.append(&attach);
        toolbar.append(&options);
        toolbar.append(&mode);
        toolbar.append(&working);
        toolbar.append(&send);

        let composer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        composer.add_css_class("composer");
        composer.append(&field);
        composer.append(&toolbar);
        let input_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        input_box.add_css_class("input-box");
        input_box.append(&composer);
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
            tools: RefCell::default(),
            stick: Rc::new(Cell::new(true)),
            commands: RefCell::default(),
            completion,
            completion_list,
            on_submit: RefCell::default(),
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
                chat.stick.set(true);
                let adj = chat.scroller.vadjustment();
                adj.set_value(adj.upper() - adj.page_size());
                chat.latest.set_visible(false);
                chat.input.grab_focus();
            }
        });
        chat
    }

    fn wire(self: &Rc<Self>) {
        // Follow the bottom while it is in view; leave the reader alone once they scroll up.
        let adjustment = self.scroller.vadjustment();
        let stick = self.stick.clone();
        let latest = self.latest.clone();
        adjustment.connect_value_changed(move |adj| {
            let at_bottom = adj.value() >= adj.upper() - adj.page_size() - 40.0;
            stick.set(at_bottom);
            latest.set_visible(!at_bottom && adj.upper() > adj.page_size() * 1.2);
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

        let drop = gtk::DropTarget::new(gtk::gdk::FileList::static_type(), gtk::gdk::DragAction::COPY);
        let input = self.input.clone();
        drop.connect_drop(move |_, value, _, _| {
            let Ok(files) = value.get::<gtk::gdk::FileList>() else { return false };
            let mentions: Vec<String> = files.files().iter().filter_map(|f| f.path()).map(|p| format!("@{} ", p.display())).collect();
            input.buffer().insert_at_cursor(&mentions.concat());
            input.grab_focus();
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

    /// Session settings as pickers: each `{id, name, currentValue, options}` (options flat or
    /// in groups). The mode has its own pill, so it is left out here.
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
            let current = option["currentValue"].as_str().unwrap_or("");
            let current_name = values.iter().find(|(v, _, _)| v == current).map_or(current.to_owned(), |(_, n, _)| n.clone());
            let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
            list.add_css_class("option-list");
            let heading = gtk::Label::builder().label(option["name"].as_str().unwrap_or(id)).xalign(0.0).build();
            heading.add_css_class("option-heading");
            list.append(&heading);
            // "Model · Opus", "Effort · High": short, and saying what it is.
            let short = current_name.split(" (").next().unwrap_or(&current_name).to_owned();
            let label = format!("{} · {short}", option["name"].as_str().unwrap_or(id));
            let button = gtk::MenuButton::builder().label(&label).build();
            button.add_css_class("chip-picker");
            button.add_css_class("flat");
            button.set_tooltip_text(option["description"].as_str().or(option["name"].as_str()));
            let popover = gtk::Popover::builder().child(&list).build();
            for (value, name, description) in values {
                let line = gtk::Box::new(gtk::Orientation::Vertical, 0);
                let title = gtk::Label::builder().label(if value == current { format!("✓  {name}") } else { format!("    {name}") }).xalign(0.0).build();
                line.append(&title);
                if !description.is_empty() {
                    let detail = gtk::Label::builder().label(&description).xalign(0.0).wrap(true).max_width_chars(44).build();
                    detail.add_css_class("dim");
                    detail.add_css_class("small");
                    detail.set_margin_start(22);
                    line.append(&detail);
                }
                let item = gtk::Button::builder().child(&line).build();
                item.add_css_class("flat");
                item.add_css_class("menu-item");
                let callback = self.on_option.borrow().clone();
                let (id, popover_ref) = (id.to_owned(), popover.clone());
                item.connect_clicked(move |_| {
                    popover_ref.popdown();
                    if let Some(callback) = &callback {
                        callback((id.clone(), value.clone()));
                    }
                });
                list.append(&item);
            }
            button.set_popover(Some(&popover));
            self.options.append(&button);
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
            self.working.set_label(&format!("Working · {} · Esc to stop", elapsed(since.elapsed().as_secs())));
        }
    }

    /// A quiet line under a finished turn: how long it took.
    pub fn turn_done(&self, seconds: u64) {
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
            auto_opened: Cell::new(false),
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
            self.title.set_label(&tool_title(title));
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
    fn mcp_tools_read_as_server_and_tool() {
        assert_eq!(super::tool_title("mcp__loom__loom_run"), "loom · run");
        assert_eq!(super::tool_title("mcp__github__create_issue"), "github · create issue");
        assert_eq!(super::tool_title("`ls -la`"), "ls -la");
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
