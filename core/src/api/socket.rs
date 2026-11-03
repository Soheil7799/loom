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
    Open { conn: u64, pid: u32, lines: mpsc::Sender<String>, waiting: Arc<AtomicUsize> },
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
    std::thread::spawn(move || accept(listener, tx));
    crate::rt::spawn_local(async move {
        let mut sessions: HashMap<u64, Rc<Session>> = HashMap::new();
        while let Ok(event) = rx.recv().await {
            match event {
                Event::Open { conn, pid, lines, waiting } => {
                    let who = principal_of(pid, &owner);
                    crate::log::debug!("api".to_string(), "connection {conn} from process {pid}: {who:?}");
                    let counted = waiting.clone();
                    let out = Out {
                        send: Box::new(move |line| {
                            counted.fetch_add(1, Ordering::Relaxed);
                            let _ = lines.send(line);
                        }),
                        room: Box::new(move || waiting.load(Ordering::Relaxed) < HIGH),
                    };
                    sessions.insert(conn, Session::new(hub(), who, out));
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

fn accept(listener: UnixListener, tx: async_channel::Sender<Event>) {
    let me = unsafe { libc::getuid() };
    for (conn, stream) in (1u64..).zip(listener.incoming()) {
        let Ok(stream) = stream else {
            // Out of file descriptors, say: wait rather than spin.
            std::thread::sleep(std::time::Duration::from_millis(200));
            continue;
        };
        let Some((uid, pid)) = peer(&stream) else { continue };
        if uid != me {
            continue;
        }
        let Ok(writer) = stream.try_clone() else { continue };
        let (lines, out) = mpsc::channel::<String>();
        let waiting = Arc::new(AtomicUsize::new(0));
        if tx.send_blocking(Event::Open { conn, pid, lines, waiting: waiting.clone() }).is_err() {
            return;
        }
        let events = tx.clone();
        std::thread::spawn(move || write(writer, out, waiting, conn, events));
        let events = tx.clone();
        std::thread::spawn(move || read(stream, conn, events));
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

/// Who process `pid` speaks for: the first of its ancestors `owner` knows, else the CLI.
pub fn principal_of(pid: u32, owner: &impl Fn(u32) -> Option<Principal>) -> Principal {
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
}
