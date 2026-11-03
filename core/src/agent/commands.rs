//! What clients of the core API may tell agents (warp): `send` a message, `answer` a question,
//! `stop` a turn. Each is checked against who sends it, its arguments included:
//!
//! - an agent (through its loom tools) does none of them;
//! - a bridge (the phone, Telegram) sends no `!` commands and never answers "Always allow";
//! - loom's window and the command-line client do everything.
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
}

/// Registers the commands (once; a client learns of them at `initialize`, agents or not).
pub fn serve() {
    if !SERVED.with(|s| s.replace(true)) {
        handle();
    }
}

fn handle() {
    let hub = crate::api::hub();
    hub.handle("send", |who, params| {
        let agent = target(who, params, "send messages")?;
        let text = params["text"].as_str().filter(|t| !t.trim().is_empty()).ok_or_else(|| Error::Invalid("send needs text".into()))?;
        if matches!(who, Principal::Bridge(_)) && text.trim_start().starts_with('!') {
            return Err(Error::Forbidden("`!` commands from a bridge".into()));
        }
        agent.submit_with(text.to_owned(), Vec::new());
        Ok(json!({}))
    });
    hub.handle("answer", |who, params| {
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
fn target(who: &Principal, params: &Value, what: &str) -> Result<Rc<Agent>, Error> {
    if matches!(who, Principal::Agent(_)) {
        return Err(Error::Forbidden(format!("an agent may not {what}")));
    }
    let id = params["agent"].as_str().ok_or_else(|| Error::Invalid("which agent?".into()))?;
    let agent = id.parse::<u32>().ok().and_then(|id| AGENTS.with_borrow(|agents| agents.get(&id).and_then(Weak::upgrade)));
    agent.ok_or_else(|| Error::NotFound(format!("agent {id}")))
}
