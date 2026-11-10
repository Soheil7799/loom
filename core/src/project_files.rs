//! A project's files and git in the core API, for loom's window and the CLI (the files panel):
//! only inside a project of the sidebar's (`projects`), each command off the core's thread.
//!
//! - `files_list {folder}` → `{entries: [{name, path, dir, size, modified}]}`, folders first
//! - `files_find {project, query, limit?}` → `{files: [relative path]}`, best first
//! - `file_read {path, bytes?}` → `{text, modified}` (`text` null: not text, or over 2 MB:
//!   `size`); with `bytes`, `{data}` (base64, 10 MB at most: a picture to show)
//! - `file_write {path, text, modified?}` → `{modified}`: a conflict if it changed on disk since
//!   `modified` (the time `file_read` gave); a new file when there was none
//! - `git_repos {project}` → `{repos: [{path, name, branch, ahead, behind, changes: [{code, file}]}]}`
//! - `git_diff {repo, file, staged?}` → `{diff}`
//! - `git_do {repo, act: stage|unstage|discard|commit, files?, message?}` → `{}`
//! - `git_log {repo, limit?, graph?}` → `{commits: [{graph, hash, refs, subject, author, when}]}`
//!   (`graph`: the branch lines drawn before each, and lines of their own between)
//! - `open_path {path, how: app|folder}` → `{}`: the file in its app, or its folder in the file
//!   manager, on this computer
//! - `trash_file {path}` → `{}`: to the desktop's trash (gio)

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::api::{Error, Principal};
use crate::files::git;

/// What `file_read` gives as text, at most; and as bytes.
const READ: u64 = 2 << 20;
const BYTES: u64 = 10 << 20;

/// The sidebar's projects (their folders), read on the core's thread.
pub fn projects() -> Vec<PathBuf> {
    crate::api::hub().store("projects").items().iter().filter_map(|p| p["id"].as_str().map(PathBuf::from)).collect()
}

/// `path` (absolute), if it lies in one of `projects`: no `..` past one, no symlink out of it
/// (a dangling one too: written through, it would make its target wherever that is).
// (what is read or written is opened by `open_checked`, which refuses it if it is not this
// path any more: a link swapped in between)
pub fn inside(path: &str, projects: &[PathBuf]) -> Result<PathBuf, Error> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(Error::Invalid(format!("{} is not an absolute path", path.display())));
    }
    // A file not there yet: its folder must be.
    let real = match path.canonicalize() {
        Ok(real) => real,
        Err(_) if path.symlink_metadata().is_ok() => return Err(Error::Forbidden(format!("{} is a link to nowhere", path.display()))),
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

/// `real` (as `inside` gave it) opened, and still `real` once open: no link at its end is
/// followed, and one swapped in above it since the check is caught (refused, nothing done).
fn open_checked(real: &Path, write: bool) -> Result<std::fs::File, Error> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new().read(!write).write(write).create(write).custom_flags(libc::O_NOFOLLOW).open(real).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::NotFound(format!("{}: {e}", real.display())),
        _ => Error::Failed(format!("{}: {e}", real.display())),
    })?;
    let opened = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).map_err(|e| Error::Failed(e.to_string()))?;
    if opened != real {
        return Err(Error::Forbidden(format!("{} changed while being opened", real.display())));
    }
    Ok(file)
}

fn read(path: &Path) -> Result<Value, Error> {
    use std::io::Read;
    let meta = std::fs::metadata(path).map_err(|e| Error::NotFound(format!("{}: {e}", path.display())))?;
    if meta.is_dir() {
        return Err(Error::Invalid("a folder: files_list it".into()));
    }
    if meta.len() > READ {
        return Ok(json!({ "text": null, "size": meta.len(), "modified": modified(&meta) }));
    }
    let mut bytes = Vec::new();
    open_checked(path, false)?.take(READ).read_to_end(&mut bytes).map_err(|e| Error::Failed(e.to_string()))?;
    Ok(json!({ "text": String::from_utf8(bytes).ok(), "size": meta.len(), "modified": modified(&meta) }))
}

