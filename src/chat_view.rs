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
    /// A long conversation's older entries, not drawn until asked for (their ids; drawn from
    /// `mirror`, so as they are by then).
    held: RefCell<std::collections::HashSet<String>>,
    me: RefCell<std::rc::Weak<Follow>>,
}

/// A conversation opens on its last this many of your messages (and what followed them).
const OPEN_ON: usize = 40;

impl Follow {
    /// Follows `resource` for `chat` (None: no such resource).
    pub fn start(resource: &str, of: Of, chat: &Rc<Chat>) -> Option<Rc<Follow>> {
        let follow = Rc::new(Follow { sub: RefCell::default(), chat: Rc::downgrade(chat), of, mirror: RefCell::default(), held: RefCell::default(), me: RefCell::default() });
        *follow.me.borrow_mut() = Rc::downgrade(&follow);
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
                // A snapshot (the first, or after a resync): what isn't drawn yet is; a long
                // conversation's first one, from its latest turns on.
                Body::Page { items } => {
                    let from = if matches!(self.of, Of::Conversation) && !chat.has_messages() && self.held.borrow().is_empty() { hold_from(&items) } else { 0 };
                    if from > 0 {
                        self.held.borrow_mut().extend(items[..from].iter().filter_map(|i| i["id"].as_str().map(str::to_owned)));
                        let weak = self.me.borrow().clone();
                        let count = items[..from].iter().filter(|i| matches!(i["kind"].as_str(), Some("user" | "agent"))).count();
                        chat.offer_earlier(count, Rc::new(move || {
                            if let Some(follow) = weak.upgrade() {
                                follow.draw_held();
                            }
                        }));
                    }
                    items[from..].iter().for_each(|item| self.item(&chat, item));
                }
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

    /// The older entries held back, drawn above the rest now.
    fn draw_held(&self) {
        let Some(chat) = self.chat.upgrade() else { return };
        let held = self.held.take();
        let items: Vec<Value> = self.mirror.borrow().items.iter().filter(|i| i["id"].as_str().is_some_and(|id| held.contains(id))).cloned().collect();
        chat.draw_earlier(|| items.iter().for_each(|item| self.item(&chat, item)));
    }

    /// Its items as they are now (a conversation: its entries).
    pub fn items(&self) -> Vec<Value> {
        self.mirror.borrow().items.clone()
    }

    fn item(&self, chat: &Chat, item: &Value) {
        // held back: drawn (as it is then) with the others
        if item["id"].as_str().is_some_and(|id| self.held.borrow().contains(id)) {
            return;
        }
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

/// Where a conversation's first drawing starts: at its OPEN_ON-th last message of yours (0:
/// it is short enough to draw whole).
fn hold_from(items: &[Value]) -> usize {
    let mine: Vec<usize> = items.iter().enumerate().filter(|(_, i)| i["kind"] == "user").map(|(at, _)| at).collect();
    if mine.len() <= OPEN_ON { 0 } else { mine[mine.len() - OPEN_ON] }
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


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_long_conversation_opens_on_its_last_messages_of_yours() {
        let turn = |n: usize| [json!({ "id": format!("u{n}"), "kind": "user" }), json!({ "id": format!("a{n}"), "kind": "agent" })];
        let short: Vec<Value> = (0..OPEN_ON).flat_map(turn).collect();
        assert_eq!(hold_from(&short), 0, "short enough: drawn whole");
        let long: Vec<Value> = (0..OPEN_ON + 5).flat_map(turn).collect();
        assert_eq!(long[hold_from(&long)]["id"], "u5", "from the 40th last message of yours");
    }
}
