//! loom's mark, drawn rather than loaded: three warp threads with two wefts woven through,
//! in the theme's accent colour (so it follows the wallpaper like the rest).

use gtk::{gdk, prelude::*};

pub fn mark(size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_width(size).content_height(size).build();
    area.add_css_class("logo");
    area.set_draw_func(|area, cr, width, height| {
        let accent = accent(area);
        let fg = area.color();
        let s = f64::from(width.min(height));
        let (ox, oy) = ((f64::from(width) - s) / 2.0, (f64::from(height) - s) / 2.0);
        let line = s * 0.085;
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_width(line);
        // Warp: three vertical threads.
        let xs = [0.28, 0.5, 0.72];
        for x in xs {
            cr.set_source_rgba(f64::from(fg.red()), f64::from(fg.green()), f64::from(fg.blue()), 0.85);
            cr.move_to(ox + s * x, oy + s * 0.16);
            cr.line_to(ox + s * x, oy + s * 0.84);
            let _ = cr.stroke();
        }
        // Weft: two accent threads passing over and under in turn.
        for (row, y) in [0.38, 0.62].into_iter().enumerate() {
            cr.set_source_rgba(f64::from(accent.red()), f64::from(accent.green()), f64::from(accent.blue()), 1.0);
            cr.move_to(ox + s * 0.14, oy + s * y);
            cr.line_to(ox + s * 0.86, oy + s * y);
            let _ = cr.stroke();
            // Where the warp passes over, redraw it on top.
            for (i, x) in xs.iter().enumerate() {
                if (i + row) % 2 == 0 {
                    cr.set_source_rgba(f64::from(fg.red()), f64::from(fg.green()), f64::from(fg.blue()), 0.85);
                    cr.move_to(ox + s * x, oy + s * (y - 0.07));
                    cr.line_to(ox + s * x, oy + s * (y + 0.07));
                    let _ = cr.stroke();
                }
            }
        }
    });
    area
}

/// The theme's accent colour (a warm yellow if it has none).
fn accent(widget: &impl IsA<gtk::Widget>) -> gdk::RGBA {
    #[allow(deprecated)]
    widget.as_ref().style_context().lookup_color("accent_color").unwrap_or_else(|| gdk::RGBA::new(0.95, 0.76, 0.36, 1.0))
}
