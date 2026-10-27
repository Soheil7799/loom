//! loom's mark, drawn rather than loaded: two threads each way, woven over and under, the
//! wefts in the theme's accent colour (so it follows the wallpaper like the rest). The same
//! shape as data/dev.shirvani.Loom.svg, without the tile.

use gtk::{cairo, gdk, prelude::*};

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

pub fn mark(size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_width(size).content_height(size).build();
    area.add_css_class("logo");
    area.set_draw_func(|area, cr, width, height| {
        let accent = accent(area);
        let fg = area.color();
        let s = f64::from(width.min(height));
        let (ox, oy) = ((f64::from(width) - s) / 2.0, (f64::from(height) - s) / 2.0);
        // In the icon's 128 grid: threads at 46 and 82, 20 wide, from 24 to 104, gaps of 5;
        // here the tile is dropped, so the 16..112 span fills the area (24 → 0.083).
        let at = |v: f64| (v - 16.0) / 96.0 * s;
        let (w, gap) = (20.0 / 96.0 * s, 5.0 / 96.0 * s);
        let (lo, hi) = (at(24.0) + w / 2.0, at(104.0) - w / 2.0);
        let threads = [at(46.0), at(82.0)];
        let set = |c: &gdk::RGBA| cr.set_source_rgba(f64::from(c.red()), f64::from(c.green()), f64::from(c.blue()), f64::from(c.alpha()));
        let line = |x1: f64, y1: f64, x2: f64, y2: f64, width: f64, cap: cairo::LineCap| {
            cr.set_line_width(width);
            cr.set_line_cap(cap);
            cr.move_to(ox + x1, oy + y1);
            cr.line_to(ox + x2, oy + y2);
            let _ = cr.stroke();
        };
        cr.push_group();
        set(&fg);
        for x in threads {
            line(x, lo, x, hi, w, cairo::LineCap::Round);
        }
        set(&accent);
        for y in threads {
            line(lo, y, hi, y, w, cairo::LineCap::Round);
        }
        // At each crossing one thread passes over: cut the gap around it, then draw it again.
        for (j, y) in threads.into_iter().enumerate() {
            for (i, x) in threads.into_iter().enumerate() {
                let warp_over = (i + j) % 2 == 0;
                let (half, reach) = (w / 2.0 + 0.5, w / 2.0 + gap + 1.0);
                cr.set_operator(cairo::Operator::Clear);
                if warp_over {
                    line(x, y - half, x, y + half, w + 2.0 * gap, cairo::LineCap::Butt);
                } else {
                    line(x - half, y, x + half, y, w + 2.0 * gap, cairo::LineCap::Butt);
                }
                cr.set_operator(cairo::Operator::Over);
                if warp_over {
                    set(&fg);
                    line(x, y - reach, x, y + reach, w, cairo::LineCap::Butt);
                } else {
                    set(&accent);
                    line(x - reach, y, x + reach, y, w, cairo::LineCap::Butt);
                }
            }
        }
        let _ = cr.pop_group_to_source();
        let _ = cr.paint();
    });
    area
}

/// The theme's accent colour (a warm yellow if it has none).
fn accent(widget: &impl IsA<gtk::Widget>) -> gdk::RGBA {
    #[allow(deprecated)]
    widget.as_ref().style_context().lookup_color("accent_color").unwrap_or_else(|| gdk::RGBA::new(0.95, 0.76, 0.36, 1.0))
}
