//! The settings window (Ctrl+,): General, Agents, Telegram, Appearance, Keyboard, About.

use gtk::{glib, pango, prelude::*};

use crate::settings::{self, Settings};

/// Every shortcut, for the Keyboard page and the in-app help.

const MODES: &[(&str, &str)] = &[
    ("", "Claude Code's default"),
    ("default", "Ask before acting"),
    ("acceptEdits", "Accept edits"),
    ("plan", "Plan only"),
    ("auto", "Auto"),
];

const SCALES: &[(f64, &str)] = &[(0.9, "Small"), (1.0, "Normal"), (1.1, "Large"), (1.25, "Larger"), (1.4, "Largest")];

pub fn open(parent: &impl IsA<gtk::Window>) {
    let s = settings::get();
    let window = gtk::Window::builder()
        .title("loom settings")
        .transient_for(parent)
        .modal(true)
        .default_width(780)
        .default_height(560)
        .build();
    window.add_css_class("loom");
    window.add_css_class("prefs");

    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).transition_duration(120).hexpand(true).build();
    stack.add_titled(&page("General", &general(&s)), Some("general"), "General");
    stack.add_titled(&page("Agents", &agents(&s)), Some("agents"), "Agents");
    stack.add_titled(&page("Telegram", &telegram(&s)), Some("telegram"), "Telegram");
    stack.add_titled(&page("Appearance", &appearance(&s)), Some("appearance"), "Appearance");
    stack.add_titled(&page("Keyboard", &keyboard()), Some("keyboard"), "Keyboard");
    stack.add_titled(&page("", &about()), Some("about"), "About");
    let sidebar = gtk::StackSidebar::builder().stack(&stack).width_request(170).build();
    sidebar.add_css_class("prefs-sidebar");
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&sidebar);
    body.append(&stack);
    window.set_child(Some(&body));

    let keys = gtk::EventControllerKey::new();
    let w = window.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            w.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    window.present();
}

fn page(title: &str, content: &gtk::Box) -> gtk::ScrolledWindow {
    content.add_css_class("prefs-page");
    if !title.is_empty() {
        let heading = gtk::Label::builder().label(title).xalign(0.0).build();
        heading.add_css_class("prefs-heading");
        content.prepend(&heading);
    }
    gtk::ScrolledWindow::builder().child(content).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build()
}

fn section(title: &str) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let label = gtk::Label::builder().label(title).xalign(0.0).build();
    label.add_css_class("prefs-section");
    section.append(&label);
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows.add_css_class("prefs-group");
    section.append(&rows);
    section
}

/// A labelled row (title, a line of explanation) with its control on the right.
fn row(section: &gtk::Box, title: &str, detail: &str, control: &impl IsA<gtk::Widget>) {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    line.add_css_class("prefs-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let title = gtk::Label::builder().label(title).xalign(0.0).build();
    title.add_css_class("prefs-title");
    text.append(&title);
    if !detail.is_empty() {
        let detail = gtk::Label::builder().label(detail).xalign(0.0).wrap(true).max_width_chars(56).build();
        detail.add_css_class("dim");
        detail.add_css_class("small");
        text.append(&detail);
    }
    control.as_ref().set_valign(gtk::Align::Center);
    line.append(&text);
    line.append(control);
    if let Some(rows) = section.last_child().and_downcast::<gtk::Box>() {
        rows.append(&line);
    }
}

fn switch(on: bool, set: impl Fn(&mut Settings, bool) + 'static) -> gtk::Switch {
    let switch = gtk::Switch::builder().active(on).build();
    switch.connect_active_notify(move |s| {
        let on = s.is_active();
        settings::update(|settings| set(settings, on));
    });
    switch
}

fn general(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);

    let memory = section("Memory");
    let idle = gtk::SpinButton::with_range(0.0, 240.0, 5.0);
    idle.set_value(s.idle_minutes as f64);
    idle.connect_value_changed(|spin| {
        let minutes = spin.value() as u64;
        settings::update(|s| s.idle_minutes = minutes);
    });
    let idle_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let unit = gtk::Label::new(Some("min"));
    unit.add_css_class("dim");
    idle_box.append(&idle);
    idle_box.append(&unit);
    row(&memory, "Suspend idle agents after", "An agent you aren't looking at stops after this long and frees its memory; opening it resumes the conversation. 0 keeps every agent running.", &idle_box);
    content.append(&memory);

    let notifications = section("Notifications");
    row(&notifications, "When an agent asks for permission", "Only when you aren't looking at that agent.", &switch(s.notify_questions, |s, on| s.notify_questions = on));
    row(&notifications, "When an agent finishes a turn", "", &switch(s.notify_done, |s, on| s.notify_done = on));
    content.append(&notifications);

    let behaviour = section("Behaviour");
    row(&behaviour, "Confirm quitting while agents work", "Quitting stops a turn in flight; conversations are kept either way.", &switch(s.confirm_quit, |s, on| s.confirm_quit = on));
    row(&behaviour, "Hide the scratch terminal when it loses focus", "", &switch(s.scratch_autohide, |s, on| s.scratch_autohide = on));
    row(
        &behaviour,
        "Agents that need you first",
        "An agent waiting for your answer goes to the top of its project (and its project to the top) until answered, then back to its place. Nothing else moves.",
        &switch(s.sort_by_urgency, |s, on| s.sort_by_urgency = on),
    );
    content.append(&behaviour);

    let trouble = section("Troubleshooting");
    row(
        &trouble,
        "Debug log",
        "Writes what loom does, in detail (messages to and from Claude, states, scrolling, the Telegram bot), to ~/.local/state/loom/debug.log. Turn it on when something misbehaves; it holds parts of your conversations, and stays on this computer.",
        &switch(s.debug_log, |s, on| s.debug_log = on),
    );
    let open = gtk::Button::with_label("Show the log");
    open.connect_clicked(|button| {
        let file = gtk::gio::File::for_path(crate::log::path());
        let window = button.root().and_downcast::<gtk::Window>();
        gtk::FileLauncher::new(Some(&file)).open_containing_folder(window.as_ref(), None::<&gtk::gio::Cancellable>, |_| {});
    });
    row(&trouble, "Log file", "Opens its folder (debug.log, and debug.log.1 before it).", &open);
    content.append(&trouble);
    content
}

