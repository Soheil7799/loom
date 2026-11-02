//! The notes vault, read-only, for a paired device (`notes_list`, `note_read`, `notes_search`):
//! its folders, a note with its links both ways, and search. Nothing is ever written. Paths are
//! the vault's, with `/`; nothing hidden (a part starting with `.`) and nothing outside the vault
//! (`..`, absolute paths, links pointing out) is listed or read. These read files: callers run
//! them off the UI's thread.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde_json::{Value, json};

use crate::notes::Index;

pub const OUTSIDE: &str = "That is not in the vault.";

/// The vault's index, kept between requests and refreshed from the files' times (as the MCP
/// server keeps its own).
static INDEX: Mutex<Option<Index>> = Mutex::new(None);

fn with_index<T>(root: &Path, use_it: impl FnOnce(&Index) -> T) -> T {
    let mut held = INDEX.lock().unwrap_or_else(|e| e.into_inner());
    if held.as_ref().is_none_or(|index| index.root() != root) {
        *held = Some(Index::new(root));
    }
    let index = held.get_or_insert_with(|| Index::new(root));
    index.refresh();
    use_it(index)
}

/// Where `relative` is in the vault at `root`: checked to be in it, not hidden, and there.
fn place(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let parts: Vec<&str> = relative.split('/').filter(|p| !p.is_empty()).collect();
    if relative.starts_with('/') || relative.contains('\\') || parts.iter().any(|p| p.starts_with('.')) {
        return Err(OUTSIDE.into());
    }
    let root = root.canonicalize().map_err(|_| "The vault folder is not there.".to_owned())?;
    let path = parts.iter().fold(root.clone(), |path, part| path.join(part)).canonicalize().map_err(|_| "It is no longer there.".to_owned())?;
    if !path.starts_with(&root) {
        return Err(OUTSIDE.into());
    }
    Ok(path)
}

/// `path`'s place in the vault at `root` (canonical), with `/`.
fn relative(root: &Path, path: &Path) -> String {
    let parts: Vec<String> = path.strip_prefix(root).unwrap_or(path).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    parts.join("/")
}

fn seconds(time: std::io::Result<SystemTime>) -> u64 {
    time.ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

fn is_note(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The entries of `folder` that may be shown: not hidden, not leading out of the vault.
fn visible(root: &Path, folder: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path().canonicalize().ok()?;
            (!name.starts_with('.') && path.starts_with(root)).then_some((name, path))
        })
        .collect()
}

