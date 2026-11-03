//! Where loom is: the socket its agents and their commands are told of (LOOM_SOCKET), now the
//! core API's (`api::socket`). The control protocol that was here (an agent naming itself in
//! each request) is gone: agents speak the API, which knows who they are.

use std::cell::RefCell;
use std::path::PathBuf;

thread_local! {
    static PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// The name sockets of this app id are made from ($XDG_RUNTIME_DIR/loom/<name>.sock): the API's
/// and the bridges' sit beside it, named from its stem.
pub fn socket_path(app_id: &str) -> PathBuf {
    let name = crate::state::suffix(app_id).map_or("loom".to_owned(), |s| format!("loom-{s}"));
    crate::paths::runtime_dir().join("loom").join(format!("{name}.sock"))
}

/// The socket agents are told of (LOOM_SOCKET), once loom listens.
pub fn current() -> Option<PathBuf> {
    PATH.with(|p| p.borrow().clone())
}

/// Says where loom listens (the API socket).
pub fn set_current(path: PathBuf) {
    PATH.with(|p| *p.borrow_mut() = Some(path));
}
