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
    /// A line across (`---`).
    Rule,
}

pub fn segments(markdown: &str) -> Vec<Segment> {
    segments_with(markdown, false)
}

/// What a person typed: Markdown too, but every line break where they put it, and a line of
/// only `---` (or `***`, `___`) always a rule: never the heading Markdown would make of the
/// text right above it.
pub fn typed_segments(markdown: &str) -> Vec<Segment> {
    let mut lines: Vec<&str> = Vec::new();
    let mut fenced = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
        }
        // 3+ of - * _ draw a line; shorter, or =, would make a heading of the line above.
        let rule = !fenced && trimmed.len() >= 3 && ["-", "*", "_"].iter().any(|c| trimmed.chars().all(|x| x.to_string() == *c));
        let underline = !fenced && !trimmed.is_empty() && (trimmed.chars().all(|c| c == '=') || trimmed.chars().all(|c| c == '-'));
        let rule = rule || underline;
        if rule && lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push("");
        }
        lines.push(line);
    }
    segments_with(&lines.join("\n"), true)
}

fn segments_with(markdown: &str, keep_lines: bool) -> Vec<Segment> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut out = Vec::new();
    let mut text = String::new();
    // Ordered lists hold the next number.
    let mut in_link = false;
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
                Tag::Link { dest_url, .. } => {
                    in_link = true;
                    text.push_str(&format!("<a href=\"{}\">", escape(&dest_url)));
                }
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
                TagEnd::Link => {
                    in_link = false;
                    text.push_str("</a>");
                }
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
            Event::Code(t) => {
                let span = format!(
                    "<span font_family=\"JetBrains Mono, Vazirmatn, monospace\" bgcolor=\"#888888\" bgalpha=\"22%\">{}</span>",
                    escape(&t)
                );
                // `src/app.rs:42` and the like open in loom's editor when clicked.
                if !in_link && looks_like_path(&t) {
                    text.push_str(&format!("<a href=\"loom-file:{}\">{span}</a>", escape(&t)));
                } else {
                    text.push_str(&span);
                }
            }
            Event::SoftBreak => text.push(if keep_lines { '\n' } else { ' ' }),
            Event::HardBreak => text.push('\n'),
            Event::Rule => {
                flush(&mut text, &mut out);
                out.push(Segment::Rule);
            }
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

/// Extensions that make a bare name ("Cargo.toml") a file rather than code ("agent.cwd").
const KNOWN: &[&str] = &[
    "rs", "toml", "md", "json", "yaml", "yml", "ts", "tsx", "js", "jsx", "mjs", "py", "go", "c", "h", "cpp", "hpp", "css", "scss",
    "html", "txt", "sh", "env", "lock", "sql", "vue", "svelte", "java", "kt", "swift", "rb", "php", "xml", "ini", "cfg", "conf",
    "log", "csv", "svg", "png", "jpg", "jpeg", "pdf", "typ", "tex", "nix", "lua", "zig", "dart", "proto", "graphql",
];

/// Files that have no extension but are files all the same.
const BARE: &[&str] = &["Makefile", "Dockerfile", "Justfile", "Containerfile", "LICENSE", "README", "CHANGELOG", "Procfile", "Gemfile", "Rakefile"];

/// A file or folder path, maybe with a line and column ("src/app.rs", "Cargo.toml:12",
/// "src/app.rs:12:5", "src/ui/", "docker/Makefile", "../README.md", "~/notes/todo.md"), not
/// code or other slashed words ("foo()", "agent.cwd", "origin/main", "application/json").
pub fn looks_like_path(text: &str) -> bool {
    let path = text.split(':').next().unwrap_or(text);
    // After the path only a line, maybe a column (":12", ":12:5").
    let rest = &text[path.len()..];
    let line_ok = rest.is_empty() || rest.split(':').skip(1).all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) && rest.split(':').count() <= 3;
    let allowed = path.chars().all(|c| c.is_alphanumeric() || "._-/~@+".contains(c));
    // "../" only at the front (a parent of the agent's folder), never inside.
    let inner = path.trim_start_matches("../");
    if path.is_empty() || path.len() >= 200 || !allowed || inner.contains("..") || !line_ok {
        return false;
    }
    let name = inner.trim_end_matches('/').rsplit('/').next().unwrap_or(inner);
    let known = name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && KNOWN.contains(&ext.to_ascii_lowercase().as_str()));
    // A slash alone is not enough ("origin/main"): a folder says so with its last "/", a
    // path from somewhere with "./", "../", "~/" or "/", and some files have no extension.
    let anchored = ["./", "../", "~/", "/"].iter().any(|p| path.starts_with(p)) && name.chars().any(char::is_alphabetic);
    let plain = |n: &str| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || "._-@+".contains(c)) && n.chars().any(char::is_alphabetic)
        // A key, not a folder ("Ctrl+/").
        && !["ctrl+", "alt+", "shift+", "super+", "cmd+", "meta+"].contains(&n.to_lowercase().as_str());
    let folder = path.contains('/') && path.ends_with('/') && inner.trim_end_matches('/').split('/').all(plain);
    known || anchored || folder || BARE.contains(&name)
}

#[cfg(test)]
mod typed_tests {
    use super::*;

    #[test]
    fn a_typed_dash_line_is_a_rule_not_a_heading() {
        let out = typed_segments("Some text\n---\nmore");
        assert_eq!(out.len(), 3, "{out:?}");
        assert_eq!(out[1], Segment::Rule);
        let Segment::Text(markup) = &out[0] else { panic!("{out:?}") };
        assert!(!markup.contains("<span size"), "{markup}");
        // Short underlines too: never a heading.
        let out = typed_segments("thanks\n--");
        assert!(out.iter().all(|s| !matches!(s, Segment::Text(m) if m.contains("<span size"))), "{out:?}");
        let out = typed_segments("total\n=");
        assert!(out.iter().all(|s| !matches!(s, Segment::Text(m) if m.contains("<span size"))), "{out:?}");
        // Their line breaks stay.
        let Segment::Text(markup) = &typed_segments("one\ntwo")[0] else { panic!() };
        assert_eq!(markup, "one\ntwo");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_told_from_code() {
        for path in ["src/app.rs", "Cargo.toml", "Cargo.toml:12", "~/notes/todo.md", "web/README.md:3"] {
            assert!(looks_like_path(path), "{path}");
        }
        for path in ["src/ui/", "node_modules/@types/", "include/c++/", "docker/Makefile", "src/app.rs:12:5", "../README.md", "../shared/config.toml", "./build", "/etc/hosts"] {
            assert!(looks_like_path(path), "{path}");
        }
        for code in [
            "foo()", "a + b", "x.len()", "cargo build", "1.5", ".env", "v0.95", "agent.cwd", "self.chat", "README.md:see below", "config.toml:foo",
            "src/../x.rs", "origin/main", "application/json", "and/or", "owner/repo", "src/app.rs:12:5:9", "Ctrl+/",
        ] {
            assert!(!looks_like_path(code), "{code}");
        }
    }

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