/// `notes_list {folder}`: the folder's entries, folders first, then notes and files by name.
pub fn list(root: &Path, folder: &str) -> Result<Value, String> {
    let dir = place(root, folder)?;
    if !dir.is_dir() {
        return Err("That is not a folder.".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut entries: Vec<(bool, String, Value)> = visible(&root, &dir)
        .into_iter()
        .filter_map(|(name, path)| {
            let meta = std::fs::metadata(&path).ok()?;
            let at = relative(&root, &path);
            Some(if meta.is_dir() {
                (true, name.to_lowercase(), json!({ "name": name, "path": at, "kind": "folder", "count": visible(&root, &path).len() }))
            } else if is_note(&path) {
                (false, name.to_lowercase(), json!({ "name": name, "path": at, "kind": "note", "title": stem(&path), "size": meta.len(), "modified": seconds(meta.modified()) }))
            } else {
                (false, name.to_lowercase(), json!({ "name": name, "path": at, "kind": "file", "size": meta.len(), "modified": seconds(meta.modified()) }))
            })
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let vault = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(json!({ "vault": vault, "folder": relative(&root, &dir), "entries": entries.into_iter().map(|e| e.2).collect::<Vec<_>>() }))
}

/// `note_read {path}`: a note's text with its tags, its `[[links]]` (each resolved as Obsidian
/// does, or null) and its backlinks; another file's text, or that it is binary.
pub fn read(root: &Path, path: &str) -> Result<Value, String> {
    let file = place(root, path)?;
    if !file.is_file() {
        return Err("That is not a file.".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let at = relative(&root, &file);
    let mut read = super::host::read_text(&file)?;
    read["path"] = json!(at);
    read["modified"] = json!(seconds(std::fs::metadata(&file).and_then(|m| m.modified())));
    if read["binary"] == true || !is_note(&file) {
        return Ok(read);
    }
    let links = crate::notes::wikilinks(read["text"].as_str().unwrap_or(""));
    with_index(&root, |index| {
        read["title"] = json!(index.title(&at).unwrap_or_else(|| stem(&file)));
        read["tags"] = json!(index.tags(&at));
        read["links"] = links.iter().map(|target| json!({ "target": target, "path": index.resolve(target) })).collect();
        read["backlinks"] = index
            .backlinks(&at)
            .into_iter()
            .map(|path| {
                let title = index.title(&path).unwrap_or_default();
                json!({ "path": path, "title": title })
            })
            .collect();
    });
    Ok(read)
}

/// `notes_search {query, limit}`: notes with every word, names first, then the newest.
pub fn search(root: &Path, query: &str, limit: usize) -> Result<Value, String> {
    let root = root.canonicalize().map_err(|_| "The vault folder is not there.".to_owned())?;
    let found = with_index(&root, |index| {
        index.matching(query, limit).into_iter().map(|(path, title)| (index.modified(&path), path, title)).collect::<Vec<_>>()
    });
    let results: Vec<Value> = found
        .into_iter()
        .map(|(modified, path, title)| {
            let text = std::fs::read_to_string(root.join(&path)).unwrap_or_default();
            let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
            json!({ "path": path, "title": title, "folder": folder, "snippet": crate::notes::line_around(&text, query, 160), "modified": modified })
        })
        .collect();
    Ok(json!({ "results": results }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vault of its own for test `name` (they run at the same time).
    fn vault(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("loom-browse-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in [
            ("Work/plan.md", "---\naliases: [Roadmap]\ntags: [work]\n---\nGo by [[Budget 2026|the budget]], then [[Missing note]]. #work\n"),
            ("Work/scan.pdf", "%PDF\0binary"),
            ("Work/notes.txt", "plain text\n"),
            ("Finances/Budget 2026.md", "Money for the roadmap. Back to [[plan]].\n"),
            ("Journal/2026-11-01.md", "Wrote the [[Plan]] today.\n"),
            ("alpha.md", "first\n"),
            (".obsidian/app.json", "{}"),
            ("Work/.hidden.md", "secret\n"),
        ] {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        root
    }

    #[test]
    fn paths_stay_in_the_vault_and_hidden_ones_are_never_read() {
        let root = vault("paths");
        let outside = root.parent().unwrap().join(format!("loom-browse-out-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.md"), "no").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("Out")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.md"), root.join("leak.md")).unwrap();
        for path in ["../x.md", "/etc/passwd", "Topics/../../x.md", ".obsidian/app.json", "Work/.hidden.md", "Out/secret.md", "leak.md", "a\\b.md"] {
            assert_eq!(read(&root, path).unwrap_err(), OUTSIDE, "{path}");
        }
        assert_eq!(list(&root, "Out").unwrap_err(), OUTSIDE);
        assert_eq!(list(&root, "..").unwrap_err(), OUTSIDE);
        // A link pointing out is not listed either.
        let names: Vec<String> = list(&root, "").unwrap()["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap().to_owned()).collect();
        assert_eq!(names, ["Finances", "Journal", "Work", "alpha.md"]);
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_lists_folders_first_then_by_name() {
        let root = vault("list");
        let top = list(&root, "").unwrap();
        assert_eq!(top["vault"], root.file_name().unwrap().to_string_lossy().as_ref());
        assert_eq!(top["folder"], "");
        assert_eq!(top["entries"][2], json!({ "name": "Work", "path": "Work", "kind": "folder", "count": 3 }));
        let work = list(&root, "Work").unwrap();
        assert_eq!(work["folder"], "Work");
        let entries = work["entries"].as_array().unwrap();
        let kinds: Vec<(&str, &str)> = entries.iter().map(|e| (e["name"].as_str().unwrap(), e["kind"].as_str().unwrap())).collect();
        assert_eq!(kinds, [("notes.txt", "file"), ("plan.md", "note"), ("scan.pdf", "file")]);
        assert_eq!(entries[1]["title"], "plan");
        assert_eq!(entries[1]["path"], "Work/plan.md");
        assert!(entries[1]["modified"].as_u64().unwrap() > 1_600_000_000);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_note_reads_with_its_links_both_ways() {
        let root = vault("read");
        let plan = read(&root, "Work/plan.md").unwrap();
        assert_eq!(plan["title"], "plan");
        assert_eq!(plan["tags"], json!(["work"]));
        assert_eq!(plan["truncated"], false);
        assert_eq!(plan["links"], json!([{ "target": "Budget 2026", "path": "Finances/Budget 2026.md" }, { "target": "Missing note", "path": null }]));
        let back: Vec<&str> = plan["backlinks"].as_array().unwrap().iter().map(|b| b["path"].as_str().unwrap()).collect();
        assert_eq!(back, ["Finances/Budget 2026.md", "Journal/2026-11-01.md"]);
        assert_eq!(plan["backlinks"][1]["title"], "2026-11-01");
        let pdf = read(&root, "Work/scan.pdf").unwrap();
        assert_eq!((pdf["binary"].as_bool(), pdf["path"].as_str()), (Some(true), Some("Work/scan.pdf")));
        let txt = read(&root, "Work/notes.txt").unwrap();
        assert_eq!(txt["text"], "plain text\n");
        assert!(txt.get("links").is_none() && txt.get("backlinks").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_needs_every_word_and_puts_names_first() {
        let root = vault("search");
        let found = search(&root, "roadmap", 30).unwrap();
        let paths: Vec<&str> = found["results"].as_array().unwrap().iter().map(|r| r["path"].as_str().unwrap()).collect();
        // The alias first, then the one with the word in its text.
        assert_eq!(paths, ["Work/plan.md", "Finances/Budget 2026.md"]);
        assert_eq!(found["results"][0]["folder"], "Work");
        assert_eq!(found["results"][1]["snippet"], "Money for the roadmap. Back to [[plan]].");
        assert_eq!(search(&root, "roadmap money", 30).unwrap()["results"].as_array().unwrap().len(), 1);
        assert_eq!(search(&root, "roadmap", 1).unwrap()["results"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
