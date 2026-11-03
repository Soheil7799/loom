//! A connection to the core API used from several places at once (a bridge): requests from
//! any thread, each waiting for its own answer, while what subscriptions bring arrives on a
//! channel. A thread of its own reads the connection and sorts the two.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use serde_json::{Value, json};

use super::Message;

type Waiting = Arc<Mutex<HashMap<String, mpsc::Sender<Result<Value, String>>>>>;

#[derive(Clone)]
pub struct Shared {
    writer: Arc<Mutex<UnixStream>>,
    waiting: Waiting,
    next: Arc<AtomicU64>,
    /// What `initialize` answered.
    pub hello: Value,
}

impl Shared {
    /// Initializes `stream` as client `name`; subscription messages go to `messages` (the
    /// channel closes when the connection does).
    pub fn new(stream: UnixStream, name: &str, messages: async_channel::Sender<Message>) -> Result<Shared, String> {
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        let waiting: Waiting = Arc::default();
        let sorting = waiting.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                if message["method"] == "subscription" {
                    if let Ok(m) = serde_json::from_value::<Message>(message["params"].clone())
                        && messages.send_blocking(m).is_err()
                    {
                        break;
                    }
                    continue;
                }
                let id = match &message["id"] {
                    Value::String(id) => id.clone(),
                    other => other.to_string(),
                };
                let Some(answer) = sorting.lock().unwrap().remove(&id) else { continue };
                let _ = answer.send(match message.get("error") {
                    Some(error) => Err(error["message"].as_str().unwrap_or("error").to_owned()),
                    None => Ok(message["result"].clone()),
                });
            }
            // Closed: whoever waits hears so.
            for (_, answer) in sorting.lock().unwrap().drain() {
                let _ = answer.send(Err("loom closed the connection".into()));
            }
        });
        let mut shared = Shared { writer: Arc::new(Mutex::new(stream)), waiting, next: Arc::default(), hello: Value::Null };
        let params = json!({ "protocol": { "min": super::rpc::PROTOCOL, "max": super::rpc::PROTOCOL }, "client": { "name": name, "version": env!("CARGO_PKG_VERSION") }, "capabilities": {} });
        shared.hello = shared.call("initialize", params)?;
        Ok(shared)
    }

    /// Request `method` (a protocol request or a command; command ids are made unique here),
    /// waiting for its answer.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = format!("b{}-{}", std::process::id(), self.next.fetch_add(1, Ordering::Relaxed));
        let (answer, answered) = mpsc::channel();
        self.waiting.lock().unwrap().insert(id.clone(), answer);
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string() + "\n";
        if let Err(err) = self.writer.lock().unwrap().write_all(line.as_bytes()) {
            self.waiting.lock().unwrap().remove(&id);
            return Err(format!("loom is not reachable ({err})"));
        }
        answered.recv().unwrap_or_else(|_| Err("loom closed the connection".into()))
    }
}
