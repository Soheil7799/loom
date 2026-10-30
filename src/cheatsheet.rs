//! The keyboard cheatsheet (Ctrl+/): every shortcut, by what it is for, keys drawn as caps.

use gtk::{gdk, glib, prelude::*};

/// Shortcuts by section. Alternatives are separated by " / "; a note in parentheses is shown
/// after the keys.
pub const SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "Agents",
        &[
            ("Ctrl+Shift+P", "Command palette: every action by name"),
            ("Ctrl+N", "New agent, or resume a past conversation"),
            ("Ctrl+Shift+N", "Open a project folder"),
            ("Ctrl+K", "Go to an agent"),
            ("Ctrl+Tab / Alt+↓", "Next agent"),
            ("Ctrl+Shift+Tab / Alt+↑", "Previous agent"),
            ("Ctrl+1…9", "Agent by its place in the sidebar"),
            ("Ctrl+Shift+T", "Reopen the last closed agent"),
            ("F2", "Rename the agent (or the project, on its page)"),
            ("Ctrl+W", "Close the agent (the conversation is kept)"),
            ("Alt+Shift+↑ / Alt+Shift+↓", "Move the agent up or down"),
        ],
    ),
    (
        "Notes (while Notes is on)",
        &[
            ("Ctrl+Shift+M", "The Notes view, and back to the agents"),
            ("Ctrl+N", "New chat (in the Notes view)"),
            ("Alt+↑ / Alt+↓", "Previous or next chat (in the Notes view)"),
            ("Ctrl+E", "Topics and history panel (in the Notes view)"),
        ],
    ),
    (
        "Chat",
        &[
            ("Enter", "Send (while Claude works: queued for when it finishes)"),
            ("Ctrl+Enter", "Send now, into the running turn (the queue too)"),
            ("Shift+Enter", "New line"),
            ("↑ / ↓", "Earlier messages you sent"),
            ("Esc", "Stop the turn (or a ! command)"),
            ("Ctrl+F", "Find in the conversation"),
            ("Page Up / Page Down", "Scroll the conversation"),
            ("Ctrl+↑ / Ctrl+↓", "Your previous / next message"),
            ("Ctrl+Home / Ctrl+End", "Top / latest"),
            ("Ctrl+V", "Paste an image"),
            ("/clear", "A fresh conversation here"),
            ("Shift+Tab", "Next permission mode"),
            ("@", "Mention a file"),
            ("/", "Claude Code's commands"),
            ("!", "Run a command; its output goes to Claude when it ends"),
            ("Ctrl+C", "Copy the text selected in the chat"),
            ("Ctrl+L", "Back to the input"),
        ],
    ),
    (
        "Questions and permissions",
        &[
            ("1…9 (input empty)", "Pick an option"),
            ("Alt+1…9", "Pick an option while typing"),
            ("Alt+Y", "Allow"),
            ("Alt+N", "Deny"),
            ("Alt+Enter", "Send the answers to Claude's question"),
            ("Ctrl+I", "Questions from every agent"),
        ],
    ),
    (
        "Split view",
        &[("Ctrl+\\", "The previous agent beside this one"), ("Alt+← / Alt+→", "Swap the two sides")],
    ),
    (
        "Tray and terminals",
        &[
            ("Ctrl+`", "Scratch terminal"),
            ("Ctrl+J", "Run a command in the tray"),
            ("Ctrl+Shift+J", "Peek at the newest job"),
            ("Ctrl+Shift+X", "Clear finished jobs"),
            ("Ctrl+Shift+C / Ctrl+Shift+V", "Copy / paste in a terminal"),
        ],
    ),
    (
        "Files",
        &[
            ("Ctrl+P", "Find a file in the project"),
            ("Ctrl+E", "Files panel"),
            ("Ctrl+O", "Open a file"),
            ("Ctrl+S", "Save (in the editor)"),
            ("Ctrl+F (in the editor)", "Find in the file"),
            ("Ctrl+Shift+O", "Open the file in another editor (Settings → Advanced)"),
        ],
    ),
    (
        "Ctrl+B, then (herdr's letters)",
        &[
            ("V", "Split"),
            ("H / L", "Swap sides"),
            ("J / K", "Next / previous agent"),
            ("C", "New agent"),
            ("X", "Close the agent"),
            ("Z", "Zoom"),
            ("B", "Sidebar"),
            ("D", "Send what runs to the background"),
            ("F", "Files"),
            ("T", "Scratch terminal"),
            ("R", "Run a command"),
            ("G", "Find a file"),
            ("S", "Go to an agent"),
            ("I", "Questions"),
            ("M", "Notes view"),
        ],
    ),
    (
        "App",
        &[
            ("Ctrl+,", "Settings"),
            ("Ctrl+/", "This cheatsheet"),
            ("Ctrl+= / Ctrl+-", "Bigger / smaller text"),
            ("Ctrl+0", "Normal text size"),
        ],
    ),
];

