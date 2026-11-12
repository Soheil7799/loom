//! A project's browser in its tray (loom_core::browser runs it, in loomd): a chip while it
//! runs, and docked above the tray, the page live (its screencast) with the user's clicks,
//! scrolls and keys passed to it, so they can watch an agent, log in, or solve a check. Their
//! own mouse and focus stay theirs: the browser draws on a screen of its own.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::{gdk, glib, prelude::*};
use loom_core::browser::{Screencast, Shown};
use serde_json::{Value, json};

pub struct Browser {
    project: PathBuf,
    /// The tray's chip (shown while the browser runs).
    pub chip: gtk::Button,
    /// The view, docked above the tray while open.
    pub frame: gtk::Box,
    picture: gtk::Picture,
    address: gtk::Label,
    cast: RefCell<Option<Screencast>>,
    /// The browser's port (0: not running).
    port: Cell<u16>,
    /// The page's size in CSS pixels, and the last frame's in pixels.
    page: Cell<(f64, f64)>,
    pointer: Cell<(f64, f64)>,
    /// `shown` as last seen: an agent asking the user to look raises it.
    shown: Cell<u64>,
    /// `browsers` was seen once (asks made before the window started aren't shown again).
    synced: Cell<bool>,
    /// Told to dock or undock the view (the tray holds the dock).
    pub on_dock: RefCell<Option<Rc<dyn Fn(bool)>>>,
    me: Weak<Browser>,
}

