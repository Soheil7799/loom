//! Right-click menus: one look and one behaviour everywhere. Items in sections, each with an
//! optional key hint; Up/Down move between them, Enter or a click runs one, Esc closes.

use gtk::prelude::*;
use gtk::{gdk, glib};
use std::rc::Rc;

struct Item {
    label: String,
    hint: String,
    danger: bool,
    enabled: bool,
    act: Rc<dyn Fn()>,
}

#[derive(Default)]
pub struct Menu {
    sections: Vec<Vec<Item>>,
}

impl Menu {
    pub fn new() -> Menu {
        Menu { sections: vec![Vec::new()] }
    }

    pub fn item(mut self, label: &str, act: impl Fn() + 'static) -> Menu {
        self.sections.last_mut().expect("a section").push(Item { label: label.to_owned(), hint: String::new(), danger: false, enabled: true, act: Rc::new(act) });
        self
    }

    /// The last item's key, shown at its right ("Ctrl+R").
    pub fn hint(mut self, keys: &str) -> Menu {
        if let Some(item) = self.sections.last_mut().and_then(|s| s.last_mut()) {
            item.hint = keys.to_owned();
        }
        self
    }

    /// The last item destroys or removes something: drawn in red.
    pub fn danger(mut self) -> Menu {
        if let Some(item) = self.sections.last_mut().and_then(|s| s.last_mut()) {
            item.danger = true;
        }
        self
    }

    /// The last item shown but greyed out (not possible here and now).
    pub fn enabled(mut self, enabled: bool) -> Menu {
        if let Some(item) = self.sections.last_mut().and_then(|s| s.last_mut()) {
            item.enabled = enabled;
        }
        self
    }

    /// A line, then a new section.
    pub fn section(mut self) -> Menu {
        if self.sections.last().is_some_and(|s| !s.is_empty()) {
            self.sections.push(Vec::new());
        }
        self
    }

    /// Opens at (x, y) in `on`'s coordinates. The menu goes away with its popover.
    pub fn popup(self, on: &impl IsA<gtk::Widget>, x: f64, y: f64) {
        let on = on.as_ref();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let popover = gtk::Popover::builder().has_arrow(false).position(gtk::PositionType::Bottom).halign(gtk::Align::Start).child(&content).build();
        popover.add_css_class("context-menu");
        popover.set_parent(on);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        let mut first = true;
        for section in self.sections.into_iter().filter(|s| !s.is_empty()) {
            if !first {
                content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            }
            first = false;
            for item in section {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 24);
                line.append(&gtk::Label::builder().label(&item.label).xalign(0.0).hexpand(true).build());
                if !item.hint.is_empty() {
                    let hint = gtk::Label::new(Some(&item.hint));
                    hint.add_css_class("menu-hint");
                    line.append(&hint);
                }
                let button = gtk::Button::builder().child(&line).sensitive(item.enabled).build();
                button.add_css_class("flat");
                button.add_css_class("menu-item");
                if item.danger {
                    button.add_css_class("danger");
                }
                let (held, act) = (popover.downgrade(), item.act.clone());
                button.connect_clicked(move |_| {
                    if let Some(popover) = held.upgrade() {
                        popover.popdown();
                    }
                    // After the menu is gone (a dialog it opens takes the focus cleanly).
                    let act = act.clone();
                    glib::idle_add_local_once(move || act());
                });
                content.append(&button);
            }
        }
        // Up and Down go from item to item, round from the last to the first (Tab does too);
        // Home and End to the first and the last.
        let keys = gtk::EventControllerKey::new();
        let moving = content.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let items: Vec<gtk::Widget> = std::iter::successors(moving.first_child(), |w| w.next_sibling()).filter(|w| w.is::<gtk::Button>() && w.is_sensitive()).collect();
            let at = items.iter().position(|w| w.has_focus());
            let to = match (key, at) {
                (gdk::Key::Down, Some(i)) => (i + 1) % items.len(),
                (gdk::Key::Up, Some(i)) => (i + items.len() - 1) % items.len(),
                (gdk::Key::Down | gdk::Key::Home, _) => 0,
                (gdk::Key::Up | gdk::Key::End, _) => items.len().saturating_sub(1),
                _ => return glib::Propagation::Proceed,
            };
            if let Some(item) = items.get(to) {
                item.grab_focus();
            }
            glib::Propagation::Stop
        });
        popover.add_controller(keys);
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
        content.child_focus(gtk::DirectionType::TabForward);
    }
}

/// Puts `text` on the clipboard.
pub fn copy(widget: &impl IsA<gtk::Widget>, text: &str) {
    widget.as_ref().clipboard().set_text(text);
}

/// Opens `path` (a folder, or a file in its app) the way the desktop does.
pub fn launch(widget: &impl IsA<gtk::Widget>, path: &std::path::Path) {
    let window = widget.as_ref().root().and_downcast::<gtk::Window>();
    gtk::FileLauncher::new(Some(&gtk::gio::File::for_path(path))).launch(window.as_ref(), None::<&gtk::gio::Cancellable>, |_| {});
}

/// Shows `path` in the file manager, selected in its folder.
pub fn reveal(widget: &impl IsA<gtk::Widget>, path: &std::path::Path) {
    let window = widget.as_ref().root().and_downcast::<gtk::Window>();
    gtk::FileLauncher::new(Some(&gtk::gio::File::for_path(path))).open_containing_folder(window.as_ref(), None::<&gtk::gio::Cancellable>, |_| {});
}

/// A right click on `widget` (any button 3 press), or the Menu key or Shift+F10 while
/// something in it has the keyboard: `f` with the point, in its coordinates.
pub fn on_right_click(widget: &impl IsA<gtk::Widget>, f: impl Fn(f64, f64) + 'static) {
    let f = Rc::new(f);
    let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
    let clicked = f.clone();
    click.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        clicked(x, y);
    });
    widget.as_ref().add_controller(click);
    on_menu_key(widget, move |x, y| f(x, y));
}

/// The Menu key or Shift+F10 while something in `widget` has the keyboard: `f` with a point
/// on that thing (its left part, half way down), in `widget`'s coordinates.
pub fn on_menu_key(widget: &impl IsA<gtk::Widget>, f: impl Fn(f64, f64) + 'static) {
    let keys = gtk::EventControllerKey::builder().propagation_phase(gtk::PropagationPhase::Capture).build();
    let at = widget.as_ref().downgrade();
    keys.connect_key_pressed(move |_, key, _, state| {
        let menu_key = key == gdk::Key::Menu || (key == gdk::Key::F10 && state.contains(gdk::ModifierType::SHIFT_MASK));
        let Some(widget) = at.upgrade().filter(|_| menu_key) else { return glib::Propagation::Proceed };
        let focus = widget.root().and_then(|r| r.focus()).filter(|f| f == &widget || f.is_ancestor(&widget));
        let point = focus.and_then(|f| f.compute_bounds(&widget)).map_or((16.0, 16.0), |b| (f64::from(b.x()) + 16.0, f64::from(b.y() + b.height() / 2.0)));
        f(point.0, point.1);
        glib::Propagation::Stop
    });
    widget.as_ref().add_controller(keys);
}
