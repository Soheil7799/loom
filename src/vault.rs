//! The Notes add-on's vault: a folder Obsidian can open, which loom keeps notes in.
//!
//! Set up the first time it is chosen (`prepare`): what Obsidian needs (`.obsidian/`), loom's
//! own folder (`.loom/`), the default topic files (`Topics/`), and git (each change loom makes
//! is a commit). The user's own files are never touched.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What a chosen folder is, for the settings to say before anything is written there.
#[derive(Debug, PartialEq, Eq)]
pub enum Folder {
    Missing,
    Empty,
    /// An Obsidian vault (it has `.obsidian/`), with this many notes.
    Vault(usize),
    /// Some other folder, with this many Markdown files.
    Other(usize),
}

/// Counting stops here: a number past it says nothing more.
const COUNT_UP_TO: usize = 10_000;

/// Where the topic files live, in the vault (visible: Obsidian edits them too).
pub const TOPICS: &str = "Topics";

/// loom's own folder in the vault (hidden from Obsidian's file list).
pub const OWN: &str = ".loom";

/// Every note of the vault: (path in it, title, last change in Unix seconds), newest first.
pub fn all_notes(vault: &Path) -> Vec<(String, String, u64)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String, u64)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            // A link is not followed (one to a folder above would never end).
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            // loom's and git's folders, and the topics' own descriptions, are not notes.
            if name.starts_with('.') || (dir == root && name == TOPICS) {
                continue;
            }
            if kind.is_dir() {
                walk(root, &path, out);
            } else if name.ends_with(".md") {
                let Ok(relative) = path.strip_prefix(root) else { continue };
                let when = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                let title = name.trim_end_matches(".md").to_owned();
                out.push((relative.to_string_lossy().into_owned(), title, when));
            }
        }
    }
    let mut out = Vec::new();
    walk(vault, vault, &mut out);
    out.sort_by(|a, b| b.2.cmp(&a.2));
    out
}

/// "3h ago", "2d ago" from Unix seconds.
pub fn ago(when: u64) -> String {
    let seconds = crate::state::now().saturating_sub(when);
    match seconds {
        ..60 => "just now".to_owned(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86_400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

pub fn inspect(path: &Path) -> Folder {
    let Ok(mut entries) = std::fs::read_dir(path) else { return Folder::Missing };
    if entries.next().is_none() {
        return Folder::Empty;
    }
    let notes = notes_in(path).len();
    if path.join(".obsidian").is_dir() { Folder::Vault(notes) } else { Folder::Other(notes) }
}

/// The Markdown files under `path` (hidden folders such as .obsidian, .git, .trash and
/// `.loom` left out), at most `COUNT_UP_TO`.
pub fn notes_in(path: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut todo = vec![path.to_owned()];
    while let Some(dir) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                todo.push(entry.path());
            } else if entry.path().extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
                found.push(entry.path());
                if found.len() >= COUNT_UP_TO {
                    return found;
                }
            }
        }
    }
    found
}

/// The line the settings show under the chosen folder.
pub fn describe(folder: &Folder) -> String {
    let notes = |n: usize| match n {
        1 => "1 note".to_owned(),
        n if n >= COUNT_UP_TO => format!("{COUNT_UP_TO}+ notes"),
        n => format!("{n} notes"),
    };
    match folder {
        Folder::Missing => "This folder doesn't exist (any more).".to_owned(),
        Folder::Empty => "Empty: a new vault is made here.".to_owned(),
        Folder::Vault(n) => format!("An Obsidian vault with {}. loom adds its own files and keeps yours as they are.", notes(*n)),
        Folder::Other(n) => format!("A folder with {}. loom adds what Obsidian needs to open it as a vault.", notes(*n)),
    }
}

/// The vault of the settings, when Notes is on and one is chosen.
pub fn current() -> Option<PathBuf> {
    let s = crate::settings::get();
    (s.notes_enabled && !s.notes_vault.trim().is_empty()).then(|| crate::state::expand(Path::new(s.notes_vault.trim())))
}

