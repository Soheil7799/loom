//! Files and folders as the editor and the vault use them: git asked about a repo, names that
//! never clash, folders copied whole, the vault's own trash.

use std::path::{Path, PathBuf};

/// Runs git in `repo`: its output, or what it said went wrong.
pub fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git").arg("-C").arg(repo).args(args).output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// `wanted`, or "name (2).ext", "name (3).ext"… if it is taken.
pub fn free_name(wanted: &Path) -> PathBuf {
    if !wanted.exists() {
        return wanted.to_owned();
    }
    let stem = wanted.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = wanted.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..1000).map(|n| wanted.with_file_name(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap_or_else(|| wanted.to_owned())
}

/// A file, or a folder with all it holds.
pub fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// The files and folders git ignores in `project` (none if it is no repository).
pub fn ignored_in(project: &Path) -> std::collections::HashSet<PathBuf> {
    let Ok(out) = std::process::Command::new("git").arg("-C").arg(project).args(["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"]).output() else {
        return Default::default();
    };
    if !out.status.success() {
        return Default::default();
    }
    String::from_utf8_lossy(&out.stdout).split('\0').filter(|p| !p.is_empty()).map(|p| project.join(p.trim_end_matches('/'))).collect()
}

/// A commit's refs as badges: (name, kind) with kind head / branch / remote / tag.
pub fn ref_badges(refs: &str, remotes: &[String]) -> Vec<(String, &'static str)> {
    let mut badges = Vec::new();
    for part in refs.split(", ").map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(branch) = part.strip_prefix("HEAD -> ") {
            badges.push(("HEAD".to_owned(), "head"));
            badges.push((branch.to_owned(), "branch"));
        } else if part == "HEAD" {
            badges.push(("HEAD (detached)".to_owned(), "head"));
        } else if let Some(tag) = part.strip_prefix("tag: ") {
            badges.push((tag.to_owned(), "tag"));
        } else if remotes.iter().any(|r| part.starts_with(&format!("{r}/"))) {
            badges.push((part.to_owned(), "remote"));
        } else {
            badges.push((part.to_owned(), "branch"));
        }
    }
    badges
}

/// Moves a file of the vault into its `.trash/` (where Obsidian puts what it deletes), under a
/// name not taken there.
pub fn to_trash(vault: &Path, file: &Path) -> std::io::Result<PathBuf> {
    let trash = vault.join(".trash");
    std::fs::create_dir_all(&trash)?;
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = file.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let place = (1..).map(|i| trash.join(if i == 1 { format!("{stem}{extension}") } else { format!("{stem} ({i}){extension}") })).find(|p| !p.exists()).unwrap_or_default();
    std::fs::rename(file, &place)?;
    Ok(place)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_read_as_badges() {
        let remotes = vec!["origin".to_owned()];
        assert_eq!(
            ref_badges("HEAD -> main, origin/main, tag: v1.0, feature/x", &remotes),
            vec![
                ("HEAD".to_owned(), "head"),
                ("main".to_owned(), "branch"),
                ("origin/main".to_owned(), "remote"),
                ("v1.0".to_owned(), "tag"),
                ("feature/x".to_owned(), "branch"),
            ]
        );
    }

    #[test]
    fn a_dropped_folder_is_copied_whole_and_names_never_clash() {
        let base = std::env::temp_dir().join(format!("loom-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src/inner")).unwrap();
        std::fs::write(base.join("src/a.txt"), "a").unwrap();
        std::fs::write(base.join("src/inner/b.txt"), "b").unwrap();
        std::fs::create_dir_all(base.join("into")).unwrap();
        copy_all(&base.join("src"), &free_name(&base.join("into/src"))).unwrap();
        assert_eq!(std::fs::read_to_string(base.join("into/src/inner/b.txt")).unwrap(), "b");
        // Again: a second copy beside the first, not over it.
        let second = free_name(&base.join("into/src"));
        assert_eq!(second, base.join("into/src (2)"));
        std::fs::write(base.join("into/note.md"), "x").unwrap();
        assert_eq!(free_name(&base.join("into/note.md")), base.join("into/note (2).md"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
