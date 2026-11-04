//! The API on a local Unix socket (warp, step 3): `$XDG_RUNTIME_DIR/loom/<name>-api.sock`,
//! the folder 0700 and the socket 0600, one JSON-RPC message per line each way (`rpc`).
//!
//! Who a connection is, loom works out when it is made, never from what the client says:
//! another user is turned away (the socket's mode already keeps them out); a process that
//! descends from an agent loom started (its Claude Code, its commands, the jobs it ran in the
//! tray) is that agent; anything else of the user's (a terminal, a script) is the CLI.
//! An agent that puts itself out of its own process tree on purpose (a daemon, `systemd-run`)
//! escapes this; handing each child its own connection (step 5) closes that.
//!
//! Threads: one accepts, each connection has a reader and a writer; the protocol runs on the
//! core's thread. A client that stops reading is not buffered for: past a few hundred lines
//! unsent, its subscriptions wait, and the store resyncs them once they would pile up.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::rpc::{Out, Session};
use super::{Principal, hub};

/// Longest line read (a command's parameters, an attachment): past it, the connection ends.
const LINE: u64 = 16 << 20;
/// Lines waiting for a slow client before its subscriptions wait.
const HIGH: usize = 256;

enum Event {
    /// `who`: fixed by loom (a connection it handed down); None: worked out from the peer.
    /// `peer`: what the audit log says of the other end, if not a process (a web page).
    Open { conn: u64, pid: u32, pidfd: Option<OwnedFd>, who: Option<Principal>, peer: Option<serde_json::Value>, lines: mpsc::Sender<String>, waiting: Arc<AtomicUsize> },
    Line { conn: u64, line: String },
    /// The writer caught up: what waits can go.
    Writable { conn: u64 },
    Closed { conn: u64 },
}

/// The API socket of this app id (beside its control socket).
pub fn socket_path(app_id: &str) -> PathBuf {
    let control = crate::control::socket_path(app_id);
    let stem = control.file_stem().map_or("loom".into(), |s| s.to_string_lossy().into_owned());
    control.with_file_name(format!("{stem}-api.sock"))
}

/// Serves the core's hub on `path` (call on the core's thread). `owner` names the agent a
/// process belongs to, if it is one of theirs.
pub fn serve(path: &Path, owner: impl Fn(u32) -> Option<Principal> + 'static) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    // A socket left by a crash; this app is the only instance of its id (GApplication).
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let (tx, rx) = async_channel::unbounded::<Event>();
    let _ = SENDER.set(tx.clone());
    std::thread::spawn(move || accept(listener, tx));
    crate::rt::spawn_local(async move {
        let mut sessions: HashMap<u64, Rc<Session>> = HashMap::new();
        while let Ok(event) = rx.recv().await {
            match event {
                Event::Open { conn, pid, pidfd, who: given, peer: described, lines, waiting } => {
                    let mut who = given.clone().unwrap_or_else(|| principal_of(pid, &owner));
                    // The process ended (its id may be another's by now): what was read of
                    // it can't be trusted, and it is placed nowhere.
                    if given.is_none() && pidfd.as_ref().is_some_and(|fd| !alive(fd)) {
                        who = Principal::Agent("unplaced".into());
                    }
                    crate::log::debug!("api".to_string(), "connection {conn} from process {pid}: {who:?}");
                    let mut peer = described.unwrap_or_else(|| serde_json::json!({ "pid": pid, "command": command_line(pid) }));
                    peer["principal"] = super::rpc::principal_json(&who);
                    let counted = waiting.clone();
                    let out = Out {
                        send: Box::new(move |line| {
                            counted.fetch_add(1, Ordering::Relaxed);
                            let _ = lines.send(line);
                        }),
                        room: Box::new(move || waiting.load(Ordering::Relaxed) < HIGH),
                    };
                    let session = Session::new(hub(), who, out);
                    session.audit(peer);
                    sessions.insert(conn, session);
                }
                Event::Line { conn, line } => {
                    // Not borrowed from the map while it runs (a command may take a while).
                    if let Some(session) = sessions.get(&conn).cloned() {
                        session.handle(&line);
                    }
                }
                Event::Writable { conn } => {
                    if let Some(session) = sessions.get(&conn).cloned() {
                        session.pump();
                    }
                }
                Event::Closed { conn } => {
                    sessions.remove(&conn);
                }
            }
        }
    });
    Ok(())
}

