//! Jobs in the core API (warp).
//!
//! - `jobs`: every project's jobs, oldest first: `{id, project_id, name, command, status:
//!   "running" | "restarting" | "exited", exit_code}`.
//! - `job/<id>`: what job `id` shows, as raw terminal bytes (research/core-api §3): one item
//!   `{id: "output", rows, cols, screen, offset}`, `screen` (base64) drawing the job's screen
//!   as it was when `offset` bytes of this run's output had been printed; then `bytes` ops,
//!   each starting at its byte offset. A follower feeds `screen`, then each op's bytes, to a
//!   terminal; an `upsert` of the item (a new run) starts over. The screen is worked out for
//!   each new follower, the bytes sent only while someone follows.
//!
//! Commands: `stop_job` and `restart_job` (`{job}`), for loom's window and the CLI.

use std::cell::RefCell;
use std::collections::HashMap;

use async_channel::Receiver;
use serde_json::{Value, json};

use crate::api::{Error, Principal, Store, wire};
use crate::jobs::{Event, Id, Jobs};

thread_local! {
    /// Every project's jobs, to find a job by id.
    static PROJECTS: RefCell<Vec<Jobs>> = RefCell::default();
}

/// Publishes `jobs` in the core API from its `events`, which come out again (for a UI that
/// still wants them). Runs on the core's thread.
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
        // Each job's run: bytes told so far (the next `Output` starts there).
        let mut told: HashMap<Id, u64> = HashMap::new();
        while let Ok(event) = events.recv().await {
            publish(&jobs, &event, &mut told);
            // Nobody takes them any more: they are published all the same.
            let _ = tx.try_send(event);
        }
        PROJECTS.with_borrow_mut(|projects| projects.retain(|p| p.cwd() != jobs.cwd()));
    });
    rx
}

fn publish(jobs: &Jobs, event: &Event, told: &mut HashMap<Id, u64>) {
    let hub = crate::api::hub();
    match event {
        Event::Added(id) => {
            output_store(jobs, *id);
            item(jobs, *id, false);
        }
        Event::Restarting(id) | Event::Exited(id, _) => item(jobs, *id, matches!(event, Event::Restarting(_))),
        Event::Started(id) => {
            item(jobs, *id, false);
            told.insert(*id, 0);
            // A new run: followers start over from its screen.
            let output = output_store(jobs, *id);
            if output.followed() {
                output.upsert(screen(jobs, *id));
            }
        }
        Event::Output(id, bytes) => {
            let at = told.entry(*id).or_default();
            let start = *at;
            *at += bytes.len() as u64;
            let output = hub.store(&format!("job/{id}"));
            if !output.followed() {
                return;
            }
            // The followers' screen may hold some of these bytes already (it was worked out
            // after they were printed, before they were told here): only the rest goes.
            let held = output.get("output").and_then(|o| o["offset"].as_u64()).unwrap_or(0);
            let skip = held.saturating_sub(start) as usize;
            if skip < bytes.len() {
                output.bytes("output", start + skip as u64, &bytes[skip..]);
            }
        }
        Event::Removed(id) => {
            hub.store("jobs").remove(&id.to_string());
            hub.close(&format!("job/{id}"));
            told.remove(id);
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

/// `job/<id>`, made the first time; each new follower gets the screen as it is then (the
/// others keep theirs by the bytes, so they are not told).
fn output_store(jobs: &Jobs, id: Id) -> Store {
    let store = crate::api::hub().store(&format!("job/{id}"));
    if store.get("output").is_none() {
        let kept = jobs.clone();
        store.on_subscribe(move |store| store.refresh_item(screen(&kept, id)));
        store.refresh_item(screen(jobs, id));
    }
    store
}

/// Job `id`'s screen as `job/<id>`'s item.
fn screen(jobs: &Jobs, id: Id) -> Value {
    let (data, rows, cols, printed) = jobs.screen(id).unwrap_or_default();
    json!({ "id": "output", "rows": rows, "cols": cols, "screen": wire::base64(&data), "offset": printed })
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
        let stream = sub.as_ref().unwrap().drain();
        for m in &stream {
            mirror.apply(m);
        }
        // The screen and the bytes after it, fed in order, are all it printed.
        let mut printed = wire::bytes_of(mirror.items[0]["screen"].as_str().unwrap());
        for m in &stream {
            if let crate::api::Body::Changes { ops } = &m.body {
                for op in ops {
                    if let crate::api::Op::Bytes { data, .. } = op {
                        printed.extend(wire::bytes_of(data));
                    }
                }
            }
        }
        let text = String::from_utf8_lossy(&printed);
        assert!(text.contains("one") && text.contains("twé"), "{text:?}");
        // Where the stream is: every byte the run printed (the terminal makes each \n a \r\n).
        assert_eq!(mirror.items[0]["offset"].as_u64(), jobs.screen(id).map(|s| s.3));
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
}
