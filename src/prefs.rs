//! The settings window (Ctrl+,): General, Agents, Appearance, Telegram, Advanced. Every
//! change is saved at once; Esc closes it.

use std::rc::Rc;

use gtk::{gdk, gio, glib, pango, prelude::*};

use crate::settings::{self, Settings};
use crate::telegram::Link;

const MODES: &[(&str, &str)] = &[
    ("", "Claude Code's default"),
    ("default", "Ask before acting"),
    ("acceptEdits", "Accept edits"),
    ("plan", "Plan only"),
    ("auto", "Auto"),
];

const SCALES: &[(f64, &str)] = &[(0.9, "Small"), (1.0, "Normal"), (1.1, "Large"), (1.25, "Larger"), (1.4, "Largest")];

const IDLE: &[(u64, &str)] = &[(0, "Never"), (5, "After 5 min"), (15, "After 15 min"), (30, "After 30 min"), (60, "After 1 hour"), (120, "After 2 hours")];

/// The pages: (name, title, icon).
const PAGES: &[(&str, &str, &str)] = &[
    ("general", "General", "preferences-system-symbolic"),
    ("agents", "Agents", "system-users-symbolic"),
    ("appearance", "Appearance", "applications-graphics-symbolic"),
    ("telegram", "Telegram", "mail-send-symbolic"),
    ("advanced", "Advanced", "utilities-terminal-symbolic"),
];

pub fn open(parent: &impl IsA<gtk::Window>) {
    let s = settings::get();
    let window = gtk::Window::builder()
        .title("loom settings")
        .transient_for(parent)
        .modal(true)
        .default_width(820)
        .default_height(600)
        .build();
    // Its buttons reach the app's actions (the overview).
    window.set_application(parent.as_ref().application().as_ref());
    window.add_css_class("loom");
    window.add_css_class("prefs");

    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).transition_duration(120).hexpand(true).build();
    stack.add_named(&page("General", &general(&s, &window)), Some("general"));
    stack.add_named(&page("Agents", &agents(&s, &window)), Some("agents"));
    stack.add_named(&page("Appearance", &appearance(&s)), Some("appearance"));
    stack.add_named(&page("Telegram", &telegram(&s)), Some("telegram"));
    stack.add_named(&page("Advanced", &advanced(&s)), Some("advanced"));

    // The pages, as loom's own sidebar lists agents.
    let nav = gtk::Box::new(gtk::Orientation::Vertical, 0);
    nav.add_css_class("prefs-nav");
    let top = gtk::Label::builder().label("Settings").xalign(0.0).build();
    top.add_css_class("brand");
    top.add_css_class("prefs-nav-top");
    nav.append(&top);
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).vexpand(true).build();
    for (_, title, icon) in PAGES {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.append(&gtk::Image::from_icon_name(icon));
        line.append(&gtk::Label::builder().label(*title).xalign(0.0).build());
        list.append(&line);
    }
    let pages = stack.clone();
    list.connect_row_selected(move |_, row| {
        if let Some((name, _, _)) = row.and_then(|r| PAGES.get(r.index() as usize)) {
            pages.set_visible_child_name(name);
        }
    });
    list.select_row(list.row_at_index(0).as_ref());
    nav.append(&list);
    let version = crate::acp::adapter_version().unwrap_or_else(|| "—".into());
    let foot = gtk::Label::builder().label(format!("loom {} · adapter {version}", env!("CARGO_PKG_VERSION"))).xalign(0.0).build();
    foot.add_css_class("prefs-nav-foot");
    foot.add_css_class("mono");
    nav.append(&foot);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&nav);
    body.append(&stack);
    window.set_child(Some(&body));

    // Esc closes; Ctrl+PageUp/PageDown and Alt+1…5 change the page.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let (w, nav_list) = (window.clone(), list.clone());
    keys.connect_key_pressed(move |_, key, keycode, modifiers| {
        let current = nav_list.selected_row().map_or(0, |r| r.index());
        let go = |index: i32| {
            if let Some(row) = nav_list.row_at_index(index.clamp(0, PAGES.len() as i32 - 1)) {
                nav_list.select_row(Some(&row));
            }
        };
        let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
        // The number row's 1…5 by position (evdev 2…6, +8): any layout.
        let digit = (10..10 + PAGES.len() as u32).contains(&keycode).then(|| keycode - 9);
        match key {
            gdk::Key::Escape => w.close(),
            gdk::Key::Page_Up if ctrl => go(current - 1),
            gdk::Key::Page_Down if ctrl => go(current + 1),
            _ if alt && digit.is_some() => go(digit.unwrap_or(1) as i32 - 1),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);
    window.present();
}

/// A page: its title over a centred column (not stretched across a wide window).
fn page(title: &str, content: &gtk::Box) -> gtk::ScrolledWindow {
    content.add_css_class("prefs-page");
    let heading = gtk::Label::builder().label(title).xalign(0.0).build();
    heading.add_css_class("prefs-heading");
    content.prepend(&heading);
    content.set_valign(gtk::Align::Start);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(content);
    column.set_layout_manager(Some(crate::column::ColumnLayout::new(620)));
    gtk::ScrolledWindow::builder().child(&column).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build()
}

fn section(title: &str) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    if !title.is_empty() {
        let label = gtk::Label::builder().label(title).xalign(0.0).build();
        label.add_css_class("prefs-section");
        section.append(&label);
    }
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows.add_css_class("prefs-group");
    section.append(&rows);
    section
}

