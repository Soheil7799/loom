//! The user's settings: kept in ~/.config/loom/settings.json, read once, written on change,
//! and announced to whoever listens (the window restyles, terminals refont…).

/// The terminals' font unless the user picks another.
pub const TERMINAL_FONT: &str = "JetBrains Mono, Vazirmatn 10";

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Idle agents nobody looks at stop after this many minutes (0: never).
    pub idle_minutes: u64,
    pub notify_questions: bool,
    pub notify_done: bool,
    /// Ask before quitting while agents work.
    pub confirm_quit: bool,
    /// loomd's agents keep working when loom's window closes (off: closing it stops loomd).
    pub keep_working: bool,
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
    /// After a turn, show Claude Code's suggestion of the next message.
    pub suggest_prompts: bool,
    /// Launched, loom shows the overview (no agent resumed until one is opened).
    pub start_on_overview: bool,
    /// The running agents listed above the projects (Ctrl+1…9 then go by that list).
    pub show_running: bool,
    /// Ctrl+Tab cycles the running agents only (none is woken by passing it).
    pub cycle_running: bool,
    /// The Telegram bot: off unless switched on, and obeys only `telegram_user`.
    pub telegram_enabled: bool,
    pub telegram_token: String,
    /// The one Telegram account the bot answers (0: none yet).
    pub telegram_user: i64,
    /// The remote bridge (your agents on your other devices, over iroh): off unless switched
    /// on. Its endpoint key (hex, made when first switched on), the relay (empty: n0 public
    /// ones) and the devices paired, as (endpoint id, name).
    pub remote_enabled: bool,
    pub remote_key: String,
    pub remote_relay: String,
    pub remote_devices: Vec<(String, String)>,
    /// Everything loom does, written to the debug log.
    pub debug_log: bool,
    /// Speech to text: on, the endpoint (OpenAI-style), its key, model and language.
    pub stt_enabled: bool,
    pub stt_url: String,
    pub stt_key: String,
    pub stt_model: String,
    pub stt_language: String,
    /// Ctrl+Shift+O opens the file in this program (empty: the system's default app).
    pub editor: String,
    /// The Notes add-on: on, and the vault it keeps notes in (empty: none chosen).
    pub notes_enabled: bool,
    pub notes_vault: String,
    /// At most this many note checks (calls to Claude) a day.
    pub notes_daily_calls: u64,
    /// The model Scribe's calls use ("haiku", "sonnet", "opus"), and their effort (empty:
    /// Claude Code's default). A topic's own `model:` is used over this one.
    pub scribe_model: String,
    pub scribe_effort: String,
    /// A Compact button by the context meter once the context is at least this full (%).
    pub compact_alert: bool,
    pub compact_at: u64,
    /// An idle agent about to be suspended first compacts its conversation, if it holds at
    /// least this many tokens.
    pub rest_compact: bool,
    pub rest_compact_tokens: u64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            idle_minutes: 45,
            notify_questions: true,
            notify_done: true,
            confirm_quit: true,
            keep_working: false,
            adapter: String::new(),
            default_mode: String::new(),
            show_thinking: true,
            text_scale: 1.0,
            terminal_font: TERMINAL_FONT.to_owned(),
            scratch_autohide: false,
            sort_by_urgency: true,
            suggest_prompts: true,
            start_on_overview: true,
            show_running: true,
            cycle_running: true,
            telegram_enabled: false,
            telegram_token: String::new(),
            telegram_user: 0,
            remote_enabled: false,
            remote_key: String::new(),
            remote_relay: String::new(),
            remote_devices: Vec::new(),
            debug_log: false,
            stt_enabled: false,
            stt_url: String::new(),
            stt_key: String::new(),
            stt_model: String::new(),
            stt_language: String::new(),
            editor: String::new(),
            notes_enabled: false,
            notes_vault: String::new(),
            notes_daily_calls: 300,
            scribe_model: "haiku".to_owned(),
            scribe_effort: String::new(),
            compact_alert: true,
            compact_at: 80,
            rest_compact: true,
            rest_compact_tokens: 100_000,
        }
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Settings>> = const { RefCell::new(None) };
    static FILE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// Told of each change; one that answers false is done listening.
    static LISTENERS: RefCell<Vec<Rc<dyn Fn(&Settings) -> bool>>> = const { RefCell::new(Vec::new()) };
}

