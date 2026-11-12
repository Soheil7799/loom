//! The conversation as data: what an agent's view is told, kept for whoever reads it from
//! outside loom's window (the bridges' `history`). The view draws it; this remembers it.
//!
//! Entries, in order: `user` (`text`, `pictures`), `agent` (`text`, `message`: the agent's id
//! for the latest message in it) and `thought` (`text`), `tool`
//! (`id`, `title`, `tool`: its kind, `status`, `parent`: the task it runs under, `output`: the
//! end of what it printed, `exit`), `notice` (`text`, `class`, `button`: one to `press`), `question` (`text`, `detail`,
//! `options`: [{id, name, kind}] for a permission, `form`: the schema for a form; once
//! settled, `answer`) and `turn` (`seconds`: how long the turn took). Entries change as they stream in; each change is a new revision, so a reader
//! asks only for what changed after the last revision it saw. Revisions count within an
//! `epoch` (one per transcript, so per agent and per run of loom): from another, all of it.
//!
//! It is also the core API's `conversation/<agent>` resource: each entry published with an
//! `id` of its own (`e1`, `e2`…; a tool's call id is its `call`), the `turn_id` of the turn it
//! belongs to (a turn starts with the user's message) and a `status`: `streaming` while text
//! still comes, a tool's `pending`, `streaming`, `done` or `failed`, a question's
//! `awaiting_user` until answered (`done`) or dropped (`cancelled`); else `done`.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Instant;

use serde_json::{Value, json};

use super::form::FormModel;
use super::view::{Card, Crew, Form, Png, View, Workflow};
use crate::api::Store;
use crate::jobs::Id;
use crate::workflow::Launch;

/// How much of a command's output an entry keeps (its end).
const OUTPUT: usize = 16_000;
/// What the bridges' `history` gets of it (as before the core API kept more).
const BRIDGE_OUTPUT: usize = 4000;
/// A tool's input or content past this is not kept (a huge file written): only that it was.
const TOOL_PART: usize = 1 << 20;

