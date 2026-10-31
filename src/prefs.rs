//! The settings window (Ctrl+,): General, Agents, Appearance, Claude, Advanced, and the
//! add-ons (Notes, Telegram, Speech to text), each off until switched on. Every change is
//! saved at once; Esc closes it.

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

const REST_TOKENS: &[(u64, &str)] = &[(50_000, "Over 50K tokens"), (100_000, "Over 100K tokens"), (200_000, "Over 200K tokens"), (400_000, "Over 400K tokens")];
const IDLE: &[(u64, &str)] = &[(0, "Never"), (5, "After 5 min"), (15, "After 15 min"), (30, "After 30 min"), (45, "After 45 min"), (60, "After 1 hour"), (120, "After 2 hours")];

/// The pages: (name, title, icon, an add-on's: listed under Add-ons).
const PAGES: &[(&str, &str, &str, bool)] = &[
    ("general", "General", "preferences-system-symbolic", false),
    ("agents", "Agents", "system-users-symbolic", false),
    ("appearance", "Appearance", "applications-graphics-symbolic", false),
    ("claude", "Claude", "applications-engineering-symbolic", false),
    ("advanced", "Advanced", "utilities-terminal-symbolic", false),
    ("addons", "Add-ons", "application-x-addon-symbolic", false),
    ("notes", "Notes", "accessories-text-editor-symbolic", true),
    ("telegram", "Telegram", "mail-send-symbolic", true),
    ("voice", "Speech to text", "audio-input-microphone-symbolic", true),
];

/// The nav row of page `name` selected (its page shown).
fn go_to(list: &gtk::ListBox, name: &str) {
    if let Some(index) = PAGES.iter().position(|(n, ..)| *n == name) {
        list.select_row(list.row_at_index(index as i32).as_ref());
    }
}

