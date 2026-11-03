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

/// Writes one command: `peer` (who and which process), `method`, its `params`, its outcome.
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
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path);
    if let Ok(mut file) = file {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        let _ = writeln!(file, "{line}");
    }
}
