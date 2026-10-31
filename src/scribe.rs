//! Scribe: loom's note agent. It reads what was said in a conversation (the journal), finds
//! which topics it touches, and records what the user stated in the vault, as small changes
//! (a line added under a heading, one line replaced, a property set, a new note) that loom
//! checks and applies itself, each a git commit. It never rewrites a note.
//!
//! A check (`check`) runs off the main thread: Claude Haiku through `claude -p`, without the
//! user's settings, hooks, tools or MCP servers, and without saving a session. Topics that
//! ask first get their changes held for review (`pending`); the others are applied at once.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::journal::Entry;
use crate::topics::{Apply, Topic};

/// The vault's index, kept between checks.
static INDEX: std::sync::Mutex<Option<crate::notes::Index>> = std::sync::Mutex::new(None);

/// An excerpt sent at once: about this many characters of conversation.
/// The model and effort Scribe's calls use (Settings → Notes): set on the main thread, read
/// by the calls running off it.
static CHOICE: std::sync::Mutex<(String, String)> = std::sync::Mutex::new((String::new(), String::new()));

pub fn choose(model: &str, effort: &str) {
    if let Ok(mut choice) = CHOICE.lock() {
        *choice = (model.trim().to_owned(), effort.trim().to_owned());
    }
}

const CHUNK: usize = 6000;

/// How many notes an edit is shown (the best matches), and how much of each.
const CANDIDATES: usize = 5;
const NOTE_MOST: usize = 8000;

const ROUTE: &str = "You sort excerpts of a person's conversations with their AI agents into the topics of their personal notes.

You get the topics (name: what it covers) and an excerpt. Return the topics for which the USER stated a lasting fact about their own life, work or setup that is worth keeping in notes: an amount, a date, a decision, a change, a plan, something they learned or fixed.

Worth keeping too: a word, phrase or grammar point the user is learning (even when it came as a question to the agent), a change to their own computer's setup.

Not worth keeping: questions without an answer, examples and hypotheticals, the code itself and its bugs and errors (even once fixed: that is the project's history; but a decision, a milestone or a plan for one of the user's projects is worth keeping, under Work), the agent's suggestions the user did not confirm, small talk, facts about other people that don't concern the user.

Most excerpts touch no topic: then return an empty list. Never assume the user's gender. For each topic you return, give `about`: 3 to 8 words that would find the related notes.";

/// Turns a correction into a rule (its own prompt, editable in loom's Claude folder).
const RULE: &str = "You help keep one topic of a person's notes. A notes keeper proposed a change to their notes and they said no to it (or undid it), and told you why.

Write ONE rule for this topic's instructions that would have kept the keeper from making that change, and that still makes sense for later conversations: one short imperative sentence, under 25 words, in the person's own terms. If what they said is already a rule, keep their words. Don't repeat the particular amounts or names of this change unless the rule is about them. Don't write anything else.

