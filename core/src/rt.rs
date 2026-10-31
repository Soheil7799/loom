//! The core's thread. Agents and their connections are single-threaded (`Rc`, `RefCell`): they
//! run on one thread, whose host the UI sets. The GTK app gives GLib's main loop; a UI with no
//! loop of its own runs the core on a thread of its own with any local executor and timer.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

type Spawn = Box<dyn Fn(Pin<Box<dyn Future<Output = ()>>>)>;
type After = Box<dyn Fn(Duration, Box<dyn FnOnce()>)>;

thread_local! {
    static HOST: RefCell<Option<(Spawn, After)>> = const { RefCell::new(None) };
}

/// How this thread runs futures, and calls something after a while (set once, before any
/// agent starts).
pub fn set_host(spawn: impl Fn(Pin<Box<dyn Future<Output = ()>>>) + 'static, after: impl Fn(Duration, Box<dyn FnOnce()>) + 'static) {
    HOST.with(|h| *h.borrow_mut() = Some((Box::new(spawn), Box::new(after))));
}

/// Runs `future` on the core's thread.
pub fn spawn_local(future: impl Future<Output = ()> + 'static) {
    HOST.with(|h| match h.borrow().as_ref() {
        Some((spawn, _)) => spawn(Box::pin(future)),
        None => panic!("loom_core::rt: no host set on this thread"),
    });
}

/// Runs `f` on the core's thread, after what it is doing now.
pub fn idle_once(f: impl FnOnce() + 'static) {
    spawn_local(async move { f() });
}

/// A call to come; `remove` calls it off.
pub struct Timer(Rc<Cell<bool>>);

impl Timer {
    pub fn remove(self) {
        self.0.set(true);
    }
}

/// Calls `f` once, `after` from now (unless removed first).
pub fn timeout_once(after: Duration, f: impl FnOnce() + 'static) -> Timer {
    let off = Rc::new(Cell::new(false));
    let called_off = off.clone();
    HOST.with(|h| match h.borrow().as_ref() {
        Some((_, later)) => later(
            after,
            Box::new(move || {
                if !called_off.get() {
                    f();
                }
            }),
        ),
        None => panic!("loom_core::rt: no host set on this thread"),
    });
    Timer(off)
}

/// Calls `f` every `interval` while it returns true (and the timer is not removed).
pub fn every(interval: Duration, f: impl FnMut() -> bool + 'static) -> Timer {
    fn next(interval: Duration, mut f: Box<dyn FnMut() -> bool>, off: Rc<Cell<bool>>) {
        let again = off.clone();
        let _ = timeout_once(interval, move || {
            if !again.get() && f() {
                next(interval, f, again);
            }
        });
    }
    let off = Rc::new(Cell::new(false));
    next(interval, Box::new(f), off.clone());
    Timer(off)
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