fn agents(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);

    let new = section("New agents");
    let names: Vec<&str> = MODES.iter().map(|(_, name)| *name).collect();
    let mode = gtk::DropDown::from_strings(&names);
    mode.set_selected(MODES.iter().position(|(id, _)| *id == s.default_mode).unwrap_or(0) as u32);
    mode.connect_selected_notify(|d| {
        let id = MODES.get(d.selected() as usize).map_or("", |(id, _)| id).to_owned();
        settings::update(|s| s.default_mode = id);
    });
    row(&new, "Permission mode", "What a new agent may do without asking. Shift+Tab changes it per agent.", &mode);
    content.append(&new);

    let chat = section("Conversation");
    row(&chat, "Show thinking", "Claude's reasoning, folded under each answer.", &switch(s.show_thinking, |s, on| s.show_thinking = on));
    content.append(&chat);

    let program = section("Agent program");
    let entry = gtk::Entry::builder()
        .text(&s.adapter)
        .placeholder_text(crate::acp::default_adapter().join(" "))
        .width_chars(34)
        .build();
    entry.add_css_class("mono");
    entry.connect_changed(|e| {
        let text = e.text().trim().to_owned();
        settings::update(|s| s.adapter = text);
    });
    row(&program, "Command", "The ACP adapter each agent runs. Empty uses the one loom installed. Takes effect for agents started from now on.", &entry);
    let version = gtk::Label::new(Some(&crate::acp::adapter_version().unwrap_or_else(|| "not found".to_owned())));
    version.add_css_class("mono");
    version.add_css_class("dim");
    row(&program, "Installed adapter", "claude-agent-acp, in ~/.local/share/loom/adapter (./install.sh updates it).", &version);
    content.append(&program);
    content
}

