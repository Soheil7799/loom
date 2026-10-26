//! The one terminal widget everything uses (scratchpad, jobs): VTE, set up in the window's
//! colours and the code font. Swapping the engine later (libghostty) means changing this file.

use gtk::{gdk, glib, pango};
use vte::prelude::*;

pub const FONT: &str = "JetBrains Mono, Vazirmatn 10";

pub fn new(widget_for_colors: &impl IsA<gtk::Widget>) -> vte::Terminal {
    let term = vte::Terminal::new();
    term.set_font(Some(&pango::FontDescription::from_string(&crate::settings::get().terminal_font)));
    term.set_scrollback_lines(10_000);
    term.set_cursor_blink_mode(vte::CursorBlinkMode::Off);
    term.set_enable_bidi(true);
    term.set_enable_shaping(true);
    term.set_hexpand(true);
    term.set_vexpand(true);
    let (fg, bg) = colors(widget_for_colors.as_ref());
    term.set_colors(Some(&fg), Some(&bg), &[]);
    crate::theme::follow(&term);
    // VTE has no keys for the clipboard: the usual terminal ones.
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(|keys, key, _, modifiers| {
        let Some(term) = keys.widget().and_downcast::<vte::Terminal>() else { return glib::Propagation::Proceed };
        let ctrl_shift = modifiers.contains(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
        let shift_only = modifiers.contains(gdk::ModifierType::SHIFT_MASK) && !modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        match key.to_lower() {
            gdk::Key::c if ctrl_shift => {
                if term.has_selection() {
                    term.copy_clipboard_format(vte::Format::Text);
                }
            }
            gdk::Key::v if ctrl_shift => term.paste_clipboard(),
            gdk::Key::Insert if shift_only => term.paste_clipboard(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    term.add_controller(keys);
    term
}

/// The theme changed: the terminal takes its new colours.
pub fn recolor(term: &vte::Terminal) {
    let (fg, bg) = colors(term.upcast_ref());
    term.set_colors(Some(&fg), Some(&bg), &[]);
}

/// Foreground and background of the GTK theme in use.
fn colors(widget: &gtk::Widget) -> (gdk::RGBA, gdk::RGBA) {
    #[allow(deprecated)]
    let context = widget.style_context();
    #[allow(deprecated)]
    let lookup = |name: &str, fallback: &str| context.lookup_color(name).unwrap_or_else(|| gdk::RGBA::parse(fallback).unwrap());
    (lookup("view_fg_color", "#cccac2"), lookup("view_bg_color", "#1f2430"))
}

/// Text from an agent ends lines with "\n"; a terminal wants "\r\n".
pub fn crlf(data: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut previous = 0u8;
    for &byte in data.as_bytes() {
        if byte == b'\n' && previous != b'\r' {
            out.push(b'\r');
        }
        out.push(byte);
        previous = byte;
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn lone_newlines_get_a_carriage_return() {
        assert_eq!(super::crlf("a\nb\r\nc"), b"a\r\nb\r\nc".to_vec());
    }
}
