//! A resource's state and its subscribers. The producer (an agent, the agent list) changes the
//! store; every change is applied at once and queued, and once the core's thread is idle the
//! queue goes out as one `changes` message per subscriber, so changes made together arrive
//! together. A subscriber whose unread messages pile up past its limit gets no backlog: its
//! queue is dropped for a resync and a fresh snapshot.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use serde_json::Value;

use super::wire::{self, Body, Message, Op, id_of};

/// What brings a store's items up to date before a snapshot.
type Refresh = Rc<dyn Fn(&Store)>;

/// How big things get before they are split or dropped.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Items in one snapshot page.
    pub page: usize,
    /// Unread ops a subscriber may have queued; past it, a resync replaces them.
    pub queue: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { page: 256, queue: 4096 }
    }
}

/// One resource: its items in order, and who follows them. Cloning gives another handle.
#[derive(Clone)]
pub struct Store(Rc<Inner>);

struct Inner {
    name: String,
    limits: Limits,
    /// Brings the items up to date before a snapshot (a resource worked out only while
    /// someone follows it).
    refresh: RefCell<Option<Refresh>>,
    /// Whether changes go out by themselves (on the core's thread, once idle) or only on
    /// `flush` (tests, which have no main loop).
    auto: bool,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    items: Vec<Value>,
    /// Changes not yet sent.
    pending: Vec<Op>,
    subs: Vec<Weak<SubInner>>,
    scheduled: bool,
    gone: bool,
}

impl Store {
    pub(crate) fn new(name: &str, limits: Limits, auto: bool) -> Store {
        Store(Rc::new(Inner { name: name.to_string(), limits, refresh: RefCell::default(), auto, state: RefCell::default() }))
    }

    /// `refresh` brings the items up to date whenever a subscriber is about to get them.
    pub fn on_subscribe(&self, refresh: impl Fn(&Store) + 'static) {
        *self.0.refresh.borrow_mut() = Some(Rc::new(refresh));
    }

    /// Brings the items up to date as for a new subscriber (what changed goes out).
    pub fn refresh(&self) {
        let refresh = self.0.refresh.borrow().clone();
        if let Some(refresh) = refresh {
            refresh(self);
        }
    }

    /// Whether anyone follows it.
    pub fn followed(&self) -> bool {
        self.0.state.borrow().subs.iter().any(|w| w.strong_count() > 0)
    }

    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// The items as they are now.
    pub fn items(&self) -> Vec<Value> {
        self.0.state.borrow().items.clone()
    }

    pub fn get(&self, id: &str) -> Option<Value> {
        self.0.state.borrow().items.iter().rfind(|it| id_of(it) == Some(id)).cloned()
    }

    fn has(&self, id: &str) -> bool {
        self.0.state.borrow().items.iter().rev().any(|it| id_of(it) == Some(id))
    }

    /// Adds `item` at the end, or replaces the one with its id. Items must be JSON objects
    /// with a string `id`; others are refused (and a debug build stops on them).
    pub fn upsert(&self, item: Value) {
        if id_of(&item).is_none() {
            debug_assert!(false, "api: an item with no id in {}: {item}", self.0.name);
            return;
        }
        self.change(Op::Upsert { item });
    }

    pub fn remove(&self, id: &str) {
        if self.has(id) {
            self.change(Op::Remove { id: id.to_string() });
        }
    }

    /// Adds `text` to the end of `field` (streaming text).
    pub fn append(&self, id: &str, field: &str, text: &str) {
        if !text.is_empty() && self.has(id) {
            self.change(Op::Append { id: id.to_string(), field: field.to_string(), text: text.to_string() });
        }
    }

    /// Puts these items first, in this order.
    pub fn order(&self, ids: Vec<String>) {
        self.change(Op::Order { ids });
    }

