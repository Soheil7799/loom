//! loom's own Claude folder: skills, agents, commands and hooks that only loom's agents use
//! (`claude` in a terminal never loads them). It is a local Claude Code plugin, passed to every
//! session loom starts; the user reads and edits it in Settings → Claude.
//!
//! Layout (a test instance keeps its own: `claude-<suffix>`):
//!
//! ```text
//! ~/.config/loom/claude/
//! ├── .claude-plugin/plugin.json
//! ├── skills/<name>/SKILL.md
//! ├── agents/<name>.md
//! ├── commands/<name>.md
//! ├── hooks/hooks.json
//! └── disabled/            what is switched off, laid out the same way (never loaded)
//! ```

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gtk::glib;

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// The skills loom ships: (name, SKILL.md).
const DEFAULT_SKILLS: &[(&str, &str)] = &[("loom", include_str!("../data/skill/SKILL.md"))];

const MANIFEST: &str = r#"{
  "name": "loom",
  "description": "loom's own skills, agents, commands and hooks, for its agents only",
  "author": { "name": "loom" }
}
"#;

const HOOKS: &str = "{\n  \"hooks\": {}\n}\n";

/// What the folder holds, by kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Skill,
    Agent,
    Command,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Skill, Kind::Agent, Kind::Command];

    fn folder(self) -> &'static str {
        match self {
            Kind::Skill => "skills",
            Kind::Agent => "agents",
            Kind::Command => "commands",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Kind::Skill => "Skills",
            Kind::Agent => "Agents",
            Kind::Command => "Commands",
        }
    }

    pub fn one(self) -> &'static str {
        match self {
            Kind::Skill => "skill",
            Kind::Agent => "agent",
            Kind::Command => "command",
        }
    }

    /// What an item's file is, in its folder.
    fn file(self, root: &Path, name: &str) -> PathBuf {
        match self {
            Kind::Skill => root.join(self.folder()).join(name).join("SKILL.md"),
            _ => root.join(self.folder()).join(format!("{name}.md")),
        }
    }

    /// What is moved when it is switched on or off.
    fn entry(self, root: &Path, name: &str) -> PathBuf {
        match self {
            Kind::Skill => root.join(self.folder()).join(name),
            _ => root.join(self.folder()).join(format!("{name}.md")),
        }
    }

    fn template(self, name: &str) -> String {
        match self {
            Kind::Skill => format!(
                "---\nname: {name}\ndescription: Use when … (when Claude should reach for this skill)\n---\n\n# {name}\n\nWhat to do, step by step.\n"
            ),
            Kind::Agent => format!(
                "---\nname: {name}\ndescription: Use for … (when Claude should hand work to this agent)\nmodel: haiku\n---\n\nYou are … What you do, and what you return.\n"
            ),
            Kind::Command => "---\ndescription: What /this does\n---\n\nThe prompt this command sends. $ARGUMENTS is what follows it.\n".to_owned(),
        }
    }
}

/// One skill, agent or command.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: Kind,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    /// The file to edit.
    pub file: PathBuf,
    /// One loom ships (Restore puts it back as it was).
    pub shipped: bool,
}

/// The folder for `app_id` (set once, at start), made with loom's defaults if missing.
pub fn init(app_id: &str) {
    let name = crate::state::suffix(app_id).map_or("claude".to_owned(), |s| format!("claude-{s}"));
    let dir = glib::user_config_dir().join("loom").join(name);
    if let Err(err) = prepare(&dir) {
        crate::log::write("plugin", &format!("folder {}: {err}", dir.display()));
    }
    let _ = DIR.set(dir);
}

pub fn dir() -> PathBuf {
    DIR.get().cloned().unwrap_or_else(|| glib::user_config_dir().join("loom/claude"))
}