Answer as JSON: {\"rule\": \"...\"}";

/// Turns an answer to Scribe's question into a rule, when it holds beyond this one case.
const RULE_ANSWER: &str = "You help keep one topic of a person's notes. While recording something from one of their conversations, the notes keeper asked them a question, and they answered.

If the answer says how this kind of thing should be recorded from now on (a default currency, gross or net, which note it goes in, what to leave out), write ONE rule for this topic's instructions so the keeper never needs to ask it again: one short imperative sentence, under 25 words, in the person's own terms. If the answer only settles this one case (a particular amount, date or name), or the topic's instructions already say it, return an empty rule. Don't write anything else.

Answer as JSON: {\"rule\": \"...\"}";

/// Drafts a topic's file from the user's description of it.
const DESCRIBE: &str = "You set up one topic of a person's personal notes (an Obsidian vault of Markdown files). A notes keeper will follow it to record what the person says in their conversations with their AI agents.

You get the topic's name, the person's description of what it should keep, and an example topic. Return:
- `folder`: the vault folder its notes go in (usually the topic's name; no slashes).
- `listens_for`: 6 to 14 short words or phrases that would show a conversation is about it.
- `apply`: \"ask\" if its notes are private or costly to get wrong (money, health, people), else \"auto\".
- `instructions`: in plain words, like the example's text below its settings: which notes to keep (their names, one per what), what goes in each, and a few rules (\"- ...\" lines). Only what the description asks for; keep it short. Never assume the person's gender.";

const EDIT: &str = "You keep one topic of a person's notes (an Obsidian vault of Markdown files).

You get the topic's own instructions (how its notes are kept: follow them), the notes that may need the change (path and full text; maybe none), and an excerpt of a conversation. Return the small changes that record what the USER stated or confirmed.

Rules:
- Only what the user stated or confirmed. Never invent anything, never guess a number or a date.
- Small changes only. Never rewrite, reorder or reformat a note; never delete anything.
- `add`: lines added at the end of the section under `under`, a heading line exactly as written in the note (\"## Expenses\"). If the note has no such heading, it is added at the note's end. `text` matches the note's style (a table row in a table, a bullet in a list).
- `replace`: `old` is one whole line exactly as it is in the note, `new` the line that takes its place. To correct a value; if the topic says history matters, add a line instead.
- `create`: a new note, `path` inside the topic's folder ending in .md, `text` its whole content, starting with frontmatter (date: YYYY-MM-DD and tags). Only when no note fits.
- `property`: sets `key` to `value` in a note's frontmatter.
- The CONVERSATION line says which of the user's projects this is: a project's note is named after that project (its folder's name, or the name the user gives it). Your own working directory is not the user's: never name anything after it.
- `path` is always the note's path in the vault, as given (\"Finances/Overview.md\").
- Each change has a short `reason`: what the user said that it records. Write it without pronouns for the user (never he or she): \"Phone plan rises to 15 EUR from November, as stated\".
- If the excerpt has nothing for this topic after all, return no changes.
- When the user doesn't say when something happened, date it TODAY (when it was said); don't ask.
- A date with no year is the next such date after TODAY (for something to come) or the last one before it (for something that happened): write it in full, don't ask.
- If something that matters is unclear (no currency, gross or net) return a question instead of guessing, with 2 to 4 short options. Keep questions rare.";

fn route_schema() -> Value {
    json!({ "type": "object", "properties": { "topics": { "type": "array", "items": { "type": "object", "properties": {
        "topic": { "type": "string" }, "about": { "type": "string" } }, "required": ["topic", "about"] } } }, "required": ["topics"] })
}

fn edit_schema() -> Value {
    json!({ "type": "object", "properties": {
        "changes": { "type": "array", "items": { "type": "object", "properties": {
            "op": { "type": "string", "enum": ["add", "replace", "create", "property"] },
            "path": { "type": "string", "description": "The note to change, as a path in the vault: Finances/Overview.md" }, "under": { "type": "string" }, "text": { "type": "string" },
            "old": { "type": "string" }, "new": { "type": "string" }, "key": { "type": "string" }, "value": { "type": "string" },
            "reason": { "type": "string" } }, "required": ["op", "path", "reason"] } },
        "questions": { "type": "array", "items": { "type": "object", "properties": {
            "question": { "type": "string" }, "options": { "type": "array", "items": { "type": "string" } } }, "required": ["question", "options"] } }
    }, "required": ["changes", "questions"] })
}

/// One change, as Scribe proposed it.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Add { note: String, under: String, text: String },
    Replace { note: String, old: String, new: String },
    Create { note: String, text: String },
    Property { note: String, key: String, value: String },
}

impl Change {
    fn from_json(v: &Value) -> Option<Change> {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_owned();
        let note = v["path"].as_str().or(v["note"].as_str()).unwrap_or("").trim().trim_start_matches('/').to_owned();
        Some(match v["op"].as_str()? {
            "add" => Change::Add { note, under: s("under").trim().to_owned(), text: s("text") },
            "replace" => Change::Replace { note, old: s("old"), new: s("new") },
            "create" => Change::Create { note, text: s("text") },
            "property" => Change::Property { note, key: s("key").trim().to_owned(), value: s("value") },
            _ => return None,
        })
    }

    pub fn to_json(&self) -> Value {
        match self {
            Change::Add { note, under, text } => json!({ "op": "add", "note": note, "under": under, "text": text }),
            Change::Replace { note, old, new } => json!({ "op": "replace", "note": note, "old": old, "new": new }),
            Change::Create { note, text } => json!({ "op": "create", "note": note, "text": text }),
            Change::Property { note, key, value } => json!({ "op": "property", "note": note, "key": key, "value": value }),
        }
    }

    pub fn note(&self) -> &str {
        match self {
            Change::Add { note, .. } | Change::Replace { note, .. } | Change::Create { note, .. } | Change::Property { note, .. } => note,
        }
    }

    /// What it does, in a few words: "added 2 lines", "set status".
    pub fn summary(&self) -> String {
        match self {
            Change::Add { text, .. } => match text.lines().count() {
                1 => "added a line".to_owned(),
                n => format!("added {n} lines"),
            },
            Change::Replace { .. } => "changed a line".to_owned(),
            Change::Create { .. } => "new note".to_owned(),
            Change::Property { key, .. } => format!("set {key}"),
        }
    }

    /// The change as a diff to read: + lines added, - lines gone.
    pub fn diff(&self) -> String {
        let plus = |text: &str| text.lines().map(|l| format!("+ {l}")).collect::<Vec<_>>().join("\n");
        match self {
            Change::Add { under, text, .. } if under.is_empty() => plus(text),
            Change::Add { under, text, .. } => format!("  {under}\n{}", plus(text)),
            Change::Replace { old, new, .. } => format!("- {old}\n+ {new}"),
            Change::Create { text, .. } => format!("(new note)\n{}", plus(text)),
            Change::Property { key, value, .. } => format!("+ {key}: {value}"),
        }
    }
}

/// A note as it was before a change (None: there was no such note) and as the change left it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Written {
    pub before: Option<String>,
    pub after: String,
}

/// A change and why, for a topic, from a conversation.
#[derive(Clone, Debug)]
pub struct Proposal {
    pub topic: String,
    pub change: Change,
    pub reason: String,
    /// The piece of the conversation it came from.
    pub excerpt: String,
}

/// A question Scribe asks instead of guessing.
#[derive(Clone, Debug)]
pub struct Question {
    pub topic: String,
    pub question: String,
    pub options: Vec<String>,
    /// What was said that raised it (the answer is recorded with it).
    pub excerpt: String,
}

/// What a check did.
#[derive(Debug, Default)]
pub struct Report {
    pub session: String,
    pub title: String,
    /// Changes written (and committed), with the note before and after.
    pub applied: Vec<(Proposal, Written)>,
    /// Changes waiting for the user's OK (an `ask` topic, or one that no longer fitted).
    pub held: Vec<(Proposal, String)>,
    pub questions: Vec<Question>,
    /// Calls to Claude made.
    pub calls: usize,
    /// The topics the conversation was found to touch.
    pub routed: Vec<String>,
    /// Why it stopped early, if it did.
    pub failed: Option<String>,
    /// The journal is read up to here now.
    pub upto: u64,
}

/// Reads `entries` (one conversation's unread part) and records what they say. `calls_left`
/// is the day's remaining budget of Claude calls.
pub fn check(vault: &Path, session: &str, entries: &[Entry], calls_left: usize) -> Report {
    let mut report = Report { session: session.to_owned(), title: entries.last().map(|e| e.title.clone()).unwrap_or_default(), ..Report::default() };
    let topics: Vec<Topic> = crate::topics::list(vault).into_iter().filter(|t| t.enabled).collect();
    if topics.is_empty() {
        report.upto = entries.last().map_or(0, |e| e.end);
        return report;
    }
    // One index, kept between checks (only notes that changed are read again).
    let mut held = INDEX.lock().unwrap_or_else(|e| e.into_inner());
    if held.as_ref().is_none_or(|index| index.root() != vault) {
        *held = Some(crate::notes::Index::new(vault));
    }
    let Some(index) = held.as_mut() else { return report };
    for chunk in pieces(session, entries) {
        // A piece is done topic by topic: a check that stopped halfway (a call failed, the
        // day's limit) does the topics it had not done, and only those, the next time.
        let id = format!("{session}@{}-{}", chunk[0].offset, chunk.last().map_or(0, |e| e.end));
        if crate::journal::is_done(session, &format!("{id}/routed")) {
            let all_done = crate::journal::done_with(session, &id);
            if all_done {
                report.upto = chunk.last().map_or(report.upto, |e| e.end);
                continue;
            }
        }
        if report.calls >= calls_left {
            report.failed = Some("Today's limit of note checks is reached (Settings → Notes).".to_owned());
            return report;
        }
        let excerpt = excerpt(chunk);
        // A resumed piece is routed already (no call).
        let routed = match crate::journal::todo(session, &id) {
            Some(routed) => routed,
            None => {
                report.calls += 1;
                match route(&topics, &excerpt) {
                    Ok(routed) => routed,
                    Err(why) => {
                        report.failed = Some(why);
                        return report;
                    }
                }
            }
        };
        report.routed.extend(routed.iter().map(|(name, _)| name.split([':', '(']).next().unwrap_or(name).trim().to_owned()));
        // "Work", or as the model sometimes writes it, "Work: job" / "Work (decisions)".
        let routed: Vec<(&Topic, String)> = routed
            .into_iter()
            .filter_map(|(name, about)| {
                let bare = name.split([':', '(']).next().unwrap_or(&name).trim().to_owned();
                Some((topics.iter().find(|t| t.name.eq_ignore_ascii_case(&bare))?, about))
            })
            .collect();
        // What this piece is about, kept: a check that resumes it does not route it again.
        crate::journal::set_todo(session, &id, &routed.iter().map(|(t, about)| (t.name.clone(), about.clone())).collect::<Vec<_>>());
        for (topic, about) in routed {
            let topic_id = format!("{id}/{}", topic.name);
            if crate::journal::is_done(session, &topic_id) {
                continue;
            }
            if report.calls >= calls_left {
                report.failed = Some("Today's limit of note checks is reached (Settings → Notes).".to_owned());
                return report;
            }
            index.refresh();
            let notes = candidates(vault, index, topic, &about);
            let (proposals, questions) = match edit(topic, &notes, &excerpt) {
                Ok(found) => found,
                Err(why) => {
                    report.failed = Some(why);
                    return report;
                }
            };
            report.calls += 1;
            report.questions.extend(questions);
            record(vault, topic, proposals, &topic_id, &mut report);
            crate::journal::mark_done(session, &topic_id);
        }
        report.upto = chunk.last().map_or(report.upto, |e| e.end);
    }
    report
}

/// Proposed changes made (their topic changes directly) or held (it asks first, or a change
/// does not fit); what was written is one commit.
fn record(vault: &Path, topic: &Topic, proposals: Vec<Proposal>, id: &str, report: &mut Report) {
    record_as(vault, topic, proposals, id, report, false);
}

/// `asked`: the user asked for these changes themselves (written even where the topic asks
/// first).
fn record_as(vault: &Path, topic: &Topic, proposals: Vec<Proposal>, id: &str, report: &mut Report, asked: bool) {
    let mut written = Vec::new();
    let mut reasons = Vec::new();
    for proposal in proposals {
        if let Err(why) = allowed(vault, topic, &proposal.change) {
            report.held.push((proposal, why));
            continue;
        }
        if topic.apply == Apply::Ask && !asked {
            report.held.push((proposal, "This topic asks before changing.".to_owned()));
            continue;
        }
        match apply(vault, &proposal.change) {
            Ok(done) => {
                written.push(proposal.change.note().to_owned());
                reasons.push(proposal.reason.clone());
                report.applied.push((proposal, done));
            }
            Err(why) => report.held.push((proposal, why)),
        }
    }
    if !written.is_empty() {
        written.dedup();
        commit(vault, &written, &format!("loom: {}: {} [{id}]", topic.name, reasons.join("; ")));
    }
}

/// The user answered one of Scribe's questions: the change it was unsure of, made now.
pub fn answer(vault: &Path, question: &Question, answer: &str) -> Report {
    let mut report = Report::default();
    let Some(topic) = crate::topics::list(vault).into_iter().find(|t| t.name == question.topic) else {
        report.failed = Some(format!("There is no topic {} any more.", question.topic));
        return report;
    };
    let mut index = crate::notes::Index::new(vault);
    index.refresh();
    let notes = candidates(vault, &index, &topic, &question.question);
    let excerpt = format!("{}\n\nUSER (answering the notes keeper's question \"{}\"): {answer}", question.excerpt, question.question);
    match edit(&topic, &notes, &excerpt) {
        Ok((proposals, _)) => {
            report.calls = 1;
            let id = format!("answer@{}", crate::state::now());
            record(vault, &topic, proposals, &id, &mut report);
        }
        Err(why) => report.failed = Some(why),
    }
    report
}

/// The user's comment on a change Scribe wrote ("it's net, not gross"): the note changed as
/// they say, at once (they asked for it). One call to Claude.
pub fn fix(vault: &Path, done: &Proposal, comment: &str) -> Report {
    let mut report = Report::default();
    let Some(topic) = crate::topics::list(vault).into_iter().find(|t| t.name == done.topic) else {
        report.failed = Some(format!("There is no topic {} any more.", done.topic));
        return report;
    };
    let note = done.change.note().to_owned();
    let notes: Vec<(String, String)> = std::fs::read_to_string(vault.join(&note)).ok().map(|text| vec![(note.clone(), text)]).unwrap_or_default();
    let excerpt = format!(
        "{}\n\nTHE NOTES KEEPER MADE THIS CHANGE TO {note}:\n{}\n\nUSER (about that change, telling the notes keeper what to change in the note; do what they say, and nothing else): {comment}",
        done.excerpt,
        done.change.diff()
    );
    // Made, answered or not.
    report.calls = 1;
    match edit(&topic, &notes, &excerpt) {
        Ok((proposals, _)) => {
            let proposals = proposals.into_iter().map(|p| Proposal { reason: format!("You asked: {comment}"), excerpt: done.excerpt.clone(), ..p }).collect();
            let id = format!("fix@{}", crate::state::now());
            record_as(vault, &topic, proposals, &id, &mut report, true);
        }
        Err(why) => report.failed = Some(why),
    }
    report
}

/// The entries in pieces of about `CHUNK` characters (a message is never split).
/// The pieces to read: a piece a check stopped halfway in comes first, as it was then (more
/// said since is a piece of its own), so it is resumed rather than read again differently.
fn pieces<'a>(session: &str, entries: &'a [Entry]) -> Vec<&'a [Entry]> {
    if let Some(first) = entries.first()
        && let Some(end) = crate::journal::routed_end(session, &format!("{session}@{}-", first.offset))
    {
        let upto = entries.iter().position(|e| e.end > end).unwrap_or(entries.len());
        if upto > 0 {
            let mut out = vec![&entries[..upto]];
            out.extend(chunks(&entries[upto..]));
            return out;
        }
    }
    chunks(entries)
}

fn chunks(entries: &[Entry]) -> Vec<&[Entry]> {
    let mut out = Vec::new();
    let (mut start, mut size) = (0, 0);
    for (i, entry) in entries.iter().enumerate() {
        if size > 0 && size + entry.text.len() > CHUNK {
            out.push(&entries[start..i]);
            (start, size) = (i, 0);
        }
        size += entry.text.len();
    }
    if start < entries.len() {
        out.push(&entries[start..]);
    }
    out
}

fn excerpt(chunk: &[Entry]) -> String {
    // Which conversation, and which of the user's projects it is in: what notes are named
    // after (not the folder Scribe itself runs in).
    let last = chunk.last();
    let project = last.map(|e| e.cwd.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_owned()).unwrap_or_default();
    let title = last.map(|e| e.title.clone()).unwrap_or_default();
    let head = match (title.is_empty(), project.is_empty()) {
        (false, false) => format!("CONVERSATION: \"{title}\", in the user's project folder \"{project}\"\n\n"),
        (false, true) => format!("CONVERSATION: \"{title}\"\n\n"),
        (true, false) => format!("CONVERSATION: in the user's project folder \"{project}\"\n\n"),
        (true, true) => String::new(),
    };
    let body = chunk
        .iter()
        .map(|e| {
            let who = if e.role == "user" { "USER" } else { "AGENT" };
            // A long reply matters less than what was asked: its start and end.
            let text = if e.role != "user" && e.text.chars().count() > 1500 {
                let all: Vec<char> = e.text.chars().collect();
                format!("{} […] {}", all[..700].iter().collect::<String>(), all[all.len() - 700..].iter().collect::<String>())
            } else {
                e.text.clone()
            };
            format!("{who}: {text}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{head}{body}")
}

fn route(topics: &[Topic], excerpt: &str) -> Result<Vec<(String, String)>, String> {
    let list = topics
        .iter()
        .map(|t| format!("- {}: {}", t.name, if t.listens_for.is_empty() { t.instructions.lines().next().unwrap_or("").to_owned() } else { t.listens_for.join(", ") }))
        .collect::<Vec<_>>()
        .join("\n");
    let input = format!("TOPICS:\n{list}\n\nEXCERPT:\n{excerpt}");
    let answer = ask(&prompt("route", ROUTE), &input, &route_schema(), "")?;
    Ok(answer["topics"]
        .as_array()
        .map(|all| all.iter().filter_map(|t| Some((t["topic"].as_str()?.to_owned(), t["about"].as_str().unwrap_or("").to_owned()))).collect())
        .unwrap_or_default())
}

/// The notes an edit for `topic` is shown: the best matches in its folder for what the
/// excerpt is about, and its overview note if it has one. (path, text)
fn candidates(vault: &Path, index: &crate::notes::Index, topic: &Topic, about: &str) -> Vec<(String, String)> {
    let mut paths: Vec<String> = index.search(about, Some(&topic.folder), CANDIDATES).into_iter().map(|h| h.path).collect();
    for name in ["Overview", "Index", &topic.name] {
        if let Some(path) = index.resolve(&format!("{}/{name}", topic.folder)) {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths.truncate(CANDIDATES + 1);
    paths
        .into_iter()
        .filter_map(|p| {
            let text = std::fs::read_to_string(vault.join(&p)).ok()?;
            let text = if text.len() > NOTE_MOST { format!("{}\n[… the rest of the note is not shown]", &text[..text.floor_char_boundary(NOTE_MOST)]) } else { text };
            Some((p, text))
        })
        .collect()
}

fn edit(topic: &Topic, notes: &[(String, String)], excerpt: &str) -> Result<(Vec<Proposal>, Vec<Question>), String> {
    let shown = if notes.is_empty() {
        "(none yet)".to_owned()
    } else {
        notes.iter().map(|(path, text)| format!("=== {path} ===\n{text}")).collect::<Vec<_>>().join("\n\n")
    };
    let today = gtk::glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%d").ok()).map(|d| d.to_string()).unwrap_or_default();
    let input = format!(
        "TODAY: {today}\n\nTOPIC: {} (its notes go in the folder {}/)\nITS INSTRUCTIONS:\n{}\n\nNOTES:\n{shown}\n\nEXCERPT:\n{excerpt}",
        topic.name, topic.folder, topic.instructions
    );
    let answer = ask(&prompt("edit", EDIT), &input, &edit_schema(), &topic.model)?;
    let proposals = answer["changes"]
        .as_array()
        .map(|all| {
            all.iter()
                .filter_map(|c| Some(Proposal { topic: topic.name.clone(), reason: c["reason"].as_str().unwrap_or("").to_owned(), change: Change::from_json(c)?, excerpt: excerpt.to_owned() }))
                .collect()
        })
        .unwrap_or_default();
    let questions = answer["questions"]
        .as_array()
        .map(|all| {
            all.iter()
                .filter_map(|q| {
                    Some(Question {
                        topic: topic.name.clone(),
                        question: q["question"].as_str()?.to_owned(),
                        options: q["options"].as_array().map(|o| o.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
                        excerpt: excerpt.to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok((proposals, questions))
}

/// Scribe's prompt `name`: the user's copy in loom's Claude folder if they have one, else
/// the built-in one.
fn prompt(name: &str, built_in: &str) -> String {
    let file = crate::plugin::dir().join("scribe").join(format!("{name}.md"));
    std::fs::read_to_string(file).ok().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| built_in.to_owned())
}

/// The built-in prompts, for loom's Claude folder (name, text).
pub fn prompts() -> [(&'static str, &'static str); 5] {
    [("route", ROUTE), ("edit", EDIT), ("rule", RULE), ("rule-answer", RULE_ANSWER), ("describe", DESCRIBE)]
}

/// One call to Claude (`claude -p`), its answer as the schema's JSON. `model`: a topic's own
/// (empty: the one chosen in Settings → Notes, else Haiku).
fn ask(system: &str, input: &str, schema: &Value, model: &str) -> Result<Value, String> {
    use std::io::Write;
    // An empty folder of its own: nothing in it to be taken for the user's project.
    let dir = std::env::temp_dir().join(format!("loom-notes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (chosen, effort) = CHOICE.lock().map(|c| c.clone()).unwrap_or_default();
    let model = [model.trim(), chosen.as_str()].into_iter().find(|m| !m.is_empty()).unwrap_or("haiku");
    let effort: &[&str] = if effort.is_empty() { &[] } else { &["--effort", effort.as_str()] };
    let mut child = Command::new("timeout")
        .args(["180", "claude", "-p", "--model", model, "--setting-sources", "project", "--tools", "", "--strict-mcp-config", "--no-session-persistence"])
        .args(effort)
        .args(["--output-format", "json", "--json-schema", &schema.to_string(), "--system-prompt", system])
        .current_dir(&dir)
        .env_remove("CLAUDECODE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run claude: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(err) = stdin.write_all(input.as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Could not talk to claude: {err}"));
        }
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let answer: Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        let why = String::from_utf8_lossy(&output.stderr);
        format!("claude gave no answer ({}): {}", output.status, why.lines().last().unwrap_or("").trim())
    })?;
    if answer["is_error"].as_bool() == Some(true) {
        return Err(format!("claude: {}", answer["result"].as_str().unwrap_or("an error")));
    }
    let structured = answer["structured_output"].clone();
    if structured.is_object() {
        return Ok(structured);
    }
    answer["result"].as_str().and_then(|r| serde_json::from_str(r).ok()).ok_or_else(|| "claude's answer was not in the expected form".to_owned())
}

/// Whether a change may be made: a Markdown note inside the vault, in the topic's folder,
/// that exists (or, to be created, does not).
fn allowed(vault: &Path, topic: &Topic, change: &Change) -> Result<(), String> {
    let note = change.note();
    let path = Path::new(note);
    let inside = !note.is_empty()
        && path.is_relative()
        && path.components().all(|c| matches!(c, std::path::Component::Normal(_)))
        && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md"))
        && !note.starts_with('.')
        && !path.starts_with(crate::vault::TOPICS);
    if !inside {
        return Err(format!("{note} is not a note loom may write."));
    }
    let folder = topic.folder.trim_matches('/');
    if !path.starts_with(folder) {
        return Err(format!("{note} is outside the {} folder ({folder}/).", topic.name));
    }
    let exists = vault.join(path).is_file();
    match change {
        // A note made meanwhile (by another change): what was to be in it goes into it.
        Change::Create { .. } => Ok(()),
        _ if !exists => Err(format!("{note} does not exist.")),
        _ => Ok(()),
    }
}

/// Makes a change in the vault (after `allowed`): the note before and after, to undo it.
pub fn apply(vault: &Path, change: &Change) -> Result<Written, String> {
    let path: PathBuf = vault.join(change.note());
    if let Change::Create { text, .. } = change {
        if path.exists() {
            let old = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", change.note()))?;
            let after = merge(&old, text);
            std::fs::write(&path, &after).map_err(|e| e.to_string())?;
            return Ok(Written { before: Some(old), after });
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let after = ensure_newline(text);
        std::fs::write(&path, &after).map_err(|e| e.to_string())?;
        return Ok(Written { before: None, after });
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", change.note()))?;
    let changed = match change {
        Change::Add { under, text: added, .. } => add_under(&text, under, added),
        Change::Replace { old, new, .. } => replace_line(&text, old, new)?,
        Change::Property { key, value, .. } => set_property(&text, key, value)?,
        Change::Create { .. } => unreachable!(),
    };
    std::fs::write(&path, &changed).map_err(|e| e.to_string())?;
    Ok(Written { before: Some(text), after: changed })
}

/// Undoes a written change. A note nothing else touched since goes back as it was (a note
/// the change made goes to the trash); one changed since loses only the change's own lines,
/// if they are still there as written. Committed, like the change was.
pub fn undo(vault: &Path, note: &str, written: &Written, what: &str) -> Result<(), String> {
    let path = vault.join(note);
    let now = std::fs::read_to_string(&path).ok();
    let back = match now {
        Some(now) if now == written.after => written.before.clone(),
        Some(now) => match &written.before {
            Some(before) => Some(take_out(&now, before, &written.after).ok_or("The note changed since, around these lines: open it to change it yourself.")?),
            None => return Err("The note changed since it was made: open it to remove it yourself.".to_owned()),
        },
        None => return Err("The note is not there any more.".to_owned()),
    };
    match back {
        Some(text) => std::fs::write(&path, text).map_err(|e| e.to_string())?,
        None => {
            use gtk::gio::prelude::FileExt;
            gtk::gio::File::for_path(&path).trash(None::<&gtk::gio::Cancellable>).map_err(|e| e.to_string())?
        }
    }
    commit(vault, &[note.to_owned()], &format!("loom: undo: {what}"));
    Ok(())
}

/// `now` with what the change put in (the lines between what `before` and `after` share at
/// their start and end) back as it was. None if that part is not in `now` just once.
fn take_out(now: &str, before: &str, after: &str) -> Option<String> {
    let (b, a): (Vec<&str>, Vec<&str>) = (before.split_inclusive('\n').collect(), after.split_inclusive('\n').collect());
    let start = b.iter().zip(&a).take_while(|(x, y)| x == y).count();
    let end = b[start..].iter().rev().zip(a[start..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (old, new) = (b[start..b.len() - end].concat(), a[start..a.len() - end].concat());
    if new.is_empty() {
        return None;
    }
    // With the line before it, when there is one: the same line may well be elsewhere too.
    let lead = if start > 0 { a[start - 1] } else { "" };
    let (find, keep) = (format!("{lead}{new}"), lead.len());
    let at = match now.match_indices(&find).count() {
        1 => now.find(&find)? + keep,
        _ if now.matches(new.as_str()).count() == 1 => now.find(&new)?,
        _ => return None,
    };
    Some(format!("{}{old}{}", &now[..at], &now[at + new.len()..]))
}

fn ensure_newline(text: &str) -> String {
    if text.ends_with('\n') { text.to_owned() } else { format!("{text}\n") }
}

/// A new note's text put into a note that exists: each section's lines under the same heading
/// (or one whose name holds it: "## Changes" → "## Setup and changes"), lines the note has
/// already left out. Its frontmatter and title are the note's own already.
fn merge(old: &str, new: &str) -> String {
    let body = match new.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---\n")) {
        Some((_, body)) => body,
        None => new,
    };
    let level = |line: &str| line.chars().take_while(|c| *c == '#').count();
    let mut sections: Vec<(String, Vec<&str>)> = vec![(String::new(), Vec::new())];
    for line in body.lines() {
        match level(line) {
            1 => {}
            n if n > 1 && line[n..].starts_with(' ') => sections.push((line.trim().to_owned(), Vec::new())),
            _ => sections.last_mut().expect("one section at least").1.push(line),
        }
    }
    let mut out = old.to_owned();
    for (heading, lines) in sections {
        let lines: Vec<&str> = lines.into_iter().filter(|l| !l.trim().is_empty() && !out.lines().any(|o| o.trim() == l.trim())).collect();
        if lines.is_empty() {
            continue;
        }
        let name = heading.trim_start_matches('#').trim().to_lowercase();
        let under = out
            .lines()
            .find(|l| l.trim() == heading)
            .or_else(|| (!name.is_empty()).then(|| out.lines().find(|l| level(l) == level(&heading) && l.to_lowercase().contains(&name))).flatten())
            .unwrap_or(&heading)
            .to_owned();
        out = add_under(&out, &under, &lines.join("\n"));
    }
    out
}

/// `added` at the end of the section under the heading `under` (before the next heading of
/// the same or a higher level); the heading made at the end when the note has none.
fn add_under(text: &str, under: &str, added: &str) -> String {
    let added = added.trim_end_matches('\n');
    let lines: Vec<&str> = text.lines().collect();
    let level = |line: &str| line.trim_start().chars().take_while(|c| *c == '#').count();
    let heading = (!under.is_empty()).then(|| lines.iter().position(|l| l.trim() == under.trim())).flatten();
    let Some(at) = heading else {
        let mut out = text.trim_end_matches('\n').to_owned();
        if !under.is_empty() {
            out.push_str(&format!("\n\n{}", under.trim()));
        }
        out.push_str(&format!("\n{added}\n"));
        return out;
    };
    let own = level(lines[at]).max(1);
    let next = lines.iter().enumerate().skip(at + 1).find(|(_, l)| {
        let l2 = level(l);
        l2 > 0 && l2 <= own && l.trim_start()[l2..].starts_with(' ')
    });
    let end = next.map_or(lines.len(), |(i, _)| i);
    // After the section's last line with something on it.
    let insert = (at + 1..end).rev().find(|&i| !lines[i].trim().is_empty()).map_or(at + 1, |i| i + 1);
    let mut out: Vec<String> = lines[..insert].iter().map(|l| (*l).to_owned()).collect();
    out.extend(added.lines().map(str::to_owned));
    out.extend(lines[insert..].iter().map(|l| (*l).to_owned()));
    format!("{}\n", out.join("\n"))
}

fn replace_line(text: &str, old: &str, new: &str) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let matches: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| l.trim() == old.trim()).map(|(i, _)| i).collect();
    match matches.as_slice() {
        [at] => {
            let mut out: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
            out[*at] = new.to_owned();
            Ok(format!("{}\n", out.join("\n")))
        }
        [] => Err("The line to replace is not in the note (any more).".to_owned()),
        _ => Err("The line to replace is in the note more than once.".to_owned()),
    }
}

fn set_property(text: &str, key: &str, value: &str) -> Result<String, String> {
    if key.is_empty() || key.contains([':', '\n', '\r']) || key.starts_with([' ', '-', '#']) {
        return Err(format!("\"{key}\" is not a property name."));
    }
    // One line: a value with a line break in it could slip other keys into the frontmatter.
    if value.contains(['\n', '\r']) {
        return Err("A property's value is one line.".to_owned());
    }
    let line = format!("{key}: {value}");
    Ok(crate::vault::edit_frontmatter(text, |lines| match lines.iter().position(|l| l.split_once(':').is_some_and(|(k, _)| k.trim() == key)) {
        Some(at) => lines[at] = line,
        None => lines.push(line),
    }))
}

/// Commits the notes written, alone (other changes in the vault are the user's to commit).
pub fn commit(vault: &Path, files: &[String], message: &str) {
    let git = |args: &[&str]| Command::new("git").arg("-C").arg(vault).args(args).output();
    let in_repo = git(&["rev-parse", "--is-inside-work-tree"]).is_ok_and(|o| o.status.success());
    if !in_repo {
        return;
    }
    let mut add = vec!["add", "--"];
    add.extend(files.iter().map(String::as_str));
    let _ = git(&add);
    let mut commit = vec!["commit", "-q", "-m", message, "--"];
    commit.extend(files.iter().map(String::as_str));
    if let Ok(out) = git(&commit) {
        if !out.status.success() {
            crate::log::write("scribe", &format!("commit: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
    }
}

/// What was proposed, kept to be reviewed (the user's OK) across restarts.
pub fn proposal_json(p: &Proposal, why: &str, session: &str, title: &str) -> Value {
    json!({ "topic": p.topic, "reason": p.reason, "change": p.change.to_json(), "why": why, "session": session, "title": title, "at": crate::state::now(), "excerpt": p.excerpt })
}

pub fn proposal_from(v: &Value) -> Option<Proposal> {
    Some(Proposal {
        topic: v["topic"].as_str()?.to_owned(),
        reason: v["reason"].as_str().unwrap_or("").to_owned(),
        change: Change::from_json(&v["change"])?,
        excerpt: v["excerpt"].as_str().unwrap_or("").to_owned(),
    })
}

/// An approved held change: checked again (the note may have changed) and made, committed.
pub fn approve(vault: &Path, proposal: &Proposal) -> Result<Written, String> {
    let topic = crate::topics::list(vault).into_iter().find(|t| t.name == proposal.topic).ok_or_else(|| format!("There is no topic {} any more.", proposal.topic))?;
    allowed(vault, &topic, &proposal.change)?;
    let done = apply(vault, &proposal.change)?;
    commit(vault, &[proposal.change.note().to_owned()], &format!("loom: {}: {} (approved)", proposal.topic, proposal.reason));
    Ok(done)
}

/// A rule for a topic's instructions, from the user's correction of a change (rejected or
/// undone) and what they said about it: one call to Claude.
pub fn rule_from(vault: &Path, topic: &str, change: &Change, reason: &str, said: &str) -> Result<String, String> {
    let topic = crate::topics::list(vault).into_iter().find(|t| t.name == topic).ok_or_else(|| format!("There is no topic {topic} any more."))?;
    let input = format!(
        "TOPIC: {}\nITS INSTRUCTIONS:\n{}\n\nTHE CHANGE THE USER SAID NO TO ({}):\n{}\n\nWHY THE NOTES KEEPER MADE IT: {reason}\n\nWHAT THE USER SAID: {said}",
        topic.name,
        topic.instructions,
        change.note(),
        change.diff()
    );
    let schema = json!({ "type": "object", "properties": { "rule": { "type": "string" } }, "required": ["rule"] });
    let answer = ask(&prompt("rule", RULE), &input, &schema, &topic.model)?;
    let rule = answer["rule"].as_str().unwrap_or("").trim().trim_start_matches("- ").replace('\n', " ");
    if rule.is_empty() { Err("No rule came of it.".to_owned()) } else { Ok(rule) }
}

/// A rule from the user's answer to a question about a topic, if it holds beyond that one
/// case (None: it doesn't): one call to Claude.
pub fn rule_from_answer(vault: &Path, topic: &str, question: &str, answer: &str) -> Result<Option<String>, String> {
    let topic = crate::topics::list(vault).into_iter().find(|t| t.name == topic).ok_or_else(|| format!("There is no topic {topic} any more."))?;
    let input = format!("TOPIC: {}\nITS INSTRUCTIONS:\n{}\n\nTHE KEEPER ASKED: {question}\n\nTHE USER ANSWERED: {answer}", topic.name, topic.instructions);
    let schema = json!({ "type": "object", "properties": { "rule": { "type": "string" } }, "required": ["rule"] });
    let answer = ask(&prompt("rule-answer", RULE_ANSWER), &input, &schema, &topic.model)?;
    let rule = answer["rule"].as_str().unwrap_or("").trim().trim_start_matches("- ").replace('\n', " ");
    Ok((!rule.is_empty()).then_some(rule))
}

/// A topic's file drafted from the user's description of it: one call to Claude.
pub fn describe_topic(name: &str, said: &str) -> Result<String, String> {
    let input = format!("TOPIC: {name}\n\nWHAT IT SHOULD KEEP: {said}\n\nAN EXAMPLE TOPIC:\n{}", crate::topics::example());
    let schema = json!({
        "type": "object",
        "properties": {
            "folder": { "type": "string" },
            "listens_for": { "type": "array", "items": { "type": "string" } },
            "apply": { "type": "string", "enum": ["ask", "auto"] },
            "instructions": { "type": "string" }
        },
        "required": ["folder", "listens_for", "apply", "instructions"]
    });
    let answer = ask(&prompt("describe", DESCRIBE), &input, &schema, "")?;
    let instructions = answer["instructions"].as_str().unwrap_or("").trim();
    if instructions.is_empty() {
        return Err("No topic came of it.".to_owned());
    }
    let folder = answer["folder"].as_str().map(|f| f.trim().trim_matches('/')).filter(|f| !f.is_empty() && !f.contains(['/', '\\']) && !f.starts_with('.')).unwrap_or(name);
    let listens: Vec<String> = answer["listens_for"].as_array().into_iter().flatten().filter_map(|w| w.as_str().map(str::to_owned)).collect();
    let apply = if answer["apply"].as_str() == Some("auto") { crate::topics::Apply::Auto } else { crate::topics::Apply::Ask };
    Ok(crate::topics::text_of(name, folder, &listens, apply, instructions))
}

/// Where a topic's learned rules go in its file.
const RULES: &str = "## Rules learned";

/// Adds a rule to a topic's instructions (under "Rules learned"), committed like a change.
pub fn add_rule(vault: &Path, topic: &str, rule: &str) -> Result<(), String> {
    let topic = crate::topics::list(vault).into_iter().find(|t| t.name == topic).ok_or_else(|| format!("There is no topic {topic} any more."))?;
    let rule = rule.trim().replace('\n', " ");
    if rule.is_empty() {
        return Err("The rule is empty.".to_owned());
    }
    let text = std::fs::read_to_string(&topic.path).map_err(|e| e.to_string())?;
    std::fs::write(&topic.path, add_under(&text, RULES, &format!("- {rule}"))).map_err(|e| e.to_string())?;
    if let Ok(file) = topic.path.strip_prefix(vault) {
        commit(vault, &[file.to_string_lossy().into_owned()], &format!("loom: {}: a rule learned", topic.name));
    }
    Ok(())
}

/// A test case from a correction (for `loom scribe-eval`): the conversation the change came
/// from, the note as it was before, and that the change must not be proposed again.
pub fn correction_case(p: &Proposal, note_before: Option<&str>, said: &str, when: &str) -> Option<Value> {
    let (talk, title, project) = talk_of(&p.excerpt);
    if talk.is_empty() {
        return None;
    }
    let mark = match &p.change {
        Change::Add { text, .. } | Change::Create { text, .. } => text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#') && *l != "---")?.trim_start_matches("- ").to_owned(),
        Change::Replace { new, .. } => new.trim().trim_start_matches("- ").to_owned(),
        Change::Property { key, value, .. } => format!("{key}: {value}"),
    };
    let notes = note_before.map(|text| json!({ p.change.note(): text })).unwrap_or_else(|| json!({}));
    Some(json!({
        "name": format!("correction {when}: {}", p.change.note()),
        "notes": notes,
        "talk": talk,
        "title": title,
        "project": project,
        "why": said,
        "expect": { "topics": [p.topic], "absent": [{ "note": p.change.note(), "contains": mark }] },
    }))
}

/// An excerpt (as `excerpt` writes it) back into its turns, the conversation's name and project.
fn talk_of(excerpt: &str) -> (Vec<Value>, String, String) {
    let (mut title, mut project) = (String::new(), String::new());
    let mut body = excerpt;
    if let Some(rest) = excerpt.strip_prefix("CONVERSATION: ") {
        let (head, after) = rest.split_once("\n\n").unwrap_or((rest, ""));
        body = after;
        let quoted: Vec<&str> = head.split('"').collect();
        if head.starts_with('"') {
            title = quoted.get(1).copied().unwrap_or("").to_owned();
            project = quoted.get(3).copied().unwrap_or("").to_owned();
        } else {
            project = quoted.get(1).copied().unwrap_or("").to_owned();
        }
    }
    let mut talk: Vec<Value> = Vec::new();
    for block in body.split("\n\n") {
        let turn = block.strip_prefix("USER: ").map(|t| ("user", t)).or_else(|| block.strip_prefix("AGENT: ").map(|t| ("assistant", t)));
        match (turn, talk.last_mut()) {
            (Some((role, text)), _) => talk.push(json!([role, text])),
            // A message with a blank line in it: still the one before.
            (None, Some(last)) => {
                let joined = format!("{}\n\n{block}", last[1].as_str().unwrap_or(""));
                last[1] = joined.into();
            }
            (None, None) => {}
        }
    }
    (talk, title, project)
}

/// `loom scribe-eval <cases.jsonl>`: runs Scribe on each case (a conversation and the notes
/// it starts from, in a fresh vault with the default topics) and scores what it did against
/// what the case expects. Real calls to Claude: a few per case.
pub fn eval(cases: &Path, topics_from: Option<&Path>) -> i32 {
    let Ok(text) = std::fs::read_to_string(cases) else {
        eprintln!("Cannot read {}", cases.display());
        return 2;
    };
    // Its own journal (the record of what is done), gone afterwards.
    let scratch = std::env::temp_dir().join(format!("loom-eval-journal-{}", std::process::id()));
    crate::journal::use_dir(scratch.clone());
    let cases: Vec<Value> = text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect();
    let results: Vec<(String, bool, String, Vec<String>, Vec<String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = cases
            .chunks(cases.len().div_ceil(4).max(1))
            .enumerate()
            .map(|(part, group)| scope.spawn(move || group.iter().enumerate().map(|(i, case)| eval_case(case, part * 1000 + i, topics_from)).collect::<Vec<_>>()))
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    let (mut passed, mut routed_right, mut routed_found, mut routed_expected) = (0, 0, 0, 0);
    for (name, ok, why, expected, found) in &results {
        println!("{} {name}{}", if *ok { "PASS" } else { "FAIL" }, if why.is_empty() { String::new() } else { format!(": {why} [topics found: {}]", found.join(", ")) });
        passed += usize::from(*ok);
        routed_expected += expected.len();
        routed_found += found.len();
        routed_right += found.iter().filter(|f| expected.iter().any(|e| e.eq_ignore_ascii_case(f))).count();
    }
    let ratio = |a: usize, b: usize| if b == 0 { 1.0 } else { a as f64 / b as f64 };
    println!(
        "\n{passed}/{} cases pass · topics: precision {:.0}%, recall {:.0}%",
        results.len(),
        100.0 * ratio(routed_right, routed_found),
        100.0 * ratio(routed_right, routed_expected)
    );
    let _ = std::fs::remove_dir_all(&scratch);
    i32::from(passed != results.len())
}

/// (name, passed, why not, topics expected, topics found)
fn eval_case(case: &Value, n: usize, topics_from: Option<&Path>) -> (String, bool, String, Vec<String>, Vec<String>) {
    let name = case["name"].as_str().unwrap_or("?").to_owned();
    let vault = std::env::temp_dir().join(format!("loom-eval-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&vault);
    if let Err(why) = crate::vault::prepare(&vault) {
        return (name, false, why, Vec::new(), Vec::new());
    }
    // The user's own topics (with the rules learned from their corrections) over the defaults.
    if let Some(from) = topics_from {
        for entry in std::fs::read_dir(from.join(crate::vault::TOPICS)).into_iter().flatten().flatten() {
            let _ = std::fs::copy(entry.path(), vault.join(crate::vault::TOPICS).join(entry.file_name()));
        }
    }
    for (path, text) in case["notes"].as_object().into_iter().flatten() {
        let file = vault.join(path);
        let _ = std::fs::create_dir_all(file.parent().unwrap_or(&vault));
        let _ = std::fs::write(file, text.as_str().unwrap_or(""));
    }
    let entries: Vec<Entry> = case["talk"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, turn)| Entry {
            offset: i as u64,
            end: i as u64 + 1,
            at: 0,
            role: turn[0].as_str().unwrap_or("user").to_owned(),
            text: turn[1].as_str().unwrap_or("").to_owned(),
            title: case["title"].as_str().unwrap_or(&name).to_owned(),
            cwd: case["project"].as_str().map(|p| format!("/home/me/Projects/{p}")).unwrap_or_default(),
        })
        .collect();
    let report = check(&vault, &format!("eval-{n}"), &entries, 20);
    let _ = std::fs::remove_dir_all(&vault);
    let expect = &case["expect"];
    let expected: Vec<String> = expect["topics"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_owned)).collect();
    let mut found = report.routed.clone();
    found.dedup();
    if let Some(why) = report.failed {
        return (name, false, why, expected, found);
    }
    let proposals: Vec<&Proposal> = report.applied.iter().map(|(p, _)| p).chain(report.held.iter().map(|(p, _)| p)).collect();
    let describe = || {
        let changes = proposals.iter().map(|p| format!("{} {}", p.change.note(), p.change.diff().replace('\n', " ")));
        let asked = report.questions.iter().map(|q| format!("asked: {}", q.question));
        changes.chain(asked).collect::<Vec<_>>().join(" | ")
    };
    if expect["changes_or_question"].as_bool() == Some(true) {
        let ok = !proposals.is_empty() || !report.questions.is_empty();
        return (name, ok, if ok { String::new() } else { "nothing proposed or asked".to_owned() }, expected, found);
    }
    // A correction: what the user said no to must not come back.
    if let Some(absent) = expect["absent"].as_array() {
        let back: Vec<String> = absent
            .iter()
            .filter(|a| {
                let (note, contains) = (a["note"].as_str().unwrap_or(""), a["contains"].as_str().unwrap_or("").to_lowercase());
                proposals.iter().any(|p| p.change.note().starts_with(note) && p.change.diff().to_lowercase().contains(&contains))
            })
            .map(|a| format!("{} ∋ {}", a["note"].as_str().unwrap_or(""), a["contains"].as_str().unwrap_or("")))
            .collect();
        let ok = back.is_empty();
        return (name, ok, if ok { String::new() } else { format!("proposed again what was said no to: {}", back.join(", ")) }, expected, found);
    }
    let wanted = expect["changes"].as_array().cloned().unwrap_or_default();
    if wanted.is_empty() {
        let ok = proposals.is_empty() && report.questions.is_empty();
        return (name, ok, if ok { String::new() } else { format!("wrote what it should not: {}", describe()) }, expected, found);
    }
    let missing: Vec<String> = wanted
        .iter()
        .filter(|w| {
            let (note, contains) = (w["note"].as_str().unwrap_or(""), w["contains"].as_str().unwrap_or("").to_lowercase());
            !proposals.iter().any(|p| p.change.note().starts_with(note) && p.change.diff().to_lowercase().contains(&contains))
        })
        .map(|w| format!("{} ∋ {}", w["note"].as_str().unwrap_or(""), w["contains"].as_str().unwrap_or("")))
        .collect();
    let misnamed = expect["path_has"].as_str().filter(|want| !proposals.iter().any(|p| p.change.note().to_lowercase().contains(&want.to_lowercase())));
    let ok = missing.is_empty() && misnamed.is_none();
    let why = match misnamed {
        _ if !missing.is_empty() => format!("missing {} (proposed: {})", missing.join(", "), describe()),
        Some(want) => format!("no note named for \"{want}\" (proposed: {})", describe()),
        None => String::new(),
    };
    (name, ok, why, expected, found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_note_made_meanwhile_is_merged_into() {
        let old = "---\ntags:\n  - system\n---\n\n# Setup\n\n## Setup and changes\n\n- OS: CachyOS.\n\n## Problems and fixes\n";
        let new = "---\ndate: 2026-10-27\n---\n# Setup\n\n## Changes\n\n- OS: CachyOS.\n- 2026-10-27: a snapshot.\n\n## Hardware\n\n- 16 GB RAM.\n";
        let merged = super::merge(old, new);
        assert_eq!(merged, "---\ntags:\n  - system\n---\n\n# Setup\n\n## Setup and changes\n\n- OS: CachyOS.\n- 2026-10-27: a snapshot.\n\n## Problems and fixes\n\n## Hardware\n- 16 GB RAM.\n");
    }

    #[test]
    fn lines_are_added_at_the_end_of_their_section() {
        let note = "# Money\n\n## Recurring\n- Rent: 650\n- Phone: 12\n\n## Savings\n- 1000\n";
        let out = add_under(note, "## Recurring", "- Gym: 30");
        assert_eq!(out, "# Money\n\n## Recurring\n- Rent: 650\n- Phone: 12\n- Gym: 30\n\n## Savings\n- 1000\n");
        let out = add_under(note, "## Savings", "- 1200 (2026-10)");
        assert!(out.ends_with("## Savings\n- 1000\n- 1200 (2026-10)\n"), "{out}");
        let out = add_under(note, "## Goals", "- A bike");
        assert!(out.ends_with("## Savings\n- 1000\n\n## Goals\n- A bike\n"), "{out}");
        // A subheading belongs to its section; the next heading of the same level ends it.
        let nested = "## A\nx\n### A1\ny\n## B\nz\n";
        assert_eq!(add_under(nested, "## A", "new"), "## A\nx\n### A1\ny\nnew\n## B\nz\n");
    }

    #[test]
    fn one_line_is_replaced_and_properties_set() {
        let note = "---\ndate: 2026-01-01\n---\n- Rent: 650\n- Phone: 12\n";
        assert_eq!(replace_line(note, "- Rent: 650", "- Rent: 700").unwrap(), "---\ndate: 2026-01-01\n---\n- Rent: 700\n- Phone: 12\n");
        assert!(replace_line(note, "- Rent: 600", "x").is_err());
        assert!(replace_line("a\na\n", "a", "b").is_err());
        assert_eq!(set_property(note, "currency", "EUR").unwrap(), "---\ndate: 2026-01-01\ncurrency: EUR\n---\n- Rent: 650\n- Phone: 12\n");
        assert_eq!(set_property(note, "date", "2026-02-01").unwrap(), "---\ndate: 2026-02-01\n---\n- Rent: 650\n- Phone: 12\n");
        assert_eq!(set_property("body\n", "k", "v").unwrap(), "---\nk: v\n---\nbody\n");
    }

    #[test]
    fn changes_stay_in_their_topic() {
        let vault = std::env::temp_dir().join(format!("loom-scribe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&vault);
        std::fs::create_dir_all(vault.join("Finances")).unwrap();
        std::fs::write(vault.join("Finances/Overview.md"), "## Recurring\n").unwrap();
        let topic = Topic {
            name: "Finances".into(),
            path: vault.join("Topics/Finances.md"),
            folder: "Finances".into(),
            listens_for: vec![],
            apply: Apply::Auto,
            sync: false,
            model: "haiku".into(),
            enabled: true,
            instructions: String::new(),
        };
        let add = |note: &str| Change::Add { note: note.into(), under: "## Recurring".into(), text: "- Rent: 650".into() };
        assert!(allowed(&vault, &topic, &add("Finances/Overview.md")).is_ok());
        assert!(allowed(&vault, &topic, &add("Work/Acme.md")).is_err());
        assert!(allowed(&vault, &topic, &add("../outside.md")).is_err());
        assert!(allowed(&vault, &topic, &add("Finances/Missing.md")).is_err());
        assert!(allowed(&vault, &topic, &add("Topics/Finances.md")).is_err());
        let create = Change::Create { note: "Finances/2026-10.md".into(), text: "# October".into() };
        assert!(allowed(&vault, &topic, &create).is_ok());
        apply(&vault, &create).unwrap();
        // Made meanwhile: merged into, not refused.
        assert!(allowed(&vault, &topic, &create).is_ok());
        apply(&vault, &add("Finances/Overview.md")).unwrap();
        assert_eq!(std::fs::read_to_string(vault.join("Finances/Overview.md")).unwrap(), "## Recurring\n- Rent: 650\n");
        let _ = std::fs::remove_dir_all(&vault);
    }

    #[test]
    fn a_correction_becomes_a_case() {
        let p = Proposal {
            topic: "Finances".into(),
            change: Change::Add { note: "Finances/Overview.md".into(), under: "## Recurring".into(), text: "- Gym: 30".into() },
            reason: "x".into(),
            excerpt: "CONVERSATION: \"Gym talk\", in the user's project folder \"life\"\n\nUSER: I might join a gym.\n\nIt costs 30.\n\nAGENT: Noted.".into(),
        };
        let case = correction_case(&p, Some("## Recurring\n"), "only might", "2026-10-28").unwrap();
        assert_eq!(case["talk"], json!([["user", "I might join a gym.\n\nIt costs 30."], ["assistant", "Noted."]]));
        assert_eq!((case["title"].as_str(), case["project"].as_str()), (Some("Gym talk"), Some("life")));
        assert_eq!(case["expect"]["absent"][0]["contains"], "Gym: 30");
        assert_eq!(case["notes"]["Finances/Overview.md"], "## Recurring\n");
        // A rule goes under its own heading, made the first time.
        let vault = std::env::temp_dir().join(format!("loom-rule-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&vault);
        crate::vault::prepare(&vault).unwrap();
        add_rule(&vault, "Finances", "Only record costs the user has committed to.").unwrap();
        add_rule(&vault, "Finances", "Amounts are net.").unwrap();
        let text = std::fs::read_to_string(vault.join("Topics/Finances.md")).unwrap();
        assert!(text.contains("## Rules learned\n- Only record costs the user has committed to.\n- Amounts are net."), "{text}");
        let _ = std::fs::remove_dir_all(&vault);
    }

    #[test]
    fn written_changes_are_undone() {
        let vault = std::env::temp_dir().join(format!("loom-undo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&vault);
        std::fs::create_dir_all(&vault).unwrap();
        let note = vault.join("Health.md");
        std::fs::write(&note, "# Health\n## Sleep\n- 7h\n## Food\n- less sugar\n").unwrap();
        let add = Change::Add { note: "Health.md".into(), under: "## Sleep".into(), text: "- 8h on weekends".into() };
        let done = apply(&vault, &add).unwrap();
        // Untouched since: back exactly as it was.
        undo(&vault, "Health.md", &done, "x").unwrap();
        assert_eq!(std::fs::read_to_string(&note).unwrap(), done.before.clone().unwrap());
        // Changed since (elsewhere): only the change's own line goes.
        let done = apply(&vault, &add).unwrap();
        let edited = done.after.replace("- less sugar", "- less sugar\n- more water");
        std::fs::write(&note, &edited).unwrap();
        undo(&vault, "Health.md", &done, "x").unwrap();
        assert_eq!(std::fs::read_to_string(&note).unwrap(), "# Health\n## Sleep\n- 7h\n## Food\n- less sugar\n- more water\n");
        // The line itself changed since: left alone.
        let done = apply(&vault, &add).unwrap();
        std::fs::write(&note, done.after.replace("- 8h on weekends", "- 9h on weekends")).unwrap();
        assert!(undo(&vault, "Health.md", &done, "x").is_err());
        let replace = Change::Replace { note: "Health.md".into(), old: "- 7h".into(), new: "- 7.5h".into() };
        std::fs::write(&note, "# Health\n## Sleep\n- 7h\n").unwrap();
        let done = apply(&vault, &replace).unwrap();
        std::fs::write(&note, format!("{}- naps\n", done.after)).unwrap();
        undo(&vault, "Health.md", &done, "x").unwrap();
        assert_eq!(std::fs::read_to_string(&note).unwrap(), "# Health\n## Sleep\n- 7h\n- naps\n");
        let _ = std::fs::remove_dir_all(&vault);
    }

    #[test]
    fn long_conversations_come_in_pieces() {
        let entry = |n: usize| Entry { offset: n as u64, end: n as u64 + 1, at: 0, role: "user".into(), text: "x".repeat(2500), title: String::new(), cwd: String::new() };
        let entries: Vec<Entry> = (0..5).map(entry).collect();
        let pieces = chunks(&entries);
        assert_eq!(pieces.iter().map(|c| c.len()).collect::<Vec<_>>(), vec![2, 2, 1]);
    }
}
