//! A project's board in the window (loom-core's board.rs keeps the file): its lanes side by
//! side, a card's title, dates and the start of its description on it. Click a card to edit
//! or move it; drag it to another lane; + on a lane adds one. Drawn again whenever the file
//! changes (an agent's tools, Obsidian, a hand edit).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::{gdk, gio, glib, pango, prelude::*};
use serde_json::{Value, json};

type OnAgent = Rc<dyn Fn(u32)>;

pub struct BoardView {
    pub root: gtk::Box,
    title: gtk::Label,
    place: gtk::Label,
    lanes: gtk::Box,
    problem: gtk::Label,
    project: RefCell<PathBuf>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    me: RefCell<Weak<BoardView>>,
    /// The board as last drawn (an action and the file's monitor both reload it: drawn once).
    drawn: RefCell<Value>,
    /// An agent started on a card (Work on this): the window opens it.
    pub on_agent: RefCell<Option<OnAgent>>,
}

impl BoardView {
    pub fn new() -> Rc<BoardView> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("chat");
        root.add_css_class("board-page");
        let title = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        title.add_css_class("project-title");
        let place = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::Start).build();
        place.add_css_class("dim");
        place.add_css_class("small");
        let heading = gtk::Box::new(gtk::Orientation::Vertical, 2);
        heading.add_css_class("board-heading");
        heading.append(&title);
        heading.append(&place);
        let problem = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
        problem.add_css_class("board-problem");
        let lanes = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        lanes.add_css_class("board-lanes");
        let scroller = gtk::ScrolledWindow::builder().child(&lanes).vexpand(true).vscrollbar_policy(gtk::PolicyType::Never).build();
        root.append(&heading);
        root.append(&problem);
        root.append(&scroller);
        let view = Rc::new(BoardView { root, title, place, lanes, problem, project: RefCell::default(), monitor: RefCell::default(), me: RefCell::default(), drawn: RefCell::default(), on_agent: RefCell::default() });
        *view.me.borrow_mut() = Rc::downgrade(&view);
        view
    }

    /// The project's name as shown (renamed in the sidebar).
    pub fn set_name(&self, name: &str) {
        self.title.set_label(name);
    }

    /// `project`'s board (`name`: as the sidebar calls it), followed from now on.
    pub fn show(self: &Rc<Self>, project: &Path, name: &str) {
        *self.project.borrow_mut() = project.to_owned();
        self.title.set_label(name);
        let file = loom_core::board::path(project);
        // the file in a tooltip; under the name, how the lanes stand (draw)
        self.title.set_tooltip_text(Some(&file.to_string_lossy().replacen(&*glib::home_dir().to_string_lossy(), "~", 1)));
        self.place.set_label("");
        // Another project's lanes don't stay up under this one's name while it loads.
        while let Some(child) = self.lanes.first_child() {
            self.lanes.remove(&child);
        }
        self.drawn.take();
        // Changed by anyone (an agent, Obsidian): drawn again once the write is done.
        let monitor = gio::File::for_path(&file).monitor_file(gio::FileMonitorFlags::WATCH_MOVES, None::<&gio::Cancellable>).ok();
        if let Some(monitor) = &monitor {
            let weak = Rc::downgrade(self);
            monitor.connect_changed(move |_, _, _, event| {
                if matches!(event, gio::FileMonitorEvent::ChangesDoneHint | gio::FileMonitorEvent::Created | gio::FileMonitorEvent::MovedIn | gio::FileMonitorEvent::Renamed | gio::FileMonitorEvent::Deleted)
                    && let Some(view) = weak.upgrade()
                {
                    view.reload();
                }
            });
        }
        *self.monitor.borrow_mut() = monitor;
        self.reload();
    }

    fn reload(self: &Rc<Self>) {
        let project = self.project.borrow().clone();
        let weak = Rc::downgrade(self);
        crate::conn::command_result("board", "board", json!({ "project": project }), move |answer| {
            let Some(view) = weak.upgrade().filter(|v| *v.project.borrow() == project) else { return };
            match answer {
                Ok(board) => {
                    view.problem.set_visible(false);
                    view.draw(&board);
                }
                Err(err) => view.say(&err.to_string()),
            }
        });
    }

    fn say(&self, problem: &str) {
        self.problem.set_label(problem);
        self.problem.set_visible(true);
    }

    /// A board command on this project, then the board again (the file monitor may be late).
    fn act(self: &Rc<Self>, method: &str, mut params: Value) {
        params["project"] = json!(*self.project.borrow());
        let weak = Rc::downgrade(self);
        crate::conn::command_result("board", method, params, move |answer| {
            let Some(view) = weak.upgrade() else { return };
            if let Err(err) = answer {
                view.say(&err.to_string());
            }
            view.reload();
        });
    }

    fn draw(self: &Rc<Self>, board: &Value) {
        if *self.drawn.borrow() == *board {
            return;
        }
        *self.drawn.borrow_mut() = board.clone();
        // Each lane scrolled where it was, by name.
        let mut scrolled = std::collections::HashMap::new();
        while let Some(child) = self.lanes.first_child() {
            let name = child.widget_name().to_string();
            if let Some(scroller) = child.last_child().and_downcast::<gtk::ScrolledWindow>() {
                scrolled.insert(name, scroller.vadjustment().value());
            }
            self.lanes.remove(&child);
        }
        let lanes: Vec<String> = board["lanes"].as_array().into_iter().flatten().filter_map(|l| l["title"].as_str().map(str::to_owned)).collect();
        let summary: Vec<String> = board["lanes"].as_array().into_iter().flatten().map(|l| format!("{} {}", l["cards"].as_array().map_or(0, Vec::len), l["title"].as_str().unwrap_or_default().to_lowercase())).collect();
        self.place.set_label(&summary.join(" · "));
        for lane in board["lanes"].as_array().into_iter().flatten() {
            let column = self.lane(lane, &lanes);
            if let Some(&at) = scrolled.get(column.widget_name().as_str()).filter(|at| **at > 0.0) {
                keep_scrolled(&column, at);
            }
            self.lanes.append(&column);
        }
    }

    fn lane(self: &Rc<Self>, lane: &Value, all: &[String]) -> gtk::Box {
        let name = lane["title"].as_str().unwrap_or_default().to_owned();
        let cards = lane["cards"].as_array().cloned().unwrap_or_default();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.add_css_class("board-lane");
        column.set_widget_name(&name);
        // its own colour by name (a dot beside the name: never colour alone)
        column.add_css_class(&format!("lane-{}", name.to_lowercase().replace(' ', "-")));
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let dot = gtk::Label::new(Some("●"));
        dot.add_css_class("board-lane-dot");
        head.append(&dot);
        let label = gtk::Label::builder().label(&name).xalign(0.0).hexpand(true).build();
        label.add_css_class("board-lane-title");
        let count = gtk::Label::new(Some(&cards.len().to_string()));
        count.add_css_class("dim");
        count.add_css_class("small");
        let add = gtk::MenuButton::builder().icon_name("list-add-symbolic").tooltip_text(format!("A new card in {name}")).build();
        add.add_css_class("flat");
        add.set_popover(Some(&self.new_card(&name)));
        head.append(&label);
        head.append(&count);
        if name.eq_ignore_ascii_case("done") && !cards.is_empty() {
            let archive = gtk::Button::builder().icon_name("folder-download-symbolic").tooltip_text("Archive these cards: moved to .loom/board-archive.md").build();
            archive.add_css_class("flat");
            archive.update_property(&[gtk::accessible::Property::Label("Archive Done's cards")]);
            let weak = Rc::downgrade(self);
            archive.connect_clicked(move |button| {
                button.set_sensitive(false);
                if let Some(view) = weak.upgrade() {
                    view.act("board_archive", json!({}));
                }
            });
            head.append(&archive);
        }
        head.append(&add);
        column.append(&head);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        for card in &cards {
            list.append(&self.card(card, all, &name));
        }
        if cards.is_empty() {
            let empty = gtk::Label::builder().label("Nothing here yet").tooltip_text("Drop a card here, or press + above").build();
            empty.add_css_class("board-empty");
            list.append(&empty);
        }
        let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        column.append(&scroller);
        // A card dropped here moves to the end of this lane.
        let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let weak = Rc::downgrade(self);
        target.connect_drop(move |_, value, _, _| {
            let card = value.get::<String>().ok().and_then(|v| v.strip_prefix("loom-card:")?.parse::<u64>().ok());
            match (weak.upgrade(), card) {
                (Some(view), Some(card)) => {
                    view.act("board_move", json!({ "card": card, "lane": name }));
                    true
                }
                _ => false,
            }
        });
        column.add_controller(target);
        column
    }

    fn card(self: &Rc<Self>, card: &Value, lanes: &[String], lane: &str) -> gtk::Button {
        let id = card["id"].as_u64().unwrap_or(0);
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let title = gtk::Label::builder().label(card["title"].as_str().unwrap_or_default()).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build();
        title.add_css_class("board-card-title");
        inner.append(&title);
        // Done: its title alone (the rest is in its editor). Else two lines of its description
        // (as one paragraph: `lines` caps each) and a quiet line of when and who.
        let done = card["done"] == true;
        let body = card["body"].as_str().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
        if !done && !body.is_empty() {
            let excerpt = gtk::Label::builder().label(&body).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).lines(2).ellipsize(pango::EllipsizeMode::End).build();
            excerpt.add_css_class("dim");
            excerpt.add_css_class("small");
            inner.append(&excerpt);
        }
        if !done {
            let when = [("done", "finished"), ("started", "started"), ("created", "created")].iter().find_map(|(word, key)| card[*key].as_str().filter(|t| !t.is_empty()).map(|t| format!("{word} {}", when(t))));
            let by = card["by"].as_str().filter(|b| !b.is_empty() && *b != "you").map(|b| format!("by {b}"));
            let words = [when, by].into_iter().flatten().map(|w| glib::markup_escape_text(&w).to_string()).collect::<Vec<_>>();
            let markup = std::iter::once(format!("<span font_family=\"JetBrains Mono\" alpha=\"55%\">c{id}</span>")).chain(words).collect::<Vec<_>>().join(" · ");
            let meta = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            meta.set_markup(&markup);
            meta.add_css_class("dim");
            meta.add_css_class("small");
            meta.add_css_class("board-card-meta");
            inner.append(&meta);
        }
        // A plain button (a MenuButton loses wrapped text's height): it opens its popover.
        let button = gtk::Button::builder().child(&inner).build();
        button.add_css_class("board-card");
        if card["done"] == true {
            button.add_css_class("done");
        }
        let editor = self.editor(card, lanes);
        editor.set_parent(&button);
        button.connect_clicked(move |_| editor.popup());
        // (unparented with the button, or GTK warns when the board is drawn again)
        button.connect_destroy(|b| {
            if let Some(popover) = b.last_child().and_downcast::<gtk::Popover>() {
                popover.unparent();
            }
        });
        let source = gtk::DragSource::builder().actions(gdk::DragAction::MOVE).build();
        source.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&format!("loom-card:{id}").to_value())));
        let shown = button.clone();
        source.connect_drag_begin(move |source, _| source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&shown))), 12, 12));
        button.add_controller(source);
        // Another card dropped on this one goes before it, in this lane.
        let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let (weak, lane) = (Rc::downgrade(self), lane.to_owned());
        target.connect_drop(move |_, value, _, _| {
            let dropped = value.get::<String>().ok().and_then(|v| v.strip_prefix("loom-card:")?.parse::<u64>().ok());
            match (weak.upgrade(), dropped) {
                (Some(view), Some(dropped)) if dropped != id => {
                    view.act("board_move", json!({ "card": dropped, "lane": lane, "before": id }));
                    true
                }
                _ => false,
            }
        });
        button.add_controller(target);
        button
    }

    /// The popover that adds a card to `lane`: a title, and a description if wanted.
    fn new_card(self: &Rc<Self>, lane: &str) -> gtk::Popover {
        let (title, body, box_) = fields("", "");
        let add = gtk::Button::with_label("Add");
        add.add_css_class("suggested-action");
        box_.append(&add);
        let popover = gtk::Popover::builder().child(&box_).build();
        let (weak, lane, shut) = (Rc::downgrade(self), lane.to_owned(), popover.clone());
        let go = {
            let (title, body) = (title.clone(), body.clone());
            Rc::new(move || {
                let Some(view) = weak.upgrade() else { return };
                if title.text().trim().is_empty() {
                    return;
                }
                view.act("board_add", json!({ "lane": lane, "title": title.text().as_str(), "body": text_of(&body) }));
                title.set_text("");
                body.buffer().set_text("");
                shut.popdown();
            })
        };
        let again = go.clone();
        add.connect_clicked(move |_| go());
        title.connect_activate(move |_| again());
        popover
    }

    /// A card's popover: its title and description to edit, the lanes to move it to, delete.
    fn editor(self: &Rc<Self>, card: &Value, lanes: &[String]) -> gtk::Popover {
        let id = card["id"].as_u64().unwrap_or(0);
        let (title, body, box_) = fields(card["title"].as_str().unwrap_or_default(), card["body"].as_str().unwrap_or_default());
        let dates = ["created", "started", "finished"].iter().filter_map(|k| card[*k].as_str().filter(|t| !t.is_empty()).map(|t| format!("{} {t}", if *k == "finished" { "done" } else { k }))).collect::<Vec<_>>().join(" · ");
        if !dates.is_empty() {
            let label = gtk::Label::builder().label(&dates).xalign(0.0).build();
            label.add_css_class("dim");
            label.add_css_class("small");
            box_.append(&label);
        }
        let popover = gtk::Popover::builder().child(&box_).build();
        let moves = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for lane in lanes {
            let button = gtk::Button::with_label(lane);
            button.add_css_class("flat");
            button.set_tooltip_text(Some(&format!("Move it to {lane}")));
            let (weak, lane, shut) = (Rc::downgrade(self), lane.clone(), popover.clone());
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.act("board_move", json!({ "card": id, "lane": lane }));
                }
                shut.popdown();
            });
            moves.append(&button);
        }
        box_.append(&moves);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let delete = gtk::Button::with_label("Delete");
        delete.add_css_class("destructive-action");
        let save = gtk::Button::builder().label("Save").hexpand(true).halign(gtk::Align::End).build();
        save.add_css_class("suggested-action");
        actions.append(&delete);
        if card["done"] != true {
            let work = gtk::Button::with_label("Work on this");
            work.set_tooltip_text(Some("An agent starts on it: the card is its first message, and it moves to Doing"));
            let (weak, shut) = (Rc::downgrade(self), popover.clone());
            work.connect_clicked(move |work| {
                let Some(view) = weak.upgrade() else { return };
                work.set_sensitive(false);
                work.set_label("Starting…");
                let (weak, work, shut) = (Rc::downgrade(&view), work.clone(), shut.clone());
                let project = view.project.borrow().clone();
                crate::conn::command_result("board", "board_work", json!({ "project": project, "card": id }), move |answer| {
                    work.set_sensitive(true);
                    work.set_label("Work on this");
                    let Some(view) = weak.upgrade() else { return };
                    match answer {
                        Ok(answer) => {
                            shut.popdown();
                            view.reload();
                            let told = view.on_agent.borrow().clone();
                            if let (Some(told), Some(agent)) = (told, answer["agent"].as_str().and_then(|a| a.parse().ok())) {
                                told(agent);
                            }
                        }
                        Err(err) => view.say(&err.to_string()),
                    }
                });
            });
            actions.append(&work);
        }
        actions.append(&save);
        box_.append(&actions);
        let (weak, shut) = (Rc::downgrade(self), popover.clone());
        let (t, b) = (title.clone(), body.clone());
        save.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.act("board_edit", json!({ "card": id, "title": t.text().as_str(), "body": text_of(&b) }));
            }
            shut.popdown();
        });
        let (weak, shut) = (Rc::downgrade(self), popover.clone());
        delete.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.act("board_remove", json!({ "card": id }));
            }
            shut.popdown();
        });
        popover
    }
}

