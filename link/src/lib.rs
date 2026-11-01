//! A device's side of loom's remote bridge: pair with loom on a desktop, then talk to its agents,
//! over iroh. `loom-remote`'s command-line peer and the phone apps are built on it, so what the
//! command line tests is what the phones run (docs/bridges.md, "The remote bridge").
//!
//! It keeps nothing itself: the device's key and the desktop's address are the caller's to
//! store (the command line in a file, a phone in its keystore).

pub mod wire;

use data_encoding::BASE64;
use iroh::endpoint::{Connection, RecvStream, presets};
use iroh::Endpoint;
pub use iroh::{EndpointAddr, SecretKey};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader, Lines};

use crate::wire::{ALPN, Ticket};

/// A new key for this device: made once, then kept.
pub fn new_key() -> [u8; 32] {
    SecretKey::generate().to_bytes()
}

/// This device's id, as loom's Settings lists it.
pub fn device_id(key: &[u8; 32]) -> String {
    SecretKey::from_bytes(key).public().to_string()
}

/// A step in a question form.
pub enum FormStep {
    /// Option `option` of field `field`.
    Pick { field: u64, option: u64 },
    /// The text of the field being written.
    Write(String),
    Send,
    Skip,
}

/// A connection to the desktop. One per device, kept while it is used: a second one with the
/// same key knocks this one off the relay. Requests and `subscribe` share it.
pub struct Desktop {
    _endpoint: Endpoint,
    connection: Connection,
    addr: EndpointAddr,
}

impl Desktop {
    /// Pairs with the desktop whose ticket (`loom:…`, from loom's Settings) this is, as `name`.
    /// Answers once the user has paired it in loom, or refused it: the desktop's address, to
    /// keep and `connect` to from then on.
    pub async fn pair(key: &[u8; 32], ticket: &str, name: &str) -> Result<EndpointAddr, String> {
        let ticket = Ticket::decode(ticket)?;
        let desktop = Self::open(key, ticket.addr.clone()).await?;
        desktop.request("pair", json!({ "code": ticket.code, "name": name })).await?;
        desktop.close();
        Ok(ticket.addr)
    }

    /// Connects to the desktop paired with, at `addr` (the one kept). Its relay or network may
    /// have changed since: `addr()` then says where it is now, to keep instead.
    pub async fn connect(key: &[u8; 32], addr: EndpointAddr) -> Result<Desktop, String> {
        let mut desktop = Self::open(key, addr).await?;
        if let Ok(now) = desktop.request("address", Value::Null).await.and_then(|a| serde_json::from_value::<EndpointAddr>(a).map_err(|e| e.to_string()))
            && now.id == desktop.addr.id
            // While it has no relay, the one known stays.
            && (now.relay_urls().next().is_some() || desktop.addr.relay_urls().next().is_none())
        {
            desktop.addr = now;
        }
        Ok(desktop)
    }

    async fn open(key: &[u8; 32], addr: EndpointAddr) -> Result<Desktop, String> {
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(SecretKey::from_bytes(key))
            .relay_mode(wire::relay_mode(addr.relay_urls().cloned()))
            .bind()
            .await
            .map_err(|e| e.to_string())?;
        let connection = endpoint.connect(addr.clone(), ALPN).await.map_err(|e| format!("The desktop is not reachable: {e}"))?;
        Ok(Desktop { _endpoint: endpoint, connection, addr })
    }

    /// Where the desktop is, as last heard.
    pub fn addr(&self) -> &EndpointAddr {
        &self.addr
    }

    /// Whether traffic goes straight to the desktop (else through the relay, still encrypted).
    pub fn direct(&self) -> bool {
        self.connection.paths().iter().any(|path| path.is_selected() && path.is_ip())
    }

    /// Why the connection ended; None while it is open.
    pub fn closed(&self) -> Option<String> {
        self.connection.close_reason().map(|why| why.to_string())
    }

    pub fn close(&self) {
        self.connection.close(0u32.into(), b"bye");
    }

