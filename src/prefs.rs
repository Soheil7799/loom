//! The settings window (Ctrl+,): General, Appearance, Agents, Claude, the add-ons (Notes,
//! Telegram, Remote, Speech to text; each off until switched on) and Advanced. Every change is
//! saved at once; Esc closes it.

use std::rc::Rc;

use gtk::{gdk, gio, glib, pango, prelude::*};

use crate::settings::{self, Settings};
use crate::bridges::Link;

/// The settings as last read (loomd writes them; this window shows you your own).
fn current() -> Settings {
    settings::get()
}

/// A change made in this window, made through the core API (`set_settings`), as any client
/// makes one.
pub(crate) fn update(change: impl FnOnce(&mut Settings)) {
    let old = current();
    let mut new = old.clone();
    change(&mut new);
    let changes = loom_core::settings::changes(&old, &new);
    if changes.as_object().is_none_or(|c| c.is_empty()) {
        return;
    }
    crate::conn::command("settings", "set_settings", serde_json::json!({ "changes": changes }));
}

const MODES: &[(&str, &str)] = &[
    ("", "Claude Code's default"),
    ("default", "Ask before acting"),
    ("acceptEdits", "Accept edits"),
    ("plan", "Plan only"),
    ("auto", "Auto"),
];

const SCALES: &[(f64, &str)] = &[(0.9, "Small"), (1.0, "Normal"), (1.1, "Large"), (1.25, "Larger"), (1.4, "Largest")];

/// Meaning search's models (embed.rs words the questions for each).
const EMBED_MODELS: &[(&str, &str)] = &[("embeddinggemma", "embeddinggemma (best on code in loom's tests)"), ("qwen3-embedding:0.6b", "qwen3-embedding 0.6B")];

const COMPACT_TOKENS: &[(u64, &str)] = &[(100_000, "At 100K tokens"), (150_000, "At 150K tokens"), (200_000, "At 200K tokens"), (300_000, "At 300K tokens"), (500_000, "At 500K tokens")];
const REST_TOKENS: &[(u64, &str)] = &[(50_000, "Over 50K tokens"), (100_000, "Over 100K tokens"), (200_000, "Over 200K tokens"), (400_000, "Over 400K tokens")];
const IDLE: &[(u64, &str)] = &[(0, "Never"), (5, "After 5 min"), (15, "After 15 min"), (30, "After 30 min"), (45, "After 45 min"), (60, "After 1 hour"), (120, "After 2 hours")];

/// The pages: (name, title, icon, an add-on's: listed under Add-ons).
const PAGES: &[(&str, &str, &str, bool)] = &[
    ("general", "General", "preferences-system-symbolic", false),
    ("appearance", "Appearance", "applications-graphics-symbolic", false),
    ("agents", "Agents", "system-users-symbolic", false),
    ("claude", "Claude", "applications-engineering-symbolic", false),
    ("addons", "Add-ons", "application-x-addon-symbolic", false),
    ("notes", "Notes", "accessories-text-editor-symbolic", true),
    ("telegram", "Telegram", "mail-send-symbolic", true),
    ("remote", "Remote", "phone-symbolic", true),
    ("voice", "Speech to text", "audio-input-microphone-symbolic", true),
    ("advanced", "Advanced", "utilities-terminal-symbolic", false),
];

/// The nav row of page `name` selected (its page shown).
fn go_to(list: &gtk::ListBox, name: &str) {
    if let Some(index) = PAGES.iter().position(|(n, ..)| *n == name) {
        list.select_row(list.row_at_index(index as i32).as_ref());
    }
}

pub fn open(parent: &impl IsA<gtk::Window>) {
    let s = current();
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
    stack.add_named(&page("General", &general(&s)), Some("general"));
    stack.add_named(&page("Appearance", &appearance(&s)), Some("appearance"));
    stack.add_named(&page("Agents", &agents(&s, &window)), Some("agents"));
    stack.add_named(&page("Claude", &claude(&window)), Some("claude"));
    stack.add_named(&page("Add-ons", &addons(&s, &list)), Some("addons"));
    stack.add_named(&page("Notes", &notes(&s, &window)), Some("notes"));
    stack.add_named(&page("Telegram", &telegram(&s)), Some("telegram"));
    stack.add_named(&page("Remote", &remote(&s)), Some("remote"));
    stack.add_named(&page("Speech to text", &voice(&s)), Some("voice"));
    stack.add_named(&page("Advanced", &advanced(&s)), Some("advanced"));

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
    let close = gtk::Button::builder().icon_name("window-close-symbolic").tooltip_text("Close (Esc)").halign(gtk::Align::End).valign(gtk::Align::Start).build();
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
        update(|settings| set(settings, on));
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
            update(|s| set(s, value));
        }
    });
    dropdown
}

