//! The conversation as data: what an agent's view is told, kept for whoever reads it from
//! outside loom's window (the bridges' `history`). The view draws it; this remembers it.
//!
//! Entries, in order: `user` (`text`, `pictures`), `agent` and `thought` (`text`), `tool`
//! (`id`, `title`, `tool`: its kind, `status`, `parent`: the task it runs under, `output`: the
//! end of what it printed, `exit`), `notice` (`text`, `class`) and `turn` (`seconds`: how long
//! the turn took). Entries change as they stream in; each change is a new revision, so a reader
//! asks only for what changed after the last revision it saw. Revisions count within an
//! `epoch` (one per transcript, so per agent and per run of loom): from another, all of it.
//!
//! It is also the core API's `conversation/<agent>` resource: each entry published with an
//! `id` of its own (`e1`, `e2`…; a tool's call id is its `call`), the `turn_id` of the turn it
//! belongs to (a turn starts with the user's message) and a `status`: `streaming` while text
//! still comes, a tool's `pending`, `streaming`, `done` or `failed`; else `done`.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use serde_json::{Value, json};

use super::view::{Card, Crew, Form, Png, View, Workflow};
use crate::api::Store;
use crate::jobs::Id;
use crate::workflow::Launch;

/// How much of a command's output an entry keeps (its end).
const OUTPUT: usize = 4000;

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
            // A tool's, as the agent reports it (ACP: pending, in_progress, completed, failed).
            Some(Value::String(acp)) => match acp.as_str() {
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
                if id.is_some() {
                    self.open = Some((i, kind, id.map(str::to_owned)));
                }
            }
            _ => {
                let i = self.push(json!({ "kind": kind, "text": text }));
                self.open = Some((i, kind, id.map(str::to_owned)));
            }
        }
    }

    fn pictures(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        // On the message they were sent with, or the one being read back; else their own.
        let reading = match self.open {
            Some((i, "user", _)) => Some(i),
            _ => None,
        };
        match self.said.take().or(reading) {
            Some(i) => self.change(i, |entry| entry["pictures"] = json!(entry["pictures"].as_u64().unwrap_or(0) + count as u64)),
            None => {
                self.push(json!({ "kind": "user", "text": "", "pictures": count }));
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
                    entry[field] = json!(value);
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
                let mut cut = output.len() - OUTPUT;
                while !output.is_char_boundary(cut) {
                    cut += 1;
                }
                output.drain(..cut);
            }
            entry["output"] = json!(output);
        });
    }

    fn tool_exit(&mut self, id: &str, code: Option<i64>) {
        if let Some(&i) = self.tools.get(id) {
            self.change(i, |entry| entry["exit"] = json!(code));
        }
    }
}

/// An agent's view that also keeps its conversation in a `Transcript`: everything else goes on
/// to the view it wraps untouched.
pub struct Recorded {
    inner: Rc<dyn View>,
    pub transcript: RefCell<Transcript>,
    /// Its usage as last told: `{used, size}` of its context, `cost` (as "$0.42").
    pub usage: RefCell<Value>,
}

impl Drop for Recorded {
    fn drop(&mut self) {
        if let Some(store) = &self.transcript.get_mut().store {
            crate::api::close(store.name());
        }
    }
}

impl Recorded {
    /// Published as resource `name` of the core API (`conversation/<agent>`).
    pub fn publish(&self, name: &str) {
        self.transcript.borrow_mut().publish_to(crate::api::hub().store(name));
    }

    /// A message is being sent from loom: pictures that follow belong to it, not to the last.
    pub fn message_starts(&self) {
        self.transcript.borrow_mut().said = None;
    }

