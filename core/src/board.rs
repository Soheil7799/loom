//! A project's board: `.loom/board.md` in the project folder, in the Obsidian Kanban plugin's
//! format (it opens as a board there too). Lanes are `## ` headings; a card is a `- [ ]` item
//! (`- [x]` once done), its description the lines indented under it, its last line loom's own:
//! `— c12 · created 2026-11-10 13:05 · started … · done … · by review`. What loom doesn't know
//! (the front matter, the plugin's settings) is kept as it was.

use std::path::{Path, PathBuf};

/// The lanes a new board gets.
pub const LANES: [&str; 4] = ["Backlog", "To do", "Doing", "Done"];

const HEAD: &str = "---\n\nkanban-plugin: board\n\n---\n";
const TAIL: &str = "%% kanban:settings\n```\n{\"kanban-plugin\":\"board\"}\n```\n%%\n";

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Card {
    pub id: u32,
    pub title: String,
    pub body: String,
    pub done: bool,
    /// Local times, "YYYY-MM-DD HH:MM".
    pub created: String,
    pub started: String,
    pub finished: String,
    /// Who filed it: "you", an agent's title, "review"…
    pub by: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    pub title: String,
    pub cards: Vec<Card>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Board {
    pub lanes: Vec<Lane>,
    head: String,
    tail: String,
}

/// Where `project`'s board is.
pub fn path(project: &Path) -> PathBuf {
    project.join(".loom").join("board.md")
}

impl Default for Board {
    fn default() -> Board {
        Board { lanes: LANES.iter().map(|t| Lane { title: (*t).to_owned(), cards: Vec::new() }).collect(), head: HEAD.to_owned(), tail: TAIL.to_owned() }
    }
}

impl Board {
    /// `project`'s board as its file has it (a new one if there is none).
    pub fn load(project: &Path) -> Board {
        std::fs::read_to_string(path(project)).map(|text| Board::parse(&text)).unwrap_or_default()
    }

    /// Written back whole (through a file beside it, so a reader never sees half of it).
    pub fn save(&self, project: &Path) -> std::io::Result<()> {
        let file = path(project);
        std::fs::create_dir_all(file.parent().expect("in .loom"))?;
        let part = file.with_extension("md.part");
        std::fs::write(&part, self.render())?;
        std::fs::rename(part, file)
    }

    pub fn parse(text: &str) -> Board {
        let (mut head, mut tail) = (String::new(), String::new());
        let mut lanes: Vec<Lane> = Vec::new();
        let mut in_tail = false;
        for line in text.lines() {
            if in_tail || line.starts_with("%% kanban:settings") {
                in_tail = true;
                tail.push_str(line);
                tail.push('\n');
            } else if let Some(title) = line.strip_prefix("## ") {
                lanes.push(Lane { title: title.trim().to_owned(), cards: Vec::new() });
            } else if lanes.is_empty() {
                head.push_str(line);
                head.push('\n');
            } else if let Some((done, title)) = line.strip_prefix("- [ ] ").map(|t| (false, t)).or_else(|| line.strip_prefix("- [x] ").map(|t| (true, t))) {
                let lane = lanes.last_mut().expect("a lane");
                lane.cards.push(Card { title: title.trim().to_owned(), done, ..Card::default() });
            } else if let Some(card) = lanes.last_mut().and_then(|l| l.cards.last_mut())
                && (line.starts_with('\t') || line.starts_with("    "))
            {
                let line = line.strip_prefix('\t').unwrap_or_else(|| &line[4..]);
                if !read_meta(card, line) {
                    if !card.body.is_empty() {
                        card.body.push('\n');
                    }
                    card.body.push_str(line);
                }
            }
        }
        let mut board = Board { lanes, head: if head.trim().is_empty() { HEAD.to_owned() } else { format!("{}\n", head.trim_end()) }, tail: if tail.is_empty() { TAIL.to_owned() } else { tail } };
        if board.lanes.is_empty() {
            board.lanes = Board::default().lanes;
        }
        // Cards written by hand (no meta line yet) get ids now.
        let mut next = board.cards().map(|c| c.id).max().unwrap_or(0);
        for card in board.lanes.iter_mut().flat_map(|l| l.cards.iter_mut()).filter(|c| c.id == 0) {
            next += 1;
            card.id = next;
        }
        board
    }

    pub fn render(&self) -> String {
        let mut out = self.head.trim_end().to_owned();
        out.push_str("\n\n");
        for lane in &self.lanes {
            out.push_str(&format!("## {}\n\n", lane.title));
            for card in &lane.cards {
                out.push_str(&format!("- [{}] {}\n", if card.done { "x" } else { " " }, card.title));
                for line in card.body.lines() {
                    out.push_str(&format!("\t{line}\n"));
                }
                out.push_str(&format!("\t{}\n", meta(card)));
            }
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&self.tail);
        out
    }

    pub fn cards(&self) -> impl Iterator<Item = &Card> {
        self.lanes.iter().flat_map(|l| l.cards.iter())
    }

    fn lane_of(&self, id: u32) -> Option<usize> {
        self.lanes.iter().position(|l| l.cards.iter().any(|c| c.id == id))
    }

    fn lane_named(&self, lane: &str) -> Option<usize> {
        self.lanes.iter().position(|l| l.title.eq_ignore_ascii_case(lane.trim()))
    }

    /// A new card at the end of `lane` (None: the first lane); its id.
    pub fn add(&mut self, lane: Option<&str>, title: &str, body: &str, by: &str, now: &str) -> Result<u32, String> {
        let title = one_line(title);
        if title.is_empty() {
            return Err("a card needs a title".into());
        }
        let at = match lane {
            Some(lane) => self.lane_named(lane).ok_or_else(|| format!("no lane {lane}"))?,
            None => 0,
        };
        let id = self.cards().map(|c| c.id).max().unwrap_or(0) + 1;
        let mut card = Card { id, title, body: body.trim().to_owned(), created: now.to_owned(), by: one_line(by), ..Card::default() };
        stamp(&mut card, &self.lanes[at].title, now);
        self.lanes[at].cards.push(card);
        Ok(id)
    }

    /// Card `id` to the end of `lane` (or before card `before` there): started on entering
    /// Doing, done (and checked) on entering Done, unchecked if it leaves Done.
    pub fn move_card(&mut self, id: u32, lane: &str, before: Option<u32>, now: &str) -> Result<(), String> {
        let from = self.lane_of(id).ok_or_else(|| format!("no card {id}"))?;
        let to = self.lane_named(lane).ok_or_else(|| format!("no lane {lane}"))?;
        let at = self.lanes[from].cards.iter().position(|c| c.id == id).expect("there");
        let mut card = self.lanes[from].cards.remove(at);
        stamp(&mut card, &self.lanes[to].title, now);
        let cards = &mut self.lanes[to].cards;
        let at = before.and_then(|b| cards.iter().position(|c| c.id == b)).unwrap_or(cards.len());
        cards.insert(at, card);
        Ok(())
    }

    /// Card `id`'s title and/or description changed; `note` is added below its description.
    pub fn edit(&mut self, id: u32, title: Option<&str>, body: Option<&str>, note: Option<&str>) -> Result<(), String> {
        let card = self.lanes.iter_mut().flat_map(|l| l.cards.iter_mut()).find(|c| c.id == id).ok_or_else(|| format!("no card {id}"))?;
        if let Some(title) = title.map(one_line).filter(|t| !t.is_empty()) {
            card.title = title;
        }
        if let Some(body) = body {
            card.body = body.trim().to_owned();
        }
        if let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) {
            if !card.body.is_empty() {
                card.body.push('\n');
            }
            card.body.push_str(note);
        }
        Ok(())
    }

    pub fn remove(&mut self, id: u32) -> Result<(), String> {
        let lane = self.lane_of(id).ok_or_else(|| format!("no card {id}"))?;
        self.lanes[lane].cards.retain(|c| c.id != id);
        Ok(())
    }
}

