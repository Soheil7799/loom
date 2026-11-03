//! loom, through the core API on the connection loom made and handed this bridge (fd 3):
//! the bridge protocol (docs/bridges.md) rides on it. A request is the API's `bridge`
//! command, answered as `{"ok": …}` or `{"error": …}`; the events are the items added to the
//! `bridge/remote` resource. loom fixed who is on this connection (this bridge): there is no
//! secret, and no socket another process could reach.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{mpsc, oneshot};

type Waiting = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

#[derive(Clone)]
pub struct Loom {
    inner: Arc<Inner>,
}

struct Inner {
    /// The connection, until `subscribe` starts it.
    stream: Mutex<Option<std::os::unix::net::UnixStream>>,
    writer: tokio::sync::Mutex<Option<OwnedWriteHalf>>,
    waiting: Waiting,
    next: AtomicU64,
}

/// loom's events, one JSON object a line (as the bridge socket wrote them).
pub struct Events(mpsc::UnboundedReceiver<String>);

impl Events {
    pub async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        Ok(self.0.recv().await)
    }
}

impl Loom {
    /// The line loom writes on stdin when starting the bridge; the bridge ends when stdin closes
    /// (loom is gone).
    pub fn from_stdin() -> Result<Loom, String> {
        use std::os::fd::FromRawFd;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
        let hello: Value = serde_json::from_str(&line).map_err(|_| "Started by loom only (loom bridge remote).".to_owned())?;
        let fd = hello["api_fd"].as_i64().and_then(|fd| i32::try_from(fd).ok()).ok_or("This loom hands down no API connection: update loom.")?;
        // SAFETY: loom left `fd` open for this process; it is taken once, here.
        let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
        std::thread::spawn(|| {
            let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
            std::process::exit(0);
        });
        Ok(Loom { inner: Arc::new(Inner { stream: Mutex::new(Some(stream)), writer: tokio::sync::Mutex::default(), waiting: Arc::default(), next: AtomicU64::new(1) }) })
    }

    /// Request `method` of the core API, its result (or the error loom said).
    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        // Unique to this run too: loom remembers commands by id (a retry isn't done twice).
        let id = format!("r{}-{}", std::process::id(), self.inner.next.fetch_add(1, Ordering::Relaxed));
        let (answer, answered) = oneshot::channel();
        self.inner.waiting.lock().unwrap().insert(id.clone(), answer);
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string() + "\n";
        {
            let mut writer = self.inner.writer.lock().await;
            let writer = writer.as_mut().ok_or("not connected to loom yet")?;
            writer.write_all(line.as_bytes()).await.map_err(|e| format!("loom is not reachable: {e}"))?;
        }
        let message = answered.await.map_err(|_| "loom closed the connection".to_owned())?;
        match message.get("error") {
            Some(error) => Err(error["message"].as_str().unwrap_or("error").to_owned()),
            None => Ok(message["result"].clone()),
        }
    }

    /// One request, its answer as loom wrote it (`{"ok"}` or `{"error"}`).
    pub async fn call(&self, method: &str, params: Value) -> Value {
        self.request("bridge", json!({ "method": method, "params": params })).await.unwrap_or_else(|error| json!({ "error": error }))
    }

    /// Connects (the handshake, then following this bridge's events): loom's events, until
    /// loom goes.
    pub async fn subscribe(&self) -> Result<Events, String> {
        let stream = self.inner.stream.lock().unwrap().take().ok_or("subscribed once already")?;
        stream.set_nonblocking(true).map_err(|e| e.to_string())?;
        let (reader, writer) = UnixStream::from_std(stream).map_err(|e| e.to_string())?.into_split();
        *self.inner.writer.lock().await = Some(writer);
        let (events, heard) = mpsc::unbounded_channel();
        let waiting = self.inner.waiting.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                if message["method"] == "subscription" {
                    // An event is an item added (and taken away again at once).
                    for op in message["params"]["ops"].as_array().into_iter().flatten().filter(|op| op["op"] == "upsert") {
                        let _ = events.send(op["item"]["event"].to_string());
                    }
                    continue;
                }
                let id = message["id"].as_str().unwrap_or_default().to_owned();
                if let Some(answer) = waiting.lock().unwrap().remove(&id) {
                    let _ = answer.send(message);
                }
            }
            // loom has gone: the events end, so does the bridge.
            waiting.lock().unwrap().clear();
        });
        let params = json!({ "protocol": { "min": 1, "max": 1 }, "client": { "name": "loom-remote", "version": env!("CARGO_PKG_VERSION") }, "capabilities": {} });
        self.request("initialize", params).await?;
        self.request("subscribe", json!({ "resource": "bridge/remote" })).await?;
        Ok(Events(heard))
    }
}