/// The manifest, the hooks file and loom's skills, where they are missing (never over the
/// user's changes; a skill they switched off stays off).
fn prepare(dir: &Path) -> std::io::Result<()> {
    write_new(&dir.join(".claude-plugin/plugin.json"), MANIFEST)?;
    write_new(&dir.join("hooks/hooks.json"), HOOKS)?;
    for kind in Kind::ALL {
        std::fs::create_dir_all(dir.join(kind.folder()))?;
    }
    for (name, text) in DEFAULT_SKILLS {
        if !Kind::Skill.entry(&dir.join("disabled"), name).exists() {
            write_new(&Kind::Skill.file(dir, name), text)?;
        }
    }
    Ok(())
}

fn write_new(path: &Path, text: &str) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

/// The plugin as session/new's `_meta.claudeCode.options.plugins` wants it.
pub fn session_plugins() -> serde_json::Value {
    serde_json::json!([{ "type": "local", "path": dir().to_string_lossy() }])
}

/// Everything of `kind`, on and off, by name.
pub fn items(kind: Kind) -> Vec<Item> {
    let root = dir();
    let mut all = Vec::new();
    for (base, enabled) in [(root.clone(), true), (root.join("disabled"), false)] {
        let Ok(entries) = std::fs::read_dir(base.join(kind.folder())) else { continue };
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let name = match kind {
                Kind::Skill if entry.path().is_dir() => file_name,
                Kind::Agent | Kind::Command => match file_name.strip_suffix(".md") {
                    Some(name) => name.to_owned(),
                    None => continue,
                },
                Kind::Skill => continue,
            };
            let file = kind.file(&base, &name);
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let shipped = kind == Kind::Skill && DEFAULT_SKILLS.iter().any(|(n, _)| *n == name);
            all.push(Item { kind, description: description(&text), enabled, file, shipped, name });
        }
    }
    all.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    all
}

/// The `description:` of a file's frontmatter.
fn description(text: &str) -> String {
    let Some(front) = text.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---").map(|(f, _)| f)) else { return String::new() };
    front
        .lines()
        .find_map(|line| line.strip_prefix("description:"))
        .map(|d| d.trim().trim_matches('"').to_owned())
        .unwrap_or_default()
}

/// Switches an item on (back into the plugin) or off (into `disabled/`).
pub fn set_enabled(item: &Item, on: bool) -> std::io::Result<()> {
    let root = dir();
    let (from, to) = if on { (root.join("disabled"), root.clone()) } else { (root.clone(), root.join("disabled")) };
    let (from, to) = (item.kind.entry(&from, &item.name), item.kind.entry(&to, &item.name));
    if to.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} is there already", to.display())));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(from, to)
}

/// A new item from its template; its file (to open). Err if the name is taken or unusable.
pub fn create(kind: Kind, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    let usable = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !usable {
        return Err("Use letters, digits, - and _ only.".to_owned());
    }
    let root = dir();
    if kind.entry(&root, name).exists() || kind.entry(&root.join("disabled"), name).exists() {
        return Err(format!("There is a {} named {name} already.", kind.one()));
    }
    let file = kind.file(&root, name);
    write_new(&file, &kind.template(name)).map_err(|e| e.to_string())?;
    Ok(file)
}

/// loom's own skill as it ships, over whatever it became (switched on again if it was off).
pub fn restore(name: &str) -> std::io::Result<()> {
    let Some((_, text)) = DEFAULT_SKILLS.iter().find(|(n, _)| *n == name) else { return Ok(()) };
    let root = dir();
    let off = Kind::Skill.entry(&root.join("disabled"), name);
    if off.exists() {
        std::fs::remove_dir_all(off)?;
    }
    let file = Kind::Skill.file(&root, name);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(file, text)
}

pub fn hooks_file() -> PathBuf {
    dir().join("hooks/hooks.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_come_from_the_frontmatter() {
        assert_eq!(description("---\nname: x\ndescription: Does a thing\n---\nbody"), "Does a thing");
        assert_eq!(description("---\ndescription: \"Quoted\"\n---\n"), "Quoted");
        assert_eq!(description("no frontmatter"), "");
    }

    #[test]
    fn templates_have_a_description() {
        for kind in Kind::ALL {
            assert!(!description(&kind.template("x")).is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn the_shipped_skill_names_itself() {
        let (name, text) = DEFAULT_SKILLS[0];
        assert!(text.contains(&format!("name: {name}")));
    }
}
