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
    hub.handle("stop", |who, params| {
        let agent = target(who, params, "stop agents")?;
        agent.cancel();
        Ok(json!({}))
    });
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
