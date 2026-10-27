//! The journal: what was said in each conversation, written down by loom as it happens, for
//! the note agent (Scribe) to read. Compaction shrinks what the model keeps; the journal keeps
//! what loom saw. Only with the Notes add-on on; only on this computer (mode 0600), never
//! synced; a file is deleted some days after its last line was turned into notes.
//!
//! One file per conversation (`<session id>.jsonl`), one line per message:
//! `{"at": unix seconds, "role": "user"|"assistant", "text": …, "title": …, "cwd": …}`.
//! Beside it, `<session id>.mark` holds how far (a byte offset) Scribe has read: lines
//! before it are done. The mark only moves after their notes are written, so a check that
//! fails halfway reads them again.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Value, json};

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// How long a fully read journal is kept (to read again after a rejected change).
const KEEP_DAYS: u64 = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Where the line starts in its file (its id); `end` is where the next one starts.
    pub offset: u64,
    pub end: u64,
    pub at: u64,
    pub role: String,
    pub text: String,
    pub title: String,
    pub cwd: String,
}

/// The folder for `app_id` (a test instance keeps its own), set once at start.
pub fn init(app_id: &str) {
    let name = crate::state::suffix(app_id).map_or("journal".to_owned(), |s| format!("journal-{s}"));
    let _ = DIR.set(crate::state::dir().join(name));
    prune();
}

/// Scribe's own record (questions, changes to review, what it did), with the journal.
pub fn store_path() -> PathBuf {
    dir().join("scribe.json")
}

fn dir() -> PathBuf {
    DIR.get().cloned().unwrap_or_else(|| crate::state::dir().join("journal"))
}

fn file(session: &str) -> Option<PathBuf> {
    // A session id is a uuid; anything else is not a file name to trust.
    let usable = !session.is_empty() && session.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    usable.then(|| dir().join(format!("{session}.jsonl")))
}

/// Writes a message down (nothing when Notes is off, or for an empty text).
pub fn append(session: &str, role: &str, text: &str, title: &str, cwd: &Path) {
    if text.trim().is_empty() || crate::vault::current().is_none() {
        return;
    }
    let Some(path) = file(session) else { return };
    let line = json!({ "at": crate::state::now(), "role": role, "text": text, "title": title, "cwd": cwd.to_string_lossy() });
    if let Err(err) = write_line(&path, &line) {
        crate::log::write("journal", &format!("{}: {err}", path.display()));
    }
}

fn write_line(path: &Path, line: &Value) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if let Some(parent) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
    }
    let mut out = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path)?;
    writeln!(out, "{line}")
}

/// What a conversation said after its mark: everything, or with `finished_only` up to its
/// last reply (a turn still going may change; its question waits for its answer).
pub fn unread(session: &str, finished_only: bool) -> Vec<Entry> {
    let Some(path) = file(session) else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    let from = mark(session);
    let mut entries = Vec::new();
    let mut offset = 0u64;
    for line in text.split_inclusive('\n') {
        let end = offset + line.len() as u64;
        // A line still being written (no newline yet) is not read.
        if offset >= from && line.ends_with('\n') {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                let s = |k: &str| v[k].as_str().unwrap_or("").to_owned();
                entries.push(Entry { offset, end, at: v["at"].as_u64().unwrap_or(0), role: s("role"), text: s("text"), title: s("title"), cwd: s("cwd") });
            }
        }
        offset = end;
    }
    if finished_only {
        let last_reply = entries.iter().rposition(|e| e.role == "assistant");
        entries.truncate(last_reply.map_or(0, |i| i + 1));
    }
    entries
}

/// How much is not read yet (bytes of journal) and since when, without reading it all: the
/// first unread line only.
pub fn unread_summary(session: &str) -> Option<(u64, u64)> {
    use std::io::{BufRead, Seek, SeekFrom};
    let path = file(session)?;
    let len = std::fs::metadata(&path).ok()?.len();
    let from = mark(session);
    if len <= from {
        return None;
    }
    let mut reader = std::io::BufReader::new(std::fs::File::open(&path).ok()?);
    reader.seek(SeekFrom::Start(from)).ok()?;
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let at = serde_json::from_str::<Value>(&line).ok()?["at"].as_u64()?;
    Some((len - from, at))
}

fn mark_file(session: &str) -> Option<PathBuf> {
    file(session).map(|f| f.with_extension("mark"))
}

pub fn mark(session: &str) -> u64 {
    mark_file(session).and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| t.trim().parse().ok()).unwrap_or(0)
}

/// Everything before `offset` is in the notes now.
pub fn set_mark(session: &str, offset: u64) {
    if let Some(path) = mark_file(session) {
        if offset > mark(session) {
            let _ = std::fs::write(path, offset.to_string());
        }
    }
}

/// The conversations with something not read yet (a check after loom starts catches up).
pub fn waiting() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir()) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".jsonl").map(str::to_owned))
        .filter(|session| std::fs::metadata(dir().join(format!("{session}.jsonl"))).is_ok_and(|m| m.len() > mark(session)))
        .collect()
}

