//! A message's text and code blocks as one read-only text view, as an editor shows text:
//! a selection runs across paragraphs and code, double-click picks a word as Pango sees words,
//! and a copy is the text as read (a code block's header line left out).
//!
//! The text comes as the markup `markdown.rs` writes (b, i, s, tt, span, a): read here into
//! the buffer's tags, links and the code blocks' "copy" included, so the view needs no state
//! of its own beside its buffer.

use gtk::{gdk, glib, pango, prelude::*};

use crate::markdown::Segment;

/// A read-only view for a message's text: clicks on links and on "copy", the pointer over
/// them, and copying without the code blocks' header lines.
pub fn view() -> gtk::TextView {
    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .hexpand(true)
        // Selecting doesn't take the focus from the message box: typing goes on there, and
        // its Ctrl+C copies what is selected here (copy_selected).
        .focus_on_click(false)
        .build();
    view.add_css_class("msg-text");
    // One selection in the chat at a time: a new one clears the last.
    view.buffer().connect_has_selection_notify(glib::clone!(#[weak] view, move |buffer| {
        if !buffer.has_selection() {
            return;
        }
        let last = SELECTED.with(|s| s.replace(view.downgrade()));
        if let Some(last) = last.upgrade().filter(|last| *last != view) {
            let buffer = last.buffer();
            buffer.place_cursor(&buffer.iter_at_mark(&buffer.get_insert()));
        }
    }));
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    click.connect_released(|gesture, presses, x, y| {
        let Some(view) = gesture.widget().and_downcast::<gtk::TextView>() else { return };
        // A click, not the end of a selection being made.
        if presses != 1 || view.buffer().has_selection() {
            return;
        }
        let (bx, by) = view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        if let Some(at) = view.iter_at_location(bx, by) {
            clicked(&view, &at);
        }
    });
    view.add_controller(click);
    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion(|motion, x, y| {
        let Some(view) = motion.widget().and_downcast::<gtk::TextView>() else { return };
        let (bx, by) = view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let active = view.iter_at_location(bx, by).is_some_and(|at| at.tags().iter().any(|t| action_of(t).is_some()));
        view.set_cursor_from_name(Some(if active { "pointer" } else { "text" }));
    });
    view.add_controller(motion);
    view.connect_copy_clipboard(|view| {
        view.stop_signal_emission_by_name("copy-clipboard");
        if let Some(text) = selection(view) {
            view.clipboard().set_text(&text);
        }
    });
    view
}

thread_local! {
    /// The message text view a selection was last made in.
    static SELECTED: std::cell::RefCell<glib::WeakRef<gtk::TextView>> = std::cell::RefCell::default();
}

/// Copies what is selected in the chat's text, if anything: whether it did.
pub fn copy_selected(from: &gtk::Widget) -> bool {
    let Some(text) = SELECTED.with(|s| s.borrow().upgrade()).and_then(|view| selection(&view)) else { return false };
    from.clipboard().set_text(&text);
    true
}

/// What is selected in `view`, as a copy gives it (None: nothing).
pub fn selection(view: &gtk::TextView) -> Option<String> {
    let buffer = view.buffer();
    let (start, end) = buffer.selection_bounds()?;
    Some(readable(&buffer, &start, &end)).filter(|t| !t.is_empty())
}

/// What a link or a "copy" does: (kind, what), from its tag's name.
fn action_of(tag: &gtk::TextTag) -> Option<(String, String)> {
    let name = tag.name()?;
    let (kind, rest) = name.split_once(':')?;
    let (_, what) = rest.split_once(':')?;
    matches!(kind, "link" | "copy").then(|| (kind.to_owned(), what.to_owned()))
}

