//! What an agent shows, and where: the seam between an agent (here) and the UI drawing it.
//! The GTK app implements these with its chat; another UI implements them its own way.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use serde_json::Value;

use crate::jobs::Id;
use crate::workflow::Launch;

/// An image attached to a message (PNG bytes).
pub type Png = Vec<u8>;

/// A row of the subagents strip.
pub struct Crew {
    /// The subagent's session id, or (reported inside the main conversation) its task's tool call.
    pub key: String,
    pub name: String,
    /// What it did last.
    pub step: String,
    /// When it last did something.
    pub at: Instant,
    pub started: Instant,
    /// "running", "requires_action", "idle".
    pub state: Option<String>,
    pub ended: Option<Instant>,
    /// No session of its own: its state is a guess from its steps.
    pub guessed: bool,
}

/// A question's card: once answered elsewhere (or dropped), it says how.
pub trait Card {
    fn resolve(&self, answer: &str);
}

/// A question form as filled in so far (Claude Code's AskUserQuestion, an MCP server's).
pub trait Form {
    fn card(&self) -> Rc<dyn Card>;
    /// What was filled in, as the schema's object, or why it can't be sent.
    fn content(&self) -> Result<Value, String>;
    /// The content if it can be sent; else the form shows what it lacks.
    fn checked(&self) -> Option<Value>;
    /// "→ Colour: Blue · Other: …".
    fn summary(&self) -> String;
    /// Its choices: (question, option names, several allowed, which are picked).
    fn choices(&self) -> Vec<(String, Vec<String>, bool, Vec<bool>)>;
    fn pick_in(&self, field: usize, option: usize) -> bool;
    fn writable(&self) -> bool;
    fn write(&self, text: &str) -> bool;
    /// Option `n` of a one-question form picked (and sent).
    fn pick(&self, n: usize) -> bool;
}

/// A workflow the agent launched, followed in a card of its own.
pub trait Workflow {
    fn tool_id(&self) -> String;
    fn name(&self) -> String;
    fn task(&self) -> String;
    fn done(&self) -> bool;
    /// (agents done, agents in all).
    fn counts(&self) -> (usize, usize);
    /// Stop was asked and nothing came of it: offered again.
    fn stop_unanswered(&self);
    fn ended(&self, status: &str, output: Option<PathBuf>);
    fn stopped(&self);
}

pub trait View {
    /// The UI's own type behind it (for the UI to get back its widgets).
    fn as_any(&self) -> &dyn Any;

    // The conversation.
    fn clear(&self);
    fn has_messages(&self) -> bool;
    fn user(&self, text: &str);
    fn user_chunk(&self, text: &str);
    fn user_images(&self, pngs: &[Png]);
    fn user_image_data(&self, base64: &str);
    fn agent_text_with(&self, text: &str, id: Option<&str>);
    fn thought(&self, text: &str);
    /// `class`: "dim", "error"…
    fn notice(&self, text: &str, class: &str);
    /// A notice with a button that stays.
    fn notice_action(&self, text: &str, button: &str, action: Rc<dyn Fn()>);
    /// A notice with a button that goes once pressed.
    fn notice_choice(&self, text: &str, button: &str, action: Rc<dyn Fn()>);
    /// A tool call (ACP's tool_call / tool_call_update).
    fn tool(&self, update: &Value);
    fn tool_output(&self, id: &str, data: &str);
    fn tool_exit(&self, id: &str, code: Option<i64>);
    fn tool_button(&self, id: &str, label: &str, tip: &str, action: Rc<dyn Fn()>);
    fn open_tool(&self, id: &str);
    fn go_to_tool(&self, id: &str);
    fn set_plan(&self, entries: &Value);
    fn turn_done(&self, seconds: u64);
    fn turn_undo(&self, undo: Rc<dyn Fn()>);
    /// A permission question; `options`: (id, name, kind).
    fn permission(&self, question: &str, detail: Option<&str>, markdown: bool, options: &[(String, String, String)], choose: Rc<dyn Fn(String)>) -> Rc<dyn Card>;
    fn form(&self, message: &str, schema: &Value, answer: Rc<dyn Fn(Option<Value>)>) -> Rc<dyn Form>;
    /// `alive`: whether the process that launched it still runs.
    fn workflow(&self, tool_id: &str, launch: Launch, live: bool, alive: Box<dyn Fn() -> bool>, on_changed: Rc<dyn Fn()>, on_stop: Rc<dyn Fn()>) -> Rc<dyn Workflow>;

    // Around it: the header, the strip, the usage.
    fn set_cwd(&self, cwd: &Path);
    fn set_title(&self, title: &str, place: &str, full: &str);
    fn set_state(&self, text: &str, class: &str);
    fn set_mode(&self, mode: &str);
    fn set_starting(&self, text: Option<&str>);
    fn set_working(&self, since: Option<Instant>);
    fn set_options(&self, options: &Value);
    fn set_commands(&self, commands: Vec<(String, String)>);
    fn set_suggestion(&self, text: Option<String>);
    fn set_show_thinking(&self, show: bool);
    fn set_workflow_note(&self, note: Option<String>);
    fn set_usage(&self, used: u64, size: u64);
    fn set_spend(&self, cost: Option<String>, limit: Option<(String, String)>);
    fn set_limits(&self, limits: Value);
    fn set_context_parts(&self, parts: Vec<(String, u64)>, compacts_at: Option<u64>);
    fn context_used(&self) -> u64;
    fn crew_set(&self, entry: Crew);
    fn crew_remove(&self, key: &str);
    fn crew_clear(&self);
    fn show_mcp(&self, servers: Option<Vec<(String, String, Option<String>)>>);

    // The input.
    fn draft(&self) -> String;
    fn set_draft(&self, text: &str);
    fn focus_input(&self);
    fn show_queued(&self, messages: &[String]);
    /// A message taken back from the queue, into the input.
    fn unqueue(&self, text: &str, images: Vec<Png>);
    fn restore_unsent(&self, messages: Vec<(String, Vec<Png>)>);

    // Elsewhere.
    /// A view of a subagent's conversation (read only).
    fn sub_view(&self) -> Rc<dyn View>;
    /// `view` shown in place of the conversation.
    fn show_aside(&self, view: &Rc<dyn View>, title: &str);
    fn show_main(&self);
    fn open_uri(&self, uri: &str);
    /// Job `id` (of the agent's project) shown.
    fn show_job(&self, id: Id);
}
