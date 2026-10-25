//! Agent messages are Markdown: this turns them into Pango markup for labels, with code
//! blocks (and tables) set apart to be drawn in a monospace box.

use gtk::glib::markup_escape_text as escape;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug, PartialEq)]
pub enum Segment {
    /// Pango markup.
    Text(String),
    /// Text to show as is, in monospace; `lang` is the fence's info string.
    Code { lang: String, code: String },
    /// Rows of cells (Pango markup), the first one the header.
    Table(Vec<Vec<String>>),
}

pub fn segments(markdown: &str) -> Vec<Segment> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut out = Vec::new();
    let mut text = String::new();
    // Ordered lists hold the next number.
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut code: Option<(String, String)> = None;
    let mut table: Option<Vec<Vec<String>>> = None;

    fn flush(text: &mut String, out: &mut Vec<Segment>) {
        let trimmed = text.trim_end();
        if !trimmed.trim().is_empty() {
            out.push(Segment::Text(trimmed.to_owned()));
        }
        text.clear();
    }

    for event in Parser::new_ext(markdown, options) {
        if let Some((_, body)) = code.as_mut() {
            match event {
                Event::Text(t) => body.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, body) = code.take().unwrap_or_default();
                    out.push(Segment::Code { lang, code: body.trim_end_matches('\n').to_owned() });
                }
                _ => {}
            }
            continue;
        }
        if let Some(rows) = table.as_mut() {
            match event {
                Event::Start(Tag::TableHead | Tag::TableRow) => rows.push(Vec::new()),
                Event::Start(Tag::TableCell) => {
                    if let Some(row) = rows.last_mut() {
                        row.push(String::new());
                    }
                }
                Event::Text(t) => {
                    if let Some(cell) = rows.last_mut().and_then(|row| row.last_mut()) {
                        cell.push_str(&escape(&t));
                    }
                }
                Event::Code(t) => {
                    if let Some(cell) = rows.last_mut().and_then(|row| row.last_mut()) {
                        cell.push_str(&format!("<tt>{}</tt>", escape(&t)));
                    }
                }
                Event::Start(Tag::Strong) => push_cell(rows, "<b>"),
                Event::End(TagEnd::Strong) => push_cell(rows, "</b>"),
                Event::Start(Tag::Emphasis) => push_cell(rows, "<i>"),
                Event::End(TagEnd::Emphasis) => push_cell(rows, "</i>"),
                Event::End(TagEnd::Table) => {
                    out.push(Segment::Table(table.take().unwrap_or_default()));
                }
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    let size = match level {
                        HeadingLevel::H1 => "x-large",
                        HeadingLevel::H2 => "large",
                        _ => "medium",
                    };
                    text.push_str(&format!("<span weight=\"bold\" size=\"{size}\">"));
                }
                Tag::Emphasis => text.push_str("<i>"),
                Tag::Strong => text.push_str("<b>"),
                Tag::Strikethrough => text.push_str("<s>"),
                Tag::Link { dest_url, .. } => text.push_str(&format!("<a href=\"{}\">", escape(&dest_url))),
                Tag::BlockQuote(_) => text.push_str("▎ "),
                Tag::List(start) => {
                    if lists.is_empty() && !text.is_empty() && !text.ends_with("\n\n") {
                        text.push('\n');
                    }
                    lists.push(start);
                }
                Tag::Item => {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    let depth = lists.len().saturating_sub(1);
                    text.push_str(&"    ".repeat(depth));
                    match lists.last_mut() {
                        Some(Some(n)) => {
                            text.push_str(&format!("{n}. "));
                            *n += 1;
                        }
                        _ => text.push_str("•  "),
                    }
                }
                Tag::CodeBlock(kind) => {
                    flush(&mut text, &mut out);
                    let lang = match kind {
                        CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_owned(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    code = Some((lang, String::new()));
                }
                Tag::Table(_) => {
                    flush(&mut text, &mut out);
                    table = Some(Vec::new());
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => text.push_str("</span>\n\n"),
                TagEnd::Emphasis => text.push_str("</i>"),
                TagEnd::BlockQuote(_) => text.push_str("\n\n"),
                TagEnd::Strong => text.push_str("</b>"),
                TagEnd::Strikethrough => text.push_str("</s>"),
                TagEnd::Link => text.push_str("</a>"),
                TagEnd::Paragraph => text.push_str(if lists.is_empty() { "\n\n" } else { "\n" }),
                TagEnd::List(_) => {
                    lists.pop();
                    if lists.is_empty() {
                        text.push_str("\n\n");
                    }
                }
                _ => {}
            },
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => text.push_str(&escape(&t)),
            Event::Code(t) => text.push_str(&format!(
                "<span font_family=\"JetBrains Mono, Vazirmatn, monospace\" bgcolor=\"#888888\" bgalpha=\"22%\">{}</span>",
                escape(&t)
            )),
            Event::SoftBreak => text.push(' '),
            Event::HardBreak => text.push('\n'),
            Event::Rule => text.push_str("\n──────────\n\n"),
            Event::TaskListMarker(done) => text.push_str(if done { "☑ " } else { "☐ " }),
            _ => {}
        }
    }
    // A message still streaming can stop inside a fence.
    if let Some((lang, body)) = code {
        flush(&mut text, &mut out);
        out.push(Segment::Code { lang, code: body.trim_end_matches('\n').to_owned() });
    }
    flush(&mut text, &mut out);
    out
}

