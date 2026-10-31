//! Workflows an agent runs (Claude Code's Workflow tool): a card in the conversation with
//! each phase's progress and every agent of it, read from Claude Code's own files. The
//! adapter says only that one was launched (its task id and its folder); after that,
//! Claude Code writes a journal (`journal.jsonl`: a line when an agent starts, with its label
//! and phase, and one when it is done, with its answer), each agent's transcript
//! (`agent-<id>.jsonl`) beside it, and, once the workflow is over, the task's output file.
//! These are Claude Code's files, not a promised interface: what cannot be read is left out.

use std::path::{Path, PathBuf};

use serde_json::Value;

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
            let first = parts.iter().filter_map(|c| c["content"]["text"].as_str()).find(|t| !t.trim().is_empty());
            if !first.is_some_and(|t| t.starts_with(LAUNCHED)) {
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

/// The workflow task a TaskStop call stopped, if `update` is that call's result: from the
/// structured response, else from its text (a conversation read back).
pub fn stopped_task(update: &Value) -> Option<String> {
    let response = &update["_meta"]["claudeCode"]["toolResponse"];
    if response["task_type"].as_str() == Some("local_workflow") && response["message"].as_str().is_some_and(|m| m.starts_with("Successfully stopped")) {
        return response["task_id"].as_str().map(str::to_owned);
    }
    let text = update["rawOutput"].as_str()?;
    let id = text.split("Successfully stopped task: ").nth(1)?.split(|c: char| c.is_whitespace() || c == '"').next()?;
    (!id.is_empty()).then(|| id.to_owned())
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
pub fn read_from(path: &Path, from: u64) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Claude Code's temporary folder (the agents run with loom's environment).
pub fn temporary() -> PathBuf {
    std::env::var_os("CLAUDE_CODE_TMPDIR").map(PathBuf::from).unwrap_or_else(|| {
        // SAFETY: getuid cannot fail.
        std::env::temp_dir().join(format!("claude-{}", unsafe { libc::getuid() }))
    })
}

/// A background task's output, once Claude Code has written it: in its temporary folder
/// (`root`), the project's folder (named as in its projects folder), a conversation's folder,
/// then tasks/<task>.output. That conversation is the one its process started with, not
/// always the workflow's own (one resumed in another), so each of the project's is looked in.
pub fn find_output(root: &Path, dir: &Path, task: &str) -> Option<PathBuf> {
    // dir: …/projects/<project>/<session>/subagents/workflows/<run>
    let session = dir.parent()?.parent()?.parent()?;
    let project = root.join(session.parent()?.file_name()?);
    let file = format!("{task}.output");
    let own = project.join(session.file_name()?).join("tasks").join(&file);
    if own.is_file() {
        return Some(own);
    }
    std::fs::read_dir(&project).ok()?.flatten().map(|e| e.path().join("tasks").join(&file)).find(|p| p.is_file())
}