/// The keys of one shortcut as caps: "Ctrl+Shift+P" → [Ctrl] + [Shift] + [P], alternatives
/// side by side, a note in parentheses after them.
pub fn keys(text: &str) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    line.set_valign(gtk::Align::Center);
    let (keys, note) = match text.split_once(" (") {
        Some((keys, note)) => (keys, Some(note.trim_end_matches(')'))),
        None => (text, None),
    };
    for (i, alternative) in keys.split(" / ").enumerate() {
        if i > 0 {
            let or = gtk::Label::new(Some("or"));
            or.add_css_class("key-or");
            line.append(&or);
        }
        // "Ctrl++" would be Ctrl and +; no shortcut here ends in a bare "+".
        for (j, part) in alternative.split('+').enumerate() {
            if j > 0 {
                let plus = gtk::Label::new(Some("+"));
                plus.add_css_class("key-plus");
                line.append(&plus);
            }
            let cap = gtk::Label::new(Some(part));
            cap.add_css_class("keycap");
            line.append(&cap);
        }
    }
    if let Some(note) = note {
        let note = gtk::Label::new(Some(note));
        note.add_css_class("key-note");
        line.append(&note);
    }
    line
}

pub fn open(parent: &impl IsA<gtk::Window>) {
    let window = gtk::Window::builder()
        .title("Keyboard shortcuts")
        .transient_for(parent)
        .modal(true)
        .default_width(1080)
        .default_height(720)
        .build();
    window.add_css_class("loom");
    window.add_css_class("cheatsheet");

    let sections = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(false)
        .min_children_per_line(1)
        .max_children_per_line(3)
        .column_spacing(16)
        .row_spacing(16)
        .valign(gtk::Align::Start)
        .build();
    for (title, shortcuts) in SECTIONS {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 2);
        card.add_css_class("cheat-card");
        let heading = gtk::Label::builder().label(*title).xalign(0.0).build();
        heading.add_css_class("cheat-heading");
        card.append(&heading);
        for (keys_text, what) in *shortcuts {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("cheat-row");
            let what = gtk::Label::builder().label(*what).xalign(0.0).hexpand(true).wrap(true).max_width_chars(30).build();
            row.append(&what);
            row.append(&keys(keys_text));
            card.append(&row);
        }
        let child = gtk::FlowBoxChild::builder().child(&card).focusable(false).build();
        sections.append(&child);
    }

    let title = gtk::Label::builder().label("Keyboard shortcuts").xalign(0.0).build();
    title.add_css_class("cheat-title");
    let subtitle = gtk::Label::builder().label("Esc closes · the command palette (Ctrl+Shift+P) finds every action by name").xalign(0.0).build();
    subtitle.add_css_class("dim");
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.add_css_class("cheat-content");
    content.append(&title);
    content.append(&subtitle);
    content.append(&sections);
    let scroller = gtk::ScrolledWindow::builder().child(&content).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
    window.set_child(Some(&scroller));

    let keys_controller = gtk::EventControllerKey::new();
    let weak = window.downgrade();
    keys_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape
            && let Some(window) = weak.upgrade()
        {
            window.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys_controller);
    window.present();
}
