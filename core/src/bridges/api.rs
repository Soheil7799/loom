//! What the core API offers bridges beyond agents' commands (warp, step 4):
//!
//! - `save` (`{agent, name, data}`: base64): a file for the agent, written in its folder
//!   under `<bridge>-files/` (never elsewhere, never over another); `{path}` back. Bridges
//!   only.
//! - `transcribe` (`{audio, name?, mime?}`): speech to text with loom's settings (its key
//!   never leaves loom); `{text}` back once heard. Loom's window, the CLI and bridges.
//! - `bridge_state` (any object): what the bridge says of itself, shown in Settings. Bridges
//!   only, each its own.
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
        let Principal::Bridge(bridge) = who else { return reply(Err(Error::Forbidden("saving files, but for bridges".into()))) };
        let dir = match crate::agent::commands::target(who, params, "be sent files") {
            Ok(agent) => agent.cwd.join(format!("{bridge}-files")),
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
    hub.handle("bridge_state", |who, params| {
        let Principal::Bridge(bridge) = who else { return Err(Error::Forbidden("a bridge's state, but from that bridge".into())) };
        super::host::set_state(bridge, params.clone());
        Ok(json!({}))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bridges_save_and_say_their_state_and_agents_hear_nothing() {
        serve();
        let hub = crate::api::hub();
        let cli = hub.connect(Principal::Cli);
        assert!(matches!(cli.command("s1", "save", json!({ "agent": "1", "name": "a", "data": "" })), Err(Error::Forbidden(_))));
        assert!(matches!(cli.command("s2", "bridge_state", json!({ "state": "ok" })), Err(Error::Forbidden(_))));
        let agent = hub.connect(Principal::Agent("1".into()));
        assert!(matches!(agent.command("s3", "transcribe", json!({ "audio": "" })), Err(Error::Forbidden(_))));
        let bot = hub.connect(Principal::Bridge("telegram".into()));
        assert_eq!(bot.command("s4", "bridge_state", json!({ "state": "connected" })), Ok(json!({})));
        assert_eq!(crate::bridges::state_of("telegram"), json!({ "state": "connected" }));
        assert!(matches!(bot.command("s5", "save", json!({ "agent": "99", "name": "a", "data": "" })), Err(Error::NotFound(_))));
    }
}
