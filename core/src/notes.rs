//! The vault's index: every note's title, aliases, tags, headings and `[[links]]` (both
//! ways), and its words, kept in memory and refreshed from the files' times. It finds the
//! notes a question or a change is about fast, without reading the vault each time.
//!
//! Plain Rust, no GTK: the MCP server (`loom mcp`) runs it in its own process.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::vault;

#[derive(Clone, Debug, Default)]
struct Note {
    /// The file's name without `.md` (what `[[links]]` name).
    title: String,
    aliases: Vec<String>,
    tags: Vec<String>,
    headings: Vec<String>,
    /// The notes it links to, as written (`[[Target]]` → "Target").
    links: Vec<String>,
    /// Its text, lowercased, for search (the text itself is read from the file when needed).
    lower: String,
    stamp: Option<(SystemTime, u64)>,
}

pub struct Index {
    root: PathBuf,
    /// By path relative to the vault.
    notes: HashMap<String, Note>,
    /// What a name means, lowercased: a path (without .md), then a title, then an alias.
    names: HashMap<String, String>,
    /// Each note's links and backlinks, as paths (worked out at each refresh).
    links: HashMap<String, Vec<String>>,
    backlinks: HashMap<String, Vec<String>>,
}

/// One search result.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: String,
    pub title: String,
    pub score: u32,
    /// The lines that matched, a few.
    pub snippet: String,
    pub links: Vec<String>,
    pub backlinks: Vec<String>,
}

/// loomd's one index of the vault, shared by Scribe and a paired device's browsing.
static SHARED: std::sync::Mutex<Option<Index>> = std::sync::Mutex::new(None);

/// Uses loomd's index of the vault at `root`, refreshed first. The lock is held only for
/// `use_it`: never call a model inside it.
pub fn shared<T>(root: &Path, use_it: impl FnOnce(&Index) -> T) -> T {
    let mut held = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    if held.as_ref().is_none_or(|index| index.root() != root) {
        *held = Some(Index::new(root));
    }
    let index = held.as_mut().expect("set above");
    index.refresh();
    use_it(index)
}

impl Index {
    pub fn new(root: &Path) -> Index {
        Index { root: root.to_owned(), notes: HashMap::new(), names: HashMap::new(), links: HashMap::new(), backlinks: HashMap::new() }
    }

    /// Reads what changed since the last time (by modification time and size); drops what is
    /// gone. The topic files are settings, not notes: left out.
    pub fn refresh(&mut self) {
        let mut seen = HashSet::new();
        let mut changed = false;
        for file in vault::notes_in(&self.root) {
            let Ok(relative) = file.strip_prefix(&self.root) else { continue };
            if relative.starts_with(vault::TOPICS) {
                continue;
            }
            let key = relative.to_string_lossy().into_owned();
            // A sync tool's conflict copy is not the note: the user settles it in loom.
            if vault::conflict_of(&key).is_some() {
                continue;
            }
            let stamp = std::fs::metadata(&file).ok().and_then(|m| Some((m.modified().ok()?, m.len())));
            seen.insert(key.clone());
            if stamp.is_some() && self.notes.get(&key).is_some_and(|n| n.stamp == stamp) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let title = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let mut note = read(&title, &text);
            note.stamp = stamp;
            self.notes.insert(key, note);
            changed = true;
        }
        let before = self.notes.len();
        self.notes.retain(|key, _| seen.contains(key));
        if changed || self.notes.len() != before || self.names.is_empty() {
            self.link_up();
        }
    }

