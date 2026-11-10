//! Past conversations: the list Claude Code keeps (ACP `session/list`, the same list and titles
//! Claude Code shows), asked by a short-lived adapter process that stops once it answered.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::{Value, json};

use crate::provider::Conn;

/// A conversation Claude Code kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Past {
    pub session_id: String,
    pub cwd: PathBuf,
    /// None: Claude Code has none for it.
    pub title: Option<String>,
    /// ISO 8601, as the adapter gives it.
    pub updated: String,
}

type Done = Box<dyn FnOnce(Result<Vec<Past>, String>)>;

/// Every kept conversation, newest first (a few pages at most), or why not. Returns the
/// listing process, to stop it early when its answer is no longer wanted.
pub fn list(done: impl FnOnce(Result<Vec<Past>, String>) + 'static) -> Option<std::rc::Weak<Conn>> {
    let done: Rc<RefCell<Option<Done>>> = Rc::new(RefCell::new(Some(Box::new(done))));
    let found: Rc<RefCell<Vec<Past>>> = Rc::default();
    let finish = {
        let done = done.clone();
        Rc::new(move |result: Result<Vec<Past>, String>| {
            if let Some(done) = done.borrow_mut().take() {
                done(result);
            }
        })
    };
    let on_exit = finish.clone();
    let conn = match Conn::spawn(&crate::paths::home(), None, move |incoming| {
        if let crate::provider::Incoming::Exited = incoming {
            on_exit(Err("the agent program stopped".to_owned()));
        }
    }) {
        Ok(conn) => conn,
        Err(err) => {
            finish(Err(format!("the agent program did not start: {err}")));
            return None;
        }
    };
    // Never left running: answered or not, it stops after 20 s (keeping the pages it gave).
    let weak = Rc::downgrade(&conn);
    let (late, partial) = (finish.clone(), found.clone());
    let _ = crate::rt::timeout_once(Duration::from_secs(20), move || {
        let partial = newest_first(&partial);
        late(if partial.is_empty() { Err("the agent program did not answer".to_owned()) } else { Ok(partial) });
        if let Some(conn) = weak.upgrade() {
            conn.stop();
        }
    });
    let params = json!({
        "protocolVersion": 1,
        "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
        "clientInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") }
    });
    let next = conn.clone();
    conn.request("initialize", params, move |result| match result {
        Ok(_) => page(next, None, found, finish),
        Err(err) => {
            next.stop();
            finish(Err(message(&err)));
        }
    });
    Some(Rc::downgrade(&conn))
}

/// Deletes a past conversation for good (ACP session/delete, through a short-lived adapter).
pub fn delete(session: String, done: impl FnOnce(Result<(), String>) + 'static) {
    let done = Rc::new(RefCell::new(Some(Box::new(done) as Box<dyn FnOnce(Result<(), String>)>)));
    let finish = {
        let done = done.clone();
        Rc::new(move |result: Result<(), String>| {
            if let Some(done) = done.borrow_mut().take() {
                done(result);
            }
        })
    };
    let on_exit = finish.clone();
    let conn = match Conn::spawn(&crate::paths::home(), None, move |incoming| {
        if let crate::provider::Incoming::Exited = incoming {
            on_exit(Err("the agent program stopped".to_owned()));
        }
    }) {
        Ok(conn) => conn,
        Err(err) => return finish(Err(err.to_string())),
    };
    let weak = Rc::downgrade(&conn);
    let _ = crate::rt::timeout_once(Duration::from_secs(20), move || {
        if let Some(conn) = weak.upgrade() {
            conn.stop();
        }
    });
    let params = json!({ "protocolVersion": 1, "clientCapabilities": {}, "clientInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") } });
    let next = conn.clone();
    conn.request("initialize", params, move |result| {
        if let Err(err) = result {
            next.stop();
            return finish(Err(message(&err)));
        }
        let again = next.clone();
        next.request("session/delete", json!({ "sessionId": session }), move |result| {
            again.stop();
            finish(result.map(|_| ()).map_err(|e| message(&e)));
        });
    });
}

fn newest_first(found: &RefCell<Vec<Past>>) -> Vec<Past> {
    let mut all = found.borrow().clone();
    all.sort_by(|a, b| b.updated.cmp(&a.updated));
    all
}

