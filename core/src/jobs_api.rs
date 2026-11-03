//! Jobs in the core API (warp).
//!
//! - `jobs`: every project's jobs, oldest first: `{id, project_id, name, command, status:
//!   "running" | "restarting" | "exited", exit_code}`.
//! - `job/<id>`: what job `id` shows, as one item `{id: "output", rows, cols, data}`: `data` is
//!   terminal output (escapes and all, as text) that draws the job's screen when fed to a
//!   terminal, and grows by `append` as the job prints. An `upsert` of it starts over: a new
//!   run, or the text grown past `KEEP` (then it is the screen again, not its history).
//!   Worked out only while someone follows it.
//!
//! Commands: `stop_job` and `restart_job` (`{job}`), for loom's window and the CLI.

use std::cell::RefCell;
use std::collections::HashMap;

use async_channel::Receiver;
use serde_json::{Value, json};

use crate::api::{Error, Principal, Store};
use crate::jobs::{Event, Id, Jobs};

/// How much output a follower's item holds before it starts over from the screen.
const KEEP: usize = 64 * 1024;

thread_local! {
    /// Every project's jobs, to find a job by id.
    static PROJECTS: RefCell<Vec<Jobs>> = RefCell::default();
}

/// Publishes `jobs` in the core API from its `events`, which come out again for the UI.
/// Runs on the core's thread.
pub fn published(jobs: &Jobs, events: Receiver<Event>) -> Receiver<Event> {
    let first = PROJECTS.with_borrow_mut(|projects| {
        projects.push(jobs.clone());
        projects.len() == 1
    });
    if first {
        handle();
    }
    crate::api::hub().store("jobs");
    let (tx, rx) = async_channel::unbounded();
    let jobs = jobs.clone();
    crate::rt::spawn_local(async move {
        // Bytes ending in the middle of a character, until the rest comes.
        let mut partial: HashMap<Id, Vec<u8>> = HashMap::new();
        while let Ok(event) = events.recv().await {
            publish(&jobs, &event, &mut partial);
            if tx.send(event).await.is_err() {
                break;
            }
        }
        PROJECTS.with_borrow_mut(|projects| projects.retain(|p| p.cwd() != jobs.cwd()));
    });
    rx
}

fn publish(jobs: &Jobs, event: &Event, partial: &mut HashMap<Id, Vec<u8>>) {
    let hub = crate::api::hub();
    match event {
        Event::Added(id) | Event::Restarting(id) | Event::Exited(id, _) => item(jobs, *id, matches!(event, Event::Restarting(_))),
        Event::Started(id) => {
            item(jobs, *id, false);
            partial.remove(id);
            let output = output_store(jobs, *id);
            if output.followed() {
                output.upsert(screen(jobs, *id));
            }
        }
        Event::Output(id, bytes) => {
            let output = hub.store(&format!("job/{id}"));
            if !output.followed() {
                partial.remove(id);
                return;
            }
            let mut held = partial.remove(id).unwrap_or_default();
            held.extend_from_slice(bytes);
            let (text, rest) = whole_chars(held);
            if !rest.is_empty() {
                partial.insert(*id, rest);
            }
            let long = output.get("output").and_then(|o| o["data"].as_str().map(str::len)).unwrap_or(0) + text.len() > KEEP;
            if long {
                output.upsert(screen(jobs, *id));
            } else {
                output.append("output", "data", &text);
            }
        }
        Event::Removed(id) => {
            hub.store("jobs").remove(&id.to_string());
            hub.close(&format!("job/{id}"));
            partial.remove(id);
        }
    }
}

/// Job `id`'s item in `jobs`.
fn item(jobs: &Jobs, id: Id, restarting: bool) {
    let Some(info) = jobs.info(id) else { return };
    let status = if restarting {
        "restarting"
    } else if info.running {
        "running"
    } else {
        "exited"
    };
    crate::api::hub().store("jobs").upsert(json!({
        "id": id.to_string(),
        "project_id": jobs.cwd().to_string_lossy(),
        "name": info.name,
        "command": info.command,
        "status": status,
        "exit_code": info.exit_code,
    }));
}

/// `job/<id>`, made the first time, with its screen brought up to date for each new follower.
fn output_store(jobs: &Jobs, id: Id) -> Store {
    let hub = crate::api::hub();
    let name = format!("job/{id}");
    let store = hub.store(&name);
    if store.get("output").is_none() {
        let kept = jobs.clone();
        store.on_subscribe(move |store| {
            if !store.followed() {
                store.upsert(screen(&kept, id));
            }
        });
        store.upsert(screen(jobs, id));
    }
    store
}

/// Job `id`'s screen as `job/<id>`'s item.
fn screen(jobs: &Jobs, id: Id) -> Value {
    let (data, rows, cols) = jobs.screen(id).unwrap_or_default();
    json!({ "id": "output", "rows": rows, "cols": cols, "data": String::from_utf8_lossy(&data) })
}

