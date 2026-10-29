//! Claude Code itself, over its stream-json protocol (`claude --input-format stream-json
//! --output-format stream-json`): one process per conversation. Its messages become the ACP
//! updates the rest of loom reads, and loom's ACP requests become its messages and control
//! requests. The shapes follow claude-agent-acp (Apache-2.0), the adapter loom used before,
//! so what loom shows is what it showed then.

mod tools;
mod transcripts;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::OnceLock;

use gtk::{gio, glib};
use serde_json::{Value, json};

use super::process::Process;
use super::{Incoming, Reply};
use tools::ToolUse;

/// The Claude Code version this provider was built and tested against (its stream-json
/// protocol is not documented; a newer Claude Code may change it).
pub const BUILT_FOR: &str = "2.1.312";

/// The installed Claude Code's version (`claude --version`), read once.
pub fn installed_version() -> Option<String> {
    static VERSION: OnceLock<Option<String>> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            let out = std::process::Command::new("claude").arg("--version").env_remove("CLAUDECODE").output().ok()?;
            String::from_utf8_lossy(&out.stdout).split_whitespace().next().map(str::to_owned)
        })
        .clone()
}

/// Permission modes, as the adapter named them.
const MODES: [(&str, &str, &str); 5] = [
    ("default", "Manual", "Always ask before making changes"),
    ("acceptEdits", "Accept edits", "Automatically accept all file edits"),
    ("plan", "Plan", "Create a plan before making changes"),
    ("auto", "Auto", "Claude handles permission decisions"),
    ("bypassPermissions", "Bypass permissions", "Accepts all permissions"),
];

/// A subagent Claude launched (its Agent tool call), shown as a session of its own.
struct Sub {
    /// Its Agent tool call's id: its session id in loom.
    key: String,
    /// Claude Code's task for it (to stop it; its permission questions name it).
    task: Option<String>,
    background: bool,
    /// Stopped by the user (its end reads "stopped").
    stopped: bool,
    ended: bool,
}

/// A question Claude Code waits on (a permission, or its own questions as a form).
struct Ask {
    request: String,
    tool: ToolUse,
    suggestions: Value,
    form: bool,
    /// Asked by a subagent: its key.
    sub: Option<String>,
    /// An MCP server's elicitation (answered as is).
    elicitation: bool,
}

pub struct Link {
    me: Weak<Link>,
    cwd: PathBuf,
    on_incoming: Rc<dyn Fn(Incoming)>,
    process: RefCell<Option<Rc<Process>>>,
    session: RefCell<Option<String>>,
    next: Cell<u64>,
    controls: RefCell<HashMap<String, Reply>>,
    /// The running session/prompt's answer, given when its turn ends.
    prompt: RefCell<Option<Reply>>,
    cancelled: Cell<bool>,
    asks: RefCell<HashMap<u64, Ask>>,
    tools: RefCell<HashMap<String, ToolUse>>,
    subs: RefCell<Vec<Sub>>,
    /// Messages whose text came streamed (their whole copy is not shown again).
    streamed: RefCell<HashSet<String>>,
    message: RefCell<String>,
    /// What Claude Code said about itself (models, commands, account…).
    init: RefCell<Value>,
    mode: RefCell<String>,
    model: RefCell<String>,
    effort: RefCell<String>,
    fast: Cell<bool>,
    used: Cell<u64>,
    size: Cell<u64>,
    cost: Cell<Option<f64>>,
    limit: RefCell<Value>,
    /// A new conversation, not named yet: its first prompt names it.
    untitled: Cell<bool>,
    /// The task list (TaskCreate / TaskUpdate) as plan entries: (task id, entry).
    tasks: RefCell<Vec<(String, Value)>>,
}

fn error(message: &str) -> Value {
    json!({ "message": message })
}

