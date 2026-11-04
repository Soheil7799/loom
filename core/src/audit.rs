//! The audit log: every command that came through the API socket (who, from which process,
//! what, and how it ended), one JSON object per line in the state folder
//! (`audit.jsonl`, or `audit-<suffix>.jsonl` for another app id). Never sent anywhere; read
//! it to see after the fact what any client, an agent that got loose included, asked of loom.

use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;

use serde_json::{Value, json};

thread_local! {
    static FILE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Longest parameters kept per line (a long prompt is cut).
const PARAMS: usize = 2000;

/// Where this app id's log goes (call once, at start).
pub fn init(app_id: &str) {
    let name = crate::state::suffix(app_id).map_or("audit.jsonl".to_owned(), |s| format!("audit-{s}.jsonl"));
    FILE.with(|f| *f.borrow_mut() = Some(crate::state::dir().join(name)));
}

/// Past this, the log moves to `<name>.1` (one kept) and starts again.
const ROTATE: u64 = 10 << 20;

/// Writes one command: `peer` (who and which process), `method`, its `params`, its outcome.
/// Written by a thread of its own (never on the core's), in order.
pub fn record(peer: &Value, method: &str, params: &Value, outcome: Result<(), String>) {
    let Some(path) = FILE.with(|f| f.borrow().clone()) else { return };
    let mut params = params.to_string();
    if params.len() > PARAMS {
        let mut cut = PARAMS;
        while !params.is_char_boundary(cut) {
            cut -= 1;
        }
        params.truncate(cut);
        params.push('…');
    }
    let line = json!({
        "time": crate::api::time(jiff::Timestamp::now().as_second()),
        "peer": peer,
        "method": method,
        "params": params,
        "ok": outcome.is_ok(),
        "error": outcome.err(),
    });
    let _ = writer().send((path, line.to_string()));
}

fn writer() -> std::sync::mpsc::Sender<(PathBuf, String)> {
    static WRITER: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<(PathBuf, String)>>> = std::sync::OnceLock::new();
    WRITER
        .get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel::<(PathBuf, String)>();
            std::thread::spawn(move || {
                for (path, line) in rx {
                    write(&path, &line);
                }
            });
            std::sync::Mutex::new(tx)
        })
        .lock()
        .unwrap()
        .clone()
}

fn write(path: &std::path::Path, line: &str) {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > ROTATE) {
        let _ = std::fs::rename(path, path.with_extension("jsonl.1"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path);
    if let Ok(mut file) = file {
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        let _ = writeln!(file, "{line}");
    }
}