    /// Names to paths, and every note's links and backlinks, worked out once.
    fn link_up(&mut self) {
        let mut paths: Vec<&String> = self.notes.keys().collect();
        // Sorted: the same name in two folders always means the same one.
        paths.sort();
        let mut names: HashMap<String, String> = HashMap::new();
        // Weakest first, so a path beats a title, which beats an alias.
        for path in &paths {
            for alias in &self.notes[path.as_str()].aliases {
                names.entry(alias.to_lowercase()).or_insert_with(|| (*path).clone());
            }
        }
        let mut titles: HashMap<String, String> = HashMap::new();
        for path in &paths {
            titles.entry(self.notes[path.as_str()].title.to_lowercase()).or_insert_with(|| (*path).clone());
        }
        names.extend(titles);
        for path in &paths {
            names.insert(path.trim_end_matches(".md").to_lowercase(), (*path).clone());
        }
        let mut links: HashMap<String, Vec<String>> = HashMap::new();
        let mut backlinks: HashMap<String, Vec<String>> = HashMap::new();
        for path in &paths {
            let mut out: Vec<String> = self.notes[path.as_str()].links.iter().filter_map(|l| names.get(&clean(l)).cloned()).collect();
            out.dedup();
            for target in &out {
                if target != *path {
                    backlinks.entry(target.clone()).or_default().push((*path).clone());
                }
            }
            links.insert((*path).clone(), out);
        }
        for list in backlinks.values_mut() {
            list.sort();
            list.dedup();
        }
        (self.names, self.links, self.backlinks) = (names, links, backlinks);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    /// The path of the note `[[name]]` means: by its path, title or alias (any case).
    pub fn resolve(&self, name: &str) -> Option<String> {
        self.names.get(&clean(name)).cloned()
    }

    /// The notes linking to `path`.
    pub fn backlinks(&self, path: &str) -> Vec<String> {
        self.backlinks.get(path).cloned().unwrap_or_default()
    }

    /// The notes `query` is about, best first: words in the title count most, then aliases,
    /// tags and headings, then the text. `folder` keeps to one folder (a topic's).
    pub fn search(&self, query: &str, folder: Option<&str>, limit: usize) -> Vec<Hit> {
        let words: Vec<String> = query.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2).map(str::to_lowercase).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let folder = folder.map(|f| f.trim_matches('/').to_lowercase()).filter(|f| !f.is_empty());
        let mut scored: Vec<(u32, &String)> = Vec::new();
        for (path, note) in &self.notes {
            if folder.as_ref().is_some_and(|f| !path.to_lowercase().starts_with(&format!("{f}/"))) {
                continue;
            }
            let title = note.title.to_lowercase();
            let mut score = 0;
            for word in &words {
                if title.contains(word.as_str()) {
                    score += 8;
                }
                if note.aliases.iter().any(|a| a.to_lowercase().contains(word.as_str())) {
                    score += 6;
                }
                if note.tags.iter().any(|t| t.to_lowercase() == *word) {
                    score += 5;
                }
                if note.headings.iter().any(|h| h.to_lowercase().contains(word.as_str())) {
                    score += 4;
                }
                score += note.lower.matches(word.as_str()).count().min(5) as u32;
            }
            if score > 0 {
                scored.push((score, path));
            }
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        scored.truncate(limit);
        // Only the few kept are read for their lines.
        scored.into_iter().map(|(score, path)| self.hit(path, score, &words)).collect()
    }

    fn hit(&self, path: &str, score: u32, words: &[String]) -> Hit {
        Hit {
            path: path.to_owned(),
            title: self.notes[path].title.clone(),
            score,
            snippet: snippet(&std::fs::read_to_string(self.root.join(path)).unwrap_or_default(), words),
            links: self.links.get(path).cloned().unwrap_or_default(),
            backlinks: self.backlinks(path),
        }
    }

    /// Every note's path, key and text for meaning search (its title and start), sorted by
    /// path: the order `search_by_meaning` takes their scores in.
    pub fn texts(&self) -> Vec<(String, u64, String)> {
        let mut all: Vec<(String, u64, String)> = self
            .notes
            .iter()
            .map(|(path, note)| {
                let text = format!("{}\n{}", note.title, note.lower.chars().take(3000).collect::<String>());
                (path.clone(), crate::embed::hash(&text), text)
            })
            .collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        all
    }

    /// The notes nearest `query` by meaning (`scores` in `texts`' order), then the best by
    /// words that meaning missed: `limit` in all, within `folder`.
    pub fn search_by_meaning(&self, query: &str, scores: &[f32], folder: Option<&str>, limit: usize) -> Vec<Hit> {
        let words: Vec<String> = query.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2).map(str::to_lowercase).collect();
        let inside = folder.map(|f| f.trim_matches('/').to_lowercase()).filter(|f| !f.is_empty());
        let paths: Vec<String> = self.texts().into_iter().map(|(p, _, _)| p).collect();
        let mut hits: Vec<Hit> = crate::embed::order(scores)
            .into_iter()
            .filter_map(|i| paths.get(i))
            .filter(|p| inside.as_ref().is_none_or(|f| p.to_lowercase().starts_with(&format!("{f}/"))))
            .take(limit.saturating_sub(2).max(1))
            .map(|p| self.hit(p, 0, &words))
            .collect();
        for extra in self.search(query, folder, limit) {
            if hits.len() >= limit {
                break;
            }
            if !hits.iter().any(|h| h.path == extra.path) {
                hits.push(extra);
            }
        }
        hits
    }

    /// A note's title (its file name, as `[[links]]` name it), by path.
    pub fn title(&self, path: &str) -> Option<String> {
        self.notes.get(path).map(|n| n.title.clone())
    }

