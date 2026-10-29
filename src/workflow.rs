//! Workflows an agent runs (Claude Code's Workflow tool): a card in the conversation with
//! each phase's progress and every agent of it, read from Claude Code's own files. The
//! adapter says only that one was launched (its task id and its folder); after that,
//! Claude Code writes a journal (`journal.jsonl`: a line when an agent starts, with its label
//! and phase, and one when it is done, with its answer), each agent's transcript
//! (`agent-<id>.jsonl`) beside it, and, once the workflow is over, the task's output file.
//! These are Claude Code's files, not a promised interface: what cannot be read is left out.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::{glib, pango, prelude::*};
use serde_json::{Value, json};

/// A workflow's launch, from its tool call: task id, folder, name, what it does.
pub struct Launch {
    pub task: String,
    pub dir: PathBuf,
    pub name: String,
    pub summary: String,
}

/// The launch of a workflow, if `update` is the Workflow tool call's result: from the
/// structured response when the adapter gives it, else from the result's text (a
/// conversation read back has only that).
pub fn launch_of(update: &Value) -> Option<Launch> {
    let response = &update["_meta"]["claudeCode"]["toolResponse"];
    if response["taskType"].as_str() == Some("local_workflow") {
        let dir = PathBuf::from(response["transcriptDir"].as_str()?);
        return Some(Launch {
            task: response["taskId"].as_str()?.to_owned(),
            name: response["workflowName"].as_str().map(str::to_owned).unwrap_or_else(|| name_of(&dir)),
            summary: response["summary"].as_str().unwrap_or("").to_owned(),
            dir,
        });
    }
    const LAUNCHED: &str = "Workflow launched in background.";
    let text = match update["rawOutput"].as_str() {
        Some(raw) if raw.starts_with(LAUNCHED) => raw.to_owned(),
        Some(_) => return None,
        None => {
            let parts = update["content"].as_array()?;
            if !parts.first().and_then(|c| c["content"]["text"].as_str()).is_some_and(|t| t.starts_with(LAUNCHED)) {
                return None;
            }
            parts.iter().filter_map(|c| c["content"]["text"].as_str()).collect::<Vec<_>>().join("\n")
        }
    };
    let field = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)).map(|v| v.trim().to_owned());
    let dir = PathBuf::from(field("Transcript dir:")?);
    // "decision-council-wf_afecaff2-414.js": the name the script gave it.
    let run = name_of(&dir);
    let named = field("Script file:")
        .and_then(|s| Path::new(&s).file_stem().map(|n| n.to_string_lossy().into_owned()))
        .and_then(|stem| stem.strip_suffix(&format!("-{run}")).map(str::to_owned));
    Some(Launch { task: field("Workflow launched in background. Task ID:")?, summary: field("Summary:").unwrap_or_default(), name: named.unwrap_or(run), dir })
}

/// "decision-council" from the script's name, else the run's folder name.
fn name_of(dir: &Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "workflow".to_owned())
}

/// One agent of a workflow.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub id: String,
    pub label: String,
    pub phase: String,
    /// Its answer, once it is done.
    pub result: Option<String>,
}

/// The journal's lines read into the members (in the order they started).
pub fn read_journal(text: &str, members: &mut Vec<Member>) {
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let Some(id) = v["agentId"].as_str() else { continue };
        match v["type"].as_str() {
            Some("started") if !members.iter().any(|m| m.id == id) => members.push(Member {
                id: id.to_owned(),
                label: v["label"].as_str().unwrap_or("agent").to_owned(),
                phase: v["phase"].as_str().unwrap_or("").to_owned(),
                result: None,
            }),
            Some("result") => {
                let result = match &v["result"] {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if let Some(m) = members.iter_mut().find(|m| m.id == id) {
                    m.result = Some(result);
                }
            }
            _ => {}
        }
    }
}

