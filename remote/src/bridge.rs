//! The desktop side (`loom bridge remote`): an iroh endpoint for the devices paired in loom.
//!
//! loom gives it its settings: the endpoint's secret key, the relay (empty: n0's public ones),
//! the paired devices, and the pairing code while Settings shows one. A device that is not paired
//! is dropped as soon as it connects, unless loom is pairing: then it may only ask to pair with
//! the code, and is paired when the user confirms it in loom. A paired device asks what the
//! bridge protocol allows (listed in `PASSED`) and hears loom's events.

use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use data_encoding::HEXLOWER;
use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointId, RelayMode, RelayUrl, SecretKey};
use serde_json::{Value, json};
use tokio::sync::{Notify, broadcast, watch};

use crate::loom::Loom;
use crate::wire::{self, ALPN, LIMIT, Ticket};

/// The bridge requests a paired device may make, passed to loom as they are.
const PASSED: &[&str] = &["agents", "send", "save", "answer", "form", "stop", "last_reply", "transcribe"];

/// How long loom waits for the user to confirm a device.
const CONFIRM: Duration = Duration::from_secs(300);

/// The most a device not paired may send in a request (a pairing request is small).
const STRANGER_LIMIT: usize = 4096;

#[derive(Default)]
struct Shared {
    devices: Vec<(EndpointId, String)>,
    /// The code Settings shows: pairing is open while there is one.
    pairing: Option<String>,
    /// The device that sent it, waiting for the user's yes in loom.
    candidate: Option<(EndpointId, String)>,
    connections: Vec<Connection>,
    /// The relay could not be reached (yet).
    problem: Option<String>,
}

struct Bridge {
    loom: Loom,
    endpoint: Endpoint,
    shared: Mutex<Shared>,
    /// Told whenever the settings change (a pairing waits on it).
    changed: Notify,
    /// loom's events, for every subscribed device.
    events: broadcast::Sender<String>,
    /// What Settings shows, sent to loom by one task (the latest wins).
    state: watch::Sender<Value>,
    debug: AtomicBool,
}

pub async fn run() -> i32 {
    let loom = match Loom::from_stdin() {
        Ok(loom) => loom,
        Err(err) => {
            eprintln!("loom-remote: {err}");
            return 2;
        }
    };
    let mut events = match loom.subscribe().await {
        Ok(events) => events,
        Err(err) => {
            eprintln!("loom-remote: {err}");
            return 1;
        }
    };
    let settings = match wire::result(&loom.call("hello", Value::Null).await) {
        Ok(hello) => hello["settings"].clone(),
        Err(err) => {
            eprintln!("loom-remote: {err}");
            return 1;
        }
    };
    if settings.is_null() {
        return 0;
    }
    let (mut settings, mut wait) = (settings, 2);
    let endpoint = loop {
        match bind(&settings).await {
            Ok(endpoint) => break endpoint,
            // Said in Settings, and tried again a while later (with the settings as they are then).
            Err(why) => {
                let _ = loom.call("state", json!({ "state": "failed", "why": why })).await;
                let again = tokio::time::sleep(Duration::from_secs(wait));
                tokio::pin!(again);
                loop {
                    tokio::select! {
                        () = &mut again => break,
                        line = events.next_line() => match line {
                            Ok(Some(line)) => {
                                let event: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                                if event["event"] == "settings" {
                                    settings = event["settings"].clone();
                                }
                            }
                            // loom is gone.
                            _ => return 0,
                        },
                    }
                }
                wait = (wait * 2).min(60);
            }
        }
    };
    let (state, reports) = watch::channel(Value::Null);
    let bridge = Arc::new(Bridge {
        loom: loom.clone(),
        endpoint: endpoint.clone(),
        shared: Mutex::default(),
        changed: Notify::new(),
        events: broadcast::channel(256).0,
        state,
        debug: AtomicBool::new(false),
    });
    tokio::spawn(report(loom, reports));
    bridge.settings(&settings);
    bridge.log(&format!("listening as {}", endpoint.id()));

    // Whether the relay answers: told in Settings until it does.
    let watching = bridge.clone();
    tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_secs(15), watching.endpoint.online()).await.is_err() {
            watching.shared.lock().unwrap().problem = Some("The relay can't be reached; only devices on this network can connect.".into());
            watching.report();
            watching.endpoint.online().await;
            watching.shared.lock().unwrap().problem = None;
        }
        watching.report();
    });

    let accepting = bridge.clone();
    tokio::spawn(async move {
        while let Some(incoming) = accepting.endpoint.accept().await {
            let bridge = accepting.clone();
            tokio::spawn(async move {
                if let Ok(connection) = incoming.await {
                    bridge.serve(connection).await;
                }
            });
        }
    });

    loop {
        match events.next_line().await {
            Ok(Some(line)) => {
                let event: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                if event["event"] == "settings" {
                    bridge.settings(&event["settings"]);
                } else {
                    let _ = bridge.events.send(line);
                }
            }
            // loom is gone.
            _ => return 0,
        }
    }
}