fn bytes(path: &Path) -> Result<Value, Error> {
    use std::io::Read;
    let meta = std::fs::metadata(path).map_err(|e| Error::NotFound(format!("{}: {e}", path.display())))?;
    if !meta.is_file() || meta.len() > BYTES {
        return Err(Error::Invalid("a file of 10 MB at most".into()));
    }
    let mut data = Vec::new();
    open_checked(path, false)?.take(BYTES).read_to_end(&mut data).map_err(|e| Error::Failed(e.to_string()))?;
    Ok(json!({ "data": crate::api::wire::base64(&data), "size": meta.len() }))
}

/// `path` opened on this computer: in the app it belongs to, or its folder in the file manager.
fn open(path: &Path, how: &str) -> Result<Value, Error> {
    let target = match how {
        "app" => path.to_owned(),
        "folder" => if path.is_dir() { path.to_owned() } else { path.parent().unwrap_or(path).to_owned() },
        _ => return Err(Error::Invalid("how: app or folder".into())),
    };
    let child = std::process::Command::new("xdg-open").arg(&target).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    let mut child = child.map_err(|e| Error::Failed(format!("xdg-open: {e}")))?;
    std::thread::spawn(move || child.wait());
    Ok(json!({}))
}

fn trash(path: &Path) -> Result<Value, Error> {
    let out = std::process::Command::new("gio").arg("trash").arg(path).output().map_err(|e| Error::Failed(format!("no trash here (gio: {e})")))?;
    if !out.status.success() {
        return Err(Error::Failed(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
    }
    Ok(json!({}))
}

fn write(path: &Path, text: &str, seen: Option<u64>) -> Result<Value, Error> {
    if let (Ok(meta), Some(seen)) = (std::fs::metadata(path), seen)
        && modified(&meta) != seen
    {
        return Err(Error::Conflict("it changed on disk since it was opened: open it again, or save once more to overwrite".into()));
    }
    // emptied only once it is known to be this file
    let mut file = open_checked(path, true)?;
    let written = file.set_len(0).and_then(|()| std::io::Write::write_all(&mut file, text.as_bytes()));
    written.map_err(|e| Error::Failed(e.to_string()))?;
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
        // only what the last commit has: a file new in the index has nothing to go back to
        "discard" if files.iter().any(|f| git(repo, &["cat-file", "-e", &format!("HEAD:{f}")]).is_err()) => {
            return Err(Error::Invalid("not in the last commit (new, or renamed): unstage it, then delete it".into()));
        }
        "discard" => with(&["restore", "--staged", "--worktree", "--source=HEAD"]),
        "commit" if message.trim().is_empty() => return Err(Error::Invalid("a commit needs a message".into())),
        "commit" => vec!["commit".into(), "-m".into(), message.trim().into()],
        _ => return Err(Error::Invalid("act: stage, unstage, discard or commit".into())),
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    git(repo, &args).map(|_| json!({})).map_err(Error::Failed)
}

fn log(repo: &Path, limit: usize, skip: usize, graph: bool, all: bool) -> Result<Value, Error> {
    let (n, skip) = (limit.to_string(), format!("--skip={skip}"));
    // --date-order: never a parent before all its children (a page draws the lanes from it)
    let mut args = vec!["-c", "core.quotePath=false", "log", "--date-order", "-n", &n, &skip, "--format=%x1e%h%x1f%D%x1f%s%x1f%an%x1f%ar%x1f%p"];
    if graph {
        args.push("--graph");
    }
    if graph || all {
        args.push("--all");
    }
    let out = git(repo, &args).map_err(Error::Failed)?;
    Ok(json!({ "commits": log_lines(&out) }))
}

/// git log's lines: the graph drawn before a commit (up to its \x1e), then its fields; a line
/// of graph alone has no hash.
fn log_lines(out: &str) -> Vec<Value> {
    out.lines()
        .map(|l| match l.split_once('\u{1e}') {
            Some((graph, rest)) => {
                let f: Vec<&str> = rest.split('\u{1f}').collect();
                let parents: Vec<&str> = f.get(5).map(|p| p.split_whitespace().collect()).unwrap_or_default();
                json!({ "graph": graph, "hash": f.first(), "refs": f.get(1), "subject": f.get(2), "author": f.get(3), "when": f.get(4), "parents": parents })
            }
            None => json!({ "graph": l, "hash": null }),
        })
        .collect()
}

/// A commit's hash, in hex: never an option or a range git would read.
fn is_hash(s: &str) -> bool {
    (4..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
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
            let raw = p["bytes"].as_bool() == Some(true);
            Ok(Box::new(move || if raw { bytes(&path) } else { read(&path) }))
        }),
        ("open_path", |p, ps| {
            let path = inside(string(p, "path")?, ps)?;
            let how = p["how"].as_str().unwrap_or("app").to_owned();
            Ok(Box::new(move || open(&path, &how)))
        }),
        ("trash_file", |p, ps| {
            let path = inside(string(p, "path")?, ps)?;
            if ps.iter().any(|project| project.canonicalize().is_ok_and(|c| c == path)) {
                return Err(Error::Forbidden("a project itself: remove it from the sidebar instead".into()));
            }
            Ok(Box::new(move || trash(&path)))
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
        ("git_show", |p, ps| {
            let repo = inside(string(p, "repo")?, ps)?;
            // a commit's hash alone: never an option git would read
            let hash = string(p, "hash")?.to_owned();
            if !is_hash(&hash) {
                return Err(Error::Invalid("hash: a commit's, in hex".into()));
            }
            Ok(Box::new(move || git(&repo, &["-c", "core.quotePath=false", "show", "--stat", "--patch", "--format=fuller", &hash, "--"]).map(|text| json!({ "text": text })).map_err(Error::Failed)))
        }),
        ("git_log", |p, ps| {
            let repo = inside(string(p, "repo")?, ps)?;
            let limit = p["limit"].as_u64().unwrap_or(200).min(2000) as usize;
            let (graph, all) = (p["graph"].as_bool() == Some(true), p["all"].as_bool() == Some(true));
            let skip = p["skip"].as_u64().unwrap_or(0) as usize;
            Ok(Box::new(move || log(&repo, limit, skip, graph, all)))
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
    fn only_a_hash_is_shown() {
        assert!(is_hash("b15ce87") && is_hash("B15CE875f0bc7bb255d3debcdb1ec62f04dd0aff"));
        assert!(!is_hash("--output=/tmp/x") && !is_hash("HEAD") && !is_hash("a1b") && !is_hash("a1b2..c3d4"));
    }

    #[test]
    fn a_link_swapped_in_after_the_check_is_refused_and_nothing_outside_is_touched() {
        let root = std::env::temp_dir().join(format!("loom-swap-{}", std::process::id()));
        let (project, outside) = (root.join("project"), root.join("outside"));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(project.join("notes.md"), "mine").unwrap();
        std::fs::write(outside.join("secret"), "keep").unwrap();
        let checked = inside(&project.join("notes.md").to_string_lossy(), std::slice::from_ref(&project)).unwrap();
        // swapped for a link out of the project, between the check and the write
        std::fs::remove_file(&checked).unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), &checked).unwrap();
        assert!(write(&checked, "overwritten", None).is_err());
        assert!(read(&checked).is_err());
        assert_eq!(std::fs::read_to_string(outside.join("secret")).unwrap(), "keep");
        let _ = std::fs::remove_dir_all(&root);
    }

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
        std::os::unix::fs::symlink(root.join("nowhere/x"), project.join("dangling")).unwrap();
        assert!(matches!(inside(&at(&project.join("dangling")), &projects), Err(Error::Forbidden(_))), "a link to nowhere");
        let file = project.join("src/a.rs");
        let first = write(&file, "one", None).unwrap()["modified"].as_u64().unwrap();
        assert_eq!(read(&file).unwrap()["text"], "one");
        assert!(matches!(write(&file, "two", Some(first + 5)), Err(Error::Conflict(_))));
        write(&file, "two", Some(first)).unwrap();
        assert_eq!(list(&project).unwrap()["entries"][0]["name"], "src", "folders first");
        assert_eq!(score("src/main.rs", "main"), Some("src/main.rs".len()));
        let lines = log_lines("* \u{1e}abc\u{1f}HEAD -> main\u{1f}two\u{1f}me\u{1f}now\u{1f}a1 b2\n|\\\n");
        assert_eq!(lines[0]["parents"], json!(["a1", "b2"]), "a merge's parents");
        assert_eq!((lines[0]["graph"].as_str(), lines[0]["hash"].as_str(), lines[0]["subject"].as_str()), (Some("* "), Some("abc"), Some("two")));
        assert!(lines[1]["hash"].is_null(), "a line of graph alone");
        assert_eq!(bytes(&file).unwrap()["data"], crate::api::wire::base64(b"two"));
        assert!(score("src/main.rs", "zz").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
