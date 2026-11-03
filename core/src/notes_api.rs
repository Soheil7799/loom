//! The notes vault in the core API (warp), read-only, for loom's window, the CLI and paired
//! devices (see `api::may_read`). The vault is Obsidian's as much as loom's: these are views
//! of its files, read when a client subscribes (off the core's thread: the snapshot comes
//! empty and the items follow).
//!
//! - `vault/<folder>` (`vault/` for the top): the folder's entries, folders first, as
//!   `bridges::browse::list` gives them, each with `id` = its `path`.
//! - `note/<path>`: one item `{id: "note", path, text, title, tags, links, backlinks, modified}`
//!   (another file: its text, or `binary`).
//! - Either, when it can't be read (no such folder, Notes off since): one item `{id: "error",
//!   kind: "error", message}`.
//!
//! Command `search_notes {query, limit}`: `{results: [{path, title, folder, snippet, modified}]}`.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::api::{Error, Principal, Store};
use crate::bridges::browse;

/// Sets the vault's resources and command up (once, at start).
pub fn serve() {
    let hub = crate::api::hub();
    hub.provide("vault/", |store, folder| {
        let folder = folder.trim_end_matches('/').to_owned();
        follow(store, move |root| browse::list(&root, &folder).map(|listed| listed["entries"].as_array().cloned().unwrap_or_default().into_iter().map(|mut entry| {
            entry["id"] = entry["path"].clone();
            entry
        }).collect()));
        true
    });
    hub.provide("note/", |store, path| {
        let path = path.to_owned();
        follow(store, move |root| {
            browse::read(&root, &path).map(|mut note| {
                note["id"] = json!("note");
                vec![note]
            })
        });
        true
    });
    hub.handle("search_notes", |who, params| {
        if !matches!(who, Principal::Window | Principal::Cli) && *who != Principal::Bridge("remote".into()) {
            return Err(Error::Forbidden("the notes vault".into()));
        }
        let query = params["query"].as_str().filter(|q| !q.trim().is_empty()).ok_or_else(|| Error::Invalid("search_notes needs a query".into()))?;
        let limit = params["limit"].as_u64().map_or(30, |limit| limit.min(100)) as usize;
        let root = crate::vault::current().ok_or_else(|| Error::NotFound("the notes vault (Notes is off)".into()))?;
        browse::search(&root, query, limit).map_err(Error::Invalid)
    });
}

/// `store`'s items read by `read` (given the vault's folder) for each new subscriber, off the
/// core's thread.
fn follow(store: &Store, read: impl Fn(PathBuf) -> Result<Vec<Value>, String> + Send + Sync + Clone + 'static) {
    store.on_subscribe(move |store| {
        let (store, read) = (store.clone(), read.clone());
        let Some(root) = crate::vault::current() else {
            store.reset(vec![error("Notes is off in loom (Settings → Add-ons → Notes).")]);
            return;
        };
        crate::rt::spawn_local(async move {
            let items = crate::rt::spawn_blocking(move || read(root)).await.unwrap_or_else(|_| Err("Could not read the vault.".into()));
            store.set(items.unwrap_or_else(|why| vec![error(&why)]));
        });
    });
}

fn error(message: &str) -> Value {
    json!({ "id": "error", "kind": "error", "message": message })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;

    type Task = Pin<Box<dyn Future<Output = ()>>>;

    /// Runs what was spawned until nothing is left (or 10 s pass).
    fn run_all(tasks: &Rc<RefCell<Vec<Task>>>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            let mut pending: Vec<Task> = tasks.borrow_mut().drain(..).collect();
            if pending.is_empty() {
                return;
            }
            pending.retain_mut(|task| task.as_mut().poll(&mut cx).is_pending());
            tasks.borrow_mut().extend(pending);
            assert!(std::time::Instant::now() < deadline, "tasks never ended");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn the_vault_reads_into_resources_for_those_allowed() {
        let tasks: Rc<RefCell<Vec<Task>>> = Rc::default();
        let queue = tasks.clone();
        crate::rt::set_host(move |f| queue.borrow_mut().push(f), |_, _| {});
        let root = std::env::temp_dir().join(format!("loom-notes-api-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Work")).unwrap();
        std::fs::write(root.join("Work/plan.md"), "The [[budget]].\n").unwrap();
        std::fs::write(root.join("budget.md"), "Money.\n").unwrap();
        crate::settings::update(|s| {
            s.notes_enabled = true;
            s.notes_vault = root.to_string_lossy().into_owned();
        });
        serve();
        let hub = crate::api::hub();
        let window = hub.connect(Principal::Window);
        let top = window.subscribe("vault/", || {}).unwrap();
        let note = window.subscribe("note/Work/plan.md", || {}).unwrap();
        run_all(&tasks);
        let (mut folder, mut read) = (crate::api::Mirror::default(), crate::api::Mirror::default());
        hub.store("vault/").flush();
        hub.store("note/Work/plan.md").flush();
        top.drain().iter().for_each(|m| drop(folder.apply(m)));
        note.drain().iter().for_each(|m| drop(read.apply(m)));
        let ids: Vec<&str> = folder.items.iter().filter_map(|e| e["id"].as_str()).collect();
        assert_eq!(ids, ["Work", "budget.md"]);
        assert_eq!(read.items[0]["links"][0]["path"], "budget.md");
        let missing = window.subscribe("vault/Nowhere", || {}).unwrap();
        run_all(&tasks);
        hub.store("vault/Nowhere").flush();
        let mut gone = crate::api::Mirror::default();
        missing.drain().iter().for_each(|m| drop(gone.apply(m)));
        assert_eq!(gone.items[0]["kind"], "error");
        // The Telegram bot never reads the vault; a paired device may.
        let bot = hub.connect(Principal::Bridge("telegram".into()));
        assert!(matches!(bot.subscribe("vault/", || {}), Err(crate::api::Error::Forbidden(_))));
        assert!(matches!(bot.command("s1", "search_notes", json!({ "query": "money" })), Err(Error::Forbidden(_))));
        let phone = hub.connect(Principal::Bridge("remote".into()));
        let found = phone.command("s1", "search_notes", json!({ "query": "money" })).unwrap();
        assert_eq!(found["results"][0]["path"], "budget.md");
        let _ = std::fs::remove_dir_all(&root);
    }
}
