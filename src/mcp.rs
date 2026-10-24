//! `loom mcp`: the MCP server loom gives every agent (stdio, one JSON-RPC message per line).
//! Its tools act on the running app through the control socket: named jobs in the tray
//! (loom's answer to herdr's named panes), files opened for the user, notifications.
//!
//! LOOM_SOCKET and LOOM_AGENT, set by loom when it starts the agent, say where and who.

use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::control;

const PROTOCOL: &str = "2025-06-18";

pub fn run() -> i32 {
    let socket = std::env::var("LOOM_SOCKET").unwrap_or_default();
    let agent: u32 = std::env::var("LOOM_AGENT").ok().and_then(|a| a.parse().ok()).unwrap_or(0);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = message.get("id").cloned() else { continue }; // notifications need no answer
        let method = message["method"].as_str().unwrap_or("");
        let reply = match method {
            "initialize" => Ok(json!({
                "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "You run inside loom, the user's agent workspace. Long-running or streaming commands (dev servers, watchers, builds, test suites, log tails, installs) go to loom's job tray with loom_run, where the user sees them, not to Bash. Read them with loom_output and loom_wait. Show the user a file you wrote for them to read or fill in with loom_open."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => Ok(call_tool(&socket, agent, &message["params"])),
            _ => Err(json!({ "code": -32601, "message": format!("unknown method {method}") })),
        };
        let response = match reply {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
        };
        if writeln!(stdout, "{response}").and_then(|()| stdout.flush()).is_err() {
            break;
        }
    }
    0
}

fn tools() -> Value {
    let name = json!({ "type": "string", "description": "The job's name, e.g. dev, test, build, logs (one job per name: running it again reuses the chip)" });
    json!([
        {
            "name": "loom_run",
            "description": "Run a shell command as a named job in loom's job tray, in the project folder, where the user sees it live. Use for anything long-running or streaming (dev server, watch mode, build, test suite, install, log tail). Returns at once; then loom_wait or loom_output. Running a name again reruns that job.",
            "inputSchema": { "type": "object", "properties": {
                "name": name,
                "command": { "type": "string", "description": "Shell command (bash -c)" },
                "restart": { "type": "boolean", "description": "Stop the job if it is still running and start this instead (default false)" }
            }, "required": ["name", "command"] }
        },
        {
            "name": "loom_output",
            "description": "The last lines a job printed, with whether it is still running and its exit code.",
            "inputSchema": { "type": "object", "properties": {
                "name": name,
                "lines": { "type": "integer", "description": "How many lines (default 60, max 2000)" }
            }, "required": ["name"] }
        },
        {
            "name": "loom_wait",
            "description": "Wait until a job exits, or until its output contains some text (e.g. 'ready on'), or a timeout. Returns the state and the last lines.",
            "inputSchema": { "type": "object", "properties": {
                "name": name,
                "until": { "type": "string", "description": "Return as soon as the output contains this text" },
                "timeout_seconds": { "type": "integer", "description": "Default 120, max 1800" },
                "lines": { "type": "integer", "description": "Lines of output to return (default 40)" }
            }, "required": ["name"] }
        },
        {
            "name": "loom_stop",
            "description": "Stop a running job (SIGTERM to its process group).",
            "inputSchema": { "type": "object", "properties": { "name": name }, "required": ["name"] }
        },
        {
            "name": "loom_jobs",
            "description": "The jobs in this project's tray: name, running, exit code.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "loom_open",
            "description": "Open a file in loom's editor panel for the user to read or fill in (an .env to complete, a document you wrote). Does not take their keyboard focus.",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string", "description": "Absolute, or relative to the agent's folder" }
            }, "required": ["path"] }
        },
        {
            "name": "loom_notify",
            "description": "Send the user a desktop notification (for something that needs them and can't wait for the end of the turn).",
            "inputSchema": { "type": "object", "properties": { "message": { "type": "string" } }, "required": ["message"] }
        },
        {
            "name": "loom_agents",
            "description": "The other agents open in loom: title, project, status.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

fn call_tool(socket: &str, agent: u32, params: &Value) -> Value {
    let args = &params["arguments"];
    let result = match params["name"].as_str().unwrap_or("") {
        "loom_wait" => wait(socket, agent, args),
        "loom_run" => control::call(socket, agent, "run_job", args.clone()).map(|v| {
            format!("Started job {} in loom's tray. Use loom_wait or loom_output to follow it.", v["name"].as_str().unwrap_or("?"))
        }),
        "loom_output" => control::call(socket, agent, "job_output", args.clone()).map(describe_job),
        "loom_stop" => control::call(socket, agent, "stop_job", args.clone()).map(|_| "Stopped.".to_owned()),
        "loom_jobs" => control::call(socket, agent, "list_jobs", json!({})).map(|v| pretty(&v)),
        "loom_open" => control::call(socket, agent, "open_file", args.clone()).map(|_| "Opened in loom's editor.".to_owned()),
        "loom_notify" => control::call(socket, agent, "notify", args.clone()).map(|_| "Sent.".to_owned()),
        "loom_agents" => control::call(socket, agent, "list_agents", json!({})).map(|v| pretty(&v)),
        other => Err(format!("unknown tool {other}")),
    };
    match result {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
        Err(error) => json!({ "content": [{ "type": "text", "text": error }], "isError": true }),
    }
}

fn wait(socket: &str, agent: u32, args: &Value) -> Result<String, String> {
    let timeout = Duration::from_secs(args["timeout_seconds"].as_u64().unwrap_or(120).min(1800));
    let until = args["until"].as_str().filter(|u| !u.is_empty());
    let lines = args["lines"].as_u64().unwrap_or(40);
    let started = Instant::now();
    loop {
        let job = control::call(socket, agent, "job_output", json!({ "name": args["name"], "lines": 400 }))?;
        let output = job["output"].as_str().unwrap_or("");
        let matched = until.is_some_and(|u| output.contains(u));
        if !job["running"].as_bool().unwrap_or(false) || matched || started.elapsed() >= timeout {
            let tail: Vec<&str> = output.lines().rev().take(lines as usize).collect::<Vec<_>>().into_iter().rev().collect();
            let mut short = job.clone();
            short["output"] = json!(tail.join("\n"));
            let why = if matched {
                "the output matched".to_owned()
            } else if job["running"].as_bool().unwrap_or(false) {
                format!("timed out after {}s; still running", timeout.as_secs())
            } else {
                "it exited".to_owned()
            };
            return Ok(format!("Stopped waiting: {why}.\n{}", describe_job(short)));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn describe_job(job: Value) -> String {
    let state = if job["running"].as_bool().unwrap_or(false) {
        "running".to_owned()
    } else {
        match job["exit_code"].as_i64() {
            Some(code) => format!("exited with {code}"),
            None => "ended".to_owned(),
        }
    };
    format!("Job {}: {state}\n--- output ---\n{}", job["name"].as_str().unwrap_or("?"), job["output"].as_str().unwrap_or(""))
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}
