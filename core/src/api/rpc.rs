//! The API as JSON-RPC 2.0 (warp, step 3): what a connection from outside the process says
//! and hears, one JSON object per line. The transport (`socket`) only moves lines; this is
//! the protocol, so tests drive it without a socket.
//!
//! - `initialize` first: the protocol range the client speaks (`{"protocol": {"min", "max"}}`)
//!   and its capabilities; the answer: the protocol chosen, who loom takes the client for
//!   (fixed by loom, never claimed), the commands there are and the limits.
//! - `subscribe {"resource"}` → `{"sub"}`; then the subscription's messages (pages, `synced`,
//!   `changes`, `resync`, `gone`) arrive as `subscription` notifications, in `seq` order.
//!   `unsubscribe {"sub"}`, `resync {"sub"}` (after a gap).
//! - Every other method is a command (`send`, `answer`, `stop_job`…). Its request id must be
//!   a string unique to the client: sent again with the same id (a retry after a lost
//!   connection), it is not done again and the first result comes back.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};

use serde_json::{Value, json};

use super::{Client, Error, Hub, Principal, Subscription};

/// The protocol versions loom speaks (the major; additive changes don't move it).
pub const PROTOCOL: u64 = 1;

/// The protocol's own methods (no command may take these names).
pub const RESERVED: [&str; 4] = ["initialize", "subscribe", "unsubscribe", "resync"];

// JSON-RPC 2.0's codes, then the API's own.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
/// Something before `initialize`.
pub const NOT_INITIALIZED: i64 = -32002;
/// No protocol version both speak.
pub const UNSUPPORTED_PROTOCOL: i64 = -32003;
pub const NOT_FOUND: i64 = -32010;
pub const FORBIDDEN: i64 = -32011;
pub const CONFLICT: i64 = -32012;

/// Where a session's lines go.
pub struct Out {
    /// Sends one line (without its newline).
    pub send: Box<dyn Fn(String)>,
    /// Whether the client keeps up: when not, subscriptions wait (and, past their limit, are
    /// resynced) instead of piling up in memory.
    pub room: Box<dyn Fn() -> bool>,
}

/// One connection's protocol state.
pub struct Session {
    hub: Hub,
    who: Principal,
    out: Out,
    client: RefCell<Option<Client>>,
    subs: RefCell<BTreeMap<String, Subscription>>,
    scheduled: Cell<bool>,
    me: Weak<Session>,
}

struct Failure {
    code: i64,
    message: String,
    data: Value,
}

impl Failure {
    fn new(code: i64, message: impl Into<String>) -> Failure {
        Failure { code, message: message.into(), data: Value::Null }
    }
}

impl From<Error> for Failure {
    fn from(error: Error) -> Failure {
        let (code, kind) = match &error {
            Error::NotFound(_) => (NOT_FOUND, "not_found"),
            Error::Forbidden(_) => (FORBIDDEN, "forbidden"),
            Error::Invalid(_) => (INVALID_PARAMS, "invalid"),
            Error::Conflict(_) => (CONFLICT, "conflict"),
        };
        Failure { code, message: error.to_string(), data: json!({ "kind": kind }) }
    }
}

/// A principal as the API shows it.
pub fn principal_json(who: &Principal) -> Value {
    match who {
        Principal::Window => json!({ "kind": "window" }),
        Principal::Cli => json!({ "kind": "cli" }),
        Principal::Agent(id) => json!({ "kind": "agent", "id": id }),
        Principal::Bridge(name) => json!({ "kind": "bridge", "name": name }),
    }
}

impl Session {
    /// A connection for `who` on `hub`. With an automatic hub, what subscriptions bring goes
    /// out once the core's thread is idle; with a manual one (tests), on `pump`.
    pub fn new(hub: Hub, who: Principal, out: Out) -> Rc<Session> {
        Rc::new_cyclic(|me| Session { hub, who, out, client: RefCell::default(), subs: RefCell::default(), scheduled: Cell::new(false), me: me.clone() })
    }

    pub fn principal(&self) -> &Principal {
        &self.who
    }