fn clicked(view: &gtk::TextView, at: &gtk::TextIter) {
    let Some((kind, what)) = at.tags().iter().find_map(action_of) else { return };
    if kind == "link" {
        crate::chat::open_link(&what, view.upcast_ref());
        return;
    }
    // "copy": the code under this header line, to its end.
    let buffer = view.buffer();
    let Some(code_tag) = buffer.tag_table().lookup("code") else { return };
    let mut start = *at;
    start.forward_line();
    let mut end = start;
    end.forward_to_tag_toggle(Some(&code_tag));
    let code = buffer.text(&start, &end, false);
    view.clipboard().set_text(code.trim_end_matches('\n'));
    let mut word_start = *at;
    let mut word_end = *at;
    if let Some(tag) = at.tags().into_iter().find(|t| action_of(t).is_some()) {
        if !word_start.starts_tag(Some(&tag)) {
            word_start.backward_to_tag_toggle(Some(&tag));
        }
        word_end.forward_to_tag_toggle(Some(&tag));
        // "copied" a moment, in place of "copy".
        buffer.delete(&mut word_start, &mut word_end);
        buffer.insert_with_tags(&mut word_start, "copied", &[&tag]);
        let (buffer, tag) = (buffer.clone(), tag.clone());
        glib::timeout_add_local_once(std::time::Duration::from_millis(1200), move || {
            let mut start = buffer.start_iter();
            if start.forward_to_tag_toggle(Some(&tag)) || start.starts_tag(Some(&tag)) {
                let mut end = start;
                end.forward_to_tag_toggle(Some(&tag));
                buffer.delete(&mut start, &mut end);
                buffer.insert_with_tags(&mut start, "copy", &[&tag]);
            }
        });
    }
}

/// The text between `start` and `end`, but for what only the view shows (a code block's
/// header line).
fn readable(buffer: &gtk::TextBuffer, start: &gtk::TextIter, end: &gtk::TextIter) -> String {
    let chrome = buffer.tag_table().lookup("chrome");
    let mut out = String::new();
    let mut at = *start;
    while at < *end {
        if !chrome.as_ref().is_some_and(|tag| at.has_tag(tag)) {
            out.push(at.char());
        }
        if !at.forward_char() {
            break;
        }
    }
    out
}

/// Shows `segments` in `body` (a message's) in place of `shown`, what it showed: one text
/// view for all of it, filled anew when they changed (a reply streaming in).
pub fn show(body: &gtk::Box, shown: &[Segment], segments: &[Segment]) {
    let only = body.first_child().filter(|c| c.next_sibling().is_none()).and_downcast::<gtk::TextView>();
    let view = match only {
        Some(_) if shown == segments => return,
        Some(view) => view,
        None => {
            while let Some(child) = body.first_child() {
                body.remove(&child);
            }
            let view = view();
            body.append(&view);
            view
        }
    };
    fill(&view, segments);
}

/// Shows `segments` in `view`, in place of what it showed.
pub fn fill(view: &gtk::TextView, segments: &[Segment]) {
    let buffer = view.buffer();
    buffer.set_text("");
    // The last fill's links and copies go with its text.
    let table = buffer.tag_table();
    let mut old = Vec::new();
    table.foreach(|tag| {
        if action_of(tag).is_some() || tag.name().is_some_and(|n| n.starts_with("tab:")) {
            old.push(tag.clone());
        }
    });
    for tag in old {
        table.remove(&tag);
    }
    let mut count = 0;
    let last = segments.len().saturating_sub(1);
    for (i, segment) in segments.iter().enumerate() {
        match segment {
            Segment::Text(markup) => {
                let markup = if i == last { markup.trim_end() } else { markup.as_str() };
                insert_markup(view, markup, &mut count);
            }
            Segment::Code { lang, code } => insert_code(view, lang, code, i == last, &mut count),
            Segment::Table(rows) => insert_table(view, rows, &mut count),
            Segment::Rule => insert_rule(view),
        }
    }
}

