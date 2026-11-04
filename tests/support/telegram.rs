//! A stand-in for Telegram's Bot API, on 127.0.0.1 (nothing goes out): updates the test
//! queues, and every call the bot makes, kept. Shared by the window's and loomd's tests.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Telegram's Bot API as far as the bot uses it, on 127.0.0.1: updates the test queues, and
/// every call the bot makes, kept.
#[derive(Default)]
pub struct FakeTelegram {
    updates: std::collections::VecDeque<Value>,
    calls: Vec<(String, Value)>,
    next: i64,
}

pub type Shared = std::sync::Arc<(std::sync::Mutex<FakeTelegram>, std::sync::Condvar)>;

pub fn fake_telegram() -> (String, Shared) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let shared: Shared = Shared::default();
    let state = shared.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let state = state.clone();
            std::thread::spawn(move || serve_telegram(stream, &state));
        }
    });
    (url, shared)
}

fn serve_telegram(stream: std::net::TcpStream, state: &Shared) {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    loop {
        let mut request = String::new();
        if reader.read_line(&mut request).unwrap_or(0) == 0 {
            return;
        }
        let mut length = 0;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            if header.trim().is_empty() {
                break;
            }
            if let Some(n) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                length = n.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let method = request.split_whitespace().nth(1).unwrap_or("").rsplit('/').next().unwrap_or("").to_owned();
        let (lock, wake) = &**state;
        let result = match method.as_str() {
            "getMe" => json!({ "id": 7, "is_bot": true, "first_name": "loom", "username": "loom_test_bot" }),
            // Asking for what is left over (the bot does at its start, to skip it): nothing.
            "getUpdates" if body["timeout"] == 0 => json!([]),
            "getUpdates" => {
                let mut fake = lock.lock().unwrap();
                fake.calls.push((method.clone(), body.clone()));
                wake.notify_all();
                let (mut fake, _) = wake.wait_timeout_while(fake, Duration::from_millis(500), |f| f.updates.is_empty()).unwrap();
                Value::Array(fake.updates.drain(..).collect())
            }
            _ => {
                let mut fake = lock.lock().unwrap();
                fake.next += 1;
                fake.calls.push((method.clone(), body.clone()));
                wake.notify_all();
                json!({ "message_id": 1000 + fake.next, "chat": { "id": 42 }, "date": 0, "text": body["text"] })
            }
        };
        let answer = json!({ "ok": true, "result": result }).to_string();
        let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n", answer.len());
        if writer.write_all(head.as_bytes()).and_then(|()| writer.write_all(answer.as_bytes())).is_err() {
            return;
        }
    }
}

/// The user (42) writes `text` to the bot.
pub fn user_says(state: &Shared, text: &str) {
    let (lock, wake) = &**state;
    let mut fake = lock.lock().unwrap();
    fake.next += 1;
    let id = fake.next;
    fake.updates.push_back(json!({ "update_id": id, "message": { "message_id": id, "from": { "id": 42, "is_bot": false, "first_name": "Me" }, "chat": { "id": 42, "type": "private" }, "date": 0, "text": text } }));
    wake.notify_all();
}

/// The user taps an inline button whose data is `data`.
pub fn user_taps(state: &Shared, data: &str) {
    let (lock, wake) = &**state;
    let mut fake = lock.lock().unwrap();
    fake.next += 1;
    let id = fake.next;
    fake.updates.push_back(json!({ "update_id": id, "callback_query": { "id": format!("cb{id}"), "from": { "id": 42, "is_bot": false, "first_name": "Me" }, "message": { "message_id": 1, "chat": { "id": 42, "type": "private" }, "date": 0 }, "data": data } }));
    wake.notify_all();
}

/// Waits for the bot's call `method` whose body satisfies `test`, from the `from`th call on
/// (`what` says what, if it never comes); its body, and where the next search starts.
pub fn bot_calls(state: &Shared, from: usize, method: &str, within: Duration, what: &str, test: impl Fn(&Value) -> bool) -> (Value, usize) {
    let (lock, wake) = &**state;
    let deadline = Instant::now() + within;
    let mut fake = lock.lock().unwrap();
    loop {
        if let Some(at) = fake.calls.iter().skip(from).position(|(m, b)| m == method && test(b)) {
            return (fake.calls[from + at].1.clone(), from + at + 1);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "the bot never did {what}; it called: {:#?}", fake.calls.iter().skip(from).map(|(m, b)| format!("{m} {b}")).collect::<Vec<_>>());
        fake = wake.wait_timeout(fake, left).unwrap().0;
    }
}