    /// One line from the client.
    pub fn handle(&self, line: &str) {
        let request: Value = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(err) => return self.respond(&Value::Null, Err(Failure::new(PARSE_ERROR, format!("not JSON: {err}")))),
        };
        let id = request.get("id").cloned();
        // A response to nothing loom asked, or no method: not a request.
        let Some(method) = request.get("method").and_then(Value::as_str).filter(|_| request["jsonrpc"] == "2.0") else {
            return self.respond(&id.unwrap_or(Value::Null), Err(Failure::new(INVALID_REQUEST, "not a JSON-RPC 2.0 request")));
        };
        let params = match request.get("params") {
            None | Some(Value::Null) => json!({}),
            Some(params) if params.is_object() => params.clone(),
            Some(_) => return self.respond(&id.unwrap_or(Value::Null), Err(Failure::new(INVALID_PARAMS, "params must be an object"))),
        };
        // A notification (no id): nothing is answered; loom takes none yet.
        let Some(id) = id else { return };
        let result = self.call(method, &id, &params);
        self.respond(&id, result);
        // A new subscription's snapshot follows its answer.
        if method == "subscribe" {
            self.pump();
        }
    }

    fn call(&self, method: &str, id: &Value, params: &Value) -> Result<Value, Failure> {
        if method == "initialize" {
            return self.initialize(params);
        }
        let client = self.client.borrow();
        let Some(client) = client.as_ref() else {
            return Err(Failure::new(NOT_INITIALIZED, "initialize first"));
        };
        let sub = || params["sub"].as_str().ok_or_else(|| Failure::new(INVALID_PARAMS, "sub is needed"));
        match method {
            "subscribe" => {
                let resource = params["resource"].as_str().filter(|r| !r.is_empty()).ok_or_else(|| Failure::new(INVALID_PARAMS, "resource is needed"))?;
                let me = self.me.clone();
                let subscription = client.subscribe(resource, move || {
                    if let Some(session) = me.upgrade() {
                        session.ready();
                    }
                })?;
                let sub = subscription.id().to_string();
                self.subs.borrow_mut().insert(sub.clone(), subscription);
                Ok(json!({ "sub": sub, "resource": resource }))
            }
            "unsubscribe" => {
                let sub = sub()?;
                self.subs.borrow_mut().remove(sub).ok_or_else(|| Failure::from(Error::NotFound(format!("subscription {sub}"))))?;
                Ok(json!({}))
            }
            "resync" => {
                let sub = sub()?;
                // (Asking calls `ready`, which only schedules a pump.)
                let subs = self.subs.borrow();
                subs.get(sub).ok_or_else(|| Failure::from(Error::NotFound(format!("subscription {sub}"))))?.resync();
                Ok(json!({}))
            }
            _ if !self.hub.has_command(method) => Err(Failure::new(METHOD_NOT_FOUND, format!("no method {method}"))),
            _ => {
                let id = id.as_str().filter(|i| !i.is_empty()).ok_or_else(|| {
                    Failure::new(INVALID_REQUEST, "a command's id must be a string unique to this client (a retry sends the same one)")
                })?;
                Ok(client.command(id, method, params.clone())?)
            }
        }
    }

    fn initialize(&self, params: &Value) -> Result<Value, Failure> {
        if self.client.borrow().is_some() {
            return Err(Failure::new(INVALID_REQUEST, "already initialized"));
        }
        let min = params["protocol"]["min"].as_u64().unwrap_or(PROTOCOL);
        let max = params["protocol"]["max"].as_u64().unwrap_or(min);
        if !(min..=max).contains(&PROTOCOL) {
            let mut failure = Failure::new(UNSUPPORTED_PROTOCOL, format!("loom speaks protocol {PROTOCOL}, the client {min} to {max}"));
            failure.data = json!({ "protocol": { "min": PROTOCOL, "max": PROTOCOL } });
            return Err(failure);
        }
        *self.client.borrow_mut() = Some(self.hub.connect(self.who.clone()));
        let limits = self.hub.limits();
        Ok(json!({
            "protocol": PROTOCOL,
            "server": { "name": "loom", "version": env!("CARGO_PKG_VERSION") },
            "principal": principal_json(&self.who),
            "capabilities": {
                "subscriptions": { "page": limits.page, "queue": limits.queue },
                "commands": self.hub.commands(),
            },
        }))
    }

    fn respond(&self, id: &Value, result: Result<Value, Failure>) {
        let message = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(f) => {
                let mut error = json!({ "code": f.code, "message": f.message });
                if !f.data.is_null() {
                    error["data"] = f.data;
                }
                json!({ "jsonrpc": "2.0", "id": id, "error": error })
            }
        };
        (self.out.send)(message.to_string());
    }

    /// A subscription has more: sent once the core's thread is idle (several together).
    fn ready(&self) {
        if !self.hub.auto() || self.scheduled.replace(true) {
            return;
        }
        let me = self.me.clone();
        crate::rt::idle_once(move || {
            if let Some(session) = me.upgrade() {
                session.scheduled.set(false);
                session.pump();
            }
        });
    }

    /// Sends what the subscriptions have, while the client keeps up (call it again when it
    /// has caught up).
    pub fn pump(&self) {
        let ended: Vec<String> = {
            let subs = self.subs.borrow();
            let mut ended = Vec::new();
            for (name, sub) in subs.iter() {
                if !(self.out.room)() {
                    break;
                }
                for message in sub.drain() {
                    if message.body == super::Body::Gone {
                        ended.push(name.clone());
                    }
                    let line = json!({ "jsonrpc": "2.0", "method": "subscription", "params": message });
                    (self.out.send)(line.to_string());
                }
            }
            ended
        };
        let mut subs = self.subs.borrow_mut();
        for name in ended {
            subs.remove(&name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Limits;

    fn session(hub: &Hub, who: Principal) -> (Rc<Session>, Rc<RefCell<Vec<Value>>>, Rc<Cell<bool>>) {
        let lines: Rc<RefCell<Vec<Value>>> = Rc::default();
        let room = Rc::new(Cell::new(true));
        let (sent, has_room) = (lines.clone(), room.clone());
        let out = Out { send: Box::new(move |line| sent.borrow_mut().push(serde_json::from_str(&line).unwrap())), room: Box::new(move || has_room.get()) };
        (Session::new(hub.clone(), who, out), lines, room)
    }

    fn take(lines: &Rc<RefCell<Vec<Value>>>) -> Vec<Value> {
        std::mem::take(&mut *lines.borrow_mut())
    }

    fn request(id: Value, method: &str, params: Value) -> String {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
    }

    #[test]
    fn nothing_before_initialize_and_protocols_must_meet() {
        let hub = Hub::manual(Limits::default());
        let (s, lines, _) = session(&hub, Principal::Cli);
        s.handle(&request(json!(1), "subscribe", json!({ "resource": "agents" })));
        assert_eq!(take(&lines)[0]["error"]["code"], NOT_INITIALIZED);
        s.handle(&request(json!(2), "initialize", json!({ "protocol": { "min": 2, "max": 3 } })));
        let out = take(&lines);
        assert_eq!(out[0]["error"]["code"], UNSUPPORTED_PROTOCOL);
        assert_eq!(out[0]["error"]["data"]["protocol"]["max"], PROTOCOL);
        hub.handle("send", |_, _| Ok(json!({ "sent": true })));
        s.handle(&request(json!(3), "initialize", json!({ "protocol": { "min": 1, "max": 5 }, "client": { "name": "test" } })));
        let out = take(&lines);
        assert_eq!(out[0]["id"], 3);
        assert_eq!(out[0]["result"]["protocol"], PROTOCOL);
        assert_eq!(out[0]["result"]["principal"]["kind"], "cli");
        assert_eq!(out[0]["result"]["capabilities"]["commands"], json!(["send"]));
        s.handle(&request(json!(4), "initialize", json!({})));
        assert_eq!(take(&lines)[0]["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn malformed_lines_get_json_rpc_errors() {
        let hub = Hub::manual(Limits::default());
        let (s, lines, _) = session(&hub, Principal::Cli);
        s.handle("{nope");
        s.handle(r#"{"id": 1, "method": "initialize"}"#);
        s.handle(r#"{"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": [1]}"#);
        // A notification is never answered.
        s.handle(r#"{"jsonrpc": "2.0", "method": "initialize"}"#);
        let out = take(&lines);
        let codes: Vec<&Value> = out.iter().map(|m| &m["error"]["code"]).collect();
        assert_eq!(codes, [&json!(PARSE_ERROR), &json!(INVALID_REQUEST), &json!(INVALID_PARAMS)]);
        assert_eq!(out[0]["id"], Value::Null);
        assert_eq!(out[1]["id"], 1);
    }

    #[test]
    fn a_subscription_answers_then_streams_its_messages() {
        let hub = Hub::manual(Limits::default());
        let store = hub.store("agents");
        store.upsert(json!({ "id": "a1", "title": "one" }));
        store.flush();
        let (s, lines, _) = session(&hub, Principal::Cli);
        s.handle(&request(json!(1), "initialize", json!({})));
        take(&lines);
        s.handle(&request(json!(2), "subscribe", json!({ "resource": "agents" })));
        let out = take(&lines);
        assert_eq!(out[0]["result"]["sub"], "s1");
        let kinds: Vec<&Value> = out[1..].iter().map(|m| &m["params"]["kind"]).collect();
        assert_eq!(kinds, [&json!("page"), &json!("synced")]);
        assert_eq!(out[1]["method"], "subscription");
        assert_eq!(out[1]["params"]["seq"], 1);
        assert_eq!(out[1]["params"]["items"][0]["title"], "one");
        store.append("a1", "title", " more");
        store.flush();
        s.pump();
        let out = take(&lines);
        assert_eq!(out[0]["params"]["seq"], 3);
        assert_eq!(out[0]["params"]["ops"][0]["op"], "append");
        // Asked again: a fresh snapshot.
        s.handle(&request(json!(3), "resync", json!({ "sub": "s1" })));
        s.pump();
        let out = take(&lines);
        assert_eq!(out[0]["result"], json!({}));
        let kinds: Vec<&Value> = out[1..].iter().map(|m| &m["params"]["kind"]).collect();
        assert_eq!(kinds, [&json!("resync"), &json!("page"), &json!("synced")]);
        assert_eq!(out[2]["params"]["items"][0]["title"], "one more");
        s.handle(&request(json!(4), "unsubscribe", json!({ "sub": "s1" })));
        s.handle(&request(json!(5), "unsubscribe", json!({ "sub": "s1" })));
        let out = take(&lines);
        assert_eq!(out[0]["result"], json!({}));
        assert_eq!(out[1]["error"]["code"], NOT_FOUND);
        store.upsert(json!({ "id": "a2" }));
        store.flush();
        s.pump();
        assert!(take(&lines).is_empty(), "nothing after unsubscribe");
    }

    #[test]
    fn a_client_that_lags_is_resynced_not_buffered() {
        let hub = Hub::manual(Limits { page: 256, queue: 3 });
        let store = hub.store("agents");
        let (s, lines, room) = session(&hub, Principal::Cli);
        s.handle(&request(json!(1), "initialize", json!({})));
        s.handle(&request(json!(2), "subscribe", json!({ "resource": "agents" })));
        take(&lines);
        room.set(false);
        for n in 0..5 {
            store.upsert(json!({ "id": format!("a{n}") }));
            store.flush();
            s.pump();
        }
        assert!(take(&lines).is_empty(), "nothing sent while the client is behind");
        room.set(true);
        s.pump();
        let out = take(&lines);
        assert_eq!(out[0]["params"]["kind"], "resync");
        assert_eq!(out[0]["params"]["reason"], "behind");
        // Replayed after the copy it had: all five, no gap.
        let mut mirror = crate::api::Mirror { seq: 2, synced: true, ..Default::default() };
        for line in &out {
            let message: crate::api::Message = serde_json::from_value(line["params"].clone()).unwrap();
            assert_ne!(mirror.apply(&message), crate::api::Applied::Gap);
        }
        assert!(mirror.synced);
        assert_eq!(mirror.items.len(), 5);
    }

    #[test]
    fn commands_need_string_ids_and_a_replay_is_not_done_twice() {
        let hub = Hub::manual(Limits::default());
        let count = Rc::new(Cell::new(0));
        let c = count.clone();
        hub.handle("stop", move |who, _| {
            if matches!(who, Principal::Agent(_)) {
                return Err(Error::Forbidden("stop".into()));
            }
            c.set(c.get() + 1);
            Ok(json!({ "stopped": c.get() }))
        });
        let (s, lines, _) = session(&hub, Principal::Cli);
        s.handle(&request(json!(1), "initialize", json!({})));
        take(&lines);
        s.handle(&request(json!(2), "stop", json!({})));
        s.handle(&request(json!("c-1"), "stop", json!({})));
        s.handle(&request(json!("c-1"), "stop", json!({})));
        s.handle(&request(json!("c-2"), "fly", json!({})));
        let out = take(&lines);
        assert_eq!(out[0]["error"]["code"], INVALID_REQUEST);
        assert_eq!(out[1]["result"]["stopped"], 1);
        assert_eq!(out[2]["result"]["stopped"], 1, "the first result again");
        assert_eq!(out[3]["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(count.get(), 1);
        let (agent, lines, _) = session(&hub, Principal::Agent("3".into()));
        agent.handle(&request(json!(1), "initialize", json!({})));
        assert_eq!(take(&lines)[0]["result"]["principal"], json!({ "kind": "agent", "id": "3" }));
        agent.handle(&request(json!("c-1"), "stop", json!({})));
        agent.handle(&request(json!(2), "subscribe", json!({ "resource": "conversation/4" })));
        let out = take(&lines);
        assert_eq!(out[0]["error"]["data"]["kind"], "forbidden");
        assert_eq!(out[1]["error"]["code"], NOT_FOUND);
    }

    #[test]
    fn a_closed_resource_says_gone_and_its_subscription_ends() {
        let hub = Hub::manual(Limits::default());
        hub.store("conversation/7");
        let (s, lines, _) = session(&hub, Principal::Cli);
        s.handle(&request(json!(1), "initialize", json!({})));
        s.handle(&request(json!(2), "subscribe", json!({ "resource": "conversation/7" })));
        take(&lines);
        hub.close("conversation/7");
        s.pump();
        assert_eq!(take(&lines)[0]["params"]["kind"], "gone");
        s.handle(&request(json!(3), "resync", json!({ "sub": "s1" })));
        assert_eq!(take(&lines)[0]["error"]["code"], NOT_FOUND);
    }
}
