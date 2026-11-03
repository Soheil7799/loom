//! The notes vault in the core API (warp), read-only, for loom's window, the CLI and paired
//! devices (see `api::may_read`). The vault is Obsidian's as much as loom's: these are views
//! of its files, read when a client subscribes (off the core's thread: the snapshot comes
//! empty and the items follow).
//!
//! Followed, they follow the files: a change in the vault (but in its hidden folders,
//! `.obsidian/` and the like) reads what is followed again, a moment later, and sends what
//! differs. The vault is watched (inotify, FSEvents) while anything of it was followed.
//!
//! - `vault/<folder>` (`vault/` for the top): the folder's entries, folders first, as
//!   `bridges::browse::list` gives them, each with `id` = its `path`.
//! - `note/<path>`: one item `{id: "note", path, text, title, tags, links, backlinks, modified}`
//!   (another file: its text, or `binary`).
//! - Either, when it can't be read (no such folder, Notes off since): one item `{id: "error",
//!   kind: "error", message}`.
//!
//! Command `search_notes {query, limit}`: `{results: [{path, title, folder, snippet, modified}]}`.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::Duration;

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
        watch(&root);
        crate::rt::spawn_local(async move {
            let items = crate::rt::spawn_blocking(move || read(root)).await.unwrap_or_else(|_| Err("Could not read the vault.".into()));
            store.set(items.unwrap_or_else(|why| vec![error(&why)]));
        });
    });
}

/// How long changes gather before what is followed is read again (an editor saving as you
/// type, a sync tool writing many files).
const SETTLE: Duration = Duration::from_millis(300);

thread_local! {
    /// The vault watched, and the watch (dropped, it stops).
    static WATCH: RefCell<Option<(PathBuf, notify::RecommendedWatcher)>> = const { RefCell::new(None) };
    /// Changes came and a reading is due.
    static DUE: Cell<bool> = const { Cell::new(false) };
}

/// Watches the vault at `root` (and no other) for changes to what clients may follow.
fn watch(root: &Path) {
    if WATCH.with_borrow(|w| w.as_ref().is_some_and(|(watched, _)| watched == root)) {
        return;
    }
    let (tx, rx) = async_channel::unbounded::<()>();
    let base = root.to_path_buf();
    let made = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && !matches!(event.kind, notify::EventKind::Access(_))
            && event.paths.iter().any(|path| visible(&base, path))
        {
            let _ = tx.send_blocking(());
        }
    });
    let watcher = made.and_then(|mut watcher| {
        use notify::Watcher;
        watcher.watch(root, notify::RecursiveMode::Recursive).map(|()| watcher)
    });
    let watcher = match watcher {
        Ok(watcher) => watcher,
        Err(err) => {
            crate::log::write("notes", &format!("can't watch the vault: {err}"));
            return;
        }
    };
    WATCH.with_borrow_mut(|w| *w = Some((root.to_path_buf(), watcher)));
    crate::rt::spawn_local(async move {
        // Ends when the watch goes (its sender with it).
        while rx.recv().await.is_ok() {
            if DUE.replace(true) {
                continue;
            }
            let _ = crate::rt::timeout_once(SETTLE, || {
                DUE.set(false);
                for prefix in ["vault/", "note/"] {
                    for store in crate::api::hub().stores_with(prefix).into_iter().filter(|s| s.followed()) {
                        store.refresh();
                    }
                }
            });
        }
    });
}

/// Whether a change at `path` is to something a client may see (not inside a hidden folder or
/// file, `.obsidian/` and the like).
fn visible(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|rest| !rest.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')))
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

    /// Runs what was spawned until nothing is left, or nothing changed for 100 ms (the
    /// vault's watch waits for ever).
    fn run_all(tasks: &Rc<RefCell<Vec<Task>>>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let mut still = 0;
        loop {
            let mut pending: Vec<Task> = tasks.borrow_mut().drain(..).collect();
            if pending.is_empty() {
                return;
            }
            let before = pending.len();
            pending.retain_mut(|task| task.as_mut().poll(&mut cx).is_pending());
            let spawned = !tasks.borrow().is_empty();
            still = if pending.len() == before && !spawned { still + 1 } else { 0 };
            tasks.borrow_mut().extend(pending);
            if still >= 20 {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "tasks never settled");
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

    #[test]
    fn a_followed_folder_follows_the_files() {
        let tasks: Rc<RefCell<Vec<Task>>> = Rc::default();
        let timers: Rc<RefCell<Vec<(std::time::Instant, Box<dyn FnOnce()>)>>> = Rc::default();
        let (queue, later) = (tasks.clone(), timers.clone());
        crate::rt::set_host(move |f| queue.borrow_mut().push(f), move |after, f| later.borrow_mut().push((std::time::Instant::now() + after, f)));
        let root = std::env::temp_dir().join(format!("loom-notes-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        std::fs::write(root.join("first.md"), "one\n").unwrap();
        crate::settings::update(|s| {
            s.notes_enabled = true;
            s.notes_vault = root.to_string_lossy().into_owned();
        });
        serve();
        let hub = crate::api::hub();
        let top = hub.connect(Principal::Window).subscribe("vault/", || {}).unwrap();
        let mut folder = crate::api::Mirror::default();
        // Runs what is due (tasks, timers) until `done` says enough.
        let settle = |folder: &mut crate::api::Mirror, done: &dyn Fn(&crate::api::Mirror) -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                run_all(&tasks);
                let due: Vec<Box<dyn FnOnce()>> = {
                    let mut timers = timers.borrow_mut();
                    let now = std::time::Instant::now();
                    let (ready, waiting): (Vec<_>, Vec<_>) = timers.drain(..).partition(|(at, _)| *at <= now);
                    *timers = waiting;
                    ready.into_iter().map(|(_, f)| f).collect()
                };
                due.into_iter().for_each(|f| f());
                run_all(&tasks);
                hub.store("vault/").flush();
                top.drain().iter().for_each(|m| drop(folder.apply(m)));
                if done(folder) {
                    return;
                }
                assert!(std::time::Instant::now() < deadline, "never came: {:?}", folder.items);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        let names = |folder: &crate::api::Mirror| folder.items.iter().filter_map(|e| e["id"].as_str().map(str::to_owned)).collect::<Vec<_>>();
        settle(&mut folder, &|f| names(f) == ["first.md"]);
        // Written by another program (Obsidian, a sync tool): the follower gets it.
        std::fs::write(root.join("second.md"), "two\n").unwrap();
        settle(&mut folder, &|f| names(f) == ["first.md", "second.md"]);
        std::fs::remove_file(root.join("first.md")).unwrap();
        settle(&mut folder, &|f| names(f) == ["second.md"]);
        // Hidden folders are not watched for: no reading is due for them.
        std::fs::write(root.join(".obsidian/workspace.json"), "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        run_all(&tasks);
        assert!(!DUE.get(), "a change in .obsidian/ made a reading due");
        let _ = std::fs::remove_dir_all(&root);
    }
}