impl Link {
    pub fn new(cwd: &Path, on_incoming: Rc<dyn Fn(Incoming)>) -> std::io::Result<Rc<Link>> {
        // Claude Code starts later (with the session), but a folder that is gone fails now,
        // as starting a program in it would.
        if !cwd.is_dir() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} is not a folder", cwd.display())));
        }
        Ok(Rc::new_cyclic(|me| Link {
            me: me.clone(),
            cwd: cwd.to_owned(),
            on_incoming,
            process: RefCell::default(),
            session: RefCell::default(),
            next: Cell::new(0),
            controls: RefCell::default(),
            prompt: RefCell::default(),
            cancelled: Cell::new(false),
            asks: RefCell::default(),
            tools: RefCell::default(),
            subs: RefCell::default(),
            streamed: RefCell::default(),
            message: RefCell::default(),
            init: RefCell::new(Value::Null),
            mode: RefCell::new("default".to_owned()),
            model: RefCell::new("default".to_owned()),
            effort: RefCell::new("default".to_owned()),
            fast: Cell::new(false),
            used: Cell::new(0),
            size: Cell::new(0),
            cost: Cell::new(None),
            limit: RefCell::new(Value::Null),
            untitled: Cell::new(false),
            tasks: RefCell::default(),
        }))
    }

    pub fn pid(&self) -> u32 {
        self.process.borrow().as_ref().map_or(0, |p| p.pid())
    }

    pub fn stop(&self) {
        if let Some(process) = self.process.borrow_mut().take() {
            process.stop();
        }
    }

    fn write(&self, value: &Value) {
        if let Some(process) = self.process.borrow().as_ref() {
            process.write(value);
        }
    }

    fn session_id(&self) -> String {
        self.session.borrow().clone().unwrap_or_default()
    }

    /// A control request to Claude Code; `reply` gets its answer.
    fn control(&self, request: Value, reply: impl FnOnce(Result<Value, Value>) + 'static) {
        let id = self.next.get() + 1;
        self.next.set(id);
        let id = format!("loom-{id}");
        self.controls.borrow_mut().insert(id.clone(), Box::new(reply));
        self.write(&json!({ "type": "control_request", "request_id": id, "request": request }));
    }

    fn answer_control(&self, request: &str, response: Value) {
        self.write(&json!({ "type": "control_response", "response": { "subtype": "success", "request_id": request, "response": response } }));
    }

    /// A session update for loom (`session`: a subagent's key; None: the conversation's).
    fn send(&self, session: Option<&str>, update: Value) {
        let session = session.map(str::to_owned).unwrap_or_else(|| self.session_id());
        (self.on_incoming)(Incoming::Call { id: None, method: "session/update".to_owned(), params: json!({ "sessionId": session, "update": update }) });
    }

    // ---- loom's requests ----------------------------------------------------------------

    pub fn request(&self, method: &str, params: Value, reply: Reply) {
        match method {
            "initialize" => reply(Ok(json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": true,
                    "promptCapabilities": { "image": true, "embeddedContext": true },
                    "sessionCapabilities": { "resume": {}, "list": {}, "fork": {}, "close": {} }
                },
                "agentInfo": { "name": "claude-code", "version": installed_version() }
            }))),
            "session/new" => {
                let id = glib::uuid_string_random().to_string();
                self.untitled.set(true);
                self.start(id, false, &params, reply);
            }
            "session/resume" => {
                let Some(id) = params["sessionId"].as_str().map(str::to_owned) else { return reply(Err(error("no sessionId"))) };
                self.start(id, true, &params, reply);
            }
            "session/load" => self.load(params, reply),
            "session/prompt" => self.prompt(&params, reply),
            "_session/steering" => {
                if self.prompt.borrow().is_some() {
                    self.write_user(&params["prompt"]);
                    reply(Ok(json!({ "outcome": "steered" })));
                } else {
                    reply(Ok(json!({ "outcome": "promptRequired" })));
                }
            }
            "session/set_mode" => {
                let mode = params["modeId"].as_str().unwrap_or("default").to_owned();
                let me = self.me.clone();
                self.control(json!({ "subtype": "set_permission_mode", "mode": mode }), move |result| {
                    if let (Ok(_), Some(link)) = (&result, me.upgrade()) {
                        *link.mode.borrow_mut() = mode;
                    }
                    reply(result.map(|_| json!({})));
                });
            }
            "session/set_config_option" => self.set_option(&params, reply),
            "session/fork" => {
                let session = params["sessionId"].as_str().unwrap_or("").to_owned();
                let at = params["_meta"]["jetbrains"]["air"]["fork"]["messageId"].as_str().map(str::to_owned);
                glib::spawn_future_local(async move {
                    let result = gio::spawn_blocking(move || transcripts::fork(&session, at.as_deref())).await.unwrap_or_else(|_| Err("fork failed".to_owned()));
                    reply(result.map(|id| json!({ "sessionId": id })).map_err(|e| error(&e)));
                });
            }
            "session/close" => reply(Ok(json!({}))),
            "session/list" => {
                glib::spawn_future_local(async move {
                    let sessions = gio::spawn_blocking(transcripts::list).await.unwrap_or_default();
                    reply(Ok(json!({ "sessions": sessions })));
                });
            }
            "session/delete" => {
                let session = params["sessionId"].as_str().unwrap_or("").to_owned();
                reply(transcripts::delete(&session).map(|()| json!({})).map_err(|e| error(&e)));
            }
            _ => reply(Err(json!({ "code": -32601, "message": "Method not found" }))),
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        if method != "session/cancel" {
            return;
        }
        let session = params["sessionId"].as_str().unwrap_or("");
        // A subagent's Stop: that task only.
        let task = self.subs.borrow_mut().iter_mut().find(|s| s.key == session).and_then(|s| {
            s.stopped = true;
            s.task.clone()
        });
        if let Some(task) = task {
            self.control(json!({ "subtype": "stop_task", "task_id": task }), |_| {});
            return;
        }
        if self.subs.borrow().iter().any(|s| s.key == session) {
            return;
        }
        self.cancelled.set(true);
        self.control(json!({ "subtype": "interrupt" }), |_| {});
    }

    pub fn respond(&self, id: Value, result: Value) {
        let Some(ask) = id.as_u64().and_then(|id| self.asks.borrow_mut().remove(&id)) else { return };
        if ask.elicitation {
            let response = match result["action"].as_str() {
                Some("accept") => json!({ "action": "accept", "content": result["content"] }),
                Some("decline") => json!({ "action": "decline" }),
                _ => json!({ "action": "cancel" }),
            };
            self.answer_control(&ask.request, response);
            return;
        }
        let (response, mode) = if ask.form {
            match tools::question_answers(&ask.tool.input, &result) {
                Some(input) => (json!({ "behavior": "allow", "updatedInput": input, "toolUseID": ask.tool.id }), None),
                None => tools::permission_result(None, &ask.tool.name, &ask.tool.input, &ask.suggestions, &ask.tool.id),
            }
        } else {
            let option = (result["outcome"]["outcome"] == "selected").then(|| result["outcome"]["optionId"].as_str()).flatten();
            tools::permission_result(option, &ask.tool.name, &ask.tool.input, &ask.suggestions, &ask.tool.id)
        };
        self.answer_control(&ask.request, response);
        if let Some(mode) = mode {
            *self.mode.borrow_mut() = mode.to_owned();
            self.send(None, json!({ "sessionUpdate": "current_mode_update", "currentModeId": mode }));
        }
        if let Some(sub) = ask.sub {
            self.sub_state(&sub, json!({ "state": "running" }));
        }
    }

    pub fn respond_error(&self, id: Value, _code: i64, message: &str) {
        let Some(ask) = id.as_u64().and_then(|id| self.asks.borrow_mut().remove(&id)) else { return };
        let response = if ask.elicitation { json!({ "action": "cancel" }) } else { json!({ "behavior": "deny", "message": message, "toolUseID": ask.tool.id }) };
        self.answer_control(&ask.request, response);
    }

    // ---- the process ----------------------------------------------------------------------

    /// Starts Claude Code on conversation `id` (`resume`: an existing one), then tells it
    /// loom's settings; `reply` gets the session's modes and options.
    fn start(&self, id: String, resume: bool, params: &Value, reply: Reply) {
        self.stop();
        let mut program: Vec<String> = ["claude", "--output-format", "stream-json", "--verbose", "--input-format", "stream-json", "--permission-prompt-tool", "stdio", "--include-partial-messages"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        program.extend(if resume { ["--resume".to_owned(), id.clone()] } else { ["--session-id".to_owned(), id.clone()] });
        let servers: serde_json::Map<String, Value> = params["mcpServers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| {
                let env: serde_json::Map<String, Value> = s["env"].as_array().into_iter().flatten().filter_map(|e| Some((e["name"].as_str()?.to_owned(), e["value"].clone()))).collect();
                Some((s["name"].as_str()?.to_owned(), json!({ "command": s["command"], "args": s["args"], "env": env })))
            })
            .collect();
        if !servers.is_empty() {
            program.push("--mcp-config".to_owned());
            program.push(json!({ "mcpServers": servers }).to_string());
        }
        for dir in params["additionalDirectories"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            program.push("--add-dir".to_owned());
            program.push(dir.to_owned());
        }
        let options = &params["_meta"]["claudeCode"]["options"];
        if options["settings"].is_object() {
            program.push("--settings".to_owned());
            program.push(options["settings"].to_string());
        }
        for plugin in options["plugins"].as_array().into_iter().flatten().filter_map(|p| p["path"].as_str()) {
            program.push("--plugin-dir".to_owned());
            program.push(plugin.to_owned());
        }
        let me = self.me.clone();
        let process = match Process::spawn(&program, &self.cwd, "claude", move |line| {
            if let Some(link) = me.upgrade() {
                match line {
                    Some(message) => link.handle(message),
                    None => link.ended(),
                }
            }
        }) {
            Ok(process) => process,
            Err(err) => return reply(Err(error(&format!("could not start claude: {err}")))),
        };
        *self.process.borrow_mut() = Some(process);
        *self.session.borrow_mut() = Some(id.clone());
        let me = self.me.clone();
        self.control(json!({ "subtype": "initialize", "hooks": null, "promptSuggestions": true }), move |result| {
            let Some(link) = me.upgrade() else { return };
            match result {
                Ok(init) => {
                    if let Some(mode) = init["current_permission_mode"].as_str() {
                        *link.mode.borrow_mut() = mode.to_owned();
                    }
                    link.fast.set(init["fast_mode_state"].as_str() == Some("on"));
                    *link.init.borrow_mut() = init;
                    let commands = link.init.borrow()["commands"].clone();
                    link.commands(&commands);
                    reply(Ok(json!({ "sessionId": id, "modes": link.modes(), "configOptions": link.options() })));
                    link.context();
                }
                Err(err) => reply(Err(err)),
            }
        });
    }

    /// Reads conversation `id` back (as updates, off the main loop), then resumes it.
    fn load(&self, params: Value, reply: Reply) {
        let Some(id) = params["sessionId"].as_str().map(str::to_owned) else { return reply(Err(error("no sessionId"))) };
        let (me, cwd, read) = (self.me.clone(), self.cwd.clone(), id.clone());
        glib::spawn_future_local(async move {
            let updates = gio::spawn_blocking(move || transcripts::replay(&read, &cwd)).await.ok().flatten();
            let Some(link) = me.upgrade() else { return };
            let Some(updates) = updates else { return reply(Err(error("Session not found"))) };
            *link.session.borrow_mut() = Some(id.clone());
            for update in updates {
                link.send(None, update);
            }
            link.start(id, true, &params, reply);
        });
    }

    /// The process ended: what waited on it fails, and loom hears of it.
    fn ended(&self) {
        let controls: Vec<Reply> = self.controls.borrow_mut().drain().map(|(_, r)| r).collect();
        for reply in controls {
            reply(Err(error("the agent stopped")));
        }
        if let Some(reply) = self.prompt.borrow_mut().take() {
            reply(Err(error("the agent stopped")));
        }
        if self.process.borrow_mut().take().is_some() {
            (self.on_incoming)(Incoming::Exited);
        }
    }

    // ---- prompts --------------------------------------------------------------------------

    /// ACP prompt blocks as a user message.
    fn write_user(&self, prompt: &Value) {
        let content: Vec<Value> = prompt
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| match block["type"].as_str()? {
                "text" => Some(json!({ "type": "text", "text": block["text"] })),
                "image" => Some(json!({ "type": "image", "source": { "type": "base64", "media_type": block["mimeType"], "data": block["data"] } })),
                "resource" => block["resource"]["text"].as_str().map(|t| json!({ "type": "text", "text": t })),
                _ => None,
            })
            .collect();
        self.write(&json!({ "type": "user", "message": { "role": "user", "content": content }, "parent_tool_use_id": null, "session_id": self.session_id() }));
    }

    fn prompt(&self, params: &Value, reply: Reply) {
        if self.process.borrow().is_none() {
            return reply(Err(error("the agent is not running")));
        }
        self.cancelled.set(false);
        if let Some(old) = self.prompt.borrow_mut().replace(reply) {
            old(Ok(json!({ "stopReason": "cancelled" })));
        }
        self.write_user(&params["prompt"]);
        // A new conversation is named after its first prompt (Claude Code's own title).
        if self.untitled.replace(false) {
            let text: String = params["prompt"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n");
            if !text.trim().is_empty() && !text.trim_start().starts_with('/') {
                let me = self.me.clone();
                self.control(json!({ "subtype": "generate_session_title", "description": text, "persist": true }), move |result| {
                    if let (Ok(answer), Some(link)) = (result, me.upgrade())
                        && let Some(title) = answer["title"].as_str().filter(|t| !t.is_empty())
                    {
                        link.send(None, json!({ "sessionUpdate": "session_info_update", "title": title }));
                    }
                });
            }
        }
    }

    /// The turn is over (Claude Code's result): its answer to the prompt, and the usage.
    fn result(&self, message: &Value) {
        if let Some(cost) = message["total_cost_usd"].as_f64() {
            self.cost.set(Some(cost));
        }
        if let Some(size) = message["modelUsage"].as_object().and_then(|m| m.values().filter_map(|u| u["contextWindow"].as_u64()).max()) {
            self.size.set(size);
        }
        self.usage();
        self.context();
        let Some(reply) = self.prompt.borrow_mut().take() else { return };
        let text = message["result"].as_str().unwrap_or("").to_owned();
        let cancelled = self.cancelled.replace(false);
        if cancelled {
            return reply(Ok(json!({ "stopReason": "cancelled" })));
        }
        match message["subtype"].as_str() {
            Some("success") if message["is_error"].as_bool() == Some(true) => reply(Err(error(if text.is_empty() { "Claude Code reported an error" } else { &text }))),
            Some("success") => {
                let reason = match message["stop_reason"].as_str() {
                    Some("max_tokens") => "max_tokens",
                    Some("refusal") => "refusal",
                    _ => "end_turn",
                };
                reply(Ok(json!({ "stopReason": reason })));
            }
            Some("error_max_turns") => reply(Ok(json!({ "stopReason": "max_turn_requests" }))),
            _ => {
                let errors: Vec<&str> = message["errors"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let text = if !errors.is_empty() { errors.join("; ") } else if !text.is_empty() { text } else { "the turn failed".to_owned() };
                reply(Err(error(&text)));
            }
        }
    }

    /// How full the context is, as Claude Code counts it (the model's own window).
    fn context(&self) {
        let me = self.me.clone();
        self.control(json!({ "subtype": "get_context_usage" }), move |result| {
            let (Ok(context), Some(link)) = (result, me.upgrade()) else { return };
            if let (Some(used), Some(size)) = (context["totalTokens"].as_u64(), context["maxTokens"].as_u64()) {
                link.used.set(used);
                link.size.set(size);
                link.usage();
            }
        });
    }

    fn usage(&self) {
        // The window unknown yet (size 0): the count alone.
        let mut update = json!({ "sessionUpdate": "usage_update", "used": self.used.get(), "size": self.size.get() });
        if let Some(cost) = self.cost.get() {
            update["cost"] = json!({ "amount": cost, "currency": "USD" });
        }
        if !self.limit.borrow().is_null() {
            update["_meta"] = json!({ "_claude/rateLimit": self.limit.borrow().clone() });
        }
        self.send(None, update);
    }

    // ---- options --------------------------------------------------------------------------

    fn modes(&self) -> Value {
        let available: Vec<Value> = MODES.iter().map(|(id, name, description)| json!({ "id": id, "name": name, "description": description })).collect();
        json!({ "currentModeId": self.mode.borrow().clone(), "availableModes": available })
    }

    /// The pickers: mode, model, effort (when the model has one), fast mode.
    fn options(&self) -> Value {
        let init = self.init.borrow();
        let mut options = vec![json!({
            "id": "mode", "name": "Mode", "category": "mode", "type": "select",
            "currentValue": self.mode.borrow().clone(),
            "options": MODES.iter().map(|(id, name, description)| json!({ "value": id, "name": name, "description": description })).collect::<Vec<_>>(),
        })];
        let models = init["models"].as_array().cloned().unwrap_or_default();
        if !models.is_empty() {
            options.push(json!({
                "id": "model", "name": "Model", "description": "The model Claude uses", "category": "model", "type": "select",
                "currentValue": self.model.borrow().clone(),
                "options": models.iter().map(|m| json!({ "value": m["value"], "name": m["displayName"].as_str().or(m["value"].as_str()), "description": m["description"] })).collect::<Vec<_>>(),
            }));
        }
        let current = models.iter().find(|m| m["value"].as_str() == Some(self.model.borrow().as_str())).or_else(|| models.first());
        let levels: Vec<&str> = current
            .filter(|m| m["supportsEffort"].as_bool() == Some(true))
            .map(|m| m["supportedEffortLevels"].as_array().into_iter().flatten().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !levels.is_empty() {
            let name = |l: &str| l.split(['_', '-']).map(|p| { let mut c = p.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }).collect::<Vec<_>>().join(" ");
            let mut choices = vec![json!({ "value": "default", "name": "Default" })];
            choices.extend(levels.iter().map(|l| json!({ "value": l, "name": name(l) })));
            let effort = self.effort.borrow().clone();
            let current = if effort == "default" || levels.contains(&effort.as_str()) { effort } else { "default".to_owned() };
            options.push(json!({ "id": "effort", "name": "Effort", "description": "Available effort levels for this model", "category": "thought_level", "type": "select", "currentValue": current, "options": choices }));
        }
        if init["fast_mode_state"].as_str().is_some_and(|s| s != "unavailable") {
            options.push(json!({
                "id": "fast", "name": "Fast mode", "description": "Faster output from the same model", "category": "model_config", "type": "select",
                "currentValue": if self.fast.get() { "on" } else { "off" },
                "options": [{ "value": "on", "name": "On" }, { "value": "off", "name": "Off" }],
            }));
        }
        Value::Array(options)
    }

    fn set_option(&self, params: &Value, reply: Reply) {
        let value = params["value"].as_str().unwrap_or("default").to_owned();
        let (request, apply): (Value, Box<dyn Fn(&Link)>) = match params["configId"].as_str() {
            Some("model") => {
                let v = value.clone();
                (json!({ "subtype": "set_model", "model": if value == "default" { Value::Null } else { value.clone().into() } }), Box::new(move |l: &Link| *l.model.borrow_mut() = v.clone()))
            }
            Some("effort") => {
                let v = value.clone();
                (json!({ "subtype": "apply_flag_settings", "settings": { "effortLevel": if value == "default" { Value::Null } else { value.clone().into() } } }), Box::new(move |l: &Link| *l.effort.borrow_mut() = v.clone()))
            }
            Some("fast") => {
                let on = value == "on";
                (json!({ "subtype": "apply_flag_settings", "settings": { "fastMode": on } }), Box::new(move |l: &Link| l.fast.set(on)))
            }
            Some("mode") => {
                let v = value.clone();
                (json!({ "subtype": "set_permission_mode", "mode": value }), Box::new(move |l: &Link| *l.mode.borrow_mut() = v.clone()))
            }
            _ => return reply(Err(error("unknown option"))),
        };
        let me = self.me.clone();
        self.control(request, move |result| {
            let Some(link) = me.upgrade() else { return };
            match result {
                Ok(_) => {
                    apply(&link);
                    reply(Ok(json!({ "configOptions": link.options() })));
                }
                Err(err) => reply(Err(err)),
            }
        });
    }

    fn commands(&self, commands: &Value) {
        let available: Vec<Value> = commands
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| Some(json!({ "name": c["name"].as_str()?, "description": c["description"].as_str().unwrap_or(""), "input": c["argumentHint"].as_str().filter(|h| !h.is_empty()).map(|h| json!({ "hint": h })) })))
            .collect();
        self.send(None, json!({ "sessionUpdate": "available_commands_update", "availableCommands": available }));
    }

    // ---- Claude Code's messages -----------------------------------------------------------

    fn handle(&self, message: Value) {
        match message["type"].as_str().unwrap_or("") {
            "control_response" => {
                let response = &message["response"];
                let Some(reply) = response["request_id"].as_str().and_then(|id| self.controls.borrow_mut().remove(id)) else { return };
                match response["subtype"].as_str() {
                    Some("success") => reply(Ok(response["response"].clone())),
                    _ => reply(Err(error(response["error"].as_str().unwrap_or("Claude Code refused")))),
                }
            }
            "control_request" => self.asked(&message),
            "system" => self.system(&message),
            "stream_event" => self.streamed_event(&message),
            "assistant" => self.assistant(&message),
            "user" => self.user(&message),
            "result" => self.result(&message),
            "rate_limit_event" => {
                let info = &message["rate_limit_info"];
                let kind = info["rateLimitType"].as_str().unwrap_or("");
                let window = &info["unifiedWindows"][kind];
                let utilization = window["utilization"].as_f64().or_else(|| info["utilization"].as_f64());
                if let Some(utilization) = utilization {
                    *self.limit.borrow_mut() = json!({ "utilization": utilization, "rateLimitType": kind, "resetsAt": window["resetsAt"].as_i64().or_else(|| info["resetsAt"].as_i64()) });
                    self.usage();
                }
            }
            "prompt_suggestion" => {
                let text = message["suggestion"].as_str().unwrap_or("");
                self.send(None, json!({ "sessionUpdate": "session_info_update", "_meta": { "promptSuggestions": { "suggestions": [{ "text": text }] } } }));
            }
            _ => {}
        }
    }

    fn system(&self, message: &Value) {
        match message["subtype"].as_str().unwrap_or("") {
            "status" => {
                if message["status"].as_str() == Some("compacting") {
                    self.send(None, json!({ "sessionUpdate": "compaction_update", "status": "in_progress" }));
                }
                if let Some(mode) = message["permissionMode"].as_str().filter(|m| *m != self.mode.borrow().as_str()) {
                    *self.mode.borrow_mut() = mode.to_owned();
                    self.send(None, json!({ "sessionUpdate": "current_mode_update", "currentModeId": mode }));
                }
            }
            "compact_boundary" => self.send(None, json!({ "sessionUpdate": "compaction_update", "status": "completed" })),
            "commands_changed" => self.commands(&message["commands"]),
            "api_retry" => {
                let attempt = message["attempt"].as_u64().map(|a| format!(" (attempt {a})")).unwrap_or_default();
                self.send(None, json!({ "sessionUpdate": "notice", "title": format!("Retrying the request{attempt}"), "description": message["error"].as_str(), "severity": "info" }));
            }
            "task_started" => {
                let (task, tool) = (message["task_id"].as_str(), message["tool_use_id"].as_str());
                if let (Some(task), Some(tool)) = (task, tool)
                    && let Some(sub) = self.subs.borrow_mut().iter_mut().find(|s| s.key == tool)
                {
                    sub.task = Some(task.to_owned());
                    sub.background |= message["is_backgrounded"].as_bool() == Some(true);
                }
            }
            "task_updated" => {
                let task = message["task_id"].as_str();
                if message["patch"]["is_backgrounded"].as_bool() == Some(true)
                    && let Some(sub) = self.subs.borrow_mut().iter_mut().find(|s| s.task.as_deref() == task)
                {
                    sub.background = true;
                }
            }
            "task_notification" => {
                let task = message["task_id"].as_str();
                let tool = message["tool_use_id"].as_str();
                let key = self.subs.borrow().iter().find(|s| s.task.as_deref() == task || tool == Some(s.key.as_str())).map(|s| s.key.clone());
                let Some(key) = key else { return };
                let status = message["status"].as_str().unwrap_or("completed");
                if status == "stopped" {
                    self.with_sub(&key, |s| s.stopped = true);
                }
                // A background subagent's report comes only here.
                let background = self.subs.borrow().iter().any(|s| s.key == key && s.background);
                if background {
                    let summary = message["summary"].as_str().unwrap_or("").to_owned();
                    self.sub_done(&key, status == "failed", (!summary.is_empty()).then_some(summary));
                }
            }
            _ => {}
        }
    }

    fn with_sub(&self, key: &str, f: impl FnOnce(&mut Sub)) {
        if let Some(sub) = self.subs.borrow_mut().iter_mut().find(|s| s.key == key) {
            f(sub);
        }
    }

    /// The subagent the message belongs to (its parent tool call: an Agent call).
    fn sub_of(&self, parent: Option<&str>) -> Option<String> {
        let parent = parent?;
        self.subs.borrow().iter().find(|s| s.key == parent).map(|s| s.key.clone())
    }

    fn streamed_event(&self, message: &Value) {
        // A subagent's words come whole, with its messages.
        if !message["parent_tool_use_id"].is_null() {
            return;
        }
        let event = &message["event"];
        match event["type"].as_str().unwrap_or("") {
            "message_start" => {
                let id = event["message"]["id"].as_str().unwrap_or("").to_owned();
                self.streamed.borrow_mut().insert(id.clone());
                *self.message.borrow_mut() = id;
            }
            "content_block_delta" => {
                let delta = &event["delta"];
                let id = self.message.borrow().clone();
                match delta["type"].as_str() {
                    Some("text_delta") => self.send(None, json!({ "sessionUpdate": "agent_message_chunk", "messageId": id, "content": { "type": "text", "text": delta["text"] } })),
                    Some("thinking_delta") if delta["thinking"].as_str().is_some_and(|t| !t.is_empty()) => {
                        self.send(None, json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": delta["thinking"] } }))
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                let usage = &event["usage"];
                let used = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"].iter().filter_map(|k| usage[*k].as_u64()).sum::<u64>();
                if used > 0 {
                    self.used.set(used);
                    self.usage();
                }
            }
            _ => {}
        }
    }

    fn assistant(&self, message: &Value) {
        let parent = message["parent_tool_use_id"].as_str();
        let sub = self.sub_of(parent);
        let id = message["message"]["id"].as_str().unwrap_or("").to_owned();
        let streamed = sub.is_none() && self.streamed.borrow().contains(&id);
        for block in message["message"]["content"].as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") if !streamed => self.send(sub.as_deref(), json!({ "sessionUpdate": "agent_message_chunk", "messageId": id, "content": { "type": "text", "text": block["text"] } })),
                Some("thinking") if !streamed && block["thinking"].as_str().is_some_and(|t| !t.is_empty()) => {
                    self.send(sub.as_deref(), json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": block["thinking"] } }))
                }
                Some("tool_use") => {
                    let tool = ToolUse { id: block["id"].as_str().unwrap_or("").to_owned(), name: block["name"].as_str().unwrap_or("").to_owned(), input: block["input"].clone() };
                    self.tools.borrow_mut().insert(tool.id.clone(), tool.clone());
                    if matches!(tool.name.as_str(), "Agent" | "Task") {
                        self.launch(&tool, sub.as_deref());
                        continue;
                    }
                    self.send(sub.as_deref(), tools::tool_call(&tool, &self.cwd));
                    if tool.name == "TodoWrite" {
                        self.send(sub.as_deref(), json!({ "sessionUpdate": "plan", "entries": tools::todo_entries(&tool.input) }));
                    }
                }
                _ => {}
            }
        }
    }

    /// A subagent launched: its session, its prompt (a card in the conversation it came from).
    fn launch(&self, tool: &ToolUse, from: Option<&str>) {
        let title = tool.input["description"].as_str().filter(|d| !d.is_empty()).unwrap_or("Subagent").to_owned();
        let prompt = tool.input["prompt"].as_str().unwrap_or("").to_owned();
        self.subs.borrow_mut().push(Sub { key: tool.id.clone(), task: None, background: tool.input["run_in_background"].as_bool() == Some(true), stopped: false, ended: false });
        self.send(None, json!({ "sessionUpdate": "subagent_update", "sessionId": tool.id, "description": title, "state": { "state": "running" } }));
        let sender = from.map(str::to_owned).unwrap_or_else(|| self.session_id());
        let launch = json!({ "sessionUpdate": "session_message", "senderSessionId": sender, "recipientSessionId": tool.id, "messageId": tool.id, "content": [{ "type": "text", "text": prompt }] });
        self.send(None, launch.clone());
        self.send(Some(&tool.id), launch);
    }

    fn sub_state(&self, key: &str, state: Value) {
        self.send(None, json!({ "sessionUpdate": "subagent_update", "sessionId": key, "state": state }));
    }

    /// A subagent is done: its report back to the conversation, its state idle.
    fn sub_done(&self, key: &str, failed: bool, report: Option<String>) {
        let (stopped, already) = {
            let mut subs = self.subs.borrow_mut();
            let Some(sub) = subs.iter_mut().find(|s| s.key == key) else { return };
            (sub.stopped, std::mem::replace(&mut sub.ended, true))
        };
        if already {
            return;
        }
        if let Some(report) = report.filter(|r| !r.is_empty()) {
            let back = json!({ "sessionUpdate": "session_message", "senderSessionId": key, "recipientSessionId": self.session_id(), "messageId": format!("{key}:report"), "content": [{ "type": "text", "text": report }] });
            self.send(Some(key), back.clone());
            self.send(None, back);
        }
        let mut state = json!({ "state": "idle" });
        if stopped {
            state["stopReason"] = "cancelled".into();
        } else if failed {
            state["error"] = "it failed".into();
        }
        self.sub_state(key, state);
    }

    fn user(&self, message: &Value) {
        let parent = message["parent_tool_use_id"].as_str();
        let sub = self.sub_of(parent);
        let structured = &message["tool_use_result"];
        let Some(blocks) = message["message"]["content"].as_array() else { return };
        for block in blocks.iter().filter(|b| b["type"] == "tool_result") {
            let Some(tool) = block["tool_use_id"].as_str().and_then(|id| self.tools.borrow().get(id).cloned()) else { continue };
            if matches!(tool.name.as_str(), "Agent" | "Task") {
                let background = self.subs.borrow().iter().any(|s| s.key == tool.id && s.background) || structured["status"] == "async_launched";
                if background {
                    self.with_sub(&tool.id, |s| s.background = true);
                } else {
                    let report = if structured["status"] == "completed" && structured["content"].as_array().is_some_and(|c| !c.is_empty()) { &structured["content"] } else { &block["content"] };
                    self.sub_done(&tool.id, block["is_error"].as_bool() == Some(true), Some(tools::agent_report(&tools::result_text(report))));
                }
                continue;
            }
            if structured.is_object() || structured.is_array() {
                self.send(sub.as_deref(), tools::response_update(&tool, structured));
            }
            for update in tools::result_updates(&tool, block, structured) {
                self.send(sub.as_deref(), update);
            }
            if sub.is_none() {
                self.task_list(&tool, structured, block["is_error"].as_bool() == Some(true));
            }
        }
    }

    /// Claude Code's task list (TaskCreate, TaskUpdate) as the plan.
    fn task_list(&self, tool: &ToolUse, structured: &Value, failed: bool) {
        if failed {
            return;
        }
        match tool.name.as_str() {
            "TaskCreate" => {
                let id = structured["task"]["id"].as_str().map(str::to_owned).or_else(|| structured["task"]["id"].as_u64().map(|n| n.to_string())).unwrap_or_default();
                let content = tool.input["subject"].as_str().unwrap_or("").to_owned();
                self.tasks.borrow_mut().push((id, json!({ "content": content, "status": "pending", "priority": "medium" })));
            }
            "TaskUpdate" => {
                let id = tool.input["taskId"].as_str().map(str::to_owned).or_else(|| tool.input["taskId"].as_u64().map(|n| n.to_string())).unwrap_or_default();
                let mut tasks = self.tasks.borrow_mut();
                if tool.input["status"] == "deleted" {
                    tasks.retain(|(t, _)| *t != id);
                } else if let Some((_, entry)) = tasks.iter_mut().find(|(t, _)| *t == id) {
                    if let Some(status) = tool.input["status"].as_str() {
                        entry["status"] = status.into();
                    }
                    if let Some(subject) = tool.input["subject"].as_str() {
                        entry["content"] = subject.into();
                    }
                }
            }
            _ => return,
        }
        let entries: Vec<Value> = self.tasks.borrow().iter().map(|(_, e)| e.clone()).collect();
        self.send(None, json!({ "sessionUpdate": "plan", "entries": entries }));
    }

    /// Claude Code asks something: a permission, its own questions, an MCP server's form.
    fn asked(&self, message: &Value) {
        let request_id = message["request_id"].as_str().unwrap_or("").to_owned();
        let request = &message["request"];
        let rpc = self.next.get() + 1;
        self.next.set(rpc);
        match request["subtype"].as_str() {
            Some("can_use_tool") => {
                let tool = ToolUse { id: request["tool_use_id"].as_str().unwrap_or("").to_owned(), name: request["tool_name"].as_str().unwrap_or("").to_owned(), input: request["input"].clone() };
                let sub = request["agent_id"].as_str().and_then(|agent| self.subs.borrow().iter().find(|s| s.task.as_deref() == Some(agent)).map(|s| s.key.clone()));
                let session = sub.clone().unwrap_or_else(|| self.session_id());
                if tool.name == "AskUserQuestion"
                    && let Some((text, schema)) = tools::question_form(&tool.input)
                {
                    self.asks.borrow_mut().insert(rpc, Ask { request: request_id, tool: tool.clone(), suggestions: Value::Null, form: true, sub: sub.clone(), elicitation: false });
                    if let Some(sub) = &sub {
                        self.sub_state(sub, json!({ "state": "requires_action" }));
                    }
                    (self.on_incoming)(Incoming::Call { id: Some(rpc.into()), method: "elicitation/create".to_owned(), params: json!({ "mode": "form", "sessionId": session, "toolCallId": tool.id, "message": text, "requestedSchema": schema }) });
                    return;
                }
                let suggestions = request["permission_suggestions"].clone();
                let modes: Vec<String> = MODES.iter().map(|m| m.0.to_owned()).collect();
                let options = tools::permission_options(&tool.name, &tool.input, &suggestions, &modes);
                let mut call = tools::permission_call(&tool, &self.cwd);
                if let Some(title) = request["title"].as_str() {
                    call["title"] = title.into();
                }
                self.asks.borrow_mut().insert(rpc, Ask { request: request_id, tool, suggestions, form: false, sub: sub.clone(), elicitation: false });
                if let Some(sub) = &sub {
                    self.sub_state(sub, json!({ "state": "requires_action" }));
                }
                (self.on_incoming)(Incoming::Call { id: Some(rpc.into()), method: "session/request_permission".to_owned(), params: json!({ "sessionId": session, "toolCall": call, "options": options }) });
            }
            Some("elicitation") => {
                let mut params = json!({ "mode": request["mode"].as_str().unwrap_or("form"), "sessionId": self.session_id(), "message": request["message"] });
                if let Some(url) = request["url"].as_str() {
                    params["url"] = url.into();
                }
                params["requestedSchema"] = if request["requested_schema"].is_object() { request["requested_schema"].clone() } else { request["requestedSchema"].clone() };
                self.asks.borrow_mut().insert(rpc, Ask { request: request_id, tool: ToolUse { id: String::new(), name: String::new(), input: Value::Null }, suggestions: Value::Null, form: false, sub: None, elicitation: true });
                (self.on_incoming)(Incoming::Call { id: Some(rpc.into()), method: "elicitation/create".to_owned(), params });
            }
            _ => {
                self.write(&json!({ "type": "control_response", "response": { "subtype": "error", "request_id": request_id, "error": "loom does not handle this request" } }));
            }
        }
    }
}