#[derive(Default)]
pub struct Transcript {
    epoch: String,
    entries: Vec<(u64, Value)>,
    /// Bumped by every change.
    rev: u64,
    /// The revision it was last emptied at.
    cleared: u64,
    /// The entry a streamed piece of the same kind goes on, with the agent's message id.
    open: Option<(usize, &'static str, Option<String>)>,
    tools: HashMap<String, usize>,
    /// The message just sent from loom (its pictures follow it), until anything else comes.
    said: Option<usize>,
    /// Each entry's id, turn and status in the API, beside `entries`.
    meta: Vec<Meta>,
    /// Entries made so far: an id is never given twice, even after `clear`.
    made: u64,
    /// Turns begun so far; `replied`: something other than the user's message came in this
    /// one (the user's next message begins another).
    turns: u64,
    replied: bool,
    /// Where the conversation is published, once it is.
    store: Option<Store>,
}

struct Meta {
    id: String,
    turn: String,
    status: &'static str,
}

impl Transcript {
    /// Whether it holds anything.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// What changed after revision `after` of `epoch`: `{epoch, rev, reset, entries}`, each
    /// entry with its place (`i`). `reset`: drop what was kept, `entries` are all of it (it was
    /// emptied since, for a new conversation or one read back again, or `epoch` is not this
    /// one's).
    pub fn since(&self, epoch: &str, after: u64) -> Value {
        let reset = epoch != self.epoch || after < self.cleared || after > self.rev;
        let entries: Vec<Value> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, (rev, _))| reset || *rev > after)
            .map(|(i, (_, entry))| {
                let mut entry = entry.clone();
                entry["i"] = json!(i);
                // The bridges' history as it was: no diffs, inputs or picture ids, a short tail.
                if let Some(fields) = entry.as_object_mut() {
                    for field in ["content", "input", "locations", "images"] {
                        fields.remove(field);
                    }
                    if let Some(Value::String(output)) = fields.get_mut("output") {
                        tail(output, BRIDGE_OUTPUT);
                    }
                }
                entry
            })
            .collect();
        json!({ "epoch": self.epoch, "rev": self.rev, "reset": reset, "entries": entries })
    }

    fn bump(&mut self) -> u64 {
        self.rev += 1;
        self.rev
    }

    fn push(&mut self, entry: Value) -> usize {
        self.close_open();
        let kind = entry["kind"].as_str().unwrap_or("");
        if kind != "user" {
            self.replied = true;
        } else if self.replied || self.turns == 0 {
            self.turns += 1;
            self.replied = false;
        }
        let status = if matches!(kind, "agent" | "thought") { "streaming" } else { "done" };
        self.made += 1;
        self.meta.push(Meta { id: format!("e{}", self.made), turn: format!("t{}", self.turns), status });
        let rev = self.bump();
        self.entries.push((rev, entry));
        self.said = None;
        let i = self.entries.len() - 1;
        self.publish(i);
        i
    }

    /// A question for the user: its entry's id.
    fn question(&mut self, entry: Value) -> String {
        let i = self.push(entry);
        self.meta[i].status = "awaiting_user";
        self.publish(i);
        self.meta[i].id.clone()
    }

    /// The conversation as Markdown: what was said, by whom (tools and thoughts left out).
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        for (_, e) in &self.entries {
            let who = match e["kind"].as_str() {
                Some("user") => "You",
                Some("agent") => "Claude",
                _ => continue,
            };
            let text = e["text"].as_str().unwrap_or("").trim();
            if !text.is_empty() {
                out.push_str(&format!("## {who}\n\n{text}\n\n"));
            }
        }
        out
    }

    /// The agent's last words: its latest `agent` entry's text.
    pub fn last_reply(&self) -> Option<&str> {
        self.entries.iter().rev().find(|(_, e)| e["kind"] == "agent").and_then(|(_, e)| e["text"].as_str())
    }

    /// Question `id` settled: answered (`answer` "→ …") or dropped (any other words).
    fn settled(&mut self, id: &str, answer: &str) {
        let Some(i) = self.meta.iter().position(|m| m.id == id) else { return };
        self.meta[i].status = if answer.starts_with('→') { "done" } else { "cancelled" };
        let answer = answer.trim_start_matches('→').trim().to_owned();
        self.change(i, |entry| entry["answer"] = json!(answer));
    }

    /// The turn ended with nothing after it (the agent went to rest): what streamed is whole.
    pub fn settle(&mut self) {
        self.close_open();
        // A tool call read back with no result (stopped before it gave one) ran no more: it
        // is over, not pending for ever.
        let open: Vec<usize> = (0..self.entries.len()).filter(|&i| matches!(self.entries[i].1["status"].as_str(), Some("pending" | "in_progress")) && self.entries[i].1["kind"] == "tool").collect();
        for i in open {
            self.change(i, |e| e["status"] = json!("completed"));
        }
    }

    /// The entry text was streaming into is whole.
    fn close_open(&mut self) {
        if let Some((i, ..)) = self.open.take()
            && let Some(meta) = self.meta.get_mut(i)
            && meta.status == "streaming"
        {
            meta.status = "done";
            self.publish(i);
        }
    }

    fn change(&mut self, i: usize, change: impl FnOnce(&mut Value)) {
        self.touch(i, change);
        self.publish(i);
    }

    /// Changes entry `i` without publishing it (the caller tells the API what changed).
    fn touch(&mut self, i: usize, change: impl FnOnce(&mut Value)) {
        let rev = self.bump();
        if let Some((at, entry)) = self.entries.get_mut(i) {
            *at = rev;
            change(entry);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.meta.clear();
        self.tools.clear();
        self.open = None;
        self.replied = true;
        self.cleared = self.bump();
        if let Some(store) = &self.store {
            store.reset(Vec::new());
        }
    }

    /// Entry `i` as the API gives it.
    fn item(&self, i: usize) -> Option<Value> {
        let (_, entry) = self.entries.get(i)?;
        let meta = self.meta.get(i)?;
        let mut item = entry.clone();
        let map = item.as_object_mut()?;
        let call = map.remove("id");
        let status = match map.remove("status") {
            // A tool's, as the agent reports it (pending, in_progress, completed, failed).
            Some(Value::String(status)) => match status.as_str() {
                "in_progress" => "streaming",
                "completed" => "done",
                "failed" => "failed",
                _ => "pending",
            },
            _ => meta.status,
        };
        map.insert("id".into(), json!(meta.id));
        map.insert("turn_id".into(), json!(meta.turn));
        map.insert("status".into(), json!(status));
        if let Some(call) = call {
            map.insert("call".into(), call);
        }
        Some(item)
    }

    fn publish(&self, i: usize) {
        if let (Some(store), Some(item)) = (&self.store, self.item(i)) {
            store.upsert(item);
        }
    }

    /// Publishes the conversation in `store` from now on (all of it, then each change).
    pub fn publish_to(&mut self, store: Store) {
        store.reset((0..self.entries.len()).filter_map(|i| self.item(i)).collect());
        self.store = Some(store);
    }

    /// A piece of a message streaming in, onto the last one if it is of the same kind.
    fn chunk(&mut self, kind: &'static str, text: &str, id: Option<&str>) {
        match self.open.clone() {
            Some((i, open, said)) if open == kind => {
                // Another message of the agent's: a paragraph of its own (as the view shows it).
                let another = id.is_some() && said.is_some() && said.as_deref() != id;
                let mut added = String::new();
                self.touch(i, |entry| {
                    if let Value::String(all) = &mut entry["text"] {
                        if another && !all.ends_with("\n\n") {
                            added.push_str("\n\n");
                        }
                        added.push_str(text);
                        all.push_str(&added);
                    }
                });
                if let (Some(store), Some(meta)) = (&self.store, self.meta.get(i)) {
                    store.append(&meta.id, "text", &added);
                }
                if let Some(id) = id
                    && self.entries[i].1["message"].as_str() != Some(id)
                {
                    self.change(i, |entry| entry["message"] = json!(id));
                }
                if id.is_some() {
                    self.open = Some((i, kind, id.map(str::to_owned)));
                }
            }
            _ => {
                let mut entry = json!({ "kind": kind, "text": text });
                if let Some(id) = id {
                    entry["message"] = json!(id);
                }
                let i = self.push(entry);
                self.open = Some((i, kind, id.map(str::to_owned)));
            }
        }
    }

    /// Pictures sent (their ids in the core API's blobs: `images`, and how many: `pictures`).
    fn pictures(&mut self, images: Vec<String>) {
        let count = images.len();
        if count == 0 {
            return;
        }
        // On the message they were sent with, or the one being read back; else their own.
        let reading = match self.open {
            Some((i, "user", _)) => Some(i),
            _ => None,
        };
        match self.said.take().or(reading) {
            Some(i) => self.change(i, |entry| {
                entry["pictures"] = json!(entry["pictures"].as_u64().unwrap_or(0) + count as u64);
                let mut all = entry["images"].as_array().cloned().unwrap_or_default();
                all.extend(images.into_iter().map(Value::String));
                entry["images"] = Value::Array(all);
            }),
            None => {
                self.push(json!({ "kind": "user", "text": "", "pictures": count, "images": images }));
            }
        }
    }

    fn tool(&mut self, update: &Value) {
        let Some(id) = update["toolCallId"].as_str() else { return };
        let i = match self.tools.get(id) {
            Some(&i) => i,
            None => {
                let parent = update["_meta"]["claudeCode"]["parentToolUseId"].as_str();
                let i = self.push(json!({ "kind": "tool", "id": id, "title": "", "tool": "", "status": "", "parent": parent }));
                self.tools.insert(id.to_owned(), i);
                i
            }
        };
        self.change(i, |entry| {
            for (field, from) in [("title", "title"), ("tool", "kind"), ("status", "status")] {
                if let Some(value) = update[from].as_str() {
                    // a title for people: "mcp__loom__loom_note_search" as "loom · note search"
                    entry[field] = json!(if field == "title" { super::tool_title(value) } else { value.to_owned() });
                }
            }
            // What the agent gave it (`input`), what it shows (`content`: diffs, text, a
            // terminal) and where (`locations`), as the driver reports them.
            // A subagent's launch card: its subagent (whose conversation a client opens).
            if let Some(sub) = update["subagent"].as_str() {
                entry["subagent"] = json!(sub);
            }
            for (field, from) in [("input", "rawInput"), ("content", "content"), ("locations", "locations")] {
                let value = &update[from];
                if !value.is_null() {
                    entry[field] = if value.to_string().len() > TOOL_PART { json!({ "too_large": true }) } else { value.clone() };
                }
            }
        });
    }

    fn tool_output(&mut self, id: &str, data: &str) {
        let Some(&i) = self.tools.get(id) else { return };
        self.change(i, |entry| {
            let mut output = entry["output"].as_str().unwrap_or("").to_owned();
            output.push_str(data);
            if output.len() > OUTPUT {
                tail(&mut output, OUTPUT);
            }
            entry["output"] = json!(output);
        });
    }

    fn tool_exit(&mut self, id: &str, code: Option<i64>) {
        if let Some(&i) = self.tools.get(id) {
            self.change(i, |entry| entry["exit"] = json!(code));
        }
    }

    /// Tool `id` to be shown open (a `!command`'s output): `opened` grows each time.
    fn open_tool(&mut self, id: &str) {
        if let Some(&i) = self.tools.get(id) {
            self.change(i, |entry| entry["opened"] = json!(entry["opened"].as_u64().unwrap_or(0) + 1));
        }
    }
}

/// `text` cut to its last `keep` bytes (at a character's start).
fn tail(text: &mut String, keep: usize) {
    if text.len() > keep {
        let mut cut = text.len() - keep;
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        text.drain(..cut);
    }
}

