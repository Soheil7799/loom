//! The core API for a web page (warp, step 6): the same JSON-RPC messages as the socket, one
//! per WebSocket frame, for a UI in a browser (or a webview: Tauri), on this computer only.
//! research/core-api/00-decisions.md §7.7 is what it follows:
//!
//! - **Off until asked for**: `web_open` (loom's window, or the CLI: `loom api web`) starts it
//!   on 127.0.0.1 (never another address) and answers a link with a one-time token; `web_close`
//!   stops it and forgets every page let in.
//! - **The token is traded once** (within two minutes) for a cookie, `HttpOnly; SameSite=Strict`,
//!   and taken out of the address (a redirect): the link can't be used again, and a script on
//!   the page can't read the cookie.
//! - **`Host` must be 127.0.0.1 or localhost at this port** (a site whose name points here, DNS
//!   rebinding, is turned away) and a WebSocket's **`Origin` exactly this page's** (another site
//!   can't open one with the cookie). No CORS headers; `frame-ancestors 'none'`.
//! - **A page is who opened its link**: the window's link, the window's rights; the CLI's, the
//!   CLI's (so whatever passes for the CLI gains nothing by opening one).
//!
//! The page itself: the files in `LOOM_WEB_UI`, else `<data dir>/loom/web` (a built UI: index.html
//! and what it loads), else a small one built in that lists the agents (to see it work).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use super::socket::External;
use super::{Error, Principal};

/// How long a link's token can be traded.
const TOKEN_LIFE: Duration = Duration::from_secs(120);
/// A request's head, at most.
const HEAD: usize = 16 << 10;
/// A message from the page, at most.
const MESSAGE: usize = 16 << 20;
const COOKIE: &str = "loom_session";

struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    keys: Arc<Keys>,
}

#[derive(Default)]
struct Keys {
    /// Tokens not traded yet: (token, who it lets in, until).
    tokens: Mutex<Vec<(String, Principal, Instant)>>,
    /// Cookies given: their value, and who the page is.
    sessions: Mutex<HashMap<String, Principal>>,
}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);

/// Registers `web_open` and `web_close`.
pub fn serve() {
    let hub = super::hub();
    hub.handle("web_open", |who, params| {
        if !matches!(who, Principal::Window | Principal::Cli) {
            return Err(Error::Forbidden("the web page, but for loom's window or the CLI".into()));
        }
        let port = params["port"].as_u64().and_then(|p| u16::try_from(p).ok()).unwrap_or(0);
        let url = open(who.clone(), port).map_err(Error::Failed)?;
        Ok(json!({ "url": url }))
    });
    hub.handle("web_close", |who, _| {
        if !matches!(who, Principal::Window | Principal::Cli) {
            return Err(Error::Forbidden("the web page, but for loom's window or the CLI".into()));
        }
        close();
        Ok(json!({}))
    });
}

/// Starts the server if it isn't (on `port`, 0: any free one), and a link letting `who` in.
pub fn open(who: Principal, port: u16) -> Result<String, String> {
    let mut server = SERVER.lock().unwrap();
    if server.is_none() {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).map_err(|e| format!("no web server on port {port}: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let (stop, keys) = (Arc::new(AtomicBool::new(false)), Arc::new(Keys::default()));
        let (stopped, kept) = (stop.clone(), keys.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(stream) = stream else { continue };
                let keys = kept.clone();
                std::thread::spawn(move || {
                    let _ = connection(stream, port, &keys);
                });
            }
        });
        *server = Some(Server { port, stop, keys });
    }
    let server = server.as_ref().expect("started above");
    let token = crate::bridges::random(40);
    let mut tokens = server.keys.tokens.lock().unwrap();
    tokens.retain(|(_, _, until)| *until > Instant::now());
    tokens.push((token.clone(), who, Instant::now() + TOKEN_LIFE));
    Ok(format!("http://127.0.0.1:{}/?t={token}", server.port))
}

/// Stops the server; every page let in is let go (its next request is turned away).
pub fn close() {
    if let Some(server) = SERVER.lock().unwrap().take() {
        server.stop.store(true, Ordering::Relaxed);
        server.keys.tokens.lock().unwrap().clear();
        server.keys.sessions.lock().unwrap().clear();
        // The listener wakes for the stop with one more connection.
        let _ = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], server.port)));
    }
}

/// The port it listens on, if it does.
pub fn port() -> Option<u16> {
    SERVER.lock().unwrap().as_ref().map(|s| s.port)
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
}

