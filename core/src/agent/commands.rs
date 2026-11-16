//! What clients of the core API may tell agents (warp): `send` a message, `answer` a question,
//! `stop` a turn. Each is checked against who sends it, its arguments included:
//!
//! - an agent (through its loom tools) does none of them;
//! - a bridge (the phone, Telegram) sends no `!` commands and never answers "Always allow";
//! - the command-line client answers no questions (but in a test instance);
//! - loom's window does everything.
//!
//! Their effect shows in `agents` and `conversation/<agent>`; a question answered elsewhere
//! first is a `Conflict`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use serde_json::{Value, json};

use super::Agent;
use crate::api::{Error, Principal};

thread_local! {
    /// The agents open (the workspace's and Notes chats'), by id.
    static AGENTS: RefCell<HashMap<u32, Weak<Agent>>> = RefCell::default();
}

/// `agent` can be told things through the API from now on.
pub(super) fn register(agent: &Rc<Agent>) {
    AGENTS.with_borrow_mut(|agents| {
        agents.retain(|_, a| a.strong_count() > 0);
        agents.insert(agent.id, Rc::downgrade(agent));
    });
    serve();
}

thread_local! {
    static SERVED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// How pictures sent in become PNG (the UI's toolkit reads them).
    static TO_PNG: RefCell<Option<crate::bridges::ToPng>> = const { RefCell::new(None) };
}

/// How pictures sent with `send` are read (set by the UI, once).
pub fn set_to_png(to_png: crate::bridges::ToPng) {
    TO_PNG.with_borrow_mut(|t| *t = Some(to_png));
}

/// Registers the commands (once; a client learns of them at `initialize`, agents or not).
pub fn serve() {
    if !SERVED.with(|s| s.replace(true)) {
        handle();
    }
}

