//! Each agent's processes in a systemd scope of their own (`loom-<loom's pid>-a<agent>-<n>.scope`),
//! so the core API knows a process as that agent's from its cgroup, which the process can't
//! leave on its own: `nohup`, `setsid`, `cmd &`, tmux and a parent that ended all keep it in.
//! (What starts a new unit, `systemd-run --user` or a timer, does leave; see docs/trust.md.)
//!
//! Without a user systemd (another init, a container), nothing is wrapped and loom goes by
//! the process tree alone.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

static WORKS: OnceLock<bool> = OnceLock::new();
/// This systemd takes --expand-environment=no (254 and later, which expand $VAR in the
/// command unless told not to; older ones never did).
static NO_EXPAND: OnceLock<bool> = OnceLock::new();

/// Finds out, off the core's thread, whether scopes work here (call once, early: loom's
/// start). Until it knows, nothing is wrapped (loom goes by the process tree).
pub fn probe() {
    std::thread::spawn(|| {
        WORKS.get_or_init(works);
    });
}

/// Whether `systemd-run --user --scope` works here, as far as known (never waits).
pub fn available() -> bool {
    WORKS.get().copied().unwrap_or(false)
}

fn works() -> bool {
    if std::env::var_os("LOOM_NO_SCOPES").is_some() {
        return false;
    }
    let probe = |flag: &[&str]| {
        std::process::Command::new("systemd-run")
            .args(["--user", "--scope", "--quiet", "--collect"])
            .args(flag)
            .args(["--", "true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if probe(&["--expand-environment=no"]) {
        let _ = NO_EXPAND.set(true);
        return true;
    }
    probe(&[])
}

/// `program` run in a new scope of agent `agent`'s (as it is, when scopes aren't available).
pub fn wrap(program: &[String], agent: u32) -> Vec<String> {
    // inside the scope, the strict sandbox (if on): systemd-run itself needs the bus
    let program = &crate::sandbox::wrap(program);
    if !available() || program.is_empty() {
        return program.to_vec();
    }
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let unit = format!("{}{agent}-{}", prefix(), NEXT.fetch_add(1, Ordering::Relaxed));
    // The command as given: systemd-run would otherwise expand $VAR and ${…} in it first
    // (`${#s}`, `$1`, `${x:-y}` emptied before bash sees them).
    let mut wrapped: Vec<String> = ["systemd-run", "--user", "--scope", "--quiet", "--collect"].map(str::to_owned).to_vec();
    if NO_EXPAND.get() == Some(&true) {
        wrapped.push("--expand-environment=no".to_owned());
    }
    wrapped.extend(["--unit".to_owned(), unit, "--".to_owned()]);
    wrapped.extend_from_slice(program);
    wrapped
}

/// This loom's scopes' names start so (another loom's agents are not ours).
fn prefix() -> String {
    format!("loom-{}-a", std::process::id())
}

/// The agent whose scope process `pid` is in, if it is in one of this loom's.
pub fn agent_of(pid: u32) -> Option<u32> {
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    agent_in(&cgroup, &prefix())
}

/// The agent of a `/proc/<pid>/cgroup` text, given the scopes' prefix.
fn agent_in(cgroup: &str, prefix: &str) -> Option<u32> {
    // cgroup v2: one line, `0::/user.slice/…/loom-1234-a5-2.scope`.
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    path.split('/').find_map(|part| part.strip_suffix(".scope")?.strip_prefix(prefix)?.split('-').next()?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_names_its_agent() {
        let line = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/loom-77-a12-3.scope\n";
        assert_eq!(agent_in(line, "loom-77-a"), Some(12));
        assert_eq!(agent_in(line, "loom-78-a"), None, "another loom's");
        assert_eq!(agent_in("0::/user.slice/app.slice/kitty.scope\n", "loom-77-a"), None);
        assert_eq!(agent_in("0::/user.slice/loom-77-a12-3.scope/sub\n", "loom-77-a"), Some(12), "in a cgroup below it");
    }

    /// Where systemd runs (the user's desktop): a process left behind in an agent's scope is
    /// still that agent's once its parent has gone.
    #[test]
    fn a_process_left_running_stays_in_its_agents_scope() {
        if !*WORKS.get_or_init(works) {
            eprintln!("no user systemd here: skipped");
            return;
        }
        let program = wrap(&["sh".into(), "-c".into(), "setsid nohup sleep 5 >/dev/null 2>&1 & echo $!".into()], 42);
        let out = std::process::Command::new(&program[0]).args(&program[1..]).output().unwrap();
        let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        assert_eq!(agent_of(pid), Some(42));
        // the shell gets the command whole: systemd-run expands nothing in it
        let program = wrap(&["sh".into(), "-c".into(), "x=abc; echo ${#x} ${x:-none}".into()], 42);
        let out = std::process::Command::new(&program[0]).args(&program[1..]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "3 abc");
        assert_eq!(agent_of(std::process::id()), None);
        // SAFETY: a plain kill of the sleep started above.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
}
