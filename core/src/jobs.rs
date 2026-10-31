//! Jobs: the commands loom runs for a project (a dev server, the tests, a login), each in a
//! terminal of its own. The process, its PTY and what it printed live here; a UI shows them
//! from the events (`Output` to draw, `Exited` to mark it done) and sends keys and sizes back.
//!
//! Named like herdr's panes: running a name again reuses that job, never a second one.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Lines of scrollback kept for `output` (the same as the terminals show).
const SCROLLBACK: usize = 10_000;
/// SIGTERM first; SIGKILL this long after if it is still there.
const GRACE: Duration = Duration::from_secs(3);
/// How long an exit waits for the last output (a child left in the background can keep the
/// PTY open forever).
const DRAIN: Duration = Duration::from_millis(500);

pub type Id = u64;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A new job (its name and command: `info`).
    Added(Id),
    /// A run began (the first, or again): what was shown before is gone.
    Started(Id),
    /// Asked to run again while running: stopping the old run first.
    Restarting(Id),
    /// Bytes it printed, terminal escapes and all.
    Output(Id, Vec<u8>),
    /// It ended: the exit code, None when killed by a signal or unknown.
    Exited(Id, Option<i64>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub id: Id,
    pub name: String,
    pub command: String,
    pub running: bool,
    pub exit_code: Option<i64>,
}

/// One project's jobs. Cheap to clone; every clone is the same set.
#[derive(Clone)]
pub struct Jobs {
    inner: Arc<Inner>,
}

struct Inner {
    cwd: PathBuf,
    jobs: Mutex<Vec<Arc<Job>>>,
    next: AtomicU64,
    events: async_channel::Sender<Event>,
}

struct Job {
    id: Id,
    state: Mutex<State>,
}

struct State {
    name: String,
    command: String,
    /// Where it runs, if not the project folder.
    cwd: Option<PathBuf>,
    /// None while running; Some(exit code, if known) once done.
    outcome: Option<Option<i64>>,
    /// Asked to run again while still running: the exit starts it (one restart, however many
    /// times it was asked).
    restart_pending: bool,
    run: Option<Run>,
    /// Counts runs, so a late thread of an old run touches nothing.
    generation: u64,
    size: PtySize,
    screen: vt100::Parser,
}

struct Run {
    pid: Option<u32>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
}

impl Jobs {
    /// Jobs running in `cwd`, and where their events arrive.
    pub fn new(cwd: PathBuf) -> (Jobs, async_channel::Receiver<Event>) {
        let (events, rx) = async_channel::unbounded();
        let inner = Inner { cwd, jobs: Mutex::default(), next: AtomicU64::new(1), events };
        (Jobs { inner: Arc::new(inner) }, rx)
    }

    /// The folder its jobs run in.
    pub fn cwd(&self) -> &Path {
        &self.inner.cwd
    }

    /// Runs `command` as the job `name`: that job again if there is one. Err if it is still
    /// running and `restart` is false.
    pub fn run_named(&self, name: &str, command: &str, restart: bool, cwd: Option<PathBuf>) -> Result<Id, String> {
        if let Some(job) = self.find(|s| s.name == name) {
            {
                let mut state = job.state.lock().unwrap();
                if state.outcome.is_none() && !restart {
                    return Err(format!("job {name} is still running (stop it, or pass restart)"));
                }
                state.cwd = cwd;
                state.command = command.to_owned();
            }
            self.inner.restart(&job);
            return Ok(job.id);
        }
        Ok(self.run(name, command, cwd))
    }

