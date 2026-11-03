//! `loom api`: the command-line client of the core API (warp, step 3). It mirrors every
//! request and subscription, so it is also how tests and scripts drive a running loom.
//!
//! ```text
//! loom api info                      the handshake: protocol, who loom takes you for, commands
//! loom api get <resource>            the resource as it is now (its items, as JSON)
//! loom api watch <resource>…         its messages as they come, one JSON object per line
//! loom api do <command> [params]     a command (params: a JSON object); prints its result
//! loom api raw                       JSON-RPC lines from stdin to loom, loom's to stdout
//! ```
//!
//! The socket: `--socket <path>`, else `LOOM_API`, else the one of `LOOM_APP_ID`'s loom.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Applied, Message, Mirror};

const USAGE: &str = "usage: loom api [--socket <path>] info | get <resource> | watch <resource>… | do <command> [<params JSON>] [--id <id>] | raw";

/// A connection to a running loom, initialized.
pub struct Connection {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next: u64,
    /// What `initialize` answered.
    pub hello: Value,
    /// Subscription messages that came while waiting for a response.
    early: Vec<Message>,
}

/// A JSON-RPC error loom answered, or a failure to talk to it.
#[derive(Debug)]
pub struct Failure {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

fn io(err: impl std::fmt::Display) -> Failure {
    Failure { code: 0, message: err.to_string() }
}

/// A command id unique enough across runs (a retry passes its own with `--id`).
pub fn new_id() -> String {
    format!("cli-{}", uuid::Uuid::new_v4().simple())
}

impl Connection {
    pub fn open(socket: &std::path::Path) -> Result<Connection, Failure> {
        let stream = UnixStream::connect(socket).map_err(|e| io(format!("loom is not reachable at {} ({e})", socket.display())))?;
        let writer = stream.try_clone().map_err(io)?;
        let mut connection = Connection { reader: BufReader::new(stream), writer, next: 0, hello: Value::Null, early: Vec::new() };
        let params = json!({ "protocol": { "min": super::rpc::PROTOCOL, "max": super::rpc::PROTOCOL }, "client": { "name": "loom api", "version": env!("CARGO_PKG_VERSION") }, "capabilities": {} });
        connection.hello = connection.request("initialize", params)?;
        Ok(connection)
    }

    fn send(&mut self, message: &Value) -> Result<(), Failure> {
        writeln!(self.writer, "{message}").map_err(io)
    }

    /// The next line from loom (None: it closed the connection).
    fn line(&mut self) -> Result<Option<Value>, Failure> {
        let mut line = String::new();
        if self.reader.read_line(&mut line).map_err(io)? == 0 {
            return Ok(None);
        }
        serde_json::from_str(&line).map(Some).map_err(|e| io(format!("bad line from loom: {e}")))
    }

    /// Sends request `method` with `id`, waits for its answer.
    fn call(&mut self, id: Value, method: &str, params: Value) -> Result<Value, Failure> {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        loop {
            let Some(message) = self.line()? else { return Err(io("loom closed the connection")) };
            if message["method"] == "subscription" {
                if let Ok(m) = serde_json::from_value(message["params"].clone()) {
                    self.early.push(m);
                }
                continue;
            }
            if message["id"] != id {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(Failure { code: error["code"].as_i64().unwrap_or(0), message: error["message"].as_str().unwrap_or("error").to_owned() });
            }
            return Ok(message["result"].clone());
        }
    }

    /// A protocol request (`subscribe`, `resync`…).
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, Failure> {
        self.next += 1;
        self.call(json!(self.next), method, params)
    }

    /// A command, with the id it is known by (sent again with the same id, it is not done twice).
    pub fn command(&mut self, id: &str, method: &str, params: Value) -> Result<Value, Failure> {
        self.call(json!(id), method, params)
    }

    /// Follows `resource`: its subscription id.
    pub fn subscribe(&mut self, resource: &str) -> Result<String, Failure> {
        let answer = self.request("subscribe", json!({ "resource": resource }))?;
        Ok(answer["sub"].as_str().unwrap_or_default().to_owned())
    }

    /// The next subscription message (None: loom closed the connection).
    pub fn next_message(&mut self) -> Result<Option<Message>, Failure> {
        if !self.early.is_empty() {
            return Ok(Some(self.early.remove(0)));
        }
        loop {
            let Some(message) = self.line()? else { return Ok(None) };
            if message["method"] == "subscription"
                && let Ok(m) = serde_json::from_value(message["params"].clone())
            {
                return Ok(Some(m));
            }
        }
    }

