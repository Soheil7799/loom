//! Topics: what the Notes add-on keeps track of (finances, work, study…). Each one is a
//! Markdown file in the vault's `Topics/` folder: its settings in the frontmatter, and below
//! them, in plain words, how its notes are kept. The user edits them in loom or Obsidian.

use std::path::{Path, PathBuf};

use crate::vault::{self, Value};

/// Whether a topic's changes wait for the user's OK.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    Ask,
    Auto,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Topic {
    /// Its name (the `topic:` key, else the file's name).
    pub name: String,
    pub path: PathBuf,
    /// The folder of the vault its notes go in.
    pub folder: String,
    /// What makes a conversation about it.
    pub listens_for: Vec<String>,
    pub apply: Apply,
    /// May leave this machine when the vault is synced.
    pub sync: bool,
    pub model: String,
    pub enabled: bool,
    /// How its notes are kept, in the user's words.
    pub instructions: String,
}

/// The topics loom starts a vault with: (file name, text).
const DEFAULTS: &[(&str, &str)] = &[
    (
        "Finances",
        "---
topic: Finances
folder: Finances
listens-for: [salary, income, rent, bills, subscriptions, savings, taxes, invoices, budget, expenses, bank]
apply: ask
sync: false
model: haiku
enabled: true
---
Keep one note per month (`Finances/2026-01.md`, named by year and month) with a table of
income and expenses: date, what, amount, category. Keep `Finances/Overview.md` with what
recurs (salary, rent, subscriptions) and current balances or savings goals.

- Amounts keep their currency; for a salary, say whether it is gross or net.
- Never delete a past entry. Correct it with a dated line saying what changed.
- Only facts the user stated about their own money, not examples or other people's.
",
    ),
    (
        "Work",
        "---
topic: Work
folder: Work
listens-for: [project, client, meeting, deadline, decision, colleague, release, job, contract]
apply: auto
sync: true
model: haiku
enabled: true
---
Keep one note per project or job (`Work/<Project>.md`): what it is, who is involved, its
decisions with their dates and reasons, deadlines, and open questions.

- A decision says what was decided, when, and why, in one or two lines.
- Link people and projects with `[[links]]` when they have notes.
- Not the code itself: what was decided about it.
",
    ),
    (
        "Study",
        "---
topic: Study
folder: Study
listens-for: [course, exam, lecture, assignment, grade, thesis, university, school, homework]
apply: auto
sync: true
model: haiku
enabled: true
---
Keep one note per course (`Study/<Course>.md`): dates of exams and deadlines, grades, what
was hard, and summaries of what was learned.

- Exam dates and deadlines go in a `## Dates` list, soonest first.
- Grades keep their scale (30/30, A, 4.0).
",
    ),
    (
        "Languages",
        "---
topic: Languages
folder: Languages
listens-for: [word, grammar, translation, vocabulary, verb, phrase, pronunciation, lesson]
apply: auto
sync: true
model: haiku
enabled: true
---
Keep one note per language (`Languages/<Language>.md`) with new words and phrases (with their
meaning and an example), grammar points, and mistakes the user made with the correct form.

- New words go in a table: word, meaning, example, date.
- A mistake is written as: what was said, what is correct, why.
",
    ),
    (
        "Life",
        "---
topic: Life
folder: Life
listens-for: [health, doctor, home, apartment, travel, trip, family, friend, plan, appointment, event]
apply: ask
sync: false
model: haiku
enabled: true
---
Keep notes by subject (`Life/Health.md`, `Life/Home.md`, `Life/Travel.md`…): appointments,
plans, things to remember, and what was decided.

- Dates are written in full (2026-03-14), never \"next Tuesday\".
- Plans and appointments go in a `## Coming up` list, soonest first.
",
    ),
    (
        "System",
        "---
topic: System
folder: System
listens-for: [install, package, config, settings, driver, kernel, fix, error, setup, shortcut]
apply: auto
sync: true
model: haiku
enabled: true
---
Keep one note per machine or tool (`System/<Name>.md`): what was installed or changed, the
config that was set and where, and problems with their fixes.

- A fix is written as: the problem, its cause, the fix, the date.
- Paths and commands go in code blocks, exactly as they were.
",
    ),
];

const TEMPLATE: &str = "---
topic: {name}
folder: {name}
listens-for: []
apply: ask
sync: false
model: haiku
enabled: true
---
How this topic's notes are kept: which notes, what goes in them, and any rules.
";

