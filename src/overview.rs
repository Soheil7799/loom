//! Every open agent on one page, most in need of you first: what each is doing, what it
//! last said, what it waits for; and the prompts scheduled to be sent.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::pango;

use crate::agent::Status;
use crate::schedule::Schedule;

/// What a card shows of an agent.
pub struct Card {
    pub id: u32,
    pub title: String,
    pub project: String,
    /// Its repo and branch, or the folder it works in.
    pub place: String,
    pub status: Status,
    pub unseen: bool,
    /// The permissions or questions waiting.
    pub waiting: Vec<String>,
    pub reply: Option<String>,
    pub idle_for: u64,
    pub scheduled: usize,
}

pub enum Act {
    RunNow,
    Pause,
    Remove,
}

type Handler<T> = RefCell<Option<Rc<dyn Fn(T)>>>;

pub struct Overview {
    pub root: gtk::ScrolledWindow,
    counts: gtk::Label,
    cards: gtk::FlowBox,
    ids: RefCell<Vec<u32>>,
    schedules: gtk::Box,
    pub on_open: Handler<u32>,
    pub on_schedule_for: Handler<u32>,
    pub on_schedule: Handler<(u64, Act)>,
}

impl Overview {
    pub fn new() -> Rc<Overview> {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 14);
        page.add_css_class("overview");
        let title = gtk::Label::builder().label("Overview").xalign(0.0).build();
        title.add_css_class("overview-title");
        let counts = gtk::Label::builder().xalign(0.0).build();
        counts.add_css_class("dim");
        page.append(&title);
        page.append(&counts);
        let cards = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(1)
            .max_children_per_line(3)
            .column_spacing(12)
            .row_spacing(12)
            .activate_on_single_click(true)
            .build();
        cards.add_css_class("overview-cards");
        // A card's spacer keeps its foot at the bottom; it must not stretch the rows too.
        cards.set_vexpand(false);
        page.set_valign(gtk::Align::Start);
        page.append(&cards);
        let heading = gtk::Label::builder().label("SCHEDULED PROMPTS").xalign(0.0).build();
        heading.add_css_class("overview-heading");
        page.append(&heading);
        let schedules = gtk::Box::new(gtk::Orientation::Vertical, 6);
        page.append(&schedules);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&page);
        column.set_layout_manager(Some(crate::column::ColumnLayout::new(1100)));
        let root = gtk::ScrolledWindow::builder().child(&column).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        root.add_css_class("chat");
        let overview = Rc::new(Overview {
            root,
            counts,
            cards,
            ids: RefCell::default(),
            schedules,
            on_open: RefCell::default(),
            on_schedule_for: RefCell::default(),
            on_schedule: RefCell::default(),
        });
        let weak = Rc::downgrade(&overview);
        overview.cards.connect_child_activated(move |_, child| {
            let Some(overview) = weak.upgrade() else { return };
            let id = overview.ids.borrow().get(child.index() as usize).copied();
            let handler = overview.on_open.borrow().clone();
            if let (Some(id), Some(handler)) = (id, handler) {
                handler(id);
            }
        });
        overview
    }

    /// The first card takes the keyboard.
    pub fn grab_focus(&self) {
        if let Some(first) = self.cards.child_at_index(0) {
            first.grab_focus();
        }
    }

    pub fn fill(self: &Rc<Self>, mut cards: Vec<Card>, schedules: &[Schedule]) {
        cards.sort_by_key(|c| c.status.urgency(c.unseen));
        let count = |f: &dyn Fn(&Card) -> bool| cards.iter().filter(|c| f(c)).count();
        let parts = [
            (count(&|c| c.status == Status::Blocked), "need you"),
            (count(&|c| c.status == Status::Failed), "stopped"),
            (count(&|c| matches!(c.status, Status::Working | Status::Starting)), "working"),
            (count(&|c| c.status == Status::Idle && c.unseen), "done, not seen yet"),
        ];
        let summary: Vec<String> = parts.iter().filter(|(n, _)| *n > 0).map(|(n, what)| format!("{n} {what}")).collect();
        self.counts.set_label(&if cards.is_empty() {
            "No agents open. Pick a project in the sidebar to start one.".to_owned()
        } else if summary.is_empty() {
            format!("{} agents, all quiet", cards.len())
        } else {
            summary.join(" · ")
        });
        while let Some(child) = self.cards.child_at_index(0) {
            self.cards.remove(&child);
        }
        *self.ids.borrow_mut() = cards.iter().map(|c| c.id).collect();
        for card in &cards {
            self.cards.insert(&self.card(card), -1);
        }

        while let Some(child) = self.schedules.first_child() {
            self.schedules.remove(&child);
        }
        if schedules.is_empty() {
            let none = gtk::Label::builder()
                .label("None yet. On a card, Schedule… sends that agent a prompt at a time you pick (every day at 9, every 2h, in 20m).")
                .xalign(0.0)
                .wrap(true)
                .build();
            none.add_css_class("dim");
            self.schedules.append(&none);
        }
        for schedule in schedules {
            self.schedules.append(&self.schedule_row(schedule));
        }
    }

    fn card(self: &Rc<Self>, card: &Card) -> gtk::Box {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("overview-card");
        if card.status == Status::Blocked {
            root.add_css_class("needs-you");
        }
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("dot");
        dot.add_css_class(if card.status == Status::Idle && card.unseen { "done" } else { card.status.css() });
        dot.set_valign(gtk::Align::Center);
        let title = gtk::Label::builder().label(&card.title).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
        title.add_css_class("card-title");
        let state = gtk::Label::new(Some(&match card.status {
            Status::Idle if card.unseen => "done".to_owned(),
            Status::Idle => ago(card.idle_for),
            other => other.describe().to_owned(),
        }));
        state.add_css_class("small");
        state.add_css_class(if card.status == Status::Blocked { "accent" } else { "dim" });
        top.append(&dot);
        top.append(&title);
        top.append(&state);
        root.append(&top);
        let place = [card.project.as_str(), card.place.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        let place = gtk::Label::builder().label(place).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        place.add_css_class("dim");
        place.add_css_class("small");
        root.append(&place);
        for question in card.waiting.iter().take(2) {
            let label = gtk::Label::builder().label(format!("Wants to: {question}")).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            label.add_css_class("card-waiting");
            root.append(&label);
        }
        let reply = card.reply.as_deref().map(one_paragraph).unwrap_or_default();
        let text = gtk::Label::builder()
            .label(match (reply.is_empty(), card.status) {
                (false, _) => reply.as_str(),
                (true, Status::Suspended) => "Not loaded yet: open it to pick the conversation up.",
                (true, _) => "Nothing said yet.",
            })
            .xalign(0.0)
            .yalign(0.0)
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .lines(3)
            .ellipsize(pango::EllipsizeMode::End)
            .width_chars(24)
            .max_width_chars(36)
            .build();
        text.add_css_class("card-reply");
        root.append(&text);
        let room = gtk::Box::new(gtk::Orientation::Vertical, 0);
        room.set_vexpand(true);
        root.append(&room);
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let scheduled = gtk::Label::builder()
            .label(match card.scheduled {
                0 => String::new(),
                1 => "1 scheduled prompt".to_owned(),
                n => format!("{n} scheduled prompts"),
            })
            .xalign(0.0)
            .hexpand(true)
            .build();
        scheduled.add_css_class("dim");
        scheduled.add_css_class("small");
        let plan = gtk::Button::with_label("Schedule…");
        plan.add_css_class("flat");
        plan.add_css_class("small");
        plan.set_tooltip_text(Some("Send this agent a prompt at a time you pick"));
        let weak = Rc::downgrade(self);
        let id = card.id;
        plan.connect_clicked(move |_| {
            let handler = weak.upgrade().and_then(|o| o.on_schedule_for.borrow().clone());
            if let Some(handler) = handler {
                handler(id);
            }
        });
        foot.append(&scheduled);
        foot.append(&plan);
        root.append(&foot);
        root
    }

    fn schedule_row(self: &Rc<Self>, schedule: &Schedule) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("schedule-row");
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
        texts.set_hexpand(true);
        let prompt = gtk::Label::builder().label(one_paragraph(&schedule.prompt)).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        let target = if schedule.session_id.is_some() { schedule.title.clone() } else { format!("a new conversation in {}", schedule.cwd.file_name().map_or_else(|| schedule.cwd.to_string_lossy(), |n| n.to_string_lossy())) };
        let when = gtk::Label::builder().label(format!("{target} · {}", schedule.describe())).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        when.add_css_class("dim");
        when.add_css_class("small");
        texts.append(&prompt);
        texts.append(&when);
        row.append(&texts);
        let id = schedule.id;
        let button = |label: &str, tip: &str, act: fn() -> Act| {
            let button = gtk::Button::with_label(label);
            button.add_css_class("flat");
            button.add_css_class("small");
            button.set_tooltip_text(Some(tip));
            button.set_valign(gtk::Align::Center);
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                let handler = weak.upgrade().and_then(|o| o.on_schedule.borrow().clone());
                if let Some(handler) = handler {
                    handler((id, act()));
                }
            });
            button
        };
        row.append(&button("Run now", "Send it now (the schedule stays)", || Act::RunNow));
        if schedule.next.is_some() {
            row.append(&button(if schedule.paused { "Resume" } else { "Pause" }, "Pause or resume it", || Act::Pause));
        }
        row.append(&button("Remove", "Remove this schedule", || Act::Remove));
        row
    }
}

/// The first paragraph, Markdown marks dropped, on one line.
fn one_paragraph(text: &str) -> String {
    let first = text.trim().split("\n\n").next().unwrap_or("");
    first.replace(['#', '*', '`'], "").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ago(seconds: u64) -> String {
    match seconds {
        s if s < 60 => "just now".to_owned(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn replies_shrink_to_their_first_paragraph() {
        assert_eq!(super::one_paragraph("## Done\nAll **658** tests pass.\n\nNext: commit."), "Done All 658 tests pass.");
    }
}
