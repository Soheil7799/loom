//! What the desktop and a paired device say to each other over iroh.
//!
//! One connection per device (ALPN `loom/remote/0`), one bidirectional stream per request: the
//! device writes `{"method", "params"}` and finishes its side, the desktop answers
//! `{"ok": …}` or `{"error": "…"}` and finishes. `subscribe` is the one that stays open: the
//! desktop writes loom's events on it, one JSON object per line.
//!
//! Pairing: loom shows a ticket (as a QR code): the desktop's address and a one-time code. A
//! device not paired yet may only send `pair` with that code; it is paired once the user
//! confirms it in loom.

use data_encoding::BASE64URL_NOPAD;
use iroh::endpoint::Connection;
use iroh::{EndpointAddr, RelayMode, RelayUrl};
use serde_json::{Value, json};

pub const ALPN: &[u8] = b"loom/remote/0";

/// The most a request or an answer may be (pictures and files go in them).
pub const LIMIT: usize = 64 << 20;

const TICKET_PREFIX: &str = "loom:";

/// What a device needs to pair: where the desktop is, and the code it shows now.
pub struct Ticket {
    pub addr: EndpointAddr,
    pub code: String,
}

impl Ticket {
    pub fn encode(&self) -> String {
        let json = json!({ "addr": self.addr, "code": self.code });
        format!("{TICKET_PREFIX}{}", BASE64URL_NOPAD.encode(json.to_string().as_bytes()))
    }

    pub fn decode(text: &str) -> Result<Ticket, String> {
        let bad = || "That is not a loom pairing ticket.".to_owned();
        let body = text.trim().strip_prefix(TICKET_PREFIX).ok_or_else(bad)?;
        let bytes = BASE64URL_NOPAD.decode(body.as_bytes()).map_err(|_| bad())?;
        let json: Value = serde_json::from_slice(&bytes).map_err(|_| bad())?;
        let addr = serde_json::from_value(json["addr"].clone()).map_err(|_| bad())?;
        let code = json["code"].as_str().ok_or_else(bad)?.to_owned();
        Ok(Ticket { addr, code })
    }
}

/// The relays an endpoint uses: the one(s) given, or none.
pub fn relay_mode(relays: impl IntoIterator<Item = RelayUrl>) -> RelayMode {
    let relays: Vec<RelayUrl> = relays.into_iter().collect();
    if relays.is_empty() { RelayMode::Disabled } else { RelayMode::custom(relays) }
}

/// An answer as it goes on the wire.
pub fn answer(result: Result<Value, String>) -> Value {
    match result {
        Ok(value) => json!({ "ok": value }),
        Err(error) => json!({ "error": error }),
    }
}

/// An answer read back.
pub fn result(answer: &Value) -> Result<Value, String> {
    match answer.get("error") {
        Some(error) if !error.is_null() => Err(error.as_str().unwrap_or("failed").to_owned()),
        _ => Ok(answer["ok"].clone()),
    }
}

/// Asks the desktop and waits for its answer.
pub async fn request(conn: &Connection, method: &str, params: Value) -> Result<Value, String> {
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
    let line = json!({ "method": method, "params": params }).to_string();
    send.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
    send.finish().map_err(|e| e.to_string())?;
    let bytes = recv.read_to_end(LIMIT).await.map_err(|e| e.to_string())?;
    let answer: Value = serde_json::from_slice(&bytes).map_err(|e| format!("bad answer: {e}"))?;
    result(&answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_reads_back() {
        let id = iroh::SecretKey::generate().public();
        let relay: RelayUrl = "http://localhost:3340".parse().unwrap();
        // As a desktop gives it: its relay and the addresses it can be reached at directly.
        let addr = EndpointAddr::new(id).with_relay_url(relay.clone()).with_ip_addr("192.168.1.20:41641".parse().unwrap()).with_ip_addr("[fe80::1]:41641".parse().unwrap());
        let ticket = Ticket { addr, code: "abc".into() };
        let back = Ticket::decode(&ticket.encode()).unwrap();
        assert_eq!(back.addr, ticket.addr);
        assert_eq!(back.code, "abc");
        assert!(Ticket::decode("loom:nope").is_err());
        assert!(Ticket::decode("something else").is_err());
    }

    #[test]
    fn answers_read_back() {
        assert_eq!(result(&answer(Ok(json!(3)))), Ok(json!(3)));
        assert_eq!(result(&answer(Err("no".into()))), Err("no".to_owned()));
        assert_eq!(result(&json!({ "ok": null, "error": null })), Ok(Value::Null));
    }
}
