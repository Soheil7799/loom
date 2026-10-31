//! The core's thread. Agents and their connections are single-threaded (`Rc`, `RefCell`): they
//! run on one thread, the one the UI sets a spawner on. The GTK app gives GLib's main loop; a
//! UI with no loop of its own runs the core on a thread of its own with any local executor.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;

type Spawner = Box<dyn Fn(Pin<Box<dyn Future<Output = ()>>>)>;

thread_local! {
    static SPAWNER: RefCell<Option<Spawner>> = const { RefCell::new(None) };
}

/// How futures are run on this thread (set once, before any agent starts).
pub fn set_spawner(spawn: impl Fn(Pin<Box<dyn Future<Output = ()>>>) + 'static) {
    SPAWNER.with(|s| *s.borrow_mut() = Some(Box::new(spawn)));
}

/// Runs `future` on the core's thread.
pub fn spawn_local(future: impl Future<Output = ()> + 'static) {
    SPAWNER.with(|s| match s.borrow().as_ref() {
        Some(spawn) => spawn(Box::pin(future)),
        None => panic!("loom_core::rt: no spawner set on this thread"),
    });
}

/// Runs `f` on the core's thread, after what it is doing now.
pub fn idle_once(f: impl FnOnce() + 'static) {
    spawn_local(async move { f() });
}

/// Runs `f` on a thread of its own; its result, awaited on the core's thread.
pub async fn spawn_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, async_channel::RecvError> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(f());
    });
    rx.recv().await
}

/// A random (v4) UUID, as text.
pub fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}