/// The endpoint the settings describe.
async fn bind(settings: &Value) -> Result<Endpoint, String> {
    let key = HEXLOWER
        .decode(settings["key"].as_str().unwrap_or("").as_bytes())
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or("The key in loom's settings is not a key.")?;
    let relay = settings["relay"].as_str().unwrap_or("").trim();
    let mode = if relay.is_empty() {
        RelayMode::Default
    } else {
        RelayMode::custom([relay.parse::<RelayUrl>().map_err(|e| format!("The relay {relay} is not an address: {e}"))?])
    };
    Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&key))
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(mode)
        .bind()
        .await
        .map_err(|e| format!("Could not open the connection: {e}"))
}

/// Sends Settings the latest state, one at a time.
async fn report(loom: Loom, mut reports: watch::Receiver<Value>) {
    while reports.changed().await.is_ok() {
        let state = reports.borrow_and_update().clone();
        let _ = loom.call("state", state).await;
    }
}

impl Bridge {
    fn log(&self, what: &str) {
        if self.debug.load(Ordering::Relaxed) {
            eprintln!("loom-remote: {what}");
        }
    }

    /// loom's settings for this bridge, now or changed.
    fn settings(&self, settings: &Value) {
        self.debug.store(settings["debug"].as_bool().unwrap_or(false), Ordering::Relaxed);
        {
            let mut shared = self.shared.lock().unwrap();
            let before = std::mem::take(&mut shared.devices);
            shared.devices = settings["devices"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| Some((d["id"].as_str()?.parse().ok()?, d["name"].as_str().unwrap_or("").to_owned())))
                .collect();
            shared.pairing = settings["pairing"].as_str().map(str::to_owned);
            let Shared { devices, pairing, candidate, connections, .. } = &mut *shared;
            // Refused: it is told so, then dropped (by `stream`).
            let refused = candidate.as_ref().map(|(id, _)| *id);
            if pairing.is_none() || candidate.as_ref().is_some_and(|(id, _)| devices.iter().any(|(d, _)| d == id)) {
                *candidate = None;
            }
            // A removed device goes now; one pairing stays while the code is shown.
            connections.retain(|c| {
                let id = c.remote_id();
                let removed = before.iter().any(|(d, _)| *d == id);
                let keep = devices.iter().any(|(d, _)| *d == id) || (pairing.is_some() && !removed) || refused == Some(id);
                if !keep {
                    c.close(1u32.into(), b"not paired");
                }
                keep
            });
        }
        self.changed.notify_waiters();
        self.report();
    }

    /// What Settings shows: this endpoint, the pairing, the devices connected.
    fn report(&self) {
        let shared = self.shared.lock().unwrap();
        let addr = self.endpoint.addr();
        let relay = addr.relay_urls().next().map(|r| r.to_string());
        let ticket = shared.pairing.as_ref().map(|code| Ticket { addr: addr.clone(), code: code.clone() }.encode());
        let online: BTreeSet<String> = shared.connections.iter().map(|c| c.remote_id().to_string()).collect();
        let _ = self.state.send(json!({
            "state": "online",
            "id": self.endpoint.id().to_string(),
            "relay": relay,
            "problem": shared.problem,
            "ticket": ticket,
            "candidate": shared.candidate.as_ref().map(|(id, name)| json!({ "id": id.to_string(), "name": name })),
            "online": online,
        }));
    }

    fn is_device(&self, id: &EndpointId) -> bool {
        self.shared.lock().unwrap().devices.iter().any(|(d, _)| d == id)
    }