/// Asks for one page (after `cursor`); the next, or the end.
fn page(conn: Rc<Conn>, cursor: Option<String>, found: Rc<RefCell<Vec<Past>>>, finish: Rc<impl Fn(Result<Vec<Past>, String>) + 'static>) {
    let params = match &cursor {
        Some(cursor) => json!({ "cursor": cursor }),
        None => json!({}),
    };
    let next = conn.clone();
    conn.request("session/list", params, move |result| {
        let value = match result {
            Ok(value) => value,
            Err(err) => {
                next.stop();
                return finish(Err(message(&err)));
            }
        };
        found.borrow_mut().extend(parse(&value));
        let count = found.borrow().len();
        match value["nextCursor"].as_str() {
            // Ten thousand is plenty to find a project's recent ones.
            Some(cursor) if count < 10_000 => page(next, Some(cursor.to_owned()), found, finish),
            _ => {
                next.stop();
                finish(Ok(newest_first(&found)));
            }
        }
    });
}

/// The folder with symlinks resolved (as it is, if it is gone).
pub fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

fn parse(value: &Value) -> Vec<Past> {
    value["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(Past {
                session_id: s["sessionId"].as_str()?.to_owned(),
                cwd: PathBuf::from(s["cwd"].as_str()?),
                title: s["title"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned),
                updated: s["updatedAt"].as_str().unwrap_or("").to_owned(),
            })
        })
        .collect()
}

fn message(err: &Value) -> String {
    err["message"].as_str().unwrap_or("unknown error").to_owned()
}

/// "just now", "5m ago", "3h ago", "yesterday", "4d ago", else the date.
pub fn ago(iso: &str) -> String {
    ago_at(iso, Timestamp::now())
}

fn ago_at(iso: &str, now: Timestamp) -> String {
    let Ok(then) = iso.parse::<Timestamp>() else { return String::new() };
    let seconds = now.as_second() - then.as_second();
    match seconds {
        ..86400 => crate::paths::ago(seconds.max(0) as u64),
        86400..172800 => "yesterday".to_owned(),
        172800..604800 => format!("{}d ago", seconds / 86400),
        _ => then.to_zoned(TimeZone::system()).strftime("%e %b %Y").to_string().trim().to_owned(),
    }
}

/// "Today", "Yesterday" or "Earlier", for an ISO 8601 time.
pub fn day_of(iso: &str) -> &'static str {
    let Ok(then) = iso.parse::<Timestamp>() else { return "Earlier" };
    let (then, today) = (then.to_zoned(TimeZone::system()).date(), jiff::Zoned::now().date());
    if then == today {
        "Today"
    } else if today.yesterday().is_ok_and(|y| y == then) {
        "Yesterday"
    } else {
        "Earlier"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_read_from_the_list() {
        let value = json!({ "sessions": [
            { "sessionId": "a", "cwd": "/p", "title": "  Fix it ", "updatedAt": "2026-10-25T09:00:00Z" },
            { "sessionId": "b", "cwd": "/p/web", "title": null, "updatedAt": "2026-10-24T09:00:00Z" },
            { "cwd": "/p" }
        ]});
        let past = parse(&value);
        assert_eq!(past.len(), 2);
        assert_eq!(past[0].title.as_deref(), Some("Fix it"));
        assert_eq!(past[1].title, None);
        assert_eq!(past[1].cwd, PathBuf::from("/p/web"));
    }

    #[test]
    fn times_read_as_ago() {
        let now: Timestamp = "2026-10-25T12:00:00Z".parse().unwrap();
        assert_eq!(ago_at("2026-10-25T11:59:30Z", now), "just now");
        assert_eq!(ago_at("2026-10-25T11:15:00Z", now), "45m ago");
        assert_eq!(ago_at("2026-10-25T09:00:00Z", now), "3h ago");
        assert_eq!(ago_at("2026-10-24T09:00:00Z", now), "yesterday");
        assert_eq!(ago_at("2026-10-21T09:00:00Z", now), "4d ago");
        assert_eq!(ago_at("nonsense", now), "");
    }
}