    /// Makes the items exactly `items`, sending only what differs (for resources worked out
    /// whole, such as the agent list).
    pub fn set(&self, items: Vec<Value>) {
        let now = self.items();
        let wanted: Vec<&str> = items.iter().filter_map(id_of).collect();
        for old in &now {
            if let Some(id) = id_of(old)
                && !wanted.contains(&id)
            {
                self.change(Op::Remove { id: id.to_string() });
            }
        }
        for item in items.iter().filter(|it| id_of(it).is_some()) {
            if !now.contains(item) {
                self.change(Op::Upsert { item: item.clone() });
            }
        }
        let same = self.0.state.borrow().items.iter().filter_map(id_of).eq(wanted.iter().copied());
        if !same {
            self.change(Op::Order { ids: wanted.iter().map(|s| s.to_string()).collect() });
        }
    }

    fn change(&self, op: Op) {
        let schedule = {
            let mut s = self.0.state.borrow_mut();
            if s.gone {
                return;
            }
            wire::apply(&mut s.items, &op);
            s.subs.retain(|w| w.strong_count() > 0);
            if s.subs.is_empty() {
                return; // nobody to tell; a subscriber starts from the snapshot
            }
            s.pending.push(op);
            let first = !s.scheduled;
            s.scheduled = true;
            first && self.0.auto
        };
        if schedule {
            let store = Rc::downgrade(&self.0);
            crate::rt::idle_once(move || {
                if let Some(inner) = store.upgrade() {
                    Store(inner).flush();
                }
            });
        }
    }

    /// Sends the queued changes now (the core's thread does this by itself once idle).
    pub fn flush(&self) {
        let (ops, subs) = {
            let mut s = self.0.state.borrow_mut();
            s.scheduled = false;
            s.subs.retain(|w| w.strong_count() > 0);
            (std::mem::take(&mut s.pending), s.subs.iter().filter_map(Weak::upgrade).collect::<Vec<_>>())
        };
        if ops.is_empty() {
            return;
        }
        let mut woken = Vec::new();
        for sub in subs {
            let mine = {
                let mut out = sub.out.borrow_mut();
                let skip = std::mem::take(&mut out.skip).min(ops.len());
                coalesce(&ops[skip..])
            };
            if mine.is_empty() {
                continue;
            }
            let count = mine.len();
            let over = {
                let mut out = sub.out.borrow_mut();
                out.queued += count;
                out.messages.push_back(Body::Changes { ops: mine });
                out.queued > self.0.limits.queue
            };
            if over {
                self.resync(&sub, "behind");
            }
            woken.push(sub);
        }
        for sub in woken {
            (sub.ready)();
        }
    }

    /// Replaces all the items at once (a conversation emptied, or read back from the start):
    /// subscribers get a resync (`reset`) and the new snapshot, not a remove for each item.
    pub fn reset(&self, items: Vec<Value>) {
        let subs: Vec<_> = {
            let mut s = self.0.state.borrow_mut();
            if s.gone {
                return;
            }
            s.items = items.into_iter().filter(|it| id_of(it).is_some()).collect();
            s.pending.clear();
            s.subs.retain(|w| w.strong_count() > 0);
            s.subs.iter().filter_map(Weak::upgrade).collect()
        };
        for sub in &subs {
            self.resync(sub, "reset");
        }
        for sub in subs {
            (sub.ready)();
        }
    }

    /// Replaces an item without telling anyone: for an item its followers keep up to date
    /// another way (a job's screen, which they keep by its bytes), made current for the next.
    pub fn refresh_item(&self, item: Value) {
        let Some(id) = id_of(&item).map(str::to_owned) else { return };
        let mut s = self.0.state.borrow_mut();
        match s.items.iter().rposition(|it| id_of(it) == Some(id.as_str())) {
            Some(i) => s.items[i] = item,
            None => s.items.push(item),
        }
    }

    /// Bytes added to item `id`'s byte stream at `offset`.
    pub fn bytes(&self, id: &str, offset: u64, bytes: &[u8]) {
        if !bytes.is_empty() && self.has(id) {
            self.change(Op::Bytes { id: id.to_string(), offset, data: wire::base64(bytes) });
        }
    }

