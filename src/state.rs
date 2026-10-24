//! What survives a restart: the projects in the sidebar, which agents were open in them and
//! their ACP session ids (the conversations themselves live in Claude Code's own session
//! files), what was focused and pinned, and the agents closed lately (to reopen).

use std::path::PathBuf;

use gtk::glib;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    pub title: String,
    /// The title was given by hand: the agent's own titles don't replace it.
    pub named: bool,
}

impl Saved {
    pub fn new(cwd: PathBuf) -> Saved {
        Saved { cwd, session_id: None, title: String::new(), named: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub path: PathBuf,
    pub collapsed: bool,
}

/// The open agents, in order, and where to show them.
#[derive(Debug, Default, PartialEq)]
pub struct Layout {
    pub projects: Vec<Project>,
    pub agents: Vec<Saved>,
    pub focused: usize,
    /// The agent shown beside the focused one.
    pub pinned: Option<usize>,
    /// Closed lately, newest first.
    pub closed: Vec<Saved>,
}

/// How many closed agents are remembered.
pub const CLOSED_KEPT: usize = 12;

/// The app id's suffix past "Loom" names a test instance, which keeps its own files.
pub fn suffix(app_id: &str) -> Option<&str> {
    match app_id.rsplit('.').next() {
        Some("Loom") | None => None,
        Some(suffix) => Some(suffix),
    }
}

fn path(app_id: &str) -> PathBuf {
    dir().join(match suffix(app_id) {
        None => "state.json".to_owned(),
        Some(suffix) => format!("state-{suffix}.json"),
    })
}

/// $XDG_STATE_HOME/loom (~/.local/state/loom).
pub fn dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"))
        .join("loom")
}

pub fn load(app_id: &str) -> Layout {
    let Ok(text) = std::fs::read_to_string(path(app_id)) else { return Layout::default() };
    parse(&text)
}

fn saved(a: &Value) -> Option<Saved> {
    Some(Saved {
        cwd: PathBuf::from(a["cwd"].as_str()?),
        session_id: a["sessionId"].as_str().map(str::to_owned),
        title: a["title"].as_str().unwrap_or("").to_owned(),
        named: a["named"].as_bool().unwrap_or(false),
    })
}

fn saved_json(a: &Saved) -> Value {
    json!({ "cwd": a.cwd.to_string_lossy(), "sessionId": a.session_id, "title": a.title, "named": a.named })
}

fn parse(text: &str) -> Layout {
    let Ok(value) = serde_json::from_str::<Value>(text) else { return Layout::default() };
    let list = |key: &str| value[key].as_array().map(|all| all.iter().filter_map(saved).collect()).unwrap_or_default();
    let projects = value["projects"]
        .as_array()
        .map(|all| {
            all.iter()
                .filter_map(|p| Some(Project { path: PathBuf::from(p["path"].as_str()?), collapsed: p["collapsed"].as_bool().unwrap_or(false) }))
                .collect()
        })
        .unwrap_or_default();
    Layout {
        projects,
        agents: list("agents"),
        focused: value["focused"].as_u64().unwrap_or(0) as usize,
        pinned: value["pinned"].as_u64().map(|p| p as usize),
        closed: list("closed"),
    }
}

fn render(layout: &Layout) -> String {
    let projects: Vec<Value> =
        layout.projects.iter().map(|p| json!({ "path": p.path.to_string_lossy(), "collapsed": p.collapsed })).collect();
    serde_json::to_string_pretty(&json!({
        "projects": projects,
        "agents": layout.agents.iter().map(saved_json).collect::<Vec<_>>(),
        "focused": layout.focused,
        "pinned": layout.pinned,
        "closed": layout.closed.iter().take(CLOSED_KEPT).map(saved_json).collect::<Vec<_>>(),
    }))
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
    fn saved_layouts_read_back() {
        let agents = vec![
            Saved { cwd: "/home/x/p".into(), session_id: Some("abc".into()), title: "Fix it".into(), named: true },
            Saved::new("/tmp".into()),
        ];
        let layout = Layout {
            projects: vec![Project { path: "/home/x/p".into(), collapsed: true }, Project { path: "/tmp".into(), collapsed: false }],
            agents: agents.clone(),
            focused: 1,
            pinned: Some(0),
            closed: vec![agents[0].clone()],
        };
        assert_eq!(parse(&render(&layout)), layout);
    }

    #[test]
    fn older_files_read_with_defaults() {
        let layout = parse(r#"{ "agents": [{ "cwd": "/tmp", "title": "t" }], "focused": 0 }"#);
        assert_eq!(layout.pinned, None);
        assert!(layout.projects.is_empty() && layout.closed.is_empty());
        assert!(!layout.agents[0].named);
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
