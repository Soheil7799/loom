//! What agents ask of the user beyond their conversation, for a UI with no window of loom's to
//! show it in (loomd's web page, the phone): the core API's `asks`, each `{id, agent, title,
//! kind, path, message, state}`:
//!
//! - `open`: a file the agent wants the user to see (`loom_open`);
//! - `edit`: a file the agent waits for the user to fill in (`loom_edit`: secrets kept out of
//!   the chat); `state` `waiting` until the user says done (or the agent gives up);
//! - `notify`: the agent wants the user (`loom_notify`);
//! - `input`: a job of its waits for the user to type in its terminal (`loom_run` with
//!   `input`: a password, a y/n), its id in `job`; a UI opens it.
//!
//! Commands, for people (loom's window, the CLI, a web page): `ask_done {id}` (an edit filled
//! in; an open or a notify seen, and gone), `ask_open {id}` (the file opened in the user's
//! editor, on this computer).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::agent::Agent;
use crate::api::{Error, Principal};

/// Asks kept at most (the oldest seen ones go first).
const KEPT: usize = 50;

thread_local! {
    static NEXT: Cell<u64> = const { Cell::new(1) };
    /// Edits by id: whose (the agent's id), and waiting or done (what its tool polls).
    static EDITS: RefCell<HashMap<u64, (u32, &'static str)>> = RefCell::default();
    static PATHS: RefCell<HashMap<String, PathBuf>> = RefCell::default();
}

fn store() -> crate::api::Store {
    crate::api::hub().store("asks")
}

fn add(agent: &Agent, kind: &str, path: Option<&Path>, message: &str) -> u64 {
    add_for(agent.id, &agent.title(), kind, path, message)
}

fn add_for(agent: u32, title: &str, kind: &str, path: Option<&Path>, message: &str) -> u64 {
    let n = NEXT.with(|n| n.replace(n.get() + 1));
    let id = format!("a{n}");
    if let Some(path) = path {
        PATHS.with_borrow_mut(|p| p.insert(id.clone(), path.to_path_buf()));
    }
    let store = store();
    store.upsert(json!({
        "id": id,
        "agent": agent.to_string(),
        "title": title,
        "kind": kind,
        "path": path.map(|p| p.to_string_lossy().into_owned()),
        "message": message,
        "state": "waiting",
        "at": crate::api::time(jiff::Timestamp::now().as_second()),
    }));
    // Too many: the oldest go, but an edit an agent still waits for.
    let items = store.items();
    let over = items.len().saturating_sub(KEPT);
    for old in items.iter().filter(|i| i["kind"] != "edit").take(over) {
        if let Some(old) = old["id"].as_str() {
            store.remove(old);
            PATHS.with_borrow_mut(|p| p.remove(old));
        }
    }
    n
}

/// `path` as the agent means it (`~`, or relative to its folder).
fn resolve(agent: &Agent, path: &Value) -> Result<PathBuf, String> {
    let path = crate::state::expand(Path::new(path.as_str().ok_or("a path is needed")?));
    Ok(if path.is_absolute() { path } else { agent.cwd.join(path) })
}

/// `loom_open`: a file shown to the user.
pub fn open_file(agent: &Agent, params: &Value) -> Result<Value, String> {
    let path = resolve(agent, &params["path"])?;
    if !path.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    add(agent, "open", Some(&path), params["message"].as_str().unwrap_or(""));
    Ok(json!({}))
}

/// `loom_edit`: a file for the user to fill in (made empty, readable by them only, if it isn't
/// there); its id, polled with `edit_state`.
/// The file `path` an agent asks the user to fill in: where it is, and whether it was made
/// now (empty, readable by the user only: it is for secrets), so a request that fails after
/// can take it away again. loom's window and `request_edit` both.
pub fn file_to_fill(agent: &Agent, path: &Value) -> Result<(PathBuf, bool), String> {
    let path = resolve(agent, path)?;
    if path.is_dir() {
        return Err(format!("{} is a folder", path.display()));
    }
    if path.exists() {
        return Ok((path, false));
    }
    if !path.parent().is_some_and(Path::is_dir) {
        return Err(format!("the folder of {} does not exist", path.display()));
    }
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((path, true))
}

pub fn request_edit(agent: &Agent, params: &Value) -> Result<Value, String> {
    let (path, _) = file_to_fill(agent, &params["path"])?;
    let message = params["message"].as_str().filter(|m| !m.trim().is_empty()).unwrap_or("Fill this file in, save it, then press Done.");
    let id = add(agent, "edit", Some(&path), message);
    EDITS.with_borrow_mut(|e| e.insert(id, (agent.id, "waiting")));
    Ok(json!({ "id": id, "path": path.to_string_lossy() }))
}

/// How agent `agent`'s edit `id` stands: waiting, done, or gone (another's is none of its).
fn edit_state(agent: u32, params: &Value) -> Result<Value, String> {
    let id = params["id"].as_u64().ok_or("an id is needed")?;
    let state = EDITS.with_borrow(|e| e.get(&id).filter(|(whose, _)| *whose == agent).map(|(_, s)| *s));
    Ok(json!({ "state": state.unwrap_or("gone") }))
}

/// Agent `agent` gave up waiting for its edit `id` (another's it can't touch).
fn edit_cancel(agent: u32, params: &Value) -> Result<Value, String> {
    let id = params["id"].as_u64().ok_or("an id is needed")?;
    if EDITS.with_borrow(|e| e.get(&id).is_some_and(|(whose, _)| *whose == agent)) {
        EDITS.with_borrow_mut(|e| e.remove(&id));
        let ask = format!("a{id}");
        store().remove(&ask);
        PATHS.with_borrow_mut(|p| p.remove(&ask));
    }
    Ok(json!({}))
}

/// `loom_notify`: the agent wants the user.
pub fn notify(agent: &Agent, params: &Value) -> Result<Value, String> {
    add(agent, "notify", None, params["message"].as_str().unwrap_or("Needs you"));
    Ok(json!({}))
}

/// `loom_run` with `input`: job `name` of the agent's waits for the user to type into it.
pub fn show_job(agent: &Agent, params: &Value) -> Result<Value, String> {
    let name = params["name"].as_str().ok_or("a job name is needed")?;
    let job = agent.jobs.named(name).ok_or_else(|| format!("no job named {name}"))?;
    let message = params["message"].as_str().filter(|m| !m.trim().is_empty()).unwrap_or("It waits for you to type in its terminal.");
    let n = add(agent, "input", None, message);
    let store = store();
    if let Some(mut item) = store.get(&format!("a{n}")) {
        item["job"] = json!(job.id.to_string());
        item["title"] = json!(format!("{}: {name}", agent.title()));
        store.upsert(item);
    }
    Ok(json!({}))
}

/// What an agent's loom tool `name` asks, if it is one of these.
pub fn tool(agent: &Agent, name: &str, params: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "open_file" => open_file(agent, params),
        "request_edit" => request_edit(agent, params),
        "edit_state" => edit_state(agent.id, params),
        "edit_cancel" => edit_cancel(agent.id, params),
        "notify" => notify(agent, params),
        "show_job" => show_job(agent, params),
        _ => return None,
    })
}