fn done_file(session: &str) -> Option<PathBuf> {
    file(session).map(|f| f.with_extension("done"))
}

fn done_lines(session: &str) -> Vec<String> {
    done_file(session).and_then(|f| std::fs::read_to_string(f).ok()).map(|t| t.lines().map(str::to_owned).collect()).unwrap_or_default()
}

fn add_done_line(session: &str, line: &str) {
    if let Some(path) = done_file(session) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let written = std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| writeln!(f, "{line}"));
        if let Err(err) = written {
            crate::log::write("journal", &format!("{}: {err}", path.display()));
        }
    }
}

/// Scribe finished `id` (a piece and a topic: `<session>@<from>-<to>/<topic>`).
pub fn mark_done(session: &str, id: &str) {
    add_done_line(session, id);
}

pub fn is_done(session: &str, id: &str) -> bool {
    done_lines(session).iter().any(|l| l == id)
}

/// The topics piece `id` was routed to, with what each is about (kept so a check that
/// resumes it doesn't route it again).
pub fn set_todo(session: &str, id: &str, topics: &[(String, String)]) {
    let one = |(topic, about): &(String, String)| format!("{}\u{1f}{}", topic.replace(['\t', '\n'], " "), about.replace(['\t', '\n'], " "));
    add_done_line(session, &format!("{id}/topics {}", topics.iter().map(one).collect::<Vec<_>>().join("\t")));
    add_done_line(session, &format!("{id}/routed"));
}

/// The topics a piece was routed to, if it was: (topic, about).
pub fn todo(session: &str, id: &str) -> Option<Vec<(String, String)>> {
    let prefix = format!("{id}/topics ");
    let line = done_lines(session).into_iter().rev().find(|l| l.starts_with(&prefix))?;
    Some(
        line[prefix.len()..]
            .split('\t')
            .filter(|t| !t.is_empty())
            .map(|t| match t.split_once('\u{1f}') {
                Some((topic, about)) => (topic.to_owned(), about.to_owned()),
                None => (t.to_owned(), String::new()),
            })
            .collect(),
    )
}

/// Every topic of a routed piece is done.
pub fn done_with(session: &str, id: &str) -> bool {
    let lines = done_lines(session);
    todo(session, id).is_some_and(|topics| topics.iter().all(|(t, _)| lines.iter().any(|l| *l == format!("{id}/{t}"))))
}

/// Journals read to the end and untouched for some days: gone.
fn prune() {
    let Ok(entries) = std::fs::read_dir(dir()) else { return };
    let old = std::time::Duration::from_secs(KEEP_DAYS * 24 * 3600);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(session) = name.strip_suffix(".jsonl") else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let stale = meta.modified().ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > old);
        if stale && mark(session) >= meta.len() {
            let _ = std::fs::remove_file(entry.path());
            for extra in [mark_file(session), done_file(session)].into_iter().flatten() {
                let _ = std::fs::remove_file(extra);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The journal's folder in tests: a temporary one (never the user's).
    fn temp() -> PathBuf {
        let root = std::env::temp_dir().join(format!("loom-journal-{}", std::process::id()));
        let _ = DIR.set(root.clone());
        root
    }

    #[test]
    fn a_piece_is_done_topic_by_topic() {
        temp();
        let session = "done-1".to_owned();
        let id = format!("{session}@0-10");
        assert!(todo(&session, &id).is_none());
        set_todo(&session, &id, &[("Work".into(), "the release".into()), ("Finances".into(), "rent".into())]);
        assert_eq!(todo(&session, &id).unwrap(), vec![("Work".to_owned(), "the release".to_owned()), ("Finances".to_owned(), "rent".to_owned())]);
        mark_done(&session, &format!("{id}/Work"));
        assert!(!done_with(&session, &id));
        mark_done(&session, &format!("{id}/Finances"));
        assert!(done_with(&session, &id));
        if let Some(f) = done_file(&session) {
            let _ = std::fs::remove_file(f);
        }
    }

    #[test]
    fn lines_are_read_after_the_mark_and_up_to_the_last_reply() {
        let root = temp();
        let path = file("s-1").unwrap();
        for (role, text) in [("user", "my rent is 650"), ("assistant", "noted"), ("user", "and the phone?")] {
            write_line(&path, &json!({ "at": 1, "role": role, "text": text, "title": "t", "cwd": "/x" })).unwrap();
        }
        assert_eq!(unread("s-1", false).len(), 3);
        let finished = unread("s-1", true);
        assert_eq!(finished.len(), 2);
        set_mark("s-1", finished[1].end);
        let rest = unread("s-1", false);
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].text, "and the phone?");
        assert!(unread("s-1", true).is_empty());
        assert!(waiting().contains(&"s-1".to_owned()));
        assert!(file("../etc").is_none());
        let _ = std::fs::remove_file(root.join("s-1.jsonl"));
        let _ = std::fs::remove_file(root.join("s-1.mark"));
    }
}
