//! Claude Code's tool calls as ACP tool updates (title, kind, content, locations), and its
//! permission questions as ACP options. The shapes follow claude-agent-acp's reporters
//! (Apache-2.0), so cards look as they did through the adapter.

use std::path::Path;

use serde_json::{Map, Value, json};

/// A tool call Claude made.
#[derive(Clone)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// What a tool call is, before it ran.
#[derive(Default)]
struct Facts {
    title: String,
    kind: &'static str,
    locations: Option<Vec<Value>>,
    /// Diffs known from the input (an edit's old and new text).
    change: Vec<Value>,
    /// Text shown in the card (a subagent's prompt, a command's description).
    display: Vec<Value>,
    /// A command: its output comes as a terminal.
    command: bool,
}

pub fn text_content(text: &str) -> Value {
    json!({ "type": "content", "content": { "type": "text", "text": text } })
}

/// `path` relative to `cwd` when inside it.
fn display_path(path: &str, cwd: &Path) -> String {
    match Path::new(path).strip_prefix(cwd) {
        Ok(rest) if !rest.as_os_str().is_empty() => rest.to_string_lossy().into_owned(),
        _ => path.to_owned(),
    }
}

fn str_of<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input[key].as_str().filter(|s| !s.is_empty())
}

/// Text fenced as code, with a fence longer than any inside it.
pub fn markdown_escape(text: &str) -> String {
    let mut fence = "```".to_owned();
    for line in text.lines() {
        let ticks = line.chars().take_while(|c| *c == '`').count();
        while ticks >= 3 && ticks >= fence.len() {
            fence.push('`');
        }
    }
    let end = if text.ends_with('\n') { "" } else { "\n" };
    format!("{fence}\n{text}{end}{fence}")
}