/// Reads the settings for `app_id` (a test instance keeps its own file).
pub fn init(app_id: &str) {
    let name = match app_id.rsplit('.').next() {
        Some("Loom") | None => "settings.json".to_owned(),
        Some(suffix) => format!("settings-{suffix}.json"),
    };
    let file = crate::paths::config_dir().join("loom").join(name);
    let settings = std::fs::read_to_string(&file).map(|t| parse(&t)).unwrap_or_default();
    FILE.with(|f| *f.borrow_mut() = Some(file));
    crate::scribe::choose(&settings.scribe_model, &settings.scribe_effort);
    publish(&settings);
    CURRENT.with(|c| *c.borrow_mut() = Some(settings));
    crate::api::hub().handle("set_settings", |who, params| {
        if !matches!(who, crate::api::Principal::Window | crate::api::Principal::Cli) {
            return Err(crate::api::Error::Forbidden("settings, but from loom's window or the CLI".into()));
        }
        // What sends anything off this machine, or decides what agents may do, is changed in
        // loom's window only (a loose agent passes for the CLI).
        if *who != crate::api::Principal::Window
            && let Some(name) = params["changes"].as_object().into_iter().flatten().map(|(name, _)| name).find(|name| WINDOW_ONLY.iter().any(|w| snake(w) == **name))
        {
            return Err(crate::api::Error::Forbidden(format!("{name}, but in loom's window")));
        }
        let changed = with_changes(&get(), &params["changes"]).map_err(crate::api::Error::Invalid)?;
        update(|settings| *settings = changed);
        Ok(json!({}))
    });
}

/// Settings written but never read through the core API (only whether they are set).
const SECRETS: &[&str] = &["telegramToken", "remoteKey", "sttKey"];
/// Settings only loom's window changes: the bridges and pairing (what may leave this
/// machine), speech to text (audio sent to a service), what agents run as and may do, the
/// editor loom runs, and the notes vault (what Scribe reads and sends).
const WINDOW_ONLY: &[&str] = &[
    "telegramEnabled",
    "telegramToken",
    "telegramUser",
    "remoteEnabled",
    "remoteRelay",
    "remoteDevices",
    "sttEnabled",
    "sttUrl",
    "sttKey",
    "adapter",
    "defaultMode",
    // A program loom runs; what Scribe reads and sends.
    "editor",
    "notesEnabled",
    "notesVault",
];
/// Settings the API never changes (made by loom itself).
const MADE: &[&str] = &["remoteKey"];