    /// Runs `command` as a new job, whatever else bears its name (the user's own runs).
    pub fn run(&self, name: &str, command: &str, cwd: Option<PathBuf>) -> Id {
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        let size = PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 };
        let job = Arc::new(Job {
            id,
            state: Mutex::new(State {
                name: name.to_owned(),
                command: command.to_owned(),
                cwd,
                outcome: None,
                restart_pending: false,
                run: None,
                generation: 0,
                size,
                screen: vt100::Parser::new(size.rows, size.cols, SCROLLBACK),
            }),
        });
        self.inner.jobs.lock().unwrap().push(job.clone());
        self.inner.emit(Event::Added(id));
        self.inner.start(&job);
        id
    }

    /// Runs it again: at once if nothing runs, else once the current run has exited (it is
    /// stopped).
    pub fn restart(&self, id: Id) {
        if let Some(job) = self.get(id) {
            self.inner.restart(&job);
        }
    }

    /// Stops it (and any restart it was waiting for).
    pub fn stop(&self, id: Id) {
        if let Some(job) = self.get(id) {
            job.state.lock().unwrap().restart_pending = false;
            terminate(&job);
        }
    }

    /// Stops it and forgets it.
    pub fn remove(&self, id: Id) {
        self.stop(id);
        self.inner.jobs.lock().unwrap().retain(|job| job.id != id);
    }

    /// Stops every job (on quit).
    pub fn stop_all(&self) {
        let ids: Vec<Id> = self.inner.jobs.lock().unwrap().iter().map(|job| job.id).collect();
        for id in ids {
            self.stop(id);
        }
    }

    /// Keys typed into its terminal.
    pub fn write(&self, id: Id, bytes: &[u8]) {
        let Some(job) = self.get(id) else { return };
        let mut state = job.state.lock().unwrap();
        if let Some(run) = state.run.as_mut() {
            let _ = run.writer.write_all(bytes).and_then(|()| run.writer.flush());
        }
    }

    /// The size its terminal is shown at.
    pub fn resize(&self, id: Id, rows: u16, cols: u16) {
        let Some(job) = self.get(id) else { return };
        if rows == 0 || cols == 0 {
            return;
        }
        let mut state = job.state.lock().unwrap();
        if (state.size.rows, state.size.cols) == (rows, cols) {
            return;
        }
        state.size.rows = rows;
        state.size.cols = cols;
        state.screen.screen_mut().set_size(rows, cols);
        if let Some(run) = state.run.as_ref() {
            let _ = run.master.resize(state.size);
        }
    }

    pub fn info(&self, id: Id) -> Option<Info> {
        self.get(id).map(|job| info(&job))
    }

    /// The job named `name`.
    pub fn named(&self, name: &str) -> Option<Info> {
        self.find(|s| s.name == name).map(|job| info(&job))
    }

    /// Every job, oldest first.
    pub fn list(&self) -> Vec<Info> {
        self.inner.jobs.lock().unwrap().iter().map(|job| info(job)).collect()
    }

    /// The last `lines` lines it printed, as text.
    pub fn output(&self, id: Id, lines: usize) -> String {
        let Some(job) = self.get(id) else { return String::new() };
        let mut state = job.state.lock().unwrap();
        text(&mut state.screen, lines)
    }

    fn get(&self, id: Id) -> Option<Arc<Job>> {
        self.inner.jobs.lock().unwrap().iter().find(|job| job.id == id).cloned()
    }

    fn find(&self, test: impl Fn(&State) -> bool) -> Option<Arc<Job>> {
        self.inner.jobs.lock().unwrap().iter().rev().find(|job| test(&job.state.lock().unwrap())).cloned()
    }
}

impl Inner {
    fn emit(&self, event: Event) {
        let _ = self.events.try_send(event);
    }

    fn restart(self: &Arc<Self>, job: &Arc<Job>) {
        {
            let mut state = job.state.lock().unwrap();
            if state.run.is_some() {
                state.restart_pending = true;
                state.outcome = None;
                drop(state);
                self.emit(Event::Restarting(job.id));
                terminate(job);
                return;
            }
        }
        self.start(job);
    }