fn facts(name: &str, input: &Value, cwd: &Path) -> Facts {
    match name {
        "Agent" | "Task" => {
            let effort = match (str_of(input, "effort"), str_of(input, "subagent_type")) {
                (Some(effort), sub) if sub != Some("fork") => format!(" ({effort} effort)"),
                _ => String::new(),
            };
            Facts {
                title: format!("{}{effort}", str_of(input, "description").unwrap_or("Task")),
                kind: "think",
                display: str_of(input, "prompt").map(|p| vec![text_content(p)]).unwrap_or_default(),
                ..Facts::default()
            }
        }
        "Bash" | "PowerShell" => {
            let description = str_of(input, "description").or_else(|| str_of(input, "command_description"));
            Facts {
                title: str_of(input, "command").unwrap_or("Terminal").to_owned(),
                kind: "execute",
                command: true,
                display: description.map(|d| vec![text_content(d)]).unwrap_or_default(),
                ..Facts::default()
            }
        }
        "Read" => {
            let offset = input["offset"].as_u64();
            let limit = match (input["limit"].as_u64().filter(|l| *l > 0), offset) {
                (Some(limit), offset) => {
                    let from = offset.unwrap_or(1);
                    format!(" ({from} - {})", from + limit - 1)
                }
                (None, Some(offset)) if offset > 0 => format!(" (from line {offset})"),
                _ => String::new(),
            };
            let path = str_of(input, "file_path");
            Facts {
                title: format!("Read {}{limit}", path.map(|p| display_path(p, cwd)).unwrap_or_else(|| "File".to_owned())),
                kind: "read",
                locations: Some(path.map(|p| vec![json!({ "path": p, "line": offset.unwrap_or(1).max(1) })]).unwrap_or_default()),
                ..Facts::default()
            }
        }
        "Write" => {
            let path = str_of(input, "file_path").or_else(|| str_of(input, "path"));
            let content = ["content", "file_text", "file_content"].iter().find_map(|k| input[*k].as_str());
            let mut facts = Facts {
                title: path.map(|p| format!("Write {}", display_path(p, cwd))).unwrap_or_else(|| "Preparing file…".to_owned()),
                kind: "edit",
                locations: Some(path.map(|p| vec![json!({ "path": p })]).unwrap_or_default()),
                ..Facts::default()
            };
            match (path, content) {
                (Some(path), content) => facts.change = vec![json!({ "type": "diff", "path": path, "oldText": null, "newText": content.unwrap_or("") })],
                (None, Some(content)) => facts.display = vec![text_content(content)],
                _ => {}
            }
            facts
        }
        "Edit" => {
            let path = str_of(input, "file_path");
            let mut facts = Facts {
                title: path.map(|p| format!("Edit {}", display_path(p, cwd))).unwrap_or_else(|| "Edit".to_owned()),
                kind: "edit",
                locations: Some(path.map(|p| vec![json!({ "path": p })]).unwrap_or_default()),
                ..Facts::default()
            };
            let (old, new) = (str_of(input, "old_string"), input["new_string"].as_str());
            if let Some(path) = path
                && (old.is_some() || new.is_some_and(|n| !n.is_empty()))
            {
                facts.change = vec![json!({ "type": "diff", "path": path, "oldText": old, "newText": new.unwrap_or("") })];
            }
            facts
        }
        "Glob" => {
            let mut title = "Find".to_owned();
            if let Some(path) = str_of(input, "path") {
                title.push_str(&format!(" `{path}`"));
            }
            if let Some(pattern) = str_of(input, "pattern") {
                title.push_str(&format!(" `{pattern}`"));
            }
            Facts { title, kind: "search", locations: Some(str_of(input, "path").map(|p| vec![json!({ "path": p })]).unwrap_or_default()), ..Facts::default() }
        }
        "Grep" => Facts { title: grep_title(input), kind: "search", ..Facts::default() },
        "WebFetch" => {
            let from = input["offset"].as_u64().map(|o| format!(" (from char {o})")).unwrap_or_default();
            Facts {
                title: str_of(input, "url").map(|u| format!("Fetch {u}{from}")).unwrap_or_else(|| "Fetch".to_owned()),
                kind: "fetch",
                display: str_of(input, "prompt").map(|p| vec![text_content(p)]).unwrap_or_default(),
                ..Facts::default()
            }
        }
        "WebSearch" => Facts {
            title: str_of(input, "query").map(|q| format!("Search \"{q}\"")).unwrap_or_else(|| "Web search".to_owned()),
            kind: "fetch",
            ..Facts::default()
        },
        "TodoWrite" => {
            let todos: Vec<&str> = input["todos"].as_array().into_iter().flatten().filter_map(|t| t["content"].as_str()).collect();
            let title = if input["todos"].is_array() { format!("Update TODOs: {}", todos.join(", ")) } else { "Update TODOs".to_owned() };
            Facts { title, kind: "think", ..Facts::default() }
        }
        "TaskCreate" => Facts { title: str_of(input, "subject").map(|s| format!("Create task: {s}")).unwrap_or_else(|| "Create task".to_owned()), kind: "think", ..Facts::default() },
        "TaskUpdate" => Facts { title: str_of(input, "subject").map(|s| format!("Update task: {s}")).unwrap_or_else(|| "Update task".to_owned()), kind: "think", ..Facts::default() },
        "TaskList" => Facts { title: "List tasks".to_owned(), kind: "think", ..Facts::default() },
        "TaskGet" => Facts { title: "Get task".to_owned(), kind: "think", ..Facts::default() },
        "ExitPlanMode" => Facts {
            title: "Approve Plan".to_owned(),
            kind: "switch_mode",
            display: str_of(input, "plan").map(|p| vec![text_content(p)]).unwrap_or_default(),
            ..Facts::default()
        },
        "EnterPlanMode" => Facts { title: "EnterPlanMode".to_owned(), kind: "switch_mode", ..Facts::default() },
        "AskUserQuestion" => {
            let questions: Vec<&str> = input["questions"].as_array().into_iter().flatten().filter_map(|q| q["question"].as_str()).collect();
            Facts {
                title: if questions.len() == 1 { questions[0].to_owned() } else { "Asking for your input".to_owned() },
                kind: "other",
                display: questions.iter().map(|q| text_content(q)).collect(),
                ..Facts::default()
            }
        }
        "Skill" => Facts { title: str_of(input, "skill").map(|s| format!("Load skill: {s}")).unwrap_or_else(|| "Load skill".to_owned()), kind: "other", ..Facts::default() },
        "ReportFindings" => {
            let findings = input["findings"].as_array().cloned().unwrap_or_default();
            Facts {
                title: match findings.len() {
                    0 => "Report findings: none found".to_owned(),
                    1 => "Report 1 finding".to_owned(),
                    n => format!("Report {n} findings"),
                },
                kind: "think",
                display: findings
                    .iter()
                    .map(|f| {
                        let line = f["line"].as_u64().map(|l| format!(":{l}")).unwrap_or_default();
                        text_content(&format!("**{}{line}** — {}", f["file"].as_str().unwrap_or(""), f["summary"].as_str().unwrap_or("")))
                    })
                    .collect(),
                ..Facts::default()
            }
        }
        other => Facts { title: if other.is_empty() { "Unknown Tool".to_owned() } else { other.to_owned() }, kind: "other", ..Facts::default() },
    }
}