/// An agent's view that also keeps its conversation in a `Transcript`: everything else goes on
/// to the view it wraps untouched.
pub struct Recorded {
    pub transcript: Rc<RefCell<Transcript>>,
    /// Its usage as last told: `{used, size}` of its context, `cost` (as "$0.42").
    pub usage: RefCell<Value>,
    /// Its id in the core API's `usage`, once published.
    agent: RefCell<Option<String>>,
    /// The buttons of notices that have one, by entry (`press`); a choice's goes once pressed.
    actions: RefCell<std::collections::HashMap<String, (Rc<dyn Fn()>, bool)>>,
    /// Its slash commands as the agent gave them (name, what it does), and the next message
    /// it suggests: for a client's composer (`inputs`).
    pub commands: RefCell<Vec<(String, String)>>,
    pub suggestion: RefCell<Option<String>>,
    /// Bumped each time it opens a job for the user (`jobs_shown`).
    shown: std::cell::Cell<u64>,
    /// What is in its input, not sent yet.
    draft: RefCell<String>,
    /// Pictures of messages taken back (blob ids), for the input to attach again.
    draft_images: RefCell<Vec<String>>,
}

/// The core API's `usage` resource: the account's limits (`{id: "account", kind: "account",
/// plan, windows: [{kind, utilization, resets_at, seen_at}]}`) and each agent's
/// (`{id: <agent>, kind: "agent", used, size, cost: {amount, currency}, limit: {kind,
/// utilization, resets_at}}`: its context's tokens, what the conversation cost, and the
/// account window Claude Code last reported with it). A share is 0.33 for 33%.
const USAGE: &str = "usage";

/// The account's limits (as merged here) as `usage`'s account item.
fn account_item(limits: &Value) -> Value {
    let windows: Vec<Value> = limits["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|w| {
            json!({
                "kind": w["kind"],
                "utilization": w["utilization"],
                "resets_at": w["resetsAt"].as_i64().map_or(Value::Null, crate::api::time),
                "seen_at": w["seenAt"].as_i64().map_or(Value::Null, crate::api::time),
            })
        })
        .collect();
    json!({ "id": "account", "kind": "account", "plan": limits["plan"], "windows": windows })
}

impl Drop for Recorded {
    fn drop(&mut self) {
        if let Some(store) = &self.transcript.borrow().store {
            crate::api::close(store.name());
        }
        if let Some(id) = self.agent.get_mut().take() {
            crate::api::remove_item(USAGE, &id);
        }
    }
}

impl Recorded {
    /// Agent `id`'s: published in the core API, as `conversation/<id>` and in `usage`.
    pub fn publish(&self, id: u32) {
        self.transcript.borrow_mut().publish_to(crate::api::hub().store(&super::conversation(id)));
        *self.agent.borrow_mut() = Some(id.to_string());
        // its subagents' list there from the start (empty), to follow before any comes
        crate::api::hub().store(&format!("crew/{id}"));
        crate::api::hub().store(&format!("plan/{id}"));
        crate::api::hub().store(&format!("workflows/{id}"));
        // shared by every agent, there from the first one on
        crate::api::hub().store("mcp");
        crate::api::hub().store("jobs_shown");
        crate::api::hub().store("uris_shown");
        let usage = crate::api::hub().store(USAGE);
        let account = account_limits();
        if !account.is_null() && usage.get("account").is_none() {
            usage.upsert(account_item(&account));
        }
    }

    /// A notice with a button: `button` in its entry, pressed through the API (`press`).
    fn with_button(&self, text: &str, button: &str, action: Rc<dyn Fn()>, once: bool) {
        let mut transcript = self.transcript.borrow_mut();
        let i = transcript.push(json!({ "kind": "notice", "text": text, "class": "", "button": button }));
        let id = transcript.meta[i].id.clone();
        self.actions.borrow_mut().insert(id, (action, once));
    }

    /// Presses the button of notice `entry`; false if it has none (any more). A choice's
    /// button goes from its entry once pressed (as the window's does).
    pub fn press(&self, entry: &str) -> bool {
        let Some((action, once)) = self.actions.borrow().get(entry).cloned() else { return false };
        if once {
            self.actions.borrow_mut().remove(entry);
            let mut transcript = self.transcript.borrow_mut();
            if let Some(i) = transcript.meta.iter().position(|m| m.id == entry) {
                transcript.change(i, |e| e["button"] = Value::Null);
            }
        }
        action();
        true
    }

    /// Its subagents in the core API (`crew/<agent>`), once it is published.
    fn crew(&self) -> Option<Store> {
        self.agent.borrow().as_ref().map(|id| crate::api::hub().store(&format!("crew/{id}")))
    }

    /// A usage report from the agent (`usage_update`): its item in `usage`.
    pub fn usage_update(&self, update: &Value) {
        let Some(id) = self.agent.borrow().clone() else { return };
        let info = &update["_meta"]["_claude/rateLimit"];
        let limit = if info["utilization"].is_number() {
            json!({ "kind": info["rateLimitType"], "utilization": info["utilization"], "resets_at": info["resetsAt"].as_i64().map_or(Value::Null, crate::api::time) })
        } else {
            Value::Null
        };
        let cost = if update["cost"]["amount"].is_number() { json!({ "amount": update["cost"]["amount"], "currency": update["cost"]["currency"].as_str().unwrap_or("USD") }) } else { Value::Null };
        let store = crate::api::hub().store(USAGE);
        // What fills its context, as last told, kept.
        let parts = store.get(&id).map(|old| (old["parts"].clone(), old["compacts_at"].clone())).unwrap_or_default();
        store.upsert(json!({ "id": id, "kind": "agent", "used": update["used"].as_u64().unwrap_or(0), "size": update["size"].as_u64().unwrap_or(0), "cost": cost, "limit": limit, "parts": parts.0, "compacts_at": parts.1 }));
    }

    /// A message is being sent from loom: pictures that follow belong to it, not to the last.
    pub fn message_starts(&self) {
        self.transcript.borrow_mut().said = None;
    }

    pub fn new() -> Recorded {
        let transcript = Transcript { epoch: crate::rt::uuid(), ..Transcript::default() };
        Recorded { transcript: Rc::new(RefCell::new(transcript)), usage: RefCell::new(json!({})), agent: RefCell::default(), actions: RefCell::default(), commands: RefCell::default(), suggestion: RefCell::default(), shown: std::cell::Cell::new(0), draft: RefCell::default(), draft_images: RefCell::default() }
    }