/// Sets the vault up where something is missing: `.obsidian/`, `.loom/`, the default topics
/// (once: topics the user deleted stay deleted), and git. Never over the user's files.
pub fn prepare(path: &Path) -> Result<(), String> {
    if let Some(why) = unsuitable(path) {
        return Err(why);
    }
    let fail = |what: &str, err: std::io::Error| format!("{what}: {err}");
    std::fs::create_dir_all(path).map_err(|e| fail("the vault folder", e))?;
    let obsidian = path.join(".obsidian");
    if !obsidian.exists() {
        std::fs::create_dir_all(&obsidian).map_err(|e| fail(".obsidian", e))?;
        std::fs::write(obsidian.join("app.json"), "{}\n").map_err(|e| fail(".obsidian/app.json", e))?;
    }
    let own = path.join(OWN);
    if !own.exists() {
        std::fs::create_dir_all(&own).map_err(|e| fail(OWN, e))?;
        crate::topics::write_defaults(path).map_err(|e| fail("the topics", e))?;
        std::fs::write(own.join("vault.json"), "{ \"version\": 1 }\n").map_err(|e| fail(".loom/vault.json", e))?;
    }
    // Every change loom makes becomes a commit; a vault in no repository gets one of its own.
    // Only loom's own files go in it: the user's notes are theirs to add (or not).
    let in_repo = Command::new("git").arg("-C").arg(path).args(["rev-parse", "--is-inside-work-tree"]).output().is_ok_and(|o| o.status.success());
    if !in_repo {
        let ignore = path.join(".gitignore");
        if !ignore.exists() {
            // What Obsidian rewrites as the window changes: not worth a commit each time.
            let _ = std::fs::write(&ignore, ".obsidian/workspace*.json\n.trash/\n");
        }
        let git = |args: &[&str]| Command::new("git").arg("-C").arg(path).args(args).output().is_ok_and(|o| o.status.success());
        if git(&["init", "-q"]) {
            // Without a git identity the commit fails; the vault still works, uncommitted.
            let own: Vec<&str> = [".gitignore", TOPICS, ".obsidian/app.json"].into_iter().filter(|f| path.join(f).exists()).collect();
            let _ = git(&[&["add", "--"][..], &own].concat()) && git(&[&["commit", "-q", "-m", "loom: the vault is set up", "--"][..], &own].concat());
        }
    }
    Ok(())
}

/// Why `path` can't be a vault (None: it can): the home folder itself, or a system folder.
pub fn unsuitable(path: &Path) -> Option<String> {
    let home = gtk::glib::home_dir();
    let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    if path == home || path == Path::new("/") || home.starts_with(&path) {
        return Some("Your home folder can't be the vault: choose or make a folder inside it.".to_owned());
    }
    if !path.starts_with(&home) && ["/usr", "/etc", "/bin", "/sbin", "/lib", "/boot", "/proc", "/sys", "/dev", "/var", "/opt"].iter().any(|s| path.starts_with(s)) {
        return Some("A system folder can't be the vault.".to_owned());
    }
    None
}

/// A note's frontmatter (`key: value`, `key: [a, b]`, or `key:` with `- item` lines), and the
/// text after it. Enough for loom's own keys and Obsidian's properties; not a YAML parser.
pub fn frontmatter(text: &str) -> (Vec<(String, Value)>, &str) {
    let Some(rest) = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) else { return (Vec::new(), text) };
    let Some(end) = rest.find("\n---").map(|i| i + 1) else { return (Vec::new(), text) };
    let (front, after) = rest.split_at(end);
    let body = after.trim_start_matches('-').trim_start_matches(['\r', '\n']);
    let mut keys: Vec<(String, Value)> = Vec::new();
    for line in front.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            if let Some((_, Value::List(items))) = keys.last_mut() {
                items.push(unquote(item));
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let value = value.trim();
        let parsed = if value.is_empty() {
            Value::List(Vec::new())
        } else if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            Value::List(inner.split(',').map(unquote).filter(|i| !i.is_empty()).collect())
        } else {
            Value::Text(unquote(value))
        };
        keys.push((key.trim().to_owned(), parsed));
    }
    (keys, body)
}

