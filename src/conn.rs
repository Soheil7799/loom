//! The window's way to loom's core API: the core in this process, or loomd's over its socket
//! once the window is its client. The rest of the window follows resources and does commands
//! through here, the same either way.

use std::cell::RefCell;

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
    let (who, said) = (who.to_owned(), method.to_owned());
    let reply = move |result: Result<Value, loom_core::api::Error>| match result {
        Ok(value) => then(Some(value)),
        Err(err) => {
            loom_core::log::debug!(who, "{said} refused: {err}");
            then(None);
        }
    };
    match remote() {
        Some(remote) => remote.command_later(&loom_core::rt::uuid(), method, params, reply),
        None => loom_core::api::hub().connect(Principal::Window).command_later(&loom_core::rt::uuid(), method, params, reply),
    }
}