    /// Messages taken back: before what is in the draft, a blank line between each (as the
    /// window's input has them).
    fn put_back(&self, messages: Vec<(String, Vec<Png>)>) {
        let mut parts: Vec<String> = messages.iter().map(|(t, _)| t).filter(|t| !t.trim().is_empty()).cloned().collect();
        let draft = self.draft.borrow().clone();
        if !draft.trim().is_empty() {
            parts.push(draft);
        }
        *self.draft.borrow_mut() = parts.join("\n\n");
        // their pictures too: kept as blobs, for the input to attach again
        let images = messages.into_iter().flat_map(|(_, images)| images).map(crate::api::blobs::put);
        self.draft_images.borrow_mut().extend(images);
    }
}

impl Default for Recorded {
    fn default() -> Recorded {
        Recorded::new()
    }
}

/// A question's card that also settles its entry in the conversation.
struct RecordedCard {
    transcript: Weak<RefCell<Transcript>>,
    entry: String,
}

impl Card for RecordedCard {
    fn resolve(&self, answer: &str) {
        if let Some(transcript) = self.transcript.upgrade() {
            transcript.borrow_mut().settled(&self.entry, answer);
        }
    }
    fn entry(&self) -> Option<String> {
        Some(self.entry.clone())
    }
}

/// A question form, filled in by the API's `answer` (whole) or step by step; its card settles
/// its entry.
struct RecordedForm {
    model: RefCell<FormModel>,
    answer: Rc<dyn Fn(Option<Value>)>,
    card: Rc<dyn Card>,
}

impl Form for RecordedForm {
    fn card(&self) -> Rc<dyn Card> {
        self.card.clone()
    }
    fn content(&self) -> Result<Value, String> {
        self.model.borrow().content()
    }
    fn checked(&self) -> Option<Value> {
        self.content().ok()
    }
    fn summary(&self) -> String {
        self.model.borrow().summary()
    }
    fn choices(&self) -> Vec<(String, Vec<String>, bool, Vec<bool>)> {
        self.model.borrow().choices()
    }
    fn pick_in(&self, field: usize, option: usize) -> bool {
        self.model.borrow_mut().pick_in(field, option).is_some()
    }
    fn writable(&self) -> bool {
        self.model.borrow().writable()
    }
    fn write(&self, text: &str) -> bool {
        self.model.borrow_mut().write(text).is_some()
    }
    fn pick(&self, n: usize) -> bool {
        if !self.pick_in(0, n) {
            return false;
        }
        let Ok(content) = self.content() else { return false };
        (self.answer)(Some(content));
        true
    }
}

thread_local! {
    /// The account's limits as Claude Code last told them (any agent): `{plan, windows}`.
    static ACCOUNT: RefCell<Value> = const { RefCell::new(Value::Null) };
}

/// The account's limits as reported so far, `new` on top of `old`: Claude Code reports one
/// window at a time (the one closest to its limit), so each kind keeps its last report, with
/// when it came (`seenAt`), until its reset time passes. The 5-hour window first, then the weekly
/// ones, as reported.
pub fn merge_limits(old: &Value, new: &Value, now: i64) -> Value {
    let mut windows: Vec<Value> = old["windows"].as_array().cloned().unwrap_or_default();
    // When Claude Code said it (an agent resends what it last heard with every update).
    let at = new["at"].as_i64().unwrap_or(now);
    for window in new["windows"].as_array().into_iter().flatten() {
        let mut window = window.clone();
        window["seenAt"] = json!(at);
        match windows.iter().position(|w| w["kind"] == window["kind"]) {
            Some(i) if windows[i]["seenAt"].as_i64().unwrap_or(0) <= at => windows[i] = window,
            Some(_) => {}
            None => windows.push(window),
        }
    }
    windows.retain(|w| w["resetsAt"].as_i64().is_none_or(|at| at > now));
    windows.sort_by_key(|w| match w["kind"].as_str() {
        Some("five_hour") => 0,
        Some("seven_day") => 1,
        _ => 2,
    });
    let plan = if new["plan"].is_null() { old["plan"].clone() } else { new["plan"].clone() };
    if windows.is_empty() && plan.is_null() {
        return Value::Null;
    }
    json!({ "plan": plan, "windows": windows })
}

/// The account's limits (`{plan, windows: [{kind, utilization, resetsAt, seenAt}]}`), null
/// until an agent's answer brought them.
pub fn account_limits() -> Value {
    let now = crate::state::now() as i64;
    ACCOUNT.with_borrow(|account| merge_limits(account, &Value::Null, now))
}