impl Request {
    fn header(&self, name: &str) -> &str {
        self.headers.get(name).map_or("", String::as_str)
    }

    fn cookie(&self, name: &str) -> Option<&str> {
        self.header("cookie").split(';').filter_map(|c| c.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
    }
}

fn read_head(reader: &mut BufReader<TcpStream>) -> Option<Request> {
    let mut head = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).ok()?;
        if n == 0 || head.len() + n > HEAD {
            return None;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        head.push(line.trim_end().to_owned());
    }
    let mut first = head.first()?.split_whitespace();
    let (method, path) = (first.next()?.to_owned(), first.next()?.to_owned());
    let headers = head[1..].iter().filter_map(|h| h.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    Some(Request { method, path, headers })
}

/// One HTTP connection: a page or file, the token traded, or the WebSocket.
fn connection(stream: TcpStream, port: u16, keys: &Keys) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let address = stream.peer_addr()?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let Some(request) = read_head(&mut reader) else { return Ok(()) };
    let mut stream = stream;
    // Only this computer's names for itself, at this port (no rebinding through another name).
    let host = request.header("host").to_owned();
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return answer(&mut stream, 421, "text/plain", b"Not this host.\n", &[]);
    }
    if request.method != "GET" {
        return answer(&mut stream, 405, "text/plain", b"Only GET.\n", &[]);
    }
    let (path, query) = request.path.split_once('?').unwrap_or((&request.path, ""));
    // The link: its token, traded once for a cookie, then out of the address.
    if let Some(token) = query.split('&').find_map(|p| p.strip_prefix("t=")) {
        let who = {
            let mut tokens = keys.tokens.lock().unwrap();
            let at = tokens.iter().position(|(t, _, until)| t == token && *until > Instant::now());
            at.map(|at| tokens.remove(at).1)
        };
        let Some(who) = who else {
            return answer(&mut stream, 403, "text/plain", b"This link was used already, or is too old: open loom's web page again (loom api web).\n", &[]);
        };
        let session = crate::bridges::random(40);
        keys.sessions.lock().unwrap().insert(session.clone(), who);
        let cookie = format!("Set-Cookie: {COOKIE}={session}; HttpOnly; SameSite=Strict; Path=/");
        return answer(&mut stream, 303, "text/plain", b"", &[&cookie, "Location: /"]);
    }
    let who = request.cookie(COOKIE).and_then(|c| keys.sessions.lock().unwrap().get(c).cloned());
    let Some(who) = who else {
        return answer(&mut stream, 403, "text/plain", b"Open loom's web page from loom (loom api web).\n", &[]);
    };
    if path == "/api" {
        // Only this page may open it (a cookie goes with any site's request; its Origin doesn't).
        if request.header("origin") != format!("http://{host}") {
            return answer(&mut stream, 403, "text/plain", b"Not from this page.\n", &[]);
        }
        return websocket(stream, reader, &request, who, address);
    }
    page(&mut stream, path)
}

/// A file of the UI (or the one built in).
fn page(stream: &mut TcpStream, path: &str) -> std::io::Result<()> {
    let path = if path == "/" { "/index.html" } else { path };
    if let Some(dir) = ui_dir() {
        // Within the folder only.
        let relative = Path::new(path.trim_start_matches('/'));
        if relative.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return answer(stream, 404, "text/plain", b"Not found.\n", &[]);
        }
        return match std::fs::read(dir.join(relative)) {
            Ok(bytes) => answer(stream, 200, mime(path), &bytes, &[]),
            Err(_) => answer(stream, 404, "text/plain", b"Not found.\n", &[]),
        };
    }
    match path {
        "/index.html" => answer(stream, 200, "text/html; charset=utf-8", BUILT_IN_PAGE.as_bytes(), &[]),
        "/loom.js" => answer(stream, 200, "text/javascript; charset=utf-8", BUILT_IN_SCRIPT.as_bytes(), &[]),
        "/loom.css" => answer(stream, 200, "text/css; charset=utf-8", BUILT_IN_STYLE.as_bytes(), &[]),
        _ => answer(stream, 404, "text/plain", b"Not found.\n", &[]),
    }
}

