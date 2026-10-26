//! The user's settings: kept in ~/.config/loom/settings.json, read once, written on change,
//! and announced to whoever listens (the window restyles, terminals refont…).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Idle agents nobody looks at stop after this many minutes (0: never).
    pub idle_minutes: u64,
    pub notify_questions: bool,
    pub notify_done: bool,
    /// Ask before quitting while agents work.
    pub confirm_quit: bool,
    /// The agent program (empty: the one loom installs, else claude-agent-acp on PATH).
    pub adapter: String,
    /// Permission mode new agents start in (empty: Claude Code's own default).
    pub default_mode: String,
    pub show_thinking: bool,
    /// Chat text size, as a factor of the normal one.
    pub text_scale: f64,
    pub terminal_font: String,
    /// Close the scratch terminal when focus leaves it.
    pub scratch_autohide: bool,
    /// The sidebar puts agents that need you first (else in the order they started).
    pub sort_by_urgency: bool,
    /// The Telegram bot: off unless switched on, and obeys only `telegram_user`.
    pub telegram_enabled: bool,
    pub telegram_token: String,
    /// The one Telegram account the bot answers (0: none yet).
    pub telegram_user: i64,
    /// Everything loom does, written to the debug log.
    pub debug_log: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            idle_minutes: 15,
            notify_questions: true,
            notify_done: true,
            confirm_quit: true,
            adapter: String::new(),
            default_mode: String::new(),
            show_thinking: true,
            text_scale: 1.0,
            terminal_font: crate::term::FONT.to_owned(),
            scratch_autohide: false,
            sort_by_urgency: true,
            telegram_enabled: false,
            telegram_token: String::new(),
            telegram_user: 0,
            debug_log: false,
        }
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Settings>> = const { RefCell::new(None) };
    static FILE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static LISTENERS: RefCell<Vec<Rc<dyn Fn(&Settings)>>> = const { RefCell::new(Vec::new()) };
}

/// Reads the settings for `app_id` (a test instance keeps its own file).
pub fn init(app_id: &str) {
    let name = match app_id.rsplit('.').next() {
        Some("Loom") | None => "settings.json".to_owned(),
        Some(suffix) => format!("settings-{suffix}.json"),
    };
    let file = glib::user_config_dir().join("loom").join(name);
    let settings = std::fs::read_to_string(&file).map(|t| parse(&t)).unwrap_or_default();
    FILE.with(|f| *f.borrow_mut() = Some(file));
    CURRENT.with(|c| *c.borrow_mut() = Some(settings));
}

pub fn get() -> Settings {
    CURRENT.with(|c| c.borrow().clone().unwrap_or_default())
}

/// Changes the settings, saves them and tells the listeners.
pub fn update(change: impl FnOnce(&mut Settings)) {
    let mut settings = get();
    let before = settings.clone();
    change(&mut settings);
    if settings == before {
        return;
    }
    CURRENT.with(|c| *c.borrow_mut() = Some(settings.clone()));
    crate::log::debug!("settings", "changed: {}", {
        let text = render(&settings).replace('\n', " ");
        if settings.telegram_token.is_empty() { text } else { text.replace(&settings.telegram_token, "…") }
    });
    if let Some(file) = FILE.with(|f| f.borrow().clone()) {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let temporary = file.with_extension("json.tmp");
        // It may hold the bot token: readable by its owner only.
        // A left-over one could be readable by others: made anew.
        let _ = std::fs::remove_file(&temporary);
        let written = {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)
                .and_then(|mut f| f.write_all(render(&settings).as_bytes()))
        };
        if written.is_ok() {
            let _ = std::fs::rename(&temporary, &file);
        }
    }
    let listeners = LISTENERS.with(|l| l.borrow().clone());
    for listener in listeners {
        listener(&settings);
    }
}

pub fn on_change(f: impl Fn(&Settings) + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Rc::new(f)));
}

fn parse(text: &str) -> Settings {
    let d = Settings::default();
    let Ok(v) = serde_json::from_str::<Value>(text) else { return d };
    let string = |key: &str, default: &str| v[key].as_str().unwrap_or(default).to_owned();
    let flag = |key: &str, default: bool| v[key].as_bool().unwrap_or(default);
    Settings {
        idle_minutes: v["idleMinutes"].as_u64().unwrap_or(d.idle_minutes),
        notify_questions: flag("notifyQuestions", d.notify_questions),
        notify_done: flag("notifyDone", d.notify_done),
        confirm_quit: flag("confirmQuit", d.confirm_quit),
        adapter: string("adapter", &d.adapter),
        default_mode: string("defaultMode", &d.default_mode),
        show_thinking: flag("showThinking", d.show_thinking),
        text_scale: v["textScale"].as_f64().filter(|s| (0.7..=1.6).contains(s)).unwrap_or(d.text_scale),
        terminal_font: string("terminalFont", &d.terminal_font),
        scratch_autohide: flag("scratchAutohide", d.scratch_autohide),
        sort_by_urgency: flag("sortByUrgency", d.sort_by_urgency),
        telegram_enabled: flag("telegramEnabled", d.telegram_enabled),
        telegram_token: string("telegramToken", &d.telegram_token),
        telegram_user: v["telegramUser"].as_i64().unwrap_or(d.telegram_user),
        debug_log: flag("debugLog", d.debug_log),
    }
}

fn render(s: &Settings) -> String {
    serde_json::to_string_pretty(&json!({
        "idleMinutes": s.idle_minutes,
        "notifyQuestions": s.notify_questions,
        "notifyDone": s.notify_done,
        "confirmQuit": s.confirm_quit,
        "adapter": s.adapter,
        "defaultMode": s.default_mode,
        "showThinking": s.show_thinking,
        "textScale": s.text_scale,
        "terminalFont": s.terminal_font,
        "scratchAutohide": s.scratch_autohide,
        "sortByUrgency": s.sort_by_urgency,
        "telegramEnabled": s.telegram_enabled,
        "telegramToken": s.telegram_token,
        "telegramUser": s.telegram_user,
        "debugLog": s.debug_log,
    }))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_read_back() {
        let s = Settings {
            idle_minutes: 3,
            notify_done: false,
            adapter: "x y".into(),
            text_scale: 1.2,
            telegram_enabled: true,
            telegram_token: "1:abc".into(),
            telegram_user: 42,
            ..Settings::default()
        };
        assert_eq!(parse(&render(&s)), s);
    }

    #[test]
    fn missing_or_wrong_values_take_the_defaults() {
        let s = parse(r#"{ "idleMinutes": "soon", "textScale": 9 }"#);
        assert_eq!(s, Settings::default());
    }
}