impl Browser {
    pub fn new(project: PathBuf) -> Rc<Browser> {
        let chip = gtk::Button::builder().label("Browser").tooltip_text("This project's browser: what agents browse; open it to watch, log in or click").visible(false).build();
        chip.add_css_class("chip");
        chip.add_css_class("flat");
        let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::Contain).can_shrink(true).hexpand(true).vexpand(true).focusable(true).build();
        picture.add_css_class("browser-page");
        picture.set_size_request(-1, 420);
        let address = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Middle).selectable(true).build();
        address.add_css_class("mono");
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 4);
        frame.add_css_class("docked");
        frame.add_css_class("browser");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.append(&address);
        frame.append(&header);
        frame.append(&picture);
        let browser = Rc::new_cyclic(|me| Browser {
            project,
            chip,
            frame,
            picture,
            address,
            cast: RefCell::default(),
            port: Cell::new(0),
            page: Cell::new((0.0, 0.0)),
            pointer: Cell::new((0.0, 0.0)),
            shown: Cell::new(0),
            synced: Cell::new(false),
            on_dock: RefCell::default(),
            me: me.clone(),
        });
        let button = |label: &str, tip: &str, act: fn(&Rc<Browser>)| {
            let b = gtk::Button::with_label(label);
            b.add_css_class("flat");
            b.add_css_class("small");
            b.set_tooltip_text(Some(tip));
            let weak = browser.me.clone();
            b.connect_clicked(move |_| {
                if let Some(browser) = weak.upgrade() {
                    act(&browser);
                }
            });
            b
        };
        header.append(&button("Back", "The page before", |b| b.eval("history.back()")));
        header.append(&button("Reload", "Load the page again", |b| b.send("Page.reload", json!({}))));
        header.append(&button("Unpin", "Back to the tray (it keeps running)", |b| b.dock(false)));
        header.append(&button("Stop", "Stop the browser (logins stay for next time)", |b| {
            crate::conn::command("browser", "browser_stop", json!({ "project": b.project }));
            b.dock(false);
        }));
        let weak = browser.me.clone();
        browser.chip.connect_clicked(move |_| {
            if let Some(browser) = weak.upgrade() {
                browser.dock(browser.cast.borrow().is_none());
            }
        });
        browser.input();
        browser
    }

    /// Its item in loomd's `browsers` (None: it doesn't run).
    pub fn show(&self, item: Option<&Value>) {
        let first = !self.synced.replace(true);
        let port = item.and_then(|i| i["port"].as_u64()).unwrap_or(0) as u16;
        self.chip.set_visible(port != 0);
        if let Some(item) = item {
            self.chip.set_label(&format!("Browser · {}", item["name"].as_str().unwrap_or("")));
        }
        self.port.set(port);
        if port == 0 {
            // started again later, it counts its asks from 0
            self.shown.set(0);
            self.dock(false);
            return;
        }
        // An agent asks the user to look (a login, a check), since the window first saw it:
        // in front.
        let shown = item.and_then(|i| i["shown"].as_u64()).unwrap_or(0);
        let before = self.shown.replace(shown);
        if !first && shown > before {
            self.dock(true);
            self.picture.grab_focus();
        }
    }

    /// Docks the view and follows the page (or stops following and undocks).
    pub fn dock(&self, open: bool) {
        let Some(me) = self.me.upgrade() else { return };
        if open && self.port.get() != 0 {
            if self.cast.borrow().is_none() {
                self.follow();
                // Kept running while the user may look at it (loomd stops it unused).
                let weak = self.me.clone();
                glib::timeout_add_local(Duration::from_secs(300), move || match weak.upgrade() {
                    Some(b) if b.cast.borrow().is_some() => {
                        crate::conn::command("browser", "browser_open", json!({ "project": b.project }));
                        glib::ControlFlow::Continue
                    }
                    _ => glib::ControlFlow::Break,
                });
            }
        } else {
            self.cast.borrow_mut().take();
        }
        let told = self.on_dock.borrow().clone();
        if let Some(told) = told {
            told(open && me.port.get() != 0);
        }
    }

    /// The page's frames into the picture, until the view closes or the browser stops.
    fn follow(&self) {
        let (send, frames) = async_channel::unbounded::<Shown>();
        let cast = Screencast::start(self.port.get(), move |shown| {
            let _ = send.send_blocking(shown);
        });
        *self.cast.borrow_mut() = Some(cast);
        let weak = self.me.clone();
        glib::spawn_future_local(async move {
            while let Ok(shown) = frames.recv().await {
                let Some(b) = weak.upgrade() else { return };
                match shown {
                    Shown::Frame { jpeg, width, height } => {
                        if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(jpeg)) {
                            b.picture.set_paintable(Some(&texture));
                        }
                        b.page.set((width, height));
                    }
                    Shown::Page { title, url } => b.address.set_label(if title.is_empty() { &url } else { &title }),
                    Shown::Ended(why) => {
                        b.address.set_label(&format!("Not shown: {why}"));
                        b.cast.borrow_mut().take();
                        // The browser still runs (another page came in front, it restarted): again.
                        let again = b.me.clone();
                        glib::timeout_add_local_once(Duration::from_secs(2), move || {
                            if let Some(b) = again.upgrade()
                                && b.port.get() != 0
                                && b.frame.parent().is_some()
                                && b.cast.borrow().is_none()
                            {
                                b.follow();
                            }
                        });
                        return;
                    }
                }
            }
        });
    }

    fn send(&self, method: &str, params: Value) {
        if let Some(cast) = self.cast.borrow().as_ref() {
            cast.send(method, params);
        }
    }

    fn eval(&self, script: &str) {
        self.send("Runtime.evaluate", json!({ "expression": script }));
    }

    /// Where (x, y) on the picture is on the page, in CSS pixels (None: off the page).
    fn on_page(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let paintable = self.picture.paintable()?;
        let widget = (self.picture.width() as f64, self.picture.height() as f64);
        let frame = (paintable.intrinsic_width() as f64, paintable.intrinsic_height() as f64);
        page_point(widget, frame, self.page.get(), x, y)
    }

    fn mouse(&self, kind: &str, x: f64, y: f64, button: &str, clicks: i32, modifiers: i32) {
        if let Some((x, y)) = self.on_page(x, y) {
            self.send("Input.dispatchMouseEvent", json!({ "type": kind, "x": x, "y": y, "button": button, "clickCount": clicks, "modifiers": modifiers }));
        }
    }

    /// The user's clicks, moves, scrolls and keys on the picture, to the page.
    fn input(&self) {
        let click = gtk::GestureClick::builder().button(0).build();
        let weak = self.me.clone();
        click.connect_pressed(move |gesture, n, x, y| {
            if let Some(b) = weak.upgrade() {
                b.picture.grab_focus();
                b.mouse("mousePressed", x, y, button_name(gesture.current_button()), n, modifiers(gesture.current_event_state()));
            }
        });
        let weak = self.me.clone();
        click.connect_released(move |gesture, n, x, y| {
            if let Some(b) = weak.upgrade() {
                b.mouse("mouseReleased", x, y, button_name(gesture.current_button()), n, modifiers(gesture.current_event_state()));
            }
        });
        self.picture.add_controller(click);
        let motion = gtk::EventControllerMotion::new();
        let weak = self.me.clone();
        motion.connect_motion(move |_, x, y| {
            if let Some(b) = weak.upgrade() {
                b.pointer.set((x, y));
                b.mouse("mouseMoved", x, y, "none", 0, 0);
            }
        });
        self.picture.add_controller(motion);
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let weak = self.me.clone();
        scroll.connect_scroll(move |_, dx, dy| {
            let Some(b) = weak.upgrade() else { return glib::Propagation::Proceed };
            let (x, y) = b.pointer.get();
            if let Some((x, y)) = b.on_page(x, y) {
                b.send("Input.dispatchMouseEvent", json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": dx * 60.0, "deltaY": dy * 60.0 }));
            }
            glib::Propagation::Stop
        });
        self.picture.add_controller(scroll);
        let keys = gtk::EventControllerKey::new();
        let weak = self.me.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(b) = weak.upgrade() else { return glib::Propagation::Proceed };
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            // Pasted from the user's clipboard, as typed.
            if ctrl && matches!(key, gdk::Key::v | gdk::Key::V) {
                let weak = b.me.clone();
                b.picture.clipboard().read_text_async(gtk::gio::Cancellable::NONE, move |text| {
                    if let (Some(b), Ok(Some(text))) = (weak.upgrade(), text) {
                        b.send("Input.insertText", json!({ "text": text.as_str() }));
                    }
                });
                return glib::Propagation::Stop;
            }
            for event in key_events(key, modifiers(state)) {
                b.send("Input.dispatchKeyEvent", event);
            }
            glib::Propagation::Stop
        });
        self.picture.add_controller(keys);
    }
}

