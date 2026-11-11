//! loom_find: where something is in a project's code when its file isn't known. Each source
//! file is cut into pieces, one per definition (a function, a type, a heading…) with its
//! doc comment and body; a question in plain words is ranked against them with BM25 (names,
//! doc comments and paths weighing more than bodies). It answers with places, never code,
//! so the agent reads only the lines it needs instead of grepping and reading whole files.
//!
//! Concept cards (`<project>/.loom/map/*.md`, written by agents when they finish work) are
//! searched with the code and weigh more: a feature in people's words (aliases), all its
//! files and symbols at once, and why it is like this. A card naming a file or symbol that
//! is gone is said to be possibly out of date; the code stays the truth.
//!
//! Plain Rust, in the MCP server's process; refreshed from the files' times on each search.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Files worth searching, by extension.
const SOURCE: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "svelte", "vue", "astro", "py", "go", "java", "kt", "kts", "swift", "c", "h", "cc", "cpp", "hpp", "cs", "rb", "php", "lua", "sh",
    "bash", "zsh", "zig", "dart", "scala", "ex", "exs", "sql", "md", "nix", "ml", "hs", "jl", "r", "css", "scss",
];
const BIGGEST: u64 = 512 * 1024;
/// Words of a body counted, at most (a long function is about its start).
const BODY_LINES: usize = 80;
const WORDS_IGNORED: &[&str] = &["the", "an", "of", "to", "in", "is", "it", "and", "or", "for", "on", "be", "with", "as", "at", "by", "this", "that", "if", "else", "let", "mut", "pub", "self", "where", "what", "how", "which", "does", "do", "are", "from", "its", "when"];

struct Piece {
    line: usize,
    /// Its first line, trimmed (what the answer shows).
    head: String,
    /// The first line of its doc comment.
    doc: String,
    terms: Vec<(String, u32)>,
    len: u32,
    /// A concept card's own (its files and symbols, checked when it is found).
    card: Option<(Vec<String>, Vec<String>)>,
}

struct File {
    stamp: (SystemTime, u64),
    pieces: Vec<Piece>,
}

pub struct Index {
    root: PathBuf,
    /// The project's concept cards (`<project>/.loom/map`), searched with the code.
    map: Option<PathBuf>,
    files: HashMap<String, File>,
    /// term → (file, piece, count), rebuilt when a file changed.
    postings: HashMap<String, Vec<(usize, usize, u32)>>,
    order: Vec<String>,
    average: f64,
}

/// One answer: `path:line`, its definition line, the first line of its doc comment, and
/// the same definition elsewhere (another checkout of the repo).
#[derive(Debug, PartialEq)]
pub struct Found {
    pub path: String,
    pub line: usize,
    pub head: String,
    pub doc: String,
    pub also: Vec<String>,
}

/// Where a project's concept cards are.
pub fn map_of(project: &Path) -> PathBuf {
    project.join(".loom").join("map")
}

