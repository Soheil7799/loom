//! What Settings keeps beyond settings, in the core API (loom's window and the CLI): the
//! notes vault's topics, and the skills, agents and commands of loom's Claude folder. Each
//! listed, switched on or off, made new, and its file opened in the user's editor on this
//! computer (only a file these lists give).
//!
//! - `topics`: `{vault, topics: [{name, path, folder, listens_for, apply: ask|auto, sync, enabled}]}`
//! - `topic_set {name, enabled}`, `topic_new {name, about?}` (`about`: Claude drafts its
//!   file from it; answered once drafted) → `{path}`
//! - `claude_items`: `{folder, items: [{kind: skill|agent|command, name, description, enabled, shipped, path}]}`
//! - `claude_set {kind, name, enabled}`, `claude_new {kind, name}` → `{path}`, `claude_restore {name}`
//! - `edit_file {path}`: one of those files, opened in the user's editor.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::api::{Error, Principal};
use crate::plugin::{self, Kind};
use crate::topics::{self, Apply, Topic};

fn person(who: &Principal) -> Result<(), Error> {
    match who {
        Principal::Window | Principal::Cli => Ok(()),
        _ => Err(Error::Forbidden("Settings, but from loom's window or the CLI".into())),
    }
}

fn vault() -> Result<PathBuf, Error> {
    crate::vault::current().ok_or_else(|| Error::Failed("Notes is off, or has no vault (Settings → Notes).".into()))
}

fn topic_named(name: &str) -> Result<Topic, Error> {
    topics::list(&vault()?).into_iter().find(|t| t.name == name).ok_or_else(|| Error::NotFound(format!("topic {name}")))
}

fn kind(params: &Value) -> Result<Kind, Error> {
    match params["kind"].as_str() {
        Some("skill") => Ok(Kind::Skill),
        Some("agent") => Ok(Kind::Agent),
        Some("command") => Ok(Kind::Command),
        _ => Err(Error::Invalid("kind: skill, agent or command".into())),
    }
}

fn string<'a>(params: &'a Value, key: &str) -> Result<&'a str, Error> {
    params[key].as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| Error::Invalid(format!("which {key}?")))
}

/// Every file `edit_file` may open: the topics' and the Claude folder's.
fn editable(path: &Path) -> bool {
    let topic = crate::vault::current().is_some_and(|v| topics::list(&v).iter().any(|t| t.path == path));
    topic || Kind::ALL.iter().any(|k| plugin::items(*k).iter().any(|i| i.file == path))
}

/// Registers the commands (once, at start).
pub fn serve() {
    let hub = crate::api::hub();
    hub.handle("topics", |who, _| {
        person(who)?;
        let vault = vault()?;
        let list: Vec<Value> = topics::list(&vault)
            .into_iter()
            .map(|t| json!({ "name": t.name, "path": t.path, "folder": t.folder, "listens_for": t.listens_for, "apply": if t.apply == Apply::Auto { "auto" } else { "ask" }, "sync": t.sync, "enabled": t.enabled }))
            .collect();
        Ok(json!({ "vault": vault, "topics": list }))
    });
    hub.handle("topic_set", |who, params| {
        person(who)?;
        let topic = topic_named(string(params, "name")?)?;
        let on = params["enabled"].as_bool().ok_or_else(|| Error::Invalid("enabled: true or false".into()))?;
        topics::set_enabled(&topic, on).map_err(|e| Error::Failed(e.to_string()))?;
        Ok(json!({}))
    });
    hub.handle_later("topic_new", |who, params, reply| {
        let checked = (|| {
            person(who)?;
            Ok::<_, Error>((vault()?, string(params, "name")?.trim().to_owned(), params["about"].as_str().unwrap_or("").trim().to_owned()))
        })();
        let (vault, name, about) = match checked {
            Ok(checked) => checked,
            Err(err) => return reply(Err(err)),
        };
        if about.is_empty() {
            return reply(topics::create(&vault, &name).map(|path| json!({ "path": path })).map_err(Error::Invalid));
        }
        // Described: Claude drafts its file (a call, off the core's thread).
        if let Err(why) = topics::new_file(&vault, &name) {
            return reply(Err(Error::Invalid(why)));
        }
        crate::rt::spawn_local(async move {
            let drafted = {
                let name = name.clone();
                crate::rt::spawn_blocking(move || crate::scribe::describe_topic(&name, &about)).await
            };
            reply(match drafted {
                Ok(Ok(text)) => topics::create_with(&vault, &name, &text).map(|path| json!({ "path": path })).map_err(Error::Failed),
                Ok(Err(why)) => Err(Error::Failed(format!("Not drafted: {why}"))),
                Err(_) => Err(Error::Failed("Drafting stopped unexpectedly.".into())),
            });
        });
    });
    hub.handle("claude_items", |who, _| {
        person(who)?;
        let items: Vec<Value> = Kind::ALL
            .iter()
            .flat_map(|k| plugin::items(*k))
            .map(|i| json!({ "kind": i.kind.one(), "name": i.name, "description": i.description, "enabled": i.enabled, "shipped": i.shipped, "path": i.file }))
            .collect();
        Ok(json!({ "folder": plugin::dir(), "items": items }))
    });
    hub.handle("claude_set", |who, params| {
        person(who)?;
        let (kind, name) = (kind(params)?, string(params, "name")?);
        let on = params["enabled"].as_bool().ok_or_else(|| Error::Invalid("enabled: true or false".into()))?;
        let item = plugin::items(kind).into_iter().find(|i| i.name == name).ok_or_else(|| Error::NotFound(format!("{} {name}", kind.one())))?;
        plugin::set_enabled(&item, on).map_err(|e| Error::Failed(e.to_string()))?;
        plugin::changed();
        Ok(json!({}))
    });
    hub.handle("claude_new", |who, params| {
        person(who)?;
        let path = plugin::create(kind(params)?, string(params, "name")?).map_err(Error::Invalid)?;
        plugin::changed();
        Ok(json!({ "path": path }))
    });
    hub.handle("claude_restore", |who, params| {
        person(who)?;
        plugin::restore(string(params, "name")?).map_err(|e| Error::Failed(e.to_string()))?;
        plugin::changed();
        Ok(json!({}))
    });
    hub.handle("edit_file", |who, params| {
        person(who)?;
        let path = PathBuf::from(string(params, "path")?);
        if !editable(&path) {
            return Err(Error::Forbidden("only a topic's file or one of loom's Claude folder".into()));
        }
        crate::asks::open_in_editor(&path).map_err(Error::Failed)?;
        Ok(json!({}))
    });
}
