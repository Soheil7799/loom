//! loom's core API (warp): every face of loom (the GTK window, the bridges, the agents' tools,
//! later a web UI or a terminal one) reads the core by subscribing to resources and acts on
//! it by commands. A subscription answers a snapshot (in pages, then `synced`), then numbered
//! `changes`; one that falls behind gets a `resync` and a fresh snapshot instead of a backlog.
//! `research/core-api/00-decisions.md`, beside the repository, is the law it follows.
//!
//! In-process for now: a [`Client`] is a connection with a [`Principal`], and its messages
//! are the ones a socket will carry later, unchanged.
//!
//! Commands (`send`, `answer`, `stop`…) carry an id the client makes: sent again with the same
//! id, a command is not done again, the first result comes back. Their effect shows as
//! changes to resources, for every client; where several may act (a question), the first
//! wins and the others are told it was answered.

pub mod blobs;
pub mod cli;
pub mod rpc;
pub mod shared;
pub mod spec;
pub mod web;
pub mod socket;
mod store;
pub mod wire;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::rc::Rc;

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
    /// No such resource, command or target (or not one this principal may know of).
    NotFound(String),
    /// Not for this principal.
    Forbidden(String),
    /// The command's parameters don't make sense.
    Invalid(String),
    /// Too late: someone else acted first (a question already answered).
    Conflict(String),
    /// It was tried and didn't work (the words say why, as they are).
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(what) => write!(f, "not found: {what}"),
            Error::Forbidden(what) => write!(f, "not allowed: {what}"),
            Error::Invalid(why) => write!(f, "invalid: {why}"),
            Error::Conflict(why) | Error::Failed(why) => write!(f, "{why}"),
        }
    }
}

/// What a command does, given who sent it and its parameters (an object).
pub type Handler = Rc<dyn Fn(&Principal, &Value) -> Result<Value, Error>>;

/// Where a command answered later puts its result (called once).
pub type Reply = Box<dyn FnOnce(Result<Value, Error>)>;

/// What a command answered later does: it calls `Reply` when done (a picture converted, a
/// recording heard), on the core's thread.
pub type Later = Rc<dyn Fn(&Principal, &Value, Reply)>;

#[derive(Clone)]
enum Handling {
    Now(Handler),
    Later(Later),
}

/// A command done: (principal, id), and its result.
type Done = ((String, String), Result<Value, Error>);

/// Sets up a resource made when first asked for (given the rest of its name); false: no such
/// resource.
pub type Provider = Rc<dyn Fn(&Store, &str) -> bool>;

/// Command results kept to answer a replay (per principal and id), the oldest dropped first.
const REMEMBERED: usize = 512;

impl std::error::Error for Error {}

/// What hosts the core in this process: `window` (loom's GTK app) or `daemon` (loomd).
static HOST_KIND: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// Says what hosts the core here (once, at start; `window` if never said).
pub fn set_host_kind(kind: &'static str) {
    let _ = HOST_KIND.set(kind);
}

pub fn host_kind() -> &'static str {
    HOST_KIND.get().copied().unwrap_or("window")
}

/// Whether this loom is a test instance (started with `LOOM_TEST_INSTANCE=1`, read once):
/// there the CLI may do what only people in loom's window and its bridges may otherwise
/// (answer agents' questions), so end-to-end tests can drive it. Set when loom starts,
/// never by a client: a loom started this way hosts none of the user's agents.
pub fn test_instance() -> bool {
    static TEST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TEST.get_or_init(|| std::env::var("LOOM_TEST_INSTANCE").as_deref() == Ok("1"))
}

/// Whether command `method` only reads (asked again and again, as `loom_wait` does): such
/// a command is neither kept for a replay nor written to the audit log.
pub fn reads_only(method: &str, params: &Value) -> bool {
    match method {
        "blob" | "search_notes" => true,
        "tool" => matches!(params["name"].as_str(), Some("job_output" | "list_jobs" | "list_agents" | "notes_info" | "edit_state")),
        "bridge" => matches!(
            params["method"].as_str(),
            Some("hello" | "agents" | "history" | "files" | "file" | "usage" | "last_reply" | "notes" | "notes_list" | "note_read" | "notes_search")
        ),
        _ => false,
    }
}