fn grep_title(input: &Value) -> String {
    let mut title = "grep".to_owned();
    let flag = |k: &str| input[k].as_bool() == Some(true) || input[k].as_str() == Some("true");
    if flag("-i") {
        title.push_str(" -i");
    }
    if flag("-n") {
        title.push_str(" -n");
    }
    for k in ["-A", "-B", "-C"] {
        if !input[k].is_null() {
            title.push_str(&format!(" {k} {}", plain(&input[k])));
        }
    }
    match input["output_mode"].as_str() {
        Some("files_with_matches") => title.push_str(" -l"),
        Some("count") => title.push_str(" -c"),
        None if flag("-l") => title.push_str(" -l"),
        None if flag("-c") => title.push_str(" -c"),
        _ => {}
    }
    if !input["head_limit"].is_null() {
        title.push_str(&format!(" | head -{}", plain(&input["head_limit"])));
    }
    if let Some(glob) = str_of(input, "glob") {
        title.push_str(&format!(" --include=\"{glob}\""));
    }
    if let Some(kind) = str_of(input, "type") {
        title.push_str(&format!(" --type={kind}"));
    }
    if flag("multiline") {
        title.push_str(" -P");
    }
    if let Some(pattern) = str_of(input, "pattern") {
        title.push_str(&format!(" \"{pattern}\""));
    }
    if let Some(path) = str_of(input, "path").or_else(|| str_of(input, "file_path")) {
        title.push_str(&format!(" {path}"));
    }
    title
}

/// A JSON scalar as plain text (a string without its quotes).
fn plain(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string())
}

/// What the tool call shows before it runs: its terminal, its diff, else its text.
fn use_content(id: &str, facts: &Facts) -> Vec<Value> {
    let mut content = Vec::new();
    if facts.command {
        content.push(json!({ "type": "terminal", "terminalId": id }));
    }
    content.extend(facts.change.iter().cloned());
    if !facts.command {
        content.extend(facts.display.iter().cloned());
    }
    content
}

/// The `tool_call` announcing `tool`.
pub fn tool_call(tool: &ToolUse, cwd: &Path) -> Value {
    let facts = facts(&tool.name, &tool.input, cwd);
    let mut meta = json!({ "claudeCode": { "toolName": tool.name } });
    if facts.command {
        meta["terminal_info"] = json!({ "terminal_id": tool.id });
    }
    let mut update = json!({
        "_meta": meta,
        "toolCallId": tool.id,
        "sessionUpdate": "tool_call",
        "rawInput": tool.input,
        "status": "pending",
        "title": facts.title,
        "kind": facts.kind,
        "content": use_content(&tool.id, &facts),
    });
    if let Some(locations) = facts.locations {
        update["locations"] = locations.into();
    }
    update
}

/// The tool call as a permission question shows it.
pub fn permission_call(tool: &ToolUse, cwd: &Path) -> Value {
    let facts = facts(&tool.name, &tool.input, cwd);
    let mut call = json!({
        "toolCallId": tool.id,
        "status": "pending",
        "rawInput": tool.input,
        "title": facts.title,
        "kind": facts.kind,
        "content": use_content(&tool.id, &facts),
    });
    if let Some(locations) = facts.locations {
        call["locations"] = locations.into();
    }
    call
}

/// The structured result Claude Code reports for a tool (its `tool_use_result`): the files an
/// edit changed, and the result itself for whoever reads `toolResponse` (workflows, stops).
pub fn response_update(tool: &ToolUse, response: &Value) -> Value {
    let mut update = json!({
        "_meta": { "claudeCode": { "toolResponse": response, "toolName": tool.name } },
        "toolCallId": tool.id,
        "sessionUpdate": "tool_call_update",
    });
    if matches!(tool.name.as_str(), "Edit" | "Write" | "MultiEdit")
        && let Some((content, locations)) = patch_diffs(response)
    {
        update["content"] = content.into();
        update["locations"] = locations.into();
    }
    update
}

/// An edit's hunks as diffs (its `structuredPatch`).
fn patch_diffs(response: &Value) -> Option<(Vec<Value>, Vec<Value>)> {
    let path = response["filePath"].as_str()?;
    let hunks = response["structuredPatch"].as_array()?;
    let (mut content, mut locations) = (Vec::new(), Vec::new());
    for hunk in hunks {
        let (mut old, mut new) = (Vec::new(), Vec::new());
        for line in hunk["lines"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            if let Some(rest) = line.strip_prefix('-') {
                old.push(rest);
            } else if let Some(rest) = line.strip_prefix('+') {
                new.push(rest);
            } else if line == "\\ No newline at end of file" {
                continue;
            } else {
                let rest = line.get(1..).unwrap_or("");
                old.push(rest);
                new.push(rest);
            }
        }
        if !old.is_empty() || !new.is_empty() {
            locations.push(json!({ "path": path, "line": hunk["newStart"] }));
            let old = old.join("\n");
            content.push(json!({ "type": "diff", "path": path, "oldText": if old.is_empty() { Value::Null } else { old.into() }, "newText": new.join("\n") }));
        }
    }
    (!content.is_empty()).then_some((content, locations))
}