fn rows_of(section: &gtk::Box) -> Option<gtk::Box> {
    let mut child = section.first_child();
    while let Some(widget) = child {
        if widget.has_css_class("prefs-group") {
            return widget.downcast::<gtk::Box>().ok();
        }
        child = widget.next_sibling();
    }
    None
}

fn texts(title: &str, detail: &str) -> gtk::Box {
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let title = gtk::Label::builder().label(title).xalign(0.0).build();
    title.add_css_class("prefs-title");
    text.append(&title);
    if !detail.is_empty() {
        let detail = gtk::Label::builder().label(detail).xalign(0.0).wrap(true).max_width_chars(52).build();
        detail.add_css_class("dim");
        detail.add_css_class("small");
        text.append(&detail);
    }
    text
}

/// A row: title and a line of explanation, its control on the right.
fn row(section: &gtk::Box, title: &str, detail: &str, control: &impl IsA<gtk::Widget>) {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    line.add_css_class("prefs-row");
    control.as_ref().set_valign(gtk::Align::Center);
    line.append(&texts(title, detail));
    line.append(control);
    if let Some(rows) = rows_of(section) {
        rows.append(&line);
    }
}

/// A row whose control needs the width (an entry): it goes under the title.
fn row_stacked(section: &gtk::Box, title: &str, detail: &str, control: &impl IsA<gtk::Widget>) {
    let line = gtk::Box::new(gtk::Orientation::Vertical, 8);
    line.add_css_class("prefs-row");
    line.append(&texts(title, detail));
    control.as_ref().set_hexpand(true);
    line.append(control);
    if let Some(rows) = rows_of(section) {
        rows.append(&line);
    }
}

/// A quiet line under a group.
fn caption(section: &gtk::Box, text: &str) {
    let label = gtk::Label::builder().label(text).xalign(0.0).wrap(true).max_width_chars(70).build();
    label.add_css_class("prefs-caption");
    section.append(&label);
}

fn switch(on: bool, set: impl Fn(&mut Settings, bool) + 'static) -> gtk::Switch {
    let switch = gtk::Switch::builder().active(on).build();
    switch.connect_active_notify(move |s| {
        let on = s.is_active();
        settings::update(|settings| set(settings, on));
    });
    switch
}

/// A choice among `choices`; the current one shown even if it isn't among them.
fn dropdown<T: PartialEq + Clone + 'static>(choices: &[(T, &str)], current: T, extra: impl Fn(&T) -> String, set: impl Fn(&mut Settings, T) + 'static) -> gtk::DropDown {
    let mut options: Vec<(T, String)> = choices.iter().map(|(v, n)| (v.clone(), (*n).to_owned())).collect();
    if !options.iter().any(|(v, _)| *v == current) {
        let name = extra(&current);
        options.push((current.clone(), name));
    }
    let names: Vec<&str> = options.iter().map(|(_, n)| n.as_str()).collect();
    let dropdown = gtk::DropDown::from_strings(&names);
    dropdown.set_selected(options.iter().position(|(v, _)| *v == current).unwrap_or(0) as u32);
    dropdown.connect_selected_notify(move |d| {
        if let Some((value, _)) = options.get(d.selected() as usize) {
            let value = value.clone();
            settings::update(|s| set(s, value));
        }
    });
    dropdown
}