    /// Starts a run, unless one is running.
    fn start(self: &Arc<Self>, job: &Arc<Job>) {
        let mut state = job.state.lock().unwrap();
        if state.run.is_some() {
            return;
        }
        state.generation += 1;
        state.outcome = None;
        state.screen = vt100::Parser::new(state.size.rows, state.size.cols, SCROLLBACK);
        let generation = state.generation;
        let cwd = state.cwd.clone().unwrap_or_else(|| self.cwd.clone());
        let (child, reader, run) = match spawn(&state.command, &cwd, state.size) {
            Ok(spawned) => spawned,
            Err(err) => {
                let message = format!("loom: could not start: {err}\r\n");
                state.screen.process(message.as_bytes());
                state.outcome = Some(Some(127));
                drop(state);
                self.emit(Event::Started(job.id));
                self.emit(Event::Output(job.id, message.into_bytes()));
                self.emit(Event::Exited(job.id, Some(127)));
                return;
            }
        };
        state.run = Some(run);
        drop(state);
        self.emit(Event::Started(job.id));

        let (drained, done) = mpsc::channel::<()>();
        let (inner, read_job) = (self.clone(), job.clone());
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut state = read_job.state.lock().unwrap();
                        if state.generation != generation {
                            break;
                        }
                        state.screen.process(&buf[..n]);
                        drop(state);
                        inner.emit(Event::Output(read_job.id, buf[..n].to_vec()));
                    }
                }
            }
            let _ = drained.send(());
        });

        let (inner, wait_job) = (self.clone(), job.clone());
        std::thread::spawn(move || {
            let mut child = child;
            let status = child.wait();
            // What it printed last arrives before the word that it ended.
            let _ = done.recv_timeout(DRAIN);
            let code = match status {
                Ok(status) if status.signal().is_none() => Some(i64::from(status.exit_code())),
                _ => None,
            };
            let mut state = wait_job.state.lock().unwrap();
            if state.generation != generation {
                return;
            }
            state.run = None;
            // Asked to run again: this exit was the old run's.
            if std::mem::take(&mut state.restart_pending) {
                drop(state);
                inner.start(&wait_job);
                return;
            }
            state.outcome = Some(code);
            drop(state);
            inner.emit(Event::Exited(wait_job.id, code));
        });
    }
}

type Spawned = (Box<dyn portable_pty::Child + Send + Sync>, Box<dyn Read + Send>, Run);

/// `command` through the shell in a new PTY.
fn spawn(command: &str, cwd: &Path, size: PtySize) -> Result<Spawned, String> {
    // portable-pty would quietly run it in the home folder instead.
    if !cwd.is_dir() {
        return Err(format!("{} is not a folder", cwd.display()));
    }
    let pair = native_pty_system().openpty(size).map_err(|e| e.to_string())?;
    let mut cmd = CommandBuilder::new("/bin/bash");
    cmd.args(["-c", command]);
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    // Only the child holds the other end now, so its exit ends the reading.
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let run = Run { pid: child.process_id(), killer: child.clone_killer(), writer, master: pair.master };
    Ok((child, reader, run))
}

/// SIGTERM to its process group, SIGKILL after GRACE if that run is still there.
fn terminate(job: &Arc<Job>) {
    let mut state = job.state.lock().unwrap();
    let generation = state.generation;
    let Some(run) = state.run.as_mut() else { return };
    #[cfg(unix)]
    if let Some(pid) = run.pid.and_then(|pid| i32::try_from(pid).ok()) {
        // SAFETY: plain kill(2); the shell leads its own session and group (setsid).
        unsafe { libc::kill(-pid, libc::SIGTERM) };
        drop(state);
        let job = job.clone();
        std::thread::spawn(move || {
            std::thread::sleep(GRACE);
            let state = job.state.lock().unwrap();
            if state.generation == generation && state.run.is_some() {
                // SAFETY: as above; a group already gone just gives ESRCH.
                unsafe { libc::kill(-pid, libc::SIGKILL) };
            }
        });
        return;
    }
    let _ = run.killer.kill();
}

fn info(job: &Job) -> Info {
    let state = job.state.lock().unwrap();
    Info {
        id: job.id,
        name: state.name.clone(),
        command: state.command.clone(),
        running: state.outcome.is_none(),
        exit_code: state.outcome.flatten(),
    }
}