thread_local! {
    /// The session settings an agent last showed (configOptions): what the defaults offer.
    static KNOWN_OPTIONS: std::cell::RefCell<serde_json::Value> = const { std::cell::RefCell::new(serde_json::Value::Null) };
}

/// An agent's session settings, seen: the defaults offer their choices.
pub fn seen_options(options: &serde_json::Value) {
    if options.as_array().is_some_and(|o| !o.is_empty()) {
        KNOWN_OPTIONS.with_borrow_mut(|known| *known = options.clone());
    }
}

/// An option's values (flat or in groups): (value, name).
fn option_values(options: &serde_json::Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in options.as_array().into_iter().flatten() {
        match entry["options"].as_array() {
            Some(group) => out.extend(option_values(&serde_json::Value::Array(group.clone()))),
            None => {
                if let Some(value) = entry["value"].as_str() {
                    out.push((value.to_owned(), entry["name"].as_str().unwrap_or(value).to_owned()));
                }
            }
        }
    }
    out
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

fn general(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);

    let notifications = section("Notifications");
    row(&notifications, "When an agent asks for permission", "", &switch(s.notify_questions, |s, on| s.notify_questions = on));
    row(&notifications, "When an agent finishes a turn", "", &switch(s.notify_done, |s, on| s.notify_done = on));
    caption(&notifications, "Only about agents you aren't looking at.");
    content.append(&notifications);

    let sidebar = section("Sidebar");
    row(
        &sidebar,
        "Waiting agents first",
        "One waiting for your answer moves to the top of its project until answered.",
        &switch(s.sort_by_urgency, |s, on| s.sort_by_urgency = on),
    );
    row(
        &sidebar,
        "Running section open",
        "Agents running now, above the projects. Folded, its header still counts who needs you.",
        &switch(s.show_running, |s, on| s.show_running = on),
    );
    row(&sidebar, "Ctrl+Tab skips suspended agents", "So passing one doesn't wake it.", &switch(s.cycle_running, |s, on| s.cycle_running = on));
    content.append(&sidebar);

    let lifetime = section("Starting and closing");
    let start = dropdown(&[(true, "The overview"), (false, "The last agent")], s.start_on_overview, |_| String::new(), |s, on| s.start_on_overview = on);
    row(&lifetime, "When loom starts", "The overview shows every agent at a glance and wakes none until you open one.", &start);
    // loomd's client: what closing the window does to loomd's agents.
    row(
        &lifetime,
        "Keep agents working when the window closes",
        "Agents, jobs, bridges and scheduled prompts keep running in the background. Off: closing the window stops them; conversations go on next time.",
        &switch(s.keep_working, |s, on| s.keep_working = on),
    );
    row(&lifetime, "Ask before quitting while agents work", "", &switch(s.confirm_quit, |s, on| s.confirm_quit = on));
    content.append(&lifetime);

    let tools = section("Editor and terminal");
    let editor = gtk::Entry::builder().text(&s.editor).placeholder_text("the system's default app").build();
    editor.add_css_class("mono");
    on_done(&editor, |text| update(|s| s.editor = text));
    row_stacked(&tools, "Other editor", "The command Ctrl+Shift+O opens files with (code, gnome-text-editor…); empty uses the app your system opens them with.", &editor);
    row(&tools, "Hide the scratch terminal when it loses focus", "", &switch(s.scratch_autohide, |s, on| s.scratch_autohide = on));
    content.append(&tools);
    content
}

