//! What travels on a subscription: the messages and the changes they carry. These are the
//! API's types, deliberately not the core's own (`research/core-api/00-decisions.md` is the
//! law they follow): readers ignore fields they don't know, and a `kind` or `op` they don't
//! know reads as `Unknown`, so that adding one never breaks an older client.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One message on a subscription. `seq` counts the subscription's messages from 1, with no
/// gaps: a client that sees one asks for a resync.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub sub: String,
    pub seq: u64,
    #[serde(flatten)]
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    /// Part of the resource as it is now, in order; pages follow each other until `synced`.
    Page { items: Vec<Value> },
    /// The snapshot is whole: what follows are changes to it.
    Synced,
    /// Changes, applied together and in order (never half of them).
    Changes { ops: Vec<Op> },
    /// Forget what you have: a fresh snapshot follows (`reason`: `behind`, `asked`…).
    Resync { reason: String },
    /// The resource is no more (an agent was closed); nothing follows.
    Gone,
    #[serde(other)]
    Unknown,
}

/// A change to a resource: an ordered list of items, each a JSON object with a string `id`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// The whole item: it replaces the one with its id, or is added at the end.
    Upsert { item: Value },
    Remove { id: String },
    /// Text added to the end of one of an item's string fields (streaming text).
    Append { id: String, field: String, text: String },
    /// The items' order: these ids first, in this order; ids not here keep theirs, after them.
    Order { ids: Vec<String> },
    #[serde(other)]
    Unknown,
}

/// An item's id (items without one are refused before they get anywhere).
pub fn id_of(item: &Value) -> Option<&str> {
    item.get("id")?.as_str()
}

/// Applies `op` to `items`, as the core and every client do. Ops about ids not there are
/// ignored: the item's next upsert, or a resync, brings it.
pub fn apply(items: &mut Vec<Value>, op: &Op) {
    let at = |items: &[Value], id: &str| items.iter().rposition(|it| id_of(it) == Some(id));
    match op {
        Op::Upsert { item } => {
            let Some(id) = id_of(item) else { return };
            match at(items, id) {
                Some(i) => items[i] = item.clone(),
                None => items.push(item.clone()),
            }
        }
        Op::Remove { id } => {
            if let Some(i) = at(items, id) {
                items.remove(i);
            }
        }
        Op::Append { id, field, text } => {
            if let Some(i) = at(items, id) {
                append_to(&mut items[i], field, text);
            }
        }
        Op::Order { ids } => {
            let mut first = Vec::with_capacity(items.len());
            for id in ids {
                if let Some(i) = at(items, id) {
                    first.push(items.remove(i));
                }
            }
            first.append(items);
            *items = first;
        }
        Op::Unknown => {}
    }
}

/// `field` of `item` with `text` added (set if it was missing or not text).
pub(crate) fn append_to(item: &mut Value, field: &str, text: &str) {
    let Some(map) = item.as_object_mut() else { return };
    match map.get_mut(field) {
        Some(Value::String(s)) => s.push_str(text),
        _ => {
            map.insert(field.to_string(), Value::String(text.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_read_as_they_are_written() {
        let m = Message { sub: "s1".into(), seq: 3, body: Body::Changes { ops: vec![Op::Append { id: "e1".into(), field: "text".into(), text: "hi".into() }] } };
        let text = serde_json::to_value(&m).unwrap();
        assert_eq!(text, json!({"sub": "s1", "seq": 3, "kind": "changes", "ops": [{"op": "append", "id": "e1", "field": "text", "text": "hi"}]}));
        assert_eq!(serde_json::from_value::<Message>(text).unwrap(), m);
    }

    #[test]
    fn what_a_reader_does_not_know_it_ignores() {
        let m: Message = serde_json::from_value(json!({"sub": "s1", "seq": 1, "kind": "hologram", "beam": true, "_meta": {}})).unwrap();
        assert_eq!(m.body, Body::Unknown);
        let m: Message = serde_json::from_value(json!({"sub": "s1", "seq": 2, "kind": "changes", "later": 1, "ops": [{"op": "twist", "id": "a"}, {"op": "remove", "id": "a", "why": "x"}]})).unwrap();
        assert_eq!(m.body, Body::Changes { ops: vec![Op::Unknown, Op::Remove { id: "a".into() }] });
    }

    #[test]
    fn ops_change_the_list() {
        let mut items = vec![json!({"id": "a"}), json!({"id": "b"}), json!({"id": "c"})];
        apply(&mut items, &Op::Upsert { item: json!({"id": "b", "n": 1}) });
        apply(&mut items, &Op::Upsert { item: json!({"id": "d"}) });
        apply(&mut items, &Op::Remove { id: "a".into() });
        apply(&mut items, &Op::Append { id: "d".into(), field: "text".into(), text: "he".into() });
        apply(&mut items, &Op::Append { id: "d".into(), field: "text".into(), text: "y".into() });
        apply(&mut items, &Op::Append { id: "nobody".into(), field: "text".into(), text: "lost".into() });
        apply(&mut items, &Op::Order { ids: vec!["d".into(), "missing".into(), "c".into()] });
        assert_eq!(items, vec![json!({"id": "d", "text": "hey"}), json!({"id": "c"}), json!({"id": "b", "n": 1})]);
    }
}