fn push_cell(rows: &mut [Vec<String>], markup: &str) {
    if let Some(cell) = rows.last_mut().and_then(|row| row.last_mut()) {
        cell.push_str(markup);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_and_inline_styles() {
        let s = segments("Hello **world** and *you* <3\n\nSecond");
        assert_eq!(s, vec![Segment::Text("Hello <b>world</b> and <i>you</i> &lt;3\n\nSecond".into())]);
    }

    #[test]
    fn code_blocks_split_the_text() {
        let s = segments("Run:\n\n```sh\ncargo build\n```\n\nDone.");
        assert_eq!(
            s,
            vec![
                Segment::Text("Run:".into()),
                Segment::Code { lang: "sh".into(), code: "cargo build".into() },
                Segment::Text("Done.".into()),
            ]
        );
    }

    #[test]
    fn an_unclosed_fence_still_shows_while_streaming() {
        let s = segments("```rust\nfn main() {");
        assert_eq!(s, vec![Segment::Code { lang: "rust".into(), code: "fn main() {".into() }]);
    }

    #[test]
    fn lists_get_bullets_and_numbers() {
        let s = segments("- a\n- b\n\n1. x\n2. y");
        assert_eq!(s, vec![Segment::Text("•  a\n•  b\n\n1. x\n2. y".into())]);
    }

    #[test]
    fn tables_keep_their_rows_and_inline_styles() {
        let s = segments("| a | **bb** |\n|---|---|\n| `c<` | d |");
        assert_eq!(
            s,
            vec![Segment::Table(vec![vec!["a".into(), "<b>bb</b>".into()], vec!["<tt>c&lt;</tt>".into(), "d".into()]])]
        );
    }

    #[test]
    fn a_code_block_in_a_quote_leaves_valid_markup() {
        let s = segments("> look:\n>\n> ```\n> x\n> ```");
        for segment in &s {
            if let Segment::Text(markup) = segment {
                assert!(gtk::pango::parse_markup(markup, '\0').is_ok(), "{markup}");
            }
        }
        assert!(s.contains(&Segment::Code { lang: String::new(), code: "x".into() }));
    }

    #[test]
    fn persian_text_passes_through() {
        let s = segments("سلام **دنیا**");
        assert_eq!(s, vec![Segment::Text("سلام <b>دنیا</b>".into())]);
    }
}