fn agents(s: &Settings, window: &gtk::Window) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);

    let defaults = section("Defaults");
    let modes: Vec<(String, &str)> = MODES.iter().map(|(id, name)| ((*id).to_owned(), *name)).collect();
    let mode = dropdown(&modes, s.default_mode.clone(), |m| m.clone(), |s, m| s.default_mode = m);
    row(&defaults, "Permission mode", "What a new agent may do without asking. Shift+Tab changes it in an agent.", &mode);

    // The session's settings (model, effort…), as Claude Code offers them: known once an agent
    // has started.
    let known = KNOWN_OPTIONS.with_borrow(Clone::clone);
    let options: Vec<&serde_json::Value> = known.as_array().into_iter().flatten().filter(|o| o["id"].as_str().is_some_and(|id| id != "mode")).collect();
    caption(
        &defaults,
        if options.is_empty() {
            "The model, the effort and the rest show here once an agent has started."
        } else {
            "The model, the effort and the rest apply to every agent, new or woken. Changed in an agent, a value stays there (its chip tinted) until set back to this one."
        },
    );
    for option in options {
        let Some(id) = option["id"].as_str().map(str::to_owned) else { continue };
        let mut choices: Vec<(String, String)> = vec![(String::new(), "Claude Code's default".to_owned())];
        choices.extend(option_values(&option["options"]));
        let choices: Vec<(String, &str)> = choices.iter().map(|(v, n)| (v.clone(), n.as_str())).collect();
        let current = s.default_options.get(&id).cloned().unwrap_or_default();
        let key = id.clone();
        let pick = dropdown(&choices, current, |v| v.clone(), move |s, v| {
            if v.is_empty() {
                s.default_options.remove(&key);
            } else {
                s.default_options.insert(key.clone(), v);
            }
        });
        let name = option["name"].as_str().unwrap_or(&id).to_owned();
        row(&defaults, &name, "", &pick);
    }
    content.append(&defaults);

    let chat = section("Conversation");
    row(&chat, "Show thinking", "Claude's reasoning, folded under each answer.", &switch(s.show_thinking, |s, on| s.show_thinking = on));
    row(&chat, "Suggest your next message", "Shown after each turn; Tab or the Right arrow fills it in.", &switch(s.suggest_prompts, |s, on| s.suggest_prompts = on));
    // How much, then on or off.
    let at = dropdown(COMPACT_TOKENS, s.compact_tokens, |t| format!("At {}K tokens", t / 1000), |s, t| s.compact_tokens = t);
    at.set_sensitive(s.compact_alert);
    let on = switch(s.compact_alert, |s, on| s.compact_alert = on);
    let spin = at.clone();
    on.connect_active_notify(move |s| spin.set_sensitive(s.is_active()));
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    controls.append(&at);
    controls.append(&on);
    row(&chat, "Remind me to compact", "Long conversations cost more each turn. Past this size, the context meter turns amber and shows a Compact button.", &controls);
    content.append(&chat);

    let resting = section("Background");
    let idle = dropdown(IDLE, s.idle_minutes, |m| format!("After {m} min"), |s, m| s.idle_minutes = m);
    row(&resting, "Suspend idle agents", "Frees an unwatched agent's memory. Opening it shows its conversation; it wakes with your next message.", &idle);
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
        "Sums up its conversation first, so waking it is quick and cheap. Never the open agent, nor one waiting on you.",
        &controls,
    );
    // Only when idle agents are suspended at all ("Never" is the first choice).
    compact.set_visible(s.idle_minutes != 0);
    idle.connect_selected_notify(move |d| compact.set_visible(d.selected() != 0));
    let open = flat_button("Open the overview");
    let w = window.clone();
    open.connect_clicked(move |_| {
        // While still attached to the app (closed, it no longer reaches its actions).
        let _ = w.activate_action("app.show-overview", None);
        w.close();
    });
    row(&resting, "Scheduled prompts", "Set from an agent's menu; listed and paused in the overview (Ctrl+Shift+A).", &open);
    content.append(&resting);
    content
}

fn appearance(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
    let colours = section("Colours");
    let mut themes: Vec<(String, &str)> = vec![("system".to_owned(), "System (your GTK theme)")];
    themes.extend(crate::theme::THEMES.iter().map(|(id, name, ..)| ((*id).to_owned(), if *id == "nocturne" { "Nocturne (loom's own)" } else { *name })));
    let theme = dropdown(&themes, s.theme.clone(), |t| t.clone(), |s, t| s.theme = t);
    row(&colours, "Theme", "System uses your GTK theme's colours and follows it live.", &theme);
    let modes: Vec<(String, &str)> = vec![(String::new(), "System"), ("dark".to_owned(), "Dark"), ("light".to_owned(), "Light")];
    row(&colours, "Dark or light", "", &dropdown(&modes, s.theme_mode.clone(), |m| m.clone(), |s, m| s.theme_mode = m));
    content.append(&colours);
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
            update(|s| s.terminal_font = name);
        }
    });
    row(&text, "Terminal font", "The scratch terminal and the tray's terminals.", &font);
    content.append(&text);
    content
}

