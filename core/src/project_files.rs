//! A project's files and git in the core API, for loom's window and the CLI (the files panel):
//! only inside a project of the sidebar's (`projects`), each command off the core's thread.
//!
//! - `files_list {folder}` → `{entries: [{name, path, dir, size, modified}]}`, folders first
//! - `files_find {project, query, limit?}` → `{files: [relative path]}`, best first
//! - `file_read {path}` → `{text, modified}` (`text` null: not text, or over 2 MB: `size`)
//! - `file_write {path, text, modified?}` → `{modified}`: a conflict if it changed on disk since
//!   `modified` (the time `file_read` gave); a new file when there was none
//! - `git_repos {project}` → `{repos: [{path, name, branch, ahead, behind, changes: [{code, file}]}]}`
//! - `git_diff {repo, file, staged?}` → `{diff}`
//! - `git_do {repo, act: stage|unstage|discard|commit, files?, message?}` → `{}`
//! - `git_log {repo, limit?}` → `{commits: [{hash, refs, subject, author, when}]}`

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::api::{Error, Principal};
use crate::files::git;

/// What `file_read` gives as text, at most.
const READ: u64 = 2 << 20;

/// The sidebar's projects (their folders), read on the core's thread.
fn projects() -> Vec<PathBuf> {
    crate::api::hub().store("projects").items().iter().filter_map(|p| p["id"].as_str().map(PathBuf::from)).collect()
}

/// `path` (absolute), if it lies in one of `projects`: no `..` past one, no symlink out of it.
fn inside(path: &str, projects: &[PathBuf]) -> Result<PathBuf, Error> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(Error::Invalid(format!("{} is not an absolute path", path.display())));
    }
    // A file not there yet: its folder must be.
    let real = match path.canonicalize() {
        Ok(real) => real,
        Err(_) => path.parent().and_then(|p| p.canonicalize().ok()).map(|p| p.join(path.file_name().unwrap_or_default())).ok_or_else(|| Error::NotFound(path.display().to_string()))?,
    };
    let within = projects.iter().filter_map(|p| p.canonicalize().ok()).any(|p| real.starts_with(&p));
    within.then_some(real).ok_or_else(|| Error::Forbidden("only files inside a project of the sidebar's".into()))
}

fn string<'a>(params: &'a Value, key: &str) -> Result<&'a str, Error> {
    params[key].as_str().ok_or_else(|| Error::Invalid(format!("which {key}?")))
}

fn modified(meta: &std::fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

fn list(folder: &Path) -> Result<Value, Error> {
    let read = std::fs::read_dir(folder).map_err(|e| Error::NotFound(format!("{}: {e}", folder.display())))?;
    let mut entries: Vec<Value> = read
        .flatten()
        .filter(|e| e.file_name() != ".git")
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some(json!({ "name": e.file_name().to_string_lossy(), "path": e.path(), "dir": meta.is_dir(), "size": meta.len(), "modified": modified(&meta) }))
        })
        .collect();
    entries.sort_by_key(|e| (!e["dir"].as_bool().unwrap_or(false), e["name"].as_str().unwrap_or("").to_lowercase()));
    Ok(json!({ "entries": entries }))
}

/// How well `file` matches `query`: its letters in order (case aside), better in the name and
/// when short. None: no match.
fn score(file: &str, query: &str) -> Option<usize> {
    let lower = file.to_lowercase();
    let mut at = 0;
    for c in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        at += lower[at..].find(c)? + c.len_utf8();
    }
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let bonus = if name.contains(&query.to_lowercase()) { 0 } else { 1000 };
    Some(bonus + file.len())
}

fn find(project: &Path, query: &str, limit: usize) -> Value {
    let mut found: Vec<(usize, String)> = crate::quickopen::list_files(project).into_iter().filter(|f| !f.ends_with('/')).filter_map(|f| score(&f, query).map(|s| (s, f))).collect();
    found.sort();
    json!({ "files": found.into_iter().take(limit).map(|(_, f)| f).collect::<Vec<_>>() })
}

