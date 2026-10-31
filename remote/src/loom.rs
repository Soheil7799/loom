//! loom's bridge socket, as docs/bridges.md describes it.

use std::path::PathBuf;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Clone)]
pub struct Loom {
    socket: PathBuf,
    secret: String,
}

impl Loom {
    /// The line loom writes on stdin when starting the bridge; the bridge ends when stdin closes
    /// (loom is gone).
    pub fn from_stdin() -> Result<Loom, String> {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
        let hello: Value = serde_json::from_str(&line).map_err(|_| "Started by loom only (loom bridge remote).".to_owned())?;
        let loom = Loom { socket: PathBuf::from(hello["socket"].as_str().unwrap_or("")), secret: hello["secret"].as_str().unwrap_or("").to_owned() };
        std::thread::spawn(|| {
            let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
            std::process::exit(0);
        });
        Ok(loom)
    }

    async fn open(&self, method: &str, params: Value) -> Result<BufReader<UnixStream>, String> {
        let mut stream = UnixStream::connect(&self.socket).await.map_err(|e| format!("loom is not reachable: {e}"))?;
        let line = json!({ "secret": self.secret, "method": method, "params": params }).to_string() + "\n";
        stream.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
        Ok(BufReader::new(stream))
    }

    /// One request, its answer as loom wrote it (`{"ok"}` or `{"error"}`).
    pub async fn call(&self, method: &str, params: Value) -> Value {
        let answer = async {
            let mut reader = self.open(method, params).await?;
            let mut line = String::new();
            reader.read_line(&mut line).await.map_err(|e| e.to_string())?;
            serde_json::from_str::<Value>(&line).map_err(|_| "loom closed the connection".to_owned())
        };
        answer.await.unwrap_or_else(|error| json!({ "error": error }))
    }

    /// loom's events, until loom goes.
    pub async fn subscribe(&self) -> Result<tokio::io::Lines<BufReader<UnixStream>>, String> {
        Ok(self.open("subscribe", Value::Null).await?.lines())
    }
}