    /// Ends the resource: subscribers get `gone`, and nothing more.
    pub fn close(&self) {
        let subs: Vec<_> = {
            let mut s = self.0.state.borrow_mut();
            s.gone = true;
            s.pending.clear();
            std::mem::take(&mut s.subs).iter().filter_map(Weak::upgrade).collect()
        };
        for sub in &subs {
            let mut out = sub.out.borrow_mut();
            out.messages.clear();
            out.queued = 0;
            out.messages.push_back(Body::Gone);
        }
        for sub in subs {
            (sub.ready)();
        }
    }

    /// Follows the store: the snapshot is waiting in the subscription when this returns;
    /// `ready` is called whenever more is (it should `drain` it, soon).
    pub(crate) fn subscribe(&self, id: String, ready: Box<dyn Fn()>) -> Subscription {
        let refresh = self.0.refresh.borrow().clone();
        if let Some(refresh) = refresh {
            refresh(self);
        }
        let sub = Rc::new(SubInner { id, store: Rc::downgrade(&self.0), out: RefCell::default(), ready });
        self.snapshot_into(&sub);
        self.0.state.borrow_mut().subs.push(Rc::downgrade(&sub));
        Subscription(sub)
    }

    fn resync(&self, sub: &SubInner, reason: &str) {
        {
            let mut out = sub.out.borrow_mut();
            out.messages.clear();
            out.queued = 0;
            out.messages.push_back(Body::Resync { reason: reason.to_string() });
        }
        self.snapshot_into(sub);
    }

    /// The items as pages, then `synced`. Changes already queued are in it: the
    /// subscriber's share of the next flush starts after them.
    fn snapshot_into(&self, sub: &SubInner) {
        let s = self.0.state.borrow();
        let mut out = sub.out.borrow_mut();
        out.skip = s.pending.len();
        for page in s.items.chunks(self.0.limits.page.max(1)) {
            out.messages.push_back(Body::Page { items: page.to_vec() });
        }
        if s.items.is_empty() {
            out.messages.push_back(Body::Page { items: Vec::new() });
        }
        out.messages.push_back(Body::Synced);
    }
}

/// Fewer ops saying the same: text appended to an item added or appended to just before
/// joins it (a streaming reply is then one op per flush, not one per chunk).
fn coalesce(ops: &[Op]) -> Vec<Op> {
    let mut out: Vec<Op> = Vec::with_capacity(ops.len());
    for op in ops {
        // A whole item replaces what was just sent of it (a tool's output, as it grows).
        if let Op::Upsert { item } = op
            && let Some(last) = out.last_mut()
            && match last {
                Op::Upsert { item: it } => id_of(it) == id_of(item),
                Op::Append { id, .. } => Some(id.as_str()) == id_of(item),
                _ => false,
            }
        {
            *last = op.clone();
            continue;
        }
        // Bytes that go on from the last ones: one op.
        if let Op::Bytes { id, offset, data } = op
            && let Some(Op::Bytes { id: last, offset: at, data: had }) = out.last_mut()
            && last == id
        {
            let mut joined = wire::bytes_of(had);
            if *at + joined.len() as u64 == *offset {
                joined.extend(wire::bytes_of(data));
                *had = wire::base64(&joined);
                continue;
            }
        }
        if let Op::Append { id, field, text } = op {
            match out.last_mut() {
                Some(Op::Append { id: last, field: f, text: t }) if last == id && f == field => {
                    t.push_str(text);
                    continue;
                }
                Some(Op::Upsert { item }) if id_of(item) == Some(id) => {
                    wire::append_to(item, field, text);
                    continue;
                }
                _ => {}
            }
        }
        out.push(op.clone());
    }
    out
}

/// A client's following of one store. Dropping it ends it.
pub struct Subscription(Rc<SubInner>);

struct SubInner {
    id: String,
    store: Weak<Inner>,
    out: RefCell<Outbox>,
    ready: Box<dyn Fn()>,
}

#[derive(Default)]
struct Outbox {
    messages: VecDeque<Body>,
    /// Ops waiting in `messages`.
    queued: usize,
    /// Messages handed out so far (the last one's `seq`).
    seq: u64,
    /// How many of the store's pending changes the snapshot already holds.
    skip: usize,
}

impl Subscription {
    pub fn id(&self) -> &str {
        &self.0.id
    }