/// `text` with its frontmatter's lines changed by `edit` (a frontmatter made when there is
/// none). Windows line ends and an empty frontmatter ("---\n---\n") are kept as they are.
pub fn edit_frontmatter(text: &str, edit: impl FnOnce(&mut Vec<String>)) -> String {
    let crlf = text.contains("\r\n");
    let text = text.replace("\r\n", "\n");
    let split = text.strip_prefix("---\n").and_then(|rest| {
        if rest.starts_with("---\n") || rest == "---" {
            Some(("", rest))
        } else {
            rest.find("\n---").map(|end| (&rest[..end], &rest[end + 1..]))
        }
    });
    let (mut lines, after) = match split {
        Some((front, after)) => (front.lines().map(str::to_owned).collect::<Vec<_>>(), after.to_owned()),
        None => (Vec::new(), format!("---\n{text}")),
    };
    edit(&mut lines);
    let front = if lines.is_empty() { String::new() } else { format!("{}\n", lines.join("\n")) };
    let out = format!("---\n{front}{after}");
    if crlf { out.replace('\n', "\r\n") } else { out }
}

fn unquote(text: &str) -> String {
    text.trim().trim_matches(|c| c == '"' || c == '\'').to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Text(String),
    List(Vec<String>),
}

impl Value {
    pub fn text(&self) -> String {
        match self {
            Value::Text(text) => text.clone(),
            Value::List(items) => items.join(", "),
        }
    }

    pub fn items(&self) -> Vec<String> {
        match self {
            Value::Text(text) => text.split(',').map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).collect(),
            Value::List(items) => items.clone(),
        }
    }
}

/// The value of `key` in parsed frontmatter.
pub fn get<'a>(keys: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    keys.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("loom-vault-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn folders_are_told_apart() {
        let root = scratch("kinds");
        assert_eq!(inspect(&root), Folder::Missing);
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(inspect(&root), Folder::Empty);
        std::fs::create_dir_all(root.join("Work/.hidden")).unwrap();
        std::fs::write(root.join("Work/a.md"), "").unwrap();
        std::fs::write(root.join("Work/.hidden/b.md"), "").unwrap();
        std::fs::write(root.join("c.txt"), "").unwrap();
        assert_eq!(inspect(&root), Folder::Other(1));
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        assert_eq!(inspect(&root), Folder::Vault(1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn frontmatter_is_read() {
        let (keys, body) = frontmatter("---\ntopic: Finances\nlistens-for: [salary, \"rent\"]\ntags:\n  - money\n  - home\nsync: false\n---\nThe body.\n");
        assert_eq!(get(&keys, "topic"), Some(&Value::Text("Finances".into())));
        assert_eq!(get(&keys, "listens-for").unwrap().items(), vec!["salary", "rent"]);
        assert_eq!(get(&keys, "tags").unwrap().items(), vec!["money", "home"]);
        assert_eq!(get(&keys, "sync").unwrap().text(), "false");
        assert_eq!(body, "The body.\n");
        let (keys, body) = frontmatter("No frontmatter here.");
        assert!(keys.is_empty());
        assert_eq!(body, "No frontmatter here.");
    }

    #[test]
    fn frontmatter_is_edited_in_place() {
        let set = |text: &str| edit_frontmatter(text, |lines| lines.push("k: v".to_owned()));
        assert_eq!(set("---\ndate: 1\n---\nbody\n"), "---\ndate: 1\nk: v\n---\nbody\n");
        assert_eq!(set("---\n---\nbody\n"), "---\nk: v\n---\nbody\n");
        assert_eq!(set("body\n"), "---\nk: v\n---\nbody\n");
        assert_eq!(set("---\r\ndate: 1\r\n---\r\nbody\r\n"), "---\r\ndate: 1\r\nk: v\r\n---\r\nbody\r\n");
    }

    #[test]
    fn unsafe_folders_are_refused() {
        let home = gtk::glib::home_dir();
        assert!(unsuitable(&home).is_some());
        assert!(unsuitable(Path::new("/")).is_some());
        assert!(unsuitable(Path::new("/etc/loom")).is_some());
        assert!(unsuitable(&home.join("Notes")).is_none());
    }

    #[test]
    fn a_new_vault_is_set_up_once() {
        let root = scratch("prepare");
        prepare(&root).unwrap();
        assert!(root.join(".obsidian/app.json").exists());
        assert!(root.join(".loom/vault.json").exists());
        let topics = crate::topics::list(&root);
        assert!(topics.len() >= 6, "{topics:?}");
        // A topic the user deleted is not brought back.
        std::fs::remove_file(&topics[0].path).unwrap();
        prepare(&root).unwrap();
        assert_eq!(crate::topics::list(&root).len(), topics.len() - 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