fn handle() {
    let hub = crate::api::hub();
    // Text, and pictures (`images`: base64, any format the UI reads) sent as pasted ones:
    // those are read off the core's thread, so the answer comes once they are.
    hub.handle_later("send", |who, params, reply| {
        let checked = (|| {
            let agent = target(who, params, "send messages")?;
            let text = params["text"].as_str().unwrap_or("").to_owned();
            let images: Vec<String> = params["images"].as_array().into_iter().flatten().filter_map(|d| d.as_str().map(str::to_owned)).collect();
            if text.trim().is_empty() && images.is_empty() {
                return Err(Error::Invalid("send needs text or pictures".into()));
            }
            if matches!(who, Principal::Bridge(_)) && text.trim_start().starts_with('!') {
                return Err(Error::Forbidden("`!` commands from a bridge".into()));
            }
            Ok((agent, text, images))
        })();
        let (agent, text, images) = match checked {
            Ok(checked) => checked,
            Err(err) => return reply(Err(err)),
        };
        if images.is_empty() {
            agent.submit_with(text, Vec::new());
            return reply(Ok(json!({})));
        }
        let Some(to_png) = TO_PNG.with_borrow(Clone::clone) else { return reply(Err(Error::Failed("pictures can't be read here".into()))) };
        let agent = Rc::downgrade(&agent);
        crate::rt::spawn_local(async move {
            let pngs = crate::rt::spawn_blocking(move || {
                images
                    .iter()
                    .map(|data| {
                        let bytes = crate::bridges::decode(data).ok_or("That picture could not be read.")?;
                        to_png(bytes).map(super::view::Png::from).map_err(|_| "That picture could not be read.".to_owned())
                    })
                    .collect::<Result<Vec<_>, String>>()
            })
            .await
            .unwrap_or_else(|_| Err("That picture could not be read.".into()));
            reply(match (pngs, agent.upgrade()) {
                (Err(why), _) => Err(Error::Invalid(why)),
                (_, None) => Err(Error::NotFound("that agent is closed".into())),
                (Ok(pngs), Some(agent)) => {
                    agent.submit_with(text, pngs);
                    Ok(json!({}))
                }
            });
        });
    });
    hub.handle("answer", |who, params| {
        // Answers are a person's: in loom's window or through a bridge they paired. The CLI is
        // whatever loom can't place under an agent, so an agent's process that left its own
        // tree would pass for it: it answers nothing (but in a test instance).
        if *who == Principal::Cli && !crate::api::test_instance() {
            return Err(Error::Forbidden("answering questions, but in loom's window or a paired bridge".into()));
        }
        let agent = target(who, params, "answer questions")?;
        let entry = params["entry"].as_str().ok_or_else(|| Error::Invalid("answer needs the question's entry".into()))?;
        let Some((serial, options, form)) = agent.question_entry(entry) else {
            return Err(Error::Conflict("not waiting for an answer (answered already?)".into()));
        };
        if form {
            return match (&params["content"], params["skip"].as_bool()) {
                (Value::Object(_), _) => {
                    agent.answer_form(serial, Some(params["content"].clone()));
                    Ok(json!({}))
                }
                (_, Some(true)) => {
                    agent.answer_form(serial, None);
                    Ok(json!({}))
                }
                _ => Err(Error::Invalid("a form is answered with content (an object) or skip".into())),
            };
        }
        let option = params["option"].as_str().ok_or_else(|| Error::Invalid("answer needs an option".into()))?;
        let Some((_, _, kind)) = options.iter().find(|(id, _, _)| id == option) else {
            return Err(Error::Invalid(format!("no option {option}")));
        };
        if matches!(who, Principal::Bridge(_)) && kind == "allow_always" {
            return Err(Error::Forbidden("\"Always allow\" from a bridge".into()));
        }
        agent.choose_serial(serial, option);
        Ok(json!({}))
    });
    // A permission mode: "bypassPermissions" from loom's window only (as "Always allow").
    hub.handle("set_mode", |who, params| {
        let mode = params["mode"].as_str().ok_or_else(|| Error::Invalid("which mode?".into()))?;
        if mode == "bypassPermissions" && *who != Principal::Window {
            return Err(Error::Forbidden("bypassing permissions, but from loom's window".into()));
        }
        // A bridge gets no standing approval ("Always allow" is refused it too): asking, or planning.
        if matches!(who, Principal::Bridge(_)) && !matches!(mode, "default" | "plan") {
            return Err(Error::Forbidden(format!("mode {mode} from a bridge")));
        }
        let agent = target(who, params, "change modes")?;
        agent.choose_mode(mode).map_err(Error::Invalid)?;
        Ok(json!({}))
    });
    hub.handle("rename", |who, params| {
        let agent = target(who, params, "rename agents")?;
        let title = params["title"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| Error::Invalid("rename needs a title".into()))?;
        agent.rename(title);
        Ok(json!({}))
    });
    // Suspend, keep awake and rewind: a person's (the window, the CLI), not a bridge's.
    hub.handle("suspend", |who, params| {
        let agent = person(who, params, "suspend agents")?;
        if !agent.suspend() {
            return Err(Error::Conflict("it is working: stop it first".into()));
        }
        Ok(json!({}))
    });
    hub.handle("run_recipe", |who, params| {
        let agent = person(who, params, "run recipes")?;
        let steps: Vec<String> = params["steps"].as_array().ok_or_else(|| Error::Invalid("steps: the recipe's messages, in order".into()))?.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect();
        if steps.iter().all(|s| s.trim().is_empty()) {
            return Err(Error::Invalid("a recipe needs a step".into()));
        }
        agent.run_recipe(steps);
        Ok(json!({}))
    });
    hub.handle("set_awake", |who, params| {
        let agent = person(who, params, "keep agents awake")?;
        let on = params["awake"].as_bool().ok_or_else(|| Error::Invalid("awake: true or false".into()))?;
        agent.set_awake(on);
        Ok(json!({}))
    });
    // What it changed, and going back on it: only files it touched. git runs off the core's
    // thread (a diff per file; an agent that touched hundreds of files would hold it).
    hub.handle_later("changes", |who, params, reply| {
        let agent = match person(who, params, "read agents' changes") {
            Ok(agent) => agent,
            Err(err) => return reply(Err(err)),
        };
        let files = agent.files();
        crate::rt::spawn_local(async move {
            let described = crate::rt::spawn_blocking(move || files.iter().map(|(path, mark)| crate::changes::describe(path, *mark)).collect::<Vec<Value>>()).await;
            reply(described.map(|files| json!({ "files": files })).map_err(|_| Error::Failed("reading the changes stopped unexpectedly".into())));
        });
    });
    hub.handle_later("revert", |who, params, reply| {
        let checked = (|| {
            let agent = person(who, params, "revert agents' changes")?;
            let path = params["path"].as_str().ok_or_else(|| Error::Invalid("which file (path)?".into()))?;
            let path = std::path::PathBuf::from(path);
            if !agent.files().iter().any(|(p, _)| *p == path) {
                return Err(Error::Forbidden("only a file this agent created or changed".into()));
            }
            let hunk = match params["hunk"].as_u64() {
                None => None,
                Some(n) => Some((n as usize, params["hunk_text"].as_str().ok_or_else(|| Error::Invalid("a hunk is undone with its text as shown (hunk_text)".into()))?.to_owned())),
            };
            Ok((path, hunk))
        })();
        let (path, hunk) = match checked {
            Ok(checked) => checked,
            Err(err) => return reply(Err(err)),
        };
        crate::rt::spawn_local(async move {
            let done = crate::rt::spawn_blocking(move || crate::changes::revert(&path, hunk.as_ref().map(|(n, text)| (*n, text.as_str())))).await;
            reply(match done {
                Ok(Ok(())) => Ok(json!({})),
                Ok(Err(why)) if why.contains("changed since") => Err(Error::Conflict(why)),
                Ok(Err(why)) => Err(Error::Failed(why)),
                Err(_) => Err(Error::Failed("reverting stopped unexpectedly".into())),
            });
        });
    });
    // The window's actions on an agent: a person's.
    hub.handle("fork", |who, params| {
        let agent = person(who, params, "copy conversations")?;
        agent.fork(None, String::new());
        Ok(json!({}))
    });
    hub.handle("clear", |who, params| {
        let agent = person(who, params, "clear conversations")?;
        agent.clear_conversation();
        Ok(json!({}))
    });
    hub.handle("restart", |who, params| {
        let agent = person(who, params, "restart agents")?;
        agent.restart();
        Ok(json!({}))
    });
    hub.handle("add_folder", |who, params| {
        let agent = person(who, params, "give agents folders")?;
        let folder = params["folder"].as_str().map(|f| crate::state::expand(std::path::Path::new(f))).ok_or_else(|| Error::Invalid("which folder?".into()))?;
        if !folder.is_absolute() || !folder.is_dir() {
            return Err(Error::Invalid(format!("{} is not a folder", folder.display())));
        }
        agent.add_dir(folder);
        Ok(json!({}))
    });
    hub.handle("export", |who, params| {
        let agent = person(who, params, "export conversations")?;
        Ok(json!({ "markdown": agent.markdown() }))
    });
    // Seen: one agent (opened in a page), or every one ("all": Mark all seen).
    hub.handle("mark_seen", |who, params| {
        if params["all"].as_bool() == Some(true) {
            if matches!(who, Principal::Agent(_)) {
                return Err(Error::Forbidden("an agent may not mark agents seen".into()));
            }
            for agent in AGENTS.with_borrow(|agents| agents.values().filter_map(Weak::upgrade).collect::<Vec<_>>()) {
                agent.mark_seen();
            }
            return Ok(json!({}));
        }
        target(who, params, "mark agents seen")?.mark_seen();
        Ok(json!({}))
    });
    // Ctrl+Enter: the queued messages go into the running turn now.
    // Ctrl+B D: what runs in the turn goes on in the background (the agent can be talked to).
    hub.handle("background", |who, params| {
        target(who, params, "send work to the background")?.background_now();
        Ok(json!({}))
    });
    hub.handle("send_now", |who, params| {
        target(who, params, "send messages")?.send_waiting_now();
        Ok(json!({}))
    });
    hub.handle("stop_workflow", |who, params| {
        let agent = person(who, params, "stop workflows")?;
        let id = params["workflow"].as_str().ok_or_else(|| Error::Invalid("which workflow (its id)?".into()))?;
        if !agent.stop_workflow_named(id) {
            return Err(Error::NotFound(format!("workflow {id}")));
        }
        Ok(json!({}))
    });
    hub.handle("stop_subagent", |who, params| {
        let agent = person(who, params, "stop subagents")?;
        let key = params["subagent"].as_str().ok_or_else(|| Error::Invalid("which subagent (a `crew/<agent>` item's id)?".into()))?;
        agent.stop_crew(key);
        Ok(json!({}))
    });
    hub.handle("mcp_server", |who, params| {
        let agent = person(who, params, "change agents' tool servers")?;
        let server = params["server"].as_str().ok_or_else(|| Error::Invalid("which server (an `mcp` item's server name)?".into()))?;
        match params["act"].as_str() {
            Some("reconnect") => agent.mcp("reconnect", server, true),
            Some("on") => agent.mcp("toggle", server, true),
            Some("off") => agent.mcp("toggle", server, false),
            _ => return Err(Error::Invalid("act: reconnect, on or off".into())),
        }
        Ok(json!({}))
    });
    hub.handle("reload_extensions", |who, params| {
        let agent = person(who, params, "reload agents' skills")?;
        agent.reload_extensions();
        Ok(json!({}))
    });
    hub.handle("press", |who, params| {
        let agent = person(who, params, "press agents' buttons")?;
        let entry = params["entry"].as_str().ok_or_else(|| Error::Invalid("which notice (its entry)?".into()))?;
        if !agent.press(entry) {
            return Err(Error::Conflict("no button there (pressed already?)".into()));
        }
        Ok(json!({}))
    });
    hub.handle("rewind", |who, params| {
        let agent = person(who, params, "rewind agents")?;
        let entry = params["entry"].as_str().ok_or_else(|| Error::Invalid("rewind needs a message of yours (its entry)".into()))?;
        // A copy is made by its running process: a resting one starts first.
        match agent.status() {
            super::Status::Suspended | super::Status::Failed => {
                agent.start();
                return Err(Error::Conflict("it was resting: it is starting now, rewind again in a few seconds".into()));
            }
            super::Status::Starting => return Err(Error::Conflict("it is starting: rewind again in a few seconds".into())),
            _ => {}
        }
        let (at, before, text) = rewind_point(&crate::api::hub().store(&super::conversation(agent.id)).items(), entry).ok_or_else(|| Error::NotFound(format!("your message {entry}")))?;
        if params["files"].as_bool() == Some(true) {
            agent.rewind_files_at(at, before, text);
        } else {
            agent.rewind(at, before, text);
        }
        Ok(json!({}))
    });
    // A session setting (model, effort…): one of the agent's `options`, to one of its values.
    hub.handle("set_option", |who, params| {
        let agent = target(who, params, "change agents' settings")?;
        let (Some(id), Some(value)) = (params["option"].as_str(), params["value"].as_str()) else { return Err(Error::Invalid("set_option needs an option and a value".into())) };
        let options = agent.options();
        let option = options.as_array().into_iter().flatten().find(|o| o["id"] == id).ok_or_else(|| Error::Invalid(format!("no option {id} (it isn't running, or has none such)")))?;
        if !values(&option["options"]).iter().any(|v| v == value) {
            return Err(Error::Invalid(format!("{id} has no value {value}")));
        }
        agent.set_option(id, value);
        Ok(json!({}))
    });
    hub.handle("set_draft", |who, params| {
        let agent = target(who, params, "change what agents have in their input")?;
        let text = params["text"].as_str().ok_or_else(|| Error::Invalid("set_draft needs text (empty clears it)".into()))?;
        agent.set_draft(text);
        Ok(json!({}))
    });
    hub.handle("unqueue", |who, params| {
        let agent = target(who, params, "take messages back")?;
        let index = params["index"].as_u64().ok_or_else(|| Error::Invalid("which queued message (index)?".into()))? as usize;
        if index >= agent.queued().len() {
            return Err(Error::Conflict("not waiting any more (sent already?)".into()));
        }
        agent.unqueue(index);
        Ok(json!({}))
    });
    hub.handle("stop", |who, params| {
        let agent = target(who, params, "stop agents")?;
        agent.cancel();
        Ok(json!({}))
    });
}

