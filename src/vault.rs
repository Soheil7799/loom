//! The Notes add-on's vault: a folder Obsidian can open, which loom keeps notes in.

use std::path::Path;

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

pub fn inspect(path: &Path) -> Folder {
    let Ok(mut entries) = std::fs::read_dir(path) else { return Folder::Missing };
    if entries.next().is_none() {
        return Folder::Empty;
    }
    let notes = count_notes(path);
    if path.join(".obsidian").is_dir() { Folder::Vault(notes) } else { Folder::Other(notes) }
}

/// Markdown files under `path`, hidden folders (.obsidian, .git, .trash…) left out.
fn count_notes(path: &Path) -> usize {
    let mut count = 0;
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
                count += 1;
                if count >= COUNT_UP_TO {
                    return count;
                }
            }
        }
    }
    count
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_told_apart() {
        let root = std::env::temp_dir().join(format!("loom-vault-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
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
}
