//! A project's board in the window (loom-core's board.rs keeps the file): its lanes side by
//! side, a card's title, dates and the start of its description on it. Click a card to edit
//! or move it; drag it to another lane; + on a lane adds one. Drawn again whenever the file
//! changes (an agent's tools, Obsidian, a hand edit).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::{gdk, gio, glib, pango, prelude::*};
use serde_json::{Value, json};

pub struct BoardView {
    pub root: gtk::Box,
    title: gtk::Label,
    place: gtk::Label,
    lanes: gtk::Box,
    problem: gtk::Label,
    project: RefCell<PathBuf>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    me: RefCell<Weak<BoardView>>,
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
        let view = Rc::new(BoardView { root, title, place, lanes, problem, project: RefCell::default(), monitor: RefCell::default(), me: RefCell::default() });
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
        self.place.set_label(&file.to_string_lossy().replacen(&*glib::home_dir().to_string_lossy(), "~", 1));
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
        while let Some(child) = self.lanes.first_child() {
            self.lanes.remove(&child);
        }
        let lanes: Vec<String> = board["lanes"].as_array().into_iter().flatten().filter_map(|l| l["title"].as_str().map(str::to_owned)).collect();
        for lane in board["lanes"].as_array().into_iter().flatten() {
            self.lanes.append(&self.lane(lane, &lanes));
        }
    }

    fn lane(self: &Rc<Self>, lane: &Value, all: &[String]) -> gtk::Box {
        let name = lane["title"].as_str().unwrap_or_default().to_owned();
        let cards = lane["cards"].as_array().cloned().unwrap_or_default();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.add_css_class("board-lane");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
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
        head.append(&add);
        column.append(&head);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        for card in &cards {
            list.append(&self.card(card, all));
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

    fn card(self: &Rc<Self>, card: &Value, lanes: &[String]) -> gtk::Button {
        let id = card["id"].as_u64().unwrap_or(0);
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let title = gtk::Label::builder().label(card["title"].as_str().unwrap_or_default()).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build();
        title.add_css_class("board-card-title");
        inner.append(&title);
        let body = card["body"].as_str().unwrap_or_default();
        if !body.is_empty() {
            let excerpt = gtk::Label::builder().label(body).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).lines(3).ellipsize(pango::EllipsizeMode::End).build();
            excerpt.add_css_class("dim");
            excerpt.add_css_class("small");
            inner.append(&excerpt);
        }
        let when = [("done", "finished"), ("started", "started"), ("created", "created")].iter().find_map(|(word, key)| card[*key].as_str().filter(|t| !t.is_empty()).map(|t| format!("{word} {t}")));
        let by = card["by"].as_str().filter(|b| !b.is_empty()).map(|b| format!("by {b}"));
        let line = [Some(format!("c{id}")), when, by].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let meta = gtk::Label::builder().label(&line).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        meta.add_css_class("dim");
        meta.add_css_class("small");
        meta.add_css_class("board-card-meta");
        inner.append(&meta);
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