impl Index {
    /// The code under `root` (the agent's folder), and the cards in `map`.
    pub fn new(root: &Path, map: Option<PathBuf>) -> Index {
        Index { root: root.to_path_buf(), map, files: HashMap::new(), postings: HashMap::new(), order: Vec::new(), average: 1.0 }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Changed files read again, gone ones dropped.
    pub fn refresh(&mut self) {
        // as shown (relative to root), and where it is
        let mut listed: Vec<(String, PathBuf)> = crate::quickopen::list_files(&self.root)
            .into_iter()
            // not folders, nor what lies in dot-folders (notes, caches, tools' state)
            .filter(|f| !f.ends_with('/') && !f.split('/').any(|part| part.starts_with('.')))
            .filter(|f| Path::new(f).extension().is_some_and(|e| SOURCE.contains(&e.to_string_lossy().to_lowercase().as_str())))
            .map(|f| (f.clone(), self.root.join(&f)))
            .collect();
        if let Some(map) = &self.map {
            for entry in std::fs::read_dir(map).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "md") {
                    let shown = path.strip_prefix(&self.root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string_lossy().into_owned());
                    listed.push((shown, path));
                }
            }
        }
        let mut changed = self.files.len() != listed.len() || listed.iter().any(|(f, _)| !self.files.contains_key(f));
        let mut kept = HashMap::with_capacity(listed.len());
        for (shown, path) in listed {
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if meta.len() > BIGGEST {
                continue;
            }
            let stamp = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
            match self.files.remove(&shown) {
                Some(file) if file.stamp == stamp => {
                    kept.insert(shown, file);
                }
                _ => {
                    changed = true;
                    let text = std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                    let is_card = self.map.as_ref().is_some_and(|m| path.starts_with(m));
                    let pieces = if is_card { card(&path, &text).into_iter().collect() } else { pieces(&shown, &text) };
                    kept.insert(shown, File { stamp, pieces });
                }
            }
        }
        self.files = kept;
        if changed {
            self.reindex();
        }
    }

    fn reindex(&mut self) {
        self.order = self.files.keys().cloned().collect();
        self.order.sort();
        self.postings.clear();
        let (mut total, mut count) = (0u64, 0u64);
        for (f, path) in self.order.iter().enumerate() {
            for (p, piece) in self.files[path].pieces.iter().enumerate() {
                total += piece.len as u64;
                count += 1;
                for (term, n) in &piece.terms {
                    self.postings.entry(term.clone()).or_default().push((f, p, *n));
                }
            }
        }
        self.average = if count == 0 { 1.0 } else { total as f64 / count as f64 };
    }

    pub fn pieces(&self) -> usize {
        self.files.values().map(|f| f.pieces.len()).sum()
    }

    /// The pieces best matching `query`, at most `limit`, at most 3 from one file; one
    /// definition met again in another checkout (same file name and line) is said once.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Found> {
        let mut terms = words(query);
        terms.sort();
        terms.dedup();
        let n = self.pieces() as f64;
        let mut scores: HashMap<(usize, usize), f64> = HashMap::new();
        for term in &terms {
            let Some(list) = self.postings.get(term) else { continue };
            let idf = (1.0 + (n - list.len() as f64 + 0.5) / (list.len() as f64 + 0.5)).ln();
            for &(f, p, tf) in list {
                let len = self.files[&self.order[f]].pieces[p].len as f64;
                let tf = tf as f64;
                *scores.entry((f, p)).or_default() += idf * tf * 2.2 / (tf + 1.2 * (0.25 + 0.75 * len / self.average));
            }
        }
        let mut ranked: Vec<_> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut per_file: HashMap<usize, usize> = HashMap::new();
        let mut found: Vec<Found> = Vec::new();
        for ((f, p), _) in ranked {
            let path = &self.order[f];
            let piece = &self.files[path].pieces[p];
            let name = |p: &str| p.rsplit('/').next().unwrap_or(p).to_owned();
            if let Some(same) = found.iter_mut().find(|x| x.head == piece.head && x.doc == piece.doc && name(&x.path) == name(path)) {
                same.also.push(format!("{path}:{}", piece.line));
                continue;
            }
            if found.len() == limit {
                continue;
            }
            let seen = per_file.entry(f).or_default();
            if *seen == 3 {
                continue;
            }
            *seen += 1;
            let mut head = piece.head.clone();
            if let Some((files, symbols)) = &piece.card {
                let gone: Vec<&str> = self.gone(files, symbols);
                if !gone.is_empty() {
                    head.push_str(&format!(" (may be out of date: {} not found)", gone.join(", ")));
                }
            }
            found.push(Found { path: path.clone(), line: piece.line, head, doc: piece.doc.clone(), also: Vec::new() });
        }
        found
    }

    /// What a card names that isn't there any more: files (relative to the project) and
    /// symbols no definition is named after.
    fn gone<'a>(&self, files: &'a [String], symbols: &'a [String]) -> Vec<&'a str> {
        let project = self.map.as_ref().and_then(|m| m.parent()?.parent());
        let mut gone: Vec<&str> = files.iter().filter(|f| project.is_some_and(|p| !p.join(f).exists())).map(String::as_str).collect();
        for symbol in symbols {
            let last = symbol.rsplit("::").next().unwrap_or(symbol);
            let defined = self.files.values().flat_map(|f| &f.pieces).any(|p| p.card.is_none() && p.head.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| w == last));
            if !defined {
                gone.push(symbol);
            }
        }
        gone
    }
}