/// May `who` read `resource`?
fn may_read(who: &Principal, resource: &str) -> bool {
    // The notes vault: the window, the CLI and the paired devices (not the Telegram bot).
    let vault = resource.starts_with("vault/") || resource.starts_with("note/") || resource == "notes_chats";
    // A bridge's events: that bridge's alone.
    if let Some(name) = resource.strip_prefix("bridge/") {
        return *who == Principal::Bridge(name.to_owned());
    }
    // The bridges' states (a pairing ticket among them): people's.
    if resource == "bridges" {
        return matches!(who, Principal::Window | Principal::Cli);
    }
    match who {
        Principal::Window | Principal::Cli => true,
        Principal::Bridge(name) => !vault || name == "remote",
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
    handlers: RefCell<HashMap<String, Handling>>,
    /// Commands answered later, still being done: (principal, id).
    pending: RefCell<Vec<(String, String)>>,
    /// Resources made when first asked for, by the start of their name (`vault/`).
    providers: RefCell<Vec<(String, Provider)>>,
    /// The names of those made so far (dropped once unfollowed, when another is made).
    provided: RefCell<Vec<String>>,
    /// Commands done: (principal, id) and their result, newest last.
    done: RefCell<VecDeque<Done>>,
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
        Hub(std::rc::Rc::new(HubInner { stores: RefCell::default(), limits, auto, handlers: RefCell::default(), pending: RefCell::default(), providers: RefCell::default(), provided: RefCell::default(), done: RefCell::default() }))
    }

    /// Resources named `prefix…` are made when a client first asks for one: `provide` sets
    /// the new store up (given the rest of the name), or says there is no such resource.
    pub fn provide(&self, prefix: &str, provide: impl Fn(&Store, &str) -> bool + 'static) {
        self.0.providers.borrow_mut().push((prefix.to_string(), Rc::new(provide)));
    }

    /// The resource `name`, if it is there (none is made).
    pub fn existing(&self, name: &str) -> Option<Store> {
        self.0.stores.borrow().get(name).cloned()
    }

    /// The resources whose names start with `prefix`.
    pub fn stores_with(&self, prefix: &str) -> Vec<Store> {
        self.0.stores.borrow().iter().filter(|(name, _)| name.starts_with(prefix)).map(|(_, store)| store.clone()).collect()
    }

    /// The resource `name`, if it is there or can be made.
    fn find(&self, name: &str) -> Option<Store> {
        if let Some(store) = self.0.stores.borrow().get(name).cloned() {
            return Some(store);
        }
        let provider = self.0.providers.borrow().iter().find(|(prefix, _)| name.starts_with(prefix.as_str())).map(|(prefix, p)| (prefix.len(), p.clone()));
        let (at, provide) = provider?;
        let store = Store::new(name, self.0.limits, self.0.auto);
        if !provide(&store, &name[at..]) {
            return None;
        }
        // Made when asked for, they go once nobody follows them (a note read once, a
        // folder looked into): they are made again when asked for again.
        let mut provided = self.0.provided.borrow_mut();
        let mut stores = self.0.stores.borrow_mut();
        provided.retain(|made| stores.get(made).is_some_and(Store::followed) || stores.remove(made).is_none());
        provided.push(name.to_string());
        stores.insert(name.to_string(), store.clone());
        Some(store)
    }

    /// What command `method` does (the part of the core that owns it says, once).
    pub fn handle(&self, method: &str, handler: impl Fn(&Principal, &Value) -> Result<Value, Error> + 'static) {
        debug_assert!(!rpc::RESERVED.contains(&method), "{method} is one of the protocol's own methods");
        self.0.handlers.borrow_mut().insert(method.to_string(), Handling::Now(Rc::new(handler)));
    }

    /// What command `method` does, answered later: `handler` calls the `Reply` when done.
    pub fn handle_later(&self, method: &str, handler: impl Fn(&Principal, &Value, Reply) + 'static) {
        debug_assert!(!rpc::RESERVED.contains(&method), "{method} is one of the protocol's own methods");
        self.0.handlers.borrow_mut().insert(method.to_string(), Handling::Later(Rc::new(handler)));
    }

    /// Keeps `result` for a replay of command `key`.
    fn remember(&self, key: (String, String), result: Result<Value, Error>) {
        let mut done = self.0.done.borrow_mut();
        done.push_back((key, result));
        if done.len() > REMEMBERED {
            done.pop_front();
        }
    }

    /// The commands there are, sorted (what `initialize` tells a client).
    pub fn commands(&self) -> Vec<String> {
        let mut names: Vec<String> = self.0.handlers.borrow().keys().cloned().collect();
        names.sort();
        names
    }

    pub fn has_command(&self, method: &str) -> bool {
        self.0.handlers.borrow().contains_key(method)
    }

    pub fn limits(&self) -> Limits {
        self.0.limits
    }

    /// Whether changes go out by themselves (see `Hub::new`, `Hub::manual`).
    pub fn auto(&self) -> bool {
        self.0.auto
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

/// Takes item `id` out of resource `name` of the core's hub (nothing, if the thread is ending).
pub fn remove_item(name: &str, id: &str) {
    let _ = HUB.try_with(|hub| hub.store(name).remove(id));
}

/// A time as the API gives times: RFC 3339, UTC (`2026-11-03T12:00:00Z`).
pub fn time(unix_seconds: i64) -> Value {
    jiff::Timestamp::from_second(unix_seconds).map_or(Value::Null, |t| Value::String(t.to_string()))
}

/// A time the API gave (`time`), back as Unix seconds.
pub fn seconds(time: &Value) -> Option<i64> {
    time.as_str()?.parse::<jiff::Timestamp>().ok().map(|t| t.as_second())
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
        if !may_read(&self.who, resource) {
            // An agent learns nothing of what it may not read, not even that it exists.
            return Err(match self.who {
                Principal::Agent(_) => Error::NotFound(resource.to_string()),
                _ => Error::Forbidden(resource.to_string()),
            });
        }
        let Some(store) = self.hub.find(resource) else {
            return Err(Error::NotFound(resource.to_string()));
        };
        self.next.set(self.next.get() + 1);
        Ok(store.subscribe(format!("s{}", self.next.get()), Box::new(ready)))
    }

    /// Does command `method` (`id`: made by the client, unique to it). Sent again with an id
    /// already used, it is not done again: the first result comes back. A command answered
    /// later that isn't done at once fails here: use `command_later`.
    pub fn command(&self, id: &str, method: &str, params: Value) -> Result<Value, Error> {
        let got: Rc<RefCell<Option<Result<Value, Error>>>> = Rc::default();
        let into = got.clone();
        self.command_later(id, method, params, move |result| *into.borrow_mut() = Some(result));
        got.take().unwrap_or_else(|| Err(Error::Failed("still being done: its answer comes later".into())))
    }

    /// Does command `method`; `reply` gets the result, now or once it is done.
    pub fn command_later(&self, id: &str, method: &str, params: Value, reply: impl FnOnce(Result<Value, Error>) + 'static) {
        if id.is_empty() {
            return reply(Err(Error::Invalid("a command needs an id".into())));
        }
        let key = (format!("{:?}", self.who), id.to_string());
        let replayed = self.hub.0.done.borrow().iter().find(|(k, _)| *k == key).map(|(_, r)| r.clone());
        if let Some(result) = replayed {
            return reply(result);
        }
        if self.hub.0.pending.borrow().contains(&key) {
            return reply(Err(Error::Conflict("still being done (sent before with this id)".into())));
        }
        let handler = self.hub.0.handlers.borrow().get(method).cloned();
        match handler {
            None => reply(Err(Error::NotFound(format!("command {method}")))),
            Some(_) if !params.is_object() => reply(Err(Error::Invalid("params must be an object".into()))),
            // Not borrowed while it runs: a command may lead to others.
            Some(Handling::Now(handler)) => {
                let result = handler(&self.who, &params);
                if !reads_only(method, &params) {
                    self.hub.remember(key, result.clone());
                }
                reply(result);
            }
            Some(Handling::Later(handler)) => {
                self.hub.0.pending.borrow_mut().push(key.clone());
                let (hub, kept) = (self.hub.clone(), !reads_only(method, &params));
                handler(
                    &self.who,
                    &params,
                    Box::new(move |result| {
                        hub.0.pending.borrow_mut().retain(|k| *k != key);
                        if kept {
                            hub.remember(key, result.clone());
                        }
                        reply(result);
                    }),
                );
            }
        }
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
    /// A gap was seen: what comes until the asked-for resync is dropped (its numbers taken).
    stale: bool,
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
    /// Applies `m`. A gap is said once (`Gap`: ask for a resync, once no copy is borrowed);
    /// what follows it is dropped until the resync comes.
    pub fn apply(&mut self, m: &Message) -> Applied {
        if self.stale {
            self.seq = m.seq;
            if let Body::Resync { .. } = m.body {
                self.stale = false;
                self.incoming = Some(Vec::new());
            }
            return Applied::Nothing;
        }
        if self.seq != 0 && m.seq != self.seq + 1 {
            self.seq = m.seq;
            self.synced = false;
            self.stale = true;
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
