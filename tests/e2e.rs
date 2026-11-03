//! End to end: a real loom, started for the test with folders of its own, driven only through
//! its API socket by `loom api`'s client (what the screenshot checks used to do by hand).
//!
//! It opens a window, so it needs a display and is not run by default:
//!     cargo test --test e2e -- --ignored --nocapture
//! With LOOM_E2E_AGENT=1 it also has Claude Code do a turn that asks a question, answered
//! through the API (a few cents of use).

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use loom_core::api::cli::{Connection, new_id};
use loom_core::api::{Applied, Body, Message, Mirror, Op, wire};
use serde_json::{Value, json};

const APP_ID: &str = "dev.shirvani.LoomE2e";

/// The loom under test; closed when dropped.
struct Instance {
    child: Child,
    root: PathBuf,
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn start() -> (Instance, Connection) {
    start_as(APP_ID, |_| {}, &[])
}

/// A loom of app id `app_id`, its folders made and `prepare`d (a settings file, say) before it
/// starts with `env` set too.
fn start_as(app_id: &str, prepare: impl FnOnce(&Path), env: &[(&str, String)]) -> (Instance, Connection) {
    let root = std::env::temp_dir().join(format!("loom-e2e-{}-{}", std::process::id(), app_id.rsplit('.').next().unwrap_or("x")));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["config/loom", "data", "state", "project", "vault"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    prepare(&root);
    let child = Command::new(env!("CARGO_BIN_EXE_loom"))
        .env("LOOM_APP_ID", app_id)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env_remove("LOOM_API")
        // Only a test instance lets the CLI answer agents' questions.
        .env("LOOM_TEST_INSTANCE", "1")
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .spawn()
        .expect("loom starts");
    let instance = Instance { child, root };
    let socket = loom_core::api::socket::socket_path(app_id);
    let deadline = Instant::now() + Duration::from_secs(30);
    let connection = loop {
        if let Ok(connection) = Connection::open(&socket) {
            break connection;
        }
        assert!(Instant::now() < deadline, "loom's API socket never came up at {}", socket.display());
        std::thread::sleep(Duration::from_millis(200));
    };
    (instance, connection)
}

/// Does command `method`, which must succeed.
fn command(c: &mut Connection, method: &str, params: Value) -> Value {
    c.command(&new_id(), method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// A followed resource: its copy, kept by applying what comes.
struct Followed {
    sub: String,
    mirror: Mirror,
    /// Every message, in order (for the byte streams).
    seen: Vec<Message>,
}

fn follow(c: &mut Connection, resource: &str) -> Followed {
    // Some resources come a moment after what makes them (a job's, once it is added).
    let deadline = Instant::now() + Duration::from_secs(10);
    let sub = loop {
        match c.subscribe(resource) {
            Ok(sub) => break sub,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => panic!("subscribe {resource}: {e}"),
        }
    };
    Followed { sub, mirror: Mirror::default(), seen: Vec::new() }
}

thread_local! {
    /// Messages read while waiting on another subscription, in order.
    static STASH: std::cell::RefCell<Vec<Message>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Applies messages to `followed` until `done` holds for it (or `within` passes: a panic
/// saying `what`). Messages of other subscriptions wait in `STASH`.
fn until(c: &mut Connection, followed: &mut Followed, within: Duration, what: &str, done: impl Fn(&Followed) -> bool) {
    let deadline = Instant::now() + within;
    while !(followed.mirror.synced && done(followed)) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            timed_out(what, &followed.mirror.items);
        }
        let kept = STASH.with_borrow_mut(|stash| stash.iter().position(|m| m.sub == followed.sub).map(|at| stash.remove(at)));
        let message = match kept {
            Some(message) => message,
            None => {
                c.set_read_timeout(Some(left)).unwrap();
                let message = match c.next_message() {
                    Ok(Some(message)) => message,
                    Ok(None) => panic!("loom closed the connection"),
                    Err(_) => timed_out(what, &followed.mirror.items),
                };
                c.set_read_timeout(None).unwrap();
                message
            }
        };
        if message.sub != followed.sub {
            // Another subscription's: kept for when it is waited on.
            STASH.with_borrow_mut(|stash| stash.push(message));
            continue;
        }
        if followed.mirror.apply(&message) == Applied::Gap {
            c.request("resync", json!({ "sub": followed.sub })).unwrap();
        }
        followed.seen.push(message);
    }
}

fn timed_out(what: &str, items: &[Value]) -> ! {
    panic!("timed out waiting for {what}; items: {:#}", Value::Array(items.to_vec()))
}

#[test]
#[ignore = "opens a window: cargo test --test e2e -- --ignored"]
fn loom_through_its_api() {
    let (instance, mut c) = start();
    let project = instance.root.join("project");
    handshake(&c);
    eprintln!("e2e: handshake ok");
    settings_round_trip(&mut c);
    eprintln!("e2e: settings_round_trip ok");
    let agent = command(&mut c, "new_agent", json!({ "folder": project }))["agent"].as_str().unwrap().to_owned();
    agents_list(&mut c, &agent, &project);
    eprintln!("e2e: agents_list ok");
    job_bytes_and_rerun(&mut c, &project);
    eprintln!("e2e: job_bytes_and_rerun ok");
    vault_follows_files(&mut c, &instance.root.join("vault"));
    eprintln!("e2e: vault_follows_files ok");
    if std::env::var_os("LOOM_E2E_AGENT").is_some() {
        agent_turn_with_question(&mut c, &agent);
    eprintln!("e2e: agent_turn_with_question ok");
    }
    window_agrees_with_the_core(&mut c, &agent);
    eprintln!("e2e: window_agrees_with_the_core ok");
    closing_an_agent_ends_its_conversation(&mut c, &agent);
    eprintln!("e2e: closing_an_agent_ends_its_conversation ok");
    audit_log_has_every_command(&instance.root);
    eprintln!("e2e: audit_log_has_every_command ok");
}

fn handshake(c: &Connection) {
    assert_eq!(c.hello["protocol"], 1);
    assert_eq!(c.hello["principal"]["kind"], "cli");
    let commands: Vec<&str> = c.hello["capabilities"]["commands"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for needed in ["send", "answer", "stop", "run_job", "restart_job", "set_settings", "new_agent", "blob"] {
        assert!(commands.contains(&needed), "no {needed} in {commands:?}");
    }
}

fn settings_round_trip(c: &mut Connection) {
    let before = c.get("settings").unwrap();
    let item = &before[0];
    assert_eq!(item["id"], "settings");
    // Secrets never: only whether they are set.
    assert!(item.get("telegram_token").is_none() && item["telegram_token_set"] == false);
    let scale = item["text_scale"].as_f64().unwrap();
    command(c, "set_settings", json!({ "changes": { "text_scale": scale + 0.25 } }));
    let after = c.get("settings").unwrap();
    assert_eq!(after[0]["text_scale"].as_f64(), Some(scale + 0.25));
    let wrong = c.command(&new_id(), "set_settings", json!({ "changes": { "telegram_token_set": true } }));
    assert!(wrong.is_err(), "a made-up or read-only setting is refused");
    // What may send things off this machine is the window's to change.
    for change in [json!({ "telegram_enabled": true }), json!({ "remote_enabled": true }), json!({ "default_mode": "bypassPermissions" })] {
        let refused = c.command(&new_id(), "set_settings", json!({ "changes": change }));
        assert!(refused.as_ref().is_err_and(|e| e.message.contains("window")), "{change} from the CLI: {refused:?}");
    }
}

fn agents_list(c: &mut Connection, agent: &str, project: &Path) {
    let agents = c.get("agents").unwrap();
    let mine = agents.iter().find(|a| a["id"] == agent).expect("the new agent is listed");
    assert_eq!(mine["project_id"], project.to_string_lossy().as_ref());
    let projects = c.get("projects").unwrap();
    assert!(projects.iter().any(|p| p["id"] == project.to_string_lossy().as_ref()));
}

fn job_bytes_and_rerun(c: &mut Connection, project: &Path) {
    let job = command(c, "run_job", json!({ "project": project, "command": "printf 'caf\\303\\251 one\\n'; sleep 0.5; printf 'two\\n'", "name": "e2e" }))["job"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut jobs = follow(c, "jobs");
    let mut output = follow(c, &format!("job/{job}"));
    until(c, &mut jobs, Duration::from_secs(15), "the job to exit", |f| f.mirror.items.iter().any(|j| j["id"] == job.as_str() && j["status"] == "exited"));
    let ended = jobs.mirror.items.iter().find(|j| j["id"] == job.as_str()).unwrap().clone();
    assert_eq!(ended["exit_code"], 0);
    assert_eq!(ended["name"], "e2e");
    until(c, &mut output, Duration::from_secs(5), "its output", |f| printed(f).contains("two"));
    let text = printed(&output);
    assert!(text.contains("café one"), "UTF-8 whole across the stream: {text:?}");
    // Run again: the output starts over.
    let (jobs_at, output_at) = (jobs.seen.len(), output.seen.len());
    command(c, "restart_job", json!({ "job": job }));
    let ran_again = |f: &Followed| f.seen[jobs_at..].iter().any(|m| matches!(&m.body, Body::Changes { ops } if ops.iter().any(|op| matches!(op, Op::Upsert { item } if item["id"] == job.as_str() && item["status"] == "running"))));
    until(c, &mut jobs, Duration::from_secs(15), "the rerun to exit", |f| ran_again(f) && f.mirror.items.iter().any(|j| j["id"] == job.as_str() && j["status"] == "exited"));
    until(c, &mut output, Duration::from_secs(5), "the rerun's output", |f| f.seen.len() > output_at && printed(f).contains("two"));
    assert_eq!(printed(&output).matches("café one").count(), 1, "the rerun's output alone: {:?}", printed(&output));
    c.request("unsubscribe", json!({ "sub": jobs.sub })).unwrap();
    c.request("unsubscribe", json!({ "sub": output.sub })).unwrap();
}

/// What a job's terminal shows: the last screen sent, then the bytes after it (what the
/// screen already holds dropped, as every follower does).
fn printed(f: &Followed) -> String {
    let (mut bytes, mut at) = (Vec::new(), 0);
    let screen = |item: &Value, bytes: &mut Vec<u8>, at: &mut u64| {
        if let Some(data) = item["screen"].as_str() {
            *bytes = wire::bytes_of(data);
            *at = item["offset"].as_u64().unwrap_or(0);
        }
    };
    for message in &f.seen {
        match &message.body {
            Body::Page { items } => items.iter().for_each(|i| screen(i, &mut bytes, &mut at)),
            Body::Changes { ops } => {
                for op in ops {
                    match op {
                        Op::Upsert { item } => screen(item, &mut bytes, &mut at),
                        Op::Bytes { offset, data, .. } => {
                            let new = wire::unseen(at, *offset, data).expect("no bytes missed");
                            at += new.len() as u64;
                            bytes.extend(new);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn vault_follows_files(c: &mut Connection, vault: &Path) {
    std::fs::write(vault.join("first.md"), "One.\n").unwrap();
    command(c, "set_settings", json!({ "changes": { "notes_enabled": true, "notes_vault": vault } }));
    let mut top = follow(c, "vault/");
    until(c, &mut top, Duration::from_secs(10), "the vault's notes", |f| f.mirror.items.iter().any(|i| i["id"] == "first.md"));
    std::fs::write(vault.join("second.md"), "Two.\n").unwrap();
    until(c, &mut top, Duration::from_secs(10), "a new note to show", |f| f.mirror.items.iter().any(|i| i["id"] == "second.md"));
    std::fs::remove_file(vault.join("first.md")).unwrap();
    until(c, &mut top, Duration::from_secs(10), "a deleted note to go", |f| !f.mirror.items.iter().any(|i| i["id"] == "first.md"));
    let found = command(c, "search_notes", json!({ "query": "Two" }));
    assert!(found.to_string().contains("second.md"), "search finds it: {found}");
    c.request("unsubscribe", json!({ "sub": top.sub })).unwrap();
}

fn agent_turn_with_question(c: &mut Connection, agent: &str) {
    let mut chat = follow(c, &format!("conversation/{agent}"));
    until(c, &mut chat, Duration::from_secs(5), "the conversation", |_| true);
    let prompt = "Use the AskUserQuestion tool once to ask me which colour I prefer, with the options Red and Blue. Then use the loom_run tool to run `echo from-agent` as a job named e2e-agent. Then reply with only the colour I chose.";
    command(c, "send", json!({ "agent": agent, "text": prompt }));
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut answered = Vec::<String>::new();
    loop {
        assert!(Instant::now() < deadline, "the turn never ended; entries: {:#}", Value::Array(chat.mirror.items.clone()));
        // A question (a form here; a permission question if Claude Code asks one): answered.
        let question = chat.mirror.items.iter().find(|e| e["status"] == "awaiting_user" && !answered.iter().any(|a| e["id"] == a.as_str())).cloned();
        if let Some(q) = question {
            let params = if q["form"].is_object() {
                json!({ "agent": agent, "entry": q["id"], "content": { "question_0": "Blue" } })
            } else {
                let option = q["options"].as_array().and_then(|o| o.iter().find(|o| o["kind"] == "allow_once")).expect("an allow-once option")["id"].clone();
                json!({ "agent": agent, "entry": q["id"], "option": option })
            };
            command(c, "answer", params.clone());
            // Answered again: the first answer won.
            let again = c.command(&new_id(), "answer", params);
            assert!(again.is_err(), "a second answer is refused: {again:?}");
            answered.push(q["id"].as_str().unwrap().to_owned());
        }
        let ended = chat.mirror.items.last().is_some_and(|e| e["kind"] == "turn");
        if ended && !answered.is_empty() {
            break;
        }
        let seen = chat.seen.len();
        until(c, &mut chat, Duration::from_secs(180), "the agent to go on", |f| f.seen.len() > seen);
    }
    eprintln!("e2e: {} question(s) answered through the API", answered.len());
    let reply = chat.mirror.items.iter().rev().find(|e| e["kind"] == "agent").expect("a reply");
    assert!(reply["text"].as_str().unwrap().contains("Blue"), "the answer reached the agent: {reply}");
    let asked = chat.mirror.items.iter().find(|e| e["id"] == answered[0].as_str()).unwrap();
    assert_eq!(asked["status"], "done");
    // Its loom tool reached loom through the API, as this agent.
    let jobs = c.get("jobs").unwrap();
    assert!(jobs.iter().any(|j| j["name"] == "e2e-agent"), "the agent's job is in the tray: {jobs:?}");
    c.request("unsubscribe", json!({ "sub": chat.sub })).unwrap();
}

/// Dev builds: the window's copies of what it follows are what the core has (a moment
/// given for what is still on its way).
fn window_agrees_with_the_core(c: &mut Connection, agent: &str) {
    if !c.hello["capabilities"]["commands"].as_array().unwrap().iter().any(|m| m == "_selfcheck") {
        eprintln!("e2e: no _selfcheck (a release build): skipped");
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let report = command(c, "_selfcheck", json!({}));
        let checked: Vec<&str> = report["checked"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        let chat = format!("conversation/{agent}");
        for resource in ["agents", "projects", "jobs", chat.as_str()] {
            assert!(checked.contains(&resource), "the window follows {resource}: {checked:?}");
        }
        if report["wrong"].as_array().unwrap().is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "the window's copies differ from the core's: {:#}", report["wrong"]);
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Every command the socket took is in the audit log, with the process that sent it.
fn audit_log_has_every_command(root: &Path) {
    let log = std::fs::read_to_string(root.join("state/loom/audit-LoomE2e.jsonl")).expect("an audit log");
    let lines: Vec<Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    for method in ["set_settings", "new_agent", "run_job", "restart_job", "close_agent"] {
        assert!(lines.iter().any(|l| l["method"] == method), "{method} logged");
    }
    let refused = lines.iter().find(|l| l["ok"] == false).expect("refused commands logged too");
    assert!(refused["error"].as_str().is_some());
    assert_eq!(lines[0]["peer"]["principal"]["kind"], "cli");
    assert!(lines[0]["peer"]["command"].as_str().unwrap().contains("e2e"), "{}", lines[0]["peer"]);
    if std::env::var_os("LOOM_E2E_AGENT").is_some() {
        let tool = lines.iter().find(|l| l["method"] == "tool").expect("the agent's tool call logged");
        assert_eq!(tool["peer"]["principal"]["kind"], "agent", "{tool}");
        assert!(tool["peer"]["command"].as_str().unwrap().contains("mcp"), "from its MCP server: {tool}");
    }
}

fn closing_an_agent_ends_its_conversation(c: &mut Connection, agent: &str) {
    let mut chat = follow(c, &format!("conversation/{agent}"));
    until(c, &mut chat, Duration::from_secs(5), "the conversation", |_| true);
    command(c, "close_agent", json!({ "agent": agent }));
    until(c, &mut chat, Duration::from_secs(10), "gone", |f| f.mirror.gone);
    assert!(!c.get("agents").unwrap().iter().any(|a| a["id"] == agent));
}

// ---- the Telegram bot, against a stand-in for Telegram -----------------------------------

/// Telegram's Bot API as far as the bot uses it, on 127.0.0.1: updates the test queues, and
/// every call the bot makes, kept.
#[derive(Default)]
struct FakeTelegram {
    updates: std::collections::VecDeque<Value>,
    calls: Vec<(String, Value)>,
    next: i64,
}

type Shared = std::sync::Arc<(std::sync::Mutex<FakeTelegram>, std::sync::Condvar)>;

fn fake_telegram() -> (String, Shared) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let shared: Shared = Shared::default();
    let state = shared.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let state = state.clone();
            std::thread::spawn(move || serve_telegram(stream, &state));
        }
    });
    (url, shared)
}

fn serve_telegram(stream: std::net::TcpStream, state: &Shared) {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    loop {
        let mut request = String::new();
        if reader.read_line(&mut request).unwrap_or(0) == 0 {
            return;
        }
        let mut length = 0;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            if header.trim().is_empty() {
                break;
            }
            if let Some(n) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                length = n.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let method = request.split_whitespace().nth(1).unwrap_or("").rsplit('/').next().unwrap_or("").to_owned();
        let (lock, wake) = &**state;
        let result = match method.as_str() {
            "getMe" => json!({ "id": 7, "is_bot": true, "first_name": "loom", "username": "loom_test_bot" }),
            // Asking for what is left over (the bot does at its start, to skip it): nothing.
            "getUpdates" if body["timeout"] == 0 => json!([]),
            "getUpdates" => {
                let mut fake = lock.lock().unwrap();
                fake.calls.push((method.clone(), body.clone()));
                wake.notify_all();
                let (mut fake, _) = wake.wait_timeout_while(fake, Duration::from_millis(500), |f| f.updates.is_empty()).unwrap();
                Value::Array(fake.updates.drain(..).collect())
            }
            _ => {
                let mut fake = lock.lock().unwrap();
                fake.next += 1;
                fake.calls.push((method.clone(), body.clone()));
                wake.notify_all();
                json!({ "message_id": 1000 + fake.next, "chat": { "id": 42 }, "date": 0, "text": body["text"] })
            }
        };
        let answer = json!({ "ok": true, "result": result }).to_string();
        let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n", answer.len());
        if writer.write_all(head.as_bytes()).and_then(|()| writer.write_all(answer.as_bytes())).is_err() {
            return;
        }
    }
}

/// The user (42) writes `text` to the bot.
fn user_says(state: &Shared, text: &str) {
    let (lock, wake) = &**state;
    let mut fake = lock.lock().unwrap();
    fake.next += 1;
    let id = fake.next;
    fake.updates.push_back(json!({ "update_id": id, "message": { "message_id": id, "from": { "id": 42, "is_bot": false, "first_name": "Me" }, "chat": { "id": 42, "type": "private" }, "date": 0, "text": text } }));
    wake.notify_all();
}

/// The user taps an inline button whose data is `data`.
fn user_taps(state: &Shared, data: &str) {
    let (lock, wake) = &**state;
    let mut fake = lock.lock().unwrap();
    fake.next += 1;
    let id = fake.next;
    fake.updates.push_back(json!({ "update_id": id, "callback_query": { "id": format!("cb{id}"), "from": { "id": 42, "is_bot": false, "first_name": "Me" }, "message": { "message_id": 1, "chat": { "id": 42, "type": "private" }, "date": 0 }, "data": data } }));
    wake.notify_all();
}

/// Waits for the bot's call `method` whose body satisfies `test`, from the `from`th call on
/// (`what` says what, if it never comes); its body, and where the next search starts.
fn bot_calls(state: &Shared, from: usize, method: &str, within: Duration, what: &str, test: impl Fn(&Value) -> bool) -> (Value, usize) {
    let (lock, wake) = &**state;
    let deadline = Instant::now() + within;
    let mut fake = lock.lock().unwrap();
    loop {
        if let Some(at) = fake.calls.iter().skip(from).position(|(m, b)| m == method && test(b)) {
            return (fake.calls[from + at].1.clone(), from + at + 1);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "the bot never did {what}; it called: {:#?}", fake.calls.iter().skip(from).map(|(m, b)| format!("{m} {b}")).collect::<Vec<_>>());
        fake = wake.wait_timeout(fake, left).unwrap().0;
    }
}

#[test]
#[ignore = "opens a window: cargo test --test e2e -- --ignored"]
fn the_telegram_bot_through_the_api() {
    let (url, telegram) = fake_telegram();
    let settings = |root: &Path| {
        let file = json!({ "telegramEnabled": true, "telegramToken": "123:test", "telegramUser": 42 });
        std::fs::write(root.join("config/loom/settings-LoomE2eTg.json"), file.to_string()).unwrap();
    };
    // LOOM_CONTROL marks a test instance: only then does the bot talk to a stand-in server.
    let env = [("LOOM_TELEGRAM_API", url), ("LOOM_CONTROL", "/nonexistent/loom-e2e-controls".to_owned())];
    let (instance, mut c) = start_as("dev.shirvani.LoomE2eTg", settings, &env);
    let (_, at) = bot_calls(&telegram, 0, "getUpdates", Duration::from_secs(30), "poll Telegram", |b| b["timeout"] != 0);
    eprintln!("e2e telegram: the bot runs");
    let project = instance.root.join("project");
    let agent = command(&mut c, "new_agent", json!({ "folder": project }))["agent"].as_str().unwrap().to_owned();
    // Written to with no agent picked: the agents, as a keyboard (made from the API's `agents`).
    user_says(&telegram, "hello");
    let (menu, at) = bot_calls(&telegram, at, "sendMessage", Duration::from_secs(10), "offer the agents", |b| b["reply_markup"]["keyboard"].is_array());
    let label = menu["reply_markup"]["keyboard"][0][0]["text"].as_str().unwrap().to_owned();
    assert!(label.contains("project"), "the agent's project in {label:?}");
    user_says(&telegram, &label);
    let (_, at) = bot_calls(&telegram, at, "sendMessage", Duration::from_secs(10), "say who it writes to", |b| b["text"].as_str().is_some_and(|t| t.contains("Your messages go to it")));
    eprintln!("e2e telegram: an agent picked");
    // A shell command never goes through a bridge.
    user_says(&telegram, "!ls");
    let (_, mut at) = bot_calls(&telegram, at, "sendMessage", Duration::from_secs(10), "refuse a ! command", |b| b["text"].as_str().is_some_and(|t| t.contains("! commands")));
    if std::env::var_os("LOOM_E2E_AGENT").is_some() {
        user_says(&telegram, "Reply with exactly: telegram ok");
        let (done, next) = bot_calls(&telegram, at, "sendMessage", Duration::from_secs(120), "tell the turn's end", |b| b["text"].as_str().is_some_and(|t| t.contains("telegram ok")));
        eprintln!("e2e telegram: the reply came back: {}", done["text"]);
        at = next;
        let chat = c.get(&format!("conversation/{agent}")).unwrap();
        assert!(chat.iter().any(|e| e["kind"] == "user" && e["text"] == "Reply with exactly: telegram ok"), "sent through the API");
        // A question, answered with the bot's buttons: Blue picked, then Send.
        user_says(&telegram, "Use the AskUserQuestion tool once to ask me which colour I prefer, with the options Red and Blue. Then reply with only the colour I chose.");
        let blue = |b: &Value| b["reply_markup"]["inline_keyboard"].as_array().into_iter().flatten().flat_map(|row| row.as_array().into_iter().flatten()).find(|k| k["text"].as_str().is_some_and(|t| t.contains("Blue"))).cloned();
        let (asked, next) = bot_calls(&telegram, at, "sendMessage", Duration::from_secs(120), "ask with buttons", |b| blue(b).is_some());
        let button = |text: &str| asked["reply_markup"]["inline_keyboard"].as_array().into_iter().flatten().flat_map(|row| row.as_array().into_iter().flatten()).find(|k| k["text"].as_str().is_some_and(|t| t.contains(text))).and_then(|k| k["callback_data"].as_str().map(str::to_owned));
        let pick = blue(&asked).unwrap()["callback_data"].as_str().unwrap().to_owned();
        user_taps(&telegram, &pick);
        let send = button("Send").expect("a Send button");
        user_taps(&telegram, &send);
        let (done, next) = bot_calls(&telegram, next, "sendMessage", Duration::from_secs(120), "tell the answered turn's end", |b| b["text"].as_str().is_some_and(|t| t.contains("done in") && t.contains("Blue")));
        eprintln!("e2e telegram: answered with buttons: {}", done["text"]);
        at = next;
        let log = std::fs::read_to_string(instance.root.join("state/loom/audit-LoomE2eTg.jsonl")).unwrap();
        assert!(log.lines().any(|l| l.contains("\"method\":\"answer\"") && l.contains("\"ok\":true") && l.contains("telegram")), "the answer went through the API as the bot");
        assert!(log.lines().any(|l| l.contains("\"method\":\"send\"") && l.contains("\"kind\":\"bridge\"") && l.contains("telegram")), "logged as the bot's");
    }
    // Closed in loom: the bot knows no agents any more.
    command(&mut c, "close_agent", json!({ "agent": agent }));
    std::thread::sleep(Duration::from_millis(500));
    user_says(&telegram, "hello again");
    bot_calls(&telegram, at, "sendMessage", Duration::from_secs(10), "say no agents are open", |b| b["text"].as_str().is_some_and(|t| t.contains("No agents open")));
    eprintln!("e2e telegram: closing told");
}
