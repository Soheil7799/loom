//! An agent's view with no window (`loomd`): it draws nothing. The core records every agent's
//! view (`transcript::Recorded`), so its conversation, questions and answers are all in the
//! API all the same; this keeps only what the agent asks back of its view (its last reply, the
//! draft, a form's state).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use serde_json::Value;

use super::form::FormModel;
use super::view::{Card, Crew, Form, Png, View, Workflow};
use crate::jobs::Id;
use crate::workflow::Launch;

#[derive(Default)]
pub struct Headless {
    reply: RefCell<Option<String>>,
    messages: Cell<bool>,
    draft: RefCell<String>,
    context: Cell<u64>,
}

impl Headless {
    pub fn new() -> Rc<Headless> {
        Rc::new(Headless::default())
    }

    /// Messages taken back: before what is in the draft, a blank line between each (as the
    /// window's input has them).
    fn put_back(&self, texts: &[String]) {
        let mut parts: Vec<String> = texts.iter().filter(|t| !t.trim().is_empty()).cloned().collect();
        let draft = self.draft.borrow().clone();
        if !draft.trim().is_empty() {
            parts.push(draft);
        }
        *self.draft.borrow_mut() = parts.join("\n\n");
    }
}

/// A question's card, nowhere to be seen (the API's entry says how it went).
struct Unseen;

impl Card for Unseen {
    fn resolve(&self, _answer: &str) {}
}

/// A form filled in by the API's `answer` (whole), or here step by step.
struct HeadlessForm {
    model: RefCell<FormModel>,
    answer: Rc<dyn Fn(Option<Value>)>,
}

impl Form for HeadlessForm {
    fn card(&self) -> Rc<dyn Card> {
        Rc::new(Unseen)
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
        let content = self.content();
        match content {
            Ok(content) => {
                (self.answer)(Some(content));
                true
            }
            Err(_) => false,
        }
    }
}

/// A workflow followed by no card: it says what it is, and nothing more.
struct UnseenWorkflow {
    tool_id: String,
    name: String,
    task: String,
    done: Cell<bool>,
}

impl Workflow for UnseenWorkflow {
    fn tool_id(&self) -> String {
        self.tool_id.clone()
    }
    fn name(&self) -> String {
        self.name.clone()
    }
    fn task(&self) -> String {
        self.task.clone()
    }
    fn done(&self) -> bool {
        self.done.get()
    }
    fn counts(&self) -> (usize, usize) {
        (0, 0)
    }
    fn stop_unanswered(&self) {}
    fn ended(&self, _status: &str, _output: Option<PathBuf>) {
        self.done.set(true);
    }
    fn stopped(&self) {
        self.done.set(true);
    }
}

impl View for Headless {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clear(&self) {
        self.messages.set(false);
        self.reply.borrow_mut().take();
    }
    fn has_messages(&self) -> bool {
        self.messages.get()
    }
    fn last_reply(&self) -> Option<String> {
        self.reply.borrow().clone()
    }
    fn user(&self, _text: &str) {
        self.messages.set(true);
    }
    fn user_chunk(&self, _text: &str) {
        self.messages.set(true);
    }
    fn user_images(&self, _pngs: &[Png]) {}
    fn user_image_data(&self, _base64: &str) {}
    fn agent_text_with(&self, text: &str, _id: Option<&str>) {
        self.messages.set(true);
        self.reply.borrow_mut().get_or_insert_with(String::new).push_str(text);
    }
    fn thought(&self, _text: &str) {}
    fn notice(&self, _text: &str, _class: &str) {}
    fn notice_action(&self, _text: &str, _button: &str, _action: Rc<dyn Fn()>) {}
    fn notice_choice(&self, _text: &str, _button: &str, _action: Rc<dyn Fn()>) {}
    fn tool(&self, _update: &Value) {
        // What comes after a tool is a new reply.
        self.reply.borrow_mut().take();
    }
    fn tool_output(&self, _id: &str, _data: &str) {}
    fn tool_exit(&self, _id: &str, _code: Option<i64>) {}
    fn tool_button(&self, _id: &str, _label: &str, _tip: &str, _action: Rc<dyn Fn()>) {}
    fn open_tool(&self, _id: &str) {}
    fn go_to_tool(&self, _id: &str) {}
    fn set_plan(&self, _entries: &Value) {}
    fn turn_done(&self, _seconds: u64) {}
    fn turn_undo(&self, _undo: Rc<dyn Fn()>) {}
    fn permission(&self, _question: &str, _detail: Option<&str>, _markdown: bool, _options: &[(String, String, String)], _choose: Rc<dyn Fn(String)>) -> Rc<dyn Card> {
        Rc::new(Unseen)
    }
    fn form(&self, _message: &str, schema: &Value, answer: Rc<dyn Fn(Option<Value>)>) -> Rc<dyn Form> {
        Rc::new(HeadlessForm { model: RefCell::new(FormModel::from_schema(schema)), answer })
    }
    fn workflow(&self, tool_id: &str, launch: Launch, _live: bool, _alive: Box<dyn Fn() -> bool>, _on_changed: Rc<dyn Fn()>, _on_stop: Rc<dyn Fn()>) -> Rc<dyn Workflow> {
        Rc::new(UnseenWorkflow { tool_id: tool_id.to_owned(), name: launch.name.clone(), task: launch.task.clone(), done: Cell::new(false) })
    }

