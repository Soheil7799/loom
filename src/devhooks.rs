//! Driving a test instance from outside (unused in normal runs).
//!
//! `LOOM_CONTROL=/path/cmds`: every 300 ms, if that file exists, its lines are run in order
//! and the file is deleted. Lines:
//!
//! - `prompt:<text>`: send text from the focused agent's input
//! - `snap:<file.png>`: save a picture of the window (works on an unseen workspace)
//! - `scratch`, `files`, `switcher`, `inbox`, `help`, `peek`: what their keys do
//! - `run:<command>`: run a command in the tray
//! - `open:<file>`: open a file in the editor (relative to the focused agent's folder)
//! - `mode`: next permission mode
//! - `split`: what Ctrl+\ does
//! - `answer:<n>`: answer the focused agent's first question with option n (from 1)
//! - `agent:<n>`: focus the n-th agent (from 1)
//! - `new:<folder>`: start an agent in a folder
//! - `close`: close the focused agent (asks first if it is busy)
//! - `quit`

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::{glib, prelude::*};

use crate::app::App;

pub fn watch(app: &Rc<App>) {
    let Some(path) = std::env::var_os("LOOM_CONTROL").map(PathBuf::from) else { return };
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
        "snap" => snapshot(&app.window, argument)?,
        "answer" => {
            let n: usize = argument.parse().map_err(|_| "answer:<n>")?;
            if !app.focused_agent().ok_or("no agent")?.answer(0, n.saturating_sub(1)) {
                return Err("no such question or option".into());
            }
        }
        "agent" => app.focus_nth(argument.parse::<usize>().map_err(|_| "agent:<n>")?.saturating_sub(1)),
        "new" => app.start_in(PathBuf::from(argument)),
        "quit" => app.window.close(),
        other => app.act(other, argument),
    }
    Ok(())
}

fn snapshot(window: &gtk::ApplicationWindow, path: &str) -> Result<(), String> {
    let content = window.child().ok_or("window has no content")?;
    // A window the compositor isn't drawing (another workspace) has its layout pending
    // forever: lay it out here, at its last size or else the size it opened at.
    let (width, height) = match (content.width(), content.height()) {
        (0, _) | (_, 0) => window.default_size(),
        size => size,
    };
    content.allocate(width.max(400), height.max(300), -1, None);
    let snap = gtk::Snapshot::new();
    window.snapshot_child(&content, &snap);
    let node = snap.to_node().ok_or("nothing was drawn")?;
    let renderer = window.renderer().ok_or("window has no renderer")?;
    renderer.render_texture(&node, None).save_to_png(path).map_err(|e| e.to_string())
}
