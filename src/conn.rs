//! The window's way to loom's core API: the core in this process, or loomd's over its socket
//! once the window is its client. The rest of the window follows resources and does commands
//! through here, the same either way.

use std::cell::RefCell;
use std::path::Path;
use std::time::{Duration, Instant};

use loom_core::api::remote::{Remote, RemoteSub};
use loom_core::api::{Message, Principal, Subscription};
use serde_json::Value;

thread_local! {
    /// loomd's connection, when the window is its client (None: the core is in this process).
    static REMOTE: RefCell<Option<Remote>> = const { RefCell::new(None) };
}

/// From now on, the API is loomd's over `remote` (before anything follows a resource).
pub fn use_remote(remote: Remote) {
    REMOTE.with_borrow_mut(|r| *r = Some(remote));
}

/// Whether the window is loomd's client (its agents are loomd's).
pub fn is_remote() -> bool {
    REMOTE.with_borrow(Option::is_some)
}

/// loomd's connection, when the window is its client.
pub fn loomd() -> Option<Remote> {
    remote()
}

fn remote() -> Option<Remote> {
    REMOTE.with_borrow(Clone::clone)
}

/// A resource followed, wherever the core is.
pub enum Sub {
    Local(Subscription),
    Remote(RemoteSub),
}

impl Sub {
    /// What came since, numbered for a `Mirror`.
    pub fn drain(&self) -> Vec<Message> {
        match self {
            Sub::Local(sub) => sub.drain(),
            Sub::Remote(sub) => sub.drain(),
        }
    }

    /// Asks for a fresh snapshot (a copy saw a gap).
    pub fn resync(&self) {
        match self {
            Sub::Local(sub) => sub.resync(),
            Sub::Remote(sub) => sub.resync(),
        }
    }

    /// Whether something waits.
    pub fn waiting(&self) -> bool {
        match self {
            Sub::Local(sub) => sub.waiting(),
            Sub::Remote(sub) => sub.waiting(),
        }
    }
}

/// Follows `resource` as the window; `ready` is called when something waits. None: no such
/// resource in this process (loomd's answer comes later: a `gone`, if it has none).
pub fn subscribe(resource: &str, ready: impl Fn() + 'static) -> Option<Sub> {
    match remote() {
        Some(remote) => Some(Sub::Remote(remote.subscribe(resource, ready))),
        None => loom_core::api::hub().connect(Principal::Window).subscribe(resource, ready).ok().map(Sub::Local),
    }
}

/// Does command `method` as the window; a refusal is logged under `who`.
pub fn command(who: &str, method: &str, params: Value) {
    command_then(who, method, params, |_| {});
}

/// Does command `method` as the window, then `then` with what it answered (a refusal is
/// logged under `who` too).
pub fn command_then(who: &str, method: &str, params: Value, then: impl FnOnce(Option<Value>) + 'static) {
    command_result(who, method, params, move |result| then(result.ok()));
}

/// Does command `method` as the window, then `then` with its answer or why it was refused
/// (logged under `who` too).
pub fn command_result(who: &str, method: &str, params: Value, then: impl FnOnce(Result<Value, loom_core::api::Error>) + 'static) {
    let (who, said) = (who.to_owned(), method.to_owned());
    let reply = move |result: Result<Value, loom_core::api::Error>| {
        if let Err(err) = &result {
            loom_core::log::debug!(who, "{said} refused: {err}");
        }
        then(result);
    };
    match remote() {
        Some(remote) => remote.command_later(&loom_core::rt::uuid(), method, params, reply),
        None => loom_core::api::hub().connect(Principal::Window).command_later(&loom_core::rt::uuid(), method, params, reply),
    }
}

/// loomd started for this window (none answers at `socket` for `app_id`): the one installed
/// beside this loom, on its own (it outlives the window: its agents go on when the window
/// closes), its output in `<state>/loom/loomd[-<suffix>].log`. Waits until it answers.
// shortcut: started as a plain process, not under systemd (no restart if it fails); loomd.service does that, use it once loomd is how loom always runs
pub fn start_loomd(socket: &Path, app_id: &str) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let program = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("loomd");
    if !program.is_file() {
        return Err(format!("{} is not installed", program.display()));
    }
    let log = loom_core::state::dir().join(match loom_core::state::suffix(app_id) {
        Some(suffix) => format!("loomd-{suffix}.log"),
        None => "loomd.log".to_owned(),
    });
    std::fs::create_dir_all(loom_core::state::dir()).map_err(|e| e.to_string())?;
    let out = std::fs::OpenOptions::new().create(true).append(true).open(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let mut child = std::process::Command::new(&program)
        // The window's own switches are not loomd's.
        .env_remove("LOOM_REMOTE")
        .env_remove("LOOM_CONTROL")
        .stdin(std::process::Stdio::null())
        .stdout(out.try_clone().map_err(|e| e.to_string())?)
        .stderr(out)
        // Its own process group: a Ctrl+C meant for the window's terminal isn't loomd's.
        .process_group(0)
        .spawn()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    // Waited for, off this thread (no zombie while the window runs).
    std::thread::spawn(move || child.wait());
    let deadline = Instant::now() + Duration::from_secs(10);
    while loom_core::api::socket::running(socket).as_deref() != Some("daemon") {
        if Instant::now() > deadline {
            return Err(format!("loomd didn't start (its output: {})", log.display()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