    /// A note's tags (front matter and #tags in the text), each once.
    pub fn tags(&self, path: &str) -> Vec<String> {
        let mut tags = Vec::new();
        for tag in self.notes.get(path).map(|n| n.tags.as_slice()).unwrap_or_default() {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
        tags
    }

    /// When a note last changed, in Unix seconds.
    pub fn modified(&self, path: &str) -> Option<u64> {
        let (at, _) = self.notes.get(path)?.stamp?;
        Some(at.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs())
    }

    /// The notes with every word of `query` in their title, aliases, tags or text (any case),
    /// as (path, title): those with a word in the title or an alias first, then the most
    /// recently changed.
    pub fn matching(&self, query: &str, limit: usize) -> Vec<(String, String)> {
        let words: Vec<String> = query.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<(bool, u64, &String)> = Vec::new();
        for (path, note) in &self.notes {
            let title = note.title.to_lowercase();
            let aliases: Vec<String> = note.aliases.iter().map(|a| a.to_lowercase()).collect();
            let named = |word: &str| title.contains(word) || aliases.iter().any(|a| a.contains(word));
            let all = words.iter().all(|w| named(w) || note.tags.iter().any(|t| t.to_lowercase().contains(w.as_str())) || note.lower.contains(w.as_str()));
            if all {
                found.push((words.iter().any(|w| named(w)), self.modified(path).unwrap_or(0), path));
            }
        }
        found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then_with(|| a.2.cmp(b.2)));
        found.into_iter().take(limit).map(|(_, _, path)| (path.clone(), self.notes[path].title.clone())).collect()
    }

    /// A note's text, by path or `[[name]]`, with what it links to and what links to it.
    pub fn read(&self, name: &str) -> Option<(String, String, Vec<String>, Vec<String>)> {
        let path = self.resolve(name)?;
        let text = std::fs::read_to_string(self.root.join(&path)).ok()?;
        Some((path.clone(), text, self.links.get(&path).cloned().unwrap_or_default(), self.backlinks(&path)))
    }
}

/// A name as `[[links]]` and lookups use it: no `.md`, any case.
fn clean(name: &str) -> String {
    name.trim().trim_end_matches(".md").to_lowercase()
}

fn read(title: &str, text: &str) -> Note {
    let (keys, body) = vault::frontmatter(text);
    let items = |key: &str| vault::get(&keys, key).map(vault::Value::items).unwrap_or_default();
    let mut tags = items("tags");
    let mut headings = Vec::new();
    let mut links = Vec::new();
    let mut fenced = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        // An ATX heading ("## Title"); "#budget #2026" is a line of tags.
        let heading = trimmed.strip_prefix('#').is_some_and(|h| h.starts_with(['#', ' ']));
        if heading {
            headings.push(trimmed.trim_start_matches('#').trim().to_owned());
        }
        links_in(line, &mut links);
        // #tags in the text (not headings, not "#1").
        for word in line.split_whitespace() {
            if let Some(tag) = word.strip_prefix('#') {
                let tag = tag.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_' && c != '/');
                if !tag.is_empty() && tag.chars().any(char::is_alphabetic) && !heading {
                    tags.push(tag.to_owned());
                }
            }
        }
    }
    let mut aliases = items("aliases");
    aliases.extend(items("alias"));
    Note { title: title.to_owned(), aliases, tags, headings, links, lower: text.to_lowercase(), stamp: None }
}

/// The `[[links]]` of one line, onto `links`: [[Target]], [[Target|shown]], [[Target#heading]],
/// ![[embed]], each as its target is written.
fn links_in(line: &str, links: &mut Vec<String>) {
    let mut rest = line;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let target = after[..end].split(['|', '#']).next().unwrap_or("").trim();
        if !target.is_empty() {
            links.push(target.to_owned());
        }
        rest = &after[end + 2..];
    }
}

/// Every `[[link]]` target in `text` (code blocks left out), in order of first appearance,
/// each once.
pub fn wikilinks(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        } else if !fenced {
            links_in(line, &mut links);
        }
    }
    let mut seen = HashSet::new();
    links.retain(|l| seen.insert(l.clone()));
    links
}

/// About `width` characters of `text` (front matter left out) on one line, around the first
/// word of `query` in it; its start when none is. None when it has no text.
pub fn line_around(text: &str, query: &str, width: usize) -> Option<String> {
    let (_, body) = vault::frontmatter(text);
    let flat: Vec<char> = body.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if flat.is_empty() {
        return None;
    }
    // One lowercased char per char, so a place in one is the same place in the other.
    let lower: Vec<char> = flat.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let first = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .filter_map(|word| {
            let word: Vec<char> = word.to_lowercase().chars().collect();
            lower.windows(word.len()).position(|w| w == word.as_slice())
        })
        .min()
        .unwrap_or(0);
    let start = first.saturating_sub(width / 3).min(flat.len().saturating_sub(width));
    let end = (start + width).min(flat.len());
    let mut line: String = flat[start..end].iter().collect();
    if start > 0 {
        line.insert(0, '…');
    }
    if end < flat.len() {
        line.push('…');
    }
    Some(line)
}

