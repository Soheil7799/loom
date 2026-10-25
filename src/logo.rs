//! loom's mark, drawn rather than loaded: two threads each way, woven over and under, the
//! wefts in the theme's accent colour (so it follows the wallpaper like the rest). The same
//! shape as data/dev.shirvani.Loom.svg, without the tile.

use gtk::{cairo, gdk, prelude::*};

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