/// "Advise 120/180 · Synthesize 0/6": each phase, in the order it first appears.
pub fn phases(members: &[Member]) -> String {
    let mut order: Vec<&str> = Vec::new();
    for m in members {
        if !order.contains(&m.phase.as_str()) {
            order.push(&m.phase);
        }
    }
    order
        .iter()
        .map(|phase| {
            let of: Vec<&Member> = members.iter().filter(|m| m.phase == *phase).collect();
            let done = of.iter().filter(|m| m.result.is_some()).count();
            let name = if phase.is_empty() { "Agents" } else { phase };
            format!("{name} {done}/{}", of.len())
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// What a file has from byte `from` on.
fn read_from(path: &Path, from: u64) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Where Claude Code puts a background task's output: its temporary folder, the project's
/// and the conversation's folders (as in its projects folder), then tasks/<task>.output.
fn output_path(dir: &Path, task: &str) -> Option<PathBuf> {
    // dir: …/projects/<project>/<session>/subagents/workflows/<run>
    let session = dir.parent()?.parent()?.parent()?;
    let project = session.parent()?;
    let root = std::env::var_os("CLAUDE_CODE_TMPDIR").map(PathBuf::from).unwrap_or_else(|| {
        // SAFETY: getuid cannot fail.
        std::env::temp_dir().join(format!("claude-{}", unsafe { libc::getuid() }))
    });
    Some(root.join(project.file_name()?).join(session.file_name()?).join("tasks").join(format!("{task}.output")))
}

/// A workflow's card in the conversation, kept up to date while it runs.
pub struct Run {
    pub tool_id: String,
    pub name: String,
    dir: PathBuf,
    output: Option<PathBuf>,
    pub root: gtk::Box,
    status: gtk::Label,
    progress: gtk::Label,
    finished: gtk::Label,
    agents: gtk::Expander,
    list: gtk::ListBox,
    members: RefCell<Vec<Member>>,
    /// How much of the journal is read.
    read: Cell<u64>,
    /// When the journal last had something new.
    news: Cell<std::time::Instant>,
    /// The members the list shows (it is filled again only when they change).
    shown: RefCell<Vec<Member>>,
    pub done: Cell<bool>,
    /// Launched in this run of the agent (not read back from an older one).
    live: bool,
    /// The agent of the conversation still runs (a workflow ends with it).
    alive: Box<dyn Fn() -> bool>,
    /// An agent of it opened (its transcript file and label).
    pub on_open: RefCell<Option<Rc<dyn Fn(PathBuf, String)>>>,
    /// Its progress changed (the strip under the conversation says it).
    pub on_changed: RefCell<Option<Rc<dyn Fn()>>>,
}

impl Run {
    /// `live`: launched now (a conversation read back has only workflows that are over).
    pub fn new(tool_id: &str, launch: Launch, live: bool, alive: impl Fn() -> bool + 'static) -> Rc<Run> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("workflow-card");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::builder().label(format!("Workflow · {}", launch.name)).xalign(0.0).ellipsize(pango::EllipsizeMode::End).hexpand(true).build();
        title.add_css_class("workflow-title");
        let status = gtk::Label::new(Some("running"));
        status.add_css_class("workflow-status");
        head.append(&title);
        head.append(&status);
        root.append(&head);
        if !launch.summary.is_empty() {
            let summary = gtk::Label::builder().label(&launch.summary).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build();
            summary.add_css_class("dim");
            summary.add_css_class("small");
            root.append(&summary);
        }
        let progress = gtk::Label::builder().xalign(0.0).wrap(true).build();
        progress.add_css_class("workflow-progress");
        root.append(&progress);
        let finished = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
        finished.add_css_class("small");
        root.append(&finished);
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        list.add_css_class("workflow-agents");
        let agents = gtk::Expander::builder().label("Agents").child(&list).build();
        agents.set_tooltip_text(Some("Every agent of the workflow: what it does and its answer; one opens its whole conversation"));
        root.append(&agents);
        let run = Rc::new(Run {
            tool_id: tool_id.to_owned(),
            name: launch.name.clone(),
            output: output_path(&launch.dir, &launch.task),
            dir: launch.dir,
            root,
            status,
            progress,
            finished,
            agents: agents.clone(),
            list: list.clone(),
            members: RefCell::default(),
            read: Cell::new(0),
            news: Cell::new(std::time::Instant::now()),
            shown: RefCell::default(),
            done: Cell::new(false),
            live,
            alive: Box::new(alive),
            on_open: RefCell::default(),
            on_changed: RefCell::default(),
        });
        // Opening the list fills it (a long one is only made when wanted).
        let weak = Rc::downgrade(&run);
        agents.connect_expanded_notify(move |_| {
            if let Some(run) = weak.upgrade() {
                run.fill_list();
            }
        });
        let weak = Rc::downgrade(&run);
        list.connect_row_activated(move |_, row| {
            let Some(run) = weak.upgrade() else { return };
            let member = run.shown.borrow().get(row.index() as usize).cloned();
            let open = run.on_open.borrow().clone();
            if let (Some(member), Some(open)) = (member, open) {
                open(run.dir.join(format!("agent-{}.jsonl", member.id)), member.label);
            }
        });
        run.poll();
        // While it runs: read what is new every second and a half.
        if !run.done.get() {
            let weak = Rc::downgrade(&run);
            glib::timeout_add_local(Duration::from_millis(1500), move || match weak.upgrade() {
                Some(run) if !run.done.get() => {
                    run.poll();
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            });
        }
        run
    }

    /// (done, all) agents, for the strip.
    pub fn counts(&self) -> (usize, usize) {
        let members = self.members.borrow();
        (members.iter().filter(|m| m.result.is_some()).count(), members.len())
    }

    /// Reads what the journal has new, and whether the workflow is over.
    fn poll(&self) {
        let journal = self.dir.join("journal.jsonl");
        let length = std::fs::metadata(&journal).map_or(0, |m| m.len());
        if length > self.read.get()
            && let Some(bytes) = read_from(&journal, self.read.get())
        {
            // Up to the last whole line (one may be half written).
            let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            read_journal(&String::from_utf8_lossy(&bytes[..end]), &mut self.members.borrow_mut());
            self.read.set(self.read.get() + end as u64);
            self.news.set(std::time::Instant::now());
        }
        let output = self.output.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok());
        if let Some(output) = output {
            self.done.set(true);
            self.status.set_label("done");
            self.status.add_css_class("done");
            let mut said = Vec::new();
            if let Some(n) = output["agentCount"].as_u64() {
                said.push(format!("{n} agents"));
            }
            if let Some(tokens) = output["totalTokens"].as_u64() {
                said.push(format!("{:.1}M tokens", tokens as f64 / 1_000_000.0));
            }
            let logs: Vec<String> = output["logs"].as_array().into_iter().flatten().filter_map(|l| l.as_str().map(str::to_owned)).collect();
            let mut text = format!("Finished: {}.", said.join(", "));
            if !logs.is_empty() {
                text.push_str(&format!("\n{}", logs.join(" · ")));
            }
            self.finished.set_label(&text);
            self.finished.set_visible(true);
        } else if !(self.alive)() {
            // Its agent stopped (suspended, closed, loom quit) before it ended.
            self.done.set(true);
            self.status.set_label("stopped");
            self.status.add_css_class("stopped");
        } else if self.all_answered_a_while_ago() {
            self.done.set(true);
            self.status.set_label("done");
            self.status.add_css_class("done");
        } else if !self.live {
            // Read back from an earlier run of the agent: over, its output not kept (Claude
            // Code keeps it in a temporary folder).
            self.done.set(true);
            self.status.set_label("ended");
            self.status.add_css_class("done");
        }
        let (done, all) = self.counts();
        let text = phases(&self.members.borrow());
        self.progress.set_label(if all == 0 { "Starting…" } else { &text });
        self.agents.set_label(Some(&format!("Agents ({done} of {all} done)")));
        if self.agents.is_expanded() {
            self.fill_list();
        }
        if let Some(changed) = self.on_changed.borrow().clone() {
            changed();
        }
    }

    /// Its output is not where loom looks (another temporary folder), yet every agent has
    /// answered and a minute went by with nothing new: over.
    fn all_answered_a_while_ago(&self) -> bool {
        let members = self.members.borrow();
        !members.is_empty() && members.iter().all(|m| m.result.is_some()) && self.news.get().elapsed() > Duration::from_secs(60)
    }

    fn fill_list(&self) {
        let members = self.members.borrow().clone();
        if *self.shown.borrow() == members {
            return;
        }
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        for m in &members {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let glyph = gtk::Label::new(Some(if m.result.is_some() { "✓" } else { "◌" }));
            glyph.add_css_class(if m.result.is_some() { "done" } else { "running" });
            let label = gtk::Label::builder().label(&m.label).xalign(0.0).build();
            label.add_css_class("workflow-agent");
            let preview = m.result.as_deref().map(|r| r.trim().lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim_start_matches(['#', '*', ' ']).to_owned()).unwrap_or_else(|| "working…".to_owned());
            let said = gtk::Label::builder().label(preview).xalign(0.0).ellipsize(pango::EllipsizeMode::End).hexpand(true).build();
            said.add_css_class("dim");
            said.add_css_class("small");
            line.append(&glyph);
            line.append(&label);
            line.append(&said);
            let row = gtk::ListBoxRow::builder().child(&line).build();
            row.set_tooltip_text(Some("Open its conversation"));
            self.list.append(&row);
        }
        *self.shown.borrow_mut() = members;
    }
}

/// A workflow agent's conversation (its transcript file) into a read-only chat: what it was
/// asked, what it said, and its tool calls.
pub fn fill_transcript(chat: &crate::chat::Chat, path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        chat.notice("Its conversation is not there (Claude Code keeps it only while the conversation's files are kept).", "dim");
        return;
    };
    let mut results: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    for v in &lines {
        for block in v["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] == "tool_result"
                && let Some(id) = block["tool_use_id"].as_str()
            {
                let text = match &block["content"] {
                    Value::String(s) => s.clone(),
                    Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
                    _ => String::new(),
                };
                results.insert(id.to_owned(), text);
            }
        }
    }
    for v in &lines {
        let content = &v["message"]["content"];
        match v["type"].as_str() {
            Some("user") => {
                let said = match content {
                    Value::String(s) => s.clone(),
                    Value::Array(parts) => parts.iter().filter(|p| p["type"] == "text").filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n\n"),
                    _ => String::new(),
                };
                if !said.trim().is_empty() {
                    chat.user(said.trim());
                }
            }
            Some("assistant") => {
                for block in content.as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => chat.agent_text_with(block["text"].as_str().unwrap_or(""), None),
                        Some("tool_use") => {
                            let (id, name) = (block["id"].as_str().unwrap_or(""), block["name"].as_str().unwrap_or("Tool"));
                            let input = &block["input"];
                            let what = ["command", "file_path", "pattern", "url", "query", "description"].iter().find_map(|k| input[*k].as_str());
                            let title = what.map_or_else(|| name.to_owned(), |w| format!("{name} {w}"));
                            let kind = match name {
                                "Bash" => "execute",
                                "Read" => "read",
                                "Edit" | "Write" | "MultiEdit" => "edit",
                                "Grep" | "Glob" => "search",
                                "WebFetch" | "WebSearch" => "fetch",
                                _ => "other",
                            };
                            let output = results.get(id).cloned().unwrap_or_default();
                            chat.tool(&json!({
                                "toolCallId": format!("wf-{id}"),
                                "title": title,
                                "kind": kind,
                                "status": "completed",
                                "rawInput": input,
                                "content": [{ "type": "content", "content": { "type": "text", "text": output } }],
                            }));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workflow_is_followed_from_its_files() {
        let raw = "Workflow launched in background. Task ID: wf1\nSummary: Ask a council\nTranscript dir: /home/me/.claude/projects/-p/s1/subagents/workflows/wf_9\nScript file: /s/council-wf_9.js";
        let launch = launch_of(&json!({ "rawOutput": raw })).unwrap();
        assert_eq!((launch.task.as_str(), launch.summary.as_str(), launch.name.as_str()), ("wf1", "Ask a council", "council"));
        assert!(output_path(&launch.dir, "wf1").unwrap().ends_with("-p/s1/tasks/wf1.output"));
        let structured = json!({ "_meta": { "claudeCode": { "toolResponse": { "taskType": "local_workflow", "taskId": "t", "workflowName": "council", "transcriptDir": "/d/s/subagents/workflows/r" } } } });
        assert_eq!(launch_of(&structured).unwrap().name, "council");
        assert!(launch_of(&json!({ "rawOutput": "something else" })).is_none());
        let mut members = Vec::new();
        read_journal(
            "{\"type\":\"launched\"}\n{\"type\":\"started\",\"agentId\":\"a\",\"label\":\"F1 · contrarian\",\"phase\":\"Advise\"}\n{\"type\":\"started\",\"agentId\":\"b\",\"label\":\"F1 · chair\",\"phase\":\"Decide\"}\n{\"type\":\"result\",\"agentId\":\"a\",\"result\":\"Choose B.\"}\n",
            &mut members,
        );
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].result.as_deref(), Some("Choose B."));
        assert_eq!(phases(&members), "Advise 1/1 · Decide 0/1");
    }
}
