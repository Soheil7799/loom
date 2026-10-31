//! An agent's view in GTK: its chat (and its project's tray) behind loom-core's `View`, and
//! the chat's buttons and keys wired to the agent.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Instant;

use gtk::{gio, glib, prelude::*};
use loom_core::agent::Agent;
use loom_core::agent::view::{Card, Crew, Form, Png, View, Workflow};
use loom_core::jobs::Id;
use serde_json::Value;

use crate::chat::{Chat, FormCard, PermissionCard};
use crate::jobs::Tray;

pub struct ChatView {
    pub chat: Rc<Chat>,
    tray: Weak<Tray>,
}

impl ChatView {
    pub fn new(chat: Rc<Chat>, tray: &Rc<Tray>) -> Rc<ChatView> {
        Rc::new(ChatView { chat, tray: Rc::downgrade(tray) })
    }
}

/// The chat an agent is shown in.
pub trait AgentChat {
    fn chat(&self) -> Rc<Chat>;
}

impl AgentChat for Agent {
    fn chat(&self) -> Rc<Chat> {
        self.view.as_any().downcast_ref::<ChatView>().expect("an agent shown in GTK").chat.clone()
    }
}

fn bytes(pngs: Vec<Png>) -> Vec<glib::Bytes> {
    pngs.into_iter().map(glib::Bytes::from_owned).collect()
}

/// The chat's buttons and keys, to `agent`.
pub fn wire(agent: &Rc<Agent>, chat: &Rc<Chat>) {
    let weak = Rc::downgrade(agent);
    *chat.on_crew_open.borrow_mut() = Some(Rc::new(move |key: String| {
        if let Some(agent) = weak.upgrade() {
            agent.open_crew(&key);
        }
    }));
    let weak = Rc::downgrade(agent);
    *chat.on_crew_stop.borrow_mut() = Some(Rc::new(move |key: String| {
        if let Some(agent) = weak.upgrade() {
            agent.stop_crew(&key);
        }
    }));
    let weak = Rc::downgrade(agent);
    *chat.on_submit.borrow_mut() = Some(Rc::new(move |(text, images): (String, Vec<glib::Bytes>)| {
        if let Some(agent) = weak.upgrade() {
            agent.submit_with(text, images.iter().map(|b| b.to_vec()).collect());
        }
    }));
    let weak = Rc::downgrade(agent);
    *chat.on_unqueue.borrow_mut() = Some(Rc::new(move |index: usize| {
        if let Some(agent) = weak.upgrade() {
            agent.unqueue(index);
        }
    }));
    let weak = Rc::downgrade(agent);
    *chat.on_send_now.borrow_mut() = Some(Rc::new(move |()| {
        if let Some(agent) = weak.upgrade() {
            agent.send_waiting_now();
        }
    }));
    let weak = Rc::downgrade(agent);
    *chat.on_rewind.borrow_mut() = Some(Rc::new(move |(at, before, text): (Option<String>, bool, String)| {
        if let Some(agent) = weak.upgrade() {
            agent.rewind(at, before, text);
        }
    }));
    let weak = Rc::downgrade(agent);
    *chat.on_rewind_files.borrow_mut() = Some(Rc::new(move |(at, before, text): (Option<String>, bool, String)| {
        if let Some(agent) = weak.upgrade() {
            agent.rewind_files_at(at, before, text);
        }
    }));
    let weak = Rc::downgrade(agent);
    *chat.on_mcp.borrow_mut() = Some(Rc::new(move |(action, server, enabled): (String, String, bool)| {
        if let Some(agent) = weak.upgrade() {
            agent.mcp(&action, &server, enabled);
        }
    }));
    let weak = Rc::downgrade(agent);
    *chat.on_cancel.borrow_mut() = Some(Rc::new(move |()| {
        if let Some(agent) = weak.upgrade() {
            agent.cancel();
        }
    }));
    let weak = Rc::downgrade(agent);
    *chat.on_cycle_mode.borrow_mut() = Some(Rc::new(move |()| {
        if let Some(agent) = weak.upgrade() {
            agent.cycle_mode();
        }
    }));
    let weak = Rc::downgrade(agent);
    *chat.on_option.borrow_mut() = Some(Rc::new(move |(id, value): (String, String)| {
        if let Some(agent) = weak.upgrade() {
            agent.set_option(&id, &value);
        }
    }));
    let weak = Rc::downgrade(agent);
    *chat.on_answer.borrow_mut() = Some(Rc::new(move |option| weak.upgrade().is_some_and(|agent| agent.answer(0, option))));
}

