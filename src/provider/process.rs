//! A provider's process: started in the agent's folder, one JSON object per line each way.
//!
//! It runs in its own process group, so stopping it also stops what it started (Claude Code's
//! own children: commands, MCP servers), which would otherwise linger and keep their memory.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use serde_json::Value;

pub struct Process {
    stdin: RefCell<Option<ChildStdin>>,
    child: RefCell<Option<Child>>,
    pid: u32,
    /// "acp 1234", "claude 1234": how the debug log names it.
    tag: String,
}

/// Where the providers' stderr goes.
pub fn log_path() -> PathBuf {
    crate::state::dir().join("agents.log")
}

impl Process {
    /// Starts `program` in `cwd`; `on_line` gets each line read (None: the output ended).
    pub fn spawn(program: &[String], cwd: &Path, name: &str, on_line: impl Fn(Option<Value>) + 'static) -> std::io::Result<Rc<Process>> {
        let log = log_path();
        if let Some(dir) = log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let stderr = std::fs::OpenOptions::new().create(true).append(true).open(&log)?;
        let mut command = Command::new(&program[0]);
        command
            .args(&program[1..])
            .current_dir(cwd)
            // Started from inside a Claude Code session (tests), Claude Code would refuse to nest.
            .env_remove("CLAUDECODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
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
        let process = Rc::new(Process { stdin: RefCell::new(child.stdin.take()), child: RefCell::new(Some(child)), pid, tag });
        glib::spawn_future_local(async move {
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

    pub fn tag(&self) -> &str {
        &self.tag
    }

    pub fn write(&self, value: &Value) {
        if let Some(stdin) = self.stdin.borrow_mut().as_mut() {
            let mut line = value.to_string();
            crate::log::debug!(self.tag.clone(), "→ {} [{} bytes]", crate::log::cut(&line), line.len());
            line.push('\n');
            if stdin.write_all(line.as_bytes()).and_then(|()| stdin.flush()).is_err() {
                eprintln!("loom: the agent is not reading");
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
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            // Whatever is left of the group (the leader may go first and leave its children).
            // SAFETY: as above.
            unsafe { libc::kill(-group, libc::SIGKILL) };
            let _ = child.wait();
        });
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}
