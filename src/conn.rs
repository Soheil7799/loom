//! The window's way to loom's core API: loomd's, over its socket (the window is its client).
//! The rest of the window follows resources and does commands through here.

use std::cell::RefCell;
use std::path::Path;
use std::time::{Duration, Instant};

use loom_core::api::remote::{Remote, RemoteSub};
use serde_json::Value;

thread_local! {
    /// loomd's connection (set at start, before anything follows a resource).
    static REMOTE: RefCell<Option<Remote>> = const { RefCell::new(None) };
}

/// From now on, the API is loomd's over `remote` (before anything follows a resource).
pub fn use_remote(remote: Remote) {
    REMOTE.with_borrow_mut(|r| *r = Some(remote));
}

/// loomd's connection.
pub fn loomd() -> Option<Remote> {
    REMOTE.with_borrow(Clone::clone)
}

/// A resource followed.
pub type Sub = RemoteSub;

/// Follows `resource` as the window; `ready` is called when something waits (loomd's answer
/// comes later: a `gone`, if it has no such resource). None: not connected to loomd yet.
pub fn subscribe(resource: &str, ready: impl Fn() + 'static) -> Option<Sub> {
    loomd().map(|remote| remote.subscribe(resource, ready))
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
    match loomd() {
        Some(remote) => remote.command_later(&loom_core::rt::uuid(), method, params, reply),
        None => reply(Err(loom_core::api::Error::NotFound("loomd".into()))),
    }
}

/// Whether this window started the loomd it speaks to (so closing it may stop it).
pub fn started_loomd() -> bool {
    STARTED.load(std::sync::atomic::Ordering::Relaxed)
}

static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// loomd started for this window (none answers at `socket` for `app_id`): the one installed
/// beside this loom, on its own (it outlives the window: its agents go on when the window
/// closes), its output in `<state>/loom/loomd[-<suffix>].log`. Under systemd where there is
/// one (started again if it crashes, as loomd.service would), else a plain process. Waits
/// until it answers.
pub fn start_loomd(socket: &Path, app_id: &str) -> Result<(), String> {
    let program = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("loomd");
    if !program.is_file() {
        return Err(format!("{} is not installed", program.display()));
    }
    let log = loom_core::state::dir().join(match loom_core::state::suffix(app_id) {
        Some(suffix) => format!("loomd-{suffix}.log"),
        None => "loomd.log".to_owned(),
    });
    std::fs::create_dir_all(loom_core::state::dir()).map_err(|e| e.to_string())?;
    if !under_systemd(&program, &log, app_id) {
        spawn_loomd(&program, &log)?;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while loom_core::api::socket::running(socket).as_deref() != Some("daemon") {
        if Instant::now() > deadline {
            return Err(format!("loomd didn't start (its output: {})", log.display()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    STARTED.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

/// loomd as a transient systemd user service (`loomd-<app id>`), with this window's
/// environment (PATH finds the agents' programs): restarted if it fails, as loomd.service
/// is. False if there's no systemd to ask (or it refused).
fn under_systemd(program: &Path, log: &Path, app_id: &str) -> bool {
    let mut run = std::process::Command::new("systemd-run");
    run.args(["--user", "--quiet", "--collect", "--unit"]).arg(format!("loomd-{app_id}"));
    // as data/loomd.service: given up after 5 failures in 2 minutes; 3 (another core runs) isn't one
    for property in ["Restart=on-failure", "RestartSec=5", "RestartPreventExitStatus=3", "StartLimitIntervalSec=120", "StartLimitBurst=5"] {
        run.arg("-p").arg(property);
    }
    run.arg("-p").arg(format!("StandardOutput=append:{}", log.display()));
    run.arg("-p").arg(format!("StandardError=append:{}", log.display()));
    // (vars() would panic on a name or value that isn't UTF-8: those are left out)
    for (name, value) in std::env::vars_os().filter_map(|(n, v)| Some((n.into_string().ok()?, v.into_string().ok()?))) {
        // The window's own switch is not loomd's; systemd refuses control characters.
        if name == "LOOM_CONTROL" || value.chars().any(char::is_control) {
            continue;
        }
        // By name only: systemd-run copies the value from its own environment, so none (a
        // token, a key) shows on its command line to other users.
        run.arg(format!("--setenv={name}"));
    }
    run.arg(program).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    run.status().is_ok_and(|status| status.success())
}

/// loomd as a plain process of its own (no systemd): not started again if it fails.
fn spawn_loomd(program: &Path, log: &Path) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let out = std::fs::OpenOptions::new().create(true).append(true).open(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let mut child = std::process::Command::new(program)
        // The window's own switch is not loomd's.
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
    Ok(())
}