    fn set_cwd(&self, _cwd: &Path) {}
    fn set_title(&self, _title: &str, _place: &str, _full: &str) {}
    fn set_state(&self, _text: &str, _class: &str) {}
    fn set_mode(&self, _mode: &str) {}
    fn set_starting(&self, _text: Option<&str>) {}
    fn set_working(&self, since: Option<Instant>) {
        // A new turn: its reply starts over.
        if since.is_some() {
            self.reply.borrow_mut().take();
        }
    }
    fn set_options(&self, _options: &Value) {}
    fn set_commands(&self, _commands: Vec<(String, String)>) {}
    fn set_suggestion(&self, _text: Option<String>) {}
    fn set_show_thinking(&self, _show: bool) {}
    fn set_workflow_note(&self, _note: Option<String>) {}
    fn set_usage(&self, used: u64, _size: u64) {
        self.context.set(used);
    }
    fn set_spend(&self, _cost: Option<String>, _limit: Option<(String, String)>) {}
    fn set_limits(&self, _limits: Value) {}
    fn set_context_parts(&self, _parts: Vec<(String, u64)>, _compacts_at: Option<u64>) {}
    fn context_used(&self) -> u64 {
        self.context.get()
    }
    fn crew_set(&self, _entry: Crew) {}
    fn crew_remove(&self, _key: &str) {}
    fn crew_clear(&self) {}
    fn show_mcp(&self, _servers: Option<Vec<(String, String, Option<String>)>>) {}

    fn draft(&self) -> String {
        self.draft.borrow().clone()
    }
    fn set_draft(&self, text: &str) {
        *self.draft.borrow_mut() = text.to_owned();
    }
    fn focus_input(&self) {}
    fn show_queued(&self, _messages: &[String]) {}
    fn unqueue(&self, text: &str, _images: Vec<Png>) {
        self.put_back(&[text.to_owned()]);
    }
    fn restore_unsent(&self, messages: Vec<(String, Vec<Png>)>) {
        let texts: Vec<String> = messages.into_iter().map(|(text, _)| text).collect();
        self.put_back(&texts);
    }

    fn sub_view(&self, _id: &str) -> Rc<dyn View> {
        Headless::new()
    }
    fn show_aside(&self, _view: &Rc<dyn View>, _title: &str) {}
    fn show_main(&self) {}
    fn open_uri(&self, _uri: &str) {}
    fn show_job(&self, _id: Id) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_taken_back_go_before_the_draft_a_blank_line_apart() {
        let view = Headless::new();
        view.set_draft("and then this");
        view.restore_unsent(vec![("fix tests".into(), Vec::new()), ("then commit".into(), Vec::new())]);
        assert_eq!(view.draft(), "fix tests\n\nthen commit\n\nand then this");
    }
}