/// A tool result's content as ACP content (an error's text fenced).
fn content_update(content: &Value, error: bool) -> Option<Vec<Value>> {
    let fence = |text: &str| if error { format!("```\n{text}\n```") } else { text.to_owned() };
    match content {
        Value::String(text) if !text.is_empty() => Some(vec![text_content(&fence(text))]),
        Value::Array(blocks) if !blocks.is_empty() => Some(
            blocks
                .iter()
                .map(|block| match block["type"].as_str() {
                    Some("text") => text_content(&fence(block["text"].as_str().unwrap_or(""))),
                    Some("image") if block["source"]["type"] == "base64" => {
                        json!({ "type": "content", "content": { "type": "image", "data": block["source"]["data"], "mimeType": block["source"]["media_type"] } })
                    }
                    Some("tool_reference") => text_content(&format!("Tool: {}", block["tool_name"].as_str().unwrap_or(""))),
                    _ => text_content(&fence(&block.to_string())),
                })
                .collect(),
        ),
        _ => None,
    }
}

/// A subagent's report without Claude Code's trailers (its usage block, its id line) and
/// hand-back frame.
pub fn agent_report(text: &str) -> String {
    const HANDBACK: &str = "[Subagent hand-back] The text below is the final report of a subagent this session delegated to.";
    let mut text = text.trim_end().to_owned();
    if text.ends_with("</usage>")
        && let Some(open) = text.rfind("<usage>")
    {
        text.truncate(open);
        text = text.trim_end().to_owned();
    }
    if let Some(last) = text.rsplit('\n').next()
        && last.starts_with("agentId: ")
        && last.ends_with(')')
    {
        let keep = text.len() - last.len();
        text.truncate(keep);
        text = text.trim_end().to_owned();
    }
    if let Some(start) = text.find(HANDBACK) {
        let notes = text[..start].trim_end().to_owned();
        let report = text[start..].split_once('\n').map(|(_, r)| r).unwrap_or("");
        let report: Vec<&str> = report.lines().map(|l| l.strip_prefix("  ").unwrap_or(l)).collect();
        let report = report.join("\n");
        text = if notes.is_empty() { report } else { format!("{notes}\n\n{report}") };
    }
    text
}

/// The text of a tool result's content.
pub fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// A command's output and exit code from its result.
fn command_output(result: &Value, structured: &Value) -> (String, Option<i64>) {
    let error = result["is_error"].as_bool() == Some(true);
    let mut exit = if error { None } else { Some(0) };
    let output = if let (Some(out), Some(err)) = (structured["stdout"].as_str(), structured["stderr"].as_str())
        && structured["isImage"].as_bool() != Some(true)
        && structured["backgroundTaskId"].is_null()
    {
        let mut output = [out, err].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n");
        if structured["interrupted"].as_bool() == Some(true) {
            output = [output.as_str(), "[Command was aborted before completion]"].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n");
            exit = None;
        }
        if !structured["returnCodeInterpretation"].is_null() {
            exit = None;
        }
        if let Some(path) = structured["persistedOutputPath"].as_str() {
            let size = structured["persistedOutputSize"].as_u64().map(|s| format!(" ({s} bytes total)")).unwrap_or_default();
            output = format!("{output}\n[Output truncated{size}: full output saved to {path}]");
        }
        output
    } else {
        result_text(&result["content"])
    };
    if error && exit.is_none() {
        exit = output.strip_prefix("Exit code ").and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()).and_then(|n| n.parse().ok());
    }
    if !structured["backgroundTaskId"].is_null() {
        exit = None;
    }
    (output, exit)
}