/// The default topics, into the vault's `Topics/` (never over a file that is there).
pub fn write_defaults(vault: &Path) -> std::io::Result<()> {
    let dir = vault.join(vault::TOPICS);
    std::fs::create_dir_all(&dir)?;
    for (name, text) in DEFAULTS {
        let file = dir.join(format!("{name}.md"));
        if !file.exists() {
            std::fs::write(file, text)?;
        }
    }
    Ok(())
}

/// The vault's topics, by name.
pub fn list(vault: &Path) -> Vec<Topic> {
    let Ok(entries) = std::fs::read_dir(vault.join(vault::TOPICS)) else { return Vec::new() };
    let mut topics: Vec<Topic> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")))
        .filter_map(|p| std::fs::read_to_string(&p).ok().map(|text| parse(&p, &text)))
        .collect();
    topics.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    topics
}

fn parse(path: &Path, text: &str) -> Topic {
    let (keys, body) = vault::frontmatter(text);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let text_of = |key: &str| vault::get(&keys, key).map(Value::text).filter(|t| !t.is_empty());
    let yes = |key: &str, default: bool| text_of(key).map_or(default, |t| !matches!(t.to_lowercase().as_str(), "false" | "no" | "off"));
    let name = text_of("topic").unwrap_or_else(|| stem.clone());
    Topic {
        folder: text_of("folder").unwrap_or_else(|| name.clone()),
        listens_for: vault::get(&keys, "listens-for").map(Value::items).unwrap_or_default(),
        apply: if text_of("apply").is_some_and(|a| a.eq_ignore_ascii_case("auto")) { Apply::Auto } else { Apply::Ask },
        sync: yes("sync", false),
        model: text_of("model").unwrap_or_else(|| "haiku".to_owned()),
        enabled: yes("enabled", true),
        instructions: body.trim().to_owned(),
        path: path.to_owned(),
        name,
    }
}

/// Switches a topic on or off (its `enabled:` line, added if missing).
pub fn set_enabled(topic: &Topic, on: bool) -> std::io::Result<()> {
    let text = std::fs::read_to_string(&topic.path)?;
    let line = format!("enabled: {on}");
    let changed = match text.strip_prefix("---\n").and_then(|rest| rest.find("\n---").map(|end| (rest, end))) {
        Some((rest, end)) => {
            let front: Vec<&str> = rest[..end].lines().collect();
            let mut lines: Vec<String> = front.iter().filter(|l| !l.trim_start().starts_with("enabled:")).map(|l| (*l).to_owned()).collect();
            lines.push(line);
            format!("---\n{}{}", lines.join("\n"), &rest[end..])
        }
        None => format!("---\n{line}\n---\n{text}"),
    };
    std::fs::write(&topic.path, changed)
}

/// A new topic from the template; its file (to open). Err if the name is unusable or taken.
pub fn create(vault: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\', ':']) || name.starts_with('.') {
        return Err("A name with no / \\ : and not starting with a dot.".to_owned());
    }
    let dir = vault.join(vault::TOPICS);
    let file = dir.join(format!("{name}.md"));
    if file.exists() {
        return Err(format!("There is a topic named {name} already."));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(&file, TEMPLATE.replace("{name}", name)).map_err(|e| e.to_string())?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_read_back() {
        for (name, text) in DEFAULTS {
            let topic = parse(Path::new(&format!("{name}.md")), text);
            assert_eq!(&topic.name, name);
            assert!(!topic.listens_for.is_empty(), "{name}");
            assert!(topic.enabled);
            assert!(!topic.instructions.is_empty(), "{name}");
        }
        let finances = parse(Path::new("Finances.md"), DEFAULTS[0].1);
        assert_eq!(finances.apply, Apply::Ask);
        assert!(!finances.sync);
    }

    #[test]
    fn a_topic_is_switched_off_and_on() {
        let root = std::env::temp_dir().join(format!("loom-topics-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let file = create(&root, "Surfing").unwrap();
        assert!(create(&root, "Surfing").is_err());
        let topic = list(&root).remove(0);
        assert!(topic.enabled);
        set_enabled(&topic, false).unwrap();
        let off = list(&root).remove(0);
        assert!(!off.enabled);
        assert_eq!(off.name, "Surfing");
        set_enabled(&off, true).unwrap();
        assert!(list(&root)[0].enabled);
        assert!(std::fs::read_to_string(file).unwrap().contains("How this topic"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
