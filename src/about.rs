//! About loom: the mark, the name, who made it, and the versions (to copy into a report).
//! A small window of its own; opening it again brings the open one forward.

use gtk::{gdk, glib, prelude::*};

/// What loom is, in a line (the welcome screen says it too).
pub const TAGLINE: &str = "Your Claude Code agents, side by side.";

/// "by shirvani.dev", the domain a link.
pub fn byline() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_markup("by <a href=\"https://shirvani.dev\">shirvani.dev</a>");
    label.add_css_class("lockup-by");
    label
}

pub fn open(parent: &impl IsA<gtk::Window>) {
    // One at a time.
    if let Some(open) = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .find(|w| w.has_css_class("about") && w.is_visible())
    {
        open.present();
        return;
    }
    let window = gtk::Window::builder().title("About loom").transient_for(parent).resizable(false).default_width(360).build();
    window.add_css_class("loom");
    window.add_css_class("about");

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("about-body");
    let mark = crate::logo::mark(64);
    mark.set_halign(gtk::Align::Center);
    mark.set_margin_bottom(14);
    body.append(&mark);
    let name = gtk::Label::new(Some("loom"));
    name.add_css_class("welcome-title");
    body.append(&name);
    let by = byline();
    by.set_margin_top(2);
    body.append(&by);
    let tagline = gtk::Label::builder().label(TAGLINE).justify(gtk::Justification::Center).wrap(true).build();
    tagline.add_css_class("dim");
    tagline.set_margin_top(12);
    body.append(&tagline);
    let adapter = crate::acp::adapter_version().unwrap_or_else(|| "—".into());
    let versions = gtk::Label::builder().label(format!("loom {} · adapter {adapter}", env!("CARGO_PKG_VERSION"))).selectable(true).build();
    for class in ["mono", "small", "dim"] {
        versions.add_css_class(class);
    }
    versions.set_margin_top(18);
    body.append(&versions);
    window.set_child(Some(&body));

    let keys = gtk::EventControllerKey::new();
    let w = window.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            w.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    window.present();
}
