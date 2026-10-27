//! Following the GTK theme live. loom's colours are the user's GTK stylesheet's (what
//! noctalia or matugen write); GTK reads that file once, so when it is rewritten (a new
//! wallpaper) it is read again here, above GTK's stale copy, and the terminals recoloured.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use gtk::{gdk, gio, glib, prelude::*};

thread_local! {
    static STATUS: gtk::CssProvider = gtk::CssProvider::new();
    static RELOADED: gtk::CssProvider = gtk::CssProvider::new();
    static ADDED: Cell<bool> = const { Cell::new(false) };
    static MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static PENDING: Cell<bool> = const { Cell::new(false) };
    static TERMINALS: RefCell<Vec<glib::WeakRef<vte::Terminal>>> = const { RefCell::new(Vec::new()) };
}

/// Picks the "waiting for you" colour: of a few clear hues, the one farthest from the
/// accent (a yellow accent and a yellow "waiting" would read the same).
pub fn status_colors(widget: &impl IsA<gtk::Widget>) {
    #[allow(deprecated)]
    let accent = widget.as_ref().style_context().lookup_color("accent_color");
    let hue = accent.map_or(45.0, |c| hue(f64::from(c.red()), f64::from(c.green()), f64::from(c.blue())));
    // Orange, violet, cyan, pink: none is the red of "failed" or the green of "done".
    let candidates = [(30.0, "#ff9e3d"), (270.0, "#c3a6ff"), (190.0, "#5ccfe6"), (320.0, "#f07ad4")];
    let distance = |h: f64| {
        let d = (h - hue).abs() % 360.0;
        d.min(360.0 - d)
    };
    let (_, color) = candidates.iter().copied().fold((-1.0, "#c3a6ff"), |best, (h, c)| if distance(h) > best.0 { (distance(h), c) } else { best });
    let Some(display) = gdk::Display::default() else { return };
    STATUS.with(|css| {
        css.load_from_string(&format!("@define-color loom_wait {color};"));
        gtk::style_context_add_provider_for_display(&display, css, gtk::STYLE_PROVIDER_PRIORITY_USER + 4);
    });
}

fn hue(r: f64, g: f64, b: f64) -> f64 {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d < 1e-6 {
        return 0.0;
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h * 60.0
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
    if let Some(window) = gtk::Window::list_toplevels().into_iter().next() {
        status_colors(&window);
    }
    TERMINALS.with(|terms| {
        terms.borrow_mut().retain(|weak| match weak.upgrade() {
            Some(term) => {
                crate::term::recolor(&term);
                true
            }
            None => false,
        })
    });
    let listeners = LISTENERS.with(|l| l.borrow().clone());
    for listener in listeners {
        listener();
    }
}

thread_local! {
    /// Told when the theme's colours change (after the new ones are in).
    static LISTENERS: std::cell::RefCell<Vec<std::rc::Rc<dyn Fn()>>> = std::cell::RefCell::new(Vec::new());
}

/// `f` runs whenever the theme's colours change.
pub fn on_change(f: impl Fn() + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(std::rc::Rc::new(f)));
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