/// Where a built UI is, if there is one.
fn ui_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("LOOM_WEB_UI").map(PathBuf::from).unwrap_or_else(|| crate::paths::data_dir().join("loom/web"));
    dir.join("index.html").is_file().then_some(dir)
}

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "woff2" => "font/woff2",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn answer(stream: &mut TcpStream, status: u16, kind: &str, body: &[u8], extra: &[&str]) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        303 => "See Other",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        421 => "Misdirected Request",
        _ => "",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for line in SECURITY.iter().chain(extra) {
        head.push_str(line);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// On every answer: nothing from elsewhere, never in a frame, nothing told to other sites.
const SECURITY: &[&str] = &[
    "Content-Security-Policy: default-src 'self'; connect-src 'self' ws://127.0.0.1:* ws://localhost:*; img-src 'self' data: blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
    "X-Frame-Options: DENY",
    "X-Content-Type-Options: nosniff",
    "Referrer-Policy: no-referrer",
    "Cache-Control: no-store",
    "Cross-Origin-Opener-Policy: same-origin",
    "Cross-Origin-Resource-Policy: same-origin",
];

/// The WebSocket: the handshake, then each text frame a message both ways.
fn websocket(mut stream: TcpStream, reader: BufReader<TcpStream>, request: &Request, who: Principal, address: SocketAddr) -> std::io::Result<()> {
    use tungstenite::protocol::{Role, WebSocketConfig};
    let key = request.header("sec-websocket-key");
    if !request.header("upgrade").eq_ignore_ascii_case("websocket") || key.is_empty() {
        return answer(&mut stream, 400, "text/plain", b"A WebSocket only.\n", &[]);
    }
    let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
    let head = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    // What the reader already took past the head (a client may write at once).
    let early = reader.buffer().to_vec();
    let external = External::open(who, json!({ "transport": "web", "address": address.to_string() })).map_err(std::io::Error::other)?;
    let config = WebSocketConfig::default().max_message_size(Some(MESSAGE)).max_frame_size(Some(MESSAGE));
    let mut socket = tungstenite::WebSocket::from_partially_read(stream, early, Role::Server, Some(config));
    // One thread does both ways: it reads a moment, then sends what waits.
    socket.get_ref().set_read_timeout(Some(Duration::from_millis(25)))?;
    loop {
        while let Ok(line) = external.outgoing.try_recv() {
            if socket.write(tungstenite::Message::text(line)).is_err() {
                return Ok(());
            }
            external.sent();
        }
        if socket.flush().is_err() {
            return Ok(());
        }
        match socket.read() {
            Ok(tungstenite::Message::Text(text)) => {
                if !external.line(text.to_string()) {
                    return Ok(());
                }
            }
            Ok(tungstenite::Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return Ok(()),
        }
    }
}

const BUILT_IN_PAGE: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>loom</title><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="/loom.css"></head>
<body>
<h1>loom</h1>
<p id="state">Connecting…</p>
<ul id="agents"></ul>
<p class="dim">loom's core API, from a web page. Its own UI comes later; this one only shows the agents.</p>
<script src="/loom.js"></script>
</body>
</html>
"#;

const BUILT_IN_STYLE: &str = "body { font: 15px/1.5 system-ui, sans-serif; max-width: 42rem; margin: 2rem auto; padding: 0 1rem }\nh1 { font-size: 1.3rem }\n.dim { color: #777 }\n";

const BUILT_IN_SCRIPT: &str = r#"const socket = new WebSocket(`ws://${location.host}/api`);
let next = 0;
const call = (method, params) => socket.send(JSON.stringify({ jsonrpc: "2.0", id: String(++next), method, params }));
let agents = [];
const draw = () => {
  const list = document.getElementById("agents");
  list.replaceChildren(...agents.map(a => Object.assign(document.createElement("li"), { textContent: `${a.project} · ${a.title} — ${a.status}` })));
};
socket.onopen = () => call("initialize", { protocol: { min: 1, max: 1 }, client: { name: "loom web (built in)" } });
socket.onclose = (e) => { console.log("loom: closed", e.code); document.getElementById("state").textContent = "Disconnected."; };
socket.onerror = () => console.log("loom: the connection failed");
socket.onmessage = ({ data }) => {
  const m = JSON.parse(data);
  if (m.id === "1" && m.result) {
    document.getElementById("state").textContent = `Connected as ${m.result.principal.kind}.`;
    call("subscribe", { resource: "agents" });
  }
  if (m.method !== "subscription") return;
  const p = m.params;
  if (p.kind === "resync") agents = [];
  if (p.kind === "page") agents.push(...p.items);
  if (p.kind === "changes") for (const op of p.ops) {
    if (op.op === "upsert") { const i = agents.findIndex(a => a.id === op.item.id); if (i < 0) agents.push(op.item); else agents[i] = op.item; }
    if (op.op === "remove") agents = agents.filter(a => a.id !== op.id);
  }
  draw();
};
"#;