/// The dates a lane gives a card arriving in it.
fn stamp(card: &mut Card, lane: &str, now: &str) {
    let lane = lane.to_lowercase();
    if lane == "doing" && card.started.is_empty() {
        card.started = now.to_owned();
    }
    card.done = lane == "done";
    if card.done {
        if card.finished.is_empty() {
            card.finished = now.to_owned();
        }
    } else {
        card.finished.clear();
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn meta(card: &Card) -> String {
    let mut parts = vec![format!("c{}", card.id)];
    for (word, value) in [("created", &card.created), ("started", &card.started), ("done", &card.finished)] {
        if !value.is_empty() {
            parts.push(format!("{word} {value}"));
        }
    }
    if !card.by.is_empty() {
        parts.push(format!("by {}", card.by));
    }
    format!("— {}", parts.join(" · "))
}

/// Reads loom's line into `card`; false if `line` isn't one.
fn read_meta(card: &mut Card, line: &str) -> bool {
    let Some(rest) = line.strip_prefix("— c") else { return false };
    let mut parts = rest.split(" · ");
    let Some(id) = parts.next().and_then(|p| p.trim().parse().ok()) else { return false };
    card.id = id;
    for part in parts {
        let (word, value) = part.split_once(' ').unwrap_or((part, ""));
        let value = value.trim().to_owned();
        match word {
            "created" => card.created = value,
            "started" => card.started = value,
            "done" => card.finished = value,
            "by" => card.by = value,
            _ => {}
        }
    }
    true
}

/// A board command on `project`'s board (`by`: who it is filed as): `board` reads it,
/// `board_add`, `board_move`, `board_edit` (`note` adds to a description) and
/// `board_remove` change it. None: not a board command.
pub fn act(project: &Path, method: &str, params: &serde_json::Value, by: &str) -> Option<Result<serde_json::Value, String>> {
    use serde_json::json;
    let card = || params["card"].as_u64().map(|c| c as u32).ok_or_else(|| "which card?".to_owned());
    let mut board = Board::load(project);
    let now = now();
    let changed = match method {
        "board" => None,
        "board_add" => Some(board.add(params["lane"].as_str(), params["title"].as_str().unwrap_or(""), params["body"].as_str().unwrap_or(""), by, &now).map(|id| json!({ "card": id }))),
        "board_move" => Some(card().and_then(|c| board.move_card(c, params["lane"].as_str().ok_or("which lane?")?, params["before"].as_u64().map(|b| b as u32), &now)).map(|()| json!({}))),
        "board_edit" => Some(card().and_then(|c| board.edit(c, params["title"].as_str(), params["body"].as_str(), params["note"].as_str())).map(|()| json!({}))),
        "board_remove" => Some(card().and_then(|c| board.remove(c)).map(|()| json!({}))),
        _ => return None,
    };
    if let Some(result) = changed {
        return Some(result.and_then(|answer| board.save(project).map(|()| answer).map_err(|e| format!("{}: {e}", path(project).display()))));
    }
    Some(Ok(json!({ "project": project.to_string_lossy(), "file": path(project).to_string_lossy(), "lanes": board.lanes.iter().map(|lane| json!({
        "title": lane.title,
        "cards": lane.cards.iter().map(|c| json!({ "id": c.id, "title": c.title, "body": c.body, "done": c.done, "created": c.created, "started": c.started, "finished": c.finished, "by": c.by })).collect::<Vec<_>>(),
    })).collect::<Vec<_>>() })))
}

/// The board commands for people (window, CLI): any project of the sidebar's. Agents reach
/// their own project's board through their loom tools (workspace.rs), bridges none.
pub fn serve() {
    use crate::api::{Error, Principal};
    for method in ["board", "board_add", "board_move", "board_edit", "board_remove"] {
        crate::api::hub().handle(method, move |who, params| {
            if !matches!(who, Principal::Window | Principal::Cli) {
                return Err(Error::Forbidden("a project's board, but from loom's window or the CLI (agents: their loom tools)".into()));
            }
            let project = params["project"].as_str().ok_or_else(|| Error::Invalid("which project?".into()))?;
            let project = crate::project_files::inside(project, &crate::project_files::projects()).map_err(|_| Error::Forbidden("only a project of the sidebar's".into()))?;
            act(&project, method, params, "you").expect("a board command").map_err(Error::Invalid)
        });
    }
}

/// Now, as cards keep it: local time to the minute.
pub fn now() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%d %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_reads_back_as_written_and_keeps_what_it_does_not_know() {
        let mut board = Board::default();
        let a = board.add(None, "Fix the  footer\ntooltip", "It opens under the panel.\nOnly at the bottom.", "review", "2026-11-10 13:05").unwrap();
        let b = board.add(Some("to do"), "Kanban board", "", "you", "2026-11-10 13:06").unwrap();
        board.move_card(a, "Doing", None, "2026-11-10 14:00").unwrap();
        board.move_card(a, "Done", None, "2026-11-10 15:20").unwrap();
        board.edit(b, None, None, Some("Obsidian's format.")).unwrap();
        let text = board.render();
        assert!(text.contains("- [x] Fix the footer tooltip\n\tIt opens under the panel.\n\tOnly at the bottom.\n\t— c1 · created 2026-11-10 13:05 · started 2026-11-10 14:00 · done 2026-11-10 15:20 · by review\n"), "{text}");
        assert!(text.starts_with("---\n\nkanban-plugin: board\n\n---\n") && text.ends_with("%%\n"));
        assert_eq!(Board::parse(&text), board);
        // Back out of Done: unchecked, its done date gone, its start kept.
        board.move_card(a, "Backlog", Some(b), "2026-11-10 16:00").unwrap_or_else(|e| panic!("{e}"));
        let card = board.cards().find(|c| c.id == a).unwrap();
        assert!(!card.done && card.finished.is_empty() && card.started == "2026-11-10 14:00");
    }

    #[test]
    fn cards_written_by_hand_in_obsidian_get_ids() {
        let text = "---\n\nkanban-plugin: board\n\n---\n\n## Ideas\n\n- [ ] Email, read only\n\tdrafts, never sending\n- [ ] Another\n\t— c7 · by you\n\n%% kanban:settings\n```\n{\"kanban-plugin\":\"board\",\"x\":1}\n```\n%%\n";
        let board = Board::parse(text);
        let ids: Vec<u32> = board.cards().map(|c| c.id).collect();
        assert_eq!(ids, vec![8, 7]);
        assert_eq!(board.lanes[0].cards[0].body, "drafts, never sending");
        assert!(board.render().contains("\"x\":1"), "the plugin's settings kept");
    }
}
