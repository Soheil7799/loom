//! loom: a light, keyboard-first home for coding agents. Each agent is a Claude Code
//! session (the `claude` program, driven over its stream-json protocol), shown as a native chat; commands land in a job tray,
//! files in a side editor, and a scratch terminal is one key away.

mod about;
use loom_core::bridges;
use loom_core::provider;
use loom_core::agent;
mod app;
mod board;
mod browser;
mod chat;
mod chat_view;
mod cheatsheet;
mod column;
#[cfg_attr(not(debug_assertions), allow(dead_code))]
mod devhooks;
mod conn;
mod remote_agent;
mod feed;
mod files;
mod history;
mod hub;
mod jobs;
use loom_core::journal;
use loom_core::log;
use loom_core::memory;
mod icons;
mod menu;
mod logo;
mod markdown;
mod overview;
mod notes_chat;
mod notes_panel;
use loom_core::plugin;
use loom_core::topics;
use loom_core::vault;
use loom_core::mcp;
mod picker;
mod prefs;
use loom_core::quickopen;
use loom_core::repos;
mod schedule;
use loom_core::scribe;
mod scratch;
use loom_core::settings;
use loom_core::state;
use loom_core::stt;
mod term;
mod theme;
mod tube;
mod workflow;

use gtk::{gdk, gio, glib, prelude::*};

const APP_ID: &str = "dev.shirvani.Loom";

/// The look (see style.css): the GTK theme's colours, so it follows the wallpaper.
const CSS: &str = include_str!("style.css");

thread_local! {
    static RUNNING: std::cell::RefCell<std::rc::Weak<app::App>> = std::cell::RefCell::new(std::rc::Weak::new());
}

// GLib's own Unix signal source (not wrapped by these bindings): its handler runs on the
// main loop, so it may touch the window.
unsafe extern "C" {
    fn g_unix_signal_add(signum: i32, handler: extern "C" fn(*mut std::ffi::c_void) -> i32, data: *mut std::ffi::c_void) -> u32;
}

extern "C" fn on_signal(_: *mut std::ffi::c_void) -> i32 {
    if let Some(loom) = RUNNING.with(|r| r.borrow().upgrade()) {
        loom.quit_now();
    }
    0 // G_SOURCE_REMOVE
}

/// kill, Ctrl+C in a terminal, the session ending: quit properly, stopping the agents.
fn quit_on_signals(loom: &std::rc::Rc<app::App>) {
    RUNNING.with(|r| *r.borrow_mut() = std::rc::Rc::downgrade(loom));
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: a plain C callback with no data, on the default main context.
        unsafe { g_unix_signal_add(signal, on_signal, std::ptr::null_mut()) };
    }
}