    pub fn new(inner: Rc<dyn View>) -> Recorded {
        let transcript = Transcript { epoch: crate::rt::uuid(), ..Transcript::default() };
        Recorded { inner, transcript: RefCell::new(transcript), usage: RefCell::new(json!({})) }
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
    // The UI's own view, as it gave it.
    fn as_any(&self) -> &dyn Any {
        self.inner.as_any()
    }

    fn clear(&self) {
        self.transcript.borrow_mut().clear();
        self.inner.clear();
    }
    fn has_messages(&self) -> bool {
        self.inner.has_messages()
    }
    fn last_reply(&self) -> Option<String> {
        self.inner.last_reply()
    }
    fn user(&self, text: &str) {
        let mut transcript = self.transcript.borrow_mut();
        let i = transcript.push(json!({ "kind": "user", "text": text }));
        transcript.said = Some(i);
        drop(transcript);
        self.inner.user(text);
    }
    fn user_chunk(&self, text: &str) {
        self.transcript.borrow_mut().chunk("user", text, None);
        self.inner.user_chunk(text);
    }
    fn user_images(&self, pngs: &[Png]) {
        self.transcript.borrow_mut().pictures(pngs.len());
        self.inner.user_images(pngs);
    }
    fn user_image_data(&self, base64: &str) {
        self.transcript.borrow_mut().pictures(1);
        self.inner.user_image_data(base64);
    }
    fn agent_text_with(&self, text: &str, id: Option<&str>) {
        self.transcript.borrow_mut().chunk("agent", text, id);
        self.inner.agent_text_with(text, id);
    }
    fn thought(&self, text: &str) {
        self.transcript.borrow_mut().chunk("thought", text, None);
        self.inner.thought(text);
    }
    fn notice(&self, text: &str, class: &str) {
        self.transcript.borrow_mut().push(json!({ "kind": "notice", "text": text, "class": class }));
        self.inner.notice(text, class);
    }
    fn notice_action(&self, text: &str, button: &str, action: Rc<dyn Fn()>) {
        self.transcript.borrow_mut().push(json!({ "kind": "notice", "text": text, "class": "" }));
        self.inner.notice_action(text, button, action);
    }
    fn notice_choice(&self, text: &str, button: &str, action: Rc<dyn Fn()>) {
        self.transcript.borrow_mut().push(json!({ "kind": "notice", "text": text, "class": "" }));
        self.inner.notice_choice(text, button, action);
    }
    fn tool(&self, update: &Value) {
        self.transcript.borrow_mut().tool(update);
        self.inner.tool(update);
    }
    fn tool_output(&self, id: &str, data: &str) {
        self.transcript.borrow_mut().tool_output(id, data);
        self.inner.tool_output(id, data);
    }
    fn tool_exit(&self, id: &str, code: Option<i64>) {
        self.transcript.borrow_mut().tool_exit(id, code);
        self.inner.tool_exit(id, code);
    }
    fn tool_button(&self, id: &str, label: &str, tip: &str, action: Rc<dyn Fn()>) {
        self.inner.tool_button(id, label, tip, action);
    }
    fn open_tool(&self, id: &str) {
        self.inner.open_tool(id);
    }
    fn go_to_tool(&self, id: &str) {
        self.inner.go_to_tool(id);
    }
    fn set_plan(&self, entries: &Value) {
        self.inner.set_plan(entries);
    }
    fn turn_done(&self, seconds: u64) {
        self.transcript.borrow_mut().push(json!({ "kind": "turn", "seconds": seconds }));
        self.inner.turn_done(seconds);
    }
    fn turn_undo(&self, undo: Rc<dyn Fn()>) {
        self.inner.turn_undo(undo);
    }
    fn permission(&self, question: &str, detail: Option<&str>, markdown: bool, options: &[(String, String, String)], choose: Rc<dyn Fn(String)>) -> Rc<dyn Card> {
        self.inner.permission(question, detail, markdown, options, choose)
    }
    fn form(&self, message: &str, schema: &Value, answer: Rc<dyn Fn(Option<Value>)>) -> Rc<dyn Form> {
        self.inner.form(message, schema, answer)
    }
    fn workflow(&self, tool_id: &str, launch: Launch, live: bool, alive: Box<dyn Fn() -> bool>, on_changed: Rc<dyn Fn()>, on_stop: Rc<dyn Fn()>) -> Rc<dyn Workflow> {
        self.inner.workflow(tool_id, launch, live, alive, on_changed, on_stop)
    }

    fn set_cwd(&self, cwd: &Path) {
        self.inner.set_cwd(cwd);
    }
    fn set_title(&self, title: &str, place: &str, full: &str) {
        self.inner.set_title(title, place, full);
    }
    fn set_state(&self, text: &str, class: &str) {
        self.inner.set_state(text, class);
    }
    fn set_mode(&self, mode: &str) {
        self.inner.set_mode(mode);
    }
    fn set_starting(&self, text: Option<&str>) {
        self.inner.set_starting(text);
    }
    fn set_working(&self, since: Option<Instant>) {
        self.inner.set_working(since);
    }
    fn set_options(&self, options: &Value) {
        self.inner.set_options(options);
    }
    fn set_commands(&self, commands: Vec<(String, String)>) {
        self.inner.set_commands(commands);
    }
    fn set_suggestion(&self, text: Option<String>) {
        self.inner.set_suggestion(text);
    }
    fn set_show_thinking(&self, show: bool) {
        self.inner.set_show_thinking(show);
    }
    fn set_workflow_note(&self, note: Option<String>) {
        self.inner.set_workflow_note(note);
    }
    fn set_usage(&self, used: u64, size: u64) {
        {
            let mut usage = self.usage.borrow_mut();
            usage["used"] = json!(used);
            usage["size"] = json!(size);
        }
        self.inner.set_usage(used, size);
    }
    fn set_spend(&self, cost: Option<String>, limit: Option<(String, String)>) {
        self.usage.borrow_mut()["cost"] = json!(cost);
        self.inner.set_spend(cost, limit);
    }
    fn set_limits(&self, limits: Value) {
        let merged = ACCOUNT.with_borrow_mut(|account| {
            *account = merge_limits(account, &limits, crate::state::now() as i64);
            account.clone()
        });
        self.inner.set_limits(merged);
    }
    fn set_context_parts(&self, parts: Vec<(String, u64)>, compacts_at: Option<u64>) {
        self.inner.set_context_parts(parts, compacts_at);
    }
    fn context_used(&self) -> u64 {
        self.inner.context_used()
    }
    fn crew_set(&self, entry: Crew) {
        self.inner.crew_set(entry);
    }
    fn crew_remove(&self, key: &str) {
        self.inner.crew_remove(key);
    }
    fn crew_clear(&self) {
        self.inner.crew_clear();
    }
    fn show_mcp(&self, servers: Option<Vec<(String, String, Option<String>)>>) {
        self.inner.show_mcp(servers);
    }

    fn draft(&self) -> String {
        self.inner.draft()
    }
    fn set_draft(&self, text: &str) {
        self.inner.set_draft(text);
    }
    fn focus_input(&self) {
        self.inner.focus_input();
    }
    fn show_queued(&self, messages: &[String]) {
        self.inner.show_queued(messages);
    }
    fn unqueue(&self, text: &str, images: Vec<Png>) {
        self.inner.unqueue(text, images);
    }
    fn restore_unsent(&self, messages: Vec<(String, Vec<Png>)>) {
        self.inner.restore_unsent(messages);
    }

    // A subagent's conversation is not recorded: it is read on the desktop.
    fn sub_view(&self) -> Rc<dyn View> {
        self.inner.sub_view()
    }
    fn show_aside(&self, view: &Rc<dyn View>, title: &str) {
        self.inner.show_aside(view, title);
    }
    fn show_main(&self) {
        self.inner.show_main();
    }
    fn open_uri(&self, uri: &str) {
        self.inner.open_uri(uri);
    }
    fn show_job(&self, id: Id) {
        self.inner.show_job(id);
    }
}

#[cfg(test)]
mod tests {

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
        t.pictures(1);
        let all = t.since("", 0);
        assert_eq!(all["entries"].as_array().unwrap().len(), 2);
        assert!(all["entries"][0].get("pictures").is_none());
        assert_eq!(all["entries"][1]["pictures"], 1);
        // Text and pictures sent together: one message.
        let j = t.push(json!({ "kind": "user", "text": "both" }));
        t.said = Some(j);
        t.pictures(2);
        assert_eq!(t.since("", 0)["entries"][2]["pictures"], 2);
        // Read back: text, a picture, more text stay one message.
        t.chunk("agent", "ok", None);
        t.chunk("user", "before ", None);
        t.pictures(1);
        t.chunk("user", "after", None);
        let entries = t.since("", 0)["entries"].as_array().unwrap().clone();
        assert_eq!(entries.len(), 5);
        assert_eq!((entries[4]["text"].as_str(), entries[4]["pictures"].as_u64()), (Some("before after"), Some(1)));
    }

    use super::*;

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
        t.pictures(2);
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
        t.tool(&json!({ "toolCallId": "t1", "status": "completed" }));
        t.tool_output("t1", &"x".repeat(OUTPUT + 10));
        t.tool_exit("t1", Some(0));
        let changed = t.since("", seen);
        assert_eq!(changed["reset"], false);
        assert_eq!(changed["entries"].as_array().unwrap().len(), 1);
        let tool = &changed["entries"][0];
        assert_eq!((tool["title"].as_str(), tool["status"].as_str(), tool["exit"].as_i64()), (Some("Read a.rs"), Some("completed"), Some(0)));
        assert_eq!(tool["output"].as_str().unwrap().len(), OUTPUT);
        assert!(t.since("", t.since("", 0)["rev"].as_u64().unwrap())["entries"].as_array().unwrap().is_empty());
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
        assert_eq!(serde_json::to_value(&streamed[0]).unwrap()["ops"], json!([{ "op": "append", "id": "e3", "field": "text", "text": "lo\n\nAgain" }]));
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