/// (x, y) on a widget of size `widget` showing a frame of size `frame` contained (scaled to
/// fit, centred), as a point of a page of size `page` (CSS pixels). None: beside the frame.
fn page_point(widget: (f64, f64), frame: (f64, f64), page: (f64, f64), x: f64, y: f64) -> Option<(f64, f64)> {
    let ((w, h), (tw, th), (pw, ph)) = (widget, frame, page);
    if tw <= 0.0 || th <= 0.0 || pw <= 0.0 || ph <= 0.0 {
        return None;
    }
    let scale = (w / tw).min(h / th);
    let (left, top) = ((w - tw * scale) / 2.0, (h - th * scale) / 2.0);
    let (px, py) = ((x - left) / scale, (y - top) / scale);
    (px >= 0.0 && py >= 0.0 && px <= tw && py <= th).then(|| (px * pw / tw, py * ph / th))
}

fn button_name(button: u32) -> &'static str {
    match button {
        2 => "middle",
        3 => "right",
        _ => "left",
    }
}

/// CDP's modifier bits: Alt 1, Ctrl 2, Meta 4, Shift 8.
fn modifiers(state: gdk::ModifierType) -> i32 {
    [(gdk::ModifierType::ALT_MASK, 1), (gdk::ModifierType::CONTROL_MASK, 2), (gdk::ModifierType::META_MASK, 4), (gdk::ModifierType::SHIFT_MASK, 8)].iter().filter(|(m, _)| state.contains(*m)).map(|(_, bit)| bit).sum()
}

