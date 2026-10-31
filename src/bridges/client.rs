//! A bridge's side of the bridge socket: what loom said on stdin when it started the bridge,
//! requests to loom, and the events loom sends.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde_json::{Value, json};

#[derive(Clone)]
pub struct Client {
    socket: PathBuf,
    secret: String,
}

impl Client {
    /// What loom wrote on stdin (the socket, this run's secret, its app id). From then on the
    /// bridge ends when stdin closes: loom has gone.
    pub fn from_stdin() -> Result<(Client, String), String> {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
        let hello: Value = serde_json::from_str(&line).map_err(|_| "a bridge is started by loom, not by hand".to_owned())?;
        let (Some(socket), Some(secret)) = (hello["socket"].as_str(), hello["secret"].as_str()) else {
            return Err("loom said no socket or secret".into());
        };
        std::thread::spawn(|| {
            let _ = std::io::stdin().read_to_end(&mut Vec::new());
            std::process::exit(0);
        });
        let client = Client { socket: PathBuf::from(socket), secret: secret.to_owned() };
        Ok((client, hello["app"].as_str().unwrap_or("").to_owned()))
    }

    fn connect(&self, method: &str, params: Value) -> Result<UnixStream, String> {
        let mut stream = UnixStream::connect(&self.socket).map_err(|e| format!("loom is not reachable ({e})"))?;
        writeln!(stream, "{}", json!({ "secret": self.secret, "method": method, "params": params })).map_err(|e| e.to_string())?;
        Ok(stream)
    }

    /// One request: loom's answer, or what went wrong.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let stream = self.connect(method, params)?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).map_err(|e| e.to_string())?;
        let answer: Value = serde_json::from_str(&line).map_err(|e| format!("bad answer from loom: {e}"))?;
        match answer.get("error").and_then(Value::as_str) {
            Some(error) => Err(error.to_owned()),
            None => Ok(answer["ok"].clone()),
        }
    }

    /// loom's events, as they come. When loom stops telling (it closed), the bridge ends.
    pub fn subscribe(&self) -> Result<async_channel::Receiver<Value>, String> {
        let stream = self.connect("subscribe", Value::Null)?;
        let (tx, rx) = async_channel::unbounded();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<Value>(&line) {
                    // Refused (a stale secret): nothing will ever come.
                    Ok(event) if event.get("event").is_none() => break,
                    Ok(event) => {
                        if tx.send_blocking(event).is_err() {
                            break;
                        }
                    }
                    Err(_) => {}
                }
            }
            std::process::exit(0);
        });
        Ok(rx)
    }
}