fn read(path: &Path) -> Result<Value, Error> {
    let meta = std::fs::metadata(path).map_err(|e| Error::NotFound(format!("{}: {e}", path.display())))?;
    if meta.is_dir() {
        return Err(Error::Invalid("a folder: files_list it".into()));
    }
    if meta.len() > READ {
        return Ok(json!({ "text": null, "size": meta.len(), "modified": modified(&meta) }));
    }
    let bytes = std::fs::read(path).map_err(|e| Error::Failed(e.to_string()))?;
    Ok(json!({ "text": String::from_utf8(bytes).ok(), "size": meta.len(), "modified": modified(&meta) }))
}

fn write(path: &Path, text: &str, seen: Option<u64>) -> Result<Value, Error> {
    if let (Ok(meta), Some(seen)) = (std::fs::metadata(path), seen)
        && modified(&meta) != seen
    {
        return Err(Error::Conflict("it changed on disk since it was opened: open it again, or save once more to overwrite".into()));
    }
    std::fs::write(path, text).map_err(|e| Error::Failed(e.to_string()))?;
    Ok(json!({ "modified": std::fs::metadata(path).map(|m| modified(&m)).unwrap_or(0) }))
}

fn repos(project: &Path) -> Value {
    let repos: Vec<Value> = crate::repos::under(project)
        .iter()
        .filter_map(|r| crate::repos::status(r))
        .map(|r| {
            let changes: Vec<Value> = r.changes.iter().map(|(code, file)| json!({ "code": code, "file": file })).collect();
            json!({ "path": r.path, "name": r.name(), "branch": r.branch, "ahead": r.ahead, "behind": r.behind, "changes": changes })
        })
        .collect();
    json!({ "repos": repos })
}