/// `bytes` as text, but for an unfinished character at the end (returned to wait for the
/// rest). Bytes that are no UTF-8 at all become U+FFFD.
fn whole_chars(bytes: Vec<u8>) -> (String, Vec<u8>) {
    // Where the last character starts, and how long it says it is.
    let start = (bytes.len().saturating_sub(4)..bytes.len()).rev().find(|&i| bytes[i] & 0xC0 != 0x80);
    let cut = match start.map(|i| (i, bytes[i])) {
        Some((i, lead)) if lead >= 0xC0 => {
            let wants = if lead >= 0xF0 { 4 } else if lead >= 0xE0 { 3 } else { 2 };
            if bytes.len() - i < wants { i } else { bytes.len() }
        }
        _ => bytes.len(),
    };
    (String::from_utf8_lossy(&bytes[..cut]).into_owned(), bytes[cut..].to_vec())
}

fn handle() {
    let hub = crate::api::hub();
    hub.handle("stop_job", |who, params| {
        let (jobs, id) = target(who, params)?;
        jobs.stop(id);
        Ok(json!({}))
    });
    hub.handle("restart_job", |who, params| {
        let (jobs, id) = target(who, params)?;
        jobs.restart(id);
        Ok(json!({}))
    });
}

/// The job a command is for (`job`: its id), if `who` may run jobs.
fn target(who: &Principal, params: &Value) -> Result<(Jobs, Id), Error> {
    if !matches!(who, Principal::Window | Principal::Cli) {
        return Err(Error::Forbidden("jobs, but from loom's window or the CLI".into()));
    }
    let job = params["job"].as_str().ok_or_else(|| Error::Invalid("which job?".into()))?;
    let id: Id = job.parse().map_err(|_| Error::NotFound(format!("job {job}")))?;
    let jobs = PROJECTS.with_borrow(|projects| projects.iter().find(|p| p.info(id).is_some()).cloned());
    jobs.map(|jobs| (jobs, id)).ok_or_else(|| Error::NotFound(format!("job {job}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_followed_job_streams_what_it_prints_and_the_list_says_how_it_ended() {
        // Changes go out by hand here (no main loop): the host only takes them.
        crate::rt::set_host(|_| {}, |_, _| {});
        let (jobs, events) = Jobs::new(std::env::temp_dir());
        let id = jobs.run("greet", "printf 'one\\n'; sleep 0.3; printf 'tw\\xc3\\xa9\\n'", None);
        let window = crate::api::hub().connect(Principal::Window);
        let mut partial = HashMap::new();
        let (mut sub, mut mirror) = (None, crate::api::Mirror::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            assert!(std::time::Instant::now() < deadline, "the job never ended");
            let Ok(event) = events.try_recv() else {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            };
            publish(&jobs, &event, &mut partial);
            if matches!(event, Event::Started(_)) {
                sub = Some(window.subscribe(&format!("job/{id}"), || {}).unwrap());
            }
            if matches!(event, Event::Exited(..)) {
                break;
            }
        }
        crate::api::hub().store(&format!("job/{id}")).flush();
        for m in sub.as_ref().unwrap().drain() {
            mirror.apply(&m);
        }
        let data = mirror.items[0]["data"].as_str().unwrap();
        assert!(data.contains("one") && data.contains("twé"), "{data:?}");
        let listed = crate::api::hub().store("jobs").get(&id.to_string()).unwrap();
        assert_eq!((listed["name"].as_str(), listed["status"].as_str(), listed["exit_code"].as_i64()), (Some("greet"), Some("exited"), Some(0)));
        // A command for it: not from a bridge.
        let phone = crate::api::hub().connect(Principal::Bridge("phone".into()));
        PROJECTS.with_borrow_mut(|projects| projects.push(jobs.clone()));
        handle();
        assert!(matches!(phone.command("c1", "restart_job", json!({ "job": id.to_string() })), Err(Error::Forbidden(_))));
        jobs.remove(id);
        publish(&jobs, &Event::Removed(id), &mut partial);
        assert!(crate::api::hub().store("jobs").get(&id.to_string()).is_none());
    }

    #[test]
    fn characters_split_between_reads_wait_for_their_rest() {
        let text = "héllo €".as_bytes().to_vec();
        let (first, rest) = whole_chars(text[..9].to_vec());
        assert_eq!((first.as_str(), rest.len()), ("héllo ", 2));
        let mut joined = rest;
        joined.extend_from_slice(&text[9..]);
        assert_eq!(whole_chars(joined), ("€".to_owned(), Vec::new()));
        // Not UTF-8 at all: replaced, and nothing held back for ever.
        let (bad, rest) = whole_chars(vec![b'a', 0xFF, b'b']);
        assert_eq!((bad.as_str(), rest.len()), ("a\u{FFFD}b", 0));
    }
}
