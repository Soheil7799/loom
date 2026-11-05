//! An agent's changes for the core API: the files it created or changed, each with git's
//! word on it and its diff against the last commit (`changes`), and going back on one, a
//! whole file or one hunk of its diff (`revert`).

use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

use crate::agent::Mark;

/// git in `repo`, as `repos::status` runs it (no optional locks, names as they are).
fn git(repo: &Path, args: &[&str], input: Option<&str>) -> Result<String, String> {
    use std::io::Write;
    let mut command = Command::new("git");
    command.args(["--no-optional-locks", "-c", "core.quotePath=false", "-C"]).arg(repo).args(args);
    command.stdin(if input.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() });
    command.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|e| format!("git: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(text.as_bytes()).map_err(|e| format!("git: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

/// One file of the agent's: `{path, mark, repo, status, diff}`. `status`: git's two letters
/// (`??` new, ` M` changed…; empty: as committed); `diff`: against the last commit (none
/// outside a repository, or for a new file: its text is all of it).
pub fn describe(path: &Path, mark: Mark) -> Value {
    let mark = match mark {
        Mark::Created => "created",
        Mark::Changed => "changed",
    };
    let Some(repo) = crate::repos::repo_of(path) else {
        return json!({ "path": path, "mark": mark, "repo": null, "status": null, "diff": null });
    };
    let file = path.strip_prefix(&repo).unwrap_or(path).to_string_lossy().into_owned();
    let status = git(&repo, &["status", "--porcelain=v1", "--", &file], None).ok().and_then(|s| s.lines().next().map(|l| l[..2.min(l.len())].to_owned())).unwrap_or_default();
    let diff = git(&repo, &["diff", "HEAD", "--", &file], None).ok().filter(|d| !d.is_empty());
    json!({ "path": path, "mark": mark, "repo": repo, "status": status, "diff": diff })
}

/// Hunk `n` (from 0) of `diff` as a patch of its own: the file's header, then that hunk.
fn hunk_patch(diff: &str, n: usize) -> Option<String> {
    let lines: Vec<&str> = diff.split_inclusive('\n').collect();
    let first = lines.iter().position(|l| l.starts_with("@@"))?;
    let starts: Vec<usize> = (first..lines.len()).filter(|&i| lines[i].starts_with("@@")).collect();
    let start = *starts.get(n)?;
    let end = starts.get(n + 1).copied().unwrap_or(lines.len());
    Some(lines[..first].concat() + &lines[start..end].concat())
}

/// Goes back on `path`'s changes since the last commit: all of them, or hunk `hunk` of its
/// diff (as `describe` gave it). A file git doesn't know (new) is not reverted here.
pub fn revert(path: &Path, hunk: Option<usize>) -> Result<(), String> {
    let repo = crate::repos::repo_of(path).ok_or("it isn't in a git repository")?;
    let file = path.strip_prefix(&repo).unwrap_or(path).to_string_lossy().into_owned();
    if git(&repo, &["ls-files", "--error-unmatch", "--", &file], None).is_err() {
        return Err("git doesn't know it yet (a new file): delete it instead".into());
    }
    match hunk {
        None => git(&repo, &["restore", "--staged", "--worktree", "--source=HEAD", "--", &file], None).map(drop),
        Some(n) => {
            // HEAD to the work tree, as `describe` showed it: undone in the work tree only
            let diff = git(&repo, &["diff", "HEAD", "--", &file], None)?;
            let patch = hunk_patch(&diff, n).ok_or("no such change (the file changed since?)")?;
            git(&repo, &["apply", "-R", "-"], Some(&patch)).map(drop)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hunk_goes_back_alone_and_a_whole_file_too() {
        let dir = std::env::temp_dir().join(format!("loom-changes-{}", crate::rt::uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap().status.success(), "{args:?}");
        run(&["init", "-q"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "0"]);
        let file = dir.join("a.txt");
        let before: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&file, &before).unwrap();
        run(&["add", "a.txt"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "1"]);
        // two changes far apart: two hunks
        std::fs::write(&file, before.replace("line 2\n", "two\n").replace("line 28\n", "twenty-eight\n")).unwrap();
        let described = describe(&file, Mark::Changed);
        assert_eq!(described["status"], " M");
        assert_eq!(described["diff"].as_str().unwrap().matches("\n@@").count(), 2);
        revert(&file, Some(1)).unwrap();
        let now = std::fs::read_to_string(&file).unwrap();
        assert!(now.contains("two\n") && now.contains("line 28\n"), "{now}");
        revert(&file, None).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
        assert!(revert(&file, Some(0)).is_err(), "nothing left to revert");
        std::fs::write(dir.join("new.txt"), "x").unwrap();
        assert!(revert(&dir.join("new.txt"), None).is_err(), "a new file isn't reverted");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