fn act(repo: &Path, act: &str, files: &[String], message: &str) -> Result<Value, Error> {
    let with = |head: &[&str]| -> Vec<String> { head.iter().map(|s| s.to_string()).chain(std::iter::once("--".into())).chain(files.iter().cloned()).collect() };
    let args: Vec<String> = match act {
        _ if act != "commit" && files.is_empty() => return Err(Error::Invalid("which files?".into())),
        "stage" => with(&["add"]),
        "unstage" => with(&["restore", "--staged"]),
        "discard" => with(&["restore", "--staged", "--worktree", "--source=HEAD"]),
        "commit" if message.trim().is_empty() => return Err(Error::Invalid("a commit needs a message".into())),
        "commit" => vec!["commit".into(), "-m".into(), message.trim().into()],
        _ => return Err(Error::Invalid("act: stage, unstage, discard or commit".into())),
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    git(repo, &args).map(|_| json!({})).map_err(Error::Failed)
}

fn log(repo: &Path, limit: usize) -> Result<Value, Error> {
    let n = limit.to_string();
    let out = git(repo, &["-c", "core.quotePath=false", "log", "--date-order", "-n", &n, "--format=%h%x1f%D%x1f%s%x1f%an%x1f%ar"]).map_err(Error::Failed)?;
    let commits: Vec<Value> = out
        .lines()
        .map(|l| {
            let f: Vec<&str> = l.split('\u{1f}').collect();
            json!({ "hash": f.first(), "refs": f.get(1), "subject": f.get(2), "author": f.get(3), "when": f.get(4) })
        })
        .collect();
    Ok(json!({ "commits": commits }))
}

/// Registers the commands (once, at start).
pub fn serve() {
    let hub = crate::api::hub();
    type Job = Box<dyn FnOnce() -> Result<Value, Error> + Send>;
    // Checked here (who, which project), done off the core's thread.
    let commands: &[(&str, fn(&Value, &[PathBuf]) -> Result<Job, Error>)] = &[
        ("files_list", |p, ps| {
            let folder = inside(string(p, "folder")?, ps)?;
            Ok(Box::new(move || list(&folder)))
        }),
        ("files_find", |p, ps| {
            let project = inside(string(p, "project")?, ps)?;
            let (query, limit) = (string(p, "query")?.to_owned(), p["limit"].as_u64().unwrap_or(50).min(500) as usize);
            Ok(Box::new(move || Ok(find(&project, &query, limit))))
        }),
        ("file_read", |p, ps| {
            let path = inside(string(p, "path")?, ps)?;
            Ok(Box::new(move || read(&path)))
        }),
        ("file_write", |p, ps| {
            let path = inside(string(p, "path")?, ps)?;
            let (text, seen) = (string(p, "text")?.to_owned(), p["modified"].as_u64());
            Ok(Box::new(move || write(&path, &text, seen)))
        }),
        ("git_repos", |p, ps| {
            let project = inside(string(p, "project")?, ps)?;
            Ok(Box::new(move || Ok(repos(&project))))
        }),
        ("git_diff", |p, ps| {
            let repo = inside(string(p, "repo")?, ps)?;
            let (file, staged) = (string(p, "file")?.to_owned(), p["staged"].as_bool() == Some(true));
            Ok(Box::new(move || {
                let mut args = vec!["-c", "core.quotePath=false", "diff"];
                if staged {
                    args.push("--staged");
                }
                args.extend(["--", &file]);
                git(&repo, &args).map(|diff| json!({ "diff": diff })).map_err(Error::Failed)
            }))
        }),
        ("git_do", |p, ps| {
            let repo = inside(string(p, "repo")?, ps)?;
            let what = string(p, "act")?.to_owned();
            let files: Vec<String> = p["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str().map(str::to_owned)).collect();
            let message = p["message"].as_str().unwrap_or("").to_owned();
            Ok(Box::new(move || act(&repo, &what, &files, &message)))
        }),
        ("git_log", |p, ps| {
            let repo = inside(string(p, "repo")?, ps)?;
            let limit = p["limit"].as_u64().unwrap_or(200).min(2000) as usize;
            Ok(Box::new(move || log(&repo, limit)))
        }),
    ];
    for (name, check) in commands {
        let check = *check;
        hub.handle_later(name, move |who, params, reply| {
            if !matches!(who, Principal::Window | Principal::Cli) {
                return reply(Err(Error::Forbidden("project files, but from loom's window or the CLI".into())));
            }
            let job = match check(params, &projects()) {
                Ok(job) => job,
                Err(err) => return reply(Err(err)),
            };
            crate::rt::spawn_local(async move {
                reply(crate::rt::spawn_blocking(job).await.unwrap_or_else(|_| Err(Error::Failed("it stopped unexpectedly".into()))));
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_projects_files_and_a_write_over_a_newer_file_conflicts() {
        let root = std::env::temp_dir().join(format!("loom-pf-{}", crate::rt::uuid()));
        let project = root.join("p");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(root.join("secret"), "x").unwrap();
        let projects = vec![project.clone()];
        let at = |p: &Path| p.to_string_lossy().into_owned();
        assert!(inside(&at(&project.join("src/new.rs")), &projects).is_ok(), "a new file in it");
        assert!(matches!(inside(&at(&project.join("../secret")), &projects), Err(Error::Forbidden(_))));
        assert!(matches!(inside(&at(&root.join("secret")), &projects), Err(Error::Forbidden(_))));
        assert!(matches!(inside("src/a", &projects), Err(Error::Invalid(_))));
        let file = project.join("src/a.rs");
        let first = write(&file, "one", None).unwrap()["modified"].as_u64().unwrap();
        assert_eq!(read(&file).unwrap()["text"], "one");
        assert!(matches!(write(&file, "two", Some(first + 5)), Err(Error::Conflict(_))));
        write(&file, "two", Some(first)).unwrap();
        assert_eq!(list(&project).unwrap()["entries"][0]["name"], "src", "folders first");
        assert_eq!(score("src/main.rs", "main"), Some("src/main.rs".len()));
        assert!(score("src/main.rs", "zz").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