/// Registers `ask_done` and `ask_open`.
pub fn serve() {
    let hub = crate::api::hub();
    // There from the start (a UI follows it before anything is asked).
    hub.store("asks");
    let people = |who: &Principal| matches!(who, Principal::Window | Principal::Cli);
    hub.handle("ask_done", move |who, params| {
        if !people(who) {
            return Err(Error::Forbidden("asks, but for people".into()));
        }
        let id = params["id"].as_str().ok_or_else(|| Error::Invalid("which ask?".into()))?;
        let store = store();
        let item = store.get(id).ok_or_else(|| Error::NotFound(format!("ask {id}")))?;
        if item["kind"] == "edit"
            && let Some(n) = id.strip_prefix('a').and_then(|n| n.parse::<u64>().ok())
        {
            EDITS.with_borrow_mut(|e| {
                if let Some(edit) = e.get_mut(&n) {
                    edit.1 = "done";
                }
            });
        }
        store.remove(id);
        PATHS.with_borrow_mut(|p| p.remove(id));
        Ok(json!({}))
    });
    hub.handle("ask_open", move |who, params| {
        if !people(who) {
            return Err(Error::Forbidden("asks, but for people".into()));
        }
        let id = params["id"].as_str().ok_or_else(|| Error::Invalid("which ask?".into()))?;
        let path = PATHS.with_borrow(|p| p.get(id).cloned()).ok_or_else(|| Error::NotFound(format!("a file of ask {id}")))?;
        open_in_editor(&path).map_err(Error::Failed)?;
        Ok(json!({}))
    });
}

/// `path` in the user's editor (the setting, else the desktop's choice).
pub(crate) fn open_in_editor(path: &Path) -> Result<(), String> {
    let editor = crate::settings::get().editor;
    let mut words = editor.split_whitespace();
    let mut command = match words.next() {
        Some(program) => {
            let mut command = std::process::Command::new(program);
            command.args(words);
            command
        }
        None => std::process::Command::new("xdg-open"),
    };
    command.arg(path).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut child = command.spawn().map_err(|e| format!("could not open {}: {e}", path.display()))?;
    // Waited for, off this thread (no zombie left).
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_waits_on_and_cancels_only_its_own_edit() {
        crate::rt::set_host(|_| {}, |_, _| {});
        serve();
        let id = add_for(1, "one", "edit", Some(Path::new("/tmp/x.env")), "fill it");
        EDITS.with_borrow_mut(|e| e.insert(id, (1, "waiting")));
        let asked = json!({ "id": id });
        assert_eq!(edit_state(1, &asked).unwrap()["state"], "waiting");
        assert_eq!(edit_state(2, &asked).unwrap()["state"], "gone", "another agent's is none of its");
        edit_cancel(2, &asked).unwrap();
        assert_eq!(edit_state(1, &asked).unwrap()["state"], "waiting", "nor can it cancel it");
        // Done by a person: the agent hears so.
        let window = crate::api::hub().connect(Principal::Window);
        window.command("d1", "ask_done", json!({ "id": format!("a{id}") })).unwrap();
        assert_eq!(edit_state(1, &asked).unwrap()["state"], "done");
        // Cancelled by its own agent: gone, path and all.
        let other = add_for(1, "one", "edit", Some(Path::new("/tmp/y.env")), "fill it");
        EDITS.with_borrow_mut(|e| e.insert(other, (1, "waiting")));
        edit_cancel(1, &json!({ "id": other })).unwrap();
        assert_eq!(edit_state(1, &json!({ "id": other })).unwrap()["state"], "gone");
        assert!(PATHS.with_borrow(|p| !p.contains_key(&format!("a{other}"))));
        assert!(store().get(&format!("a{other}")).is_none());
    }
}