/// An entry whose text is saved when done with (Enter, or focus leaving it), not on every key:
/// each save restarts the bot.
fn on_done(entry: &(impl IsA<gtk::Editable> + IsA<gtk::Widget>), save: impl Fn(String) + 'static) {
    let save = std::rc::Rc::new(save);
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

fn telegram(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);

    let intro = gtk::Label::builder()
        .label(
            "Your agents in a Telegram chat: their finished replies and questions come to your phone, and you can answer or \
             write to them from there. It talks to Telegram itself (nothing listens on this computer), obeys only your \
             account, and keeps \u{201c}Always allow\u{201d} for loom. Messages pass through Telegram's servers.",
        )
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(70)
        .build();
    intro.add_css_class("dim");
    content.append(&intro);

    let bot = section("Bot");
    row(&bot, "Use the Telegram bot", "Off until you switch it on.", &switch(s.telegram_enabled, |s, on| s.telegram_enabled = on));
    let token = gtk::PasswordEntry::builder().text(&s.telegram_token).show_peek_icon(true).placeholder_text("123456:ABC…").width_chars(30).build();
    token.add_css_class("mono");
    on_done(&token, |text| settings::update(|s| s.telegram_token = text));
    row(&bot, "Bot token", "Make a bot with @BotFather in Telegram (/newbot) and paste the token it gives.", &token);
    let user = gtk::Entry::builder()
        .text(if s.telegram_user == 0 { String::new() } else { s.telegram_user.to_string() })
        .placeholder_text("your user ID")
        .input_purpose(gtk::InputPurpose::Digits)
        .width_chars(16)
        .build();
    user.add_css_class("mono");
    let entry = user.clone();
    on_done(&user, move |text| {
        let id = text.parse::<i64>().unwrap_or(0);
        if id == 0 && !text.is_empty() {
            entry.add_css_class("error");
            return;
        }
        entry.remove_css_class("error");
        settings::update(|s| s.telegram_user = id);
    });
    row(
        &bot,
        "Your Telegram user ID",
        "Only this account is obeyed; anyone else writing to the bot is ignored. Left empty, the status below shows a code: send it to the bot and your account is linked.",
        &user,
    );
    content.append(&bot);

    let state = section("Status");
    let line = gtk::Label::builder().xalign(0.0).wrap(true).max_width_chars(48).selectable(true).build();
    line.add_css_class("dim");
    row(&state, "Connection", "", &line);
    let (weak, weak_user) = (line.downgrade(), user.downgrade());
    crate::telegram::watch(move |text| {
        let (Some(line), Some(user)) = (weak.upgrade(), weak_user.upgrade()) else { return false };
        line.set_label(text);
        // Linked by the code: the account shows here too.
        let id = settings::get().telegram_user;
        if id != 0 && !user.has_focus() && user.text() != id.to_string() {
            user.set_text(&id.to_string());
        }
        true
    });
    content.append(&state);
    content
}

fn appearance(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    let text = section("Text");
    let names: Vec<&str> = SCALES.iter().map(|(_, name)| *name).collect();
    let scale = gtk::DropDown::from_strings(&names);
    let current = SCALES.iter().position(|(v, _)| (v - s.text_scale).abs() < 0.01).unwrap_or(1);
    scale.set_selected(current as u32);
    scale.connect_selected_notify(|d| {
        let value = SCALES.get(d.selected() as usize).map_or(1.0, |(v, _)| *v);
        settings::update(|s| s.text_scale = value);
    });
    row(&text, "Conversation text size", "", &scale);
    let font = gtk::FontDialogButton::new(Some(gtk::FontDialog::builder().title("Terminal font").build()));
    font.set_font_desc(&pango::FontDescription::from_string(&s.terminal_font));
    font.set_use_size(true);
    font.connect_font_desc_notify(|b| {
        if let Some(desc) = b.font_desc() {
            let name = desc.to_string();
            settings::update(|s| s.terminal_font = name);
        }
    });
    row(&text, "Terminal font", "Scratchpad and job terminals.", &font);
    content.append(&text);

    let colours = section("Colours");
    let note = gtk::Label::new(Some("From your GTK theme"));
    note.add_css_class("dim");
    row(&colours, "Theme", "loom uses the colours your GTK stylesheet defines (noctalia, matugen…) and follows them live when the wallpaper changes.", &note);
    content.append(&colours);
    content
}

fn keyboard() -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    for (title, shortcuts) in crate::cheatsheet::SECTIONS {
        let keys = section(title);
        for (key, what) in *shortcuts {
            row(&keys, what, "", &crate::cheatsheet::keys(key));
        }
        content.append(&keys);
    }
    content
}

fn about() -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_valign(gtk::Align::Center);
    content.set_vexpand(true);
    let mark = crate::logo::mark(96);
    mark.set_halign(gtk::Align::Center);
    content.append(&mark);
    let name = gtk::Label::new(Some("loom"));
    name.add_css_class("about-name");
    content.append(&name);
    let version = gtk::Label::new(Some(&format!("Version {}", env!("CARGO_PKG_VERSION"))));
    version.add_css_class("dim");
    content.append(&version);
    let blurb = gtk::Label::builder()
        .label("A light, keyboard-first home for coding agents.\nClaude Code over ACP, in GTK, without Electron.")
        .justify(gtk::Justification::Center)
        .build();
    blurb.add_css_class("about-blurb");
    content.append(&blurb);
    let adapter = gtk::Label::new(Some(&format!("claude-agent-acp {}", crate::acp::adapter_version().unwrap_or_else(|| "—".into()))));
    adapter.add_css_class("dim");
    adapter.add_css_class("small");
    content.append(&adapter);
    content
}