fn flat_button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("prefs-button");
    button
}

/// An entry whose text is saved when done with (Enter, or focus leaving it), not on every key:
/// each save restarts the bot.
fn on_done(entry: &(impl IsA<gtk::Editable> + IsA<gtk::Widget>), save: impl Fn(String) + 'static) {
    let save = Rc::new(save);
    let (s, e) = (save.clone(), entry.clone().upcast::<gtk::Editable>());
    if let Some(entry) = entry.dynamic_cast_ref::<gtk::Entry>() {
        entry.connect_activate(move |_| s(e.text().trim().to_owned()));
    } else if let Some(entry) = entry.dynamic_cast_ref::<gtk::PasswordEntry>() {
        entry.connect_activate(move |_| s(e.text().trim().to_owned()));
    }
    let focus = gtk::EventControllerFocus::new();
    let e = entry.clone().upcast::<gtk::Editable>();
    focus.connect_leave(move |_| save(e.text().trim().to_owned()));
    entry.add_controller(focus);
}

fn general(s: &Settings, window: &gtk::Window) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 22);

    let notifications = section("Notifications");
    row(&notifications, "When an agent asks for permission", "", &switch(s.notify_questions, |s, on| s.notify_questions = on));
    row(&notifications, "When an agent finishes a turn", "", &switch(s.notify_done, |s, on| s.notify_done = on));
    caption(&notifications, "Only about agents you aren't looking at.");
    content.append(&notifications);

    let behaviour = section("Behaviour");
    row(
        &behaviour,
        "Agents that need you first",
        "One waiting for your answer moves to the top of its project until answered.",
        &switch(s.sort_by_urgency, |s, on| s.sort_by_urgency = on),
    );
    let start = dropdown(&[(true, "The overview"), (false, "The last agent")], s.start_on_overview, |_| String::new(), |s, on| s.start_on_overview = on);
    row(&behaviour, "When loom starts", "The overview shows every agent at a glance and resumes none until you open it.", &start);
    row(&behaviour, "Confirm quitting while agents work", "", &switch(s.confirm_quit, |s, on| s.confirm_quit = on));
    row(&behaviour, "Hide the scratch terminal when it loses focus", "", &switch(s.scratch_autohide, |s, on| s.scratch_autohide = on));
    let idle = dropdown(IDLE, s.idle_minutes, |m| format!("After {m} min"), |s, m| s.idle_minutes = m);
    row(&behaviour, "Suspend idle agents", "Frees an unwatched agent's memory; opening it resumes the conversation.", &idle);
    content.append(&behaviour);

    let keyboard = section("Keyboard");
    let shortcuts = flat_button("Show  Ctrl+/");
    let parent = window.transient_for();
    shortcuts.connect_clicked(move |_| {
        if let Some(parent) = &parent {
            crate::cheatsheet::open(parent);
        }
    });
    row(&keyboard, "Keyboard shortcuts", "Every key loom knows.", &shortcuts);
    content.append(&keyboard);
    content
}

fn agents(s: &Settings, window: &gtk::Window) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 22);

    let new = section("New agents");
    let modes: Vec<(String, &str)> = MODES.iter().map(|(id, name)| ((*id).to_owned(), *name)).collect();
    let mode = dropdown(&modes, s.default_mode.clone(), |m| m.clone(), |s, m| s.default_mode = m);
    row(&new, "Permission mode", "What a new agent may do without asking. Shift+Tab changes it per agent.", &mode);
    content.append(&new);

    let chat = section("Conversation");
    row(&chat, "Show thinking", "Claude's reasoning, folded under each answer.", &switch(s.show_thinking, |s, on| s.show_thinking = on));
    row(
        &chat,
        "Suggest your next message",
        "After a turn, Claude Haiku guesses what you'll write next; → or Tab fills it in. A little usage each turn.",
        &switch(s.suggest_prompts, |s, on| s.suggest_prompts = on),
    );
    content.append(&chat);

    let schedules = section("Scheduled prompts");
    let open = flat_button("Open the overview");
    let w = window.clone();
    open.connect_clicked(move |_| {
        // While still attached to the app (closed, it no longer reaches its actions).
        let _ = w.activate_action("app.show-overview", None);
        w.close();
    });
    row(&schedules, "Prompts on a schedule", "Set from an agent's menu; listed and paused in the overview (Ctrl+Shift+A).", &open);
    content.append(&schedules);
    content
}