impl View for Recorded {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clear(&self) {
        self.transcript.borrow_mut().clear();
        // the notices are gone, their buttons with them
        self.actions.borrow_mut().clear();
    }
    fn has_messages(&self) -> bool {
        self.transcript.borrow().entries.iter().any(|(_, e)| e["kind"] == "user" || e["kind"] == "agent")
    }
    /// What the agent said since its last tool call in this turn (none yet: None).
    fn last_reply(&self) -> Option<String> {
        let transcript = self.transcript.borrow();
        let mut parts: Vec<&str> = transcript.entries.iter().rev().take_while(|(_, e)| e["kind"] != "tool" && e["kind"] != "user").filter(|(_, e)| e["kind"] == "agent").filter_map(|(_, e)| e["text"].as_str()).collect();
        parts.reverse();
        (!parts.is_empty()).then(|| parts.concat())
    }
    fn user(&self, text: &str) {
        let mut transcript = self.transcript.borrow_mut();
        let i = transcript.push(json!({ "kind": "user", "text": text }));
        transcript.said = Some(i);
        drop(transcript);
    }
    fn user_chunk(&self, text: &str) {
        self.transcript.borrow_mut().chunk("user", text, None);
    }
    fn user_images(&self, pngs: &[Png]) {
        self.transcript.borrow_mut().pictures(pngs.iter().map(|png| crate::api::blobs::put(png.clone())).collect());
    }
    fn user_image_data(&self, base64: &str) {
        let png = Png::from(crate::api::wire::bytes_of(base64.trim()));
        self.transcript.borrow_mut().pictures(vec![crate::api::blobs::put(png)]);
    }
    fn agent_text_with(&self, text: &str, id: Option<&str>) {
        self.transcript.borrow_mut().chunk("agent", text, id);
    }
    fn thought(&self, text: &str) {
        self.transcript.borrow_mut().chunk("thought", text, None);
    }
    fn notice(&self, text: &str, class: &str) {
        self.transcript.borrow_mut().push(json!({ "kind": "notice", "text": text, "class": class }));
    }
    fn notice_action(&self, text: &str, button: &str, action: Rc<dyn Fn()>) {
        self.with_button(text, button, action.clone(), false);
    }
    fn notice_choice(&self, text: &str, button: &str, action: Rc<dyn Fn()>) {
        self.with_button(text, button, action.clone(), true);
    }
    fn tool(&self, update: &Value) {
        self.transcript.borrow_mut().tool(update);
    }
    fn tool_output(&self, id: &str, data: &str) {
        self.transcript.borrow_mut().tool_output(id, data);
    }
    fn tool_exit(&self, id: &str, code: Option<i64>) {
        self.transcript.borrow_mut().tool_exit(id, code);
    }
    fn open_tool(&self, id: &str) {
        self.transcript.borrow_mut().open_tool(id);
    }
    fn set_plan(&self, entries: &Value) {
        if let Some(id) = self.agent.borrow().as_ref() {
            let items = entries.as_array().into_iter().flatten().enumerate().map(|(i, e)| json!({ "id": i.to_string(), "content": e["content"], "status": e["status"], "priority": e["priority"] })).collect();
            crate::api::hub().store(&format!("plan/{id}")).set(items);
        }
    }
    fn turn_done(&self, seconds: u64) {
        self.transcript.borrow_mut().push(json!({ "kind": "turn", "seconds": seconds }));
    }
    fn turn_undo(&self, undo: Rc<dyn Fn()>) {
        // for a client: a notice with its button (`press`), the window's own aside
        self.with_button("This turn changed files.", "Undo its changes to files", undo.clone(), true);
    }
    fn permission(&self, question: &str, detail: Option<&str>, markdown: bool, options: &[(String, String, String)], _choose: Rc<dyn Fn(String)>) -> Rc<dyn Card> {
        let options_json: Vec<Value> = options.iter().map(|(id, name, kind)| json!({ "id": id, "name": name, "kind": kind })).collect();
        let entry = self.transcript.borrow_mut().question(json!({ "kind": "question", "text": question, "detail": detail, "markdown": markdown, "options": options_json }));
        Rc::new(RecordedCard { transcript: Rc::downgrade(&self.transcript), entry })
    }
    fn form(&self, message: &str, schema: &Value, answer: Rc<dyn Fn(Option<Value>)>) -> Rc<dyn Form> {
        let entry = self.transcript.borrow_mut().question(json!({ "kind": "question", "text": message, "form": schema }));
        let card = Rc::new(RecordedCard { transcript: Rc::downgrade(&self.transcript), entry });
        Rc::new(RecordedForm { model: RefCell::new(FormModel::from_schema(schema)), answer, card })
    }
    fn workflow(&self, tool_id: &str, launch: Launch, live: bool, _alive: Box<dyn Fn() -> bool>, _on_changed: Rc<dyn Fn()>, _on_stop: Rc<dyn Fn()>) -> Rc<dyn Workflow> {
        let store = self.agent.borrow().as_ref().map(|id| crate::api::hub().store(&format!("workflows/{id}")));
        let progress = Rc::new(RefCell::new(crate::workflow::Progress::new(&launch, live)));
        let recorded = Rc::new(RecordedWorkflow { tool_id: tool_id.to_owned(), progress, store, launch, unanswered: std::cell::Cell::new(0) });
        recorded.publish();
        // followed from its files while it runs, as the window's card is
        let weak = Rc::downgrade(&recorded);
        let _ = crate::rt::every(std::time::Duration::from_secs(2), move || {
            let Some(w) = weak.upgrade() else { return false };
            w.progress.borrow_mut().poll(true);
            w.publish();
            !w.progress.borrow().done()
        });
        recorded
    }

    fn set_commands(&self, commands: Vec<(String, String)>) {
        *self.commands.borrow_mut() = commands.clone();
    }
    fn set_suggestion(&self, text: Option<String>) {
        *self.suggestion.borrow_mut() = text.clone();
    }
    fn set_usage(&self, used: u64, size: u64) {
        {
            let mut usage = self.usage.borrow_mut();
            usage["used"] = json!(used);
            usage["size"] = json!(size);
        }
    }
    fn set_spend(&self, cost: Option<String>, _limit: Option<(String, String)>) {
        self.usage.borrow_mut()["cost"] = json!(cost);
    }
    fn set_limits(&self, limits: Value) {
        let merged = ACCOUNT.with_borrow_mut(|account| {
            *account = merge_limits(account, &limits, crate::state::now() as i64);
            account.clone()
        });
        if !merged.is_null() {
            crate::api::hub().store(USAGE).upsert(account_item(&merged));
        }
    }
    fn set_context_parts(&self, parts: Vec<(String, u64)>, compacts_at: Option<u64>) {
        // In its `usage` item too (a client's context popover).
        if let Some(id) = self.agent.borrow().clone() {
            let store = crate::api::hub().store(USAGE);
            let mut item = store.get(&id).unwrap_or_else(|| json!({ "id": id, "kind": "agent", "used": 0, "size": 0 }));
            item["parts"] = parts.iter().map(|(name, tokens)| json!({ "name": name, "tokens": tokens })).collect();
            item["compacts_at"] = json!(compacts_at);
            store.upsert(item);
        }
    }
    fn context_used(&self) -> u64 {
        self.usage.borrow()["used"].as_u64().unwrap_or(0)
    }
    fn crew_set(&self, entry: Crew) {
        if let Some(store) = self.crew() {
            store.upsert(crew_item(&entry));
        }
    }
    fn crew_remove(&self, key: &str) {
        if let Some(store) = self.crew() {
            store.remove(key);
        }
    }
    fn crew_clear(&self) {
        if let Some(store) = self.crew() {
            store.set(Vec::new());
        }
    }
    fn show_mcp(&self, servers: Option<Vec<(String, String, Option<String>)>>) {
        if let Some(id) = self.agent.borrow().as_ref() {
            let list: Vec<Value> = servers.iter().flatten().map(|(name, status, error)| json!({ "name": name, "status": status, "error": error })).collect();
            crate::api::hub().store("mcp").upsert(json!({ "id": id, "servers": list }));
        }
    }

    fn draft(&self) -> String {
        self.draft.borrow().clone()
    }
    fn set_draft(&self, text: &str) {
        *self.draft.borrow_mut() = text.to_owned();
        // the input took what was put back (its pictures are attached there now)
        self.draft_images.borrow_mut().clear();
    }
    fn draft_images(&self) -> Vec<String> {
        self.draft_images.borrow().clone()
    }
    fn unqueue(&self, text: &str, images: Vec<Png>) {
        self.put_back(vec![(text.to_owned(), images)]);
    }
    fn restore_unsent(&self, messages: Vec<(String, Vec<Png>)>) {
        self.put_back(messages);
    }

