//! Bridges: programs of their own that reach loom over its bridge socket (see docs/bridges.md).
//! The host side runs in the app; here are the client and the Telegram bridge itself.

use serde_json::{Value, json};

use crate::agent::Asking;

pub mod browse;
pub mod client;
pub mod host;
pub mod link;
pub mod telegram;

pub mod api;
pub use host::{Host, ToPng, decode, pair, remote_program, safe_name, socket_path, state_of, watch};

#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Off,
    NeedsToken,
    Connecting,
    /// `code`: no account linked yet, sending it proposes one. `candidate`: who sent it, to be
    /// linked by the user in loom (never by the bot itself). `problem`: the last message that
    /// could not be sent.
    Connected { bot: String, user: i64, code: Option<String>, candidate: Option<(i64, String)>, problem: Option<String> },
    Failed(String),
}

impl Link {
    pub fn to_json(&self) -> Value {
        match self {
            Link::Off => json!({ "state": "off" }),
            Link::NeedsToken => json!({ "state": "needs-token" }),
            Link::Connecting => json!({ "state": "connecting" }),
            Link::Connected { bot, user, code, candidate, problem } => json!({
                "state": "connected", "bot": bot, "user": user, "code": code, "candidate": candidate, "problem": problem,
            }),
            Link::Failed(why) => json!({ "state": "failed", "why": why }),
        }
    }

    pub fn from_json(v: &Value) -> Link {
        match v["state"].as_str().unwrap_or("") {
            "off" => Link::Off,
            "needs-token" => Link::NeedsToken,
            "connecting" => Link::Connecting,
            "connected" => Link::Connected {
                bot: v["bot"].as_str().unwrap_or("").to_owned(),
                user: v["user"].as_i64().unwrap_or(0),
                code: v["code"].as_str().map(str::to_owned),
                candidate: v["candidate"].as_array().map(|c| (c[0].as_i64().unwrap_or(0), c[1].as_str().unwrap_or("").to_owned())),
                problem: v["problem"].as_str().map(str::to_owned),
            },
            _ => Link::Failed(v["why"].as_str().unwrap_or("The Telegram bridge failed.").to_owned()),
        }
    }
}

/// Characters nobody can guess (from /dev/urandom).
pub fn random(len: usize) -> String {
    const LETTERS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    random_bytes(len).iter().map(|b| LETTERS[*b as usize % LETTERS.len()] as char).collect()
}

/// `len` random bytes from /dev/urandom, as hex (a key: nothing weaker will do).
pub fn random_hex(len: usize) -> Option<String> {
    urandom(len).map(|bytes| bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn urandom(len: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut bytes = vec![0u8; len];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).ok().map(|()| bytes)
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    if let Some(random) = urandom(len) {
        bytes = random;
    } else {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()) ^ u128::from(std::process::id());
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (seed >> ((i % 16) * 8)) as u8;
        }
    }
    bytes
}

/// A question as a bridge got it.
pub fn asking_from(v: &Value) -> Asking {
    let strings = |v: &Value| v.as_array().into_iter().flatten().map(|s| s.as_str().unwrap_or("").to_owned()).collect::<Vec<_>>();
    Asking {
        title: v["title"].as_str().unwrap_or("").to_owned(),
        detail: v["detail"].as_str().map(str::to_owned),
        options: v["options"].as_array().into_iter().flatten().map(|o| (o[0].as_str().unwrap_or("").to_owned(), o[1].as_str().unwrap_or("").to_owned())).collect(),
        fields: v["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| (f[0].as_str().unwrap_or("").to_owned(), strings(&f[1]), f[2].as_bool().unwrap_or(false), f[3].as_array().into_iter().flatten().map(|b| b.as_bool().unwrap_or(false)).collect()))
            .collect(),
        is_form: v["is_form"].as_bool().unwrap_or(false),
        writable: v["writable"].as_bool().unwrap_or(false),
        summary: v["summary"].as_str().unwrap_or("").to_owned(),
        blank: v["blank"].as_bool().unwrap_or(false),
    }
}