/// The updates for a tool's result: its output (a command's goes to its terminal first),
/// its status, its content.
pub fn result_updates(tool: &ToolUse, result: &Value, structured: &Value) -> Vec<Value> {
    let error = result["is_error"].as_bool() == Some(true);
    let content = &result["content"];
    let has_error_text = error && match content {
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => false,
    };
    let mut updates = Vec::new();
    let mut fields = Map::new();
    let mut meta = json!({ "claudeCode": { "toolName": tool.name } });
    let command = matches!(tool.name.as_str(), "Bash" | "PowerShell");
    if command {
        // A failed command's text is its output too.
        let (output, exit) = command_output(result, structured);
        let interrupted = structured["interrupted"].as_bool() == Some(true);
        updates.push(json!({
            "_meta": { "terminal_output_delta": { "terminal_id": tool.id, "data": output } },
            "toolCallId": tool.id,
            "sessionUpdate": "tool_call_update",
        }));
        meta["terminal_exit"] = json!({ "terminal_id": tool.id, "exit_code": exit, "signal": null });
        fields.insert("content".into(), json!([{ "type": "terminal", "terminalId": tool.id }]));
        let status = if error || interrupted { "failed" } else { "completed" };
        fields.insert("status".into(), status.into());
    } else {
        fields.insert("status".into(), if error { "failed" } else { "completed" }.into());
        let shown = if has_error_text {
            content_update(content, true)
        } else {
            match tool.name.as_str() {
                "Read" => read_content(result, structured),
                "Agent" | "Task" => {
                    let report = if structured["status"] == "completed" && structured["content"].as_array().is_some_and(|c| !c.is_empty()) {
                        structured["content"].clone()
                    } else {
                        content.clone()
                    };
                    let text = agent_report(&result_text(&report));
                    (!text.is_empty()).then(|| vec![text_content(&text)])
                }
                "WebSearch" => {
                    let hits: Vec<String> = structured["results"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(|entry| match entry {
                            Value::String(s) => vec![s.clone()],
                            _ => entry["content"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|h| Some(format!("{} ({})", h["title"].as_str()?, h["url"].as_str()?)))
                                .collect(),
                        })
                        .collect();
                    if hits.is_empty() { content_update(content, false) } else { Some(vec![text_content(&hits.join("\n"))]) }
                }
                // Their diff came with the call and the edit's own result.
                "Edit" | "Write" | "Skill" => None,
                "ExitPlanMode" => {
                    fields.insert("title".into(), "Exited Plan Mode".into());
                    None
                }
                _ => content_update(content, false),
            }
        };
        if let Some(shown) = shown {
            fields.insert("content".into(), shown.into());
        }
        if !content.is_null() {
            fields.insert("rawOutput".into(), content.clone());
        }
    }
    let mut last = json!({ "_meta": meta, "toolCallId": tool.id, "sessionUpdate": "tool_call_update" });
    for (key, value) in fields {
        last[key] = value;
    }
    updates.push(last);
    updates
}

/// A read file's text, numbered as the file's lines.
fn read_content(result: &Value, structured: &Value) -> Option<Vec<Value>> {
    let file = &structured["file"];
    if structured["type"] == "text"
        && let Some(text) = file["content"].as_str().filter(|t| !t.is_empty())
    {
        let start = file["startLine"].as_u64().unwrap_or(1);
        let mut numbered = text.strip_suffix('\n').unwrap_or(text).split('\n').enumerate().map(|(i, line)| format!("{}\t{line}", start + i as u64)).collect::<Vec<_>>().join("\n");
        if file["truncatedByTokenCap"].as_bool() == Some(true) {
            let detail = match (file["numLines"].as_u64(), file["totalLines"].as_u64()) {
                (Some(n), Some(total)) => format!(": showing {n} of {total} lines"),
                _ => String::new(),
            };
            numbered.push_str(&format!("\n[File truncated{detail}]"));
        }
        return Some(vec![text_content(&markdown_escape(&numbered))]);
    }
    match &result["content"] {
        Value::String(text) if !text.is_empty() => Some(vec![text_content(&markdown_escape(text))]),
        Value::Array(blocks) if !blocks.is_empty() => Some(
            blocks
                .iter()
                .map(|b| match b["type"].as_str() {
                    Some("text") => text_content(&markdown_escape(b["text"].as_str().unwrap_or(""))),
                    _ => content_update(&json!([b]), false).and_then(|mut c| c.pop()).unwrap_or_else(|| text_content("")),
                })
                .collect(),
        ),
        _ => None,
    }
}

/// The todo list Claude keeps, as plan entries (TodoWrite's input).
pub fn todo_entries(input: &Value) -> Vec<Value> {
    input["todos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| Some(json!({ "content": t["content"].as_str()?, "status": t["status"].as_str().unwrap_or("pending"), "priority": "medium" })))
        .collect()
}

// ---- permissions ----------------------------------------------------------------------

pub const ALLOW_ONCE: &str = "allow-once";
pub const ALLOW_ALWAYS: &str = "allow-with-updates";
pub const REJECT: &str = "reject";
const EXIT_PLAN_AUTO: &str = "exit-plan-auto";
const EXIT_PLAN_BYPASS: &str = "exit-plan-bypass";
const EXIT_PLAN_ACCEPT_EDITS: &str = "exit-plan-accept-edits";
const EXIT_PLAN_DEFAULT: &str = "exit-plan-default";
const EXIT_PLAN_CLEAR_AUTO: &str = "exit-plan-clear-auto";
const EXIT_PLAN_CLEAR_BYPASS: &str = "exit-plan-clear-bypass";
const EXIT_PLAN_CLEAR_ACCEPT_EDITS: &str = "exit-plan-clear-accept-edits";

/// The mode a "clear context" choice on leaving plan mode carries the plan out in (None: the
/// choice keeps the context).
pub fn plan_clear_mode(option: &str) -> Option<&'static str> {
    match option {
        EXIT_PLAN_CLEAR_AUTO => Some("auto"),
        EXIT_PLAN_CLEAR_BYPASS => Some("bypassPermissions"),
        EXIT_PLAN_CLEAR_ACCEPT_EDITS => Some("acceptEdits"),
        _ => None,
    }
}

fn option(id: &str, name: &str, kind: &str) -> Value {
    json!({ "optionId": id, "name": name, "kind": kind })
}

/// The choices for a permission question about `tool` (`suggestions`: the rules Claude Code
/// proposes for "don't ask again"; `modes`: the modes there are, for leaving plan mode;
/// `used`: how full the context is, in percent, for its "clear context" choice).
pub fn permission_options(tool: &str, input: &Value, suggestions: &Value, modes: &[String], used: Option<u64>) -> Vec<Value> {
    match tool {
        "ExitPlanMode" => {
            let mut options = Vec::new();
            let elevated: Vec<&str> = ["auto", "bypassPermissions"].into_iter().filter(|m| modes.iter().any(|x| x == m)).collect();
            // A plan carried out in a fresh context (only the plan goes with it).
            if input["plan"].as_str().is_some_and(|p| !p.trim().is_empty()) {
                let usage = used.map(|u| format!(" ({u}% used)")).unwrap_or_default();
                options.push(match elevated.first() {
                    Some(&"auto") => option(EXIT_PLAN_CLEAR_AUTO, &format!("Yes, clear context{usage} and use auto mode"), "allow_always"),
                    Some(_) => option(EXIT_PLAN_CLEAR_BYPASS, &format!("Yes, clear context{usage} and bypass permissions"), "allow_always"),
                    None => option(EXIT_PLAN_CLEAR_ACCEPT_EDITS, &format!("Yes, clear context{usage} and auto-accept edits"), "allow_always"),
                });
            }
            for mode in &elevated {
                options.push(match *mode {
                    "auto" => option(EXIT_PLAN_AUTO, "Yes, and use auto mode", "allow_always"),
                    _ => option(EXIT_PLAN_BYPASS, "Yes, and bypass permissions", "allow_always"),
                });
            }
            if options.is_empty() {
                options.push(option(EXIT_PLAN_ACCEPT_EDITS, "Yes, auto-accept edits", "allow_always"));
            }
            options.push(option(EXIT_PLAN_DEFAULT, "Yes, manually approve edits", "allow_once"));
            options.push(option(REJECT, "No, keep planning", "reject_once"));
            options
        }
        "EnterPlanMode" => vec![option(ALLOW_ONCE, "Yes, enter plan mode", "allow_once"), option(REJECT, "No, start implementing now", "reject_once")],
        _ => {
            let mut options = vec![option(ALLOW_ONCE, "Yes", "allow_once")];
            if suggestions.as_array().is_some_and(|s| !s.is_empty()) {
                options.push(option(ALLOW_ALWAYS, &always_name(tool, input, suggestions), "allow_always"));
            }
            options.push(option(REJECT, "No", "reject_once"));
            options
        }
    }
}

/// "Yes, and don't ask again for …", naming what the suggested rule allows.
fn always_name(tool: &str, input: &Value, suggestions: &Value) -> String {
    let rules: Vec<&Value> = suggestions.as_array().into_iter().flatten().flat_map(|s| s["rules"].as_array().into_iter().flatten()).collect();
    let contents: Vec<&str> = rules.iter().filter_map(|r| r["ruleContent"].as_str()).collect();
    if let Some(mode) = suggestions.as_array().into_iter().flatten().find_map(|s| (s["type"] == "setMode").then(|| s["mode"].as_str()).flatten()) {
        return match mode {
            "acceptEdits" => "Yes, and auto-accept edits".to_owned(),
            other => format!("Yes, and switch to {other}"),
        };
    }
    match tool {
        "Bash" | "PowerShell" if !contents.is_empty() => format!("Yes, and don't ask again for {} commands", contents.join(", ")),
        "WebFetch" => match input["url"].as_str().and_then(|u| u.split("://").nth(1)).and_then(|r| r.split('/').next()) {
            Some(host) => format!("Yes, and don't ask again for {host}"),
            None => "Yes, and don't ask again".to_owned(),
        },
        _ if !contents.is_empty() => format!("Yes, and don't ask again for {}", contents.join(", ")),
        _ => format!("Yes, and don't ask again for {tool} commands"),
    }
}

/// Claude Code's permission result for the option picked (None: cancelled), and the mode to
/// switch to (leaving plan mode).
pub fn permission_result(option: Option<&str>, tool: &str, input: &Value, suggestions: &Value, tool_use_id: &str) -> (Value, Option<&'static str>) {
    let allow = |updates: Option<Value>, permanent: bool| {
        let mut result = json!({ "behavior": "allow", "updatedInput": input, "toolUseID": tool_use_id, "decisionClassification": if permanent { "user_permanent" } else { "user_temporary" } });
        if let Some(updates) = updates {
            result["updatedPermissions"] = updates;
        }
        result
    };
    let deny = |message: &str, interrupt: bool| {
        let mut result = json!({ "behavior": "deny", "message": message, "toolUseID": tool_use_id, "decisionClassification": "user_reject" });
        if interrupt {
            result["interrupt"] = true.into();
        }
        result
    };
    let Some(option) = option else { return (deny("Tool use aborted", true), None) };
    if tool == "ExitPlanMode" {
        let mode = match option {
            EXIT_PLAN_AUTO => Some("auto"),
            EXIT_PLAN_BYPASS => Some("bypassPermissions"),
            EXIT_PLAN_ACCEPT_EDITS => Some("acceptEdits"),
            EXIT_PLAN_DEFAULT => Some("default"),
            _ => None,
        };
        if let Some(mode) = mode {
            return (allow(Some(json!([{ "type": "setMode", "mode": mode, "destination": "session" }])), mode != "default"), Some(mode));
        }
        if option == REJECT {
            return (deny("User chose to keep planning", true), None);
        }
    }
    match option {
        ALLOW_ONCE => (allow(None, false), None),
        ALLOW_ALWAYS => (allow(Some(suggestions.clone()), true), None),
        _ => (deny("User refused permission to run tool", false), None),
    }
}

// ---- questions (AskUserQuestion) ------------------------------------------------------

/// Claude's questions as a form (one choice and one "Other" text per question).
pub fn question_form(input: &Value) -> Option<(String, Value)> {
    let questions: Vec<&Value> = input["questions"].as_array()?.iter().filter(|q| q["question"].is_string() && q["options"].as_array().is_some_and(|o| !o.is_empty())).collect();
    if questions.is_empty() {
        return None;
    }
    let single = questions.len() == 1;
    let mut properties = Map::new();
    for (index, question) in questions.iter().enumerate() {
        let options: Vec<Value> = question["options"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| {
                let mut choice = json!({ "const": o["label"], "title": o["label"] });
                if let Some(description) = o["description"].as_str() {
                    choice["description"] = description.into();
                }
                choice
            })
            .collect();
        let title = question["header"].as_str().filter(|h| !h.is_empty());
        let description = (!single).then(|| question["question"].clone());
        let field = if question["multiSelect"].as_bool() == Some(true) {
            json!({ "type": "array", "title": title, "description": description, "items": { "anyOf": options } })
        } else {
            json!({ "type": "string", "title": title, "description": description, "oneOf": options })
        };
        properties.insert(format!("question_{index}"), field);
        let hint = if question["multiSelect"].as_bool() == Some(true) {
            "Type your own answer to add to your selection above (optional)."
        } else {
            "Type your own answer, or add a note to the option you chose above (optional)."
        };
        properties.insert(format!("question_{index}_custom"), json!({ "type": "string", "title": "Other", "description": hint }));
    }
    let message = if single { questions[0]["question"].as_str().unwrap_or("").to_owned() } else { "Please answer the following questions.".to_owned() };
    Some((message, json!({ "type": "object", "properties": properties })))
}

/// The tool's input with the answers filled in from the form (None: the form was cancelled).
pub fn question_answers(input: &Value, response: &Value) -> Option<Value> {
    let questions: Vec<&Value> = input["questions"].as_array().into_iter().flatten().filter(|q| q["question"].is_string() && q["options"].as_array().is_some_and(|o| !o.is_empty())).collect();
    let mut answers = Map::new();
    let mut annotations = Map::new();
    match response["action"].as_str() {
        Some("decline") => {}
        Some("accept") => {
            let content = &response["content"];
            for (index, question) in questions.iter().enumerate() {
                let text = question["question"].as_str().unwrap_or("").to_owned();
                let custom = content[format!("question_{index}_custom")].as_str().map(str::trim).unwrap_or("").to_owned();
                let picks: Vec<String> = match &content[format!("question_{index}")] {
                    Value::Array(items) => items.iter().filter(|i| !i.is_null() && *i != "").map(plain).collect(),
                    Value::Null => Vec::new(),
                    one => vec![plain(one)],
                };
                if question["multiSelect"].as_bool() == Some(true) {
                    let mut all = picks;
                    if !custom.is_empty() {
                        all.push(custom);
                    }
                    let joined = all.iter().map(|i| if i.contains(", ") || i.contains('"') { Value::from(i.as_str()).to_string() } else { i.clone() }).collect::<Vec<_>>().join(", ");
                    if !joined.is_empty() {
                        answers.insert(text, joined.into());
                    }
                    continue;
                }
                let picked = picks.join(", ");
                if picked.is_empty() {
                    if !custom.is_empty() {
                        answers.insert(text, custom.into());
                    }
                    continue;
                }
                answers.insert(text.clone(), picked.into());
                if !custom.is_empty() {
                    annotations.insert(text, json!({ "notes": custom }));
                }
            }
        }
        _ => return None,
    }
    let mut updated = input.clone();
    updated["answers"] = answers.into();
    if !annotations.is_empty() {
        updated["annotations"] = annotations.into();
    }
    Some(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, input: Value) -> ToolUse {
        ToolUse { id: "t1".into(), name: name.into(), input }
    }

    #[test]
    fn titles_follow_the_adapter() {
        let cwd = Path::new("/home/me/p");
        assert_eq!(tool_call(&tool("Read", json!({ "file_path": "/home/me/p/src/a.rs", "offset": 10, "limit": 5 })), cwd)["title"], "Read src/a.rs (10 - 14)");
        assert_eq!(tool_call(&tool("Bash", json!({ "command": "ls -la" })), cwd)["title"], "ls -la");
        assert_eq!(tool_call(&tool("Grep", json!({ "pattern": "fn", "path": "src", "-n": true })), cwd)["title"], "grep -n \"fn\" src");
        assert_eq!(tool_call(&tool("Glob", json!({ "pattern": "**/*.rs" })), cwd)["title"], "Find `**/*.rs`");
        let edit = tool_call(&tool("Edit", json!({ "file_path": "/home/me/p/n.txt", "old_string": "a", "new_string": "b" })), cwd);
        assert_eq!(edit["title"], "Edit n.txt");
        assert_eq!(edit["content"][0]["oldText"], "a");
        let bash = tool_call(&tool("Bash", json!({ "command": "ls" })), cwd);
        assert_eq!(bash["content"][0]["type"], "terminal");
    }

    #[test]
    fn a_command_result_goes_to_its_terminal() {
        let updates = result_updates(&tool("Bash", json!({ "command": "ls" })), &json!({ "content": "a\nb", "is_error": false }), &json!({ "stdout": "a\nb", "stderr": "" }));
        assert_eq!(updates[0]["_meta"]["terminal_output_delta"]["data"], "a\nb");
        assert_eq!(updates[1]["_meta"]["terminal_exit"]["exit_code"], 0);
        assert_eq!(updates[1]["status"], "completed");
    }

    #[test]
    fn an_edit_result_carries_its_diff() {
        let response = json!({ "filePath": "/p/n.txt", "structuredPatch": [{ "newStart": 1, "lines": ["-alpha", "+beta"] }] });
        let update = response_update(&tool("Edit", json!({})), &response);
        assert_eq!(update["content"][0]["oldText"], "alpha");
        assert_eq!(update["content"][0]["newText"], "beta");
        assert_eq!(update["_meta"]["claudeCode"]["toolResponse"]["filePath"], "/p/n.txt");
    }

    #[test]
    fn answers_go_back_by_question() {
        let input = json!({ "questions": [{ "question": "Colour?", "header": "Colour", "options": [{ "label": "Red" }, { "label": "Blue" }] }] });
        let (message, schema) = question_form(&input).unwrap();
        assert_eq!(message, "Colour?");
        assert!(schema["properties"]["question_0"]["oneOf"].is_array());
        let answered = question_answers(&input, &json!({ "action": "accept", "content": { "question_0": "Blue", "question_0_custom": "dark" } })).unwrap();
        assert_eq!(answered["answers"]["Colour?"], "Blue");
        assert_eq!(answered["annotations"]["Colour?"]["notes"], "dark");
        assert!(question_answers(&input, &json!({ "action": "cancel" })).is_none());
    }

    #[test]
    fn permission_choices_and_results() {
        let suggestions = json!([{ "type": "addRules", "rules": [{ "toolName": "Bash", "ruleContent": "ls:*" }], "behavior": "allow", "destination": "localSettings" }]);
        let options = permission_options("Bash", &json!({ "command": "ls" }), &suggestions, &[], None);
        assert_eq!(options.len(), 3);
        assert_eq!(options[1]["name"], "Yes, and don't ask again for ls:* commands");
        let (result, _) = permission_result(Some(ALLOW_ALWAYS), "Bash", &json!({ "command": "ls" }), &suggestions, "t1");
        assert_eq!(result["behavior"], "allow");
        assert_eq!(result["updatedPermissions"], suggestions);
        let (result, mode) = permission_result(Some("exit-plan-default"), "ExitPlanMode", &json!({}), &Value::Null, "t1");
        assert_eq!(result["behavior"], "allow");
        assert_eq!(mode, Some("default"));
    }

    #[test]
    fn a_plan_can_go_on_in_a_fresh_context() {
        let modes = vec!["default".to_owned(), "auto".to_owned(), "bypassPermissions".to_owned()];
        let options = permission_options("ExitPlanMode", &json!({ "plan": "1. do it" }), &Value::Null, &modes, Some(12));
        assert_eq!(options[0]["name"], "Yes, clear context (12% used) and use auto mode");
        assert_eq!(plan_clear_mode(options[0]["optionId"].as_str().unwrap()), Some("auto"));
        assert_eq!(options.last().unwrap()["name"], "No, keep planning");
        let none = permission_options("ExitPlanMode", &json!({}), &Value::Null, &modes, None);
        assert!(none.iter().all(|o| plan_clear_mode(o["optionId"].as_str().unwrap()).is_none()));
    }

    #[test]
    fn agent_trailers_are_cut() {
        assert_eq!(agent_report("The answer.\nagentId: abc123 (for resume)\n<usage>tokens: 5</usage>"), "The answer.");
    }
}
