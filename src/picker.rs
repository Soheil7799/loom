//! One fuzzy picker for everything typed-to-find: the command palette, going to an agent,
//! finding a file. Type parts of a name, ↑/↓ to choose, Enter to run.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::{gdk, glib, pango, prelude::*};

const SHOWN: usize = 40;

pub struct Choice {
    pub title: String,
    /// Shown dimmed on the right (a shortcut, a folder…); searched too.
    pub detail: String,
    /// A status dot class ("working"…), for agents.
    pub dot: Option<&'static str>,
    pub run: Rc<dyn Fn()>,
}

impl Choice {
    pub fn new(title: impl Into<String>, detail: impl Into<String>, run: impl Fn() + 'static) -> Choice {
        Choice { title: title.into(), detail: detail.into(), dot: None, run: Rc::new(run) }
    }
}

pub struct Picker {
    popover: gtk::Popover,
    entry: gtk::SearchEntry,
    list: gtk::ListBox,
    choices: RefCell<Vec<Choice>>,
    shown: RefCell<Vec<usize>>,
}

impl Picker {
    pub fn new(parent: &impl IsA<gtk::Widget>) -> Rc<Picker> {
        let entry = gtk::SearchEntry::new();
        let list = gtk::ListBox::new();
        list.add_css_class("picker-list");
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(440)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.add_css_class("picker");
        content.set_size_request(580, -1);
        content.append(&entry);
        content.append(&scroller);
        let popover = gtk::Popover::builder().child(&content).has_arrow(false).position(gtk::PositionType::Bottom).build();
        popover.set_parent(parent);
        let picker = Rc::new(Picker { popover, entry, list, choices: RefCell::default(), shown: RefCell::default() });

        let weak = Rc::downgrade(&picker);
        picker.entry.connect_search_changed(move |_| {
            if let Some(picker) = weak.upgrade() {
                picker.fill();
            }
        });
        let weak = Rc::downgrade(&picker);
        picker.entry.connect_activate(move |_| {
            if let Some(picker) = weak.upgrade() {
                picker.pick(picker.list.selected_row().map_or(0, |r| r.index()));
            }
        });
        let weak = Rc::downgrade(&picker);
        picker.list.connect_row_activated(move |_, row| {
            if let Some(picker) = weak.upgrade() {
                picker.pick(row.index());
            }
        });
        let keys = gtk::EventControllerKey::new();
        let list = picker.list.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let index = list.selected_row().map_or(-1, |r| r.index());
            let next = match key {
                gdk::Key::Down => index + 1,
                gdk::Key::Up => (index - 1).max(0),
                gdk::Key::Page_Down => index + 8,
                gdk::Key::Page_Up => (index - 8).max(0),
                _ => return glib::Propagation::Proceed,
            };
            let row = list.row_at_index(next).or_else(|| list.last_child().and_downcast::<gtk::ListBoxRow>());
            if let Some(row) = row {
                list.select_row(Some(&row));
                row.grab_focus();
            }
            glib::Propagation::Stop
        });
        picker.entry.add_controller(keys);
        picker
    }

    pub fn open(&self, placeholder: &str, choices: Vec<Choice>) {
        *self.choices.borrow_mut() = choices;
        self.entry.set_placeholder_text(Some(placeholder));
        self.entry.set_text("");
        self.fill();
        if let Some(parent) = self.popover.parent() {
            self.popover.set_pointing_to(Some(&gdk::Rectangle::new(parent.width() / 2, 10, 1, 1)));
        }
        self.popover.popup();
        self.entry.grab_focus();
    }

    fn fill(&self) {
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let query = self.entry.text().to_lowercase();
        let choices = self.choices.borrow();
        let mut ranked: Vec<(i64, usize)> = choices
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                // The title counts more than the detail.
                let title = score(&query, &c.title).map(|s| s * 2);
                let both = score(&query, &format!("{} {}", c.title, c.detail));
                title.max(both).map(|s| (s, i))
            })
            .collect();
        // Best first; ties keep the given order.
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let shown: Vec<usize> = ranked.into_iter().take(SHOWN).map(|(_, i)| i).collect();
        for &i in &shown {
            let choice = &choices[i];
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            if let Some(dot) = choice.dot {
                let d = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                d.add_css_class("dot");
                d.add_css_class(dot);
                d.set_valign(gtk::Align::Center);
                line.append(&d);
            }
            let title = gtk::Label::builder().label(&choice.title).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            let detail = gtk::Label::builder()
                .label(&choice.detail)
                .xalign(1.0)
                .hexpand(true)
                .ellipsize(pango::EllipsizeMode::Start)
                .build();
            detail.add_css_class("dim");
            detail.add_css_class("small");
            line.append(&title);
            line.append(&detail);
            self.list.append(&line);
        }
        drop(choices);
        *self.shown.borrow_mut() = shown;
        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
    }

    fn pick(&self, index: i32) {
        let run = {
            let shown = self.shown.borrow();
            let Some(&i) = shown.get(index.max(0) as usize) else { return };
            self.choices.borrow()[i].run.clone()
        };
        self.popover.popdown();
        run();
    }
}

/// How well `query` matches `text`: its characters in order (None if not all are there).
/// Matches after the last '/', at word starts and in runs score higher.
pub fn score(query: &str, text: &str) -> Option<i64> {
    if query.trim().is_empty() {
        return Some(0);
    }
    let lower = text.to_lowercase();
    let name_start = lower.rfind('/').map_or(0, |i| i + 1);
    let chars: Vec<(usize, char)> = lower.char_indices().collect();
    let mut score = 0i64;
    let mut at = 0usize;
    let mut previous: Option<usize> = None;
    for wanted in query.chars().filter(|c| !c.is_whitespace()) {
        let found = chars[at..].iter().position(|(_, c)| *c == wanted)?;
        let index = at + found;
        score += 1;
        if chars[index].0 >= name_start {
            score += 4;
        }
        if index == 0 || matches!(chars[index - 1].1, '/' | '_' | '-' | '.' | ' ' | ':') {
            score += 3;
        }
        if previous.is_some_and(|p| p + 1 == index) {
            score += 2;
        }
        previous = Some(index);
        at = index + 1;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn letters_in_order_match() {
        assert!(score("agrs", "src/agent.rs").is_some());
        assert!(score("zz", "src/agent.rs").is_none());
    }

    #[test]
    fn the_file_name_beats_the_folder() {
        assert!(score("app", "src/app.rs") > score("app", "apps/x/main.rs"));
    }

    #[test]
    fn dotfiles_and_spaces_work() {
        assert!(score(".env", "config/.env").is_some());
        assert!(score("new ag", "New agent here").is_some());
    }
}