/// A tag of the buffer's, made the first time by `make`.
fn tag(buffer: &gtk::TextBuffer, name: &str, make: impl FnOnce() -> gtk::TextTag) -> gtk::TextTag {
    let table = buffer.tag_table();
    table.lookup(name).unwrap_or_else(|| {
        let tag = make();
        table.add(&tag);
        tag
    })
}

fn grey(alpha: f32) -> gdk::RGBA {
    gdk::RGBA::new(0.53, 0.53, 0.53, alpha)
}

fn insert_code(view: &gtk::TextView, lang: &str, code: &str, last: bool, count: &mut usize) {
    let buffer = view.buffer();
    set_apart(&buffer);
    let mut end = buffer.end_iter();
    let chrome = tag(&buffer, "chrome", || gtk::TextTag::builder().name("chrome").build());
    let head = tag(&buffer, "code-head", || {
        gtk::TextTag::builder()
            .name("code-head")
            .paragraph_background_rgba(&grey(0.16))
            .justification(gtk::Justification::Right)
            .scale(0.8)
            .foreground_rgba(&grey(1.0))
            .left_margin(12)
            .right_margin(12)
            .pixels_above_lines(6)
            .build()
    });
    let code_tag = tag(&buffer, "code", || {
        gtk::TextTag::builder()
            .name("code")
            .family("JetBrains Mono, monospace")
            .scale(0.92)
            .paragraph_background_rgba(&grey(0.16))
            .wrap_mode(gtk::WrapMode::Char)
            .left_margin(12)
            .right_margin(12)
            .build()
    });
    *count += 1;
    let copy = gtk::TextTag::builder().name(format!("copy:{count}:")).underline(pango::Underline::None).weight(600).build();
    buffer.tag_table().add(&copy);
    let label = if lang.is_empty() { String::new() } else { format!("{lang}   ") };
    buffer.insert_with_tags(&mut end, &label, &[&chrome, &head]);
    buffer.insert_with_tags(&mut end, "copy", &[&chrome, &head, &copy]);
    buffer.insert_with_tags(&mut end, "\n", &[&chrome, &head]);
    let body = code.trim_end_matches('\n');
    buffer.insert_with_tags(&mut end, body, &[&code_tag]);
    // The block's bottom padding: an empty line of its own, read as nothing.
    let pad = tag(&buffer, "code-foot", || gtk::TextTag::builder().name("code-foot").paragraph_background_rgba(&grey(0.16)).scale(0.4).left_margin(12).right_margin(12).pixels_below_lines(14).build());
    buffer.insert_with_tags(&mut end, "\n", &[&code_tag]);
    buffer.insert_with_tags(&mut end, if last { " " } else { " \n" }, &[&chrome, &pad]);
}

/// Inserts the markup `markdown.rs` writes as the buffer's tags; then list items and quotes
/// get their indents.
fn insert_markup(view: &gtk::TextView, markup: &str, count: &mut usize) {
    let buffer = view.buffer();
    let end = buffer.end_iter();
    // At most one blank line between blocks (a quote's end and a paragraph's both add one),
    // none at the start of what follows a block already set apart.
    let mut markup = markup.to_owned();
    while markup.contains("\n\n\n") {
        markup = markup.replace("\n\n\n", "\n\n");
    }
    let markup = if end.offset() > 0 { markup.trim_start_matches('\n') } else { markup.as_str() };
    let first_line = end.line();
    insert_inline(view, markup, count);
    indent_lines(view, first_line);
}

/// Inserts markup at the buffer's end as its tags.
fn insert_inline(view: &gtk::TextView, markup: &str, count: &mut usize) {
    let buffer = view.buffer();
    let mut end = buffer.end_iter();
    let mut open: Vec<Option<gtk::TextTag>> = Vec::new();
    let mut rest = markup;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('<') {
            let Some(close) = after.find('>') else { break };
            let inside = &after[..close];
            rest = &after[close + 1..];
            if inside.starts_with('/') {
                open.pop();
            } else {
                open.push(tag_for(view, inside, count));
            }
            continue;
        }
        let stop = rest.find('<').unwrap_or(rest.len());
        let text = unescape(&rest[..stop]);
        rest = &rest[stop..];
        let tags: Vec<&gtk::TextTag> = open.iter().flatten().collect();
        buffer.insert_with_tags(&mut end, &text, &tags);
    }
}

