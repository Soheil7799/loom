//! Prompts sent on a schedule: "every day at 9 ask the ERP agent for a status", "in 20m
//! check the build". loom sends them while it runs; a run missed while it was closed is not
//! made up (the next one comes as usual).

use std::path::PathBuf;

use jiff::{ToSpan, Zoned, tz::TimeZone};
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

pub fn local(t: i64, format: &str) -> String {
    jiff::Timestamp::from_second(t).map(|t| t.to_zoned(TimeZone::system()).strftime(format).to_string()).unwrap_or_default()
}

pub fn now() -> Zoned {
    Zoned::now()
}

/// The next run for `when` after `now`, and whether it repeats; or what is wrong with it.
pub fn next_run(when: &str, now: &Zoned) -> Result<(i64, bool), String> {
    let text = when.trim().to_lowercase();
    let words: Vec<&str> = text.split_whitespace().collect();
    let rest = |n: usize| words[n.min(words.len())..].join(" ");
    let at = |time: &str, weekdays: bool| -> Result<i64, String> {
        let (h, m) = clock(time).ok_or_else(|| format!("\"{time}\" is not a time (try 09:00)"))?;
        let mut day = now.date().at(h as i8, m as i8, 0, 0).to_zoned(now.time_zone().clone()).map_err(|e| e.to_string())?;
        while day.timestamp() <= now.timestamp() || (weekdays && day.weekday().to_monday_one_offset() > 5) {
            day = day.checked_add(1.day()).map_err(|e| e.to_string())?;
        }
        Ok(day.timestamp().as_second())
    };
    match words.as_slice() {
        [] => Err("When? e.g. 09:00, weekdays 9:30, every 2h, in 20m, at 18:00".into()),
        ["every", "weekday" | "weekdays", ..] => Ok((at(&rest(2).replace("at ", ""), true)?, true)),
        ["weekdays", ..] => Ok((at(&rest(1).replace("at ", ""), true)?, true)),
        ["every", "day", ..] => Ok((at(&rest(2).replace("at ", ""), false)?, true)),
        ["daily", ..] => Ok((at(&rest(1).replace("at ", ""), false)?, true)),
        ["every", ..] => Ok((now.timestamp().as_second() + duration(&rest(1))?, true)),
        ["in", ..] => Ok((now.timestamp().as_second() + duration(&rest(1))?, false)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i16, mo: i8, d: i8, h: i8, mi: i8) -> Zoned {
        jiff::civil::date(y, mo, d).at(h, mi, 0, 0).to_zoned(TimeZone::system()).unwrap()
    }

    trait Unix {
        fn to_unix(&self) -> i64;
    }
    impl Unix for Zoned {
        fn to_unix(&self) -> i64 {
            self.timestamp().as_second()
        }
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