/// `name` (camelCase, as in the file) as the API names it (snake_case).
fn snake(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// The core API's `settings`: one item (`id: "settings"`) with every setting under its
/// snake_case name, but the secrets: for those, only whether they are set (`telegram_token_set`…).
pub fn item(settings: &Settings) -> Value {
    let all: Value = serde_json::from_str(&render(settings)).unwrap_or_default();
    let mut item = serde_json::Map::new();
    item.insert("id".into(), json!("settings"));
    for (name, value) in all.as_object().into_iter().flatten() {
        if SECRETS.contains(&name.as_str()) {
            item.insert(format!("{}_set", snake(name)), json!(value.as_str().is_some_and(|v| !v.is_empty())));
        } else {
            item.insert(snake(name), value.clone());
        }
    }
    Value::Object(item)
}

fn publish(settings: &Settings) {
    crate::api::hub().store("settings").upsert(item(settings));
}

/// `settings` with `changes` (snake_case names, as the API gives them) made; Err names what
/// can't be changed so.
pub fn with_changes(settings: &Settings, changes: &Value) -> Result<Settings, String> {
    let changes = changes.as_object().ok_or("changes must be an object")?;
    let mut all: Value = serde_json::from_str(&render(settings)).unwrap_or_default();
    for (name, value) in changes {
        let Some(key) = all.as_object().and_then(|a| a.keys().find(|k| snake(k) == *name)).cloned() else {
            return Err(format!("no setting {name}"));
        };
        if MADE.contains(&key.as_str()) {
            return Err(format!("{name} is made by loom"));
        }
        if std::mem::discriminant(&all[&key]) != std::mem::discriminant(value) && !(all[&key].is_number() && value.is_number()) {
            return Err(format!("{name}: not a {}", kind(&all[&key])));
        }
        all[&key] = value.clone();
    }
    Ok(parse(&all.to_string()))
}

/// The settings as the API's `settings` item has them, with the secrets (which it never has)
/// taken from `secrets`.
pub fn from_item(item: &Value, secrets: &Settings) -> Settings {
    let named = render(&Settings::default());
    let defaults: Value = serde_json::from_str(&named).unwrap_or_default();
    let mut all = serde_json::Map::new();
    for (key, default) in defaults.as_object().into_iter().flatten() {
        all.insert(key.clone(), item.get(snake(key)).cloned().unwrap_or_else(|| default.clone()));
    }
    let mut settings = parse(&Value::Object(all).to_string());
    settings.telegram_token = secrets.telegram_token.clone();
    settings.remote_key = secrets.remote_key.clone();
    settings.stt_key = secrets.stt_key.clone();
    settings
}

/// What differs from `old` in `new`, as `set_settings`' changes (snake_case names; secrets as
/// they are: they can be written, never read).
pub fn changes(old: &Settings, new: &Settings) -> Value {
    let (old, new): (Value, Value) = (serde_json::from_str(&render(old)).unwrap_or_default(), serde_json::from_str(&render(new)).unwrap_or_default());
    let mut changed = serde_json::Map::new();
    for (key, value) in new.as_object().into_iter().flatten() {
        if old.get(key) != Some(value) {
            changed.insert(snake(key), value.clone());
        }
    }
    Value::Object(changed)
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Bool(_) => "true or false",
        Value::Number(_) => "number",
        Value::String(_) => "text",
        Value::Array(_) => "list",
        _ => "value of its kind",
    }
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
    crate::scribe::choose(&settings.scribe_model, &settings.scribe_effort);
    publish(&settings);
    crate::log::debug!("settings", "changed: {}", {
        let text = render(&settings).replace('\n', " ");
        let text = if settings.telegram_token.is_empty() { text } else { text.replace(&settings.telegram_token, "…") };
        let text = if settings.remote_key.is_empty() { text } else { text.replace(&settings.remote_key, "…") };
        if settings.stt_key.is_empty() { text } else { text.replace(&settings.stt_key, "…") }
    });
    if let Some(file) = FILE.with(|f| f.borrow().clone()) {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let temporary = file.with_extension("json.tmp");
        // It may hold the bot token and the remote key: readable by its owner only.
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
    let done: Vec<_> = listeners.into_iter().filter(|listener| !listener(&settings)).collect();
    if !done.is_empty() {
        LISTENERS.with(|l| l.borrow_mut().retain(|kept| !done.iter().any(|d| Rc::ptr_eq(d, kept))));
    }
}

pub fn on_change(f: impl Fn(&Settings) + 'static) {
    on_change_while(move |s| {
        f(s);
        true
    });
}

/// Told of each change for as long as `f` answers true (one for something that can go).
pub fn on_change_while(f: impl Fn(&Settings) -> bool + 'static) {
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
        keep_working: flag("keepWorking", d.keep_working),
        adapter: string("adapter", &d.adapter),
        default_mode: string("defaultMode", &d.default_mode),
        show_thinking: flag("showThinking", d.show_thinking),
        text_scale: v["textScale"].as_f64().filter(|s| (0.7..=1.6).contains(s)).unwrap_or(d.text_scale),
        terminal_font: string("terminalFont", &d.terminal_font),
        scratch_autohide: flag("scratchAutohide", d.scratch_autohide),
        sort_by_urgency: flag("sortByUrgency", d.sort_by_urgency),
        suggest_prompts: flag("suggestPrompts", d.suggest_prompts),
        start_on_overview: flag("startOnOverview", d.start_on_overview),
        show_running: flag("showRunning", d.show_running),
        cycle_running: flag("cycleRunning", d.cycle_running),
        telegram_enabled: flag("telegramEnabled", d.telegram_enabled),
        telegram_token: string("telegramToken", &d.telegram_token),
        telegram_user: v["telegramUser"].as_i64().unwrap_or(d.telegram_user),
        remote_enabled: flag("remoteEnabled", d.remote_enabled),
        remote_key: string("remoteKey", &d.remote_key),
        remote_relay: string("remoteRelay", &d.remote_relay),
        remote_devices: v["remoteDevices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| Some((d["id"].as_str()?.to_owned(), d["name"].as_str().unwrap_or("").to_owned())))
            .collect(),
        debug_log: flag("debugLog", d.debug_log),
        stt_enabled: flag("sttEnabled", d.stt_enabled),
        stt_url: string("sttUrl", &d.stt_url),
        stt_key: string("sttKey", &d.stt_key),
        stt_model: string("sttModel", &d.stt_model),
        stt_language: string("sttLanguage", &d.stt_language),
        editor: string("editor", &d.editor),
        notes_enabled: flag("notesEnabled", d.notes_enabled),
        notes_vault: string("notesVault", &d.notes_vault),
        notes_daily_calls: v["notesDailyCalls"].as_u64().unwrap_or(d.notes_daily_calls),
        scribe_model: string("scribeModel", &d.scribe_model),
        scribe_effort: string("scribeEffort", &d.scribe_effort),
        compact_alert: flag("compactAlert", d.compact_alert),
        compact_at: v["compactAt"].as_u64().unwrap_or(d.compact_at).clamp(10, 95),
        rest_compact: flag("restCompact", d.rest_compact),
        rest_compact_tokens: v["restCompactTokens"].as_u64().unwrap_or(d.rest_compact_tokens),
    }
}

fn render(s: &Settings) -> String {
    serde_json::to_string_pretty(&json!({
        "idleMinutes": s.idle_minutes,
        "notifyQuestions": s.notify_questions,
        "notifyDone": s.notify_done,
        "confirmQuit": s.confirm_quit,
        "keepWorking": s.keep_working,
        "adapter": s.adapter,
        "defaultMode": s.default_mode,
        "showThinking": s.show_thinking,
        "textScale": s.text_scale,
        "terminalFont": s.terminal_font,
        "scratchAutohide": s.scratch_autohide,
        "sortByUrgency": s.sort_by_urgency,
        "suggestPrompts": s.suggest_prompts,
        "startOnOverview": s.start_on_overview,
        "showRunning": s.show_running,
        "cycleRunning": s.cycle_running,
        "telegramEnabled": s.telegram_enabled,
        "telegramToken": s.telegram_token,
        "telegramUser": s.telegram_user,
        "remoteEnabled": s.remote_enabled,
        "remoteKey": s.remote_key,
        "remoteRelay": s.remote_relay,
        "remoteDevices": s.remote_devices.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
        "debugLog": s.debug_log,
        "sttEnabled": s.stt_enabled,
        "sttUrl": s.stt_url,
        "sttKey": s.stt_key,
        "sttModel": s.stt_model,
        "sttLanguage": s.stt_language,
        "editor": s.editor,
        "notesEnabled": s.notes_enabled,
        "notesVault": s.notes_vault,
        "notesDailyCalls": s.notes_daily_calls,
        "scribeModel": s.scribe_model,
        "scribeEffort": s.scribe_effort,
        "compactAlert": s.compact_alert,
        "compactAt": s.compact_at,
        "restCompact": s.rest_compact,
        "restCompactTokens": s.rest_compact_tokens,
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
            remote_enabled: true,
            remote_relay: "http://localhost:3340".into(),
            remote_devices: vec![("ab12".into(), "Phone".into())],
            ..Settings::default()
        };
        assert_eq!(parse(&render(&s)), s);
    }

    #[test]
    fn the_api_sees_settings_but_never_secrets_and_changes_them_by_name() {
        let s = Settings { telegram_token: "1:abc".into(), remote_key: "k".into(), idle_minutes: 30, ..Settings::default() };
        let item = item(&s);
        assert_eq!((item["id"].as_str(), item["idle_minutes"].as_u64()), (Some("settings"), Some(30)));
        assert_eq!((item["telegram_token_set"].as_bool(), item["stt_key_set"].as_bool()), (Some(true), Some(false)));
        assert!(!item.to_string().contains("1:abc") && item.get("telegram_token").is_none() && item.get("remote_key").is_none());
        let changed = with_changes(&s, &json!({ "idle_minutes": 5, "show_thinking": false, "stt_key": "secret" })).unwrap();
        assert_eq!((changed.idle_minutes, changed.show_thinking, changed.stt_key.as_str()), (5, false, "secret"));
        assert_eq!(changed.telegram_token, "1:abc", "what isn't named stays");
        assert!(with_changes(&s, &json!({ "remote_key": "mine" })).is_err());
        assert!(with_changes(&s, &json!({ "idle_minutes": "soon" })).is_err());
        assert!(with_changes(&s, &json!({ "weather": 1 })).is_err());
        // Read back from the item (secrets from elsewhere), changed by name: the same settings.
        assert_eq!(from_item(&item, &s), s);
        let mut new = s.clone();
        new.idle_minutes = 9;
        new.telegram_token = "2:xyz".into();
        assert_eq!(changes(&s, &new), json!({ "idle_minutes": 9, "telegram_token": "2:xyz" }));
        assert_eq!(with_changes(&s, &changes(&s, &new)).unwrap(), new);
    }

    #[test]
    fn missing_or_wrong_values_take_the_defaults() {
        let s = parse(r#"{ "idleMinutes": "soon", "textScale": 9 }"#);
        assert_eq!(s, Settings::default());
    }
}
