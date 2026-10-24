//! Following the GTK theme live. loom's colours are the user's GTK stylesheet's (what
//! noctalia or matugen write); GTK reads that file once, so when it is rewritten (a new
//! wallpaper) it is read again here, above GTK's stale copy, and the terminals recoloured.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use gtk::{gdk, gio, glib, prelude::*};

thread_local! {
    static RELOADED: gtk::CssProvider = gtk::CssProvider::new();
    static ADDED: Cell<bool> = const { Cell::new(false) };
    static MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static PENDING: Cell<bool> = const { Cell::new(false) };
    static TERMINALS: RefCell<Vec<glib::WeakRef<vte::Terminal>>> = const { RefCell::new(Vec::new()) };
}

/// Starts watching the GTK 4 config folder.
pub fn watch() {
    let dir = glib::user_config_dir().join("gtk-4.0");
    let Ok(monitor) = gio::File::for_path(&dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, None::<&gio::Cancellable>)
    else {
        return;
    };
    monitor.connect_changed(|_, file, other, event| {
        // Writers replace a file by renaming a temporary one over it: then the css name is
        // the event's other file.
        let is_css = |f: &gio::File| f.basename().is_some_and(|name| name.extension().is_some_and(|e| e == "css"));
        if (is_css(file) || other.is_some_and(is_css)) && !matches!(event, gio::FileMonitorEvent::AttributeChanged) {
            schedule();
        }
    });
    MONITOR.with(|m| *m.borrow_mut() = Some(monitor));
}

/// Theme writers touch several files in a row: reload once they are done.
fn schedule() {
    if PENDING.with(|p| p.replace(true)) {
        return;
    }
    glib::timeout_add_local_once(Duration::from_millis(400), || {
        PENDING.with(|p| p.set(false));
        reload();
    });
}

fn reload() {
    let Some(display) = gdk::Display::default() else { return };
    let path = glib::user_config_dir().join("gtk-4.0/gtk.css");
    RELOADED.with(|css| {
        if path.exists() {
            // Imports in it resolve against its own folder.
            css.load_from_path(&path);
        } else {
            css.load_from_string("");
        }
        if !ADDED.with(|a| a.replace(true)) {
            // Over the copy GTK read at startup, under loom's own rules.
            gtk::style_context_add_provider_for_display(&display, css, gtk::STYLE_PROVIDER_PRIORITY_USER + 1);
        }
    });
    TERMINALS.with(|terms| {
        terms.borrow_mut().retain(|weak| match weak.upgrade() {
            Some(term) => {
                crate::term::recolor(&term);
                true
            }
            None => false,
        })
    });
}

/// Every terminal takes `font` (from the settings).
pub fn refont(font: &str) {
    let desc = gtk::pango::FontDescription::from_string(font);
    TERMINALS.with(|terms| {
        terms.borrow_mut().retain(|weak| match weak.upgrade() {
            Some(term) => {
                use vte::prelude::*;
                term.set_font(Some(&desc));
                true
            }
            None => false,
        })
    });
}

/// Keeps `term` in the theme's colours from now on.
pub fn follow(term: &vte::Terminal) {
    TERMINALS.with(|terms| terms.borrow_mut().push(term.downgrade()));
}