    // A subagent's conversation: published beside its agent's (`subagent/<agent>/<id>`), for
    // a client to read (loomd's window).
    // (closed with their agent: `close_resources`)
    fn sub_view(&self, id: &str) -> Rc<dyn View> {
        let recorded = Recorded::new();
        let Some(agent) = self.agent.borrow().clone() else { return Rc::new(recorded) };
        recorded.transcript.borrow_mut().publish_to(crate::api::hub().store(&format!("subagent/{agent}/{id}")));
        Rc::new(recorded)
    }
    fn open_uri(&self, uri: &str) {
        // a page an MCP server asked to open (accepted by the user): a client opens it
        if let Some(agent) = self.agent.borrow().as_ref() {
            self.shown.set(self.shown.get() + 1);
            crate::api::hub().store("uris_shown").upsert(json!({ "id": agent, "uri": uri, "n": self.shown.get() }));
        }
    }
    fn show_job(&self, id: Id) {
        // the job it opened for the user (a login): a client brings it forward
        if let Some(agent) = self.agent.borrow().as_ref() {
            self.shown.set(self.shown.get() + 1);
            crate::api::hub().store("jobs_shown").upsert(json!({ "id": agent, "job": id.to_string(), "n": self.shown.get() }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::view::View;

    #[test]
    fn a_conversation_exports_as_what_was_said() {
        let mut t = Transcript::default();
        t.push(json!({ "kind": "user", "text": "Hi " }));
        t.push(json!({ "kind": "tool", "title": "Read" }));
        t.push(json!({ "kind": "agent", "text": "Hello." }));
        t.push(json!({ "kind": "agent", "text": "" }));
        assert_eq!(t.markdown(), "## You\n\nHi\n\n## Claude\n\nHello.\n\n");
    }

    #[test]
    fn a_notices_button_is_pressed_once_through_the_api() {
        let recorded = Recorded::new();
        let pressed = Rc::new(std::cell::Cell::new(0));
        let count = pressed.clone();
        recorded.notice_choice("Rewind with the files?", "Rewind", Rc::new(move || count.set(count.get() + 1)));
        let id = recorded.transcript.borrow().meta[0].id.clone();
        assert_eq!(recorded.transcript.borrow().item(0).unwrap()["button"], "Rewind");
        assert!(recorded.press(&id));
        assert_eq!(pressed.get(), 1);
        assert!(recorded.transcript.borrow().item(0).unwrap()["button"].is_null(), "a choice's button goes");
        assert!(!recorded.press(&id), "and is not pressed twice");
        assert!(!recorded.press("e99"));
    }


    #[test]
    fn limits_keep_each_window_until_it_resets() {
        let five = json!({ "plan": "max", "windows": [{ "kind": "five_hour", "utilization": 0.3, "resetsAt": 200 }] });
        let week = json!({ "plan": null, "windows": [{ "kind": "seven_day", "utilization": 0.6, "resetsAt": 900 }] });
        let both = merge_limits(&merge_limits(&Value::Null, &five, 100), &week, 150);
        assert_eq!(both["plan"], "max");
        let kinds: Vec<_> = both["windows"].as_array().unwrap().iter().map(|w| (w["kind"].as_str().unwrap().to_owned(), w["seenAt"].as_i64().unwrap())).collect();
        assert_eq!(kinds, [("five_hour".to_owned(), 100), ("seven_day".to_owned(), 150)]);
        // A newer report of a kind replaces it.
        let again = merge_limits(&both, &json!({ "windows": [{ "kind": "five_hour", "utilization": 0.5, "resetsAt": 200 }] }), 160);
        assert_eq!(again["windows"][0]["utilization"], 0.5);
        // Past its reset, a window goes.
        let later = merge_limits(&again, &week, 300);
        assert_eq!(later["windows"].as_array().unwrap().len(), 1);
        assert_eq!(later["windows"][0]["kind"], "seven_day");
    }

    #[test]
    fn an_old_report_resent_never_beats_a_newer_one() {
        let newer = merge_limits(&Value::Null, &json!({ "windows": [{ "kind": "five_hour", "utilization": 0.7, "resetsAt": 900 }], "at": 200 }), 210);
        // Another agent resends what it heard at 100, at 220.
        let resent = merge_limits(&newer, &json!({ "windows": [{ "kind": "five_hour", "utilization": 0.3, "resetsAt": 900 }], "at": 100 }), 220);
        assert_eq!(resent["windows"][0]["utilization"], 0.7);
        assert_eq!(resent["windows"][0]["seenAt"], 200);
        // Read with nothing new, an expired window is left out.
        assert_eq!(merge_limits(&resent, &Value::Null, 901), Value::Null);
    }

    #[test]
    fn pictures_go_with_the_message_they_came_with() {
        let mut t = Transcript::default();
        // Sent from loom: "look", then a picture alone as a message of its own.
        let i = t.push(json!({ "kind": "user", "text": "look" }));
        t.said = Some(i);
        t.said = None; // the next message starts
        t.pictures(ids(1));
        let all = t.since("", 0);
        assert_eq!(all["entries"].as_array().unwrap().len(), 2);
        assert!(all["entries"][0].get("pictures").is_none());
        assert_eq!(all["entries"][1]["pictures"], 1);
        // Text and pictures sent together: one message.
        let j = t.push(json!({ "kind": "user", "text": "both" }));
        t.said = Some(j);
        t.pictures(ids(2));
        assert_eq!(t.since("", 0)["entries"][2]["pictures"], 2);
        // Read back: text, a picture, more text stay one message.
        t.chunk("agent", "ok", None);
        t.chunk("user", "before ", None);
        t.pictures(ids(1));
        t.chunk("user", "after", None);
        let entries = t.since("", 0)["entries"].as_array().unwrap().clone();
        assert_eq!(entries.len(), 5);
        assert_eq!((entries[4]["text"].as_str(), entries[4]["pictures"].as_u64()), (Some("before after"), Some(1)));
    }

    use super::*;

    /// `n` picture ids.
    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("b{i}")).collect()
    }

    fn texts(t: &Transcript, after: u64) -> Vec<(u64, String, String)> {
        t.since("", after)["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["i"].as_u64().unwrap(), e["kind"].as_str().unwrap().to_owned(), e["text"].as_str().or(e["title"].as_str()).unwrap_or("").to_owned()))
            .collect()
    }

    #[test]
    fn streams_into_entries_and_reads_back_what_changed() {
        let mut t = Transcript::default();
        let hi = t.push(json!({ "kind": "user", "text": "hi" }));
        t.said = Some(hi);
        t.pictures(ids(2));
        t.chunk("thought", "hm", None);
        t.chunk("agent", "Hel", Some("m1"));
        t.chunk("agent", "lo", Some("m1"));
        t.chunk("agent", "Again", Some("m2"));
        let seen = t.since("", 0)["rev"].as_u64().unwrap();
        assert_eq!(texts(&t, 0), [(0, "user".into(), "hi".into()), (1, "thought".into(), "hm".into()), (2, "agent".into(), "Hello\n\nAgain".into())]);
        assert_eq!(t.since("", 0)["entries"][0]["pictures"], 2);

        // A tool call between: the next words are a message of their own.
        t.tool(&json!({ "toolCallId": "t1", "title": "Read a.rs", "kind": "read", "status": "pending" }));
        t.chunk("agent", "Done.", None);
        assert_eq!(texts(&t, seen), [(3, "tool".into(), "Read a.rs".into()), (4, "agent".into(), "Done.".into())]);

        // An update changes the entry in place, and only it is new.
        let seen = t.since("", 0)["rev"].as_u64().unwrap();
        t.tool(&json!({ "toolCallId": "t1", "status": "completed", "rawInput": { "file_path": "a.rs" }, "content": [{ "type": "content", "content": { "type": "text", "text": "fn main" } }] }));
        // The core API's entry has what the card draws; the bridges' history does not.
        assert_eq!((t.item(3).unwrap()["input"]["file_path"].as_str(), t.item(3).unwrap()["content"][0]["type"].as_str()), (Some("a.rs"), Some("content")));
        assert!(t.since("", 0)["entries"][3].get("content").is_none());
        t.tool_output("t1", &"x".repeat(OUTPUT + 10));
        t.tool_exit("t1", Some(0));
        let changed = t.since("", seen);
        assert_eq!(changed["reset"], false);
        assert_eq!(changed["entries"].as_array().unwrap().len(), 1);
        let tool = &changed["entries"][0];
        assert_eq!((tool["title"].as_str(), tool["status"].as_str(), tool["exit"].as_i64()), (Some("Read a.rs"), Some("completed"), Some(0)));
        // The bridges get the short tail they always had; the core API the longer one.
        assert_eq!(tool["output"].as_str().unwrap().len(), BRIDGE_OUTPUT);
        assert_eq!(t.item(3).unwrap()["output"].as_str().unwrap().len(), OUTPUT);
        assert!(t.since("", t.since("", 0)["rev"].as_u64().unwrap())["entries"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_tool_left_without_a_result_is_over_once_settled() {
        let mut t = Transcript::default();
        t.push(json!({ "kind": "user", "text": "hi" }));
        t.tool(&json!({ "toolCallId": "t1", "title": "note search", "kind": "other", "status": "pending" }));
        assert_eq!(t.item(1).unwrap()["status"], "pending");
        t.settle();
        assert_eq!(t.item(1).unwrap()["status"], "done");
    }

    /// A conversation as the API's subscriber sees it, after each flush.
    fn followed(t: &mut Transcript) -> (crate::api::Store, crate::api::Subscription, crate::api::Mirror) {
        let hub = crate::api::Hub::manual(crate::api::Limits::default());
        t.publish_to(hub.store("conversation/1"));
        let sub = hub.connect(crate::api::Principal::Window).subscribe("conversation/1", || {}).unwrap();
        let mut mirror = crate::api::Mirror::default();
        for m in sub.drain() {
            mirror.apply(&m);
        }
        (hub.store("conversation/1"), sub, mirror)
    }

    fn sync(store: &crate::api::Store, sub: &crate::api::Subscription, mirror: &mut crate::api::Mirror) -> Vec<crate::api::Message> {
        store.flush();
        let got = sub.drain();
        for m in &got {
            assert_ne!(mirror.apply(m), crate::api::Applied::Gap);
        }
        got
    }

    fn brief(mirror: &crate::api::Mirror) -> Vec<String> {
        mirror.items.iter().map(|e| format!("{} {} {} {}", e["id"].as_str().unwrap(), e["turn_id"].as_str().unwrap(), e["kind"].as_str().unwrap(), e["status"].as_str().unwrap())).collect()
    }

    #[test]
    fn the_api_follows_the_conversation_entry_by_entry() {
        let mut t = Transcript::default();
        t.push(json!({ "kind": "notice", "text": "Started.", "class": "dim" }));
        let (store, sub, mut mirror) = followed(&mut t);
        assert_eq!(brief(&mirror), ["e1 t0 notice done"]);

        t.push(json!({ "kind": "user", "text": "hi" }));
        t.chunk("agent", "Hel", Some("m1"));
        let got = sync(&store, &sub, &mut mirror);
        t.chunk("agent", "lo", Some("m1"));
        t.chunk("agent", "Again", Some("m2"));
        let streamed = sync(&store, &sub, &mut mirror);
        assert_eq!(got.len() + streamed.len(), 2, "one message a flush");
        // Another message of the agent's in it: the whole entry, with its message id.
        assert_eq!(
            serde_json::to_value(&streamed[0]).unwrap()["ops"],
            json!([{ "op": "upsert", "item": { "id": "e3", "kind": "agent", "text": "Hello\n\nAgain", "message": "m2", "turn_id": "t1", "status": "streaming" } }])
        );
        t.chunk("agent", "!", Some("m2"));
        let more = sync(&store, &sub, &mut mirror);
        assert_eq!(serde_json::to_value(&more[0]).unwrap()["ops"], json!([{ "op": "append", "id": "e3", "field": "text", "text": "!" }]));
        assert_eq!(brief(&mirror), ["e1 t0 notice done", "e2 t1 user done", "e3 t1 agent streaming"]);

        t.tool(&json!({ "toolCallId": "call_1", "title": "Run ls", "kind": "execute", "status": "in_progress" }));
        t.tool_output("call_1", "a.rs\n");
        sync(&store, &sub, &mut mirror);
        assert_eq!(brief(&mirror)[2..], ["e3 t1 agent done", "e4 t1 tool streaming"]);
        assert_eq!((mirror.items[3]["call"].as_str(), mirror.items[3]["output"].as_str()), (Some("call_1"), Some("a.rs\n")));
        t.tool(&json!({ "toolCallId": "call_1", "status": "failed" }));
        t.push(json!({ "kind": "turn", "seconds": 3 }));
        // The next message: another turn.
        t.push(json!({ "kind": "user", "text": "again" }));
        sync(&store, &sub, &mut mirror);
        assert_eq!(brief(&mirror)[3..], ["e4 t1 tool failed", "e5 t1 turn done", "e6 t2 user done"]);

        let mut all = Vec::new();
        for i in 0..t.entries.len() {
            all.push(t.item(i).unwrap());
        }
        assert_eq!(mirror.items, all, "the subscriber has what the core has");

        // Emptied (a new conversation): a resync, and ids never given again.
        t.clear();
        t.chunk("user", "fresh", None);
        let got = sync(&store, &sub, &mut mirror);
        assert_eq!(serde_json::to_value(&got[0]).unwrap()["reason"], "reset");
        assert_eq!(brief(&mirror), ["e7 t3 user done"]);
    }

    #[test]
    fn questions_wait_as_entries_until_answered_or_dropped() {
        let mut t = Transcript::default();
        let (store, sub, mut mirror) = followed(&mut t);
        t.push(json!({ "kind": "user", "text": "clean up" }));
        let asked = t.question(json!({ "kind": "question", "text": "Allow Bash?", "detail": "rm -r build", "options": [{ "id": "a", "name": "Allow", "kind": "allow_once" }] }));
        let form = t.question(json!({ "kind": "question", "text": "Which colour?", "form": { "type": "object" } }));
        sync(&store, &sub, &mut mirror);
        assert_eq!(brief(&mirror), ["e1 t1 user done", "e2 t1 question awaiting_user", "e3 t1 question awaiting_user"]);
        assert_eq!(mirror.items[1]["options"][0]["kind"], "allow_once");
        t.settled(&asked, "→ Allow");
        t.settled(&form, "Cancelled");
        sync(&store, &sub, &mut mirror);
        assert_eq!(brief(&mirror)[1..], ["e2 t1 question done", "e3 t1 question cancelled"]);
        assert_eq!((mirror.items[1]["answer"].as_str(), mirror.items[2]["answer"].as_str()), (Some("Allow"), Some("Cancelled")));
    }

    #[test]
    fn usage_items_say_the_limits_in_api_times_and_words() {
        let account = account_item(&json!({ "plan": "max", "windows": [{ "kind": "seven_day", "utilization": 0.06, "resetsAt": 1_800_000_000, "seenAt": 1_799_990_000 }] }));
        assert_eq!(account["id"], "account");
        assert_eq!(account["windows"][0]["resets_at"], "2027-01-15T08:00:00Z");
        assert_eq!(crate::api::seconds(&account["windows"][0]["resets_at"]), Some(1_800_000_000));
        let item = json!({ "id": "3", "kind": "agent", "used": 29_000, "size": 1_000_000, "cost": { "amount": 0.123, "currency": "USD" }, "limit": { "kind": "five_hour", "utilization": 0.38, "resets_at": null } });
        let (cost, limit) = crate::agent::spend_words(&item);
        assert_eq!(cost.as_deref(), Some("$0.12"));
        assert_eq!(limit, Some(("5h 38%".to_owned(), "5-hour limit: 38% used".to_owned())));
        assert_eq!(crate::agent::spend_words(&json!({ "cost": null, "limit": null })), (None, None));
    }

    #[test]
    fn the_last_reply_is_what_came_after_the_last_tool_in_this_turn() {
        let view = Recorded::new();
        view.user("hi");
        assert_eq!(view.last_reply(), None);
        view.agent_text_with("Looking.", None);
        view.tool(&json!({ "toolCallId": "t1", "title": "Read" }));
        assert_eq!(view.last_reply(), None);
        view.agent_text_with("Done.", None);
        view.turn_done(3);
        assert_eq!(view.last_reply().as_deref(), Some("Done."));
        view.user("next");
        assert_eq!(view.last_reply(), None);
    }

    #[test]
    fn messages_taken_back_go_before_the_draft_a_blank_line_apart() {
        let view = Recorded::new();
        view.set_draft("and then this");
        view.restore_unsent(vec![("fix tests".into(), Vec::new()), ("then commit".into(), Vec::new())]);
        assert_eq!(view.draft(), "fix tests\n\nthen commit\n\nand then this");
    }

    #[test]
    fn what_fills_the_context_is_in_usage_and_stays() {
        let recorded = Recorded::new();
        *recorded.agent.borrow_mut() = Some("9".to_owned());
        recorded.set_context_parts(vec![("System prompt".to_owned(), 3000), ("Messages".to_owned(), 12_000)], Some(160_000));
        let usage = crate::api::hub().store(USAGE);
        let item = usage.get("9").unwrap();
        assert_eq!((item["parts"][1]["name"].as_str(), item["parts"][1]["tokens"].as_u64(), item["compacts_at"].as_u64()), (Some("Messages"), Some(12_000), Some(160_000)));
        // A usage report after: the parts kept.
        recorded.usage_update(&json!({ "used": 15_000, "size": 200_000 }));
        let item = usage.get("9").unwrap();
        assert_eq!((item["used"].as_u64(), item["parts"].as_array().map(Vec::len)), (Some(15_000), Some(2)));
    }

    #[test]
    fn emptied_it_says_so() {
        let mut t = Transcript::default();
        t.push(json!({ "kind": "user", "text": "old" }));
        let seen = t.since("", 0)["rev"].as_u64().unwrap();
        t.clear();
        t.chunk("user", "new", None);
        let after = t.since("", seen);
        assert_eq!(after["reset"], true);
        assert_eq!(texts(&t, seen), [(0, "user".into(), "new".into())]);
        // A reader from before loom started again: all of it.
        assert_eq!(t.since("another", 2)["reset"], true);
        assert_eq!(t.since("", 999)["reset"], true);
    }
}

/// A subagent as the API's `crew/<agent>` has it: times as RFC 3339.
fn crew_item(c: &Crew) -> Value {
    let when = |at: Instant| crate::api::time(crate::state::now() as i64 - at.elapsed().as_secs() as i64);
    json!({ "id": c.key, "name": c.name, "step": c.step, "state": c.state, "started": when(c.started), "at": when(c.at), "ended": c.ended.map(when), "guessed": c.guessed })
}

/// A workflow as the API has it (`workflows/<agent>`): followed from its files, its end as the
/// agent tells it.
struct RecordedWorkflow {
    tool_id: String,
    progress: Rc<RefCell<crate::workflow::Progress>>,
    store: Option<Store>,
    launch: Launch,
    /// Stops asked of it that went unanswered (each grows it: a client offers Stop again).
    unanswered: std::cell::Cell<u64>,
}

impl RecordedWorkflow {
    fn publish(&self) {
        let Some(store) = &self.store else { return };
        let p = self.progress.borrow();
        let state = match &p.state {
            crate::workflow::State::Running => json!("running"),
            crate::workflow::State::Done(text) => json!(text),
            crate::workflow::State::Failed => json!("failed"),
            crate::workflow::State::Stopped => json!("stopped"),
            crate::workflow::State::Ended => json!("ended"),
        };
        let members: Vec<Value> = p.members.iter().map(|m| json!({ "label": m.label, "phase": m.phase, "done": m.result.is_some() })).collect();
        let (done, all) = p.counts();
        store.upsert(json!({ "id": self.tool_id, "name": self.launch.name, "summary": self.launch.summary, "task": self.launch.task, "dir": self.launch.dir.to_string_lossy(), "running": !p.done(), "state": state, "done": done, "agents": all, "members": members, "unanswered": self.unanswered.get() }));
    }
}

impl Workflow for RecordedWorkflow {
    fn tool_id(&self) -> String {
        self.tool_id.clone()
    }
    fn name(&self) -> String {
        self.launch.name.clone()
    }
    fn task(&self) -> String {
        self.launch.task.clone()
    }
    fn done(&self) -> bool {
        self.progress.borrow().done()
    }
    fn counts(&self) -> (usize, usize) {
        self.progress.borrow().counts()
    }
    fn stop_unanswered(&self) {
        if !self.progress.borrow().done() {
            self.unanswered.set(self.unanswered.get() + 1);
            self.publish();
        }
    }
    fn ended(&self, status: &str, output: Option<std::path::PathBuf>) {
        self.progress.borrow_mut().ended(status, output, true);
        self.publish();
    }
    fn stopped(&self) {
        self.progress.borrow_mut().state = crate::workflow::State::Stopped;
        self.publish();
    }
}
