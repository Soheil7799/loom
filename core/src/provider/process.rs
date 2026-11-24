//! A provider's process: started in the agent's folder, one JSON object per line each way.
//!
//! It runs in its own process group, so stopping it also stops what it started (Claude Code's
//! own children: commands, MCP servers), which would otherwise linger and keep their memory.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

use serde_json::Value;

/// agents.log is started over (the old one kept as agents.log.1) past this size.
const LOG_MOST: u64 = 8 << 20;

pub struct Process {
    /// Lines for its stdin, written by a thread of their own: a process that stops reading
    /// fills the pipe, and that must not stop loom's core thread. None once stopped.
    stdin: RefCell<Option<std::sync::mpsc::SyncSender<Vec<u8>>>>,
    child: RefCell<Option<Child>>,
    pid: u32,
    /// "claude 1234": how the debug log names it.
    tag: String,
}

/// Where the providers' stderr goes.
pub fn log_path() -> PathBuf {
    crate::state::dir().join("agents.log")
}

impl Process {
    /// Starts `program` in `cwd` (with `env` set), in a scope of `agent`'s if it is an agent's
    /// (`scope`); `on_line` gets each line read (None: the output ended).
    pub fn spawn(program: &[String], cwd: &Path, env: &[(&str, &str)], name: &str, agent: Option<u32>, on_line: impl Fn(Option<Value>) + 'static) -> std::io::Result<Rc<Process>> {
        let program = &match agent {
            Some(agent) => crate::scope::wrap(program, agent),
            None => program.to_vec(),
        };
        let log = log_path();
        if let Some(dir) = log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::metadata(&log).is_ok_and(|m| m.len() > LOG_MOST) {
            let _ = std::fs::rename(&log, log.with_extension("log.1"));
        }
        // 0600 like every other log (an agent's stderr may hold anything).
        let stderr = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&log)?;
        let _ = stderr.set_permissions(std::fs::Permissions::from_mode(0o600));
        let mut command = Command::new(&program[0]);
        command
            .args(&program[1..])
            .current_dir(cwd)
            // Started from inside a Claude Code session (tests), Claude Code would refuse to nest.
            .env_remove("CLAUDECODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .envs(env.iter().copied())
            .process_group(0);
        // loom started from a herdr pane: its agents are not in that pane (herdr's hooks would
        // report them there, and CLAUDE.md's herdr rules would apply).
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("HERDR_") {
                command.env_remove(name);
            }
        }
        // So the agent, its commands and its hooks know they run in loom.
        if let Some(socket) = crate::control::current() {
            command.env("LOOM_SOCKET", socket);
        }
        // SAFETY: prctl only, between fork and exec.
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let pid = child.id();
        let tag = format!("{name} {pid}");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = async_channel::unbounded::<Option<Value>>();
        let reader_tag = tag.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).split(b'\n') {
                let Ok(line) = line else { break };
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                // Logged as read (here, off the main loop): only the part kept.
                if crate::log::on() {
                    let text = String::from_utf8_lossy(&line[..line.len().min(8000)]);
                    crate::log::write(&reader_tag, &format!("← {} [{} bytes]", crate::log::cut(&text), line.len()));
                }
                match serde_json::from_slice(&line) {
                    Ok(value) => {
                        if tx.send_blocking(Some(value)).is_err() {
                            return;
                        }
                    }
                    Err(err) => eprintln!("loom: unreadable line from the agent ({err})"),
                }
            }
            let _ = tx.send_blocking(None);
        });
        crate::log::debug!(tag.clone(), "started {} in {}", program.join(" "), cwd.display());
        // Bounded: an agent that stops reading can't make loom keep its lines without end.
        let (to_stdin, lines) = std::sync::mpsc::sync_channel::<Vec<u8>>(256);
        if let Some(mut stdin) = child.stdin.take() {
            std::thread::spawn(move || {
                // Ends when the Process drops its sender (stopped), closing the pipe with it.
                for line in lines {
                    if stdin.write_all(&line).and_then(|()| stdin.flush()).is_err() {
                        eprintln!("loom: the agent is not reading");
                        break;
                    }
                }
            });
        }
        let process = Rc::new(Process { stdin: RefCell::new(Some(to_stdin)), child: RefCell::new(Some(child)), pid, tag });
        crate::rt::spawn_local(async move {
            while let Ok(line) = rx.recv().await {
                let end = line.is_none();
                on_line(line);
                if end {
                    break;
                }
            }
        });
        Ok(process)
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn write(&self, value: &Value) {
        if let Some(stdin) = self.stdin.borrow_mut().as_mut() {
            let mut line = value.to_string();
            crate::log::debug!(self.tag.clone(), "→ {} [{} bytes]", crate::log::cut(&line), line.len());
            line.push('\n');
            if stdin.try_send(line.into_bytes()).is_err() {
                eprintln!("loom: the agent is not reading; a message to it was dropped");
            }
        }
    }

    /// Stops it and everything it started: SIGTERM to the group, SIGKILL after 3 s.
    pub fn stop(&self) {
        self.stdin.borrow_mut().take();
        let Some(mut child) = self.child.borrow_mut().take() else { return };
        let group = child.id() as i32;
        // SAFETY: plain kill(2) on the group this process leads.
        unsafe { libc::kill(-group, libc::SIGTERM) };
        std::thread::spawn(move || {
            for _ in 0..30 {
                if exited(group) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            // Whatever is left of the group (the leader may go first and leave its children).
            // The leader is not reaped yet, so its id (the group's) can't have gone to another.
            // SAFETY: as above.
            unsafe { libc::kill(-group, libc::SIGKILL) };
            let _ = child.wait();
        });
    }
}

/// Whether child `pid` has exited, leaving it unreaped (WNOWAIT): its id stays taken.
fn exited(pid: i32) -> bool {
    // SAFETY: waitid(2) fills `info`, a zeroed siginfo_t of our own.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) == 0 && info.si_pid() != 0
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_exited_child_stays_unreaped_until_waited() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id() as i32;
        let start = std::time::Instant::now();
        while !super::exited(pid) {
            assert!(start.elapsed().as_secs() < 5, "never seen to exit");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Still ours to reap: the id was not given back in between.
        assert!(child.wait().unwrap().success());
    }
}
