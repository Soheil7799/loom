//! loom's core API (warp): every face of loom (the GTK window, the bridges, the agents' tools,
//! later a web UI or a terminal one) reads the core by subscribing to resources and acts on
//! it by commands. A subscription answers a snapshot (in pages, then `synced`), then numbered
//! `changes`; one that falls behind gets a `resync` and a fresh snapshot instead of a backlog.
//! `research/core-api/00-decisions.md`, beside the repository, is the law it follows.
//!
//! In-process for now: a [`Client`] is a connection with a [`Principal`], and its messages
//! are the ones a socket will carry later, unchanged.

mod store;
pub mod wire;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;

use serde_json::Value;

pub use store::{Limits, Store, Subscription};
pub use wire::{Body, Message, Op};

/// Who is on the other end of a connection, fixed by loom when the connection is made (never
/// claimed by the client): what it may read and do follows from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Principal {
    /// loom's own window: everything.
    Window,
    /// The command-line client, run by the user.
    Cli,
    /// An agent, through its loom tools: the agent list and its own conversation.
    Agent(String),
    /// A bridge (Telegram, a paired device), named for the audit log.
    Bridge(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No such resource (or not one this principal may know of).
    NotFound(String),
    Forbidden(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(r) => write!(f, "no resource {r}"),
            Error::Forbidden(r) => write!(f, "not allowed to read {r}"),
        }
    }
}

impl std::error::Error for Error {}

/// May `who` read `resource`?
fn may_read(who: &Principal, resource: &str) -> bool {
    match who {
        Principal::Window | Principal::Cli | Principal::Bridge(_) => true,
        Principal::Agent(me) => resource == "agents" || resource.strip_prefix("conversation/") == Some(me.as_str()),
    }
}

/// The resources, by name (`agents`, `conversation/<agent id>`…). Cloning gives another
/// handle to the same ones.
#[derive(Clone)]
pub struct Hub(std::rc::Rc<HubInner>);

struct HubInner {
    stores: RefCell<HashMap<String, Store>>,
    limits: Limits,
    auto: bool,
}

impl Hub {
    /// Changes go out by themselves, once the core's thread (`rt`) is idle.
    pub fn new() -> Hub {
        Hub::with(Limits::default(), true)
    }

    /// For tests: changes go out on `Store::flush` only (no main loop needed).
    pub fn manual(limits: Limits) -> Hub {
        Hub::with(limits, false)
    }

    fn with(limits: Limits, auto: bool) -> Hub {
        Hub(std::rc::Rc::new(HubInner { stores: RefCell::default(), limits, auto }))
    }

    /// The resource `name`, made empty if it isn't there yet.
    pub fn store(&self, name: &str) -> Store {
        self.0.stores.borrow_mut().entry(name.to_string()).or_insert_with(|| Store::new(name, self.0.limits, self.0.auto)).clone()
    }

    /// Ends the resource `name`: its subscribers get `gone`.
    pub fn close(&self, name: &str) {
        let store = self.0.stores.borrow_mut().remove(name);
        if let Some(store) = store {
            store.close();
        }
    }

    /// A connection for `who`.
    pub fn connect(&self, who: Principal) -> Client {
        Client { hub: self.clone(), who, next: Cell::new(0) }
    }
}

thread_local! {
    static HUB: Hub = Hub::new();
}

/// The core's resources (one set, on the core's thread).
pub fn hub() -> Hub {
    HUB.with(Hub::clone)
}

/// Ends resource `name` of the core's hub (nothing, if the thread is ending).
pub fn close(name: &str) {
    let _ = HUB.try_with(|hub| hub.close(name));
}

/// A time as the API gives times: RFC 3339, UTC (`2026-11-03T12:00:00Z`).
pub fn time(unix_seconds: i64) -> Value {
    jiff::Timestamp::from_second(unix_seconds).map_or(Value::Null, |t| Value::String(t.to_string()))
}

impl Default for Hub {
    fn default() -> Self {
        Hub::new()
    }
}

/// One connection to the core, in-process.
pub struct Client {
    hub: Hub,
    who: Principal,
    next: Cell<u64>,
}

impl Client {
    pub fn principal(&self) -> &Principal {
        &self.who
    }

    /// Follows `resource`: its snapshot waits in the subscription already; `ready` is called
    /// when more does.
    pub fn subscribe(&self, resource: &str, ready: impl Fn() + 'static) -> Result<Subscription, Error> {
        let store = self.hub.0.stores.borrow().get(resource).cloned();
        let Some(store) = store else {
            return Err(Error::NotFound(resource.to_string()));
        };
        if !may_read(&self.who, resource) {
            // An agent learns nothing of what it may not read, not even that it exists.
            return Err(match self.who {
                Principal::Agent(_) => Error::NotFound(resource.to_string()),
                _ => Error::Forbidden(resource.to_string()),
            });
        }
        self.next.set(self.next.get() + 1);
        Ok(store.subscribe(format!("s{}", self.next.get()), Box::new(ready)))
    }
}

/// A client's copy of one resource, kept by applying its messages: what every client does,
/// here for the GTK window and for tests.
#[derive(Debug, Default)]
pub struct Mirror {
    pub items: Vec<Value>,
    /// Whether the snapshot is whole (`synced` came, and no resync since).
    pub synced: bool,
    pub gone: bool,
    /// The last `seq` seen.
    pub seq: u64,
    /// Pages of a snapshot under way (it replaces `items` once `synced`).
    incoming: Option<Vec<Value>>,
}

/// What applying a message did.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    /// `items` changed (or a snapshot completed): draw them.
    Changed,
    /// Nothing to draw yet (a page of a snapshot, an unknown message).
    Nothing,
    /// A `seq` was skipped: ask for a resync (`Subscription::resync`).
    Gap,
}

impl Mirror {
    pub fn apply(&mut self, m: &Message) -> Applied {
        if self.seq != 0 && m.seq != self.seq + 1 {
            self.seq = m.seq;
            self.synced = false;
            return Applied::Gap;
        }
        self.seq = m.seq;
        match &m.body {
            Body::Page { items } => {
                self.incoming.get_or_insert_with(Vec::new).extend(items.iter().cloned());
                Applied::Nothing
            }
            Body::Synced => {
                self.items = self.incoming.take().unwrap_or_default();
                self.synced = true;
                Applied::Changed
            }
            Body::Changes { ops } => {
                for op in ops {
                    wire::apply(&mut self.items, op);
                }
                Applied::Changed
            }
            Body::Resync { .. } => {
                self.synced = false;
                self.incoming = Some(Vec::new());
                Applied::Nothing
            }
            Body::Gone => {
                self.gone = true;
                self.items.clear();
                Applied::Changed
            }
            Body::Unknown => Applied::Nothing,
        }
    }
}

#[cfg(test)]
mod tests;