/// Where connections go once `serve` runs (`hand` puts its own there too).
static SENDER: std::sync::OnceLock<async_channel::Sender<Event>> = std::sync::OnceLock::new();
/// Connections' numbers.
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Serves one end of a connection loom made itself, as `who` (a bridge it starts gets the
/// other end): no socket path, no secret, nothing another process could claim. `pid`: the
/// process given the other end, for the audit log.
pub fn hand(stream: UnixStream, who: Principal, pid: u32) -> Result<(), String> {
    let tx = SENDER.get().ok_or("the API isn't served")?;
    open(stream, tx, pid, None, Some(who))
}

/// Starts serving `stream`: its writer and reader threads, once the core's thread knows it.
fn open(stream: UnixStream, tx: &async_channel::Sender<Event>, pid: u32, pidfd: Option<OwnedFd>, who: Option<Principal>) -> Result<(), String> {
    let conn = NEXT.fetch_add(1, Ordering::Relaxed);
    let writer = stream.try_clone().map_err(|e| e.to_string())?;
    let (lines, out) = mpsc::channel::<String>();
    let waiting = Arc::new(AtomicUsize::new(0));
    tx.send_blocking(Event::Open { conn, pid, pidfd, who, peer: None, lines, waiting: waiting.clone() }).map_err(|_| "loom is closing".to_owned())?;
    let events = tx.clone();
    std::thread::spawn(move || write(writer, out, waiting, conn, events));
    let events = tx.clone();
    std::thread::spawn(move || read(stream, conn, events));
    Ok(())
}

/// A connection over another transport (a WebSocket): the protocol as on the socket, as
/// `who`. The transport hands each line it reads to `line`, sends what `outgoing` gives, says
/// `sent` after each, and `closed` at the end.
pub(crate) struct External {
    conn: u64,
    events: async_channel::Sender<Event>,
    waiting: Arc<AtomicUsize>,
    pub outgoing: mpsc::Receiver<String>,
}

impl External {
    /// `peer`: what the audit log says of the other end.
    pub fn open(who: Principal, peer: serde_json::Value) -> Result<External, String> {
        let events = SENDER.get().ok_or("the API isn't served")?.clone();
        let conn = NEXT.fetch_add(1, Ordering::Relaxed);
        let (lines, outgoing) = mpsc::channel::<String>();
        let waiting = Arc::new(AtomicUsize::new(0));
        events.send_blocking(Event::Open { conn, pid: 0, pidfd: None, who: Some(who), peer: Some(peer), lines, waiting: waiting.clone() }).map_err(|_| "loom is closing".to_owned())?;
        Ok(External { conn, events, waiting, outgoing })
    }

    /// A message from the client; false: loom has gone.
    pub fn line(&self, line: String) -> bool {
        self.events.send_blocking(Event::Line { conn: self.conn, line }).is_ok()
    }

    /// One outgoing line sent (what waited for room goes once half are).
    pub fn sent(&self) {
        if self.waiting.fetch_sub(1, Ordering::Relaxed) == HIGH / 2 + 1 {
            let _ = self.events.send_blocking(Event::Writable { conn: self.conn });
        }
    }
}

impl Drop for External {
    fn drop(&mut self) {
        let _ = self.events.send_blocking(Event::Closed { conn: self.conn });
    }
}

fn accept(listener: UnixListener, tx: async_channel::Sender<Event>) {
    let me = unsafe { libc::getuid() };
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            // Out of file descriptors, say: wait rather than spin.
            std::thread::sleep(std::time::Duration::from_millis(200));
            continue;
        };
        let Some((uid, pid)) = peer(&stream) else { continue };
        if uid != me {
            continue;
        }
        // Taken now, while the process is surely the one that connected.
        let pidfd = peer_pidfd(&stream);
        if open(stream, &tx, pid, pidfd, None).is_err() && tx.is_closed() {
            return;
        }
    }
}

