//! loom's core API over its socket, for a window that is loomd's client: the in-process
//! `Client`'s surface (follow a resource, do a command) on the core's thread, from a
//! connection that comes back by itself. loomd gone (restarted, upgraded): what was asked
//! meanwhile fails, it is reached again every second, and each resource followed starts over
//! from a fresh snapshot (a `resync` first, so a copy kept from it forgets the old one).
//!
//! The socket is read on a thread of its own; its lines come to the core's thread.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use serde_json::{Value, json};

use super::{Body, Error, Message};

type Reply = Box<dyn FnOnce(Result<Value, Error>)>;

/// A connection to loomd that comes back after loomd does.
#[derive(Clone)]
pub struct Remote(Rc<Inner>);

struct Inner {
    socket: PathBuf,
    name: String,
    writer: RefCell<Option<UnixStream>>,
    /// Bumped by each connection: a reader's lines from an older one are not this one's.
    round: Cell<u64>,
    next: Cell<u64>,
    pending: RefCell<HashMap<String, Reply>>,
    /// Followed resources, by loomd's id of their subscription (this connection's).
    by_sub: RefCell<HashMap<String, Weak<SubInner>>>,
    /// Every resource followed, to follow again on the next connection.
    followed: RefCell<Vec<Weak<SubInner>>>,
    /// What `initialize` answered (the principal loomd gave, its version…); Null: not connected.
    hello: RefCell<Value>,
    /// Told when it connects (true) or loses loomd (false).
    on_state: RefCell<Option<Rc<dyn Fn(bool)>>>,
    /// Why loomd last refused this client (`initialize` failed: another protocol…).
    refused: RefCell<Option<String>>,
}

/// A resource followed through a `Remote`: what came waits here, numbered for a `Mirror`.
pub struct RemoteSub(Rc<SubInner>);

struct SubInner {
    resource: String,
    remote: Weak<Inner>,
    /// loomd's id of it on this connection (None: not subscribed yet, or loomd gone).
    sub: RefCell<Option<String>>,
    queue: RefCell<VecDeque<Message>>,
    /// The numbers handed out: loomd's, after the ones given before this connection.
    base: Cell<u64>,
    last: Cell<u64>,
    ready: Box<dyn Fn()>,
}

impl Remote {
    /// Connects to the socket at `socket` (as `name`, for loomd's log), now and whenever the
    /// connection is lost.
    pub fn connect(socket: PathBuf, name: &str) -> Remote {
        let remote = Remote(Rc::new(Inner {
            socket,
            name: name.to_owned(),
            writer: RefCell::default(),
            round: Cell::new(0),
            next: Cell::new(0),
            pending: RefCell::default(),
            by_sub: RefCell::default(),
            followed: RefCell::default(),
            hello: RefCell::new(Value::Null),
            on_state: RefCell::default(),
            refused: RefCell::default(),
        }));
        Inner::open(&remote.0);
        remote
    }

    /// Whether it is connected (initialized) now.
    pub fn connected(&self) -> bool {
        !self.0.hello.borrow().is_null()
    }

    /// What loomd said when this connection began: `{protocol, server, principal, …}`.
    pub fn hello(&self) -> Value {
        self.0.hello.borrow().clone()
    }

    /// Why loomd refused this client last time it tried (None: it didn't).
    pub fn refused(&self) -> Option<String> {
        self.0.refused.borrow().clone()
    }

    /// `f` is told each time it connects (true) or loses loomd (false).
    pub fn on_state(&self, f: impl Fn(bool) + 'static) {
        *self.0.on_state.borrow_mut() = Some(Rc::new(f));
    }

    /// Follows `resource`; `ready` is called when something waits in it (`drain` then).
    pub fn subscribe(&self, resource: &str, ready: impl Fn() + 'static) -> RemoteSub {
        let sub = Rc::new(SubInner {
            resource: resource.to_owned(),
            remote: Rc::downgrade(&self.0),
            sub: RefCell::default(),
            queue: RefCell::default(),
            base: Cell::new(0),
            last: Cell::new(0),
            ready: Box::new(ready),
        });
        self.0.followed.borrow_mut().push(Rc::downgrade(&sub));
        if self.connected() {
            Inner::follow(&self.0, &sub);
        }
        RemoteSub(sub)
    }

