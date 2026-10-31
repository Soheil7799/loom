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
        scored
            .into_iter()
            .map(|(score, path)| Hit {
                path: path.clone(),
                title: self.notes[path].title.clone(),
                score,
                snippet: snippet(&std::fs::read_to_string(self.root.join(path)).unwrap_or_default(), &words),
                links: self.links.get(path).cloned().unwrap_or_default(),
                backlinks: self.backlinks(path),
            })
            .collect()
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
        // [[Target]], [[Target|shown]], [[Target#heading]], ![[embed]].
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
}