impl Card for PermissionCard {
    fn resolve(&self, answer: &str) {
        PermissionCard::resolve(self, answer);
    }
}

impl Form for FormCard {
    fn card(&self) -> Rc<dyn Card> {
        Rc::new(self.card.clone())
    }
    fn content(&self) -> Result<Value, String> {
        FormCard::content(self)
    }
    fn checked(&self) -> Option<Value> {
        FormCard::checked(self)
    }
    fn summary(&self) -> String {
        FormCard::summary(self)
    }
    fn choices(&self) -> Vec<(String, Vec<String>, bool, Vec<bool>)> {
        FormCard::choices(self)
    }
    fn pick_in(&self, field: usize, option: usize) -> bool {
        FormCard::pick_in(self, field, option)
    }
    fn writable(&self) -> bool {
        FormCard::writable(self)
    }
    fn write(&self, text: &str) -> bool {
        FormCard::write(self, text)
    }
    fn pick(&self, n: usize) -> bool {
        FormCard::pick(self, n)
    }
}

impl Workflow for crate::workflow::Run {
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
        crate::workflow::Run::counts(self)
    }
    fn stop_unanswered(&self) {
        crate::workflow::Run::stop_unanswered(self);
    }
    fn ended(&self, status: &str, output: Option<PathBuf>) {
        crate::workflow::Run::ended(self, status, output);
    }
    fn stopped(&self) {
        crate::workflow::Run::stopped(self);
    }
}