fn read(stream: UnixStream, conn: u64, events: async_channel::Sender<Event>) {
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = Vec::new();
        match (&mut reader).take(LINE + 1).read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) if line.last() != Some(&b'\n') && line.len() as u64 > LINE => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&line);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if events.send_blocking(Event::Line { conn, line: line.to_owned() }).is_err() {
            return;
        }
    }
    let _ = events.send_blocking(Event::Closed { conn });
}

fn write(mut stream: UnixStream, lines: mpsc::Receiver<String>, waiting: Arc<AtomicUsize>, conn: u64, events: async_channel::Sender<Event>) {
    for mut line in lines {
        line.push('\n');
        if stream.write_all(line.as_bytes()).is_err() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return;
        }
        // Down to half: what waited for room can go.
        if waiting.fetch_sub(1, Ordering::Relaxed) == HIGH / 2 + 1 && events.send_blocking(Event::Writable { conn }).is_err() {
            return;
        }
    }
}

/// The user and process on the other end.
fn peer(stream: &UnixStream) -> Option<(u32, u32)> {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `len` bytes into `cred`.
    let ok = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len) };
    (ok == 0).then_some((cred.uid, cred.pid as u32))
}

/// A handle on the process on the other end (Linux 6.5+; None before): it keeps naming that
/// process even once its id is reused.
fn peer_pidfd(stream: &UnixStream) -> Option<OwnedFd> {
    use std::os::fd::{AsRawFd, FromRawFd};
    const SO_PEERPIDFD: libc::c_int = 77;
    let mut fd: libc::c_int = -1;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `len` bytes into `fd`.
    let ok = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, SO_PEERPIDFD, (&raw mut fd).cast(), &mut len) };
    // SAFETY: on success `fd` is a new descriptor of ours.
    (ok == 0 && fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Whether the process `pidfd` names still runs.
fn alive(pidfd: &OwnedFd) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: signal 0 checks only; no memory is passed.
    unsafe { libc::syscall(libc::SYS_pidfd_send_signal, pidfd.as_raw_fd(), 0, std::ptr::null::<libc::siginfo_t>(), 0) == 0 }
}

/// Process `pid`'s command line, for the audit log.
fn command_line(pid: u32) -> String {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    raw.split(|b| *b == 0).filter(|a| !a.is_empty()).map(String::from_utf8_lossy).collect::<Vec<_>>().join(" ")
}

/// Who process `pid` speaks for: the first of its ancestors `owner` knows, else the CLI.
pub fn principal_of(pid: u32, owner: &impl Fn(u32) -> Option<Principal>) -> Principal {
    // In an agent's scope: that agent's, wherever it went in the process tree.
    if let Some(agent) = crate::scope::agent_of(pid) {
        return Principal::Agent(agent.to_string());
    }
    let me = std::process::id();
    let mut at = pid;
    for _ in 0..64 {
        if at <= 1 || at == me {
            break;
        }
        if let Some(who) = owner(at) {
            return who;
        }
        match parent(at) {
            Some(up) => at = up,
            None => break,
        }
    }
    Principal::Cli
}

