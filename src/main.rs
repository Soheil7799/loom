//! loom: a light, keyboard-first home for coding agents. Each agent is a Claude Code
//! session over ACP (claude-agent-acp), shown as a native chat; commands land in a job tray,
//! files in a side editor, and a scratch terminal is one key away.

mod about;
mod acp;
mod agent;
mod app;
mod chat;
mod cheatsheet;
mod column;
mod control;
mod devhooks;
mod files;
mod history;
mod hub;
mod jobs;
mod journal;
mod log;
mod memory;
mod compacted;
mod icons;
mod menu;
mod logo;
mod markdown;
mod overview;
mod notes;
mod plugin;
mod topics;
mod vault;
mod mcp;
mod picker;
mod prefs;
mod quickopen;
mod repos;
mod schedule;
mod scribe;
mod scratch;
mod settings;
mod state;
mod stt;
mod suggest;
mod telegram;
mod term;
mod theme;

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
    // `loom mcp`: the MCP server an agent runs to reach loom (no window).
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        return glib::ExitCode::from(mcp::run() as u8);
    }
    // `loom scribe-eval <cases.jsonl>`: how well the note agent does on a test set.
    if std::env::args().nth(1).as_deref() == Some("scribe-eval") {
        let cases = std::env::args().nth(2).unwrap_or_else(|| "data/scribe-cases.jsonl".to_owned());
        return glib::ExitCode::from(scribe::eval(std::path::Path::new(&cases)) as u8);
    }
    // LOOM_APP_ID: a separate instance (tests), with its own saved agents and settings.
    let app_id = std::env::var("LOOM_APP_ID").unwrap_or_else(|_| APP_ID.to_owned());
    settings::init(&app_id);
    log::name_for(&app_id);
    log::install();
    log::set(settings::get().debug_log);
    plugin::init(&app_id);
    journal::init(&app_id);
    if let Some(vault) = vault::current() {
        if let Err(err) = vault::prepare(&vault) {
            log::write("notes", &format!("vault {}: {err}", vault.display()));
        }
    }
    // `loom <folder>`: that folder's project page (in the running window, if there is one).
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
        devhooks::watch(&loom);
        quit_on_signals(&loom);
        // The window owns everything through its widgets; keep the app state alive with it.
        // SAFETY: the key stores the App and is read by nothing; it drops with the window.
        unsafe { loom.window.set_data("loom", loom.clone()) };
    });
    app.run()
}