    /// Any request of the protocol, as it is on the wire.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        wire::request(&self.connection, method, params).await
    }

    /// The agents, in loom's sidebar order, each with its open questions.
    pub async fn agents(&self) -> Result<Value, String> {
        self.request("agents", Value::Null).await
    }

    /// A message to agent `agent`, with pictures in any common format. The agent back.
    pub async fn send(&self, agent: u64, text: &str, images: &[Vec<u8>]) -> Result<Value, String> {
        let images: Vec<String> = images.iter().map(|image| BASE64.encode(image)).collect();
        self.request("send", json!({ "agent": agent, "text": text, "images": images })).await
    }

    /// A file for agent `agent`, saved in its folder. Where it was saved.
    pub async fn save(&self, agent: u64, name: &str, data: &[u8]) -> Result<String, String> {
        let saved = self.request("save", json!({ "agent": agent, "name": name, "data": BASE64.encode(data) })).await?;
        Ok(saved.as_str().unwrap_or("").to_owned())
    }

    /// Answers question `serial` of agent `agent` with its option `option` (from 0).
    pub async fn answer(&self, agent: u64, serial: u64, option: u64) -> Result<Value, String> {
        self.request("answer", json!({ "agent": agent, "serial": serial, "option": option })).await
    }

    /// A step in question form `serial` of agent `agent`.
    pub async fn form(&self, agent: u64, serial: u64, step: FormStep) -> Result<Value, String> {
        let mut params = json!({ "agent": agent, "serial": serial });
        match step {
            FormStep::Pick { field, option } => {
                params["do"] = json!("pick");
                params["field"] = json!(field);
                params["option"] = json!(option);
            }
            FormStep::Write(text) => {
                params["do"] = json!("write");
                params["text"] = json!(text);
            }
            FormStep::Send => params["do"] = json!("send"),
            FormStep::Skip => params["do"] = json!("skip"),
        }
        self.request("form", params).await
    }

    /// Stops what agent `agent` is doing.
    pub async fn stop(&self, agent: u64) -> Result<Value, String> {
        self.request("stop", json!({ "agent": agent })).await
    }

    /// Agent `agent`'s last reply (Markdown), if it has one.
    pub async fn last_reply(&self, agent: u64) -> Result<Option<String>, String> {
        Ok(self.request("last_reply", json!({ "agent": agent })).await?.as_str().map(str::to_owned))
    }

    /// A recording as text, with loom's speech-to-text settings.
    pub async fn transcribe(&self, audio: &[u8], name: &str, mime: &str) -> Result<String, String> {
        let text = self.request("transcribe", json!({ "audio": BASE64.encode(audio), "name": name, "mime": mime })).await?;
        Ok(text.as_str().unwrap_or("").to_owned())
    }

    /// loom's events from now on.
    pub async fn subscribe(&self) -> Result<Events, String> {
        let (mut send, recv) = self.connection.open_bi().await.map_err(|e| e.to_string())?;
        send.write_all(json!({ "method": "subscribe" }).to_string().as_bytes()).await.map_err(|e| e.to_string())?;
        send.finish().map_err(|e| e.to_string())?;
        Ok(Events { lines: BufReader::new(recv).lines() })
    }
}

/// loom's events as they come (docs/bridges.md, "Events").
pub struct Events {
    lines: Lines<BufReader<RecvStream>>,
}

