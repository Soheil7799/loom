//! Prompts sent on a schedule: "every day at 9 ask the ERP agent for a status", "in 20m
//! check the build". loom sends them while it runs; a run missed while it was closed is not
//! made up (the next one comes as usual).

use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    pub id: u64,
    /// Where it runs: the conversation (None: a new one each time, in `cwd`).
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    /// The agent's title when it was made, to show.
    pub title: String,
    pub prompt: String,
    /// As typed: "09:00", "weekdays 9:30", "every 2h", "in 20m", "at 18:00".
    pub when: String,
    /// The next run (unix seconds); None once a one-off has run.
    pub next: Option<i64>,
    pub paused: bool,
    pub last_run: Option<i64>,
}

impl Schedule {
    /// "every day 09:00 · next Tue 09:00".
    pub fn describe(&self) -> String {
        let next = match (self.paused, self.next) {
            (true, _) => "paused".to_owned(),
            (false, Some(t)) => format!("next {}", local(t, "%a %H:%M")),
            (false, None) => match self.last_run {
                Some(t) => format!("ran {}", local(t, "%a %H:%M")),
                None => "done".to_owned(),
            },
        };
        format!("{} · {next}", self.when)
    }
}

fn local(t: i64, format: &str) -> String {
    glib::DateTime::from_unix_local(t).ok().and_then(|d| d.format(format).ok()).map(|s| s.to_string()).unwrap_or_default()
}

pub fn now() -> glib::DateTime {
    glib::DateTime::now_local().expect("local time")
}

/// The next run for `when` after `now`, and whether it repeats; or what is wrong with it.
pub fn next_run(when: &str, now: &glib::DateTime) -> Result<(i64, bool), String> {
    let text = when.trim().to_lowercase();
    let words: Vec<&str> = text.split_whitespace().collect();
    let rest = |n: usize| words[n.min(words.len())..].join(" ");
    let at = |time: &str, weekdays: bool| -> Result<i64, String> {
        let (h, m) = clock(time).ok_or_else(|| format!("\"{time}\" is not a time (try 09:00)"))?;
        let mut day = glib::DateTime::from_local(now.year(), now.month(), now.day_of_month(), h, m, 0.0).map_err(|e| e.to_string())?;
        while day.to_unix() <= now.to_unix() || (weekdays && day.day_of_week() > 5) {
            day = day.add_days(1).map_err(|e| e.to_string())?;
        }
        Ok(day.to_unix())
    };
    match words.as_slice() {
        [] => Err("When? e.g. 09:00, weekdays 9:30, every 2h, in 20m, at 18:00".into()),
        ["every", "weekday" | "weekdays", ..] => Ok((at(&rest(2).replace("at ", ""), true)?, true)),
        ["weekdays", ..] => Ok((at(&rest(1).replace("at ", ""), true)?, true)),
        ["every", "day", ..] => Ok((at(&rest(2).replace("at ", ""), false)?, true)),
        ["daily", ..] => Ok((at(&rest(1).replace("at ", ""), false)?, true)),
        ["every", ..] => Ok((now.to_unix() + duration(&rest(1))?, true)),
        ["in", ..] => Ok((now.to_unix() + duration(&rest(1))?, false)),
        ["at" | "once", ..] => Ok((at(&rest(1).replace("at ", ""), false)?, false)),
        _ => Ok((at(&text, false)?, true)),
    }
}

/// "9", "09:00", "9.30", "18:05".
fn clock(text: &str) -> Option<(i32, i32)> {
    let text = text.trim();
    let (h, m) = text.split_once([':', '.']).unwrap_or((text, "0"));
    let (h, m) = (h.parse::<i32>().ok()?, m.parse::<i32>().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some((h, m))
}

/// "30m", "2h", "90 min", "1 hour" in seconds.
fn duration(text: &str) -> Result<i64, String> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let split = compact.find(|c: char| !c.is_ascii_digit()).unwrap_or(compact.len());
    let (number, unit) = compact.split_at(split);
    let n: i64 = number.parse().map_err(|_| format!("\"{text}\" is not a length of time (try 30m or 2h)"))?;
    let unit = match unit {
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86400,
        _ => return Err(format!("\"{text}\": minutes (m) or hours (h)?")),
    };
    if n == 0 {
        return Err("Longer than that, please".into());
    }
    n.checked_mul(unit).filter(|s| *s <= 366 * 86400).ok_or_else(|| "At most a year, please".to_owned())
}

pub fn to_json(s: &Schedule) -> Value {
    json!({
        "id": s.id, "cwd": s.cwd.to_string_lossy(), "sessionId": s.session_id, "title": s.title,
        "prompt": s.prompt, "when": s.when, "next": s.next, "paused": s.paused, "lastRun": s.last_run,
    })
}

pub fn from_json(v: &Value) -> Option<Schedule> {
    Some(Schedule {
        id: v["id"].as_u64()?,
        cwd: PathBuf::from(v["cwd"].as_str()?),
        session_id: v["sessionId"].as_str().map(str::to_owned),
        title: v["title"].as_str().unwrap_or("").to_owned(),
        prompt: v["prompt"].as_str()?.to_owned(),
        when: v["when"].as_str()?.to_owned(),
        next: v["next"].as_i64(),
        paused: v["paused"].as_bool().unwrap_or(false),
        last_run: v["lastRun"].as_i64(),
    })
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: i32, d: i32, h: i32, mi: i32) -> glib::DateTime {
        glib::DateTime::from_local(y, mo, d, h, mi, 0.0).unwrap()
    }

    #[test]
    fn times_of_day_repeat_daily() {
        // Sunday 25 Oct 2026, 10:00.
        let now = at(2026, 10, 25, 10, 0);
        assert_eq!(next_run("09:00", &now).unwrap(), (at(2026, 10, 26, 9, 0).to_unix(), true));
        assert_eq!(next_run("every day at 18:30", &now).unwrap(), (at(2026, 10, 25, 18, 30).to_unix(), true));
        assert_eq!(next_run("at 18", &now).unwrap(), (at(2026, 10, 25, 18, 0).to_unix(), false));
    }

    #[test]
    fn weekdays_skip_the_weekend() {
        let saturday = at(2026, 10, 24, 12, 0);
        assert_eq!(next_run("weekdays 9:30", &saturday).unwrap().0, at(2026, 10, 26, 9, 30).to_unix());
    }

    #[test]
    fn intervals_and_one_offs() {
        let now = at(2026, 10, 25, 10, 0);
        assert_eq!(next_run("every 2h", &now).unwrap(), (now.to_unix() + 7200, true));
        assert_eq!(next_run("in 20 min", &now).unwrap(), (now.to_unix() + 1200, false));
        assert!(next_run("every 0m", &now).is_err());
        assert!(next_run("tomorrowish", &now).is_err());
        assert!(next_run("25:00", &now).is_err());
        assert!(next_run("every 9999999999999999h", &now).is_err());
    }

    #[test]
    fn schedules_read_back() {
        let s = Schedule {
            id: 3, cwd: "/p".into(), session_id: Some("s".into()), title: "ERP".into(), prompt: "status?".into(),
            when: "09:00".into(), next: Some(5), paused: true, last_run: None,
        };
        assert_eq!(from_json(&to_json(&s)), Some(s));
    }
}