impl View for ChatView {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn clear(&self) {
        self.chat.clear();
    }
    fn has_messages(&self) -> bool {
        self.chat.has_messages()
    }
    fn last_reply(&self) -> Option<String> {
        self.chat.last_reply()
    }
    fn user(&self, text: &str) {
        self.chat.user(text);
    }
    fn user_chunk(&self, text: &str) {
        self.chat.user_chunk(text);
    }
    fn user_images(&self, pngs: &[Png]) {
        self.chat.user_images(&bytes(pngs.to_vec()));
    }
    fn user_image_data(&self, base64: &str) {
        self.chat.user_image_data(base64);
    }
    fn agent_text_with(&self, text: &str, id: Option<&str>) {
        self.chat.agent_text_with(text, id);
    }
    fn thought(&self, text: &str) {
        self.chat.thought(text);
    }
    fn notice(&self, text: &str, class: &str) {
        self.chat.notice(text, class);
    }
    fn notice_action(&self, text: &str, button: &str, action: Rc<dyn Fn()>) {
        self.chat.notice_action(text, button, action);
    }
    fn notice_choice(&self, text: &str, button: &str, action: Rc<dyn Fn()>) {
        self.chat.notice_choice(text, button, action);
    }
    fn tool(&self, update: &Value) {
        self.chat.tool(update);
    }
    fn tool_output(&self, id: &str, data: &str) {
        self.chat.tool_output(id, data);
    }
    fn tool_exit(&self, id: &str, code: Option<i64>) {
        self.chat.tool_exit(id, code);
    }
    fn tool_button(&self, id: &str, label: &str, tip: &str, action: Rc<dyn Fn()>) {
        self.chat.tool_button(id, label, tip, action);
    }
    fn open_tool(&self, id: &str) {
        self.chat.open_tool(id);
    }
    fn go_to_tool(&self, id: &str) {
        self.chat.go_to_tool(id);
    }
    fn set_plan(&self, entries: &Value) {
        self.chat.set_plan(entries);
    }
    fn turn_done(&self, seconds: u64) {
        self.chat.turn_done(seconds);
    }
    fn turn_undo(&self, undo: Rc<dyn Fn()>) {
        self.chat.turn_undo(undo);
    }
    fn permission(&self, question: &str, detail: Option<&str>, markdown: bool, options: &[(String, String, String)], choose: Rc<dyn Fn(String)>) -> Rc<dyn Card> {
        Rc::new(self.chat.permission(question, detail, markdown, options, choose))
    }
    fn form(&self, message: &str, schema: &Value, answer: Rc<dyn Fn(Option<Value>)>) -> Rc<dyn Form> {
        self.chat.form(message, schema, answer)
    }
    fn workflow(&self, tool_id: &str, launch: crate::workflow::Launch, live: bool, alive: Box<dyn Fn() -> bool>, on_changed: Rc<dyn Fn()>, on_stop: Rc<dyn Fn()>) -> Rc<dyn Workflow> {
        let run = crate::workflow::Run::new(tool_id, launch, live, alive);
        // An agent of it opened: its transcript, in place of the conversation.
        let chat = Rc::downgrade(&self.chat);
        *run.on_open.borrow_mut() = Some(Rc::new(move |path: PathBuf, label: String| {
            let Some(chat) = chat.upgrade() else { return };
            let viewer = Chat::viewer();
            viewer.set_show_thinking(loom_core::settings::get().show_thinking);
            crate::workflow::fill_transcript(&viewer, &path);
            chat.show_aside(viewer.root.upcast_ref(), &format!("Workflow agent · {label}"));
        }));
        *run.on_changed.borrow_mut() = Some(on_changed);
        *run.on_stop.borrow_mut() = Some(on_stop);
        self.chat.block(&run.root);
        run
    }
    fn set_cwd(&self, cwd: &Path) {
        self.chat.set_cwd(cwd);
    }
    fn set_title(&self, title: &str, place: &str, full: &str) {
        self.chat.set_title(title, place, full);
    }
    fn set_state(&self, text: &str, class: &str) {
        self.chat.set_state(text, class);
    }
    fn set_mode(&self, mode: &str) {
        self.chat.set_mode(mode);
    }
    fn set_starting(&self, text: Option<&str>) {
        self.chat.set_starting(text);
    }
    fn set_working(&self, since: Option<Instant>) {
        self.chat.set_working(since);
    }
    fn set_options(&self, options: &Value) {
        self.chat.set_options(options);
    }
    fn set_commands(&self, commands: Vec<(String, String)>) {
        self.chat.set_commands(commands);
    }
    fn set_suggestion(&self, text: Option<String>) {
        self.chat.set_suggestion(text);
    }
    fn set_show_thinking(&self, show: bool) {
        self.chat.set_show_thinking(show);
    }
    fn set_workflow_note(&self, note: Option<String>) {
        self.chat.set_workflow_note(note);
    }
    fn set_usage(&self, used: u64, size: u64) {
        self.chat.set_usage(used, size);
    }
    fn set_spend(&self, cost: Option<String>, limit: Option<(String, String)>) {
        self.chat.set_spend(cost, limit);
    }
    fn set_limits(&self, limits: Value) {
        self.chat.set_limits(limits);
    }
    fn set_context_parts(&self, parts: Vec<(String, u64)>, compacts_at: Option<u64>) {
        self.chat.set_context_parts(parts, compacts_at);
    }
    fn context_used(&self) -> u64 {
        self.chat.context_used()
    }
    fn crew_set(&self, entry: Crew) {
        self.chat.crew_set(entry);
    }
    fn crew_remove(&self, key: &str) {
        self.chat.crew_remove(key);
    }
    fn crew_clear(&self) {
        self.chat.crew_clear();
    }
    fn show_mcp(&self, servers: Option<Vec<(String, String, Option<String>)>>) {
        self.chat.show_mcp(servers);
    }
    fn draft(&self) -> String {
        self.chat.draft()
    }
    fn set_draft(&self, text: &str) {
        self.chat.input.buffer().set_text(text);
    }
    fn focus_input(&self) {
        self.chat.input.grab_focus();
    }
    fn show_queued(&self, messages: &[String]) {
        self.chat.show_queued(messages);
    }
    fn unqueue(&self, text: &str, images: Vec<Png>) {
        self.chat.unqueue(text, bytes(images));
    }
    fn restore_unsent(&self, messages: Vec<(String, Vec<Png>)>) {
        self.chat.restore_unsent(messages.into_iter().map(|(text, images)| (text, bytes(images))).collect());
    }
    fn sub_view(&self) -> Rc<dyn View> {
        Rc::new(ChatView { chat: Chat::viewer(), tray: self.tray.clone() })
    }
    fn show_aside(&self, view: &Rc<dyn View>, title: &str) {
        if let Some(view) = view.as_any().downcast_ref::<ChatView>() {
            self.chat.show_aside(view.chat.root.upcast_ref(), title);
        }
    }
    fn show_main(&self) {
        self.chat.show_main();
    }
    fn open_uri(&self, uri: &str) {
        gtk::UriLauncher::new(uri).launch(None::<&gtk::Window>, None::<&gio::Cancellable>, |_| {});
    }
    fn show_job(&self, id: Id) {
        if let Some(tray) = self.tray.upgrade()
            && let Some(job) = tray.adopt(id)
        {
            tray.peek(&job);
        }
    }
}
