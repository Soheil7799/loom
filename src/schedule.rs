//! The dialog that schedules a prompt (the schedules themselves: `loom_core::schedule`).

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

pub use loom_core::schedule::*;

/// The dialog that makes one: the prompt, when, and whether it goes on in this conversation.
/// `done` gets (prompt, when, same conversation).
pub fn ask(parent: &gtk::Window, agent: &str, has_conversation: bool, done: impl Fn(String, String, bool) + 'static) {
    let window = gtk::Window::builder().title("Schedule a prompt").transient_for(parent).modal(true).default_width(520).resizable(false).build();
    window.add_css_class("schedule-dialog");
    window.add_css_class("loom");
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.add_css_class("schedule-body");
    let heading = gtk::Label::builder().label(format!("Schedule a prompt for {agent}")).xalign(0.0).wrap(true).build();
    heading.add_css_class("title-4");
    body.append(&heading);
    let prompt = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).top_margin(8).bottom_margin(8).left_margin(10).right_margin(10).build();
    prompt.add_css_class("schedule-prompt");
    let prompt_scroller = gtk::ScrolledWindow::builder().child(&prompt).min_content_height(110).hscrollbar_policy(gtk::PolicyType::Never).build();
    let prompt_label = gtk::Label::builder().label("Prompt").xalign(0.0).build();
    prompt_label.add_css_class("dim");
    body.append(&prompt_label);
    body.append(&prompt_scroller);
    let when_label = gtk::Label::builder().label("When").xalign(0.0).build();
    when_label.add_css_class("dim");
    body.append(&when_label);
    let when = gtk::Entry::builder().placeholder_text("09:00 · weekdays 9:30 · every 2h · in 20m · at 18:00").build();
    body.append(&when);
    let next = gtk::Label::builder().xalign(0.0).build();
    next.add_css_class("small");
    next.add_css_class("dim");
    body.append(&next);
    let same = gtk::CheckButton::builder().label("Continue this conversation (off: a new one each time)").active(has_conversation).sensitive(has_conversation).build();
    if !has_conversation {
        same.set_tooltip_text(Some("Nothing has been said here yet: each run starts a new conversation in this folder"));
    }
    body.append(&same);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Schedule");
    save.add_css_class("suggested-action");
    save.set_sensitive(false);
    buttons.append(&cancel);
    buttons.append(&save);
    body.append(&buttons);
    window.set_child(Some(&body));

    let check = {
        let (prompt, when, next, save) = (prompt.clone(), when.clone(), next.clone(), save.clone());
        Rc::new(move || {
            let buffer = prompt.buffer();
            let has_prompt = !buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).trim().is_empty();
            match next_run(&when.text(), &now()) {
                Ok((t, repeats)) => {
                    next.set_label(&format!("{} {}", if repeats { "First run" } else { "Runs" }, local(t, "%A %d %b, %H:%M")));
                    next.remove_css_class("error");
                    save.set_sensitive(has_prompt);
                }
                Err(e) => {
                    next.set_label(if when.text().is_empty() { "" } else { &e });
                    next.add_css_class("error");
                    save.set_sensitive(false);
                }
            }
        })
    };
    let c = check.clone();
    when.connect_changed(move |_| c());
    let c = check.clone();
    prompt.buffer().connect_changed(move |_| c());
    let w = window.clone();
    cancel.connect_clicked(move |_| w.close());
    let (w, p, wh, s) = (window.clone(), prompt.clone(), when.clone(), same.clone());
    let done = Rc::new(done);
    let go = move || {
        let buffer = p.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).trim().to_owned();
        if text.is_empty() || next_run(&wh.text(), &now()).is_err() {
            return;
        }
        done(text, wh.text().trim().to_owned(), s.is_active());
        w.close();
    };
    let go = Rc::new(go);
    let g = go.clone();
    save.connect_clicked(move |_| g());
    when.connect_activate(move |_| go());
    let keys = gtk::EventControllerKey::new();
    let w = window.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            w.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    window.present();
    prompt.grab_focus();
}
