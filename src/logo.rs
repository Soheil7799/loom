//! loom's mark (data/logo/README.md): a fox whose body is an l, its tail curling through the
//! l's foot, in Nocturne's gradients. Shipped as PNGs rendered from data/logo/*.svg, since GTK
//! can't count on an SVG loader; the same mark as data/dev.shirvani.Loom.svg, without the tile.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::{cairo, gdk_pixbuf::Pixbuf, prelude::*};

const DARK: &[u8] = include_bytes!("../data/logo/agile-l-dark.png");
const LIGHT: &[u8] = include_bytes!("../data/logo/agile-l-light.png");
const SMALL_DARK: &[u8] = include_bytes!("../data/logo/agile-l-small-dark.png");
const SMALL_LIGHT: &[u8] = include_bytes!("../data/logo/agile-l-small-light.png");

/// A coffee cup in outline (the Running list's keep-awake), in the widget's own colour (CSS
/// sets it: dim off, gold on); `steam` above it when on.
pub fn cup(size: i32, steam: bool) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_width(size).content_height(size).valign(gtk::Align::Center).halign(gtk::Align::Center).build();
    area.set_draw_func(move |area, cr, width, height| {
        let c = area.color();
        cr.set_source_rgba(f64::from(c.red()), f64::from(c.green()), f64::from(c.blue()), f64::from(c.alpha()));
        let s = f64::from(width.min(height));
        let (ox, oy) = ((f64::from(width) - s) / 2.0, (f64::from(height) - s) / 2.0);
        let at = |x: f64, y: f64| (ox + x * s, oy + y * s);
        cr.set_line_width((s / 11.0).max(1.2));
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_join(cairo::LineJoin::Round);
        // The cup: straight sides, a rounded bottom.
        let (x0, y0) = at(0.16, 0.42);
        cr.move_to(x0, y0);
        let (x1, y1) = at(0.16, 0.70);
        cr.line_to(x1, y1);
        let (cx, cy) = at(0.25, 0.86);
        let (ex, ey) = at(0.38, 0.86);
        cr.curve_to(x1, cy, cx, ey, ex, ey);
        let (fx, fy) = at(0.54, 0.86);
        cr.line_to(fx, fy);
        let (gx, gy) = at(0.67, 0.86);
        let (hx, hy) = at(0.76, 0.70);
        cr.curve_to(gx, gy, hx, gy, hx, hy);
        let (ix, iy) = at(0.76, 0.42);
        cr.line_to(ix, iy);
        cr.close_path();
        let _ = cr.stroke();
        // The handle.
        let (hcx, hcy) = at(0.78, 0.56);
        cr.new_sub_path();
        cr.arc(hcx, hcy, s * 0.11, -std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
        let _ = cr.stroke();
        // Steam: two short wisps.
        if steam {
            for x in [0.36, 0.56] {
                let (sx, sy) = at(x, 0.30);
                cr.move_to(sx, sy);
                let (ax, ay) = at(x - 0.06, 0.22);
                let (bx, by) = at(x + 0.06, 0.16);
                let (tx, ty) = at(x, 0.08);
                cr.curve_to(ax, ay, bx, by, tx, ty);
                let _ = cr.stroke();
            }
        }
    });
    area
}

/// The mark at `size` px: the dark-theme drawing when the theme's text is light, the
/// thicker small drawing below 48 px.
pub fn mark(size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_width(size).content_height(size).build();
    area.add_css_class("logo");
    area.set_draw_func(|area, cr, width, height| {
        let c = area.color();
        let dark = 0.2126 * c.red() + 0.7152 * c.green() + 0.0722 * c.blue() > 0.5;
        let s = width.min(height);
        let Some(pixbuf) = picture(dark, s < 48, s * area.scale_factor()) else { return };
        let scale = f64::from(s) / f64::from(pixbuf.width().max(pixbuf.height()));
        let (w, h) = (f64::from(pixbuf.width()) * scale, f64::from(pixbuf.height()) * scale);
        cr.translate((f64::from(width) - w) / 2.0, (f64::from(height) - h) / 2.0);
        cr.scale(scale, scale);
        cr.set_source_pixbuf(&pixbuf, 0.0, 0.0);
        let _ = cr.paint();
    });
    area
}

/// The drawing scaled to `px` device pixels on its longer side, kept once made.
fn picture(dark: bool, small: bool, px: i32) -> Option<Pixbuf> {
    thread_local! {
        static MADE: RefCell<HashMap<(bool, bool, i32), Pixbuf>> = RefCell::new(HashMap::new());
    }
    MADE.with(|made| {
        if let Some(p) = made.borrow().get(&(dark, small, px)) {
            return Some(p.clone());
        }
        let bytes = match (dark, small) {
            (true, false) => DARK,
            (false, false) => LIGHT,
            (true, true) => SMALL_DARK,
            (false, true) => SMALL_LIGHT,
        };
        let full = Pixbuf::from_read(std::io::Cursor::new(bytes)).ok()?;
        let k = f64::from(px) / f64::from(full.width().max(full.height()));
        let (w, h) = ((f64::from(full.width()) * k).round().max(1.0) as i32, (f64::from(full.height()) * k).round().max(1.0) as i32);
        let p = full.scale_simple(w, h, gtk::gdk_pixbuf::InterpType::Hyper)?;
        made.borrow_mut().insert((dark, small, px), p.clone());
        Some(p)
    })
}
