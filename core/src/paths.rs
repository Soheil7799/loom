//! The user's folders, as GLib names them (so nothing moves for existing installs).

use std::path::PathBuf;

pub fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_default()
}

/// $XDG_CONFIG_HOME, else ~/.config.
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config"))
}

/// $XDG_DATA_HOME, else ~/.local/share.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME").unwrap_or_else(|| home().join(".local/share"))
}

/// $XDG_RUNTIME_DIR, else ~/.cache (GLib's fallback too).
pub fn runtime_dir() -> PathBuf {
    xdg("XDG_RUNTIME_DIR").or_else(|| xdg("XDG_CACHE_HOME")).unwrap_or_else(|| home().join(".cache"))
}

fn xdg(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute())
}

/// Where `name` is found on $PATH.
pub fn find_program(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(name)).find(|path| path.is_file())
}

/// "3m ago", "2h ago"…
pub fn ago(seconds: u64) -> String {
    match seconds {
        s if s < 60 => "just now".to_owned(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86400),
    }
}

/// "42s", "3m 05s", "1h 02m".
pub fn elapsed(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// The local time now in a strftime `format`.
pub fn local_now(format: &str) -> String {
    jiff::Zoned::now().strftime(format).to_string()
}