/// A concept card (a feature, in the words people use for it): front matter with `title`,
/// `aliases`, `files`, `symbols` (lists as `[a, b]` or `- a` lines), then what it is, why
/// and its gotchas. Weighs more than code: it is the map, written for finding.
fn card(path: &Path, text: &str) -> Option<Piece> {
    let mut fields: HashMap<String, Vec<String>> = HashMap::new();
    let mut body = text;
    if let Some(rest) = text.strip_prefix("---\n") {
        let (front, after) = rest.split_once("\n---").unwrap_or((rest, ""));
        body = after.trim_start_matches(['-', '\n']);
        let mut key = String::new();
        for line in front.lines() {
            if let Some(item) = line.trim_start().strip_prefix("- ") {
                fields.entry(key.clone()).or_default().push(item.trim().trim_matches(['"', '\'']).to_owned());
            } else if let Some((k, v)) = line.split_once(':') {
                key = k.trim().to_lowercase();
                let v = v.trim().trim_start_matches('[').trim_end_matches(']');
                fields.entry(key.clone()).or_default().extend(v.split(',').map(|x| x.trim().trim_matches(['"', '\'']).to_owned()).filter(|x| !x.is_empty()));
            }
        }
    }
    let one = |k: &str| fields.get(k).cloned().unwrap_or_default();
    let stem = path.file_stem()?.to_string_lossy().replace(['-', '_'], " ");
    let title = one("title").first().cloned().unwrap_or(stem);
    let (aliases, files, symbols) = (one("aliases"), one("files"), one("symbols"));
    let summary = body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).unwrap_or("");
    let mut counts: HashMap<String, u32> = HashMap::new();
    for (text, weight) in [(format!("{title} {}", aliases.join(" ")), 8), (format!("{} {}", files.join(" "), symbols.join(" ")), 4), (body.to_owned(), 2)] {
        for w in words(&text) {
            *counts.entry(w).or_default() += weight;
        }
    }
    let mut doc = summary.chars().take(200).collect::<String>();
    if !files.is_empty() {
        doc.push_str(&format!("\n    files: {}", files.join(", ")));
    }
    if !symbols.is_empty() {
        doc.push_str(&format!("\n    symbols: {}", symbols.join(", ")));
    }
    let len = counts.values().sum::<u32>() / 2;
    Some(Piece { line: 1, head: format!("card: {title}"), doc, terms: counts.into_iter().collect(), len, card: Some((files, symbols)) })
}

/// `text` cut at its definitions: each from its line to the next one's, with the comment
/// right above it; what comes before the first (a module's own comment) is a piece too.
fn pieces(path: &str, text: &str) -> Vec<Piece> {
    let lines: Vec<&str> = text.lines().collect();
    let markdown = path.ends_with(".md");
    let starts: Vec<usize> = (0..lines.len()).filter(|&i| if markdown { lines[i].starts_with('#') } else { defines(lines[i]) }).collect();
    let mut bounds = vec![0];
    bounds.extend(starts.iter().copied().filter(|&s| s > 0));
    let path_words = words(path);
    let mut out = Vec::new();
    for (k, &start) in bounds.iter().enumerate() {
        let end = bounds.get(k + 1).copied().unwrap_or(lines.len());
        // its doc comment: the comment lines right above it (cut from the piece before)
        let mut above = start;
        while above > 0 && start - above < 12 && comment(lines[above - 1]) {
            above -= 1;
        }
        let doc_lines: Vec<&str> = lines[above..start].iter().map(|l| uncomment(l)).filter(|l| !l.is_empty()).collect();
        let first = lines.get(start).copied().unwrap_or("");
        let head_words = if starts.contains(&start) { words(first) } else { Vec::new() };
        let body = lines[start..end.min(start + BODY_LINES)].join("\n");
        let mut counts: HashMap<String, u32> = HashMap::new();
        for (list, weight) in [(&head_words, 3), (&words(&doc_lines.join(" ")), 2), (&path_words, 1), (&words(&body), 1)] {
            for w in list {
                *counts.entry(w.clone()).or_default() += weight;
            }
        }
        // the piece before the first definition: only its comments speak for it
        let (head, doc) = if starts.contains(&start) {
            (first.trim().chars().take(140).collect(), doc_lines.first().map(|d| d.chars().take(140).collect()).unwrap_or_default())
        } else {
            let own: Vec<&str> = lines[..end].iter().filter(|l| comment(l)).map(|l| uncomment(l)).filter(|l| !l.is_empty()).collect();
            if own.is_empty() && counts.is_empty() {
                continue;
            }
            ("(the file's start)".to_owned(), own.first().map(|d| d.chars().take(140).collect()).unwrap_or_default())
        };
        let len = counts.values().sum();
        out.push(Piece { line: start + 1, head, doc, terms: counts.into_iter().collect(), len, card: None });
    }
    out
}

