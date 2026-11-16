//! Following the GTK theme live. loom's colours are the user's GTK stylesheet's (what
//! noctalia or matugen write); GTK reads that file once, so when it is rewritten (a new
//! wallpaper) it is read again here, above GTK's stale copy, and the terminals recoloured.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use gtk::{gdk, gio, glib, prelude::*};

/// One palette: window, sidebar, pane, bubble, line, fg, quiet, accent, accentfg, wait, ok, err.
type Palette = [&'static str; 12];

/// loom's own themes, dark and light (research/palette-3/palettes.json, their contrast and
/// colour-blind checks there). Nocturne is loom/default.
pub const THEMES: &[(&str, &str, Palette, Palette)] = &[
    ("nocturne", "Nocturne", ["#0a1729", "#051122", "#0f1f32", "#0f385a", "#26364c", "#e1eaf7", "#9bacc4", "#75c8fc", "#00111e", "#fec84b", "#60db89", "#fd7277"],
        ["#eef4fb", "#e7eff7", "#f8fbff", "#d3e9fc", "#d2dae2", "#17202a", "#586676", "#0067ab", "#fafcfe", "#826000", "#008341", "#92002c"]),
    ("klein", "Klein", ["#111315", "#0c0d10", "#18191d", "#162a67", "#2d2f32", "#e8e9eb", "#a9abaf", "#7099ff", "#060e25", "#febf32", "#56d57b", "#f9667c"],
        ["#f2f3f6", "#eceef1", "#fafbfd", "#dbe6ff", "#d7d9dc", "#1d1f23", "#62656b", "#1b3ce3", "#fafcff", "#875d00", "#00843b", "#9b0038"]),
    ("kingfisher", "Kingfisher", ["#0f1315", "#0a0e10", "#151a1d", "#00354b", "#2b3032", "#e7eaeb", "#a6acaf", "#43c0f3", "#00121b", "#fea92f", "#5edb81", "#fa618c"],
        ["#f0f4f6", "#eaeff1", "#f9fbfd", "#cdeefd", "#d5dadc", "#1a2023", "#60696d", "#00769f", "#f9fcfe", "#935a00", "#00843b", "#990047"]),
    ("emerald", "Emerald", ["#0f1413", "#0a0e0d", "#151b1a", "#003a2a", "#2a312f", "#e7eae9", "#a6acab", "#37d59f", "#00140b", "#ffbe3d", "#84de70", "#f86680"],
        ["#f0f5f3", "#eaefed", "#f9fcfb", "#cef4e3", "#d5dad9", "#1a211f", "#626b68", "#007c5a", "#f9fdfb", "#8c6100", "#368223", "#9b0038"]),
    ("peony", "Peony", ["#151113", "#100c0e", "#1c181a", "#4d1e31", "#322e30", "#ebe8ea", "#aea9ab", "#f478ab", "#1f0510", "#fdce3e", "#5edb81", "#fd736f"],
        ["#f6f2f4", "#f0ecee", "#fcfafb", "#fcdfe8", "#dcd8d9", "#221e20", "#6b6467", "#b9186d", "#fefbfc", "#7f6200", "#00843b", "#8a021c"]),
];

/// The GTK colour names a palette sets (libadwaita's, which adw-gtk3 and loom's CSS read, and
/// GTK 3's older ones), each from its role.
fn colours(p: &Palette, dark: bool) -> Vec<(&'static str, String)> {
    let [window, sidebar, pane, _bubble, line, fg, quiet, accent, on_accent, wait, ok, err] = p.map(str::to_owned);
    let shade = if dark { "rgba(0, 0, 0, 0.36)" } else { "rgba(0, 0, 0, 0.07)" }.to_owned();
    vec![
        ("window_bg_color", window.clone()),
        ("window_fg_color", fg.clone()),
        ("view_bg_color", pane.clone()),
        ("view_fg_color", fg.clone()),
        ("headerbar_bg_color", window.clone()),
        ("headerbar_fg_color", fg.clone()),
        ("headerbar_backdrop_color", window.clone()),
        ("headerbar_border_color", line.clone()),
        ("headerbar_shade_color", shade.clone()),
        ("sidebar_bg_color", sidebar.clone()),
        ("sidebar_fg_color", fg.clone()),
        ("sidebar_backdrop_color", sidebar.clone()),
        ("sidebar_border_color", line.clone()),
        ("secondary_sidebar_bg_color", sidebar.clone()),
        ("secondary_sidebar_fg_color", fg.clone()),
        ("card_bg_color", pane.clone()),
        ("card_fg_color", fg.clone()),
        ("card_shade_color", shade.clone()),
        ("popover_bg_color", pane.clone()),
        ("popover_fg_color", fg.clone()),
        ("popover_shade_color", shade.clone()),
        ("dialog_bg_color", window.clone()),
        ("dialog_fg_color", fg.clone()),
        ("thumbnail_bg_color", pane.clone()),
        ("thumbnail_fg_color", fg.clone()),
        ("shade_color", shade),
        ("accent_color", accent.clone()),
        ("accent_bg_color", accent.clone()),
        ("accent_fg_color", on_accent.clone()),
        ("destructive_color", err.clone()),
        ("destructive_bg_color", err.clone()),
        ("destructive_fg_color", on_accent.clone()),
        ("error_color", err.clone()),
        ("error_bg_color", err.clone()),
        ("error_fg_color", on_accent.clone()),
        ("success_color", ok.clone()),
        ("success_bg_color", ok.clone()),
        ("success_fg_color", on_accent.clone()),
        ("warning_color", wait.clone()),
        ("warning_bg_color", wait.clone()),
        ("warning_fg_color", on_accent.clone()),
        ("borders", line.clone()),
        ("theme_bg_color", window),
        ("theme_fg_color", fg.clone()),
        ("theme_base_color", pane),
        ("theme_text_color", fg),
        ("theme_selected_bg_color", accent.clone()),
        ("theme_selected_fg_color", on_accent),
        ("insensitive_fg_color", quiet.clone()),
        ("loom_quiet", quiet),
        ("loom_ok", ok),
        ("loom_err", err),
        ("loom_wait", wait.clone()),
        ("loom_warn", wait),
        ("loom_run", accent),
        ("loom_line", line),
    ]
}

/// The stylesheet for those names: as named colours and as the CSS variables newer themes read.
fn stylesheet(colours: &[(&str, String)]) -> String {
    let named: String = colours.iter().map(|(n, c)| format!("@define-color {n} {c};\n")).collect();
    let vars: String = colours.iter().filter(|(n, _)| !n.starts_with("loom_") && !n.starts_with("theme_")).map(|(n, c)| format!("--{}: {c}; ", n.replace('_', "-"))).collect();
    format!("{named}:root {{ {vars}}}\n")
}

/// The desktop's own GTK theme and dark preference, before loom changed them.
struct Desktop {
    theme: Option<glib::GString>,
    dark: bool,
}

thread_local! {
    /// The chosen theme's colours, over the user's GTK stylesheet and loom's own defaults.
    static COLOURS: gtk::CssProvider = gtk::CssProvider::new();
    /// Nocturne's, under every GTK theme: the names loom's CSS reads, where the theme lacks them
    /// (GTK's own Default theme has none of them).
    static FALLBACK: gtk::CssProvider = gtk::CssProvider::new();
    static DESKTOP: RefCell<Option<Desktop>> = const { RefCell::new(None) };
    static APPLIED: RefCell<(String, String)> = const { RefCell::new((String::new(), String::new())) };
    static STATUS: gtk::CssProvider = gtk::CssProvider::new();
    static RELOADED: gtk::CssProvider = gtk::CssProvider::new();
    static ADDED: Cell<bool> = const { Cell::new(false) };
    static MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static PENDING: Cell<bool> = const { Cell::new(false) };
    static TERMINALS: RefCell<Vec<glib::WeakRef<vte::Terminal>>> = const { RefCell::new(Vec::new()) };
}

/// Uses `theme` ("system": the desktop's GTK theme, or one of `THEMES`) in `mode` ("dark",
/// "light", or "": as the desktop prefers). Nothing is done when they are as they were.
pub fn apply(theme: &str, mode: &str) {
    let (Some(display), Some(settings)) = (gdk::Display::default(), gtk::Settings::default()) else { return };
    if APPLIED.with_borrow(|a| a.0 == theme && a.1 == mode) {
        return;
    }
    APPLIED.with_borrow_mut(|a| *a = (theme.to_owned(), mode.to_owned()));
    let first = DESKTOP.with_borrow(Option::is_none);
    let desktop_dark = DESKTOP.with_borrow_mut(|d| d.get_or_insert_with(|| Desktop { theme: settings.gtk_theme_name(), dark: settings.is_gtk_application_prefer_dark_theme() }).dark);
    // shortcut: the desktop's dark preference is read once; a change to it while loom runs is seen at its next start
    let dark = match mode {
        "dark" => true,
        "light" => false,
        _ => desktop_dark,
    };
    settings.set_gtk_application_prefer_dark_theme(dark);
    let own = THEMES.iter().find(|t| t.0 == theme);
    // loom's colours reach every widget through adw-gtk3's stylesheet (it reads the named
    // colours); without it, GTK's own theme draws the controls in its greys.
    let wanted = match own {
        Some(_) if adw_gtk3() => Some(glib::GString::from("adw-gtk3")),
        _ => DESKTOP.with_borrow(|d| d.as_ref().and_then(|d| d.theme.clone())),
    };
    if settings.gtk_theme_name() != wanted && (own.is_some() || !first) {
        settings.set_gtk_theme_name(wanted.as_deref());
    }
    let nocturne = &THEMES[0];
    let fallback = stylesheet(&colours(if dark { &nocturne.2 } else { &nocturne.3 }, dark));
    let chosen = own.map_or_else(String::new, |t| stylesheet(&colours(if dark { &t.2 } else { &t.3 }, dark)));
    FALLBACK.with(|css| css.load_from_string(&fallback));
    COLOURS.with(|css| css.load_from_string(&chosen));
    if first {
        FALLBACK.with(|css| gtk::style_context_add_provider_for_display(&display, css, gtk::STYLE_PROVIDER_PRIORITY_THEME - 1));
        // Over the "waiting" colour picked for a desktop theme (status_colors): a palette has its own.
        COLOURS.with(|css| gtk::style_context_add_provider_for_display(&display, css, gtk::STYLE_PROVIDER_PRIORITY_USER + 5));
        return;
    }
    changed();
}

/// Whether adw-gtk3's GTK 4 stylesheet is installed (Arch: adw-gtk-theme).
fn adw_gtk3() -> bool {
    let mut dirs = vec![glib::user_data_dir().join("themes"), glib::home_dir().join(".themes")];
    dirs.extend(glib::system_data_dirs().iter().map(|d| d.join("themes")));
    dirs.iter().any(|d| d.join("adw-gtk3/gtk-4.0/gtk.css").is_file())
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
    changed();
}

/// The colours changed: what reads them at runtime takes the new ones.
fn changed() {
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

/// The brand's font (Onest, OFL: data/fonts/OFL-Onest.txt) ships inside loom, so the window
/// looks the same where it isn't installed: written once to the cache, given to fontconfig as
/// this program's own font. Before GTK starts (Pango then finds it), and with any Pango (its
/// own add_font_file needs 1.56).
pub fn load_fonts() {
    #[link(name = "fontconfig")]
    unsafe extern "C" {
        fn FcConfigAppFontAddFile(config: *mut std::ffi::c_void, file: *const std::ffi::c_char) -> std::ffi::c_int;
    }
    const FONTS: [(&str, &[u8]); 2] = [
        ("onest-normal-latin.woff2", include_bytes!("../data/fonts/onest-normal-latin.woff2")),
        ("onest-normal-latin-ext.woff2", include_bytes!("../data/fonts/onest-normal-latin-ext.woff2")),
    ];
    let dir = gtk::glib::user_cache_dir().join("loom").join("fonts");
    for (name, bytes) in FONTS {
        let path = dir.join(name);
        let fresh = std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64);
        if !fresh && std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, bytes)).is_err() {
            continue;
        }
        use std::os::unix::ffi::OsStrExt;
        let Ok(file) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { continue };
        // SAFETY: a NUL-terminated path; a null config is fontconfig's current one.
        if unsafe { FcConfigAppFontAddFile(std::ptr::null_mut(), file.as_ptr()) } == 0 {
            eprintln!("loom: font {name}: fontconfig refused it");
        }
    }
}