    /// A device connected: its requests, until it goes.
    async fn serve(self: Arc<Self>, connection: Connection) {
        let id = connection.remote_id();
        {
            let mut shared = self.shared.lock().unwrap();
            if !shared.devices.iter().any(|(d, _)| *d == id) && shared.pairing.is_none() {
                drop(shared);
                self.log(&format!("{} is not paired: dropped", id.fmt_short()));
                connection.close(1u32.into(), b"not paired");
                return;
            }
            shared.connections.push(connection.clone());
        }
        self.log(&format!("{} connected", id.fmt_short()));
        self.report();
        while let Ok((send, recv)) = connection.accept_bi().await {
            tokio::spawn(self.clone().stream(connection.clone(), send, recv));
        }
        let gone = connection.stable_id();
        {
            let mut shared = self.shared.lock().unwrap();
            shared.connections.retain(|c| c.stable_id() != gone);
            // A device that asked to pair and left is not offered any more.
            if shared.candidate.as_ref().is_some_and(|(c, _)| *c == id) && !shared.connections.iter().any(|c| c.remote_id() == id) {
                shared.candidate = None;
            }
        }
        self.changed.notify_waiters();
        self.log(&format!("{} went", id.fmt_short()));
        self.report();
    }

    /// One request on `connection`, answered on `send`.
    async fn stream(self: Arc<Self>, connection: Connection, mut send: SendStream, mut recv: RecvStream) {
        let id = connection.remote_id();
        let limit = if self.is_device(&id) { LIMIT } else { STRANGER_LIMIT };
        let Ok(bytes) = recv.read_to_end(limit).await else { return };
        let request: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let method = request["method"].as_str().unwrap_or("");
        self.log(&format!("{} → {method}", id.fmt_short()));
        let answer = if method == "pair" {
            wire::answer(self.pair(id, &request["params"]).await)
        } else if !self.is_device(&id) {
            wire::answer(Err("This device is not paired.".into()))
        } else if method == "subscribe" {
            let mut events = self.events.subscribe();
            loop {
                match events.recv().await {
                    // Removed meanwhile: told nothing more.
                    Ok(_) if !self.is_device(&id) => return,
                    Ok(line) => {
                        if send.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        } else if method == "address" {
            // Where it is now, for the device to find it next time.
            wire::answer(Ok(json!(self.endpoint.addr())))
        } else if PASSED.contains(&method) {
            self.loom.call(method, request["params"].clone()).await
        } else {
            wire::answer(Err(format!("unknown request {method}")))
        };
        let _ = send.write_all(answer.to_string().as_bytes()).await;
        let _ = send.finish();
        // Refused or not paired: it goes once it has heard why.
        if !self.is_device(&id) && self.shared.lock().unwrap().pairing.is_none() {
            let _ = send.stopped().await;
            connection.close(1u32.into(), b"not paired");
        }
    }

    /// Device `id` asks to pair: it must send the code loom shows, then the user confirms it.
    async fn pair(&self, id: EndpointId, params: &Value) -> Result<Value, String> {
        let code = params["code"].as_str().unwrap_or("").to_owned();
        let name: String = params["name"].as_str().unwrap_or("").trim().chars().take(60).collect();
        let name = if name.is_empty() { "A device".to_owned() } else { name };
        let mut notified: Pin<Box<_>> = Box::pin(self.changed.notified());
        notified.as_mut().enable();
        {
            let mut shared = self.shared.lock().unwrap();
            if shared.devices.iter().any(|(d, _)| *d == id) {
                return Ok(json!({ "paired": true }));
            }
            if code.is_empty() || shared.pairing.as_deref() != Some(code.as_str()) {
                return Err("That is not the code loom shows now.".into());
            }
            match &shared.candidate {
                Some((other, _)) if *other != id => return Err("Another device is pairing; try again in a moment.".into()),
                _ => shared.candidate = Some((id, name.clone())),
            }
        }
        self.log(&format!("{} asks to pair as {name}", id.fmt_short()));
        self.report();
        let decided = tokio::time::timeout(CONFIRM, async {
            loop {
                notified.as_mut().await;
                // It left meanwhile: nothing to wait for.
                if self.shared.lock().unwrap().candidate.as_ref().is_none_or(|(c, _)| *c != id) && !self.is_device(&id) {
                    return Err("Not paired: refused or cancelled in loom.".to_owned());
                }
                notified = Box::pin(self.changed.notified());
                notified.as_mut().enable();
                let shared = self.shared.lock().unwrap();
                if shared.devices.iter().any(|(d, _)| *d == id) {
                    return Ok(json!({ "paired": true }));
                }
                if shared.pairing.as_deref() != Some(code.as_str()) {
                    return Err("Not paired: refused or cancelled in loom.".to_owned());
                }
            }
        })
        .await;
        decided.unwrap_or_else(|_| {
            let mut shared = self.shared.lock().unwrap();
            if shared.candidate.as_ref().is_some_and(|(c, _)| *c == id) {
                shared.candidate = None;
            }
            drop(shared);
            self.report();
            Err("Not confirmed in loom in time.".to_owned())
        })
    }
}
