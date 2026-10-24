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

/// The open agents, in order, and where to show them.
#[derive(Debug, Default, PartialEq)]
pub struct Layout {
    pub agents: Vec<Saved>,
    pub focused: usize,
    /// The agent shown beside the focused one.
    pub pinned: Option<usize>,
}

pub fn load(app_id: &str) -> Layout {
    let Ok(text) = std::fs::read_to_string(path(app_id)) else { return Layout::default() };
    parse(&text)
}

fn parse(text: &str) -> Layout {
    let Ok(value) = serde_json::from_str::<Value>(text) else { return Layout::default() };
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
    Layout {
        agents,
        focused: value["focused"].as_u64().unwrap_or(0) as usize,
        pinned: value["pinned"].as_u64().map(|p| p as usize),
    }
}

fn render(layout: &Layout) -> String {
    let agents: Vec<Value> = layout
        .agents
        .iter()
        .map(|a| json!({ "cwd": a.cwd.to_string_lossy(), "sessionId": a.session_id, "title": a.title }))
        .collect();
    serde_json::to_string_pretty(&json!({ "agents": agents, "focused": layout.focused, "pinned": layout.pinned }))
        .unwrap_or_default()
}

pub fn save(app_id: &str, layout: &Layout) {
    let path = path(app_id);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Written aside and renamed, so a crash mid-write can't lose the list.
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, render(layout)).is_ok() {
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
        let layout = Layout { agents, focused: 1, pinned: Some(0) };
        assert_eq!(parse(&render(&layout)), layout);
    }

    #[test]
    fn files_from_before_pinning_read_without_a_pin() {
        let layout = parse(r#"{ "agents": [{ "cwd": "/tmp" }], "focused": 0 }"#);
        assert_eq!(layout.pinned, None);
        assert_eq!(layout.agents.len(), 1);
    }

    #[test]
    fn a_broken_file_reads_as_empty() {
        assert_eq!(parse("{nope"), Layout::default());
    }

    #[test]
    fn test_instances_get_their_own_file() {
        assert!(path("dev.shirvani.Loom").ends_with("loom/state.json"));
        assert!(path("dev.shirvani.LoomTest").ends_with("loom/state-LoomTest.json"));
    }
}