fn appearance(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
    let text = section("Text");
    let scale = dropdown(SCALES, s.text_scale, |v| format!("{:.0}%", v * 100.0), |s, v| s.text_scale = v);
    row(&text, "Conversation text size", "", &scale);
    let font = gtk::FontDialogButton::new(Some(gtk::FontDialog::builder().title("Terminal font").build()));
    // A list of families ("JetBrains Mono, Vazirmatn 10") shows as its first one.
    let mut desc = pango::FontDescription::from_string(&s.terminal_font);
    let first = desc.family().and_then(|f| f.split(',').next().map(|f| f.trim().to_owned())).filter(|f| !f.is_empty());
    match first {
        Some(first) => desc.set_family(&first),
        None => {
            desc = pango::FontDescription::from_string(crate::term::FONT);
            desc.set_family("JetBrains Mono");
        }
    }
    font.set_font_desc(&desc);
    font.set_use_size(true);
    font.connect_font_desc_notify(|b| {
        if let Some(desc) = b.font_desc() {
            let name = desc.to_string();
            settings::update(|s| s.terminal_font = name);
        }
    });
    row(&text, "Terminal font", "The scratchpad and the tray's terminals.", &font);
    caption(&text, "Colours come from your GTK theme (noctalia, matugen…) and follow it live.");
    content.append(&text);
    content
}

fn telegram(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 22);

    // What the bot is doing, at the top, with its switch.
    let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    card.add_css_class("prefs-status");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let dot = gtk::Box::builder().valign(gtk::Align::Center).build();
    dot.add_css_class("dot");
    let title = gtk::Label::builder().xalign(0.0).build();
    title.add_css_class("prefs-title");
    let detail = gtk::Label::builder().xalign(0.0).wrap(true).max_width_chars(52).selectable(true).build();
    detail.add_css_class("dim");
    detail.add_css_class("small");
    let words = gtk::Box::new(gtk::Orientation::Vertical, 2);
    words.set_hexpand(true);
    words.append(&title);
    words.append(&detail);
    let on = switch(s.telegram_enabled, |s, on| s.telegram_enabled = on);
    on.set_valign(gtk::Align::Center);
    head.append(&dot);
    head.append(&words);
    head.append(&on);
    card.append(&head);
    // Linking an account: the code to send, copied or sent in one tap.
    let link = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let link_hint = gtk::Label::new(Some("Send this code to the bot:"));
    link_hint.add_css_class("small");
    let code = gtk::Label::builder().selectable(true).build();
    code.add_css_class("link-code");
    let copy = flat_button("Copy");
    let open = flat_button("Open in Telegram");
    link.append(&link_hint);
    link.append(&code);
    link.append(&copy);
    link.append(&open);
    link.set_visible(false);
    card.append(&link);
    content.append(&card);

    let setup = section("Setup");
    let token = gtk::PasswordEntry::builder().text(&s.telegram_token).show_peek_icon(true).placeholder_text("123456:ABC…").build();
    token.add_css_class("mono");
    on_done(&token, |text| settings::update(|s| s.telegram_token = text));
    row_stacked(&setup, "Bot token", "Make a bot with @BotFather in Telegram (/newbot) and paste the token it gives.", &token);
    let user = gtk::Entry::builder()
        .text(if s.telegram_user == 0 { String::new() } else { s.telegram_user.to_string() })
        .placeholder_text("Linked with the code above, or typed here")
        .input_purpose(gtk::InputPurpose::Digits)
        .build();
    let entry = user.clone();
    on_done(&user, move |text| {
        let id = text.parse::<i64>().unwrap_or(0);
        if id == 0 && !text.is_empty() {
            entry.add_css_class("error");
            entry.set_tooltip_text(Some("A Telegram user ID is a number."));
            return;
        }
        entry.remove_css_class("error");
        entry.set_tooltip_text(None);
        settings::update(|s| s.telegram_user = id);
    });
    row_stacked(&setup, "Your Telegram account", "Its user ID. Only this account is obeyed; anyone else writing to the bot is ignored.", &user);
    caption(&setup, "Messages pass through Telegram's servers. “Always allow” and ! commands stay in loom.");
    content.append(&setup);

    let bot_name = Rc::new(std::cell::RefCell::new(String::new()));
    let names = bot_name.clone();
    let (wd, wt, wdet, wl, wc, wu, wcard) = (dot.downgrade(), title.downgrade(), detail.downgrade(), link.downgrade(), code.downgrade(), user.downgrade(), card.downgrade());
    crate::telegram::watch(move |state| {
        let (Some(dot), Some(title), Some(detail), Some(link), Some(code), Some(user), Some(card)) =
            (wd.upgrade(), wt.upgrade(), wdet.upgrade(), wl.upgrade(), wc.upgrade(), wu.upgrade(), wcard.upgrade())
        else {
            return false;
        };
        for class in ["suspended", "starting", "done", "failed"] {
            dot.remove_css_class(class);
        }
        card.remove_css_class("ok");
        card.remove_css_class("failed");
        let (class, heading, words, linking) = match state {
            Link::Off => ("suspended", "Off".to_owned(), "Switch it on to reach your agents from Telegram.".to_owned(), None),
            Link::NeedsToken => ("suspended", "Needs a bot token".to_owned(), "Paste it below.".to_owned(), None),
            Link::Connecting => ("starting", "Connecting…".to_owned(), String::new(), None),
            Link::Connected { bot, user, code, problem } => {
                *names.borrow_mut() = bot.clone();
                card.add_css_class("ok");
                let mut words = if *user != 0 { format!("Linked to account {user}.") } else { "No account linked yet.".to_owned() };
                if let Some(problem) = problem {
                    words.push_str(&format!(" A message could not be sent: {problem}"));
                }
                ("done", format!("Connected as @{bot}"), words, code.clone())
            }
            Link::Failed(why) => {
                card.add_css_class("failed");
                ("failed", "Can't connect".to_owned(), why.clone(), None)
            }
        };
        dot.add_css_class(class);
        title.set_label(&heading);
        detail.set_label(&words);
        detail.set_visible(!words.is_empty());
        link.set_visible(linking.is_some());
        code.set_label(linking.as_deref().unwrap_or(""));
        // Linked by the code: the account shows below too.
        let id = settings::get().telegram_user;
        if id != 0 && (!user.has_focus() || user.text().is_empty()) && user.text() != id.to_string() {
            user.set_text(&id.to_string());
        }
        true
    });
    let shown = code.clone();
    copy.connect_clicked(move |button| button.clipboard().set_text(&shown.label()));
    let shown = code.clone();
    open.connect_clicked(move |button| {
        let url = format!("https://t.me/{}?start={}", bot_name.borrow(), shown.label());
        let window = button.root().and_downcast::<gtk::Window>();
        gtk::UriLauncher::new(&url).launch(window.as_ref(), None::<&gio::Cancellable>, |_| {});
    });
    content
}