/// Up to three lines of `text` with a query word in them (else its first lines).
fn snippet(text: &str, words: &[String]) -> String {
    let (_, body) = vault::frontmatter(text);
    let lines: Vec<&str> = body.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let matching: Vec<&str> = lines.iter().copied().filter(|l| words.iter().any(|w| l.to_lowercase().contains(w.as_str()))).take(3).collect();
    let chosen = if matching.is_empty() { lines.into_iter().take(2).collect() } else { matching };
    chosen.iter().map(|l| if l.chars().count() > 200 { format!("{}…", l.chars().take(200).collect::<String>()) } else { (*l).to_owned() }).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault_with(files: &[(&str, &str)]) -> (PathBuf, Index) {
        let root = std::env::temp_dir().join(format!("loom-notes-{}-{}", std::process::id(), files.len()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in files {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        let mut index = Index::new(&root);
        index.refresh();
        (root, index)
    }

    #[test]
    fn notes_are_found_and_linked() {
        let (root, mut index) = vault_with(&[
            ("Finances/Overview.md", "---\naliases: [money]\ntags: [finance]\n---\n## Salary\nNet 2,400 EUR a month. See [[2026-01|January]].\n"),
            ("Finances/2026-01.md", "# January\n| rent | 650 EUR |\n#budget\n"),
            ("Work/Acme.md", "Meeting about the release. Nothing on salary here? salary\n"),
            ("Topics/Finances.md", "---\ntopic: Finances\n---\nsalary salary salary\n"),
        ]);
        assert_eq!(index.len(), 3, "topic files are not notes");
        let hits = index.search("salary", None, 5);
        assert_eq!(hits[0].path, "Finances/Overview.md", "{hits:?}");
        assert_eq!(hits[0].links, vec!["Finances/2026-01.md"]);
        let only = index.search("salary", Some("Work"), 5);
        assert_eq!(only.len(), 1);
        assert_eq!(index.resolve("money").as_deref(), Some("Finances/Overview.md"));
        let (_, _, _, back) = index.read("2026-01").unwrap();
        assert_eq!(back, vec!["Finances/Overview.md"]);
        let budget = index.search("budget", None, 5);
        assert!(budget.iter().any(|h| h.path == "Finances/2026-01.md" && h.score >= 5), "{budget:?}");
        // A deleted note leaves the index.
        std::fs::remove_file(root.join("Work/Acme.md")).unwrap();
        index.refresh();
        assert_eq!(index.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn every_word_must_match_and_names_come_first() {
        let (root, index) = vault_with(&[
            ("Plan.md", "---
tags: [work]
---
The budget for the trip.
"),
            ("Trips/Budget.md", "Nothing about the plan of work here. trip
"),
            ("Other.md", "budget only
"),
        ]);
        let found: Vec<String> = index.matching("Budget trip", 10).into_iter().map(|(p, _)| p).collect();
        assert_eq!(found, ["Trips/Budget.md", "Plan.md"], "a title match first; Other.md lacks 'trip'");
        assert_eq!(index.matching("work plan", 10).len(), 2);
        assert_eq!(index.tags("Plan.md"), ["work"]);
        assert_eq!(index.title("Trips/Budget.md").as_deref(), Some("Budget"));
        assert!(index.modified("Plan.md").is_some_and(|t| t > 1_600_000_000));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn links_and_a_line_around_a_word() {
        let text = "See [[Budget 2026|money]] and [[Plan#Goals]], [[Budget 2026]] again.\n```\n[[In code]]\n```\n![[pic.png]] [[#Here]]\n";
        assert_eq!(wikilinks(text), ["Budget 2026", "Plan", "pic.png"]);
        let long = format!("---\ntags: [x]\n---\n{}needle{}", "a ".repeat(200), " b".repeat(200));
        let line = line_around(&long, "Needle", 160).unwrap();
        assert!(line.contains("needle") && line.starts_with('…') && line.ends_with('…') && !line.contains('\n'), "{line}");
        assert!(line.chars().count() <= 162);
        assert_eq!(line_around("Short note.", "absent", 160).as_deref(), Some("Short note."));
        assert_eq!(line_around("---\ntags: [x]\n---\n", "x", 160), None);
    }
}
