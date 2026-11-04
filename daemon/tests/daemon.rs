//! loomd, run for real with folders of its own, driven through its API only (no window, so
//! it runs with the other tests). With LOOM_E2E_AGENT=1 also a Claude Code turn in an agent
//! that has no window.

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use loom_core::api::cli::{Connection, new_id};
use serde_json::{Value, json};

const APP_ID: &str = "dev.shirvani.LoomE2eDaemon";

struct Daemon {
    child: Child,
    root: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn spawn(root: &std::path::Path) -> Child {
    spawn_with(root, &[])
}

fn spawn_with(root: &std::path::Path, args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_loomd"))
        .args(args)
        .stderr(if args.is_empty() { std::process::Stdio::inherit() } else { std::process::Stdio::piped() })
        .env("LOOM_APP_ID", APP_ID)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("LOOM_TEST_INSTANCE", "1")
        .spawn()
        .expect("loomd starts")
}

fn connect() -> Connection {
    let socket = loom_core::api::socket::socket_path(APP_ID);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(c) = Connection::open(&socket) {
            return c;
        }
        assert!(Instant::now() < deadline, "loomd's API never came up");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn command(c: &mut Connection, method: &str, params: Value) -> Value {
    c.command(&new_id(), method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// Polls `resource` until `done` holds for its items.
fn until(c: &mut Connection, resource: &str, within: Duration, what: &str, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    let deadline = Instant::now() + within;
    loop {
        let items = c.get(resource).unwrap();
        if done(&items) {
            return items;
        }
        assert!(Instant::now() < deadline, "never {what}: {items:#?}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn loomd_hosts_the_core_with_no_window() {
    let root = std::env::temp_dir().join(format!("loomd-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["config/loom", "data", "state", "project", "vault"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("vault/first.md"), "A note.\n").unwrap();
    let settings = json!({ "notesEnabled": true, "notesVault": root.join("vault") });
    std::fs::write(root.join("config/loom/settings-LoomE2eDaemon.json"), settings.to_string()).unwrap();
    let mut child = spawn_with(&root, &["--web"]);
    // Its stderr: the window's link, then read on so loomd never waits on it.
    let (link, said) = std::sync::mpsc::channel::<String>();
    let stderr = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            eprintln!("{line}");
            if let Some(url) = line.split_whitespace().find(|w| w.starts_with("http://127.0.0.1:")) {
                let _ = link.send(url.to_owned());
            }
        }
    });
    let mut daemon = Daemon { child, root: root.clone() };
    let mut c = connect();
    // The spec lists what loomd has, and loomd what the spec lists.
    let commands: Vec<&str> = c.hello["capabilities"]["commands"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for name in commands.iter().filter(|n| !n.starts_with('_')) {
        assert!(loom_core::api::spec::COMMANDS.iter().any(|c| c.name == *name), "{name} is not in spec.rs");
    }
    for listed in loom_core::api::spec::COMMANDS {
        assert!(commands.contains(&listed.name), "spec.rs lists {}, which loomd doesn't have", listed.name);
    }
    let link = said.recv_timeout(Duration::from_secs(10)).expect("loomd --web says a link");
    assert_eq!(page_principal(&link), "window", "--web's page is as the window");
    assert_eq!(c.hello["server"]["host"], "daemon");
    // One core per app id: a second one is turned away.
    let second = spawn(&root).wait_with_output().unwrap();
    assert_eq!(second.status.code(), Some(1), "a second loomd starts no second core");

    let project = root.join("project");
    let agent = command(&mut c, "new_agent", json!({ "folder": project }))["agent"].as_str().unwrap().to_owned();
    let agents = c.get("agents").unwrap();
    assert!(agents.iter().any(|a| a["id"] == agent.as_str()));

    let job = command(&mut c, "run_job", json!({ "project": project, "command": "echo from-loomd", "name": "d" }))["job"].as_str().unwrap().to_owned();
    until(&mut c, "jobs", Duration::from_secs(10), "the job ending", |jobs| jobs.iter().any(|j| j["id"] == job.as_str() && j["status"] == "exited" && j["exit_code"] == 0));
    let output = c.get(&format!("job/{job}")).unwrap();
    let screen = loom_core::api::wire::bytes_of(output[0]["screen"].as_str().unwrap());
    assert!(String::from_utf8_lossy(&screen).contains("from-loomd"));

    let url = command(&mut c, "web_open", json!({}))["url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("http://127.0.0.1:"));

    // The Notes chat, with no window: a chat opened, an agent working in the vault.
    let chat = command(&mut c, "notes_open", json!({}))["agent"].as_str().unwrap().to_owned();
    assert!(chat.parse::<u32>().unwrap() > 1_000_000);
    assert!(c.get(&format!("conversation/{chat}")).is_ok());
    assert!(c.get("notes_chats").is_ok());
    assert!(c.get("scribe").is_ok(), "Scribe runs");
    assert!(c.get("asks").unwrap().is_empty());
    assert!(c.get("schedules").unwrap().is_empty());
    // Renamed; bypassing permissions isn't for the CLI.
    command(&mut c, "rename", json!({ "agent": agent, "title": "Loomd ok" }));
    let renamed = until(&mut c, "agents", Duration::from_secs(5), "the new title", |a| a.iter().any(|a| a["title"] == "Loomd ok"));
    assert!(renamed.iter().any(|a| a["id"] == agent.as_str()));
    let bypass = c.command(&new_id(), "set_mode", json!({ "agent": agent, "mode": "bypassPermissions" }));
    assert!(bypass.is_err_and(|e| e.message.contains("window")));

    if std::env::var_os("LOOM_E2E_AGENT").is_some() {
        command(&mut c, "send", json!({ "agent": agent, "text": "Reply with exactly: loomd ok" }));
        until(&mut c, &format!("conversation/{agent}"), Duration::from_secs(120), "the reply", |entries| {
            entries.iter().any(|e| e["kind"] == "agent" && e["text"].as_str().is_some_and(|t| t.contains("loomd ok"))) && entries.last().is_some_and(|e| e["kind"] == "turn")
        });
        eprintln!("loomd test: a turn with no window");
        // A file to fill in: an ask, done from here, and the agent goes on.
        let prompt = "Use the loom_edit tool to have me fill in the file secret.env with the message 'type anything'. When it returns, reply with exactly: edit ok";
        command(&mut c, "send", json!({ "agent": agent, "text": prompt }));
        let asks = until(&mut c, "asks", Duration::from_secs(120), "the ask", |asks| asks.iter().any(|a| a["kind"] == "edit"));
        let ask = asks.iter().find(|a| a["kind"] == "edit").unwrap();
        assert_eq!(ask["agent"], agent.as_str());
        assert!(ask["path"].as_str().unwrap().ends_with("secret.env"));
        std::fs::write(project.join("secret.env"), "KEY=value\n").unwrap();
        command(&mut c, "ask_done", json!({ "id": ask["id"] }));
        until(&mut c, &format!("conversation/{agent}"), Duration::from_secs(120), "the reply after the edit", |entries| {
            entries.iter().any(|e| e["kind"] == "agent" && e["text"].as_str().is_some_and(|t| t.contains("edit ok"))) && entries.last().is_some_and(|e| e["kind"] == "turn")
        });
        assert!(c.get("asks").unwrap().is_empty(), "done: gone");
        eprintln!("loomd test: a file filled in through an ask");
    }

    // Stopped: the workspace saved, the socket gone.
    // SAFETY: a plain signal to the child started above.
    unsafe { libc_kill(daemon.child.id() as i32, 15) };
    let status = daemon.child.wait().unwrap();
    assert_eq!(status.code(), Some(0), "loomd ends cleanly on SIGTERM");
    let saved = std::fs::read_to_string(root.join("state/loom/state-LoomE2eDaemon.json")).expect("the workspace saved");
    assert!(saved.contains(project.to_str().unwrap()), "{saved}");
    assert!(!loom_core::api::socket::socket_path(APP_ID).exists(), "its socket removed");
}

unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, signal: i32) -> i32;
}

/// Opens `link` as a browser would (the token for a session, then the WebSocket): who the page is.
fn page_principal(link: &str) -> String {
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
    let (mut ws, _) = tungstenite::client::client(request, std::net::TcpStream::connect(host).unwrap()).map_err(|e| e.to_string()).unwrap();
    ws.send(tungstenite::Message::text(json!({ "jsonrpc": "2.0", "id": "1", "method": "initialize", "params": {} }).to_string())).unwrap();
    let hello: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
    hello["result"]["principal"]["kind"].as_str().unwrap_or_default().to_owned()
}