/// An option's values (a list of {value, name}, or of groups with their own `options`).
fn values(options: &Value) -> Vec<String> {
    options.as_array().into_iter().flatten().flat_map(|o| match o["options"].as_array() {
        Some(_) => values(&o["options"]),
        None => o["value"].as_str().map(|v| vec![v.to_owned()]).unwrap_or_default(),
    }).collect()
}

/// The agent a command is for, if `who` is a person at this computer (loom's window, the CLI).
fn person(who: &Principal, params: &Value, what: &str) -> Result<Rc<Agent>, Error> {
    if !matches!(who, Principal::Window | Principal::Cli) {
        return Err(Error::Forbidden(format!("only loom's window or the CLI may {what}")));
    }
    target(who, params, what)
}

/// Where rewinding to user message `entry` goes, as `Agent::rewind` takes it: the agent's
/// message before it (its `message` id), whether there was one, and the message's text.
fn rewind_point(entries: &[Value], entry: &str) -> Option<(Option<String>, bool, String)> {
    let at = entries.iter().position(|e| e["id"] == entry && e["kind"] == "user")?;
    let previous = entries[..at].iter().rev().find(|e| e["kind"] == "agent");
    let message = previous.and_then(|e| e["message"].as_str()).map(str::to_owned);
    Some((message, previous.is_some(), entries[at]["text"].as_str().unwrap_or("").trim().to_owned()))
}