/// What comes next starts on a line of its own after a blank one (the buffer's end counts as
/// a line's end, so its last characters tell).
fn set_apart(buffer: &gtk::TextBuffer) {
    let mut end = buffer.end_iter();
    let text = buffer.text(&buffer.start_iter(), &end, false);
    if !text.is_empty() {
        let breaks = text.chars().rev().take_while(|c| *c == '\n').count();
        buffer.insert(&mut end, &"\n".repeat(2usize.saturating_sub(breaks)));
    }
}

/// A table as text: a row a line, its cells at tab stops measured to fit the widest cell of
/// each column (up to a limit; a longer one wraps). Copied, its cells come tab-separated, as
/// a spreadsheet takes them.
fn insert_table(view: &gtk::TextView, rows: &[Vec<String>], count: &mut usize) {
    const GAP: i32 = 28;
    const WIDEST: i32 = 360;
    let buffer = view.buffer();
    set_apart(&buffer);
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; columns];
    for (r, row) in rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let width = view.create_pango_layout(Some(&unescape(&strip_tags(cell)))).pixel_size().0;
            // The header is bold: a little wider.
            let width = if r == 0 { width * 11 / 10 } else { width };
            widths[c] = widths[c].max(width.min(WIDEST));
        }
    }
    let mut tabs = pango::TabArray::new(columns.saturating_sub(1) as i32, true);
    let mut x = 0;
    for (c, width) in widths.iter().take(columns.saturating_sub(1)).enumerate() {
        x += width + GAP;
        tabs.set_tab(c as i32, pango::TabAlign::Left, x);
    }
    *count += 1;
    let stops = gtk::TextTag::builder().name(format!("tab:{count}")).tabs(&tabs).left_margin(12).right_margin(12).pixels_above_lines(5).pixels_below_lines(5).build();
    buffer.tag_table().add(&stops);
    let head = tag(&buffer, "table-head", || gtk::TextTag::builder().name("table-head").weight(600).scale(0.9).foreground_rgba(&grey(1.0)).paragraph_background_rgba(&grey(0.12)).build());
    let body = tag(&buffer, "table-row", || gtk::TextTag::builder().name("table-row").paragraph_background_rgba(&grey(0.05)).build());
    let foot = tag(&buffer, "table-end", || gtk::TextTag::builder().name("table-end").pixels_below_lines(14).build());
    for (r, row) in rows.iter().enumerate() {
        let start = buffer.end_iter().offset();
        for (c, cell) in row.iter().enumerate() {
            if c > 0 {
                buffer.insert(&mut buffer.end_iter(), "\t");
            }
            insert_inline(view, cell, count);
        }
        let (from, to) = (buffer.iter_at_offset(start), buffer.end_iter());
        buffer.apply_tag(&stops, &from, &to);
        buffer.apply_tag(if r == 0 { &head } else { &body }, &from, &to);
        if r + 1 == rows.len() {
            buffer.apply_tag(&foot, &from, &to);
        }
        buffer.insert(&mut buffer.end_iter(), "\n");
    }
}

/// A line across: a thin full-width paragraph, read as nothing.
fn insert_rule(view: &gtk::TextView) {
    let buffer = view.buffer();
    set_apart(&buffer);
    let chrome = tag(&buffer, "chrome", || gtk::TextTag::builder().name("chrome").build());
    let rule = tag(&buffer, "rule", || gtk::TextTag::builder().name("rule").paragraph_background_rgba(&grey(0.3)).scale(0.12).build());
    buffer.insert_with_tags(&mut buffer.end_iter(), " \n", &[&chrome, &rule]);
}