    /// Does command `method` (`id`: the client's, unique to it); `reply` gets its result once
    /// loomd answers, or an error at once if loomd isn't there.
    pub fn command_later(&self, id: &str, method: &str, params: Value, reply: impl FnOnce(Result<Value, Error>) + 'static) {
        if !self.connected() {
            return reply(Err(Error::Failed("loom's core (loomd) isn't reachable now".into())));
        }
        self.0.call(json!(id), method, params, Box::new(reply));
    }
}

impl Inner {
    /// A new connection (in the background); trying again every second until there is one.
    fn open(me: &Rc<Inner>) {
        let round = me.round.get() + 1;
        me.round.set(round);
        let (socket, weak) = (me.socket.clone(), Rc::downgrade(me));
        crate::rt::spawn_local(async move {
            let found = crate::rt::spawn_blocking(move || UnixStream::connect(&socket)).await;
            let Some(me) = weak.upgrade() else { return };
            match found {
                Ok(Ok(stream)) => Inner::begin(&me, stream, round),
                _ => {
                    let weak = Rc::downgrade(&me);
                    crate::rt::timeout_once(Duration::from_secs(1), move || {
                        if let Some(me) = weak.upgrade() {
                            Inner::open(&me);
                        }
                    });
                }
            }
        });
    }

    /// Reads `stream` on a thread, says who it is, and follows again what was followed.
    fn begin(me: &Rc<Inner>, stream: UnixStream, round: u64) {
        let Ok(reading) = stream.try_clone() else { return Inner::lost(me, round) };
        *me.writer.borrow_mut() = Some(stream);
        let (tx, rx) = async_channel::unbounded::<Option<Value>>();
        std::thread::spawn(move || {
            for line in BufReader::new(reading).lines() {
                let Ok(line) = line else { break };
                if let Ok(value) = serde_json::from_str(&line)
                    && tx.send_blocking(Some(value)).is_err()
                {
                    return;
                }
            }
            let _ = tx.send_blocking(None);
        });
        let weak = Rc::downgrade(me);
        crate::rt::spawn_local(async move {
            while let Ok(Some(message)) = rx.recv().await {
                let Some(me) = weak.upgrade() else { return };
                if me.round.get() != round {
                    return;
                }
                me.receive(message);
            }
            if let Some(me) = weak.upgrade() {
                Inner::lost(&me, round);
            }
        });
        let params = json!({ "protocol": { "min": super::rpc::PROTOCOL, "max": super::rpc::PROTOCOL }, "client": { "name": me.name, "version": env!("CARGO_PKG_VERSION") }, "capabilities": {} });
        let weak = Rc::downgrade(me);
        me.call(
            json!("hello"),
            "initialize",
            params,
            Box::new(move |answer| {
                let Some(me) = weak.upgrade() else { return };
                let hello = match answer {
                    Ok(hello) => hello,
                    Err(err) => {
                        // loomd's answer (still connected), not the connection lost meanwhile.
                        if me.writer.borrow().is_some() {
                            *me.refused.borrow_mut() = Some(err.to_string());
                        }
                        return Inner::lost(&me, round);
                    }
                };
                me.refused.borrow_mut().take();
                *me.hello.borrow_mut() = hello;
                let followed: Vec<Rc<SubInner>> = me.followed.borrow().iter().filter_map(Weak::upgrade).collect();
                me.followed.borrow_mut().retain(|s| s.strong_count() > 0);
                for sub in followed {
                    Inner::follow(&me, &sub);
                }
                let told = me.on_state.borrow().clone();
                if let Some(told) = told {
                    told(true);
                }
            }),
        );
    }

    /// The connection is gone: what waited for an answer fails; another is tried in a second.
    fn lost(me: &Rc<Inner>, round: u64) {
        if me.round.get() != round {
            return;
        }
        let was = !me.hello.replace(Value::Null).is_null();
        me.writer.take();
        me.by_sub.borrow_mut().clear();
        for sub in me.followed.borrow().iter().filter_map(Weak::upgrade) {
            sub.sub.take();
        }
        let pending: Vec<Reply> = me.pending.borrow_mut().drain().map(|(_, r)| r).collect();
        for reply in pending {
            reply(Err(Error::Failed("loom's core (loomd) went away before answering".into())));
        }
        if was {
            let told = me.on_state.borrow().clone();
            if let Some(told) = told {
                told(false);
            }
        }
        let weak = Rc::downgrade(me);
        crate::rt::timeout_once(Duration::from_secs(1), move || {
            if let Some(me) = weak.upgrade() {
                Inner::open(&me);
            }
        });
    }