fn main() -> glib::ExitCode {
    // The core's agents run on this thread, in GLib's main loop.
    loom_core::rt::set_host(
        |future| {
            glib::spawn_future_local(future);
        },
        |after, f| {
            glib::timeout_add_local_once(after, f);
        },
    );
    // Each mode below (not the window) is in loom_core::api::socket::NOT_THE_WINDOW too: loomd
    // gives the window's rights to `loom` run as anything else.
    // `loom mcp`: the MCP server an agent runs to reach loom (no window).
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        return glib::ExitCode::from(mcp::run() as u8);
    }
    // `loom sandbox -- <program>`: an agent's program held to the strict sandbox (sandbox.rs).
    if std::env::args().nth(1).as_deref() == Some("sandbox") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        return glib::ExitCode::from(loom_core::sandbox::run(&args) as u8);
    }
    // `loom api …`: the core API's command-line client (no window).
    if std::env::args().nth(1).as_deref() == Some("api") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        return glib::ExitCode::from(loom_core::api::cli::run(&args) as u8);
    }
    // `loom bridge <name>`: a bridge, started by loom itself (see bridges.rs).
    if std::env::args().nth(1).as_deref() == Some("bridge") {
        return match std::env::args().nth(2).as_deref() {
            Some("telegram") => {
                // Its own process, on GLib's main loop like the app.
                let context = glib::MainContext::default();
                let Ok(_owner) = context.acquire() else { return glib::ExitCode::from(1) };
                glib::ExitCode::from(bridges::telegram::run(|| glib::MainLoop::new(Some(&context), false).run()) as u8)
            }
            // Its own program: iroh and its runtime stay out of loom.
            Some("remote") => {
                use std::os::unix::process::CommandExt;
                let Some(program) = bridges::remote_program() else {
                    eprintln!("loom bridge remote: loom-remote is not installed beside loom");
                    return glib::ExitCode::from(2);
                };
                let err = std::process::Command::new(program).arg("bridge").exec();
                eprintln!("loom bridge remote: {err}");
                glib::ExitCode::from(1)
            }
            _ => {
                eprintln!("loom bridge: no such bridge");
                glib::ExitCode::from(2)
            }
        };
    }
    // `loom scribe-eval <cases.jsonl> [vault]`: how well the note agent does on a test set
    // (with that vault's topics, rules learned included; else the default topics).
    if std::env::args().nth(1).as_deref() == Some("scribe-eval") {
        let cases = std::env::args().nth(2).unwrap_or_else(|| "data/scribe-cases.jsonl".to_owned());
        let topics = std::env::args().nth(3).map(std::path::PathBuf::from);
        return glib::ExitCode::from(scribe::eval(std::path::Path::new(&cases), topics.as_deref()) as u8);
    }
    // LOOM_APP_ID: a separate instance (tests), with its own saved agents and settings.
    let app_id = std::env::var("LOOM_APP_ID").unwrap_or_else(|_| APP_ID.to_owned());
    // The window is loomd's client (another window is GApplication's to raise).
    let socket = loom_core::api::socket::socket_path(&app_id);
    let daemon = loom_core::api::socket::running(&socket).as_deref() == Some("daemon");
    // Its agents are loomd's, the window runs none of its own. None runs: started now
    // (closing the window stops it, unless Settings keep agents working).
    if !daemon && let Err(err) = conn::start_loomd(&socket, &app_id) {
        eprintln!("loom: {err}");
        return glib::ExitCode::from(1);
    }
    conn::use_remote(loom_core::api::remote::Remote::connect(socket, "loom window"));
    settings::init(&app_id);
    log::name_for(&app_id);
    log::install();
    // GTK's own warnings go into the log too.
    glib::log_set_writer_func(|level, fields| {
        if log::on() {
            let message = fields.iter().find(|f| f.key() == "MESSAGE").and_then(|f| f.value_str()).unwrap_or("");
            let domain = fields.iter().find(|f| f.key() == "GLIB_DOMAIN").and_then(|f| f.value_str()).unwrap_or("glib");
            log::write(&format!("{domain} {level:?}"), message);
        }
        glib::log_writer_default(level, fields)
    });
    log::set(settings::get().debug_log);
    plugin::init(&app_id);
    journal::init(&app_id);
    // `loom <folder>`: that folder's project page (in the running window, if there is one).
    // Dialogs (no application of their own) take the program name as their app id: this one's.
    glib::set_prgname(Some(app_id.as_str()));
    theme::load_fonts();
    let app = gtk::Application::builder().application_id(app_id).flags(gio::ApplicationFlags::HANDLES_OPEN).build();
    app.connect_open(|app, files, _| {
        app.activate();
        let Some(window) = app.active_window() else { return };
        // SAFETY: "loom" holds the App, set when the window was made (below).
        let Some(loom) = (unsafe { window.data::<std::rc::Rc<app::App>>("loom") }) else { return };
        let loom = unsafe { loom.as_ref() }.clone();
        for file in files {
            if let Some(path) = file.path().filter(|p| p.is_dir()) {
                loom.show_project(&app::project_of(&path), &path);
            }
        }
    });
    app.connect_startup(|_| {
        let css = gtk::CssProvider::new();
        css.load_from_string(CSS);
        theme::watch();
        if let Some(display) = gdk::Display::default() {
            // Above the user's GTK stylesheet (which it reads colours from).
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_USER + 2);
        }
    });
    app.connect_activate(|app| {
        // A second launch brings the window back rather than opening another.
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        let loom = app::App::new(app);
        loom.window.present();
        // Driven from a file of commands (LOOM_CONTROL): test builds only. A release build is
        // never driven that way, so a process outside the window can't press its buttons.
        #[cfg(debug_assertions)]
        devhooks::watch(&loom);
        quit_on_signals(&loom);
        // The window owns everything through its widgets; keep the app state alive with it.
        // SAFETY: the key stores the App and is read by nothing; it drops with the window.
        unsafe { loom.window.set_data("loom", loom.clone()) };
    });
    app.run()
}