fn telegram(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);

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
            update(|s| s.telegram_user = id);
        }
    });
    content.append(&card);

    let setup = section("Setup");
    let token = gtk::PasswordEntry::builder().text(&s.telegram_token).show_peek_icon(true).placeholder_text("123456:ABC…").build();
    token.add_css_class("mono");
    on_done(&token, |text| update(|s| s.telegram_token = text));
    let botfather = gtk::Button::with_label("Open BotFather");
    botfather.set_tooltip_text(Some("t.me/BotFather, in Telegram: /newbot makes the bot"));
    botfather.connect_clicked(|b| gtk::UriLauncher::new("https://t.me/BotFather").launch(b.root().and_downcast_ref::<gtk::Window>(), None::<&gio::Cancellable>, |_| {}));
    let token_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    token.set_hexpand(true);
    token_line.append(&token);
    token_line.append(&botfather);
    row_stacked(&setup, "Bot token", "Make a bot with @BotFather in Telegram (/newbot) and paste the token it gives.", &token_line);
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
        update(|s| s.telegram_user = id);
    });
    row_stacked(
        &setup,
        "Your Telegram account",
        "Its user ID, linked with the code above or typed here. The bot ignores everyone else.",
        &user,
    );
    caption(&setup, "Messages pass through Telegram's servers. “Always allow” and ! commands stay in loom.");
    content.append(&setup);

    let bot_name = Rc::new(std::cell::RefCell::new(String::new()));
    let names = bot_name.clone();
    let (wd, wt, wdet, wl, wc, wu, wcard) = (dot.downgrade(), title.downgrade(), detail.downgrade(), link.downgrade(), code.downgrade(), user.downgrade(), card.downgrade());
    let (wp, wpl) = (proposal.downgrade(), proposed.downgrade());
    watch_bridge("telegram", move |state| {
        let state = &Link::from_json(state);
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
        let id = current().telegram_user;
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

/// The remote bridge: your agents on your other devices, over iroh. Pairing shows a QR code
/// (this computer's address and a one-time code); a device that uses it is added only when
/// the user says yes here.
fn remote(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);

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
    let on = switch(s.remote_enabled, |s, on| s.remote_enabled = on);
    on.set_valign(gtk::Align::Center);
    head.append(&dot);
    head.append(&words);
    head.append(&on);
    card.append(&head);

    // Pairing: the code to scan, or the ticket to paste into loom-remote.
    let pairing = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let qr = gtk::DrawingArea::builder().content_width(220).content_height(220).valign(gtk::Align::Start).build();
    let ticket = Rc::new(std::cell::RefCell::new(String::new()));
    let drawn = ticket.clone();
    qr.set_draw_func(move |_, cr, width, height| draw_qr(cr, width, height, &drawn.borrow()));
    let side = gtk::Box::new(gtk::Orientation::Vertical, 8);
    side.set_hexpand(true);
    let how = gtk::Label::builder()
        .label("Scan this with the loom app, or paste the ticket into: loom-remote pair <ticket>. You confirm the device here before it is added.")
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(40)
        .build();
    how.add_css_class("small");
    let text = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).max_width_chars(30).build();
    text.add_css_class("mono");
    text.add_css_class("small");
    text.add_css_class("dim");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let copy = flat_button("Copy ticket");
    let cancel = flat_button("Cancel");
    buttons.append(&copy);
    buttons.append(&cancel);
    side.append(&how);
    side.append(&text);
    side.append(&buttons);
    pairing.append(&qr);
    pairing.append(&side);
    pairing.set_visible(false);
    card.append(&pairing);
    let shown = ticket.clone();
    copy.connect_clicked(move |button| button.clipboard().set_text(&shown.borrow()));
    cancel.connect_clicked(|_| open_pairing(false));

    // The device that used the code: added only when the user says so, here.
    let proposal = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let proposed = gtk::Label::builder().xalign(0.0).hexpand(true).wrap(true).build();
    proposed.add_css_class("small");
    let accept = flat_button("Pair it");
    let refuse = flat_button("Refuse");
    proposal.append(&proposed);
    proposal.append(&accept);
    proposal.append(&refuse);
    proposal.set_visible(false);
    card.append(&proposal);
    let candidate = Rc::new(std::cell::RefCell::new((String::new(), String::new())));
    let chosen = candidate.clone();
    accept.connect_clicked(move |_| {
        let id = chosen.borrow().0.clone();
        if !id.is_empty() {
            accept_device(&id);
        }
    });
    refuse.connect_clicked(|_| open_pairing(false));
    content.append(&card);

    let devices = section("Devices");
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    if let Some(rows) = rows_of(&devices) {
        rows.append(&list);
    }
    let pair = flat_button("Show code");
    pair.connect_clicked(|_| open_pairing(true));
    row(&devices, "Pair a device", "The code works for ten minutes.", &pair);
    caption(&devices, "A paired device can do what the Telegram bot can: read replies, send messages, answer questions. “Always allow” and ! commands stay in loom.");
    content.append(&devices);

    let connection = section("Connection");
    let relay = gtk::Entry::builder().text(&s.remote_relay).placeholder_text("empty: n0's public relays").build();
    relay.add_css_class("mono");
    on_done(&relay, |text| update(|s| s.remote_relay = text));
    row_stacked(
        &connection,
        "Relay",
        "Helps your devices find each other, and passes their traffic when they can't connect directly. Your own (iroh-relay) or, empty, the public ones of n0, who make iroh.",
        &relay,
    );
    caption(&connection, "Everything is end-to-end encrypted between your devices: a relay passes it on but can't read it. On the same network, devices connect directly.");
    content.append(&connection);

    // The devices, with which are connected now: redrawn on every change.
    let online = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let fill = {
        let (list, online) = (list.downgrade(), online.clone());
        Rc::new(move || {
            let Some(list) = list.upgrade() else { return false };
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            for (id, name) in current().remote_devices {
                let here = online.borrow().contains(&id);
                let short: String = id.chars().take(10).collect();
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
                line.add_css_class("prefs-row");
                line.append(&texts(&name, &format!("{short}…{}", if here { " · connected now" } else { "" })));
                let remove = flat_button("Remove");
                remove.set_valign(gtk::Align::Center);
                remove.connect_clicked(move |_| update(|s| s.remote_devices.retain(|(d, _)| *d != id)));
                line.append(&remove);
                list.append(&line);
            }
            true
        })
    };
    fill();
    let refill = fill.clone();
    settings::on_change_while(move |_| refill());

    let (wd, wt, wdet, wp, wq, wtext, wprop, wproposed, wcard, wpair) =
        (dot.downgrade(), title.downgrade(), detail.downgrade(), pairing.downgrade(), qr.downgrade(), text.downgrade(), proposal.downgrade(), proposed.downgrade(), card.downgrade(), pair.downgrade());
    watch_bridge("remote", move |state| {
        let (Some(dot), Some(title), Some(detail), Some(pairing), Some(qr), Some(text), Some(proposal), Some(proposed), Some(card), Some(pair)) =
            (wd.upgrade(), wt.upgrade(), wdet.upgrade(), wp.upgrade(), wq.upgrade(), wtext.upgrade(), wprop.upgrade(), wproposed.upgrade(), wcard.upgrade(), wpair.upgrade())
        else {
            return false;
        };
        let str_of = |key: &str| state[key].as_str().unwrap_or("").to_owned();
        for class in ["suspended", "starting", "done", "failed"] {
            dot.remove_css_class(class);
        }
        card.remove_css_class("ok");
        card.remove_css_class("failed");
        let up = state["state"] == "online";
        let (class, heading, words) = match state["state"].as_str().unwrap_or("off") {
            "off" => ("suspended", "Off".to_owned(), "Switch it on to reach your agents from your other devices.".to_owned()),
            "connecting" => ("starting", "Starting…".to_owned(), String::new()),
            "online" => {
                card.add_css_class("ok");
                let relay = state["relay"].as_str().map_or_else(|| "no relay yet".to_owned(), |r| format!("through {r}"));
                let mut words = format!("This computer is {}…, {relay}.", str_of("id").chars().take(10).collect::<String>());
                if let Some(problem) = state["problem"].as_str() {
                    words.push(' ');
                    words.push_str(problem);
                }
                ("done", "Online".to_owned(), words)
            }
            _ => {
                card.add_css_class("failed");
                ("failed", "Can't start".to_owned(), str_of("why"))
            }
        };
        dot.add_css_class(class);
        title.set_label(&heading);
        detail.set_label(&words);
        detail.set_visible(!words.is_empty());
        // A ticket without the relay would be good on this network only.
        pair.set_sensitive(up && (!state["relay"].is_null() || !state["problem"].is_null()));

        let shown = str_of("ticket");
        pairing.set_visible(up && !shown.is_empty());
        if *ticket.borrow() != shown {
            text.set_label(&shown);
            *ticket.borrow_mut() = shown;
            qr.queue_draw();
        }
        let asking = state["candidate"].is_object();
        proposal.set_visible(up && asking);
        if asking {
            let (id, name) = (state["candidate"]["id"].as_str().unwrap_or("").to_owned(), state["candidate"]["name"].as_str().unwrap_or("").to_owned());
            proposed.set_label(&format!("“{name}” ({}…) used the code. Pair it if it is yours.", id.chars().take(10).collect::<String>()));
            *candidate.borrow_mut() = (id, name);
        }
        let now: Vec<String> = state["online"].as_array().into_iter().flatten().filter_map(|i| i.as_str().map(str::to_owned)).collect();
        if *online.borrow() != now {
            *online.borrow_mut() = now;
            fill();
        }
        true
    });
    content
}

