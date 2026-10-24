//! Git repositories inside a project. An agent often runs in a folder above several repos
//! (for the shared context and CLAUDE.md) and works in some of them: loom notices which, and
//! shows each one's branch and changes.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub struct Repo {
    pub path: PathBuf,
    pub branch: String,
    /// Changed files (staged, unstaged, untracked).
    pub changes: Vec<(String, String)>,
    /// Commits ahead of / behind the upstream.
    pub ahead: u32,
    pub behind: u32,
}

impl Repo {
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// "main ●3 ↑1", for the sidebar.
    pub fn summary(&self) -> String {
        let mut text = self.branch.clone();
        if !self.changes.is_empty() {
            text.push_str(&format!(" ●{}", self.changes.len()));
        }
        if self.ahead > 0 {
            text.push_str(&format!(" ↑{}", self.ahead));
        }
        if self.behind > 0 {
            text.push_str(&format!(" ↓{}", self.behind));
        }
        text
    }
}

/// The repository `path` is in (its top folder), if any.
pub fn repo_of(path: &Path) -> Option<PathBuf> {
    let start = if path.is_dir() { path } else { path.parent()? };
    start.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

/// Branch, changed files and upstream distance (None if git can't read it).
pub fn status(repo: &Path) -> Option<Repo> {
    // No optional locks (an agent's own `git commit` must not find index.lock taken), and
    // file names as they are (Persian ones would come out as octal escapes otherwise).
    let output = Command::new("git")
        .args(["--no-optional-locks", "-c", "core.quotePath=false", "-C"])
        .arg(repo)
        .args(["status", "--porcelain=v1", "--branch", "--untracked-files=normal"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(parse_status(repo, &String::from_utf8_lossy(&output.stdout)))
}

fn parse_status(repo: &Path, text: &str) -> Repo {
    let mut repo = Repo { path: repo.to_owned(), branch: String::new(), changes: Vec::new(), ahead: 0, behind: 0 };
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            // "main...origin/main [ahead 1, behind 2]", "No commits yet on main", "HEAD (no branch)"
            let head = head.strip_prefix("No commits yet on ").unwrap_or(head);
            let (names, tracking) = head.split_once(" [").unwrap_or((head, ""));
            repo.branch = names.split("...").next().unwrap_or(names).to_owned();
            for part in tracking.trim_end_matches(']').split(", ") {
                if let Some(n) = part.strip_prefix("ahead ") {
                    repo.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("behind ") {
                    repo.behind = n.parse().unwrap_or(0);
                }
            }
        } else if line.len() > 3 {
            let (code, path) = line.split_at(3);
            // A rename shows "old -> new": the new name is the file now.
            let path = path.rsplit(" -> ").next().unwrap_or(path).trim_matches('"');
            repo.changes.push((code.trim().to_owned(), path.to_owned()));
        }
    }
    repo
}

/// Repositories at or under `dir`, a few levels down (not inside one another's dependencies).
pub fn under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if dir.join(".git").exists() {
        found.push(dir.to_owned());
    }
    let mut visited = 0;
    walk(dir, 0, &mut found, &mut visited);
    found
}

/// At most 3 levels down, 60 repos, 2000 folders looked at (a project at ~ stays quick).
fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>, visited: &mut usize) {
    *visited += 1;
    if depth >= 3 || found.len() >= 60 || *visited > 2000 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && !crate::quickopen::SKIP.contains(&name.as_ref())
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for sub in dirs {
        if sub.join(".git").exists() {
            found.push(sub.clone());
        }
        walk(&sub, depth + 1, found, visited);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines_are_read() {
        let repo = parse_status(
            Path::new("/r"),
            "## main...origin/main [ahead 2, behind 1]\n M src/a.rs\n?? new.txt\nR  old.rs -> moved.rs\n",
        );
        assert_eq!(repo.branch, "main");
        assert_eq!((repo.ahead, repo.behind), (2, 1));
        assert_eq!(
            repo.changes,
            vec![("M".into(), "src/a.rs".into()), ("??".into(), "new.txt".into()), ("R".into(), "moved.rs".into())]
        );
        assert_eq!(repo.summary(), "main ●3 ↑2 ↓1");
    }

    #[test]
    fn a_fresh_repo_has_a_branch_name() {
        let repo = parse_status(Path::new("/r"), "## No commits yet on main\n");
        assert_eq!(repo.branch, "main");
        assert_eq!(repo.summary(), "main");
    }
}