/// A line that starts a definition, in most languages: a keyword first, after modifiers,
/// not deeply indented (a method in a class or impl is; a local `let` in a body is not).
fn defines(line: &str) -> bool {
    let indent = line.len() - line.trim_start().len();
    if indent > 8 {
        return false;
    }
    let mut rest = line.trim_start();
    loop {
        let before = rest;
        for modifier in ["pub(crate) ", "pub(super) ", "pub ", "export ", "default ", "async ", "static ", "public ", "private ", "protected ", "unsafe ", "extern ", "abstract ", "override ", "final ", "inline ", "const fn ", "declare "] {
            if let Some(r) = rest.strip_prefix(modifier) {
                rest = if modifier == "const fn " { "fn " } else { r };
            }
        }
        if rest == before {
            break;
        }
    }
    const TOP: &[&str] = &["const ", "let ", "var ", "static "];
    const ANY: &[&str] = &["fn ", "struct ", "enum ", "trait ", "impl ", "impl<", "type ", "mod ", "macro_rules!", "class ", "interface ", "def ", "function ", "function*", "func ", "module ", "record ", "object ", "union "];
    ANY.iter().any(|k| rest.starts_with(k)) || (indent == 0 && TOP.iter().any(|k| rest.starts_with(k)))
}

fn comment(line: &str) -> bool {
    let t = line.trim_start();
    ["//", "#", "/*", "*", "--", "\"\"\"", "<!--"].iter().any(|c| t.starts_with(c)) && !t.starts_with("#[") && !t.starts_with("#!")
}

fn uncomment(line: &str) -> &str {
    line.trim().trim_start_matches(['/', '#', '*', '-', '!', '"', '<']).trim()
}

/// The words of `text`, lowercased: identifiers whole and in parts (`show_sub` → show_sub,
/// show, sub; `crewRemove` → crewremov, crew, remov), endings dropped (stem), short and
/// common ones dropped.
pub fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if raw.is_empty() {
            continue;
        }
        let mut parts: Vec<String> = Vec::new();
        let mut part = String::new();
        let mut last_lower = false;
        for c in raw.chars() {
            if (c == '_' || (c.is_uppercase() && last_lower)) && !part.is_empty() {
                parts.push(std::mem::take(&mut part));
            }
            if c != '_' {
                part.extend(c.to_lowercase());
            }
            last_lower = c.is_lowercase() || c.is_ascii_digit();
        }
        if !part.is_empty() {
            parts.push(part);
        }
        if parts.len() > 1 {
            out.push(raw.to_lowercase());
        }
        out.extend(parts);
    }
    out.into_iter()
        .filter(|w| w.chars().count() > 1 && !w.chars().all(|c| c.is_ascii_digit()) && !WORDS_IGNORED.contains(&w.as_str()))
        .map(stem)
        .collect()
}

