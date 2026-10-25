//! Driving a test instance from outside (unused in normal runs).
//!
//! `LOOM_CONTROL=/path/cmds`: every 300 ms, if that file exists, its lines are run in order
//! and the file is deleted. Lines:
//!
//! - `prompt:<text>`: send text from the focused agent's input
//! - `latest`: scroll the focused chat to its end
//! - `toast:<text>`: the in-window notice, about the focused agent
//! - `snapchat:<file.png>`: the focused chat's whole message list, scrolled or not
//! - `snap:<file.png>`: save a picture of the window (works on an unseen workspace)
//! - `snappop:<prefix>`: pictures of the open popovers
//! - `snapall:<prefix>`: pictures of the other windows (settings…), <prefix>-1.png…
//! - `scratch`, `files`, `switcher`, `inbox`, `help`, `peek`: what their keys do
//! - `run:<command>`: run a command in the tray
//! - `open:<file>`: open a file in the editor (relative to the focused agent's folder)
//! - `mode`: next permission mode
//! - `split`: what Ctrl+\ does
//! - `answer:<n>`: answer the focused agent's first question with option n (from 1)
//! - `agent:<n>`: focus the n-th agent (from 1)
//! - `new:<folder>`: start an agent in a folder
//! - `pick:<n>`: row n of the project page (0: new conversation)
//! - `project:<folder>`: that folder's project page (new or past conversations)
//! - `key:<letter>`: Ctrl+B then that key (shift for capitals)
//! - `find`: Ctrl+P · `tree`: the files panel's project view
//! - `close`: close the focused agent (asks first if it is busy)
//! - `quit`

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::{glib, prelude::*};

use crate::app::App;

pub fn watch(app: &Rc<App>) {
    let Some(path) = std::env::var_os("LOOM_CONTROL").map(PathBuf::from) else { return };
    // An unseen window gets no frames, so animations (revealers) would never finish.
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_enable_animations(false);
    }
    let weak = Rc::downgrade(app);
    glib::timeout_add_local(Duration::from_millis(300), move || {
        let Some(app) = weak.upgrade() else { return glib::ControlFlow::Break };
        if let Ok(text) = std::fs::read_to_string(&path) {
            let _ = std::fs::remove_file(&path);
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                if let Err(err) = run(&app, line) {
                    eprintln!("loom control: {line}: {err}");
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

fn run(app: &Rc<App>, line: &str) -> Result<(), String> {
    let (command, argument) = line.split_once(':').unwrap_or((line, ""));
    match command {
        "prompt" => app.focused_agent().ok_or("no agent")?.submit(argument.to_owned()),
        "latest" => {
            // Laid out first, so the end is where the latest message really is.
            lay_out(app.window.upcast_ref())?;
            app.focused_agent().ok_or("no agent")?.chat.scroll_to_latest();
        }
        "toast" => app.show_toast(&app.focused_agent().ok_or("no agent")?, argument, false),
        "snap" => snapshot(app.window.upcast_ref(), argument)?,
        "snapchat" => {
            let list = app.focused_agent().ok_or("no agent")?.chat.messages();
            lay_out(app.window.upcast_ref())?;
            let snap = gtk::Snapshot::new();
            list.parent().ok_or("no parent")?.snapshot_child(&list, &snap);
            let node = snap.to_node().ok_or("nothing was drawn")?;
            let renderer = app.window.renderer().ok_or("window has no renderer")?;
            renderer.render_texture(&node, None).save_to_png(argument).map_err(|e| e.to_string())?;
        }
        // Open popovers (palette, menus, pickers): <prefix>-1.png, -2…
        "snappop" => {
            let mut found = Vec::new();
            popovers(app.window.upcast_ref(), &mut found);
            for (i, popover) in found.iter().enumerate() {
                let child = popover.child().ok_or("empty popover")?;
                let (_, width, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
                let (_, height, _, _) = child.measure(gtk::Orientation::Vertical, width);
                child.allocate(width.max(1), height.max(1), -1, None);
                let snap = gtk::Snapshot::new();
                popover.snapshot_child(&child, &snap);
                let node = snap.to_node().ok_or("nothing drawn")?;
                let renderer = popover.renderer().ok_or("no renderer")?;
                renderer.render_texture(&node, None).save_to_png(format!("{argument}-{}.png", i + 1)).map_err(|e| e.to_string())?;
            }
            if found.is_empty() {
                return Err("no popover open".into());
            }
        }
        // Every other window too (settings…): <prefix>-1.png, -2…
        "snapall" => {
            let windows: Vec<gtk::Window> = gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Window>().ok())
                .filter(|w| w.is_visible() && w != app.window.upcast_ref::<gtk::Window>())
                .collect();
            for (i, window) in windows.iter().enumerate() {
                snapshot(window, &format!("{argument}-{}.png", i + 1))?;
            }
        }
        "answer" => {
            let n: usize = argument.parse().map_err(|_| "answer:<n>")?;
            if !app.focused_agent().ok_or("no agent")?.answer(0, n.saturating_sub(1)) {
                return Err("no such question or option".into());
            }
        }
        "agent" => app.focus_nth(argument.parse::<usize>().map_err(|_| "agent:<n>")?.saturating_sub(1)),
        "new" => app.start_in(PathBuf::from(argument)),
        "pick" => app.pick_history(argument.parse().map_err(|_| "pick:<n>")?),
        "project" => app.show_project(&crate::app::project_of(Path::new(argument)), Path::new(argument)),
        "quit" => app.window.close(),
        // Minimum widths down the widget tree (to find what holds a pane wide).
        "measure" => measure(app.window.upcast_ref(), 0, argument.parse().unwrap_or(150)),
        "key" => app.prefix_key(argument),
        other => app.act(other, argument),
    }
    Ok(())
}

fn measure(widget: &gtk::Widget, depth: usize, threshold: i32) {
    let (min, _, _, _) = widget.measure(gtk::Orientation::Horizontal, -1);
    if min >= threshold && depth < 40 {
        eprintln!("{}{} {} min={min}", "  ".repeat(depth), widget.type_().name(), widget.css_classes().join("."));
        let mut child = widget.first_child();
        while let Some(w) = child {
            if w.is_visible() {
                measure(&w, depth + 1, threshold);
            }
            child = w.next_sibling();
        }
    }
}

fn popovers(widget: &gtk::Widget, found: &mut Vec<gtk::Popover>) {
    let mut child = widget.first_child();
    while let Some(w) = child {
        if let Some(p) = w.downcast_ref::<gtk::Popover>()
            && p.is_visible()
        {
            found.push(p.clone());
        }
        popovers(&w, found);
        child = w.next_sibling();
    }
}

/// A window the compositor isn't drawing (another workspace) has its layout pending
/// forever: lay it out here, at its last size or else the size it opened at.
fn lay_out(window: &gtk::Window) -> Result<gtk::Widget, String> {
    let content = window.child().ok_or("window has no content")?;
    let (width, height) = match (content.width(), content.height()) {
        (0, _) | (_, 0) => window.default_size(),
        size => size,
    };
    content.allocate(width.max(400), height.max(300), -1, None);
    Ok(content)
}

fn snapshot(window: &gtk::Window, path: &str) -> Result<(), String> {
    let content = lay_out(window)?;
    let snap = gtk::Snapshot::new();
    window.snapshot_child(&content, &snap);
    let node = snap.to_node().ok_or("nothing was drawn")?;
    let renderer = window.renderer().ok_or("window has no renderer")?;
    renderer.render_texture(&node, None).save_to_png(path).map_err(|e| e.to_string())
}