pub fn open(parent: &impl IsA<gtk::Window>) {
    let s = settings::get();
    let window = gtk::Window::builder()
        .title("loom settings")
        .transient_for(parent)
        .default_width(820)
        .default_height(600)
        .build();
    // Its buttons reach the app's actions (the overview).
    window.set_application(parent.as_ref().application().as_ref());
    window.add_css_class("loom");
    window.add_css_class("prefs");

    // The nav first: pages link to each other through it.
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).vexpand(true).build();
    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).transition_duration(120).hexpand(true).build();
    stack.add_named(&page("General", &general(&s, &window)), Some("general"));
    stack.add_named(&page("Agents", &agents(&s, &window)), Some("agents"));
    stack.add_named(&page("Appearance", &appearance(&s)), Some("appearance"));
    stack.add_named(&page("Claude", &claude(&window)), Some("claude"));
    stack.add_named(&page("Advanced", &advanced(&s)), Some("advanced"));
    stack.add_named(&page("Add-ons", &addons(&s, &list)), Some("addons"));
    stack.add_named(&page("Notes", &notes(&s, &window)), Some("notes"));
    stack.add_named(&page("Telegram", &telegram(&s)), Some("telegram"));
    stack.add_named(&page("Speech to text", &voice(&s)), Some("voice"));

    // The pages, as loom's own sidebar lists agents.
    let nav = gtk::Box::new(gtk::Orientation::Vertical, 0);
    nav.add_css_class("prefs-nav");
    let top = gtk::Label::builder().label("Settings").xalign(0.0).build();
    top.add_css_class("prefs-nav-title");
    top.add_css_class("prefs-nav-top");
    nav.append(&top);
    for (_, title, icon, addon) in PAGES {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.append(&gtk::Image::from_icon_name(icon));
        line.append(&gtk::Label::builder().label(*title).xalign(0.0).build());
        list.append(&line);
        if *addon {
            if let Some(row) = list.last_child() {
                row.add_css_class("prefs-nav-sub");
            }
        }
    }
    let pages = stack.clone();
    list.connect_row_selected(move |_, row| {
        if let Some((name, ..)) = row.and_then(|r| PAGES.get(r.index() as usize)) {
            pages.set_visible_child_name(name);
        }
    });
    list.select_row(list.row_at_index(0).as_ref());
    nav.append(&list);
    // The versions, and About behind them.
    let foot_label = gtk::Label::builder().label(format!("loom {} · {}", env!("CARGO_PKG_VERSION"), crate::provider::describe())).xalign(0.0).build();
    foot_label.add_css_class("mono");
    let foot = gtk::Button::builder().child(&foot_label).tooltip_text("About Loom").build();
    foot.add_css_class("flat");
    foot.add_css_class("prefs-nav-foot");
    let about_parent = window.clone();
    foot.connect_clicked(move |_| crate::about::open(&about_parent));
    nav.append(&foot);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&nav);
    // The pages, with a close button over their top right corner.
    let pages_overlay = gtk::Overlay::builder().child(&stack).hexpand(true).build();
    let close = gtk::Button::builder().label("×").tooltip_text("Close (Esc)").halign(gtk::Align::End).valign(gtk::Align::Start).build();
    close.add_css_class("prefs-close");
    close.update_property(&[gtk::accessible::Property::Label("Close Settings")]);
    let w = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    pages_overlay.add_overlay(&close);
    body.append(&pages_overlay);
    window.set_child(Some(&body));

    // A click anywhere in the main window puts Settings away, and still lands. Not on the
    // main window turning active (focus follows the mouse: a hover would close it), nor on
    // Settings losing focus (its own dialogs take it).
    let click = gtk::GestureClick::builder().button(0).propagation_phase(gtk::PropagationPhase::Capture).build();
    let w = window.downgrade();
    click.connect_pressed(move |_, _, _, _| {
        if let Some(w) = w.upgrade().filter(|w| w.is_visible()) {
            w.close();
        }
    });
    let parent = parent.as_ref().clone();
    parent.add_controller(click.clone());
    window.connect_close_request(move |_| {
        parent.remove_controller(&click);
        glib::Propagation::Proceed
    });

    // Esc closes; Ctrl+PageUp/PageDown and Alt+1…9 change the page.
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
        // The number row's 1…9 by position (evdev 2…10, +8): any layout.
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
fn row(section: &gtk::Box, title: &str, detail: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    line.add_css_class("prefs-row");
    control.as_ref().set_valign(gtk::Align::Center);
    line.append(&texts(title, detail));
    line.append(control);
    if let Some(rows) = rows_of(section) {
        rows.append(&line);
    }
    line
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
    let switch = gtk::Switch::builder().active(on).valign(gtk::Align::Center).build();
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
    row(
        &behaviour,
        "Running agents above the projects",
        "A list of the agents running now (each also stays in its project). Ctrl+1…9 go by it; drag to reorder; ☕ keeps one awake.",
        &switch(s.show_running, |s, on| s.show_running = on),
    );
    row(
        &behaviour,
        "Ctrl+Tab cycles running agents only",
        "Suspended ones are skipped, so passing one doesn't wake it.",
        &switch(s.cycle_running, |s, on| s.cycle_running = on),
    );
    row(&behaviour, "Confirm quitting while agents work", "", &switch(s.confirm_quit, |s, on| s.confirm_quit = on));
    row(&behaviour, "Hide the scratch terminal when it loses focus", "", &switch(s.scratch_autohide, |s, on| s.scratch_autohide = on));
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
        "After a turn, Claude Code suggests what you'll write next; → or Tab fills it in.",
        &switch(s.suggest_prompts, |s, on| s.suggest_prompts = on),
    );
    // How full, then on or off.
    let at = gtk::SpinButton::with_range(10.0, 95.0, 5.0);
    at.set_value(s.compact_at as f64);
    at.set_sensitive(s.compact_alert);
    at.set_tooltip_text(Some("Percent of the context"));
    at.connect_value_changed(|b| {
        let value = b.value() as u64;
        settings::update(|s| s.compact_at = value);
    });
    let on = switch(s.compact_alert, |s, on| s.compact_alert = on);
    let spin = at.clone();
    on.connect_active_notify(move |s| spin.set_sensitive(s.is_active()));
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    controls.append(&at);
    controls.append(&gtk::Label::new(Some("%")));
    controls.append(&on);
    row(&chat, "Remind me to compact", "Once the context is this full, a Compact button shows by the context meter, top right.", &controls);
    content.append(&chat);

    let resting = section("Idle agents");
    let idle = dropdown(IDLE, s.idle_minutes, |m| format!("After {m} min"), |s, m| s.idle_minutes = m);
    row(&resting, "Suspend idle agents", "Frees an unwatched agent's memory; opening it resumes the conversation.", &idle);
    let tokens = dropdown(REST_TOKENS, s.rest_compact_tokens, |t| format!("Over {}K tokens", t / 1000), |s, t| s.rest_compact_tokens = t);
    tokens.set_sensitive(s.rest_compact);
    let on = switch(s.rest_compact, |s, on| s.rest_compact = on);
    let t = tokens.clone();
    on.connect_active_notify(move |s| t.set_sensitive(s.is_active()));
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    controls.append(&tokens);
    controls.append(&on);
    let compact = row(
        &resting,
        "Compact before suspending",
        "Sums up its conversation first, so resuming it is quick and cheap. Never the open agent, nor one waiting on you.",
        &controls,
    );
    // Only when idle agents are suspended at all ("Never" is the first choice).
    compact.set_visible(s.idle_minutes != 0);
    idle.connect_selected_notify(move |d| compact.set_visible(d.selected() != 0));
    content.append(&resting);

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
    // Who sent the code: linked only when the user says so, here.
    let proposal = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let proposed = gtk::Label::builder().xalign(0.0).hexpand(true).wrap(true).build();
    proposed.add_css_class("small");
    let accept = flat_button("Link this account");
    proposal.append(&proposed);
    proposal.append(&accept);
    proposal.set_visible(false);
    card.append(&proposal);
    let candidate_id = Rc::new(std::cell::Cell::new(0i64));
    let chosen = candidate_id.clone();
    accept.connect_clicked(move |_| {
        let id = chosen.get();
        if id != 0 {
            settings::update(|s| s.telegram_user = id);
        }
    });
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
    row_stacked(
        &setup,
        "Your Telegram account",
        "Its user ID: typed here, or linked above after sending the bot the code. Only this account is obeyed; anyone else writing to the bot is ignored.",
        &user,
    );
    caption(&setup, "Messages pass through Telegram's servers. “Always allow” and ! commands stay in loom.");
    content.append(&setup);

    let bot_name = Rc::new(std::cell::RefCell::new(String::new()));
    let names = bot_name.clone();
    let (wd, wt, wdet, wl, wc, wu, wcard) = (dot.downgrade(), title.downgrade(), detail.downgrade(), link.downgrade(), code.downgrade(), user.downgrade(), card.downgrade());
    let (wp, wpl) = (proposal.downgrade(), proposed.downgrade());
    crate::telegram::watch(move |state| {
        let (Some(dot), Some(title), Some(detail), Some(link), Some(code), Some(user), Some(card), Some(proposal), Some(proposed)) =
            (wd.upgrade(), wt.upgrade(), wdet.upgrade(), wl.upgrade(), wc.upgrade(), wu.upgrade(), wcard.upgrade(), wp.upgrade(), wpl.upgrade())
        else {
            return false;
        };
        let candidate = match state {
            Link::Connected { candidate, .. } => candidate.clone(),
            _ => None,
        };
        proposal.set_visible(candidate.is_some());
        if let Some((id, name)) = &candidate {
            proposed.set_label(&format!("{name} ({id}) sent the code. Link it if this is you."));
            candidate_id.set(*id);
        }
        for class in ["suspended", "starting", "done", "failed"] {
            dot.remove_css_class(class);
        }
        card.remove_css_class("ok");
        card.remove_css_class("failed");
        let (class, heading, words, linking) = match state {
            Link::Off => ("suspended", "Off".to_owned(), "Switch it on to reach your agents from Telegram.".to_owned(), None),
            Link::NeedsToken => ("suspended", "Needs a bot token".to_owned(), "Paste it below.".to_owned(), None),
            Link::Connecting => ("starting", "Connecting…".to_owned(), String::new(), None),
            Link::Connected { bot, user, code, problem, .. } => {
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

    let other = section("Other editor");
    let editor = gtk::Entry::builder().text(&s.editor).placeholder_text("the system's default app").build();
    editor.add_css_class("mono");
    on_done(&editor, |text| settings::update(|s| s.editor = text));
    row_stacked(&other, "Command", "Ctrl+Shift+O opens the file there (code, gnome-text-editor…); empty uses the app your system opens it with.", &editor);
    content.append(&other);

    let program = section("Agent program");
    let entry = gtk::Entry::builder().text(&s.adapter).placeholder_text("empty: Claude Code itself").build();
    entry.add_css_class("mono");
    entry.connect_changed(|e| {
        let text = e.text().trim().to_owned();
        settings::update(|s| s.adapter = text);
    });
    let example = crate::provider::default_adapter().join(" ");
    row_stacked(
        &program,
        "ACP adapter",
        &format!("Empty: agents run Claude Code itself. An ACP adapter's command here runs them through it instead ({example}, or another ACP agent). For agents started from now on."),
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

/// Speech to text: the user's own service (Groq, OpenAI…) or a Whisper server of their own.
fn voice(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
    let voice = section("");
    row(&voice, "Use speech to text", "The microphone in the message box, and voice messages sent to the Telegram bot.", &switch(s.stt_enabled, |s, on| s.stt_enabled = on));
    let url = gtk::Entry::builder().text(&s.stt_url).placeholder_text("https://api.groq.com/openai/v1  ·  http://127.0.0.1:8000/v1").build();
    url.add_css_class("mono");
    on_done(&url, |text| settings::update(|s| s.stt_url = text));
    row_stacked(&voice, "Address", "An OpenAI-style transcription API: Groq, OpenAI, or a Whisper server of your own (speaches, LocalAI, whisper.cpp).", &url);
    let key = gtk::PasswordEntry::builder().text(&s.stt_key).show_peek_icon(true).placeholder_text("Empty for a local server").build();
    key.add_css_class("mono");
    on_done(&key, |text| settings::update(|s| s.stt_key = text));
    row_stacked(&voice, "API key", "", &key);
    let model = gtk::Entry::builder().text(&s.stt_model).placeholder_text("none").width_chars(22).build();
    model.add_css_class("mono");
    on_done(&model, |text| settings::update(|s| s.stt_model = text));
    row(&voice, "Model", "As the service names it (Groq: whisper-large-v3-turbo; OpenAI: whisper-1). A local server may need none.", &model);
    let language = gtk::Entry::builder().text(&s.stt_language).placeholder_text("fa, en…").width_chars(8).build();
    on_done(&language, |text| settings::update(|s| s.stt_language = text));
    row(&voice, "Language", "Empty: found from the speech. Or a code: fa, en, it…", &language);
    let test = flat_button("Test");
    let result = gtk::Label::builder().xalign(0.0).wrap(true).max_width_chars(40).build();
    result.add_css_class("dim");
    result.add_css_class("small");
    let shown = result.clone();
    test.connect_clicked(move |button| {
        // The fields just typed in count: saved first (they save on leaving them).
        button.grab_focus();
        shown.set_label("Sending a second of silence…");
        let shown = shown.clone();
        crate::stt::transcribe(crate::stt::silence(), "test.wav", "audio/wav", move |result| {
            shown.set_label(&match result {
                Ok(_) => "It works.".to_owned(),
                Err(why) => format!("It doesn't: {why}"),
            });
        });
    });
    let test_line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    test_line.append(&result);
    test_line.append(&test);
    row(&voice, "Check it", "Sends a second of silence to the address.", &test_line);
    caption(&voice, "What you record goes to the address above, and nowhere else.");
    content.append(&voice);
    content
}

/// The add-ons, each off until switched on on its own page.
fn addons(s: &Settings, list: &gtk::ListBox) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
    let all = section("");
    let entries: [(&str, &str, &str, bool); 3] = [
        ("notes", "Notes", "Keeps what you tell your agents in a vault of notes, by topic (finances, work, study…). The vault opens in Obsidian too.", s.notes_enabled),
        ("telegram", "Telegram", "Your agents in a Telegram bot of your own: their replies, your messages, their questions.", s.telegram_enabled),
        ("voice", "Speech to text", "Talk instead of typing, through a transcription service you choose.", s.stt_enabled),
    ];
    for (name, title, detail, on) in entries {
        let state = gtk::Label::new(None);
        state.add_css_class("small");
        let show = |state: &gtk::Label, on: bool| {
            state.set_label(if on { "On" } else { "Off" });
            if on {
                state.add_css_class("addon-on");
                state.remove_css_class("dim");
            } else {
                state.add_css_class("dim");
                state.remove_css_class("addon-on");
            }
        };
        show(&state, on);
        // Switched on or off on its page while this window is open: told here too.
        let weak = state.downgrade();
        settings::on_change(move |s| {
            if let Some(state) = weak.upgrade() {
                show(&state, match name {
                    "notes" => s.notes_enabled,
                    "telegram" => s.telegram_enabled,
                    _ => s.stt_enabled,
                });
            }
        });
        let open = flat_button("Open");
        let (list, page) = (list.clone(), name);
        open.connect_clicked(move |_| go_to(&list, page));
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        controls.append(&state);
        controls.append(&open);
        row(&all, title, detail, &controls);
    }
    caption(&all, "Each one is off until you switch it on, and keeps its settings on its own page.");
    content.append(&all);
    content
}

/// The Notes add-on: on or off, its vault, and its topics.
fn notes(s: &Settings, window: &gtk::Window) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
    let main = section("");
    let topics = gtk::Box::new(gtk::Orientation::Vertical, 22);

    let path = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::Start).build();
    let state = gtk::Label::builder().xalign(0.0).wrap(true).max_width_chars(60).build();
    state.add_css_class("dim");
    state.add_css_class("small");
    // The vault set up (when Notes is on), then what it is and its topics shown. Setting up
    // and counting notes read the disk: off the main thread.
    let show = {
        let (path, state, topics, window) = (path.clone(), state.clone(), topics.downgrade(), window.downgrade());
        move || {
            let s = settings::get();
            let vault = s.notes_vault.trim().to_owned();
            path.set_css_classes(if vault.is_empty() { &["dim"] } else { &["mono"] });
            if vault.is_empty() {
                path.set_label("None chosen");
                state.set_label("Choose an empty folder for a new vault, or a vault you have.");
            } else {
                path.set_label(&vault);
                state.set_label("…");
            }
            let (state, topics, window) = (state.clone(), topics.clone(), window.clone());
            glib::spawn_future_local(async move {
                let line = if vault.is_empty() {
                    None
                } else {
                    let enabled = s.notes_enabled;
                    gio::spawn_blocking(move || {
                        let folder = crate::state::expand(std::path::Path::new(&vault));
                        if !enabled {
                            return format!("{} Nothing is written there until Notes is on.", crate::vault::describe(&crate::vault::inspect(&folder)));
                        }
                        match crate::vault::prepare(&folder) {
                            Err(why) => format!("Could not set the vault up: {why}"),
                            Ok(()) => {
                                let notes = crate::vault::notes_in(&folder).iter().filter(|n| !n.strip_prefix(&folder).is_ok_and(|r| r.starts_with(crate::vault::TOPICS))).count();
                                format!("Ready: {notes} notes, {} topics. It opens in Obsidian too.", crate::topics::list(&folder).len())
                            }
                        }
                    })
                    .await
                    .ok()
                };
                if let Some(line) = line {
                    state.set_label(&line);
                }
                if let (Some(topics), Some(window)) = (topics.upgrade(), window.upgrade()) {
                    fill_topics(&topics, &window);
                }
            });
        }
    };
    let on = gtk::Switch::builder().active(s.notes_enabled).valign(gtk::Align::Center).build();
    let refresh = show.clone();
    on.connect_active_notify(move |on| {
        let active = on.is_active();
        settings::update(|s| s.notes_enabled = active);
        refresh();
    });
    row(&main, "Use Notes", "What you tell your agents is kept in the vault below, sorted by topic.", &on);

    let choose = flat_button("Choose…");
    let (w, refresh) = (window.clone(), show.clone());
    choose.connect_clicked(move |_| {
        let dialog = gtk::FileDialog::builder().title("A folder for the vault").modal(true).build();
        let current = settings::get().notes_vault;
        if !current.is_empty() {
            dialog.set_initial_folder(Some(&gio::File::for_path(crate::state::expand(std::path::Path::new(&current)))));
        }
        let (refresh, w2) = (refresh.clone(), w.clone());
        dialog.select_folder(Some(&w), None::<&gio::Cancellable>, move |result| {
            let Some(folder) = result.ok().and_then(|f| f.path()) else { return };
            if let Some(why) = crate::vault::unsuitable(&folder) {
                gtk::AlertDialog::builder().message("Not this folder").detail(why).modal(true).build().show(Some(&w2));
                return;
            }
            let chosen = folder.to_string_lossy().into_owned();
            let use_it = {
                let refresh = refresh.clone();
                move || {
                    settings::update(|s| s.notes_vault = chosen.clone());
                    refresh();
                }
            };
            // A folder that is neither empty nor a vault: said what loom adds, asked first.
            match crate::vault::inspect(&folder) {
                crate::vault::Folder::Other(_) => {
                    let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let ask = gtk::AlertDialog::builder()
                        .message(format!("Keep your notes in “{name}”?"))
                        .detail("It isn't an Obsidian vault yet. loom adds Topics/, .loom/ and .obsidian/ to it, and git for its own files; what is there now stays as it is.")
                        .buttons(["Cancel", "Use it"])
                        .cancel_button(0)
                        .default_button(1)
                        .modal(true)
                        .build();
                    ask.choose(Some(&w2), None::<&gio::Cancellable>, move |choice| {
                        if choice == Ok(1) {
                            use_it();
                        }
                    });
                }
                _ => use_it(),
            }
        });
    });
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    line.append(&path);
    line.append(&choose);
    let place = gtk::Box::new(gtk::Orientation::Vertical, 6);
    place.append(&line);
    place.append(&state);
    row_stacked(&main, "Vault", "A folder Obsidian can open as a vault. Your notes stay plain Markdown files there.", &place);
    let calls = gtk::SpinButton::with_range(20.0, 2000.0, 20.0);
    calls.set_value(s.notes_daily_calls as f64);
    calls.connect_value_changed(|b| {
        let value = b.value() as u64;
        settings::update(|s| s.notes_daily_calls = value);
    });
    row(&main, "Calls a day", "At most this many calls to Claude a day for notes (one per piece of a conversation read, plus one per topic it touches). Past it, checks wait for tomorrow.", &calls);
    let model = dropdown(&[("haiku".to_owned(), "Haiku"), ("sonnet".to_owned(), "Sonnet"), ("opus".to_owned(), "Opus")], s.scribe_model.clone(), Clone::clone, |s, v| s.scribe_model = v);
    row(&main, "Model", "What Scribe reads and writes your notes with. Haiku is the lightest on your usage limits; Sonnet and Opus take more of them each call. A topic whose file names a model (model: sonnet) uses that one.", &model);
    let effort = dropdown(
        &[(String::new(), "Default"), ("low".to_owned(), "Low"), ("medium".to_owned(), "Medium"), ("high".to_owned(), "High"), ("xhigh".to_owned(), "Extra high"), ("max".to_owned(), "Max")],
        s.scribe_effort.clone(),
        Clone::clone,
        |s, v| s.scribe_effort = v,
    );
    row(&main, "Effort", "How hard it thinks on each call (Default: Claude Code's own). Higher is slower and takes more of your usage limits.", &effort);
    caption(&main, "Your agents can search and read these notes, and Scribe keeps them up to date from your conversations (the Notes tab beside the chat shows what it wrote and asks).");
    // Corrections the user explained: rules in the topics, and tests kept on this machine.
    let corrections = crate::hub::corrections_path();
    let kept = std::fs::read_to_string(&corrections).map(|t| t.lines().filter(|l| !l.trim().is_empty()).count()).unwrap_or(0);
    if kept > 0 {
        caption(
            &main,
            &format!(
                "When you say why Scribe got something wrong, it becomes a rule in that topic (under Rules learned) and a test. {kept} kept: check Scribe still passes them with  loom scribe-eval {} <your vault>",
                corrections.display()
            ),
        );
    }
    content.append(&main);
    content.append(&topics);
    show();
    content
}

/// The Notes page's topics: each one's folder, what it listens for, how its changes apply,
/// with a switch and Edit; and a new one.
fn fill_topics(holder: &gtk::Box, window: &gtk::Window) {
    while let Some(child) = holder.first_child() {
        holder.remove(&child);
    }
    let group = section("Topics");
    let Some(vault) = crate::vault::current() else {
        caption(&group, "Topics show here once Notes is on and has a vault.");
        holder.append(&group);
        return;
    };
    let open = |file: std::path::PathBuf, w: &gtk::Window| {
        // While still attached to the app (closed, it no longer reaches its actions).
        let _ = w.activate_action("app.open-file", Some(&file.to_string_lossy().to_variant()));
        w.close();
    };
    let refill = {
        let (holder, window) = (holder.downgrade(), window.downgrade());
        move || {
            if let (Some(holder), Some(window)) = (holder.upgrade(), window.upgrade()) {
                fill_topics(&holder, &window);
            }
        }
    };
    for topic in crate::topics::list(&vault) {
        let listens = match topic.listens_for.len() {
            0 => "listens for nothing yet".to_owned(),
            n if n > 4 => format!("listens for {}…", topic.listens_for[..4].join(", ")),
            _ => format!("listens for {}", topic.listens_for.join(", ")),
        };
        let apply = match topic.apply {
            crate::topics::Apply::Ask => "asks before changing",
            crate::topics::Apply::Auto => "changes directly",
        };
        let sync = if topic.sync { "" } else { " · stays on this computer" };
        let detail = format!("{}/ · {listens} · {apply}{sync}", topic.folder);
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let edit = flat_button("Edit");
        let (file, w) = (topic.path.clone(), window.clone());
        edit.connect_clicked(move |_| open(file.clone(), &w));
        controls.append(&edit);
        let on = gtk::Switch::builder().active(topic.enabled).valign(gtk::Align::Center).build();
        let (now, refill) = (topic.clone(), refill.clone());
        on.connect_active_notify(move |on| {
            if on.is_active() != now.enabled && crate::topics::set_enabled(&now, on.is_active()).is_ok() {
                let refill = refill.clone();
                glib::idle_add_local_once(move || refill());
            }
        });
        controls.append(&on);
        row(&group, &topic.name, &detail, &controls);
    }
    let name = gtk::Entry::builder().placeholder_text("Surfing, Garden…").width_chars(16).build();
    let create = flat_button("Create");
    let problem = gtk::Label::builder().xalign(0.0).visible(false).build();
    problem.add_css_class("small");
    problem.add_css_class("error");
    let describe = gtk::Entry::builder().placeholder_text("Optional: what it keeps, in a sentence (I surf; track sessions, spots and boards)").hexpand(true).build();
    let make = {
        let (name, describe, create, problem, w, vault) = (name.clone(), describe.clone(), create.clone(), problem.clone(), window.clone(), vault.clone());
        move || {
            let said = describe.text().trim().to_owned();
            let topic = name.text().trim().to_owned();
            if said.is_empty() {
                match crate::topics::create(&vault, &topic) {
                    Ok(file) => open(file, &w),
                    Err(why) => {
                        problem.set_label(&why);
                        problem.set_visible(true);
                    }
                }
                return;
            }
            // Described: Claude drafts the topic's file, opened to adjust.
            if let Err(why) = crate::topics::new_file(&vault, &topic) {
                problem.set_label(&why);
                problem.set_visible(true);
                return;
            }
            problem.set_visible(false);
            create.set_sensitive(false);
            create.set_label("Drafting…");
            let (create, problem, w, vault) = (create.clone(), problem.clone(), w.clone(), vault.clone());
            glib::spawn_future_local(async move {
                let name = topic.clone();
                let drafted = gio::spawn_blocking(move || crate::scribe::describe_topic(&name, &said)).await;
                create.set_sensitive(true);
                create.set_label("Create");
                let made = match drafted {
                    Ok(Ok(text)) => crate::topics::create_with(&vault, &topic, &text),
                    Ok(Err(why)) => Err(format!("Not drafted: {why}")),
                    Err(_) => Err("Drafting stopped unexpectedly.".to_owned()),
                };
                match made {
                    Ok(file) => open(file, &w),
                    Err(why) => {
                        problem.set_label(&why);
                        problem.set_visible(true);
                    }
                }
            });
        }
    };
    let again = make.clone();
    create.connect_clicked(move |_| again());
    let again = make.clone();
    describe.connect_activate(move |_| again());
    name.connect_activate(move |_| make());
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    controls.append(&problem);
    controls.append(&name);
    controls.append(&create);
    row(&group, "New topic", "", &controls);
    row_stacked(&group, "Describe it", "Say what it should keep and Claude drafts the topic (one call); it opens for you to adjust. Empty: a blank topic.", &describe);
    caption(&group, "Each topic is a note in Topics/: its settings at the top, and below them, in your words, how its notes are kept.");
    holder.append(&group);
}

/// loom's own Claude folder: skills, agents and commands for loom's agents only.
fn claude(window: &gtk::Window) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 22);
    content.append(&holder);
    fill_claude(&holder, window);
    content
}