    fn call(&self, id: Value, method: &str, params: Value, reply: Reply) {
        let key = id.as_str().map_or_else(|| id.to_string(), str::to_owned);
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string();
        let sent = self.writer.borrow_mut().as_mut().is_some_and(|w| writeln!(w, "{line}").is_ok());
        if !sent {
            return reply(Err(Error::Failed("loom's core (loomd) isn't reachable now".into())));
        }
        self.pending.borrow_mut().insert(key, reply);
    }

    /// A protocol request of the client's own (subscribe, resync…), with an id of its own.
    fn request(&self, method: &str, params: Value, reply: Reply) {
        self.next.set(self.next.get() + 1);
        self.call(json!(format!("r{}", self.next.get())), method, params, reply);
    }

    /// Subscribes `sub` on this connection: a `resync` first, so a copy forgets the last one's.
    fn follow(me: &Rc<Inner>, sub: &Rc<SubInner>) {
        if sub.last.get() > 0 {
            sub.base.set(sub.last.get() + 1);
            sub.deliver(Body::Resync { reason: "reconnected".into() }, sub.last.get() + 1);
        }
        let (weak, held) = (Rc::downgrade(me), Rc::downgrade(sub));
        me.request(
            "subscribe",
            json!({ "resource": sub.resource }),
            Box::new(move |answer| {
                let (Some(me), Some(sub)) = (weak.upgrade(), held.upgrade()) else { return };
                match answer.ok().and_then(|a| a["sub"].as_str().map(str::to_owned)) {
                    Some(id) => {
                        me.by_sub.borrow_mut().insert(id.clone(), Rc::downgrade(&sub));
                        *sub.sub.borrow_mut() = Some(id);
                    }
                    // Not there (an agent closed meanwhile): gone, as it would be told.
                    None => sub.deliver(Body::Gone, sub.last.get() + 1),
                }
            }),
        );
    }

    fn receive(&self, message: Value) {
        if message["method"] == "subscription" {
            let Ok(m) = serde_json::from_value::<Message>(message["params"].clone()) else { return };
            let sub = self.by_sub.borrow().get(&m.sub).and_then(Weak::upgrade);
            if let Some(sub) = sub {
                let seq = sub.base.get() + m.seq;
                sub.deliver(m.body, seq);
            }
            return;
        }
        let key = message["id"].as_str().map_or_else(|| message["id"].to_string(), str::to_owned);
        let Some(reply) = self.pending.borrow_mut().remove(&key) else { return };
        reply(match message.get("error") {
            Some(error) => Err(error_of(error["code"].as_i64().unwrap_or(0), error["message"].as_str().unwrap_or("error"))),
            None => Ok(message["result"].clone()),
        });
    }
}

/// An error as loomd sent it, back as the core's (its words without the prefix they got).
fn error_of(code: i64, message: &str) -> Error {
    use super::rpc::{CONFLICT, FORBIDDEN, INVALID_PARAMS, NOT_FOUND};
    let bare = |prefix: &str| message.strip_prefix(prefix).unwrap_or(message).to_owned();
    match code {
        NOT_FOUND => Error::NotFound(bare("not found: ")),
        FORBIDDEN => Error::Forbidden(bare("not allowed: ")),
        INVALID_PARAMS => Error::Invalid(bare("invalid: ")),
        CONFLICT => Error::Conflict(message.to_owned()),
        _ => Error::Failed(message.to_owned()),
    }
}

impl SubInner {
    fn deliver(&self, body: Body, seq: u64) {
        self.last.set(seq);
        self.queue.borrow_mut().push_back(Message { sub: self.resource.clone(), seq, body });
        (self.ready)();
    }
}

impl RemoteSub {
    /// What came since, in order.
    pub fn drain(&self) -> Vec<Message> {
        self.0.queue.borrow_mut().drain(..).collect()
    }

    /// Whether something waits.
    pub fn waiting(&self) -> bool {
        !self.0.queue.borrow().is_empty()
    }

    /// Asks loomd for a fresh snapshot (a copy saw a gap).
    pub fn resync(&self) {
        let (Some(remote), Some(id)) = (self.0.remote.upgrade(), self.0.sub.borrow().clone()) else { return };
        remote.request("resync", json!({ "sub": id }), Box::new(|_| {}));
    }
}

impl Drop for RemoteSub {
    fn drop(&mut self) {
        if let (Some(remote), Some(id)) = (self.0.remote.upgrade(), self.0.sub.take()) {
            remote.by_sub.borrow_mut().remove(&id);
            remote.request("unsubscribe", json!({ "sub": id }), Box::new(|_| {}));
        }
    }
}
