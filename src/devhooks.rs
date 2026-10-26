//! Driving a test instance from outside (unused in normal runs).
//!
//! `LOOM_CONTROL=/path/cmds`: every 300 ms, if that file exists, its lines are run in order
//! and the file is deleted. Lines:
//!
//! - `prompt:<text>`: send text from the focused agent's input
//! - `latest`: scroll the focused chat to its end
//! - `send`: sends the focused agent's question form (Alt+Enter)
//! - `attach:<image file>`: attaches an image to the next message (as a paste would)
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
//! - `name:<folder>=<name>`: names a project (empty: its folder's name)
//! - `pick:<n>`: row n of the project page (0: new conversation)
//! - `project:<folder>`: that folder's project page (new or past conversations)
//! - `key:<letter>`: Ctrl+B then that key (shift for capitals)
//! - `find`: Ctrl+P · `tree`: the files panel's project view
//! - `squeezed`: widgets given less room than they measure, and every paned
//! - `page:<name>`: shows that page of a stack in any open window (settings…)
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
        // As if typed and sent: attachments waiting in the composer go with it.
        "prompt" => app.focused_agent().ok_or("no agent")?.chat.send_text(argument),
        "latest" => {
            // Laid out first, so the end is where the latest message really is.
            lay_out(app.window.upcast_ref())?;
            app.focused_agent().ok_or("no agent")?.chat.scroll_to_latest();
        }
        "send" => {
            if !app.focused_agent().ok_or("no agent")?.send_form() {
                return Err("no question form open".into());
            }
        }
        "attach" => {
            let texture = gtk::gdk::Texture::from_filename(argument).map_err(|e| e.to_string())?;
            app.focused_agent().ok_or("no agent")?.chat.attach(&texture);
        }
        "usage" => print_labels(&app.focused_agent().ok_or("no agent")?.chat.usage_content(argument != "read")),
        // Two subagents at work (for the strip over the input).
        "fakecrew" => {
            let chat = app.focused_agent().ok_or("no agent")?.chat.clone();
            chat.set_working(Some(std::time::Instant::now()));
            for (id, name, step) in [("t1", "Convert batch 1", "Edit HandoverPage.tsx"), ("t2", "Convert batch 2", "grep -rn \"آزاد\" web/src")] {
                chat.tool(&serde_json::json!({ "toolCallId": id, "title": name, "kind": "think", "status": "completed" }));
                chat.tool(&serde_json::json!({
                    "toolCallId": format!("{id}-c"), "title": step, "kind": "edit", "status": "in_progress",
                    "_meta": { "claudeCode": { "parentToolUseId": id } }
                }));
            }
        }
        "overview" => app.show_overview(),
        // A schedule due now: a new conversation in <dir> that runs `!echo scheduled-ok`.
        "schedtest" => app.add_schedule_for_tests(PathBuf::from(argument), "!echo scheduled-ok"),
        "schedule" => {
            let agent = app.agents_for_tests().into_iter().next().ok_or("no agent")?;
            app.schedule_for(&agent);
        }
        // A plan to review: "fakeplan", then "plancomment:<text>" (on its first part) and
        // "planchanges" (Request changes).
        "fakeplan" => {
            let chat = app.focused_agent().ok_or("no agent")?.chat.clone();
            let plan = "## Plan\n\nAdd a **price estimate** column to the purchase form.\n\n```sql\nALTER TABLE items ADD estimate numeric;\n```\n\nThen show the sum under the table.";
            let options = [("go".to_owned(), "Yes, start".to_owned(), "allow_once".to_owned()), ("keep".to_owned(), "No, keep planning".to_owned(), "reject_once".to_owned())];
            chat.permission("Ready to code?", Some(plan), true, &options, std::rc::Rc::new(|id: String| eprintln!("CHOSE {id}")));
        }
        // Presses the last button labelled <label> (the newest card's).
        "click" => {
            let mut found = Vec::new();
            collect(app.window.upcast_ref(), &|w| w.downcast_ref::<gtk::Button>().and_then(|b| b.label()).is_some_and(|l| l == argument), &mut found);
            found.last().and_then(|w| w.downcast_ref::<gtk::Button>()).ok_or("no such button")?.emit_clicked();
        }
        "type" => app.focused_agent().ok_or("no agent")?.chat.input.buffer().set_text(argument),
        "plancomment" => {
            let entry = find(app.window.upcast_ref(), &|w| w.has_css_class("plan-comment")).and_downcast::<gtk::Entry>().ok_or("no comment field")?;
            entry.set_text(argument);
        }
        "planchanges" => {
            let button = find(app.window.upcast_ref(), &|w| w.downcast_ref::<gtk::Button>().and_then(|b| b.label()).is_some_and(|l| l.starts_with("Request changes")))
                .and_downcast::<gtk::Button>()
                .ok_or("no Request changes button")?;
            eprintln!("BUTTON {}", button.label().unwrap_or_default());
            button.emit_clicked();
        }
        // Scrolls the chat by <px> as a reader would (negative: up).
        "scrollby" => app.focused_agent().ok_or("no agent")?.chat.scroll_by_for_tests(argument.parse().map_err(|_| "scrollby:<px>")?),
        // Labels whose text is taller than the room they were given (drawn past their end).
        "overflow" => {
            let list = app.focused_agent().ok_or("no agent")?.chat.messages();
            let mut found = Vec::new();
            collect(list.upcast_ref(), &|w| w.is::<gtk::Label>(), &mut found);
            let mut n = 0;
            for w in found {
                let label = w.downcast_ref::<gtk::Label>().unwrap();
                if !label.is_drawable() {
                    continue;
                }
                let (_, text_h) = label.layout().pixel_size();
                let (min_h, nat_h, _, _) = label.measure(gtk::Orientation::Vertical, label.width());
                if text_h > label.height() + 1 {
                    n += 1;
                    if n <= 12 {
                        let text: String = label.text().chars().take(50).collect();
                        eprintln!("OVER w={} h={} text_h={text_h} min={min_h} nat={nat_h} lines={} classes={:?} {text:?}", label.width(), label.height(), label.layout().line_count(), label.css_classes());
                    }
                }
            }
            eprintln!("OVER total {n}");
        }
        // The last items of the conversation: where each sits and how tall it is.
        "tail" => {
            let list = app.focused_agent().ok_or("no agent")?.chat.messages();
            let mut items = Vec::new();
            let mut child = list.last_child();
            while let Some(w) = child {
                if items.len() >= 8 {
                    break;
                }
                child = w.prev_sibling();
                items.push(w);
            }
            for w in items.iter().rev() {
                let y = w.compute_point(&list, &gtk::graphene::Point::new(0.0, 0.0)).map_or(-1.0, |p| p.y());
                let (_, nat, _, _) = w.measure(gtk::Orientation::Vertical, w.width());
                eprintln!("TAIL y={y:.0} h={} nat={nat} visible={} {} {:?}", w.height(), w.get_visible(), w.type_().name(), w.css_classes());
            }
            eprintln!("TAIL list_h={}", list.height());
            let mut labels = Vec::new();
            if let Some(last) = list.last_child() {
                collect(&last, &|w| w.is::<gtk::Label>(), &mut labels);
            }
            for w in labels.iter().rev().take(3) {
                let label = w.downcast_ref::<gtk::Label>().unwrap();
                let y = label.compute_point(&list, &gtk::graphene::Point::new(0.0, 0.0)).map_or(-1.0, |p| p.y());
                let text = label.text();
                let tail: String = text.chars().rev().take(80).collect::<String>().chars().rev().collect();
                eprintln!("LAST y={y:.0} h={} {:?}", label.height(), tail);
            }
        }
        "maximize" => app.window.maximize(),
        "scrollinfo" => eprintln!("SCROLL {}", app.focused_agent().ok_or("no agent")?.chat.scroll_info()),
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
        // In any open window (settings…): the stack page named <name> shown.
        "page" => {
            let mut found = Vec::new();
            for window in gtk::Window::list_toplevels() {
                collect(&window, &|w| w.downcast_ref::<gtk::Stack>().is_some_and(|s| s.child_by_name(argument).is_some()), &mut found);
            }
            found.first().and_then(|w| w.downcast_ref::<gtk::Stack>()).ok_or("no such page")?.set_visible_child_name(argument);
            if let Some(page) = found.first().and_then(|w| w.downcast_ref::<gtk::Stack>()).and_then(|s| s.visible_child()) {
                print_labels(&page);
            }
        }
        // Widgets given less room than they need (with their ancestors), and every paned.
        "squeezed" => squeezed(app.window.upcast_ref(), 0),
        // The text in open popovers, for when they cannot be drawn.
        "poptext" => {
            let mut found = Vec::new();
            popovers(app.window.upcast_ref(), &mut found);
            for popover in found {
                print_labels(popover.upcast_ref());
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
        "name" => {
            let (path, name) = argument.split_once('=').ok_or("name:<folder>=<name>")?;
            app.name_project(Path::new(path), name);
        }
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

fn squeezed(widget: &gtk::Widget, depth: usize) {
    let (min_h, _, _, _) = widget.measure(gtk::Orientation::Vertical, -1);
    let (min_w, _, _, _) = widget.measure(gtk::Orientation::Horizontal, -1);
    let (w, h) = (widget.width(), widget.height());
    let short = widget.is_visible() && (h < min_h || w < min_w);
    if short || widget.is::<gtk::Paned>() {
        let path: Vec<String> = std::iter::successors(Some(widget.clone()), |w| w.parent())
            .map(|w| format!("{}{:?}", w.type_().name(), w.css_classes().iter().map(|c| c.to_string()).collect::<Vec<_>>()))
            .collect();
        println!("SQUEEZE {:p} {w}x{h} min {min_w}x{min_h} visible={} :: {}", widget.as_ptr(), widget.is_visible(), path.join(" < "));
        if let Some(paned) = widget.downcast_ref::<gtk::Paned>() {
            println!("   position {} start {:?} end {:?}", paned.position(),
                paned.start_child().map(|c| (c.type_().name(), c.is_visible(), c.width(), c.height())),
                paned.end_child().map(|c| (c.type_().name(), c.is_visible(), c.width(), c.height())));
        }
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        squeezed(&c, depth + 1);
        child = c.next_sibling();
    }
}

fn print_labels(widget: &gtk::Widget) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        eprintln!("LABEL {}", label.label());
    } else if let Some(bar) = widget.downcast_ref::<gtk::LevelBar>() {
        eprintln!("BAR {:.2}", bar.value());
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        print_labels(&w);
        child = w.next_sibling();
    }
}

fn collect(widget: &gtk::Widget, test: &dyn Fn(&gtk::Widget) -> bool, found: &mut Vec<gtk::Widget>) {
    if test(widget) {
        found.push(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        collect(&w, test, found);
        child = w.next_sibling();
    }
}

fn find(widget: &gtk::Widget, test: &dyn Fn(&gtk::Widget) -> bool) -> Option<gtk::Widget> {
    if test(widget) {
        return Some(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        if let Some(found) = find(&w, test) {
            return Some(found);
        }
        child = w.next_sibling();
    }
    None
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
    // Twice: a scroll position set while laying out (following the end) applies on the next.
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