/// (Re)builds the Claude page's sections (after a change to the folder).
fn fill_claude(holder: &gtk::Box, window: &gtk::Window) {
    while let Some(child) = holder.first_child() {
        holder.remove(&child);
    }
    let refill = {
        let (holder, window) = (holder.downgrade(), window.downgrade());
        move || {
            if let (Some(holder), Some(window)) = (holder.upgrade(), window.upgrade()) {
                fill_claude(&holder, &window);
            }
        }
    };
    let edit = |file: std::path::PathBuf| {
        let w = window.clone();
        move |_: &gtk::Button| {
            // While still attached to the app (closed, it no longer reaches its actions).
            let _ = w.activate_action("app.open-file", Some(&file.to_string_lossy().to_variant()));
            w.close();
        }
    };

    let folder = section("Folder");
    let where_ = gtk::Label::builder().label(crate::plugin::dir().to_string_lossy()).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::Start).build();
    where_.add_css_class("mono");
    let open = flat_button("Open folder");
    open.connect_clicked(|button| {
        let window = button.root().and_downcast::<gtk::Window>();
        gtk::FileLauncher::new(Some(&gio::File::for_path(crate::plugin::dir()))).launch(window.as_ref(), None::<&gio::Cancellable>, |_| {});
    });
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    line.append(&where_);
    line.append(&open);
    row_stacked(&folder, "Where", "Skills, agents, commands and hooks only loom's agents use; claude in a terminal doesn't load them.", &line);
    caption(&folder, "Changes reach agents started or woken from now on.");
    holder.append(&folder);

    for kind in crate::plugin::Kind::ALL {
        let group = section(kind.title());
        for item in crate::plugin::items(kind) {
            let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            if item.shipped {
                let restore = flat_button("Restore");
                restore.set_tooltip_text(Some("Put back loom's own version"));
                let (name, refill) = (item.name.clone(), refill.clone());
                restore.connect_clicked(move |_| {
                    if crate::plugin::restore(&name).is_ok() {
                        crate::plugin::changed();
                        refill();
                    }
                });
                controls.append(&restore);
            }
            let edit_button = flat_button("Edit");
            edit_button.set_sensitive(item.enabled);
            edit_button.connect_clicked(edit(item.file.clone()));
            controls.append(&edit_button);
            let on = gtk::Switch::builder().active(item.enabled).valign(gtk::Align::Center).build();
            let (item_now, refill) = (item.clone(), refill.clone());
            on.connect_active_notify(move |on| {
                if on.is_active() != item_now.enabled && crate::plugin::set_enabled(&item_now, on.is_active()).is_ok() {
                    crate::plugin::changed();
                    // Rebuilt after the switch has finished moving.
                    let refill = refill.clone();
                    glib::idle_add_local_once(move || refill());
                }
            });
            controls.append(&on);
            row(&group, &item.name, &short(&item.description), &controls);
        }
        // A new one: its name, then its file opens in loom's editor.
        let name = gtk::Entry::builder().placeholder_text("name").width_chars(16).build();
        name.add_css_class("mono");
        let create = flat_button("Create");
        let problem = gtk::Label::builder().xalign(0.0).visible(false).build();
        problem.add_css_class("small");
        problem.add_css_class("error");
        let make = {
            let (name, problem, w) = (name.clone(), problem.clone(), window.clone());
            move || match crate::plugin::create(kind, &name.text()) {
                Ok(file) => {
                    crate::plugin::changed();
                    let _ = w.activate_action("app.open-file", Some(&file.to_string_lossy().to_variant()));
                    w.close();
                }
                Err(why) => {
                    problem.set_label(&why);
                    problem.set_visible(true);
                }
            }
        };
        let again = make.clone();
        create.connect_clicked(move |_| again());
        name.connect_activate(move |_| make());
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        controls.append(&problem);
        controls.append(&name);
        controls.append(&create);
        row(&group, &format!("New {}", kind.one()), "", &controls);
        holder.append(&group);
    }

    let hooks = section("Hooks");
    let edit_hooks = flat_button("Edit");
    edit_hooks.connect_clicked(edit(crate::plugin::hooks_file()));
    row(&hooks, "hooks.json", "Commands run on Claude Code's events (a tool used, a turn ended), for loom's agents only.", &edit_hooks);
    holder.append(&hooks);

    // What Claude Code does without asking: the rules "Yes, and don't ask again" added.
    let rules = section("Allowed without asking");
    let found = allow_rules();
    if found.is_empty() {
        caption(&rules, "No rules yet. \"Yes, and don't ask again\" on a question adds one.");
    }
    for (file, place, rule) in found {
        let remove = flat_button("Remove");
        remove.set_tooltip_text(Some(&format!("Claude asks again before this ({})", file.display())));
        let (file_now, rule_now, holder_now, window_now) = (file.clone(), rule.clone(), holder.clone(), window.clone());
        remove.connect_clicked(move |_| {
            if remove_allow_rule(&file_now, &rule_now).is_ok() {
                fill_claude(&holder_now, &window_now);
            }
        });
        row(&rules, &rule, &place, &remove);
    }
    holder.append(&rules);
}

