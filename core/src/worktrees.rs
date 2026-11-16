//! An agent's own git worktree: a branch of the project checked out beside it
//! (`<parent>/<project>.worktrees/<branch>`), so agents working at once don't edit the same
//! files. It belongs to its project (repos::project_of): its agents are on the project's
//! card. Merged back into what the project has checked out, then removed; neither is ever
//! forced (a conflict is undone, an unmerged branch kept). These run git: off the UI's thread.

use std::path::{Path, PathBuf};

use crate::files::git;

/// The main checkout `dir` is a worktree of (its `.git` is a file pointing into
/// `<main>/.git/worktrees/`); None for a main checkout or a submodule.
pub fn main_of(dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dir.join(".git")).ok()?;
    let gitdir = PathBuf::from(text.strip_prefix("gitdir:")?.trim());
    let worktrees = gitdir.parent()?;
    (worktrees.file_name()? == "worktrees" && worktrees.parent()?.file_name()? == ".git").then(|| worktrees.parent()?.parent().map(Path::to_path_buf)).flatten()
}

/// The project `dir` is a worktree of, if loom made it (it is in `<project>.worktrees/`).
pub fn made_from(dir: &Path) -> Option<PathBuf> {
    let main = main_of(dir)?;
    let group = dir.parent()?.file_name()?.to_string_lossy().into_owned();
    (group == format!("{}.worktrees", main.file_name()?.to_string_lossy())).then_some(main)
}

/// The branch checked out in `dir`.
pub fn branch_of(dir: &Path) -> Option<String> {
    git(dir, &["branch", "--show-current"]).ok().map(|b| b.trim().to_owned()).filter(|b| !b.is_empty())
}

/// A new branch `branch` from what `project` has checked out, in a worktree of its own.
pub fn add(project: &Path, branch: &str) -> Result<PathBuf, String> {
    if branch.is_empty() || branch.starts_with('-') || branch.contains(char::is_whitespace) || git(project, &["check-ref-format", "--branch", branch]).is_err() {
        return Err(format!("\"{branch}\" can't be a branch's name"));
    }
    let name = project.file_name().ok_or("the project has no folder name")?.to_string_lossy().into_owned();
    let dir = project.parent().ok_or("the project has no parent folder")?.join(format!("{name}.worktrees")).join(branch.replace('/', "-"));
    if dir.exists() {
        return Err(format!("{} is there already", dir.display()));
    }
    git(project, &["worktree", "add", "-b", branch, &dir.to_string_lossy()])?;
    Ok(dir)
}

/// `dir`'s branch merged into what its project has checked out (a merge commit). A conflict
/// is undone (`merge --abort`) and said.
pub fn merge(dir: &Path) -> Result<String, String> {
    let project = main_of(dir).ok_or("not a worktree")?;
    let branch = branch_of(dir).ok_or("its worktree has no branch checked out")?;
    if !git(dir, &["status", "--porcelain"])?.trim().is_empty() {
        return Err(format!("{branch} has changes not committed yet: commit them first"));
    }
    let base = branch_of(&project).unwrap_or_else(|| "the project".into());
    match git(&project, &["merge", "--no-ff", "--no-edit", &branch]) {
        Ok(_) => Ok(format!("{branch} merged into {base}")),
        Err(err) => {
            let _ = git(&project, &["merge", "--abort"]);
            // (the window reads "didn't merge into <base>" to offer the agent the conflict)
            Err(format!("{branch} didn't merge into {base} (nothing changed): {err}"))
        }
    }
}

/// `dir` removed, and its branch with it once merged (an unmerged one is kept, and said).
/// Refused while it has changes not committed.
pub fn remove(dir: &Path) -> Result<String, String> {
    let project = main_of(dir).ok_or("not a worktree")?;
    let branch = branch_of(dir);
    git(&project, &["worktree", "remove", &dir.to_string_lossy()])?;
    let Some(branch) = branch else { return Ok("Its worktree is removed.".into()) };
    Ok(match git(&project, &["branch", "-d", &branch]) {
        Ok(_) => format!("Its worktree and {branch} are removed."),
        Err(_) => format!("Its worktree is removed; {branch} is kept (not merged)."),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        let root = std::env::temp_dir().join(format!("loom-worktrees-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("app");
        std::fs::create_dir_all(&project).unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "start"]] {
            git(&project, args).unwrap();
        }
        project
    }

    #[test]
    fn a_worktree_is_its_projects_merged_back_then_removed() {
        let project = repo();
        let dir = add(&project, "fix/tooltip").unwrap();
        assert_eq!(dir, project.parent().unwrap().join("app.worktrees/fix-tooltip"));
        assert_eq!(main_of(&dir).as_deref(), Some(project.as_path()));
        assert_eq!(crate::repos::project_of(&dir.join("src")), project, "on its project's card");
        assert_eq!(main_of(&project), None);
        // one made by hand elsewhere stays a project of its own
        let other = project.parent().unwrap().join("by-hand");
        git(&project, &["worktree", "add", "-q", "-b", "hand", &other.to_string_lossy()]).unwrap();
        assert_eq!((main_of(&other).as_deref(), crate::repos::project_of(&other)), (Some(project.as_path()), other.clone()));
        assert!(add(&project, "--force").is_err());
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        assert!(merge(&dir).unwrap_err().contains("not committed"));
        git(&dir, &["add", "a.txt"]).unwrap();
        git(&dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "a"]).unwrap();
        // (the merge commit needs a name too)
        git(&project, &["config", "user.name", "t"]).unwrap();
        git(&project, &["config", "user.email", "t@t"]).unwrap();
        assert_eq!(merge(&dir).unwrap(), "fix/tooltip merged into main");
        assert!(project.join("a.txt").exists());
        assert_eq!(remove(&dir).unwrap(), "Its worktree and fix/tooltip are removed.");
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(project.parent().unwrap());
    }
}
