//! Driving a test instance from outside (unused in normal runs).
//!
//! Debug builds only (`cfg(debug_assertions)`: a release build ignores LOOM_CONTROL here).
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
//! - `snapnode:<file.node>`: its render nodes, for `gtk4-rendernode-tool benchmark`
//! - `snappop:<prefix>`: pictures of the open popovers
//! - `snapall:<prefix>`: pictures of the other windows (settings…), <prefix>-1.png…
//! - `scratch`, `files`, `switcher`, `inbox`, `help`, `peek`: what their keys do
//! - `run:<command>`: run a command in the tray
//! - `pin` / `unpin`: pin the newest job above the tray, or back
//! - `notesstep:<n>`: the next past Notes chat (n 1) or the one before (-1), as Ctrl+↑/↓
//! - `loomdrestart`: loomd stopped and started again (as the sign does for an older one)
//! - `loomdclick`: what clicking the footer's loomd sign does
//! - `putproject:<folder>=<folder>`: the first project dropped before the second
//! - `jobkeys:<text>`: keys typed into the newest job's terminal (`\n`: Enter)
//! - `open:<file>`: open a file in the editor (relative to the focused agent's folder)
//! - `mode`: next permission mode
//! - `split`: what Ctrl+\ does · `swap`: what Alt+←/→ does (the two change places)
//! - `answer:<n>`: answer the focused agent's first question with option n (from 1)
//! - `fakereply` / `fakestream`: a reply with every kind of Markdown, at once / typed in
//! - `fakeform`: a one-question form with long options, in the focused chat
//! - `minwidth:<px>`: each window's widgets whose minimum width is at least px (what holds it wide)
//! - `stop`: what Esc does while the focused agent works (through the chat)
//! - `agent:<n>`: focus the n-th agent (from 1) · `row:<n>`: open sidebar row n (from 0)
//! - `new:<folder>`: start an agent in a folder
//! - `board:<folder>`: that project's board
//! - `name:<folder>=<name>`: names a project (empty: its folder's name)
//! - `pick:<n>`: row n of the project page (0: new conversation)
//! - `project:<folder>`: that folder's project page (new or past conversations)
//! - `key:<letter>`: Ctrl+B then that key (shift for capitals)
//! - `find`: Ctrl+P · `tree`: the files panel's project view
//! - `squeezed`: widgets given less room than they measure, and every paned
//! - `page:<name>`: shows that page of a stack in any open window (settings…)
//! - `sendto:<text>`: "Send to another agent…" with that text, to the first agent offered
//! - `pickerfirst`: the open picker's first entry chosen (Enter)
//! - `findcode:<words>`: the palette's "Find code by meaning…" asked that
//! - `awayfor:<seconds>`: away mode on for that long
//! - `close`: close the focused agent (asks first if it is busy)
//! - `scribenews`: as if Scribe had something new (the Notes tab shown or marked)
//! - `rowmenu:<n>`: the sidebar's right-click menu of row n (empty: of its empty part)
//! - `noteshistory`: the Notes history page · `unfold`: every folded row unfolded
//! - `teach:<why>` / `keeprule`: answer the newest "Why not?", keep its drafted rule
//! - `fixnewest:<comment>`: a comment on the newest written change
//! - `wfopen:<n>`: open agent n of the first workflow card
//! - `state:<css class>=<file>`: whether those widgets show (in any window), each up to its window
//! - `remotestate:<file>`: the remote bridge state Settings shows (ticket…), written there
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