/// A key press as the page gets it: a key down (with its text, when it types one) and up.
fn key_events(key: gdk::Key, modifiers: i32) -> Vec<Value> {
    let special = |name: &str, code: i32, text: &str| (name.to_owned(), code, text.to_owned());
    let (name, code, text) = match key {
        gdk::Key::Return | gdk::Key::KP_Enter => special("Enter", 13, "\r"),
        gdk::Key::BackSpace => special("Backspace", 8, ""),
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab => special("Tab", 9, ""),
        gdk::Key::Escape => special("Escape", 27, ""),
        gdk::Key::Delete => special("Delete", 46, ""),
        gdk::Key::Left => special("ArrowLeft", 37, ""),
        gdk::Key::Up => special("ArrowUp", 38, ""),
        gdk::Key::Right => special("ArrowRight", 39, ""),
        gdk::Key::Down => special("ArrowDown", 40, ""),
        gdk::Key::Home => special("Home", 36, ""),
        gdk::Key::End => special("End", 35, ""),
        gdk::Key::Page_Up => special("PageUp", 33, ""),
        gdk::Key::Page_Down => special("PageDown", 34, ""),
        _ => match key.to_unicode() {
            Some(c) if !c.is_control() => {
                let typed = if modifiers & 2 != 0 || modifiers & 1 != 0 { String::new() } else { c.to_string() };
                (c.to_string(), c.to_ascii_uppercase() as i32, typed)
            }
            _ => return Vec::new(),
        },
    };
    let mut down = json!({ "type": if text.is_empty() { "rawKeyDown" } else { "keyDown" }, "key": name, "windowsVirtualKeyCode": code, "modifiers": modifiers });
    if !text.is_empty() {
        down["text"] = json!(text);
    }
    vec![down, json!({ "type": "keyUp", "key": name, "windowsVirtualKeyCode": code, "modifiers": modifiers })]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_on_the_view_lands_where_it_is_on_the_page() {
        // a 1366×900 frame shown at half size in a wider widget: bars left and right
        let (widget, frame, page) = ((1000.0, 450.0), (1366.0, 900.0), (1366.0, 900.0));
        let left = (1000.0 - 683.0) / 2.0;
        assert_eq!(page_point(widget, frame, page, left, 0.0), Some((0.0, 0.0)));
        assert_eq!(page_point(widget, frame, page, left + 341.5, 225.0), Some((683.0, 450.0)));
        assert_eq!(page_point(widget, frame, page, 10.0, 10.0), None, "on the bar beside the page");
        // a frame in device pixels (2×) of a page in CSS pixels
        assert_eq!(page_point((683.0, 450.0), (1366.0, 900.0), (683.0, 450.0), 683.0, 450.0), Some((683.0, 450.0)));
        assert_eq!(page_point(widget, frame, (0.0, 0.0), 500.0, 200.0), None, "no frame yet");
    }

    #[test]
    fn keys_reach_the_page_as_a_browser_would_send_them() {
        let typed = key_events(gdk::Key::a, 0);
        assert_eq!((typed[0]["type"].as_str(), typed[0]["text"].as_str(), typed[1]["type"].as_str()), (Some("keyDown"), Some("a"), Some("keyUp")));
        let enter = key_events(gdk::Key::Return, 0);
        assert_eq!((enter[0]["key"].as_str(), enter[0]["windowsVirtualKeyCode"].as_i64()), (Some("Enter"), Some(13)));
        let select_all = key_events(gdk::Key::a, 2);
        assert!(select_all[0].get("text").is_none() && select_all[0]["type"] == "rawKeyDown", "Ctrl+A selects, doesn't type: {select_all:?}");
        assert!(key_events(gdk::Key::Shift_L, 8).is_empty());
    }
}