/// `text` as a QR code, dark on light whatever the theme (scanners want that), centred.
fn draw_qr(cr: &gtk::cairo::Context, width: i32, height: i32, text: &str) {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else { return };
    let modules = code.width();
    // Two modules of quiet zone around it.
    let size = width.min(height) as f64;
    let cell = (size / (modules + 4) as f64).floor().max(1.0);
    let side = cell * (modules + 4) as f64;
    let (x0, y0) = (((width as f64 - side) / 2.0).floor(), ((height as f64 - side) / 2.0).floor());
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.rectangle(x0, y0, side, side);
    let _ = cr.fill();
    cr.set_source_rgb(0.0, 0.0, 0.0);
    for (i, color) in code.to_colors().iter().enumerate() {
        if *color == qrcode::Color::Dark {
            let (x, y) = ((i % modules + 2) as f64, (i / modules + 2) as f64);
            cr.rectangle(x0 + x * cell, y0 + y * cell, cell, cell);
        }
    }
    let _ = cr.fill();
}

fn advanced(s: &Settings) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);

    let meaning = section("Meaning search");
    row(
        &meaning,
        "Search code by meaning",
        "Agents' loom_find ranks code by what a question means, not only its words. Runs on this machine through Ollama.",
        &switch(s.embed_code, |s, on| s.embed_code = on),
    );
    row(&meaning, "Search notes by meaning", "", &switch(s.embed_notes, |s, on| s.embed_notes = on));
    let models: Vec<(String, &str)> = EMBED_MODELS.iter().map(|(id, name)| ((*id).to_owned(), *name)).collect();
    let model = dropdown(&models, s.embed_model.clone(), |m| m.clone(), |s, m| s.embed_model = m);
    row(&meaning, "Model", "Changing it indexes code and notes again, once (a few minutes; the old index is kept).", &model);
    row_stacked(&meaning, "Ollama", "loom starts Ollama when nothing answers and downloads the model when it is missing.", &embeddings_status());
    content.append(&meaning);

    let browser = section("Agent tools");
    // The browsers found here, by program; the one set stays listed even if it is gone.
    let found = loom_core::browser::found();
    let mut choices: Vec<(String, String)> = vec![(String::new(), match found.first() {
        Some((name, _)) => format!("Automatic ({name})"),
        None => "Automatic (none found)".to_owned(),
    })];
    choices.extend(found.iter().map(|(name, path)| (path.to_string_lossy().into_owned(), name.clone())));
    let named: Vec<(String, &str)> = choices.iter().map(|(v, n)| (v.clone(), n.as_str())).collect();
    let pick = dropdown(&named, s.browser.clone(), |set| format!("{set} (not found)"), |s, path| s.browser = path);
    row(
        &browser,
        "Browser agents use",
        "Any Chromium-based browser, with a profile of its own per project: yours and your logins are never touched. It shows in the project's tray (headed when Xvfb is installed).",
        &pick,
    );
    let devtools = switch(s.browser_devtools, |s, on| s.browser_devtools = on);
    row(
        &browser,
        "Use Chrome DevTools MCP",
        "Off: agents use agent-browser, which costs fewer tokens. On: Chrome's DevTools MCP, with network, console and performance tools.",
        &devtools,
    );
    let strict = switch(s.strict_sandbox, |s, on| s.strict_sandbox = on);
    let can = loom_core::sandbox::available();
    strict.set_sensitive(can || s.strict_sandbox);
    row(
        &browser,
        "Strict sandbox",
        if can {
            "Agents and their jobs reach no local socket except loom's, SSH's and GPG's, so nothing starts outside loom's sight. They lose sudo, systemctl --user, notifications, gsettings, podman and portals: run those in the tray."
        } else {
            "Needs Linux's Landlock with Unix socket rules, which this kernel lacks."
        },
        &strict,
    );
    caption(&browser, "Changes reach agents and browsers started from now on.");
    content.append(&browser);

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
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
    let voice = section("");
    row(&voice, "Use speech to text", "The microphone in the message box, and voice messages sent to the Telegram bot.", &switch(s.stt_enabled, |s, on| s.stt_enabled = on));
    let url = gtk::Entry::builder().text(&s.stt_url).placeholder_text("https://api.groq.com/openai/v1  ·  http://127.0.0.1:8000/v1").build();
    url.add_css_class("mono");
    on_done(&url, |text| update(|s| s.stt_url = text));
    row_stacked(&voice, "Address", "An OpenAI-style transcription API: Groq, OpenAI, or a Whisper server of your own (speaches, LocalAI, whisper.cpp).", &url);
    let key = gtk::PasswordEntry::builder().text(&s.stt_key).show_peek_icon(true).placeholder_text("Empty for a local server").build();
    key.add_css_class("mono");
    on_done(&key, |text| update(|s| s.stt_key = text));
    row_stacked(&voice, "API key", "", &key);
    let model = gtk::Entry::builder().text(&s.stt_model).placeholder_text("none").width_chars(22).build();
    model.add_css_class("mono");
    on_done(&model, |text| update(|s| s.stt_model = text));
    row(&voice, "Model", "As the service names it (Groq: whisper-large-v3-turbo; OpenAI: whisper-1). A local server may need none.", &model);
    let language = gtk::Entry::builder().text(&s.stt_language).placeholder_text("fa, en…").width_chars(8).build();
    on_done(&language, |text| update(|s| s.stt_language = text));
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
    row(&voice, "Test the service", "Sends a second of silence to the address.", &test_line);
    caption(&voice, "What you record goes to the address above, and nowhere else.");
    content.append(&voice);
    content
}

