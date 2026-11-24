//! A paired device on the command line: what the phone app will do, to try the bridge before
//! there is one. Built on `loom-link`, as the phone apps are. Its key and the desktop it paired
//! with are kept in `$XDG_CONFIG_HOME/loom-remote/peer.json`.

use std::path::PathBuf;
use std::sync::Arc;

use data_encoding::HEXLOWER;
use loom_link::{Desktop, EndpointAddr};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

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

/// Saved aside and renamed (a crash mid-write can't lose the device key), owner-only: the
/// folder 0700, the file made anew at 0600 whatever an older one's mode was.
fn save(config: &Value) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| e.to_string())?;
        // An older folder too (DirBuilder's mode is for one it makes).
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let temporary = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&temporary);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary).map_err(|e| e.to_string())?;
    file.write_all(serde_json::to_string_pretty(config).unwrap_or_default().as_bytes()).map_err(|e| e.to_string())?;
    // On disk before it replaces the old one: a power cut can't leave an empty key file.
    file.sync_all().map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|e| e.to_string())
}

/// This device's key, made the first time.
fn key(config: &mut Value) -> [u8; 32] {
    let known = HEXLOWER.decode(config["key"].as_str().unwrap_or("").as_bytes()).ok().and_then(|b| <[u8; 32]>::try_from(b).ok());
    known.unwrap_or_else(|| {
        let key = loom_link::new_key();
        config["key"] = json!(HEXLOWER.encode(&key));
        key
    })
}

/// The desktop paired with. Where it is now is kept for next time.
async fn desktop() -> Result<Desktop, String> {
    let mut config = load();
    let addr: EndpointAddr = serde_json::from_value(config["desktop"].clone()).map_err(|_| "Not paired yet: loom-remote pair <ticket>".to_owned())?;
    let desktop = Desktop::connect(&key(&mut config), addr.clone()).await?;
    if *desktop.addr() != addr {
        config["desktop"] = json!(desktop.addr());
        let _ = save(&config);
    }
    Ok(desktop)
}

pub async fn pair(ticket: &str, name: &str) -> Result<(), String> {
    let mut config = load();
    let key = key(&mut config);
    save(&config)?;
    println!("This device: {}", loom_link::device_id(&key));
    println!("Confirm “{name}” in loom's settings…");
    config["desktop"] = json!(Desktop::pair(&key, ticket, name).await?);
    save(&config)?;
    println!("Paired.");
    Ok(())
}

/// Any request of the bridge protocol, its answer printed (as the phone app would get it).
pub async fn call(method: &str, params: &str) -> Result<(), String> {
    let params: Value = if params.trim().is_empty() { json!({}) } else { serde_json::from_str(params).map_err(|e| format!("params: {e}"))? };
    let answer = desktop().await?.request(method, params).await?;
    println!("{answer}");
    Ok(())
}

pub async fn agents() -> Result<(), String> {
    let desktop = desktop().await?;
    for agent in desktop.agents().await?.as_array().into_iter().flatten() {
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
    let agent = desktop().await?.send(agent, text, &[]).await?;
    println!("{}", line(&agent));
    Ok(())
}

pub async fn answer(agent: u64, serial: u64, option: u64) -> Result<(), String> {
    let answer = desktop().await?.answer(agent, serial, option).await?;
    println!("{}", if answer["answered"] == true { "Answered." } else { "Nothing to answer." });
    Ok(())
}

/// An agent's conversation, one line per entry (long ones cut).
pub async fn history(agent: u64) -> Result<(), String> {
    let history = desktop().await?.history(agent, "", 0).await?;
    if history["loading"] == true {
        println!("(still being read back: ask again in a moment)");
    }
    for entry in history["entries"].as_array().into_iter().flatten() {
        let kind = entry["kind"].as_str().unwrap_or("?");
        let text = match kind {
            "tool" => format!("{} [{}]", entry["title"].as_str().unwrap_or(""), entry["status"].as_str().unwrap_or("")),
            "turn" => format!("took {} s", entry["seconds"]),
            _ => entry["text"].as_str().unwrap_or("").to_owned(),
        };
        let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(100).collect();
        let pictures = entry["pictures"].as_u64().map_or(String::new(), |n| format!(" (+{n} pictures)"));
        println!("{:>4} {kind:<8} {text}{pictures}", entry["i"]);
    }
    Ok(())
}

/// loom's events as they come, until the desktop goes; lines typed meanwhile are sent over the
/// same connection (a second one with this key would knock this one off the relay).
pub async fn watch() -> Result<(), String> {
    let desktop = Arc::new(desktop().await?);
    let talking = desktop.clone();
    tokio::spawn(async move {
        let mut input = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(typed)) = input.next_line().await {
            let words: Vec<&str> = typed.split_whitespace().collect();
            let number = |w: &str| w.parse::<u64>().unwrap_or(0);
            let said = match words.as_slice() {
                [] => continue,
                ["answer", agent, serial, option] => talking.answer(number(agent), number(serial), number(option)).await,
                [agent, _, ..] if agent.parse::<u64>().is_ok() => {
                    let text = typed.trim_start().split_once(char::is_whitespace).map_or("", |(_, text)| text.trim());
                    talking.send(number(agent), text, &[]).await
                }
                _ => Err("type <agent> <text>, or answer <agent> <serial> <option>".to_owned()),
            };
            if let Err(err) = said {
                eprintln!("  {err}");
            }
        }
    });
    let mut events = desktop.subscribe().await?;
    println!("Watching (Ctrl+C to stop). Type <agent> <text> to send a message.");
    while let Some(event) = events.next().await? {
        let kind = event["event"].as_str().unwrap_or("?");
        match kind {
            "closed" => println!("closed  {}", event["agent"]),
            _ => println!("{kind:<7} {}{}", line(&event["agent"]), if event["looking"] == true { "  (looking)" } else { "" }),
        }
        if kind == "done"
            && let Some(reply) = event["reply"].as_str()
        {
            println!("{}\n", reply.trim_end());
        }
    }
    Ok(())
}
