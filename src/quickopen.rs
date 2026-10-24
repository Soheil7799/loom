//! The project's files, for Ctrl+P (picked with picker.rs) and the files tree.

use std::path::Path;
use std::process::Command;

/// Directories never worth listing.
pub const SKIP: &[&str] = &[".git", "target", "node_modules", "__pycache__", ".venv", ".cache", ".next", "dist"];
const MAX_FILES: usize = 20_000;

/// The project's files, relative: what git tracks (and untracked, unignored), else a walk.
pub fn list_files(root: &Path) -> Vec<String> {
    let git = Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(root)
        .output();
    if let Ok(output) = git
        && output.status.success()
    {
        return String::from_utf8_lossy(&output.stdout).lines().take(MAX_FILES).map(str::to_owned).collect();
    }
    let mut files = Vec::new();
    walk(root, root, 0, &mut files);
    files
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