/// The last `lines` lines on the screen and in its scrollback, trailing blank ones dropped.
fn text(parser: &mut vt100::Parser, lines: usize) -> String {
    let screen = parser.screen_mut();
    let (rows, cols) = screen.size();
    screen.set_scrollback(usize::MAX);
    let kept = screen.scrollback();
    let mut all = Vec::new();
    // Page through the scrollback from its oldest line, then the screen itself.
    let mut seen = 0;
    while seen < kept {
        screen.set_scrollback(kept - seen);
        let page = (kept - seen).min(usize::from(rows));
        all.extend(screen.rows(0, cols).take(page));
        seen += page;
    }
    screen.set_scrollback(0);
    all.extend(screen.rows(0, cols));
    while all.last().is_some_and(|line: &String| line.trim().is_empty()) {
        all.pop();
    }
    let start = all.len().saturating_sub(lines);
    all[start..].iter().map(|line| line.trim_end()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Events until `stop` says enough (or 10 s pass).
    fn until(rx: &async_channel::Receiver<Event>, stop: impl Fn(&Event) -> bool) -> Vec<Event> {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(event) => {
                    let last = stop(&event);
                    seen.push(event);
                    if last {
                        return seen;
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        panic!("timed out; saw {seen:?}");
    }

    fn exited(event: &Event) -> bool {
        matches!(event, Event::Exited(..))
    }

    fn jobs() -> (Jobs, async_channel::Receiver<Event>) {
        Jobs::new(std::env::temp_dir())
    }

    #[test]
    fn runs_and_keeps_its_output() {
        let (jobs, rx) = jobs();
        let id = jobs.run_named("hi", "printf 'one\\ntwo\\n'; exit 3", false, None).unwrap();
        let seen = until(&rx, exited);
        assert_eq!(seen[..2], [Event::Added(id), Event::Started(id)]);
        assert_eq!(seen.last(), Some(&Event::Exited(id, Some(3))));
        assert_eq!(jobs.output(id, 10), "one\ntwo");
        assert_eq!(jobs.output(id, 1), "two");
        let info = jobs.named("hi").unwrap();
        assert!(!info.running);
        assert_eq!(info.exit_code, Some(3));
    }

    #[test]
    fn output_reaches_into_the_scrollback() {
        let (jobs, rx) = jobs();
        let id = jobs.run_named("seq", "seq 1 100", false, None).unwrap();
        until(&rx, exited);
        let text = jobs.output(id, 100);
        assert!(text.starts_with("1\n2\n"), "{text}");
        assert!(text.ends_with("99\n100"), "{text}");
    }

    #[test]
    fn one_job_per_name() {
        let (jobs, rx) = jobs();
        let first = jobs.run_named("s", "sleep 30", false, None).unwrap();
        until(&rx, |e| *e == Event::Started(first));
        assert!(jobs.run_named("s", "true", false, None).is_err());
        let again = jobs.run_named("s", "echo again", true, None).unwrap();
        assert_eq!(first, again);
        let seen = until(&rx, exited);
        assert!(seen.contains(&Event::Restarting(first)));
        assert_eq!(seen.last(), Some(&Event::Exited(first, Some(0))));
        assert_eq!(jobs.output(first, 5), "again");
        assert_eq!(jobs.list().len(), 1);
    }

    #[test]
    fn stop_ends_the_whole_group() {
        let (jobs, rx) = jobs();
        let id = jobs.run_named("s", "sleep 30 & sleep 30; wait", false, None).unwrap();
        until(&rx, |e| *e == Event::Started(id));
        jobs.stop(id);
        // Killed by SIGTERM: no exit code.
        assert_eq!(until(&rx, exited).last(), Some(&Event::Exited(id, None)));
    }

    #[test]
    fn keys_reach_the_program() {
        let (jobs, rx) = jobs();
        let id = jobs.run_named("read", "read line; echo got $line", false, None).unwrap();
        until(&rx, |e| *e == Event::Started(id));
        jobs.write(id, b"hello\r");
        until(&rx, exited);
        assert!(jobs.output(id, 5).ends_with("got hello"), "{}", jobs.output(id, 5));
    }

    #[test]
    fn the_size_reaches_the_program() {
        let (jobs, rx) = jobs();
        let id = jobs.run_named("size", "read; stty size", false, None).unwrap();
        until(&rx, |e| *e == Event::Started(id));
        jobs.resize(id, 40, 120);
        jobs.write(id, b"\r");
        until(&rx, exited);
        assert_eq!(jobs.output(id, 1), "40 120");
    }

    #[test]
    fn a_missing_folder_says_so() {
        let (jobs, rx) = jobs();
        let id = jobs.run_named("x", "true", false, Some(PathBuf::from("/nonexistent/loom"))).unwrap();
        assert_eq!(until(&rx, exited).last(), Some(&Event::Exited(id, Some(127))));
        assert!(jobs.output(id, 5).starts_with("loom: could not start"));
    }
}