/// A reply with every kind of Markdown (fakereply, fakestream).
const SAMPLE: &str = "## Heading two\n\nA paragraph with **bold**, *italic*, `inline code`, a path `src/app.rs:42` and [a link](https://example.com). Double-click a word, then drag across sentences.\n\n- First item that is long enough to wrap onto a second line so the hanging indent shows\n- Second item\n    - Nested item\n\n1. One\n2. Two\n\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n\nAfter the code.\n\n```svg\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"220\" height=\"80\"><rect x=\"4\" y=\"4\" width=\"212\" height=\"72\" rx=\"12\" fill=\"#4a7dff\"/><text x=\"110\" y=\"48\" font-size=\"22\" text-anchor=\"middle\" fill=\"white\">an SVG</text></svg>\n```\n\n```mermaid\ngraph LR\n  A[Agent] --> B{Review}\n  B -->|keep| C[Merge]\n  B -->|drop| D[Remove]\n```\n\n| Idea | Verdict, the short version of it | Why it is covered elsewhere already |\n|---|---|---|\n| a | 1 | short |\n| OCR for the index (scans, images, PDFs in the vault) | Later | The vault index reads `.md` only today. That is where OCR saves real tokens. Start with Tesseract `eng+fas`, then try **glm-ocr** in Ollama. |\n| Crawler if ever needed | Later | trafilatura fed by loom's own Chromium. Pages go into the existing index. |\n\n> A quote that runs long enough to wrap onto a second line, so its bar runs beside both.\n>\n> And its second paragraph.\n\nThe end.";

