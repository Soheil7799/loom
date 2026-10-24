//! What survives a restart: which agents were open, where, and their ACP session ids (the
//! conversations themselves live in Claude Code's own session files).

use std::path::PathBuf;

use gtk::glib;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    pub title: String,
}

/// One file per app id, so a test instance doesn't touch the real list.
fn path(app_id: &str) -> PathBuf {
    let name = match app_id.rsplit('.').next() {
        Some("Loom") | None => "state.json".to_owned(),
        Some(suffix) => format!("state-{suffix}.json"),
    };
    dir().join(name)
}

/// $XDG_STATE_HOME/loom (~/.local/state/loom).
pub fn dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"))
        .join("loom")
}

pub fn load(app_id: &str) -> (Vec<Saved>, usize) {
    let Ok(text) = std::fs::read_to_string(path(app_id)) else { return (Vec::new(), 0) };
    parse(&text)
}

fn parse(text: &str) -> (Vec<Saved>, usize) {
    let Ok(value) = serde_json::from_str::<Value>(text) else { return (Vec::new(), 0) };
    let agents = value["agents"]
        .as_array()
        .map(|agents| {
            agents
                .iter()
                .filter_map(|a| {
                    Some(Saved {
                        cwd: PathBuf::from(a["cwd"].as_str()?),
                        session_id: a["sessionId"].as_str().map(str::to_owned),
                        title: a["title"].as_str().unwrap_or("").to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (agents, value["focused"].as_u64().unwrap_or(0) as usize)
}

fn render(agents: &[Saved], focused: usize) -> String {
    let agents: Vec<Value> = agents
        .iter()
        .map(|a| json!({ "cwd": a.cwd.to_string_lossy(), "sessionId": a.session_id, "title": a.title }))
        .collect();
    serde_json::to_string_pretty(&json!({ "agents": agents, "focused": focused })).unwrap_or_default()
}

pub fn save(app_id: &str, agents: &[Saved], focused: usize) {
    let path = path(app_id);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Written aside and renamed, so a crash mid-write can't lose the list.
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, render(agents, focused)).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_agents_read_back() {
        let agents = vec![
            Saved { cwd: "/home/x/p".into(), session_id: Some("abc".into()), title: "Fix it".into() },
            Saved { cwd: "/tmp".into(), session_id: None, title: String::new() },
        ];
        assert_eq!(parse(&render(&agents, 1)), (agents, 1));
    }

    #[test]
    fn a_broken_file_reads_as_empty() {
        assert_eq!(parse("{nope"), (Vec::new(), 0));
    }

    #[test]
    fn test_instances_get_their_own_file() {
        assert!(path("dev.shirvani.Loom").ends_with("loom/state.json"));
        assert!(path("dev.shirvani.LoomTest").ends_with("loom/state-LoomTest.json"));
    }
}
