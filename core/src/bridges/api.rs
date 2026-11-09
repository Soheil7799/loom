//! What the core API offers bridges beyond agents' commands (warp, step 4):
//!
//! - `save` (`{agent, name, data}`: base64): a file for the agent, written in its folder
//!   under `<bridge>-files/` (never elsewhere, never over another); `{path}` back. Bridges
//!   only.
//! - `transcribe` (`{audio, name?, mime?}`): speech to text with loom's settings (its key
//!   never leaves loom); `{text}` back once heard. Loom's window, the CLI and bridges.
//! - `bridge_state` (any object): what the bridge says of itself, shown in Settings. Bridges
//!   only, each its own.
//! - `bridge` (`{method, params}`): a request of the bridge protocol (docs/bridges.md),
//!   `{"ok"}` or `{"error"}` back; its events in the resource `bridge/<name>`. For the
//!   bridges that still speak it (the remote one, for the shipped phone app).
//!
//! `save` and `transcribe` are answered once done (off the core's thread).

use serde_json::json;

use crate::api::{Error, Principal};

thread_local! {
    static SERVED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Registers the commands (once).
pub fn serve() {
    if SERVED.with(|s| s.replace(true)) {
        return;
    }
    let hub = crate::api::hub();
    hub.handle_later("save", |who, params, reply| {
        // a bridge's under its name; loom's web page's (dropped there: the browser gives no path) under web-files
        let folder = match who {
            Principal::Bridge(bridge) => format!("{bridge}-files"),
            _ => "web-files".to_owned(),
        };
        let dir = match crate::agent::commands::target(who, params, "be sent files") {
            Ok(agent) => agent.cwd.join(folder),
            Err(err) => return reply(Err(err)),
        };
        let name = super::safe_name(params["name"].as_str().unwrap_or(""));
        let data = params["data"].as_str().unwrap_or("").to_owned();
        crate::rt::spawn_local(async move {
            let saved = crate::rt::spawn_blocking(move || {
                let bytes = super::decode(&data).ok_or("That file could not be read.")?;
                let target = crate::files::free_name(&dir.join(name));
                std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&target, &bytes)).map_err(|e| format!("Could not save it in the agent's folder: {e}"))?;
                Ok::<_, String>(json!({ "path": target.to_string_lossy() }))
            })
            .await
            .unwrap_or_else(|_| Err("Could not save it in the agent's folder.".into()));
            reply(saved.map_err(Error::Failed));
        });
    });
    hub.handle_later("transcribe", |who, params, reply| {
        if matches!(who, Principal::Agent(_)) {
            return reply(Err(Error::Forbidden("speech to text, but for people".into())));
        }
        if !crate::stt::configured() {
            return reply(Err(Error::Failed("Speech to text is not set up in loom.".into())));
        }
        let Some(audio) = super::decode(params["audio"].as_str().unwrap_or("")) else {
            return reply(Err(Error::Invalid("that recording could not be read".into())));
        };
        crate::stt::transcribe(audio, params["name"].as_str().unwrap_or("voice.ogg"), params["mime"].as_str().unwrap_or("audio/ogg"), move |heard| {
            reply(heard.map(|text| json!({ "text": text })).map_err(Error::Failed));
        });
    });
    // The bridge protocol (docs/bridges.md) for a bridge that still speaks it: one request,
    // answered as the bridge socket did (`{"ok"}` / `{"error"}`).
    hub.handle_later("bridge", |who, params, reply| {
        let Principal::Bridge(bridge) = who else { return reply(Err(Error::Forbidden("the bridge protocol, but for bridges".into()))) };
        let Some(host) = super::host::current() else { return reply(Err(Error::Failed("loom is closing".into()))) };
        let method = params["method"].as_str().unwrap_or("").to_owned();
        let (answer, answered) = async_channel::bounded(1);
        host.request(bridge, &method, &params["params"], answer);
        crate::rt::spawn_local(async move {
            reply(Ok(answered.recv().await.unwrap_or_else(|_| json!({ "error": "loom is closing" }))));
        });
    });
    // Its events: `bridge/<name>`, for that bridge alone (may_read).
    hub.provide("bridge/", |_, name| matches!(name, "remote" | "telegram"));
    // Each bridge's state, for people (the pairing ticket, a device waiting to be confirmed).
    publish_states();
    hub.handle("pairing", |who, params| {
        if *who != Principal::Window {
            return Err(Error::Forbidden("pairing, but from loom's window".into()));
        }
        super::host::pair(params["open"].as_bool().unwrap_or(true));
        Ok(json!({}))
    });
    // The device asking to pair (the remote bridge's `candidate`), let in.
    hub.handle("pair_confirm", |who, params| {
        if *who != Principal::Window {
            return Err(Error::Forbidden("pairing, but from loom's window".into()));
        }
        // The one the user saw: a device that took its place meanwhile isn't let in.
        let asked = params["id"].as_str().ok_or_else(|| Error::Invalid("which device (the candidate's id)?".into()))?;
        let candidate = super::host::state_of("remote")["candidate"].clone();
        let (Some(id), Some(name)) = (candidate["id"].as_str(), candidate["name"].as_str()) else {
            return Err(Error::Conflict("no device is asking to pair".into()));
        };
        if asked != id {
            return Err(Error::Conflict("another device is asking to pair now".into()));
        }
        super::host::accept_device(id, name);
        Ok(json!({}))
    });
    hub.handle("bridge_state", |who, params| {
        let Principal::Bridge(bridge) = who else { return Err(Error::Forbidden("a bridge's state, but from that bridge".into())) };
        super::host::set_state(bridge, params.clone());
        Ok(json!({}))
    });
}