fn run(app: &Rc<App>, line: &str) -> Result<(), String> {
    let (command, argument) = line.split_once(':').unwrap_or((line, ""));
    match command {
        // As if typed and sent: attachments waiting in the composer go with it.
        "prompt" => app.chat_in_view().ok_or("no agent")?.send_text(&argument.replace("\\n", "\n")),
        // A message sent without the input (a "/command" typed there opens its popup, which an
        // unseen test window can hang on).
        "submit" => app.chat_in_view().ok_or("no agent")?.submit_direct(argument),
        // loom's Claude folder changed (as when an item is switched in Settings).
        "pluginchanged" => crate::plugin::changed(),
        // Ctrl+Enter: what waits goes into the running turn.
        "sendnow" => crate::conn::command("window", "send_now", serde_json::json!({ "agent": app.remote_view().ok_or("no agent")?.id.to_string() })),
        "latest" => {
            // Laid out first, so the end is where the latest message really is.
            lay_out(app.window.upcast_ref())?;
            app.chat_in_view().ok_or("no agent")?.scroll_to_latest();
        }
        "send" => {
            if !app.remote_view().is_some_and(|a| a.send_form()) {
                return Err("no question form open".into());
            }
        }
        "attach" => {
            let texture = gtk::gdk::Texture::from_filename(argument).map_err(|e| e.to_string())?;
            app.chat_in_view().ok_or("no agent")?.attach(&texture);
        }
        "usage" => print_labels(&app.chat_in_view().ok_or("no agent")?.usage_content(argument != "read")),
        // Two subagents at work (for the strip over the input).
        "fakecrew" => {
            let chat = app.chat_in_view().ok_or("no agent")?.clone();
            chat.set_working(Some(std::time::Instant::now()));
            for (id, name, step) in [("t1", "Convert batch 1", "Edit HandoverPage.tsx"), ("t2", "Convert batch 2", "grep -rn \"آزاد\" web/src")] {
                chat.tool(&serde_json::json!({ "toolCallId": id, "title": name, "kind": "think", "status": "completed" }));
                chat.tool(&serde_json::json!({
                    "toolCallId": format!("{id}-c"), "title": step, "kind": "edit", "status": "in_progress",
                    "_meta": { "claudeCode": { "parentToolUseId": id } }
                }));
            }
        }
        // A subagent left running in the background after the turn (a reported one).
        "fakebg" => {
            let chat = app.chat_in_view().ok_or("no agent")?.clone();
            chat.set_working(None);
            let now = std::time::Instant::now();
            chat.crew_set(crate::chat::Crew {
                key: "bg1".into(),
                name: "research".into(),
                step: "Reading the docs".into(),
                at: now,
                started: now,
                state: Some("running".into()),
                ended: None,
                guessed: false,
            });
        }
        "overview" => app.show_overview(),
        "noteshistory" => app.show_notes_history(),
        // The newest "Why not?" answered, and its drafted rule kept.
        "teach" => app.hub.teach_newest(argument),
        "keeprule" => app.hub.keep_newest_rule(),
        // Row n (from 0) of the first workflow card's agents, opened.
        "wfopen" => {
            let mut found = Vec::new();
            collect(app.window.upcast_ref(), &|w| w.has_css_class("workflow-agents"), &mut found);
            let row = found.first().and_then(|l| l.downcast_ref::<gtk::ListBox>()).and_then(|l| l.row_at_index(argument.parse().unwrap_or(0)));
            row.ok_or("no workflow agent there")?.emit_activate();
        }
        // The Stop of the first workflow card that has one showing.
        "wfstop" => {
            let mut found = Vec::new();
            collect(app.window.upcast_ref(), &|w| w.has_css_class("workflow-stop") && w.is_visible(), &mut found);
            found.first().and_then(|b| b.downcast_ref::<gtk::Button>()).ok_or("no workflow to stop")?.emit_clicked();
        }
        // A comment on the newest written change (Scribe changes the note as it says).
        "fixnewest" => app.hub.fix_newest(argument),
        // Every folded row unfolded (their buttons exist only then).
        "unfold" => {
            let mut found = Vec::new();
            collect(app.window.upcast_ref(), &|w| w.is::<gtk::Expander>(), &mut found);
            for w in found {
                if let Some(e) = w.downcast_ref::<gtk::Expander>() {
                    e.set_expanded(true);
                }
            }
        }
        // The sidebar's right-click menu: of row n (from 0), or of its empty part.
        "rowmenu" => app.row_menu_for_tests(argument.parse().ok()),
        // The open popovers' text, printed (a menu's items, greyed out ones marked).
        "poptext" => {
            let mut found = Vec::new();
            popovers(app.window.upcast_ref(), &mut found);
            for popover in found {
                eprintln!("POPOVER");
                let mut buttons = Vec::new();
                collect(popover.upcast_ref(), &|w| w.is::<gtk::Button>(), &mut buttons);
                if buttons.is_empty() {
                    print_labels(popover.upcast_ref());
                } else {
                    print_items(popover.upcast_ref());
                }
            }
        }
        // As if Scribe had something new (a question, a change to review).
        "scribenews" => {
            if let Some(attention) = app.hub.on_attention.borrow().as_ref() {
                attention();
            }
        }
        "about" => crate::about::open(&app.window),
        // The line a project dragged over project <n> (from 1) shows: dropline:<n>:before|after.
        "dropline" => {
            let (n, side) = argument.split_once(':').ok_or("dropline:<n>:before|after")?;
            app.drop_line_for_tests(n.parse::<usize>().map_err(|_| "a number")?.saturating_sub(1), side == "before");
        }
        "schedule" => app.schedule_for_remote(app.remote_view().ok_or("no agent")?.id),
        // A plan to review: "fakeplan", then "plancomment:<text>" (on its first part) and
        // "planchanges" (Request changes).
        "fakeplan" => {
            let chat = app.chat_in_view().ok_or("no agent")?.clone();
            let plan = "## Plan\n\nAdd a **price estimate** column to the purchase form.\n\n```sql\nALTER TABLE items ADD estimate numeric;\n```\n\nThen show the sum under the table.";
            let options = [("go".to_owned(), "Yes, start".to_owned(), "allow_once".to_owned()), ("keep".to_owned(), "No, keep planning".to_owned(), "reject_once".to_owned())];
            chat.permission("Ready to code?", Some(plan), true, &options, std::rc::Rc::new(|id: String| eprintln!("CHOSE {id}")));
        }
        // A reply with every kind of Markdown, for looking at the chat's text.
        "pressswitch" => crate::rich::press_switch(&app.chat_in_view().ok_or("no agent")?.messages())?,
        "fakereply" => {
            let chat = app.chat_in_view().ok_or("no agent")?.clone();
            chat.agent_text_with(SAMPLE, None);
        }
        // The same reply typed in a few characters at a time, as an agent's streams in (to
        // watch the redraw, and select the text before it meanwhile).
        "fakestream" => {
            let chat = app.chat_in_view().ok_or("no agent")?.clone();
            let chars: Vec<char> = SAMPLE.chars().collect();
            let at = std::rc::Rc::new(std::cell::Cell::new(0));
            glib::timeout_add_local(Duration::from_millis(50), move || {
                let end = (at.get() + 3).min(chars.len());
                chat.agent_text_with(&chars[at.get()..end].iter().collect::<String>(), Some("fakestream"));
                at.set(end);
                if end == chars.len() { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
            });
        }
        // A one-question form with long options (as AskUserQuestion sends), for width checks.
        "fakeform" => {
            let chat = app.chat_in_view().ok_or("no agent")?.clone();
            let long = "This option comes with a long description, the way agents write them: it explains the trade-off in a full sentence or two so the reader can decide without asking again, and it should wrap, never widen the window.";
            let schema = serde_json::json!({ "properties": { "q": { "type": "string", "title": "Which way should the cache be invalidated when a project is renamed or moved?", "oneOf": [
                { "const": "a", "title": "Invalidate everything on every rename, the simplest and safest choice", "description": long },
                { "const": "b", "title": "Only_the_renamed_project_with_a_very_long_identifier_name_that_has_no_spaces_anywhere", "description": long }
            ] } }, "required": ["q"] });
            chat.form("Claude asks one question", &schema, std::rc::Rc::new(|v: Option<serde_json::Value>| eprintln!("FORM {v:?}")));
        }
        // Each window's widgets whose minimum width is at least <px> (what keeps it wide).
        "minwidth" => {
            let threshold = argument.parse().unwrap_or(600);
            for window in gtk::Window::list_toplevels() {
                if window.is_visible() {
                    eprintln!("MINWIDTH window {:?}", window.downcast_ref::<gtk::Window>().and_then(|w| w.title()));
                    measure(&window, 0, threshold);
                }
            }
        }
        // Presses the last button labelled <label> (the newest card's).
        "click" => {
            let mut found = Vec::new();
            // In any window (settings…), the main one last so its buttons win.
            // By its label, or the first text inside it (a menu's rows).
            let test = |w: &gtk::Widget| {
                let Some(button) = w.downcast_ref::<gtk::Button>() else { return false };
                if button.label().is_some_and(|l| l == argument) {
                    return true;
                }
                let mut texts = Vec::new();
                collect_text(w, &mut texts);
                texts.first().is_some_and(|t| t == argument)
            };
            for window in gtk::Window::list_toplevels().into_iter().filter(|w| w != app.window.upcast_ref::<gtk::Widget>()) {
                collect(&window, &test, &mut found);
            }
            collect(app.window.upcast_ref(), &test, &mut found);
            // One on screen (a hidden one of the same label is not the one meant).
            let shown = found.iter().rev().find(|w| w.is_mapped()).or(found.last());
            shown.and_then(|w| w.downcast_ref::<gtk::Button>()).ok_or("no such button")?.emit_clicked();
        }
        // Whether the widgets of a CSS class are shown, and why not: each one, then every parent
        // up to the window (visible, mapped, child-visible), into a file.
        "state" => {
            let (class, file) = argument.split_once('=').ok_or("state:<class>=<file>")?;
            let mut found = Vec::new();
            // In every window (settings…).
            for window in gtk::Window::list_toplevels() {
                collect(&window, &|w: &gtk::Widget| w.has_css_class(class), &mut found);
            }
            let mut out = String::new();
            for widget in found {
                let mut at = Some(widget);
                while let Some(w) = at {
                    out += &format!(
                        "{} .{} own={} visible={} mapped={} child_visible={} h={}\n",
                        w.type_().name(),
                        w.css_classes().join("."),
                        w.get_visible(),
                        w.is_visible(),
                        w.is_mapped(),
                        w.is_child_visible(),
                        w.height()
                    );
                    at = w.parent();
                }
                out += "--\n";
            }
            std::fs::write(file, out).map_err(|_| "not written")?;
        }
        // The remote bridge as Settings sees it (its pairing ticket…), into a file.
        "remotestate" => std::fs::write(argument, crate::bridges::state_of("remote").to_string()).map_err(|_| "not written")?,
        // "\n" in it: a line break (the control file is a line each).
        "type" => app.chat_in_view().ok_or("no agent")?.input.buffer().set_text(&argument.replace("\\n", "\n")),
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
        "scrollby" => app.chat_in_view().ok_or("no agent")?.scroll_by_for_tests(argument.parse().map_err(|_| "scrollby:<px>")?),
        // Labels whose text is taller than the room they were given (drawn past their end).
        "overflow" => {
            let list = app.chat_in_view().ok_or("no agent")?.messages();
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
            let list = app.chat_in_view().ok_or("no agent")?.messages();
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
        "scrollinfo" => eprintln!("SCROLL {}", app.chat_in_view().ok_or("no agent")?.scroll_info()),
        "toast" => app.show_remote_toast(app.remote_view().ok_or("no agent")?.id, argument, false),
        "snap" => snapshot(app.window.upcast_ref(), argument)?,
        // The window's render nodes, serialized (gtk4-rendernode-tool benchmark times them).
        "snapnode" => {
            let content = lay_out(app.window.upcast_ref())?;
            let snap = gtk::Snapshot::new();
            app.window.snapshot_child(&content, &snap);
            let node = snap.to_node().ok_or("nothing was drawn")?;
            std::fs::write(argument, node.serialize()).map_err(|e| e.to_string())?;
        }
        "snapchat" => {
            let list = app.chat_in_view().ok_or("no agent")?.messages();
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
                // (its child sits in the popover's own contents box)
                child.parent().ok_or("no parent")?.snapshot_child(&child, &snap);
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
        // The memory snapshot the debug log takes every 3 minutes, now.
        "memory" => {
            eprintln!("{}", crate::memory::snapshot(&[], None).0);
        }
        "stop" => app.chat_in_view().ok_or("no agent")?.press_stop(),
        // A view over the focused conversation (as a subagent's), and back.
        "aside" => app.chat_in_view().ok_or("no agent")?.show_aside(gtk::Label::new(Some("aside")).upcast_ref(), "Test"),
        "main" => {
            app.chat_in_view().ok_or("no agent")?.show_main();
        }
        // Widgets given less room than they need (with their ancestors), and every paned.
        "squeezed" => squeezed(app.window.upcast_ref()),
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
            if !app.remote_view().is_some_and(|a| a.answer(n.saturating_sub(1))) {
                return Err("no such question or option".into());
            }
        }
        "palette" if !argument.is_empty() => app.palette_run(argument)?,
        // A message passed on ("Send to another agent…"), to the picker's first agent.
        "sendto" => {
            app.send_elsewhere(argument.replace("\\n", "\n"));
            picker_first(app)?;
        }
        "pickerfirst" => picker_first(app)?,
        "findcode" => app.search_code(argument.to_owned()),
        "awayfor" => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let seconds: u64 = argument.parse().map_err(|_| "awayfor:<seconds>")?;
            crate::conn::command("window", "set_settings", serde_json::json!({ "changes": { "away_until": now + seconds } }));
        }
        "row" => app.activate_row(argument.parse().map_err(|_| "row:<n>")?),
        "agent" => app.focus_nth(argument.parse::<usize>().map_err(|_| "agent:<n>")?.saturating_sub(1)),
        "new" => app.start_in(PathBuf::from(argument)),
        "board" => app.show_project(Path::new(argument), Path::new(argument)),
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

fn squeezed(widget: &gtk::Widget) {
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
        squeezed(&c);
        child = c.next_sibling();
    }
}

fn print_items(widget: &gtk::Widget) {
    if widget.is::<gtk::Separator>() {
        eprintln!("  ----");
    } else if let Some(button) = widget.downcast_ref::<gtk::Button>() {
        let mut texts = Vec::new();
        collect_text(button.upcast_ref(), &mut texts);
        let marks = format!("{}{}", if button.is_sensitive() { "" } else { " [off]" }, if button.has_css_class("danger") { " [red]" } else { "" });
        eprintln!("  {}{marks}", texts.join("   "));
        return;
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        print_items(&w);
        child = w.next_sibling();
    }
}

fn collect_text(widget: &gtk::Widget, texts: &mut Vec<String>) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        texts.push(label.label().to_string());
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        collect_text(&w, texts);
        child = w.next_sibling();
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

/// The open picker's first entry, chosen (as Enter in it does).
fn picker_first(app: &Rc<App>) -> Result<(), String> {
    let mut found = Vec::new();
    popovers(app.window.upcast_ref(), &mut found);
    let entry = found.iter().filter_map(|p| p.child()).find(|c| c.has_css_class("picker")).and_then(|c| c.first_child()).and_downcast::<gtk::SearchEntry>();
    entry.ok_or("no picker open")?.emit_activate();
    Ok(())
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