/// A card's time ("YYYY-MM-DD HH:MM") as read: "today 13:15", "yesterday", "9 Nov".
fn when(at: &str) -> String {
    let (date, time) = at.split_once(' ').unwrap_or((at, ""));
    let day = |offset: i32| glib::DateTime::now_local().and_then(|d| d.add_days(offset)).and_then(|d| d.format("%Y-%m-%d")).map(|d| d.to_string()).unwrap_or_default();
    if date == day(0) {
        return format!("today {time}");
    }
    if date == day(-1) {
        return "yesterday".to_owned();
    }
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut parts = date.split('-').skip(1);
    match (parts.next().and_then(|m| m.parse::<usize>().ok()), parts.next().and_then(|d| d.parse::<u32>().ok())) {
        (Some(m @ 1..=12), Some(d)) => format!("{d} {}", months[m - 1]),
        _ => at.to_owned(),
    }
}

/// A card's title and description fields, in a box for a popover.
fn fields(title: &str, body: &str) -> (gtk::Entry, gtk::TextView, gtk::Box) {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 8);
    box_.add_css_class("board-editor");
    let entry = gtk::Entry::builder().text(title).placeholder_text("Title").build();
    let text = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).build();
    text.buffer().set_text(body);
    text.add_css_class("board-body");
    let scroller = gtk::ScrolledWindow::builder().child(&text).min_content_height(120).max_content_height(320).propagate_natural_height(true).width_request(380).build();
    box_.append(&entry);
    box_.append(&scroller);
    (entry, text, box_)
}

fn text_of(view: &gtk::TextView) -> String {
    let buffer = view.buffer();
    buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
}

/// `column`'s lane scrolled to `at` once its cards are laid out far enough (until then its
/// scroller can't go there).
fn keep_scrolled(column: &gtk::Box, at: f64) {
    let Some(scroller) = column.last_child().and_downcast::<gtk::ScrolledWindow>() else { return };
    let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
    let once = handler.clone();
    let id = scroller.vadjustment().connect_changed(move |adjustment| {
        if adjustment.upper() - adjustment.page_size() >= at {
            adjustment.set_value(at);
            if let Some(id) = once.borrow_mut().take() {
                adjustment.disconnect(id);
            }
        }
    });
    *handler.borrow_mut() = Some(id);
}
