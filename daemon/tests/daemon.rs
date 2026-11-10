//! loomd, run for real with folders of its own, driven through its API only (no window: these
//! run with the other tests). Each test its own loomd (its own app id and folders). With
//! LOOM_E2E_AGENT=1 also what needs Claude Code: turns, a file filled in through an ask, a
//! conversation read back while it rests, a Notes chat, a schedule firing, Scribe reading.
//!
//! The bridges and agents' tools run as the `loom` beside loomd: `cargo build` first.

#[path = "../../tests/support/telegram.rs"]
mod telegram;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use loom_core::api::cli::{Connection, new_id};
use serde_json::{Value, json};

struct Loomd {
    child: Child,
    root: PathBuf,
    app_id: String,
    /// What loomd said on stderr, line by line (the --web link among it).
    said: std::sync::mpsc::Receiver<String>,
}

impl Drop for Loomd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A loomd of app id `dev.shirvani.LoomD<name>`, its settings file `settings` (camelCase, as
/// loom writes it), with `env` and `args`.
fn start(name: &str, settings: Value, env: &[(&str, String)], args: &[&str]) -> (Loomd, Connection) {
    // Each test its own loomd (app id, socket, folder): tests run side by side.
    static TAKEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut taken = TAKEN.lock().unwrap_or_else(|e| e.into_inner());
    assert!(!taken.iter().any(|t| t == name), "two tests start a loomd named {name}");
    taken.push(name.to_owned());
    drop(taken);
    let app_id = format!("dev.shirvani.LoomD{name}");
    let root = std::env::temp_dir().join(format!("loomd-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["config/loom", "data", "state", "project", "vault"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("vault/first.md"), "A note.\n").unwrap();
    let mut settings = settings;
    settings["notesEnabled"] = json!(true);
    settings["notesVault"] = json!(root.join("vault"));
    std::fs::write(root.join(format!("config/loom/settings-LoomD{name}.json")), settings.to_string()).unwrap();
    let mut child = spawn(&app_id, &root, env, args);
    let (tell, said) = std::sync::mpsc::channel();
    let stderr = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            eprintln!("{line}");
            let _ = tell.send(line);
        }
    });
    let loomd = Loomd { child, root, app_id, said };
    let socket = loom_core::api::socket::socket_path(&loomd.app_id);
    let deadline = Instant::now() + Duration::from_secs(20);
    let c = loop {
        if let Ok(c) = Connection::open(&socket) {
            break c;
        }
        assert!(Instant::now() < deadline, "loomd's API never came up");
        std::thread::sleep(Duration::from_millis(100));
    };
    (loomd, c)
}

fn spawn(app_id: &str, root: &Path, env: &[(&str, String)], args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_loomd"))
        .args(args)
        .env("LOOM_APP_ID", app_id)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("LOOM_TEST_INSTANCE", "1")
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .stderr(Stdio::piped())
        .spawn()
        .expect("loomd starts")
}