/// Endings dropped, so forms of a word meet: removed, removes, remove → remov.
fn stem(word: String) -> String {
    for end in ["ing", "ed", "es", "s", "e"] {
        if word.len() > end.len() + 2 && word.ends_with(end) && !(end == "s" && word.ends_with("ss")) {
            return word[..word.len() - end.len()].to_owned();
        }
    }
    word
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_found_by_their_parts_and_plain_words() {
        assert_eq!(words("crewRemove show_sub"), ["crewremov", "crew", "remov", "show_sub", "show", "sub"]);
        assert_eq!(words("the rows removed"), ["row", "remov"]);
    }

    #[test]
    fn a_question_finds_the_definition_that_answers_it() {
        let root = std::env::temp_dir().join(format!("loom-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("core/agent")).unwrap();
        std::fs::write(
            root.join("core/agent/mod.rs"),
            "//! An agent.\n\nuse std::rc::Rc;\n\n/// Its subagent's row in the strip, or gone once it ended.\nfn show_sub(&self, sub: &Sub) {\n    if sub.ended { self.view.crew_remove(&sub.id); }\n}\n\npub fn rename(&self, title: &str) {\n    self.title = title;\n}\n",
        )
        .unwrap();
        std::fs::write(root.join("web.ts"), "export function archiveDone(board: Board) {\n  // the Done lane, moved to the archive\n}\n").unwrap();
        std::fs::write(root.join("README.md"), "# loom\n\n## Board\n\nCards in lanes.\n").unwrap();
        let mut index = Index::new(&root, None);
        index.refresh();
        fn top(index: &Index, q: &str) -> Option<(String, usize)> {
            index.search(q, 3).into_iter().next().map(|f| (f.path, f.line))
        }
        assert_eq!(top(&index, "where are finished subagent rows removed"), Some(("core/agent/mod.rs".into(), 6)));
        assert_eq!(top(&index, "archive done"), Some(("web.ts".into(), 1)));
        assert_eq!(top(&index, "board cards lanes"), Some(("README.md".into(), 3)));
        assert_eq!(index.search("rename", 1)[0].head, "pub fn rename(&self, title: &str) {");
        // a change is seen at the next search
        std::fs::write(root.join("web.ts"), "export function wipeLane() {}\n").unwrap();
        index.refresh();
        assert_eq!(top(&index, "wipe lane"), Some(("web.ts".into(), 1)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cards_answer_in_peoples_words_and_say_when_they_are_out_of_date() {
        let root = std::env::temp_dir().join(format!("loom-cards-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let code = "/// Its row in the strip, gone once it ended.\nfn show_sub(&self) {}\n";
        for dir in ["app/core", "app-copy/core", ".remember"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("app/core/agent.rs"), code).unwrap();
        // the same repo checked out twice (a worktree): said once
        std::fs::write(root.join("app-copy/core/agent.rs"), code).unwrap();
        // a dot-folder's notes are not code
        std::fs::write(root.join(".remember/today.md"), "# stuck subagent rows\n").unwrap();
        let map = map_of(&root);
        std::fs::create_dir_all(&map).unwrap();
        std::fs::write(
            map.join("subagent-strip.md"),
            "---\ntitle: Subagent strip\naliases: [stuck rows, crew]\nfiles: [app/core/agent.rs]\nsymbols:\n  - show_sub\n---\nSubagents' live rows above the chat; loomd keeps them.\n",
        )
        .unwrap();
        std::fs::write(map.join("old.md"), "---\nfiles: [app/gone.rs]\nsymbols: [vanished_fn]\n---\nThe old pairing flow.\n").unwrap();
        let mut index = Index::new(&root, Some(map.clone()));
        index.refresh();
        let found = index.search("stuck rows", 5);
        assert_eq!(found[0].head, "card: Subagent strip");
        assert!(found[0].doc.contains("files: app/core/agent.rs"), "{}", found[0].doc);
        assert!(found.iter().all(|f| !f.path.starts_with(".remember")), "{found:?}");
        let code = index.search("row strip ended", 5);
        let sub = code.iter().find(|f| f.head.starts_with("fn show_sub")).expect("show_sub");
        assert_eq!(sub.also.len(), 1, "{code:?}");
        assert_eq!(index.search("old pairing flow", 1)[0].head, "card: old (may be out of date: app/gone.rs, vanished_fn not found)");
        let _ = std::fs::remove_dir_all(&root);
    }
}