    /// The messages waiting, numbered: `seq` is given here, when they are delivered, so
    /// what a resync dropped leaves no gap.
    pub fn drain(&self) -> Vec<Message> {
        let mut out = self.0.out.borrow_mut();
        out.queued = 0;
        let bodies: Vec<Body> = out.messages.drain(..).collect();
        bodies
            .into_iter()
            .map(|body| {
                out.seq += 1;
                Message { sub: self.0.id.clone(), seq: out.seq, body }
            })
            .collect()
    }

    /// Asks for a fresh snapshot (a client that saw a gap, or lost its copy).
    pub fn resync(&self) {
        if let Some(inner) = self.0.store.upgrade() {
            Store(inner).resync(&self.0, "asked");
            (self.0.ready)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    fn store() -> Store {
        Store::new("things", Limits { page: 2, queue: 5 }, false)
    }

    fn bodies(sub: &Subscription) -> Vec<Value> {
        sub.drain().into_iter().map(|m| serde_json::to_value(m).unwrap()).collect()
    }

    #[test]
    fn a_subscriber_gets_the_snapshot_in_pages_then_changes_together() {
        let s = store();
        for id in ["a", "b", "c"] {
            s.upsert(json!({"id": id}));
        }
        let rang = Rc::new(Cell::new(0));
        let r = rang.clone();
        let sub = s.subscribe("s1".into(), Box::new(move || r.set(r.get() + 1)));
        assert_eq!(
            bodies(&sub),
            vec![
                json!({"sub": "s1", "seq": 1, "kind": "page", "items": [{"id": "a"}, {"id": "b"}]}),
                json!({"sub": "s1", "seq": 2, "kind": "page", "items": [{"id": "c"}]}),
                json!({"sub": "s1", "seq": 3, "kind": "synced"}),
            ]
        );
        s.upsert(json!({"id": "d", "text": "he"}));
        s.append("d", "text", "l");
        s.append("d", "text", "lo");
        s.remove("a");
        s.remove("never");
        assert_eq!(rang.get(), 0, "nothing goes out before the flush");
        s.flush();
        assert_eq!(rang.get(), 1);
        assert_eq!(
            bodies(&sub),
            vec![json!({"sub": "s1", "seq": 4, "kind": "changes", "ops": [
                {"op": "upsert", "item": {"id": "d", "text": "hello"}},
                {"op": "remove", "id": "a"},
            ]})]
        );
        s.flush();
        assert_eq!(rang.get(), 1, "an empty flush wakes nobody");
    }

    #[test]
    fn an_empty_resource_is_one_empty_page() {
        let s = store();
        let sub = s.subscribe("s1".into(), Box::new(|| {}));
        assert_eq!(bodies(&sub), vec![json!({"sub": "s1", "seq": 1, "kind": "page", "items": []}), json!({"sub": "s1", "seq": 2, "kind": "synced"})]);
    }

    #[test]
    fn a_subscriber_joining_between_flushes_gets_each_change_once() {
        let s = store();
        let early = s.subscribe("s1".into(), Box::new(|| {}));
        early.drain();
        s.upsert(json!({"id": "e", "text": "a"}));
        s.append("e", "text", "b");
        let late = s.subscribe("s2".into(), Box::new(|| {}));
        s.append("e", "text", "c");
        s.flush();
        let mut mirror = super::super::Mirror::default();
        for m in late.drain() {
            mirror.apply(&m);
        }
        assert_eq!(mirror.items, vec![json!({"id": "e", "text": "abc"})]);
        let ops: Vec<Value> = early.drain().into_iter().map(|m| serde_json::to_value(m).unwrap()["ops"].clone()).collect();
        assert_eq!(ops, vec![json!([{"op": "upsert", "item": {"id": "e", "text": "abc"}}])]);
    }

    #[test]
    fn a_subscriber_that_falls_behind_gets_a_resync_not_a_backlog() {
        let s = store();
        let sub = s.subscribe("s1".into(), Box::new(|| {}));
        assert_eq!(sub.drain().len(), 2);
        for n in 0..4 {
            s.upsert(json!({"id": format!("x{n}")}));
            s.remove("nothing");
            s.flush();
        }
        // 4 ops waiting: under the limit of 5.
        assert_eq!(sub.drain().iter().map(|m| m.seq).collect::<Vec<_>>(), vec![3, 4, 5, 6]);
        for n in 0..6 {
            s.upsert(json!({"id": format!("y{n}")}));
            s.flush();
        }
        let got = bodies(&sub);
        assert_eq!(got[0], json!({"sub": "s1", "seq": 7, "kind": "resync", "reason": "behind"}));
        assert_eq!(got.last().unwrap()["kind"], "synced");
        let items: Vec<Value> = got.iter().filter(|m| m["kind"] == "page").flat_map(|m| m["items"].as_array().unwrap().clone()).collect();
        assert_eq!(items, s.items());
        let seqs: Vec<u64> = got.iter().map(|m| m["seq"].as_u64().unwrap()).collect();
        assert_eq!(seqs, (7..7 + got.len() as u64).collect::<Vec<_>>(), "no gap after a resync");
    }

    #[test]
    fn set_sends_only_the_difference() {
        let s = store();
        s.set(vec![json!({"id": "a", "n": 1}), json!({"id": "b"}), json!({"id": "c"})]);
        let sub = s.subscribe("s1".into(), Box::new(|| {}));
        sub.drain();
        s.set(vec![json!({"id": "c"}), json!({"id": "a", "n": 2}), json!({"id": "d"})]);
        s.flush();
        let ops = bodies(&sub)[0]["ops"].clone();
        assert_eq!(
            ops,
            json!([
                {"op": "remove", "id": "b"},
                {"op": "upsert", "item": {"id": "a", "n": 2}},
                {"op": "upsert", "item": {"id": "d"}},
                {"op": "order", "ids": ["c", "a", "d"]},
            ])
        );
        assert_eq!(s.items(), vec![json!({"id": "c"}), json!({"id": "a", "n": 2}), json!({"id": "d"})]);
        s.set(s.items());
        s.flush();
        assert!(sub.drain().is_empty(), "the same items again send nothing");
    }

    #[test]
    fn closing_tells_subscribers_and_dropping_unsubscribes() {
        let s = store();
        let sub = s.subscribe("s1".into(), Box::new(|| {}));
        let dropped = s.subscribe("s2".into(), Box::new(|| panic!("dropped, never woken")));
        drop(dropped);
        sub.drain();
        s.upsert(json!({"id": "a"}));
        s.close();
        s.upsert(json!({"id": "b"}));
        s.flush();
        assert_eq!(bodies(&sub), vec![json!({"sub": "s1", "seq": 3, "kind": "gone"})]);
    }

    #[test]
    fn bytes_that_follow_each_other_go_as_one_op_and_move_the_offset() {
        let s = store();
        s.upsert(json!({"id": "output", "screen": "", "offset": 4}));
        let sub = s.subscribe("s1".into(), Box::new(|| {}));
        sub.drain();
        s.bytes("output", 4, b"ab");
        s.bytes("output", 6, b"c");
        s.bytes("output", 9, b"z");
        s.flush();
        let ops = bodies(&sub)[0]["ops"].clone();
        assert_eq!(ops, json!([
            {"op": "bytes", "id": "output", "offset": 4, "data": "YWJj"},
            {"op": "bytes", "id": "output", "offset": 9, "data": "eg=="},
        ]));
        assert_eq!(s.get("output").unwrap()["offset"], 10);
        // Quietly made current for the next follower: the first is told nothing.
        s.refresh_item(json!({"id": "output", "screen": "c2NyZWVu", "offset": 10}));
        s.flush();
        assert!(sub.drain().is_empty());
    }

    #[test]
    fn asking_for_a_resync_sends_the_snapshot_again() {
        let s = store();
        s.upsert(json!({"id": "a"}));
        let sub = s.subscribe("s1".into(), Box::new(|| {}));
        sub.drain();
        sub.resync();
        let kinds: Vec<Value> = bodies(&sub).iter().map(|m| m["kind"].clone()).collect();
        assert_eq!(kinds, vec![json!("resync"), json!("page"), json!("synced")]);
    }
}
