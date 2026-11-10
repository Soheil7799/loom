//! The strict sandbox (Settings → Advanced, off by default): agents and the jobs they start
//! run under Landlock, reaching no Unix socket but loom's own and the SSH and GPG agents'
//! (and no abstract one). That shuts the session bus and systemd's socket, so an agent can't
//! start a unit of its own (`systemd-run --user`, a timer) outside its scope: the way out of
//! being known as that agent (docs/trust.md). The cost: no `systemctl --user`, notify-send,
//! gsettings, podman or portals for agents, and no sudo (no new privileges): run those in the
//! tray yourself.
//!
//! `loom sandbox -- <program> <args>` enters it and becomes the program; `scope::wrap` puts
//! that in front of what agents run when the setting is on.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const CREATE_RULESET_VERSION: u32 = 1 << 0;
const ACCESS_FS_RESOLVE_UNIX: u64 = 1 << 16;
const SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
/// Signals only to processes in the same sandbox (killing loomd would undo the scopes).
const SCOPE_SIGNAL: u64 = 1 << 1;
const RULE_PATH_BENEATH: i32 = 1;
/// The ABI with ACCESS_FS_RESOLVE_UNIX (the one that matters here).
const ABI_NEEDED: i64 = 9;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

/// The kernel's Landlock ABI version (0: none).
fn abi() -> i64 {
    // SAFETY: the version query takes no ruleset.
    let v = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<RulesetAttr>(), 0usize, CREATE_RULESET_VERSION) };
    v.max(0)
}

/// Whether this kernel can hold agents to it.
pub fn available() -> bool {
    abi() >= ABI_NEEDED
}

/// Where agents may still connect: loom's socket's folder, the SSH agent's, GPG's.
pub fn allowed(env: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let parent = |p: String| Path::new(&p).parent().map(Path::to_path_buf);
    let mut allow: Vec<PathBuf> = Vec::new();
    allow.extend(env("LOOM_SOCKET").and_then(parent));
    allow.extend(env("SSH_AUTH_SOCK").and_then(parent));
    allow.extend(env("XDG_RUNTIME_DIR").map(|d| Path::new(&d).join("gnupg")));
    allow.extend(env("HOME").map(|h| Path::new(&h).join(".gnupg")));
    // Never the runtime folder or one above it (an SSH agent socket right in it would let in
    // the bus and systemd's socket beside it).
    // SAFETY: getuid can't fail.
    let runtime = env("XDG_RUNTIME_DIR").map_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })), PathBuf::from);
    allow.retain(|p| p.is_dir() && !runtime.starts_with(p));
    allow
}

/// This process held to the sandbox from now on (and what it runs). Err: not entered.
pub fn enter(allow: &[PathBuf]) -> Result<(), String> {
    if !available() {
        return Err(format!("the strict sandbox needs Landlock ABI {ABI_NEEDED} (this kernel has {})", abi()));
    }
    let attr = RulesetAttr { handled_access_fs: ACCESS_FS_RESOLVE_UNIX, handled_access_net: 0, scoped: SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL };
    // SAFETY: a valid attr of its own size.
    let ruleset = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, &attr, std::mem::size_of::<RulesetAttr>(), 0u32) } as i32;
    if ruleset < 0 {
        return Err(format!("landlock_create_ruleset: {}", std::io::Error::last_os_error()));
    }
    for dir in allow {
        let Ok(path) = CString::new(dir.as_os_str().as_bytes()) else { continue };
        // SAFETY: a NUL-terminated path.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            continue;
        }
        let rule = PathBeneathAttr { allowed_access: ACCESS_FS_RESOLVE_UNIX, parent_fd: fd };
        // SAFETY: a valid rule for an open fd.
        let added = unsafe { libc::syscall(libc::SYS_landlock_add_rule, ruleset, RULE_PATH_BENEATH, &rule, 0u32) };
        // SAFETY: our own fd.
        unsafe { libc::close(fd) };
        if added < 0 {
            return Err(format!("landlock_add_rule {}: {}", dir.display(), std::io::Error::last_os_error()));
        }
    }
    // SAFETY: plain prctl and the ruleset made above.
    let entered = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0 && libc::syscall(libc::SYS_landlock_restrict_self, ruleset, 0u32) == 0 };
    let err = std::io::Error::last_os_error();
    // SAFETY: our own fd.
    unsafe { libc::close(ruleset) };
    if entered { Ok(()) } else { Err(format!("landlock_restrict_self: {err}")) }
}

