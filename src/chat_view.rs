//! A chat following one of loomd's resources: its agent's conversation, or its usage.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use loom_core::api::{Body, Op};
use serde_json::Value;

use crate::chat::Chat;

/// What a chat follows in the core.
pub enum Of {
    /// Its agent's conversation: the text messages.
    Conversation,
    /// `usage`: its agent's item (by id) and the account's.
    Usage(String),
}

/// A chat following one of the core's resources.
pub struct Follow {
    sub: RefCell<Option<crate::conn::Sub>>,
    chat: Weak<Chat>,
    of: Of,
    /// What came, kept (a conversation's entries: what its questions and messages are).
    mirror: RefCell<loom_core::api::Mirror>,
}

impl Follow {
    /// Follows `resource` for `chat` (None: no such resource).
    pub fn start(resource: &str, of: Of, chat: &Rc<Chat>) -> Option<Rc<Follow>> {
        let follow = Rc::new(Follow { sub: RefCell::default(), chat: Rc::downgrade(chat), of, mirror: RefCell::default() });
        let weak = Rc::downgrade(&follow);
        let sub = crate::conn::subscribe(resource, move || {
            if let Some(follow) = weak.upgrade() {
                follow.update();
            }
        })?;
        *follow.sub.borrow_mut() = Some(sub);
        follow.update();
        Some(follow)
    }

    fn update(&self) {
        let Some(chat) = self.chat.upgrade() else { return };
        let messages = match self.sub.borrow().as_ref() {
            Some(sub) => sub.drain(),
            None => return,
        };
        for message in messages {
            self.mirror.borrow_mut().apply(&message);
            match message.body {
                // A snapshot (the first, or after a resync): what isn't drawn yet is.
                Body::Page { items } => items.iter().for_each(|item| self.item(&chat, item)),
                Body::Changes { ops } => {
                    for op in &ops {
                        match op {
                            Op::Upsert { item } => self.item(&chat, item),
                            Op::Append { id, field, text } if matches!(self.of, Of::Conversation) => chat.entry_append(id, field, text),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Its items as they are now (a conversation: its entries).
    pub fn items(&self) -> Vec<Value> {
        self.mirror.borrow().items.clone()
    }

    fn item(&self, chat: &Chat, item: &Value) {
        match &self.of {
            Of::Conversation => chat.entry(item),
            Of::Usage(agent) => match item["kind"].as_str() {
                Some("account") => chat.set_limits(limits_of(item)),
                Some("agent") if item["id"].as_str() == Some(agent.as_str()) => {
                    chat.set_usage(item["used"].as_u64().unwrap_or(0), item["size"].as_u64().unwrap_or(0));
                    if let Some(parts) = item["parts"].as_array() {
                        let parts = parts.iter().map(|p| (p["name"].as_str().unwrap_or_default().to_owned(), p["tokens"].as_u64().unwrap_or(0))).collect();
                        chat.set_context_parts(parts, item["compacts_at"].as_u64());
                    }
                    let (cost, limit) = loom_core::agent::spend_words(item);
                    chat.set_spend(cost, limit);
                }
                _ => {}
            },
        }
    }
}

/// The account item of `usage` as the chat's usage popover reads limits (times as Unix
/// seconds).
fn limits_of(item: &Value) -> Value {
    let windows: Vec<Value> = item["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|w| serde_json::json!({ "kind": w["kind"], "utilization": w["utilization"], "resetsAt": loom_core::api::seconds(&w["resets_at"]), "seenAt": loom_core::api::seconds(&w["seen_at"]) }))
        .collect();
    serde_json::json!({ "plan": item["plan"], "windows": windows })
}

