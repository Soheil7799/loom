//! The next message, guessed: after a turn, Claude Haiku reads the last exchange and says
//! what the user will likely write next (as Claude Code does; its adapter keeps its own
//! guess for one client only). Run through the user's `claude` login, in a folder of loom's
//! own with no user settings, tools, MCP servers or saved session: about 700 tokens a turn.

use std::io::Write;
use std::process::{Command, Stdio};

use gtk::{gio, glib};

const INSTRUCTIONS: &str = "You guess the next message a developer will send to their AI coding agent, from the last \
exchange between them. Reply with only that message, as the developer would type it: their language, their tone, short \
(2 to 12 words), no quotes, no preamble. If the agent asked them something, answer it the likely way. If there is no \
clear next step, reply with nothing at all.";

/// The guess for `user` (what was sent) and `reply` (what came back), to `done` on the main
/// loop; None if there is none (or `claude` can't be run).
pub fn guess(user: String, reply: String, done: impl FnOnce(Option<String>) + 'static) {
    glib::spawn_future_local(async move {
        let found = gio::spawn_blocking(move || run(&user, &reply)).await.ok().flatten();
        done(found);
    });
}

fn run(user: &str, reply: &str) -> Option<String> {
    // The ends matter most: what was asked last, how the reply ended.
    let tail = |text: &str, most: usize| -> String {
        let count = text.chars().count();
        text.chars().skip(count.saturating_sub(most)).collect()
    };
    let input = format!("THE DEVELOPER WROTE:\n{}\n\nTHE AGENT REPLIED:\n{}", tail(user, 2000), tail(reply, 3000));
    let dir = crate::state::dir().join("suggest");
    std::fs::create_dir_all(&dir).ok()?;
    let mut child = Command::new("timeout")
        .args(["60", "claude", "-p", "--model", "haiku", "--setting-sources", "project", "--tools", "", "--strict-mcp-config", "--no-session-persistence"])
        .args(["--system-prompt", INSTRUCTIONS])
        .current_dir(&dir)
        // Started from inside a Claude Code session (tests), it would refuse to nest.
        .env_remove("CLAUDECODE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(input.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().trim_matches(['"', '“', '”', '\'']).trim().to_owned();
    crate::log::debug!("suggest", "exit {:?}: {text:?}", output.status.code());
    (output.status.success() && !text.is_empty() && text.chars().count() <= 200 && !text.contains('\n')).then_some(text)
}