fn command(c: &mut Connection, method: &str, params: Value) -> Value {
    c.command(&new_id(), method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// Reads `resource` until `done` holds for its items.
fn until(c: &mut Connection, resource: &str, within: Duration, what: &str, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    let deadline = Instant::now() + within;
    loop {
        let items = c.get(resource).unwrap();
        if done(&items) {
            return items;
        }
        assert!(Instant::now() < deadline, "never {what}: {items:#?}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// A conversation's last reply contains `text`, its turn over.
fn replied(entries: &[Value], text: &str) -> bool {
    entries.iter().any(|e| e["kind"] == "agent" && e["text"].as_str().is_some_and(|t| t.contains(text))) && entries.last().is_some_and(|e| e["kind"] == "turn")
}

/// The agent's item in `agents`.
fn agent_item(c: &mut Connection, id: &str) -> Value {
    c.get("agents").unwrap().into_iter().find(|a| a["id"] == id).expect("the agent")
}

#[test]
fn done_is_said_once_the_work_is_over_not_while_background_agents_run() {
    let fake = format!("python3 {}/tests/fake_agent.py", env!("CARGO_MANIFEST_DIR"));
    let (d, mut c) = start("Done", json!({}), &[("LOOM_AGENT", fake)], &[]);
    let agent = command(&mut c, "new_agent", json!({ "folder": d.root.join("project") }))["agent"].as_str().unwrap().to_owned();
    // A turn that leaves a background agent going: its reply ends the turn, the work goes on.
    command(&mut c, "send", json!({ "agent": agent, "text": "background" }));
    until(&mut c, &format!("conversation/{agent}"), Duration::from_secs(20), "the first turn's end", |e| replied(e, "started"));
    let a = agent_item(&mut c, &agent);
    assert_eq!((a["busy"].as_bool(), a["unseen"].as_bool()), (Some(true), Some(false)), "still working, not done: {a}");
    assert_eq!(a["last_turn"]["turns"], 1, "the turn did end (its reply goes to the phone)");
    // The background agent ends, the agent goes on by itself, then the work is done: once.
    until(&mut c, "agents", Duration::from_secs(20), "done", |all| all.iter().any(|a| a["id"] == agent.as_str() && a["unseen"] == true && a["busy"] == false));
    let a = agent_item(&mut c, &agent);
    assert_eq!(a["last_turn"]["turns"], 2, "two turns ended: {a}");
    command(&mut c, "mark_seen", json!({ "agent": agent }));
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(agent_item(&mut c, &agent)["unseen"], false, "done only once");
    // The window says the user looks at it: `looking`, and no desktop notice of it done.
    command(&mut c, "set_looking", json!({ "active": true, "agents": [agent] }));
    until(&mut c, "agents", Duration::from_secs(5), "looked at", |all| all.iter().any(|a| a["id"] == agent.as_str() && a["looking"] == true));
    while d.said.try_recv().is_ok() {}
    // A turn that leaves a dev server running is done at once: it never ends by itself.
    command(&mut c, "send", json!({ "agent": agent, "text": "server" }));
    until(&mut c, "agents", Duration::from_secs(20), "done beside a dev server", |all| all.iter().any(|a| a["id"] == agent.as_str() && a["last_turn"]["turns"] == 3 && a["busy"] == false));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(agent_item(&mut c, &agent)["unseen"], false, "done while looked at: seen");
    let said: Vec<String> = d.said.try_iter().collect();
    assert!(!said.iter().any(|l| l.ends_with(": Finished")), "no desktop notice of what the user looks at: {said:?}");
    command(&mut c, "set_looking", json!({ "active": false, "agents": [] }));
    until(&mut c, "agents", Duration::from_secs(5), "no longer looked at", |all| all.iter().any(|a| a["id"] == agent.as_str() && a["looking"] == false));
    // Its plan, as a client sees it.
    command(&mut c, "send", json!({ "agent": agent, "text": "plan" }));
    let plan = until(&mut c, &format!("plan/{agent}"), Duration::from_secs(20), "the plan", |p| p.len() == 2);
    assert_eq!((plan[1]["content"].as_str(), plan[1]["status"].as_str()), (Some("Fix it"), Some("in_progress")));
}

#[test]
fn a_file_a_shell_command_writes_counts_as_edited() {
    let fake = format!("python3 {}/tests/fake_agent.py", env!("CARGO_MANIFEST_DIR"));
    let (d, mut c) = start("Shell", json!({}), &[("LOOM_AGENT", fake)], &[]);
    let project = d.root.join("project");
    assert!(std::process::Command::new("git").arg("init").arg("-q").arg(&project).status().unwrap().success());
    let agent = command(&mut c, "new_agent", json!({ "folder": project }))["agent"].as_str().unwrap().to_owned();
    command(&mut c, "send", json!({ "agent": agent, "text": "write" }));
    until(&mut c, &format!("conversation/{agent}"), Duration::from_secs(20), "the reply", |e| replied(e, "wrote"));
    until(&mut c, "agents", Duration::from_secs(20), "made.txt counted", |all| all.iter().any(|a| a["id"] == agent.as_str() && a["edited"] == 1));
}

#[test]
fn a_projects_board_is_a_file_in_it() {
    let fake = format!("python3 {}/tests/fake_agent.py", env!("CARGO_MANIFEST_DIR"));
    let (d, mut c) = start("Board", json!({}), &[("LOOM_AGENT", fake)], &[]);
    let project = d.root.join("project");
    // a project of the sidebar's: one with an agent in it
    command(&mut c, "new_agent", json!({ "folder": project }));
    let card = command(&mut c, "board_add", json!({ "project": project, "title": "Fix the tooltip", "body": "It opens under the panel." }))["card"].clone();
    command(&mut c, "board_move", json!({ "project": project, "card": card, "lane": "Done" }));
    let board = command(&mut c, "board", json!({ "project": project }));
    let done = &board["lanes"][3];
    assert_eq!((done["title"].as_str(), done["cards"][0]["title"].as_str(), done["cards"][0]["done"].as_bool()), (Some("Done"), Some("Fix the tooltip"), Some(true)), "{board}");
    assert!(!done["cards"][0]["finished"].as_str().unwrap_or_default().is_empty());
    let file = std::fs::read_to_string(project.join(".loom/board.md")).unwrap();
    assert!(file.contains("- [x] Fix the tooltip\n\tIt opens under the panel.\n"), "{file}");
    // only the sidebar's projects
    let elsewhere = c.command(&new_id(), "board", json!({ "project": d.root.join("vault") }));
    assert!(elsewhere.is_err(), "{elsewhere:?}");
}

fn agent_tests() -> bool {
    std::env::var_os("LOOM_E2E_AGENT").is_some()
}

#[test]
fn loomd_hosts_the_core_with_no_window() {
    let (mut d, mut c) = start("Core", json!({}), &[], &["--web"]);
    assert_eq!(c.hello["server"]["host"], "daemon");
    // The spec lists what loomd has, and loomd what the spec lists.
    let commands: Vec<&str> = c.hello["capabilities"]["commands"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for name in commands.iter().filter(|n| !n.starts_with('_')) {
        assert!(loom_core::api::spec::COMMANDS.iter().any(|c| c.name == *name), "{name} is not in spec.rs");
    }
    for listed in loom_core::api::spec::COMMANDS {
        assert!(commands.contains(&listed.name), "spec.rs lists {}, which loomd doesn't have", listed.name);
    }
    // --web's link: a page as the window.
    let link = loop {
        let line = d.said.recv_timeout(Duration::from_secs(10)).expect("loomd --web says a link");
        if let Some(url) = line.split_whitespace().find(|w| w.starts_with("http://127.0.0.1:")) {
            break url.to_owned();
        }
    };
    assert_eq!(page_principal(&link), "window", "--web's page is as the window");
    // One core per app id.
    let second = spawn(&d.app_id, &d.root, &[], &[]).wait_with_output().unwrap();
    assert_eq!(second.status.code(), Some(3), "a second loomd starts no second core (and says so with 3)");

    let project = d.root.join("project");
    let agent = command(&mut c, "new_agent", json!({ "folder": project }))["agent"].as_str().unwrap().to_owned();
    assert!(c.get("agents").unwrap().iter().any(|a| a["id"] == agent.as_str()));
    let job = command(&mut c, "run_job", json!({ "project": project, "command": "echo from-loomd", "name": "d" }))["job"].as_str().unwrap().to_owned();
    until(&mut c, "jobs", Duration::from_secs(10), "the job ending", |jobs| jobs.iter().any(|j| j["id"] == job.as_str() && j["status"] == "exited" && j["exit_code"] == 0));
    let output = c.get(&format!("job/{job}")).unwrap();
    assert!(command(&mut c, "job_text", json!({ "job": job }))["text"].as_str().unwrap().contains("from-loomd"), "a job's output as text");
    let dirs = command(&mut c, "dirs_list", json!({ "folder": d.root }));
    assert!(dirs["dirs"].as_array().unwrap().iter().any(|n| n == "project"), "{dirs}");
    assert!(String::from_utf8_lossy(&loom_core::api::wire::bytes_of(output[0]["screen"].as_str().unwrap())).contains("from-loomd"));
    assert!(command(&mut c, "web_open", json!({}))["url"].as_str().unwrap().starts_with("http://127.0.0.1:"));

    // What the window would show, there for a UI.
    let chat = command(&mut c, "notes_open", json!({}))["agent"].as_str().unwrap().to_owned();
    assert!(chat.parse::<u32>().unwrap() > 1_000_000);
    assert!(c.get(&format!("conversation/{chat}")).is_ok());
    // Settings' lists: topics and the Claude folder; only their files open.
    let made = command(&mut c, "topic_new", json!({ "name": "Garden" }));
    let garden = made["path"].as_str().unwrap().to_owned();
    let about = command(&mut c, "notes_open", json!({ "topic": "Garden" }))["agent"].as_str().unwrap().to_owned();
    assert!(c.get(&format!("conversation/{about}")).is_ok(), "a chat about a topic");
    command(&mut c, "topic_set", json!({ "name": "Garden", "enabled": false }));
    let topics = command(&mut c, "topics", json!({}));
    assert!(topics["topics"].as_array().unwrap().iter().any(|t| t["name"] == "Garden" && t["enabled"] == false && t["path"] == garden.as_str()), "{topics}");
    let items = command(&mut c, "claude_items", json!({}));
    assert!(!items["items"].as_array().unwrap().is_empty(), "loom ships some: {items}");
    assert!(c.command(&new_id(), "edit_file", json!({ "path": "/etc/hostname" })).is_err_and(|e| e.message.contains("only")));
    for resource in ["notes_chats", "scribe", "asks", "schedules", "closed"] {
        assert!(c.get(resource).is_ok(), "{resource}");
    }
    let bridges = c.get("bridges").unwrap();
    assert!(bridges.iter().any(|b| b["id"] == "telegram" && b["state"] == "off"), "{bridges:?}");
    assert!(c.command(&new_id(), "pairing", json!({ "open": true })).is_err_and(|e| e.message.contains("window")), "pairing is the window's");
    command(&mut c, "rename", json!({ "agent": agent, "title": "Renamed" }));
    until(&mut c, "agents", Duration::from_secs(5), "the new title", |a| a.iter().any(|a| a["id"] == agent.as_str() && a["title"] == "Renamed"));
    assert!(c.command(&new_id(), "set_mode", json!({ "agent": agent, "mode": "bypassPermissions" })).is_err_and(|e| e.message.contains("window")));
    command(&mut c, "set_awake", json!({ "agent": agent, "awake": true }));
    until(&mut c, "agents", Duration::from_secs(5), "kept awake", |a| a.iter().any(|a| a["id"] == agent.as_str() && a["awake"] == true));
    let folder = project.to_string_lossy().into_owned();
    command(&mut c, "rename_project", json!({ "project": folder, "name": "Mine" }));
    command(&mut c, "set_collapsed", json!({ "project": folder, "collapsed": true }));
    until(&mut c, "projects", Duration::from_secs(5), "renamed and folded", |p| p.iter().any(|p| p["id"] == folder.as_str() && p["name"] == "Mine" && p["collapsed"] == true));
    command(&mut c, "close_agent", json!({ "agent": agent }));
    command(&mut c, "remove_project", json!({ "project": folder }));
    until(&mut c, "projects", Duration::from_secs(5), "the project gone", |p| !p.iter().any(|p| p["id"] == folder.as_str()));
    // Jobs left running, in a project and in the Notes vault: stopped with loomd.
    let mut pids = Vec::new();
    for (folder, name) in [(project.clone(), "p"), (d.root.join("vault"), "v")] {
        let file = d.root.join(format!("{name}.pid"));
        let line = format!("echo $$ > {}; exec sleep 300", file.display());
        command(&mut c, "run_job", json!({ "project": folder, "command": line, "name": name }));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !file.exists() || std::fs::read_to_string(&file).unwrap().trim().is_empty() {
            assert!(Instant::now() < deadline, "the job never started");
            std::thread::sleep(Duration::from_millis(50));
        }
        pids.push(std::fs::read_to_string(&file).unwrap().trim().parse::<u32>().unwrap());
    }

    // Stopped: the workspace saved, the socket gone.
    // SAFETY: a plain signal to the child started above.
    unsafe { kill(d.child.id() as i32, 15) };
    assert_eq!(d.child.wait().unwrap().code(), Some(0), "loomd ends cleanly on SIGTERM");
    let saved = std::fs::read_to_string(d.root.join("state/loom/state-LoomDCore.json")).expect("the workspace saved");
    assert!(saved.contains("\"projects\""), "{saved}");
    assert!(!loom_core::api::socket::socket_path(&d.app_id).exists(), "its socket removed");
    let deadline = Instant::now() + Duration::from_secs(5);
    while pids.iter().any(|pid| Path::new(&format!("/proc/{pid}")).exists()) {
        assert!(Instant::now() < deadline, "a job outlived loomd: {pids:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The Telegram bot run by loomd, against a stand-in for Telegram (nothing goes out).
#[test]
fn the_telegram_bot_under_loomd() {
    let (url, tg) = telegram::fake_telegram();
    let settings = json!({ "telegramEnabled": true, "telegramToken": "123:test", "telegramUser": 42 });
    let env = [("LOOM_TELEGRAM_API", url), ("LOOM_CONTROL", "/nonexistent/loomd-test".to_owned())];
    let (d, mut c) = start("Telegram", settings, &env, &[]);
    let (_, at) = telegram::bot_calls(&tg, 0, "getUpdates", Duration::from_secs(30), "poll Telegram", |b| b["timeout"] != 0);
    until(&mut c, "bridges", Duration::from_secs(10), "the bot's state in `bridges`", |b| b.iter().any(|b| b["id"] == "telegram" && b["state"] != "off"));
    let agent = command(&mut c, "new_agent", json!({ "folder": d.root.join("project") }))["agent"].as_str().unwrap().to_owned();
    telegram::user_says(&tg, "hello");
    let (menu, at) = telegram::bot_calls(&tg, at, "sendMessage", Duration::from_secs(10), "offer the agents", |b| b["reply_markup"]["keyboard"].is_array());
    let label = menu["reply_markup"]["keyboard"][0][0]["text"].as_str().unwrap().to_owned();
    telegram::user_says(&tg, &label);
    let (_, at) = telegram::bot_calls(&tg, at, "sendMessage", Duration::from_secs(10), "say who it writes to", |b| b["text"].as_str().is_some_and(|t| t.contains("Your messages go to it")));
    telegram::user_says(&tg, "!ls");
    let (_, mut at) = telegram::bot_calls(&tg, at, "sendMessage", Duration::from_secs(10), "refuse a ! command", |b| b["text"].as_str().is_some_and(|t| t.contains("! commands")));
    if agent_tests() {
        telegram::user_says(&tg, "Reply with exactly: telegram under loomd");
        let (_, next) = telegram::bot_calls(&tg, at, "sendMessage", Duration::from_secs(120), "tell the turn's end", |b| b["text"].as_str().is_some_and(|t| t.contains("telegram under loomd")));
        at = next;
        eprintln!("loomd test: a Telegram turn under loomd");
    }
    command(&mut c, "close_agent", json!({ "agent": agent }));
    std::thread::sleep(Duration::from_millis(500));
    telegram::user_says(&tg, "hello again");
    telegram::bot_calls(&tg, at, "sendMessage", Duration::from_secs(10), "say no agents are open", |b| b["text"].as_str().is_some_and(|t| t.contains("No agents open")));
}

/// The phone's bridge run by loomd: it comes up and says how it is, through the API, as itself
/// (its relay a closed port here: nothing goes out).
#[test]
fn the_remote_bridge_under_loomd() {
    let (d, mut c) = start("Remote", json!({ "remoteEnabled": true, "remoteRelay": "http://127.0.0.1:9" }), &[], &[]);
    let log = d.root.join("state/loom/audit-LoomDRemote.jsonl");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let said = std::fs::read_to_string(&log).unwrap_or_default();
        let ok = said.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).any(|l| l["method"] == "bridge" && l["ok"] == true && l["peer"]["principal"] == json!({ "kind": "bridge", "name": "remote" }));
        if ok {
            break;
        }
        assert!(Instant::now() < deadline, "the remote bridge never spoke through the API: {said}");
        std::thread::sleep(Duration::from_millis(200));
    }
    until(&mut c, "bridges", Duration::from_secs(10), "its state in `bridges`", |b| b.iter().any(|b| b["id"] == "remote" && b["state"] == "online"));
}

/// The phone's side under loomd, with a stand-in phone (`loom-remote` as a paired device),
/// on this computer only (the relay a closed port; iroh finds the bridge on loopback): pairing
/// opened and confirmed through the API from the window's page, then the phone's requests,
/// its Notes tab among them.
#[test]
fn a_phone_pairs_and_works_under_loomd() {
    let (d, mut c) = start("Phone", json!({ "remoteEnabled": true, "remoteRelay": "http://127.0.0.1:9" }), &[], &["--web"]);
    let link = loop {
        let line = d.said.recv_timeout(Duration::from_secs(10)).expect("loomd --web says a link");
        if let Some(url) = line.split_whitespace().find(|w| w.starts_with("http://127.0.0.1:")) {
            break url.to_owned();
        }
    };
    let mut window = Page::open(&link);
    until(&mut c, "bridges", Duration::from_secs(30), "the bridge online", |b| b.iter().any(|b| b["id"] == "remote" && b["state"] == "online"));
    assert!(window.call("pairing", json!({ "open": true }))["error"].is_null());
    // The ticket: the window's to see, not the CLI's.
    assert!(c.get("pairing").is_err(), "the CLI doesn't see the pairing ticket");
    assert!(c.get("bridges").unwrap().iter().all(|b| b.get("ticket").is_none()));
    let ticket = window.until("pairing", Duration::from_secs(20), "a pairing ticket", |p| p.first().is_some_and(|t| t["ticket"].is_string()))[0]["ticket"].as_str().unwrap().to_owned();
    let phone = d.root.join("phone");
    let remote = PathBuf::from(env!("CARGO_BIN_EXE_loomd")).with_file_name("loom-remote");
    let peer = |args: &[&str]| Command::new(&remote).args(args).env("XDG_CONFIG_HOME", &phone).stderr(Stdio::inherit()).output().unwrap();
    let pairing = Command::new(&remote).args(["pair", &ticket, "test phone"]).env("XDG_CONFIG_HOME", &phone).stdout(Stdio::piped()).spawn().unwrap();
    let bridges = until(&mut c, "bridges", Duration::from_secs(30), "the phone asking to pair", |b| b.iter().any(|b| b["id"] == "remote" && b["candidate"].is_object()));
    let candidate = bridges.iter().find(|b| b["id"] == "remote").unwrap()["candidate"].clone();
    assert_eq!(candidate["name"], "test phone");
    // Not for the CLI; from the window's page, yes.
    assert!(c.command(&new_id(), "pair_confirm", json!({})).is_err());
    assert!(window.call("pair_confirm", json!({ "id": candidate["id"] }))["error"].is_null());
    let paired = pairing.wait_with_output().unwrap();
    assert!(paired.status.success() && String::from_utf8_lossy(&paired.stdout).contains("Paired"), "{paired:?}");
    eprintln!("loomd test: a phone paired through the API");

    let agent = command(&mut c, "new_agent", json!({ "folder": d.root.join("project") }))["agent"].as_str().unwrap().to_owned();
    let listed = peer(&["agents"]);
    let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
    assert!(listed.lines().any(|l| l.trim_start().starts_with(&format!("{agent} ")) && l.contains("project")), "the phone sees the agent: {listed}");
    let notes = peer(&["call", "notes", "{}"]);
    let notes: Value = serde_json::from_slice(&notes.stdout).unwrap_or_default();
    assert_eq!(notes["vault"], "vault", "the phone's Notes tab: {notes}");
    let opened = peer(&["call", "notes_open", "{}"]);
    let opened: Value = serde_json::from_slice(&opened.stdout).unwrap_or_default();
    assert!(opened["id"].as_u64().is_some_and(|id| id > 1_000_000), "a Notes chat opened from the phone: {opened}");
    let note = peer(&["call", "note_read", r#"{"path": "first.md"}"#]);
    assert!(String::from_utf8_lossy(&note.stdout).contains("A note."), "a note read from the phone: {note:?}");
    eprintln!("loomd test: the phone's requests and Notes tab");
}

/// A web page's connection, as the browser makes it (the token for a session, then the
/// WebSocket), for requests made as whoever opened the link.
struct Page {
    ws: tungstenite::WebSocket<std::net::TcpStream>,
    next: u64,
    /// What `initialize` answered.
    hello: Value,
}

impl Page {
    fn open(link: &str) -> Page {
        use std::io::{Read, Write};
        use tungstenite::client::IntoClientRequest;
        let rest = link.strip_prefix("http://").unwrap();
        let (host, path) = rest.split_once('/').unwrap();
        let mut stream = std::net::TcpStream::connect(host).unwrap();
        write!(stream, "GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        let session = answer.lines().find_map(|l| l.strip_prefix("Location: /#")).expect("a session").trim().to_owned();
        let mut request = format!("ws://{host}/api?s={session}").into_client_request().unwrap();
        request.headers_mut().insert("Origin", format!("http://{host}").parse().unwrap());
        let (ws, _) = tungstenite::client::client(request, std::net::TcpStream::connect(host).unwrap()).map_err(|e| e.to_string()).unwrap();
        let mut page = Page { ws, next: 0, hello: Value::Null };
        page.hello = page.call("initialize", json!({}))["result"].clone();
        page
    }

    /// Follows `resource` until `done` holds for its items (a snapshot, then its changes).
    fn until(&mut self, resource: &str, within: Duration, what: &str, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let sub = self.call("subscribe", json!({ "resource": resource }))["result"]["sub"].as_str().expect("subscribed").to_owned();
        let mut mirror = loom_core::api::Mirror::default();
        let _ = self.ws.get_ref().set_read_timeout(Some(Duration::from_millis(200)));
        let deadline = Instant::now() + within;
        loop {
            if mirror.synced && done(&mirror.items) {
                let _ = self.ws.get_ref().set_read_timeout(None);
                return mirror.items;
            }
            assert!(Instant::now() < deadline, "never {what}: {:#?}", mirror.items);
            let Ok(message) = self.ws.read() else { continue };
            let Ok(message) = serde_json::from_str::<Value>(message.to_text().unwrap_or("")) else { continue };
            if message["method"] == "subscription" && message["params"]["sub"] == sub.as_str()
                && let Ok(m) = serde_json::from_value::<loom_core::api::Message>(message["params"].clone())
            {
                mirror.apply(&m);
            }
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = format!("page-{}-{}", std::process::id(), self.next);
        self.ws.send(tungstenite::Message::text(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string())).unwrap();
        loop {
            let message: Value = serde_json::from_str(self.ws.read().unwrap().to_text().unwrap()).unwrap();
            if message["id"] == id.as_str() {
                return message;
            }
        }
    }
}

/// What needs Claude Code (LOOM_E2E_AGENT=1).
#[test]
fn agents_under_loomd() {
    if !agent_tests() {
        eprintln!("LOOM_E2E_AGENT not set: skipped");
        return;
    }
    let (mut d, mut c) = start("Agents", json!({}), &[], &[]);
    let project = d.root.join("project");
    let agent = command(&mut c, "new_agent", json!({ "folder": project }))["agent"].as_str().unwrap().to_owned();
    let calls_before = c.get("scribe").unwrap().iter().find(|i| i["id"] == "status").and_then(|s| s["calls_left"].as_u64()).unwrap_or(0);

    // A turn, with something worth a note in it.
    command(&mut c, "send", json!({ "agent": agent, "text": "Note for later: my dentist appointment is on 3 December 2026 at 10:00, Via Roma 5. Reply with exactly: loomd ok" }));
    until(&mut c, &format!("conversation/{agent}"), Duration::from_secs(120), "the reply", |e| replied(e, "loomd ok"));

    // A file filled in through an ask.
    command(&mut c, "send", json!({ "agent": agent, "text": "Use the loom_edit tool to have me fill in the file secret.env with the message 'type anything'. When it returns, reply with exactly: edit ok" }));
    let asks = until(&mut c, "asks", Duration::from_secs(120), "the ask", |a| a.iter().any(|a| a["kind"] == "edit"));
    let ask = asks.iter().find(|a| a["kind"] == "edit").unwrap().clone();
    std::fs::write(project.join("secret.env"), "KEY=value\n").unwrap();
    command(&mut c, "ask_done", json!({ "id": ask["id"] }));
    until(&mut c, &format!("conversation/{agent}"), Duration::from_secs(120), "the reply after the edit", |e| replied(e, "edit ok"));
    eprintln!("loomd test: a file filled in through an ask");

    // Closed, then reopened: its conversation is there before it runs again.
    command(&mut c, "close_agent", json!({ "agent": agent }));
    let closed = c.get("closed").unwrap();
    let session = closed.first().expect("the conversation among the closed ones")["id"].as_str().unwrap().to_owned();
    // Scribe reads a conversation as it closes: a model call counted.
    until(&mut c, "scribe", Duration::from_secs(180), "Scribe reading the closed conversation", |items| {
        items.iter().find(|i| i["id"] == "status").is_some_and(|s| s["calls_left"].as_u64().unwrap_or(0) < calls_before && s["running"] == false)
    });
    eprintln!("loomd test: Scribe read the conversation");
    let reopened = command(&mut c, "reopen", json!({ "session": session }))["agent"].as_str().unwrap().to_owned();
    let entries = until(&mut c, &format!("conversation/{reopened}"), Duration::from_secs(20), "the conversation read back", |e| e.iter().any(|e| e["kind"] == "user" && e["text"].as_str().is_some_and(|t| t.contains("dentist"))));
    assert!(entries.iter().any(|e| e["kind"] == "agent"), "its replies too");
    assert_eq!(c.get("agents").unwrap().iter().find(|a| a["id"] == reopened.as_str()).unwrap()["status"], "suspended", "read back, not started");
    eprintln!("loomd test: a resting conversation read back");
    // Among Claude Code's past ones; resumed: the agent open.
    let past = command(&mut c, "past_conversations", json!({ "folder": project }));
    assert!(past["conversations"].as_array().unwrap().iter().any(|p| p["session"] == session.as_str()), "{past}");
    let resumed = command(&mut c, "resume", json!({ "session": session, "folder": project }))["agent"].as_str().unwrap().to_owned();
    assert_eq!(resumed, reopened, "the one open already");
    assert!(!c.get("closed").unwrap().iter().any(|c| c["id"] == session.as_str()), "open: not among the closed");
    let again = command(&mut c, "reopen", json!({ "session": session }));
    assert!(again.is_null() || again["agent"] == reopened.as_str(), "reopened twice: still one agent ({again})");

    // A schedule fires (checked every 20 s).
    command(&mut c, "schedule_new", json!({ "agent": reopened, "prompt": "Reply with exactly: scheduled ok", "when": "in 1m" }));
    assert_eq!(c.get("schedules").unwrap().len(), 1);
    until(&mut c, &format!("conversation/{reopened}"), Duration::from_secs(150), "the scheduled prompt's reply", |e| replied(e, "scheduled ok"));
    eprintln!("loomd test: a schedule fired");

    // A Notes chat: a turn in the vault.
    let chat = command(&mut c, "notes_open", json!({}))["agent"].as_str().unwrap().to_owned();
    command(&mut c, "send", json!({ "agent": chat, "text": "Reply with exactly: notes ok" }));
    until(&mut c, &format!("conversation/{chat}"), Duration::from_secs(120), "the Notes chat's reply", |e| replied(e, "notes ok"));
    until(&mut c, "notes_chats", Duration::from_secs(30), "the chat among the past ones", |chats| chats.iter().any(|c| c["agent"] == chat.as_str()));
    eprintln!("loomd test: a Notes chat turn");

    // A start that fails (its folder gone) leaves its conversation readable all the same.
    command(&mut c, "close_agent", json!({ "agent": reopened }));
    let again = command(&mut c, "reopen", json!({ "session": session }))["agent"].as_str().unwrap().to_owned();
    let away = d.root.join("project-away");
    std::fs::rename(&project, &away).unwrap();
    // Its start fails (the send itself may say so, or the agent's status does).
    let _ = c.command(&new_id(), "send", json!({ "agent": again, "text": "this start fails" }));
    until(&mut c, "agents", Duration::from_secs(20), "the start failing", |a| a.iter().any(|a| a["id"] == again.as_str() && a["status"] == "failed"));
    std::fs::rename(&away, &project).unwrap();
    // Nothing left stuck: it starts again, replays the whole conversation and answers.
    command(&mut c, "send", json!({ "agent": again, "text": "Reply with exactly: back ok" }));
    let entries = until(&mut c, &format!("conversation/{again}"), Duration::from_secs(120), "the agent back after a failed start", |e| replied(e, "back ok"));
    assert!(entries.iter().any(|e| e["kind"] == "user" && e["text"].as_str().is_some_and(|t| t.contains("dentist"))), "its whole conversation again");
    eprintln!("loomd test: back after a failed start");

    // Stopped mid-turn: a message waiting to be sent is in the draft saved.
    command(&mut c, "send", json!({ "agent": again, "text": "Count slowly from 1 to 40, one number per line." }));
    until(&mut c, "agents", Duration::from_secs(60), "the agent at work", |a| a.iter().any(|a| a["id"] == again.as_str() && a["busy"] == true));
    command(&mut c, "send", json!({ "agent": again, "text": "a message queued behind the turn" }));
    // SAFETY: a plain signal to the child started above.
    unsafe { kill(d.child.id() as i32, 15) };
    let _ = d.child.wait();
    let saved = std::fs::read_to_string(d.root.join("state/loom/state-LoomDAgents.json")).unwrap();
    assert!(saved.contains("a message queued behind the turn"), "the queued message saved as the draft: {saved}");
    eprintln!("loomd test: a queued message kept on stop");
}

/// Who a page opened from `link` is.
fn page_principal(link: &str) -> String {
    Page::open(link).hello["principal"]["kind"].as_str().unwrap_or_default().to_owned()
}

unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

/// The window stops loomd (to start a newer one): it ends cleanly, its socket gone.
#[test]
fn loomd_stops_when_the_window_says_so() {
    let (mut d, mut c) = start("Stop", json!({}), &[], &[]);
    command(&mut c, "loomd_stop", json!({}));
    let deadline = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Some(status) = d.child.try_wait().unwrap() {
            break status.code();
        }
        assert!(Instant::now() < deadline, "loomd never stopped");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(code, Some(0), "it ends as on SIGTERM");
    assert!(!loom_core::api::socket::socket_path(&d.app_id).exists(), "its socket removed");
}

/// The window as loomd's client (`api::remote`): it follows a resource and does commands over
/// the socket, and when loomd goes and comes back, it reconnects by itself and its copy starts
/// over from a fresh snapshot.
#[test]
fn a_remote_connection_comes_back_after_loomd_does() {
    use loom_core::api::{Mirror, remote::Remote};
    use std::cell::RefCell;
    use std::rc::Rc;
    let (mut loomd, _c) = start("Reconnect", json!({ "idleMinutes": 30 }), &[], &[]);
    let socket = loom_core::api::socket::socket_path(&loomd.app_id);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async {
        loom_core::rt::set_host(
            |future| {
                tokio::task::spawn_local(future);
            },
            |after, f| {
                tokio::task::spawn_local(async move {
                    tokio::time::sleep(after).await;
                    f();
                });
            },
        );
        let remote = Remote::connect(socket, "test window");
        let states: Rc<RefCell<Vec<bool>>> = Rc::default();
        let said = states.clone();
        remote.on_state(move |up| said.borrow_mut().push(up));
        let sub = remote.subscribe("settings", || {});
        let mirror = RefCell::new(Mirror::default());
        settle(&sub, &mirror, Duration::from_secs(10), "the snapshot", |m| m.synced && m.items.len() == 1).await;
        // The test isn't loom's window: the CLI's rights.
        assert_eq!(remote.hello()["principal"]["kind"], "cli");
        let answer: Rc<RefCell<Option<bool>>> = Rc::default();
        let got = answer.clone();
        remote.command_later("c1", "set_settings", json!({ "changes": { "idle_minutes": 31 } }), move |r| *got.borrow_mut() = Some(r.is_ok()));
        settle(&sub, &mirror, Duration::from_secs(10), "the change", |m| m.items[0]["idle_minutes"] == 31).await;
        assert_eq!(*answer.borrow(), Some(true));

        // loomd goes and comes back: the connection follows, the copy starts over.
        let _ = loomd.child.kill();
        let _ = loomd.child.wait();
        let deadline = Instant::now() + Duration::from_secs(10);
        while remote.connected() {
            assert!(Instant::now() < deadline, "never noticed loomd went");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let failed: Rc<RefCell<Option<bool>>> = Rc::default();
        let got = failed.clone();
        remote.command_later("c2", "set_settings", json!({ "changes": { "idle_minutes": 32 } }), move |r| *got.borrow_mut() = Some(r.is_err()));
        assert_eq!(*failed.borrow(), Some(true), "no loomd: an error at once");
        loomd.child = spawn(&loomd.app_id, &loomd.root, &[], &[]);
        settle(&sub, &mirror, Duration::from_secs(20), "a fresh snapshot after reconnecting", |m| remote.connected() && m.synced && m.items.len() == 1).await;
        assert_eq!(mirror.borrow().items[0]["idle_minutes"], 31, "what loomd saved");
        assert_eq!(*states.borrow(), vec![true, false, true]);
    });
}

/// Applies what came to `sub` until `done` holds (or fails after `within`).
async fn settle(sub: &loom_core::api::remote::RemoteSub, mirror: &std::cell::RefCell<loom_core::api::Mirror>, within: Duration, what: &str, done: impl Fn(&loom_core::api::Mirror) -> bool) {
    let deadline = Instant::now() + within;
    loop {
        for m in sub.drain() {
            mirror.borrow_mut().apply(&m);
        }
        if done(&mirror.borrow()) {
            return;
        }
        assert!(Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
