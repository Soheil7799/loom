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

/// A unified diff's hunks: each its `@@` line and its lines (with their `+`, `-` or ` `).
/// git's remarks ("\\ No newline at end of file") are left out.
pub fn hunks(diff: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in diff.lines() {
        if line.starts_with("@@") {
            out.push((line.to_owned(), Vec::new()));
        } else if let Some((_, lines)) = out.last_mut()
            && (line.starts_with(' ') || line.starts_with('+') || line.starts_with('-'))
        {
            lines.push(line.to_owned());
        }
    }
    out
}

/// A whole repository's diff cut into its files: (path, that file's diff), in order.
pub fn by_file(diff: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            out.push((String::new(), String::new()));
        }
        let Some((path, text)) = out.last_mut() else { continue };
        // The new name, or the old one for a file deleted (+++ /dev/null); git ends a name
        // with spaces in it with a tab. A change with no text lines (a binary file, a rename,
        // a mode) has only its "diff --git a/… b/…" line: the name after " b/".
        let name = line.strip_prefix("+++ b/").or_else(|| line.strip_prefix("--- a/").filter(|_| path.is_empty()));
        if let Some(name) = name.filter(|_| !text.contains("\n@@")) {
            *path = name.trim_end_matches(['\n', '\t']).to_owned();
        } else if let Some(names) = line.strip_prefix("diff --git a/").filter(|_| path.is_empty())
            && let Some((_, b)) = names.trim_end().rsplit_once(" b/")
        {
            *path = b.to_owned();
        }
        text.push_str(line);
    }
    out.retain(|(path, _)| !path.is_empty());
    out
}

/// Each line of a hunk (as `hunks` gives them) with its number in the file: the new file's
/// for a line kept or added, the old one's for a line taken out ("12", "was 12").
pub fn numbers(head: &str, lines: &[String]) -> Vec<String> {
    // "@@ -old,count +new,count @@"
    let start = |sign: char| {
        head.split_whitespace().find_map(|w| w.strip_prefix(sign)).and_then(|w| w.split(',').next()?.parse::<usize>().ok()).unwrap_or(1)
    };
    let (mut old, mut new) = (start('-'), start('+'));
    lines
        .iter()
        .map(|line| match line.chars().next() {
            Some('-') => {
                old += 1;
                format!("was {}", old - 1)
            }
            Some('+') => {
                new += 1;
                (new - 1).to_string()
            }
            _ => {
                old += 1;
                new += 1;
                (new - 1).to_string()
            }
        })
        .collect()
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

/// Goes back on `path`'s changes since the last commit: all of them, or hunk `(n, shown)` of
/// its diff: the `n`th (from 0), only if it still reads `shown` (from its `@@` line, as
/// `describe` gave it: the file may have changed since). A file git doesn't know (new) is not
/// reverted here.
pub fn revert(path: &Path, hunk: Option<(usize, &str)>) -> Result<(), String> {
    let repo = crate::repos::repo_of(path).ok_or("it isn't in a git repository")?;
    let file = path.strip_prefix(&repo).unwrap_or(path).to_string_lossy().into_owned();
    if git(&repo, &["ls-files", "--error-unmatch", "--", &file], None).is_err() {
        return Err("git doesn't know it yet (a new file): delete it instead".into());
    }
    match hunk {
        None => git(&repo, &["restore", "--staged", "--worktree", "--source=HEAD", "--", &file], None).map(drop),
        Some((n, shown)) => {
            // HEAD to the work tree, as `describe` showed it: undone in the work tree only
            let diff = git(&repo, &["diff", "HEAD", "--", &file], None)?;
            let patch = hunk_patch(&diff, n).ok_or("no such change (the file changed since?)")?;
            let now = &patch[patch.find("\n@@").map_or(0, |i| i + 1)..];
            // "\ No newline at end of file" is git's remark, not a line: a page may leave it out
            let lines = |t: &str| t.lines().filter(|l| !l.starts_with('\\')).map(str::to_owned).collect::<Vec<_>>();
            if lines(now) != lines(shown) {
                return Err("the file changed since it was shown: refresh, then undo again".into());
            }
            git(&repo, &["apply", "-R", "-"], Some(&patch))?;
            // staged too: out of the index as well, or the next commit would still have it
            // (not staged: the index never had it, and this does nothing)
            let _ = git(&repo, &["apply", "-R", "--cached", "-"], Some(&patch));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_diff_cuts_into_files_with_their_line_numbers() {
        let diff = "diff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -3,3 +3,3 @@ fn x\n keep\n-old\n+new\n keep\ndiff --git a/gone.rs b/gone.rs\ndeleted file mode 100644\n--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n";
        let files = by_file(diff);
        assert_eq!(files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["a.rs", "gone.rs"]);
        assert!(files[0].1.ends_with(" keep\n") && !files[0].1.contains("gone"));
        let (head, lines) = hunks(&files[0].1).remove(0);
        assert_eq!(numbers(&head, &lines), ["3", "was 4", "4", "5"]);
        // a name with a space (git ends it with a tab); a binary file, no text lines
        let diff = "diff --git a/my notes.md b/my notes.md\n--- a/my notes.md\t\n+++ b/my notes.md\t\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n";
        let files = by_file(diff);
        assert_eq!(files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["my notes.md", "logo.png"]);
    }

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
        let second = &described["diff"].as_str().unwrap()[described["diff"].as_str().unwrap().rfind("\n@@").unwrap() + 1..];
        // the hunks as a window shows them: the second one's text is what revert checks
        let parts = hunks(described["diff"].as_str().unwrap());
        assert_eq!(parts.len(), 2);
        assert_eq!(std::iter::once(parts[1].0.clone()).chain(parts[1].1.clone()).collect::<Vec<_>>().join("\n"), second.trim_end());
        assert!(revert(&file, Some((0, second))).is_err(), "not the hunk shown there");
        revert(&file, Some((1, second))).unwrap();
        let now = std::fs::read_to_string(&file).unwrap();
        assert!(now.contains("two\n") && now.contains("line 28\n"), "{now}");
        // a staged change undone goes from the index too
        run(&["add", "a.txt"]);
        let staged = describe(&file, Mark::Changed);
        let first = &staged["diff"].as_str().unwrap()[staged["diff"].as_str().unwrap().find("\n@@").unwrap() + 1..];
        revert(&file, Some((0, first))).unwrap();
        let index = Command::new("git").arg("-C").arg(&dir).args(["show", ":a.txt"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&index.stdout), before, "nothing of it left staged");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
        std::fs::write(&file, before.replace("line 2\n", "two\n")).unwrap();
        revert(&file, None).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
        assert!(revert(&file, Some((0, second))).is_err(), "nothing left to revert");
        std::fs::write(dir.join("new.txt"), "x").unwrap();
        assert!(revert(&dir.join("new.txt"), None).is_err(), "a new file isn't reverted");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