fn advanced(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 22);

    let program = section("Agent program");
    let entry = gtk::Entry::builder().text(&s.adapter).placeholder_text(crate::acp::default_adapter().join(" ")).build();
    entry.add_css_class("mono");
    entry.connect_changed(|e| {
        let text = e.text().trim().to_owned();
        settings::update(|s| s.adapter = text);
    });
    let version = crate::acp::adapter_version().unwrap_or_else(|| "not found".to_owned());
    row_stacked(
        &program,
        "Command",
        &format!("The ACP adapter each agent runs; empty uses the one loom installed (claude-agent-acp {version}). For agents started from now on."),
        &entry,
    );
    content.append(&program);

    let trouble = section("Troubleshooting");
    let open = flat_button("Open folder");
    open.set_visible(s.debug_log);
    let log_switch = switch(s.debug_log, |s, on| s.debug_log = on);
    let shown = open.clone();
    log_switch.connect_active_notify(move |s| shown.set_visible(s.is_active()));
    open.connect_clicked(|button| {
        let file = gio::File::for_path(crate::log::path());
        let window = button.root().and_downcast::<gtk::Window>();
        gtk::FileLauncher::new(Some(&file)).open_containing_folder(window.as_ref(), None::<&gio::Cancellable>, |_| {});
    });
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    controls.append(&open);
    controls.append(&log_switch);
    row(
        &trouble,
        "Debug log",
        "What loom does, in detail, and its memory use every few minutes, in ~/.local/state/loom/debug.log. Holds parts of conversations; stays on this computer.",
        &controls,
    );
    content.append(&trouble);
    content
}