/// The agent a command is for (`agent`: its id), if `who` may tell agents anything.
pub(crate) fn target(who: &Principal, params: &Value, what: &str) -> Result<Rc<Agent>, Error> {
    if matches!(who, Principal::Agent(_)) {
        return Err(Error::Forbidden(format!("an agent may not {what}")));
    }
    let id = params["agent"].as_str().ok_or_else(|| Error::Invalid("which agent?".into()))?;
    let agent = id.parse::<u32>().ok().and_then(|id| AGENTS.with_borrow(|agents| agents.get(&id).and_then(Weak::upgrade)));
    agent.ok_or_else(|| Error::NotFound(format!("agent {id}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bridge_may_not_turn_on_standing_approval() {
        serve();
        let bot = crate::api::hub().connect(Principal::Bridge("telegram".into()));
        for mode in ["acceptEdits", "auto", "bypassPermissions"] {
            assert!(matches!(bot.command(mode, "set_mode", json!({ "agent": "99", "mode": mode })), Err(Error::Forbidden(_))), "{mode}");
        }
        // asking and planning pass (then: no agent 99)
        let plan = bot.command("plan", "set_mode", json!({ "agent": "99", "mode": "plan" }));
        assert!(matches!(plan, Err(Error::NotFound(_))), "{plan:?}");
    }

    #[test]
    fn rewinding_goes_to_the_agents_message_before_yours() {
        let entries = vec![
            json!({ "id": "e1", "kind": "user", "text": "first" }),
            json!({ "id": "e2", "kind": "agent", "text": "ok", "message": "m1" }),
            json!({ "id": "e3", "kind": "tool" }),
            json!({ "id": "e4", "kind": "user", "text": " second " }),
        ];
        assert_eq!(rewind_point(&entries, "e4"), Some((Some("m1".into()), true, "second".into())));
        assert_eq!(rewind_point(&entries, "e1"), Some((None, false, "first".into())));
        assert_eq!(rewind_point(&entries, "e2"), None, "only a message of yours");
        assert_eq!(rewind_point(&entries, "e9"), None);
    }

    #[test]
    fn an_options_values_are_found_in_its_groups_too() {
        let options = json!([{ "value": "default", "name": "Default" }, { "group": "More", "options": [{ "value": "opus" }, { "value": "haiku" }] }]);
        assert_eq!(values(&options), ["default", "opus", "haiku"]);
    }
}