impl Events {
    /// The next event. None: the desktop ended them (this device was removed, or loom quit).
    /// An error: refused (not paired), or the connection was lost.
    pub async fn next(&mut self) -> Result<Option<Value>, String> {
        let Some(line) = self.lines.next_line().await.map_err(|e| e.to_string())? else { return Ok(None) };
        let event: Value = serde_json::from_str(&line).map_err(|e| format!("bad event: {e}"))?;
        // Not an event: the desktop's answer, saying no.
        if event.get("event").is_none() {
            return Err(wire::result(&event).err().unwrap_or_else(|| "refused".to_owned()));
        }
        Ok(Some(event))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use iroh::RelayUrl;

    /// A stand-in desktop on this computer, with no relay. It answers `pair` (code "abc"),
    /// `agents`, `subscribe` with two events, and `address`: its own, with `relay` added, as if
    /// it had found one since (or, with `stranger`, another endpoint's).
    async fn stand_in(relay: Option<RelayUrl>, stranger: bool) -> (Endpoint, EndpointAddr) {
        let endpoint = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).relay_mode(iroh::RelayMode::Disabled).bind().await.unwrap();
        let port = endpoint.bound_sockets().iter().find(|s| s.is_ipv4()).unwrap().port();
        let addr = EndpointAddr::new(endpoint.id()).with_ip_addr(([127, 0, 0, 1], port).into());
        let mut told = addr.clone();
        if stranger {
            told.id = SecretKey::generate().public();
        }
        if let Some(relay) = relay {
            told = told.with_relay_url(relay);
        }
        let serving = endpoint.clone();
        tokio::spawn(async move {
            while let Some(incoming) = serving.accept().await {
                let told = told.clone();
                tokio::spawn(async move {
                    let Ok(connection) = incoming.await else { return };
                    while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                        let request: Value = serde_json::from_slice(&recv.read_to_end(1 << 20).await.unwrap()).unwrap();
                        let answer = match request["method"].as_str().unwrap() {
                            "subscribe" => "{\"event\":\"agent\",\"agent\":{\"id\":1}}\n{\"event\":\"done\",\"agent\":{\"id\":1},\"reply\":\"PONG\"}\n".to_owned(),
                            "address" => wire::answer(Ok(json!(told))).to_string(),
                            "agents" => wire::answer(Ok(json!([{ "id": 1, "title": "One" }]))).to_string(),
                            "pair" if request["params"]["code"] == "abc" => wire::answer(Ok(json!({ "paired": true }))).to_string(),
                            _ => wire::answer(Err("This device is not paired.".into())).to_string(),
                        };
                        let _ = send.write_all(answer.as_bytes()).await;
                        let _ = send.finish();
                    }
                });
            }
        });
        (endpoint, addr)
    }

    #[tokio::test]
    async fn pairs_then_talks_to_the_desktop() {
        let (endpoint, addr) = stand_in(None, false).await;
        let key = new_key();
        let ticket = Ticket { addr: addr.clone(), code: "abc".into() }.encode();
        assert_eq!(Desktop::pair(&key, &ticket, "Test").await.unwrap(), addr);
        let wrong = Ticket { addr: addr.clone(), code: "nope".into() }.encode();
        assert_eq!(Desktop::pair(&key, &wrong, "Test").await.unwrap_err(), "This device is not paired.");
        assert!(Desktop::pair(&key, "loom:nope", "Test").await.is_err());

        let desktop = Desktop::connect(&key, addr.clone()).await.unwrap();
        assert_eq!(desktop.agents().await.unwrap()[0]["title"], "One");
        let mut events = desktop.subscribe().await.unwrap();
        assert_eq!(events.next().await.unwrap().unwrap()["event"], "agent");
        assert_eq!(events.next().await.unwrap().unwrap()["reply"], "PONG");
        assert_eq!(events.next().await.unwrap(), None);
        assert!(desktop.closed().is_none());
        endpoint.close().await;
    }

    #[tokio::test]
    async fn keeps_where_the_desktop_moved() {
        let relay: RelayUrl = "http://localhost:3340".parse().unwrap();
        let key = new_key();
        // It found a relay since: kept.
        let (endpoint, addr) = stand_in(Some(relay.clone()), false).await;
        let desktop = Desktop::connect(&key, addr.clone()).await.unwrap();
        assert_eq!(desktop.addr().relay_urls().next(), Some(&relay));
        assert_eq!(desktop.addr().id, addr.id);
        endpoint.close().await;
        // Another endpoint's address: not taken.
        let (endpoint, addr) = stand_in(Some(relay), true).await;
        let desktop = Desktop::connect(&key, addr.clone()).await.unwrap();
        assert_eq!(desktop.addr(), &addr);
        endpoint.close().await;
    }
}