/// Process `pid`'s parent (Linux: `/proc/<pid>/stat`).
fn parent(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (name) state ppid …`; the name may hold spaces and parentheses.
    stat[stat.rfind(')')? + 1..].split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_speaks_for_the_agent_it_descends_from() {
        let mut child = std::process::Command::new("sh").args(["-c", "sleep 5"]).spawn().unwrap();
        let pid = child.id();
        // Its parent is this test: loom's own process ends the walk.
        assert_eq!(principal_of(pid, &|_| None), Principal::Cli);
        assert_eq!(principal_of(pid, &|p| (p == pid).then(|| Principal::Agent("4".into()))), Principal::Agent("4".into()));
        assert_eq!(parent(pid), Some(std::process::id()));
        let _ = child.kill();
        let _ = child.wait();
    }

    /// The known escape, and why it costs nothing that matters: a process an agent left
    /// running on its own (`setsid`, `nohup`, `cmd &` then the shell gone) is placed under no
    /// agent, so it is taken for the CLI; and the CLI answers no agent's questions.
    #[test]
    fn a_process_cut_loose_from_its_agent_passes_for_the_cli_and_still_cannot_answer() {
        let out = std::process::Command::new("sh").args(["-c", "setsid nohup sleep 5 >/dev/null 2>&1 & echo $!"]).output().unwrap();
        let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        // Its shell has ended: it is the reaper's now, under no agent.
        assert_ne!(parent(pid), Some(std::process::id()));
        let agent_shells = |p: u32| (p == std::process::id()).then(|| Principal::Agent("1".into()));
        assert_eq!(principal_of(pid, &agent_shells), Principal::Cli);
        crate::agent::commands::serve();
        let loose = hub().connect(Principal::Cli);
        let answer = loose.command("a1", "answer", serde_json::json!({ "agent": "1", "entry": "e1", "option": "allow" }));
        assert!(matches!(answer, Err(super::super::Error::Forbidden(_))), "{answer:?}");
        // SAFETY: a plain kill of the sleep started above.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }

    /// A connection loom handed down (a bridge's) is who loom said, with that one's rights,
    /// whatever process uses it.
    #[test]
    fn a_handed_down_connection_is_the_bridge_loom_made_it_for() {
        type Task = std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>;
        let tasks: Rc<std::cell::RefCell<Vec<Task>>> = Rc::default();
        let queue = tasks.clone();
        crate::rt::set_host(move |f| queue.borrow_mut().push(f), |_, _| {});
        let path = std::env::temp_dir().join(format!("loom-handed-{}", std::process::id())).join("api.sock");
        serve(&path, |_| None).unwrap();
        crate::settings::init("dev.shirvani.LoomHandedTest");
        let (ours, theirs) = UnixStream::pair().unwrap();
        hand(ours, Principal::Bridge("telegram".into()), std::process::id()).unwrap();
        let client = std::thread::spawn(move || {
            let mut bot = crate::api::cli::Connection::over(theirs, "test bridge").unwrap();
            let refused_vault = bot.subscribe("vault/").err().map(|e| e.code);
            let refused_settings = bot.command("c1", "set_settings", serde_json::json!({ "changes": { "text_scale": 2 } })).err().map(|e| e.code);
            (bot.hello["principal"].clone(), refused_vault, refused_settings)
        });
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !client.is_finished() {
            assert!(std::time::Instant::now() < deadline, "the bridge got no answers");
            let mut pending: Vec<Task> = tasks.borrow_mut().drain(..).collect();
            pending.retain_mut(|task| task.as_mut().poll(&mut cx).is_pending());
            tasks.borrow_mut().extend(pending);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let (principal, vault, settings) = client.join().unwrap();
        assert_eq!(principal, serde_json::json!({ "kind": "bridge", "name": "telegram" }));
        assert_eq!(vault, Some(super::super::rpc::FORBIDDEN), "the Telegram bot reads no notes");
        assert_eq!(settings, Some(super::super::rpc::FORBIDDEN), "nor changes settings");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_connection_names_its_process_even_after_it_ends() {
        let (ours, theirs) = UnixStream::pair().unwrap();
        // A pair's peer is this process: alive.
        if let Some(fd) = peer_pidfd(&ours) {
            assert!(alive(&fd));
        }
        drop(theirs);
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(command_line(pid).is_empty(), "an ended process has no command line");
    }
}