    /// `resource`'s items as they are now.
    pub fn get(&mut self, resource: &str) -> Result<Vec<Value>, Failure> {
        let sub = self.subscribe(resource)?;
        let mut mirror = Mirror::default();
        while let Some(message) = self.next_message()? {
            if message.sub != sub {
                continue;
            }
            match mirror.apply(&message) {
                Applied::Gap => {
                    self.request("resync", json!({ "sub": sub }))?;
                }
                _ if mirror.synced || mirror.gone => break,
                _ => {}
            }
        }
        let _ = self.request("unsubscribe", json!({ "sub": sub }));
        Ok(mirror.items)
    }
}

/// The socket to talk to (see the module's docs).
pub fn default_socket() -> PathBuf {
    if let Some(path) = std::env::var_os("LOOM_API").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    let app_id = std::env::var("LOOM_APP_ID").unwrap_or_else(|_| "dev.shirvani.Loom".to_owned());
    super::socket::socket_path(&app_id)
}

/// `loom api <args>`: the exit code.
pub fn run(args: &[String]) -> i32 {
    let mut args: Vec<String> = args.to_vec();
    let mut socket = default_socket();
    if let Some(at) = args.iter().position(|a| a == "--socket") {
        let Some(path) = args.get(at + 1).cloned() else { return usage() };
        socket = PathBuf::from(path);
        args.drain(at..at + 2);
    }
    let Some(verb) = args.first().cloned() else { return usage() };
    if verb == "--help" || verb == "-h" || verb == "help" {
        println!("{USAGE}");
        return 0;
    }
    if verb == "raw" {
        return raw(&socket);
    }
    let mut connection = match Connection::open(&socket) {
        Ok(connection) => connection,
        Err(err) => return fail(&err),
    };
    let result = match (verb.as_str(), &args[1..]) {
        ("info", []) => {
            print(&connection.hello);
            Ok(())
        }
        ("get", [resource]) => connection.get(resource).map(|items| print(&Value::Array(items))),
        ("watch", resources) if !resources.is_empty() => watch(&mut connection, resources),
        ("do", [method, rest @ ..]) => {
            let mut rest = rest.to_vec();
            let mut id = new_id();
            if let Some(at) = rest.iter().position(|a| a == "--id") {
                let Some(given) = rest.get(at + 1).cloned() else { return usage() };
                id = given;
                rest.drain(at..at + 2);
            }
            let params = match rest.as_slice() {
                [] => json!({}),
                [params] => match serde_json::from_str::<Value>(params) {
                    Ok(params) if params.is_object() => params,
                    _ => {
                        eprintln!("loom api: params must be a JSON object");
                        return 2;
                    }
                },
                _ => return usage(),
            };
            connection.command(&id, method, params).map(|result| print(&result))
        }
        _ => return usage(),
    };
    match result {
        Ok(()) => 0,
        Err(err) => fail(&err),
    }
}

fn watch(connection: &mut Connection, resources: &[String]) -> Result<(), Failure> {
    let mut followed = 0;
    for resource in resources {
        connection.subscribe(resource)?;
        followed += 1;
    }
    let stdout = std::io::stdout();
    while let Some(message) = connection.next_message()? {
        let ended = message.body == super::Body::Gone;
        let line = serde_json::to_string(&message).map_err(io)?;
        let mut out = stdout.lock();
        if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
            return Ok(());
        }
        if ended {
            followed -= 1;
            if followed == 0 {
                return Ok(());
            }
        }
    }
    Err(io("loom closed the connection"))
}

/// Lines both ways, as they are (no handshake: the caller speaks the protocol).
fn raw(socket: &std::path::Path) -> i32 {
    let stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(e) => return fail(&io(format!("loom is not reachable at {} ({e})", socket.display()))),
    };
    let Ok(mut writer) = stream.try_clone() else { return 1 };
    let reader = std::thread::spawn(move || {
        let stdout = std::io::stdout();
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            let mut out = stdout.lock();
            if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
                break;
            }
        }
    });
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if writeln!(writer, "{line}").is_err() {
            break;
        }
    }
    // Input ended: loom's answers to what was sent still come, until it is quiet.
    let _ = writer.shutdown(std::net::Shutdown::Write);
    let _ = reader.join();
    0
}

fn print(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
}

fn usage() -> i32 {
    eprintln!("{USAGE}");
    2
}

fn fail(err: &Failure) -> i32 {
    eprintln!("loom api: {err}");
    1
}
