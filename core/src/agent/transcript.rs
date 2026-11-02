//! The conversation as data: what an agent's view is told, kept for whoever reads it from
//! outside loom's window (the bridges' `history`). The view draws it; this remembers it.
//!
//! Entries, in order: `user` (`text`, `pictures`), `agent` and `thought` (`text`), `tool`
//! (`id`, `title`, `tool`: its kind, `status`, `parent`: the task it runs under, `output`: the
//! end of what it printed, `exit`), `notice` (`text`, `class`) and `turn` (`seconds`: how long
//! the turn took). Entries change as they stream in; each change is a new revision, so a reader
//! asks only for what changed after the last revision it saw. Revisions count within an
//! `epoch` (one per transcript, so per agent and per run of loom): from another, all of it.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use serde_json::{Value, json};

use super::view::{Card, Crew, Form, Png, View, Workflow};
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
        let rev = self.bump();
        self.entries.push((rev, entry));
        self.open = None;
        self.entries.len() - 1
    }

    fn change(&mut self, i: usize, change: impl FnOnce(&mut Value)) {
        let rev = self.bump();
        if let Some((at, entry)) = self.entries.get_mut(i) {
            *at = rev;
            change(entry);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.tools.clear();
        self.open = None;
        self.cleared = self.bump();
    }

    /// A piece of a message streaming in, onto the last one if it is of the same kind.
    fn chunk(&mut self, kind: &'static str, text: &str, id: Option<&str>) {
        match self.open.clone() {
            Some((i, open, said)) if open == kind => {
                // Another message of the agent's: a paragraph of its own (as the view shows it).
                let another = id.is_some() && said.is_some() && said.as_deref() != id;
                self.change(i, |entry| {
                    let mut all = entry["text"].as_str().unwrap_or("").to_owned();
                    if another && !all.ends_with("\n\n") {
                        all.push_str("\n\n");
                    }
                    all.push_str(text);
                    entry["text"] = json!(all);
                });
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
        // On the message they came with, else on one of their own.
        match self.entries.len().checked_sub(1).filter(|&i| self.entries[i].1["kind"] == "user") {
            Some(i) => self.change(i, |entry| entry["pictures"] = json!(entry["pictures"].as_u64().unwrap_or(0) + count as u64)),
            None => {
                self.push(json!({ "kind": "user", "text": "", "pictures": count }));
            }
        }
        self.open = None;
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

impl Recorded {
    pub fn new(inner: Rc<dyn View>) -> Recorded {
        let transcript = Transcript { epoch: crate::rt::uuid(), ..Transcript::default() };
        Recorded { inner, transcript: RefCell::new(transcript), usage: RefCell::new(json!({})) }
    }
}

thread_local! {
    /// The account's limits as Claude Code last told them (any agent): `{plan, windows}`.
    static ACCOUNT: RefCell<Value> = const { RefCell::new(Value::Null) };
}

/// The account's limits (`{plan, windows: [{kind, utilization, resetsAt}]}`), null until an
/// agent's answer brought them.
pub fn account_limits() -> Value {
    ACCOUNT.with_borrow(Clone::clone)
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
        self.transcript.borrow_mut().push(json!({ "kind": "user", "text": text }));
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
        ACCOUNT.set(limits.clone());
        self.inner.set_limits(limits);
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
        t.push(json!({ "kind": "user", "text": "hi" }));
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