/// The API's `bridges`: `{id: "telegram" | "remote", state, …}` as each says it (or loom
/// knows), but the pairing ticket: that is `pairing`'s, the window's alone (whoever has it can
/// ask to pair first).
pub fn publish_states() {
    let hub = crate::api::hub();
    let mut ticket = None;
    let items = ["telegram", "remote"].iter().map(|name| {
        let mut state = super::host::state_of(name);
        if let Some(fields) = state.as_object_mut() {
            if let Some(taken) = fields.remove("ticket") {
                ticket = Some(taken);
            }
            // The remote bridge's own id (this computer's, on iroh): the item's id is its name.
            if let Some(node) = fields.remove("id") {
                fields.insert("node".into(), node);
            }
        }
        state["id"] = json!(name);
        state
    });
    hub.store("bridges").set(items.collect());
    let pairing = ticket.filter(|t| !t.is_null()).map(|t| json!({ "id": "remote", "ticket": t }));
    hub.store("pairing").set(pairing.into_iter().collect());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_and_bridges_save_only_bridges_say_their_state_and_agents_hear_nothing() {
        serve();
        let hub = crate::api::hub();
        let cli = hub.connect(Principal::Cli);
        assert!(matches!(cli.command("s1", "save", json!({ "agent": "99", "name": "a", "data": "" })), Err(Error::NotFound(_))));
        assert!(matches!(hub.connect(Principal::Agent("1".into())).command("s1a", "save", json!({ "agent": "1", "name": "a", "data": "" })), Err(Error::Forbidden(_))));
        assert!(matches!(cli.command("s2", "bridge_state", json!({ "state": "ok" })), Err(Error::Forbidden(_))));
        let agent = hub.connect(Principal::Agent("1".into()));
        assert!(matches!(agent.command("s3", "transcribe", json!({ "audio": "" })), Err(Error::Forbidden(_))));
        let bot = hub.connect(Principal::Bridge("telegram".into()));
        assert_eq!(bot.command("s4", "bridge_state", json!({ "state": "connected" })), Ok(json!({})));
        assert_eq!(crate::bridges::state_of("telegram"), json!({ "state": "connected" }));
        assert!(matches!(bot.command("s5", "save", json!({ "agent": "99", "name": "a", "data": "" })), Err(Error::NotFound(_))));
    }
}
