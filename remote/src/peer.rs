//! A paired device on the command line: what the phone app will do, to try the bridge before
//! there is one. Its key and the desktop it paired with are kept in
//! `$XDG_CONFIG_HOME/loom-remote/peer.json`.

use std::path::PathBuf;

use data_encoding::HEXLOWER;
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, SecretKey};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::wire::{self, ALPN, Ticket};

fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_default();
    base.join("loom-remote").join("peer.json")
}

fn load() -> Value {
    std::fs::read(config_path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_else(|| json!({}))
}

fn save(config: &Value) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::io::Write;
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path).map_err(|e| e.to_string())?;
    file.write_all(serde_json::to_string_pretty(config).unwrap_or_default().as_bytes()).map_err(|e| e.to_string())
}

/// This device's key, made the first time.
fn key(config: &mut Value) -> SecretKey {
    let known = HEXLOWER.decode(config["key"].as_str().unwrap_or("").as_bytes()).ok().and_then(|b| <[u8; 32]>::try_from(b).ok());
    match known {
        Some(bytes) => SecretKey::from_bytes(&bytes),
        None => {
            let key = SecretKey::generate();
            config["key"] = json!(HEXLOWER.encode(&key.to_bytes()));
            key
        }
    }
}

/// A connection to the desktop at `addr`, through its relay. It lasts as long as the endpoint
/// it came with.
pub struct Desktop {
    _endpoint: Endpoint,
    connection: Connection,
}

impl std::ops::Deref for Desktop {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.connection
    }
}

async fn connect(key: SecretKey, addr: EndpointAddr) -> Result<Desktop, String> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .relay_mode(wire::relay_mode(addr.relay_urls().cloned()))
        .bind()
        .await
        .map_err(|e| e.to_string())?;
    let connection = endpoint.connect(addr, ALPN).await.map_err(|e| format!("The desktop is not reachable: {e}"))?;
    Ok(Desktop { _endpoint: endpoint, connection })
}

/// The desktop paired with. Its address now is kept for next time (its relay or network may
/// change; while it has none, the one known stays).
async fn desktop() -> Result<Desktop, String> {
    let mut config = load();
    let addr: EndpointAddr = serde_json::from_value(config["desktop"].clone()).map_err(|_| "Not paired yet: loom-remote pair <ticket>".to_owned())?;
    let desktop = connect(key(&mut config), addr.clone()).await?;
    if let Ok(now) = wire::request(&desktop, "address", Value::Null).await.and_then(|a| serde_json::from_value::<EndpointAddr>(a).map_err(|e| e.to_string()))
        && now.id == addr.id
        && now != addr
        && (now.relay_urls().next().is_some() || addr.relay_urls().next().is_none())
    {
        config["desktop"] = json!(now);
        let _ = save(&config);
    }
    Ok(desktop)
}

pub async fn pair(ticket: &str, name: &str) -> Result<(), String> {
    let ticket = Ticket::decode(ticket)?;
    let mut config = load();
    let key = key(&mut config);
    save(&config)?;
    println!("This device: {}", key.public());
    let connection = connect(key, ticket.addr.clone()).await?;
    println!("Connected. Confirm “{name}” in loom's settings…");
    wire::request(&connection, "pair", json!({ "code": ticket.code, "name": name })).await?;
    config["desktop"] = json!(ticket.addr);
    save(&config)?;
    connection.close(0u32.into(), b"bye");
    println!("Paired.");
    Ok(())
}

pub async fn agents() -> Result<(), String> {
    let connection = desktop().await?;
    for agent in wire::request(&connection, "agents", Value::Null).await?.as_array().into_iter().flatten() {
        println!("{}", line(agent));
        for q in agent["questions"].as_array().into_iter().flatten() {
            let options: Vec<String> = q["options"].as_array().into_iter().flatten().enumerate().map(|(i, o)| format!("{i}: {}", o[0].as_str().unwrap_or(""))).collect();
            println!("    ? [{}] {}  ({})", q["serial"], q["title"].as_str().unwrap_or(""), options.join(", "));
        }
    }
    Ok(())
}

fn line(agent: &Value) -> String {
    format!(
        "{:>4}  {:<9} {} · {}",
        agent["id"],
        agent["status"].as_str().unwrap_or(""),
        agent["title"].as_str().unwrap_or(""),
        agent["project"].as_str().unwrap_or("")
    )
}

pub async fn send(agent: u64, text: &str) -> Result<(), String> {
    let connection = desktop().await?;
    let agent = wire::request(&connection, "send", json!({ "agent": agent, "text": text })).await?;
    println!("{}", line(&agent));
    Ok(())
}

pub async fn answer(agent: u64, serial: u64, option: u64) -> Result<(), String> {
    let connection = desktop().await?;
    let answer = wire::request(&connection, "answer", json!({ "agent": agent, "serial": serial, "option": option })).await?;
    println!("{}", if answer["answered"] == true { "Answered." } else { "Nothing to answer." });
    Ok(())
}

/// loom's events as they come, until the desktop goes; lines typed meanwhile are sent over the
/// same connection (a second process with this key would knock this one off the relay).
pub async fn watch() -> Result<(), String> {
    let connection = desktop().await?;
    let talking = connection.connection.clone();
    tokio::spawn(async move {
        let mut input = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(typed)) = input.next_line().await {
            let words: Vec<&str> = typed.split_whitespace().collect();
            let said = match words.as_slice() {
                [] => continue,
                ["answer", agent, serial, option] => {
                    let number = |w: &str| w.parse::<u64>().unwrap_or(0);
                    wire::request(&talking, "answer", json!({ "agent": number(agent), "serial": number(serial), "option": number(option) })).await
                }
                [agent, _, ..] if agent.parse::<u64>().is_ok() => {
                    let text = typed.trim_start().split_once(char::is_whitespace).map_or("", |(_, text)| text.trim());
                    wire::request(&talking, "send", json!({ "agent": agent.parse::<u64>().unwrap_or(0), "text": text })).await
                }
                _ => Err("type <agent> <text>, or answer <agent> <serial> <option>".to_owned()),
            };
            if let Err(err) = said {
                eprintln!("  {err}");
            }
        }
    });
    let (mut send, recv) = connection.open_bi().await.map_err(|e| e.to_string())?;
    send.write_all(json!({ "method": "subscribe" }).to_string().as_bytes()).await.map_err(|e| e.to_string())?;
    send.finish().map_err(|e| e.to_string())?;
    println!("Watching (Ctrl+C to stop). Type <agent> <text> to send a message.");
    let mut lines = BufReader::new(recv).lines();
    while let Some(text) = lines.next_line().await.map_err(|e| e.to_string())? {
        let event: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        // Not an event: the desktop said no.
        if event.get("event").is_none() {
            return wire::result(&event).and(Err("refused".to_owned()));
        }
        let kind = event["event"].as_str().unwrap_or("?");
        match kind {
            "closed" => println!("closed  {}", event["agent"]),
            _ => println!("{kind:<7} {}{}", line(&event["agent"]), if event["looking"] == true { "  (looking)" } else { "" }),
        }
        if kind == "done" {
            if let Some(reply) = event["reply"].as_str() {
                println!("{}\n", reply.trim_end());
            }
        }
    }
    Ok(())
}
