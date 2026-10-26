//! Every open agent on one page, by what it asks of you: needs you, working, done but not
//! seen, then the quiet ones (smaller); each group the last used first. And, in a column on
//! the right, the prompts scheduled to be sent.

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
    /// When it was last used (Unix seconds, 0 unknown).
    pub used: u64,
    pub scheduled: usize,
}

pub enum Act {
    RunNow,
    Pause,
    Remove,
}

type Handler<T> = RefCell<Option<Rc<dyn Fn(T)>>>;

pub struct Overview {
    pub root: gtk::Box,
    counts: gtk::Label,
    /// The groups (a heading and its cards each).
    groups: gtk::Box,
    /// Each group's cards and the agents they stand for, in order.
    flows: RefCell<Vec<(gtk::FlowBox, Vec<u32>)>>,
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
        let groups = gtk::Box::new(gtk::Orientation::Vertical, 18);
        page.set_valign(gtk::Align::Start);
        page.append(&groups);
        let cards_scroller = gtk::ScrolledWindow::builder().child(&page).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).hexpand(true).build();
        // The scheduled prompts: a column of their own on the right, like a sidebar.
        let side = gtk::Box::new(gtk::Orientation::Vertical, 8);
        side.add_css_class("overview-side");
        let heading = gtk::Label::builder().label("SCHEDULED PROMPTS").xalign(0.0).build();
        heading.add_css_class("overview-heading");
        side.append(&heading);
        let schedules = gtk::Box::new(gtk::Orientation::Vertical, 6);
        side.append(&schedules);
        let side_scroller = gtk::ScrolledWindow::builder().child(&side).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).width_request(320).build();
        side_scroller.add_css_class("overview-side-scroller");
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&cards_scroller);
        root.append(&side_scroller);
        root.add_css_class("chat");
        let overview = Rc::new(Overview {
            root,
            counts,
            groups,
            flows: RefCell::default(),
            schedules,
            on_open: RefCell::default(),
            on_schedule_for: RefCell::default(),
            on_schedule: RefCell::default(),
        });
        overview
    }

    /// The agent whose card has the keyboard.
    pub fn focused_id(&self) -> Option<u32> {
        let root = self.groups.root()?;
        let focus = gtk::prelude::GtkWindowExt::focus(root.downcast_ref::<gtk::Window>()?)?;
        let child = focus.ancestor(gtk::FlowBoxChild::static_type()).and_downcast::<gtk::FlowBoxChild>()?;
        let flow = child.parent().and_downcast::<gtk::FlowBox>()?;
        let flows = self.flows.borrow();
        let (_, ids) = flows.iter().find(|(f, _)| *f == flow)?;
        ids.get(child.index() as usize).copied()
    }

    /// Gives the keyboard back to that agent's card (after a refill).
    pub fn focus_id(&self, id: Option<u32>) {
        let Some(id) = id else { return };
        for (flow, ids) in self.flows.borrow().iter() {
            if let Some(child) = ids.iter().position(|i| *i == id).and_then(|i| flow.child_at_index(i as i32)) {
                child.grab_focus();
                return;
            }
        }
    }

    /// The first card takes the keyboard.
    pub fn grab_focus(&self) {
        if let Some(first) = self.flows.borrow().first().and_then(|(flow, _)| flow.child_at_index(0)) {
            first.grab_focus();
        }
    }

    /// A group: its heading over its cards (compact: one line each).
    fn group(self: &Rc<Self>, heading: &str, cards: &[&Card], compact: bool) {
        if cards.is_empty() {
            return;
        }
        let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let label = gtk::Label::builder().label(format!("{heading}  {}", cards.len())).xalign(0.0).build();
        label.add_css_class("overview-heading");
        section.append(&label);
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            // Each card as tall as what it shows: rows of any height.
            .homogeneous(false)
            .min_children_per_line(1)
            .max_children_per_line(if compact { 5 } else { 4 })
            .column_spacing(10)
            .row_spacing(if compact { 6 } else { 10 })
            .activate_on_single_click(true)
            .build();
        flow.add_css_class("overview-cards");
        flow.set_vexpand(false);
        for card in cards {
            flow.insert(&self.card(card, compact), -1);
        }
        let ids: Vec<u32> = cards.iter().map(|c| c.id).collect();
        let weak = Rc::downgrade(self);
        let mine = ids.clone();
        flow.connect_child_activated(move |_, child| {
            let Some(overview) = weak.upgrade() else { return };
            let handler = overview.on_open.borrow().clone();
            if let (Some(id), Some(handler)) = (mine.get(child.index() as usize).copied(), handler) {
                handler(id);
            }
        });
        section.append(&flow);
        self.groups.append(&section);
        self.flows.borrow_mut().push((flow, ids));
    }

    pub fn fill(self: &Rc<Self>, mut cards: Vec<Card>, schedules: &[Schedule]) {
        // The last used first (never used: by name, last).
        cards.sort_by(|a, b| b.used.cmp(&a.used).then_with(|| a.title.cmp(&b.title)));
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
        while let Some(child) = self.groups.first_child() {
            self.groups.remove(&child);
        }
        self.flows.borrow_mut().clear();
        let needs = |c: &&Card| matches!(c.status, Status::Blocked | Status::Failed);
        let working = |c: &&Card| matches!(c.status, Status::Working | Status::Starting);
        let unseen = |c: &&Card| c.status == Status::Idle && c.unseen;
        let pick = |f: &dyn Fn(&&Card) -> bool| cards.iter().filter(|c| f(c)).collect::<Vec<_>>();
        self.group("NEEDS YOU", &pick(&needs), false);
        self.group("WORKING", &pick(&working), false);
        self.group("DONE, NOT SEEN", &pick(&unseen), false);
        self.group("QUIET", &pick(&|c| !needs(c) && !working(c) && !unseen(c)), true);

        while let Some(child) = self.schedules.first_child() {
            self.schedules.remove(&child);
        }
        if schedules.is_empty() {
            let none = gtk::Label::builder()
                .label("None yet. On a card, Schedule… sends that agent a prompt at a time you pick: every day at 9, every 2h, in 20m.")
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

    fn card(self: &Rc<Self>, card: &Card, compact: bool) -> gtk::Box {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("overview-card");
        if compact {
            root.add_css_class("compact");
        }
        // As tall as what it shows (not as the tallest card of its row).
        root.set_valign(gtk::Align::Start);
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
            Status::Idle | Status::Suspended if card.used > 0 => ago(crate::state::now().saturating_sub(card.used)),
            other => other.describe().to_owned(),
        }));
        state.add_css_class("small");
        state.add_css_class(if card.status == Status::Blocked { "accent" } else { "dim" });
        top.append(&dot);
        top.append(&title);
        if compact {
            let project = gtk::Label::new(Some(&card.project));
            project.add_css_class("dim");
            project.add_css_class("small");
            top.append(&project);
        }
        top.append(&state);
        root.append(&top);
        if compact {
            return root;
        }
        // Where it works, how many prompts wait for it, and Schedule…: one line.
        let place = [card.project.as_str(), card.place.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        let place = gtk::Label::builder().label(place).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
        place.add_css_class("dim");
        place.add_css_class("small");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.append(&place);
        if card.scheduled > 0 {
            let scheduled = gtk::Label::new(Some(&format!("⏱ {}", card.scheduled)));
            scheduled.add_css_class("dim");
            scheduled.add_css_class("small");
            scheduled.set_tooltip_text(Some("Prompts scheduled for it"));
            line.append(&scheduled);
        }
        let plan = gtk::Button::with_label("Schedule…");
        plan.add_css_class("flat");
        plan.add_css_class("small");
        plan.add_css_class("schedule-button");
        plan.set_tooltip_text(Some("Send this agent a prompt at a time you pick"));
        let weak = Rc::downgrade(self);
        let id = card.id;
        plan.connect_clicked(move |_| {
            let handler = weak.upgrade().and_then(|o| o.on_schedule_for.borrow().clone());
            if let Some(handler) = handler {
                handler(id);
            }
        });
        line.append(&plan);
        root.append(&line);
        for question in card.waiting.iter().take(2) {
            let label = gtk::Label::builder().label(format!("Wants to: {question}")).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            label.add_css_class("card-waiting");
            root.append(&label);
        }
        // What it said last (a suspended one has not been read in yet: nothing then).
        let reply = card.reply.as_deref().map(one_paragraph).unwrap_or_default();
        if reply.is_empty() {
            return root;
        }
        let text = gtk::Label::builder()
            .label(reply.as_str())
            .xalign(0.0)
            .yalign(0.0)
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(pango::EllipsizeMode::End)
            .width_chars(22)
            .max_width_chars(32)
            .build();
        text.add_css_class("card-reply");
        root.append(&text);
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

/// "just now", "5m ago", "3h ago", "2d ago": loom's one way of saying it.
pub fn ago(seconds: u64) -> String {
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
