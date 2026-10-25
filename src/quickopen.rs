//! The project's files, for Ctrl+P (picked with picker.rs) and the files tree.

use std::path::Path;
use std::process::Command;

/// Directories never worth listing.
pub const SKIP: &[&str] = &[".git", "target", "node_modules", "__pycache__", ".venv", ".cache", ".next", "dist"];
const MAX_FILES: usize = 20_000;

/// The project's files, relative: what git tracks (and untracked, unignored), else a walk.
pub fn list_files(root: &Path) -> Vec<String> {
    let mut files = git_files(root).unwrap_or_else(|| {
        // Not a repo (a folder above several): each repo's own listing (its .gitignore
        // kept), plus what lies outside them.
        let repos: Vec<std::path::PathBuf> = crate::repos::under(root).into_iter().filter(|r| r != root).collect();
        let mut files = Vec::new();
        for repo in &repos {
            let Some(prefix) = repo.strip_prefix(root).ok().map(|p| p.to_string_lossy().into_owned()) else { continue };
            for file in git_files(repo).unwrap_or_default() {
                files.push(format!("{prefix}/{file}"));
            }
        }
        let mut outside = Vec::new();
        walk(root, root, 0, &mut outside);
        files.extend(outside.into_iter().filter(|f| !repos.iter().any(|r| root.join(f).starts_with(r))));
        files
    });
    files.truncate(MAX_FILES);
    // Folders too ("src/"), for mentioning a whole one.
    let mut folders: Vec<String> = files
        .iter()
        .flat_map(|f| {
            let parts: Vec<&str> = f.split('/').collect();
            (1..parts.len()).map(move |n| format!("{}/", parts[..n].join("/")))
        })
        .collect();
    folders.sort();
    folders.dedup();
    files.extend(folders);
    files
}

fn git_files(root: &Path) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(["-c", "core.quotePath=false", "ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&output.stdout).lines().take(MAX_FILES).map(str::to_owned).collect())
}

fn walk(root: &Path, dir: &Path, depth: usize, files: &mut Vec<String>) {
    if depth > 8 || files.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {
                if !SKIP.contains(&name.as_str()) {
                    walk(root, &path, depth + 1, files);
                }
            }
            Ok(_) => {
                if let Ok(relative) = path.strip_prefix(root) {
                    files.push(relative.to_string_lossy().into_owned());
                }
            }
            Err(_) => {}
        }
    }
}