/// The add-ons, each off until switched on on its own page.
fn addons(s: &Settings, list: &gtk::ListBox) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
    let all = section("");
    let entries: [(&str, &str, &str, bool); 4] = [
        ("notes", "Notes", "Keeps what you tell your agents in a vault of notes, by topic (finances, work, study…). The vault opens in Obsidian too.", s.notes_enabled),
        ("telegram", "Telegram", "Your agents in a Telegram bot of your own: their replies, your messages, their questions.", s.telegram_enabled),
        ("remote", "Remote", "Your agents on your other devices, end-to-end encrypted, with no account anywhere.", s.remote_enabled),
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
                    "remote" => s.remote_enabled,
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
    content.append(&all);
    content
}

/// The Notes add-on: on or off, its vault, and its topics.
fn notes(s: &Settings, window: &gtk::Window) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
    let main = section("");
    let topics = gtk::Box::new(gtk::Orientation::Vertical, 24);

    let path = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::Start).build();
    let state = gtk::Label::builder().xalign(0.0).wrap(true).max_width_chars(60).build();
    state.add_css_class("dim");
    state.add_css_class("small");
    // The vault set up (when Notes is on), then what it is and its topics shown. Setting up
    // and counting notes read the disk: off the main thread.
    let show = {
        let (path, state, topics, window) = (path.clone(), state.clone(), topics.downgrade(), window.downgrade());
        move || {
            let s = current();
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
        update(|s| s.notes_enabled = active);
        refresh();
    });
    row(&main, "Use Notes", "Scribe files what you tell your agents into the vault below, by topic. The Notes tab beside the chat shows what it wrote.", &on);

    let choose = flat_button("Choose…");
    let (w, refresh) = (window.clone(), show.clone());
    choose.connect_clicked(move |_| {
        let dialog = gtk::FileDialog::builder().title("A folder for the vault").modal(true).build();
        let current = current().notes_vault;
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
                    update(|s| s.notes_vault = chosen.clone());
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
    caption(&main, "Your agents can search and read these notes.");
    let scribe = section("Scribe");
    let calls = gtk::SpinButton::with_range(20.0, 2000.0, 20.0);
    calls.set_value(s.notes_daily_calls as f64);
    calls.connect_value_changed(|b| {
        let value = b.value() as u64;
        update(|s| s.notes_daily_calls = value);
    });
    let model = dropdown(&[("haiku".to_owned(), "Haiku"), ("sonnet".to_owned(), "Sonnet"), ("opus".to_owned(), "Opus")], s.scribe_model.clone(), Clone::clone, |s, v| s.scribe_model = v);
    row(&scribe, "Model", "What Scribe reads and writes with. Haiku uses the least of your limits; a topic can name its own (model: sonnet).", &model);
    let effort = dropdown(
        &[(String::new(), "Default"), ("low".to_owned(), "Low"), ("medium".to_owned(), "Medium"), ("high".to_owned(), "High"), ("xhigh".to_owned(), "Extra high"), ("max".to_owned(), "Max")],
        s.scribe_effort.clone(),
        Clone::clone,
        |s, v| s.scribe_effort = v,
    );
    row(&scribe, "Effort", "How hard it thinks on each call (Default: Claude Code's own). Higher is slower and takes more of your usage limits.", &effort);
    row(&scribe, "Daily call limit", "At most this many Scribe calls a day (one per piece of conversation read, plus one per topic touched). Past it, checks wait for tomorrow.", &calls);
    // Corrections the user explained: rules in the topics, and tests kept on this machine.
    let corrections = loom_core::hub::corrections_path();
    let kept = std::fs::read_to_string(&corrections).map(|t| t.lines().filter(|l| !l.trim().is_empty()).count()).unwrap_or(0);
    if kept > 0 {
        caption(
            &scribe,
            &format!(
                "When you say why Scribe got something wrong, it becomes a rule in that topic (under Rules learned) and a test. {kept} kept: check Scribe still passes them with  loom scribe-eval {} <your vault>",
                corrections.display()
            ),
        );
    }
    content.append(&main);
    content.append(&scribe);
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
            if !create.is_sensitive() {
                return;
            }
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
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 24);
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
    row_stacked(&folder, "Location", "Skills, agents, commands and hooks only loom's agents use; claude in a terminal doesn't load them.", &line);
    let edit_hooks = flat_button("Edit");
    edit_hooks.connect_clicked(edit(crate::plugin::hooks_file()));
    row(&folder, "Hooks", "Commands run on Claude Code's events (a tool used, a turn ended).", &edit_hooks);
    caption(&folder, "Changes reach agents started or woken from now on.");
    holder.append(&folder);

    // What Claude Code does without asking: the rules "Yes, and don't ask again" added.
    let rules = section("Allowed without asking");
    let found = allow_rules();
    if found.is_empty() {
        caption(&rules, "No rules yet. “Yes, and don't ask again” on a question adds one.");
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

/// Bridge `name`'s state, now and on every change while `f` returns true: the core here's, or
/// loomd's when the window is its client (its `bridges` item, the ticket from `pairing`).
fn watch_bridge(name: &'static str, f: impl Fn(&serde_json::Value) -> bool + 'static) {
    type Feeds = Rc<std::cell::RefCell<Option<(crate::feed::Feed, crate::feed::Feed)>>>;
    // Kept by their own callbacks until `f` says it is done (its widgets gone).
    let feeds: Feeds = Rc::default();
    let draw = {
        let feeds = feeds.clone();
        Rc::new(move || {
            let state = {
                let held = feeds.borrow();
                let Some((bridges, pairing)) = held.as_ref() else { return };
                bridges.update();
                pairing.update();
                let mut state = bridges.items().iter().find(|i| i["id"] == name).cloned().unwrap_or_else(|| serde_json::json!({ "state": "off" }));
                state["id"] = state["node"].clone();
                if let Some(pairing) = pairing.items().iter().find(|p| p["id"] == name) {
                    state["ticket"] = pairing["ticket"].clone();
                }
                state
            };
            if !f(&state) {
                feeds.borrow_mut().take();
            }
        })
    };
    let ready = || {
        let draw = draw.clone();
        move || draw()
    };
    let followed = crate::feed::Feed::new("bridges", ready()).zip(crate::feed::Feed::new("pairing", ready()));
    *feeds.borrow_mut() = followed;
    draw();
}

/// Meaning search's state as loomd says it (`embeddings`), and Refresh to check again (Ollama
/// started, the model downloaded).
fn embeddings_status() -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let said = gtk::Label::builder().xalign(0.0).wrap(true).hexpand(true).css_classes(["dim-label"]).build();
    let refresh = gtk::Button::with_label("Refresh");
    refresh.set_tooltip_text(Some("Check again: start Ollama, download the model if it is missing"));
    line.append(&said);
    line.append(&refresh);
    refresh.connect_clicked({
        let said = said.downgrade();
        move |button| {
            button.set_sensitive(false);
            button.set_label("Checking…");
            let button = button.downgrade();
            let said = said.clone();
            crate::conn::command_then("settings", "embeddings_check", serde_json::json!({}), move |item| {
                if let Some(button) = button.upgrade() {
                    button.set_sensitive(true);
                    button.set_label("Refresh");
                }
                if let (Some(said), Some(item)) = (said.upgrade(), item) {
                    paint_embeddings(&said, &item);
                }
            });
        }
    });
    // Followed for as long as the label is there.
    let feed: Rc<std::cell::RefCell<Option<crate::feed::Feed>>> = Rc::default();
    let draw = {
        let (feed, said) = (feed.clone(), said.downgrade());
        move || {
            let Some(label) = said.upgrade() else {
                feed.borrow_mut().take();
                return;
            };
            let held = feed.borrow();
            let Some(f) = held.as_ref() else { return };
            f.update();
            let item = f.items().first().cloned().unwrap_or_default();
            paint_embeddings(&label, &item);
        }
    };
    let ready = {
        let draw = Rc::new(draw);
        let first = draw.clone();
        let made = crate::feed::Feed::new("embeddings", move || draw());
        *feed.borrow_mut() = made;
        first
    };
    ready();
    line
}

/// The `embeddings` item said: its sentence, green when ready, amber when it failed.
fn paint_embeddings(label: &gtk::Label, item: &serde_json::Value) {
    label.set_label(item["detail"].as_str().unwrap_or("Off: searches go by words."));
    for class in ["meaning-ready", "meaning-failed"] {
        label.remove_css_class(class);
    }
    match item["state"].as_str() {
        Some("failed") => label.add_css_class("meaning-failed"),
        Some("ready") => label.add_css_class("meaning-ready"),
        _ => {}
    }
}

/// Opens pairing (a code to scan) or closes it, in loomd.
fn open_pairing(open: bool) {
    crate::conn::command("settings", "pairing", serde_json::json!({ "open": open }));
}

/// The device asking to pair let in, in loomd.
fn accept_device(id: &str) {
    crate::conn::command("settings", "pair_confirm", serde_json::json!({ "id": id }));
}