/// Markup's text alone.
fn strip_tags(markup: &str) -> String {
    let mut out = String::with_capacity(markup.len());
    let mut inside = false;
    for c in markup.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

/// The tag for one opening markup element (None: one this view ignores).
fn tag_for(view: &gtk::TextView, element: &str, count: &mut usize) -> Option<gtk::TextTag> {
    let buffer = view.buffer();
    let (name, attrs) = element.split_once(' ').unwrap_or((element, ""));
    let attr = |key: &str| attrs.split(&format!("{key}=\"")).nth(1).and_then(|v| v.split('"').next()).map(unescape);
    Some(match name {
        "b" => tag(&buffer, "b", || gtk::TextTag::builder().name("b").weight(700).build()),
        "i" => tag(&buffer, "i", || gtk::TextTag::builder().name("i").style(pango::Style::Italic).build()),
        "s" => tag(&buffer, "s", || gtk::TextTag::builder().name("s").strikethrough(true).build()),
        "tt" => tag(&buffer, "tt", || gtk::TextTag::builder().name("tt").family("JetBrains Mono, monospace").build()),
        "a" => {
            *count += 1;
            let link = gtk::TextTag::builder().name(format!("link:{count}:{}", attr("href").unwrap_or_default())).underline(pango::Underline::Single).build();
            buffer.tag_table().add(&link);
            link
        }
        "span" if attr("size").is_some() => {
            let size = attr("size").unwrap_or_default();
            let (scale, above) = match size.as_str() {
                "x-large" => (1.35, 10),
                "large" => (1.18, 8),
                _ => (1.05, 6),
            };
            tag(&buffer, &format!("h-{size}"), || gtk::TextTag::builder().name(format!("h-{size}")).weight(700).scale(scale).pixels_above_lines(above).build())
        }
        "span" if attr("font_family").is_some() => tag(&buffer, "code-inline", || {
            gtk::TextTag::builder().name("code-inline").family("JetBrains Mono, monospace").scale(0.92).background_rgba(&grey(0.2)).build()
        }),
        _ => return None,
    })
}

/// List items ("•  ", "3. ", indented by 4 spaces a level) hang under their first word's
/// start; quotes ("▎ ") are set in.
fn indent_lines(view: &gtk::TextView, from_line: i32) {
    let buffer = view.buffer();
    for line in from_line..buffer.line_count() {
        let Some(start) = buffer.iter_at_line(line) else { continue };
        let mut end = start;
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        let text = buffer.text(&start, &end, false);
        let spaces = text.len() - text.trim_start_matches(' ').len();
        let body = &text[spaces..];
        let digits = body.chars().take_while(char::is_ascii_digit).count();
        let marker = if body.starts_with("•  ") {
            "•  ".len()
        } else if digits > 0 && body[digits..].starts_with(". ") {
            digits + 2
        } else {
            continue;
        };
        let lead = &text[..spaces + marker];
        let width = view.create_pango_layout(Some(lead)).pixel_size().0;
        let name = format!("li-{width}");
        let hang = tag(&buffer, &name, || gtk::TextTag::builder().name(&name).left_margin(width).indent(-width).build());
        buffer.apply_tag(&hang, &start, &end);
    }
}

fn unescape(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';') else { break };
        let entity = &rest[1..end];
        let ch = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity.strip_prefix('#').and_then(|n| match n.strip_prefix('x') {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => n.parse().ok(),
            }).and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn entities_come_back_as_text() {
        assert_eq!(super::unescape("a &lt;b&gt; &amp; &quot;c&quot; &#39; &#x41; &bogus"), "a <b> & \"c\" ' A &bogus");
    }

    #[test]
    fn a_cell_measures_by_its_text() {
        assert_eq!(super::strip_tags("<b>bold</b> and <tt>c&lt;</tt>"), "bold and c&lt;");
    }
}