/// Claude Code's settings files with rules: the user's, and each of loom's projects' local one.
fn rule_files() -> Vec<(std::path::PathBuf, String)> {
    let mut files = vec![(crate::state::claude_dir().join("settings.json"), "Everywhere (your Claude Code settings)".to_owned())];
    let app_id = gtk::gio::Application::default().and_then(|a| a.application_id()).map(|id| id.to_string()).unwrap_or_default();
    let layout = crate::state::load(&app_id);
    // The projects' folders and the agents' (one may work in a repo or folder inside a project).
    let mut folders: Vec<std::path::PathBuf> = layout.projects.iter().map(|p| p.path.clone()).collect();
    folders.extend(layout.agents.iter().map(|a| a.cwd.clone()));
    folders.sort();
    folders.dedup();
    for folder in folders {
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        files.push((folder.join(".claude/settings.local.json"), format!("In {name} (yours)")));
        files.push((folder.join(".claude/settings.json"), format!("In {name} (shared)")));
    }
    files
}

/// Every allow rule: (its file, where it applies, the rule).
fn allow_rules() -> Vec<(std::path::PathBuf, String, String)> {
    let mut all = Vec::new();
    for (file, place) in rule_files() {
        let Some(settings) = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { continue };
        for rule in settings["permissions"]["allow"].as_array().into_iter().flatten().filter_map(|r| r.as_str()) {
            all.push((file.clone(), place.clone(), rule.to_owned()));
        }
    }
    all
}

/// Takes `rule` out of the file's allow list (nothing else of the file is touched).
fn remove_allow_rule(file: &std::path::Path, rule: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let mut settings: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let Some(allow) = settings["permissions"]["allow"].as_array_mut() else { return Ok(()) };
    allow.retain(|r| r.as_str() != Some(rule));
    let out = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(file, out + "\n").map_err(|e| e.to_string())
}

/// A description as a row shows it: the first sentence, at most two lines' worth.
fn short(description: &str) -> String {
    if description.is_empty() {
        return "No description.".to_owned();
    }
    let first = description.split_inclusive(". ").next().unwrap_or(description).trim_end();
    let first = first.split(" (").next().unwrap_or(first);
    if first.chars().count() <= 110 {
        return first.to_owned();
    }
    let cut: String = first.chars().take(108).collect();
    format!("{}…", cut.trim_end())
}