/// `program` as run under the sandbox, when the setting is on. Fails closed: with no `loom`
/// to enter it, what runs only says so and fails.
pub fn wrap(program: &[String]) -> Vec<String> {
    if !crate::settings::strict_sandbox() || program.is_empty() {
        return program.to_vec();
    }
    let Some(loom) = crate::paths::loom_program() else {
        return ["sh", "-c", "echo 'loom sandbox: the loom program is not beside loomd, so nothing runs' >&2; exit 1"].map(str::to_owned).to_vec();
    };
    let mut wrapped = vec![loom.to_string_lossy().into_owned(), "sandbox".to_owned(), "--".to_owned()];
    wrapped.extend_from_slice(program);
    wrapped
}

/// `loom sandbox -- <program> <args>`: enters the sandbox and becomes the program. Fails
/// closed: a sandbox that can't be entered runs nothing.
pub fn run(args: &[String]) -> i32 {
    use std::os::unix::process::CommandExt;
    let program = match args.first().map(String::as_str) {
        Some("--") => &args[1..],
        _ => args,
    };
    if program.is_empty() {
        eprintln!("usage: loom sandbox -- <program> [args]");
        return 2;
    }
    if let Err(err) = enter(&allowed(|name| std::env::var(name).ok())) {
        eprintln!("loom sandbox: {err}");
        return 1;
    }
    let err = std::process::Command::new(&program[0]).args(&program[1..]).exec();
    eprintln!("loom sandbox: {}: {err}", program[0]);
    127
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};

    #[test]
    fn an_ssh_agent_right_in_the_runtime_folder_opens_nothing() {
        let root = std::env::temp_dir().join(format!("loom-allowed-{}", std::process::id()));
        let (runtime, loom) = (root.join("run"), root.join("run/loom"));
        std::fs::create_dir_all(&loom).unwrap();
        let env = |name: &str| match name {
            "XDG_RUNTIME_DIR" => Some(runtime.to_string_lossy().into_owned()),
            "LOOM_SOCKET" => Some(loom.join("loom-api.sock").to_string_lossy().into_owned()),
            // systemd's ssh-agent.service: the socket right in the runtime folder
            "SSH_AUTH_SOCK" => Some(runtime.join("ssh-agent.socket").to_string_lossy().into_owned()),
            _ => None,
        };
        assert_eq!(allowed(env), vec![loom.clone()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// In a child process (the sandbox can't be left): a socket outside what is allowed is
    /// refused, one inside is reached.
    #[test]
    fn the_sandbox_lets_agents_reach_only_the_allowed_sockets() {
        if !available() {
            eprintln!("no Landlock ABI {ABI_NEEDED} here: skipped");
            return;
        }
        let root = std::env::temp_dir().join(format!("loom-sandbox-{}", std::process::id()));
        let (inside, outside) = (root.join("loom"), root.join("bus"));
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let _a = UnixListener::bind(inside.join("s")).unwrap();
        let _b = UnixListener::bind(outside.join("s")).unwrap();
        // SAFETY: the child only makes syscalls and exits.
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            let ok = enter(std::slice::from_ref(&inside)).is_ok() && UnixStream::connect(inside.join("s")).is_ok() && UnixStream::connect(outside.join("s")).is_err();
            // SAFETY: leaving the forked child at once.
            unsafe { libc::_exit(if ok { 0 } else { 1 }) };
        }
        let mut status = 0;
        // SAFETY: our own child.
        unsafe { libc::waitpid(pid, &mut status, 0) };
        let _ = std::fs::remove_dir_all(&root);
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0, "the sandboxed child saw the wrong sockets");
    }
}
