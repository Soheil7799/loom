//! The core API for a web page (warp, step 6): the same JSON-RPC messages as the socket, one
//! per WebSocket frame, for a UI in a browser (or a webview: Tauri), on this computer only.
//! research/core-api/00-decisions.md §7.7 is what it follows:
//!
//! - **Off until asked for**: `web_open` (loom's window, or the CLI: `loom api web`) starts it
//!   on 127.0.0.1 (never another address) and answers a link with a one-time token; `web_close`
//!   stops it and forgets every page let in.
//! - **The token is traded once** (within two minutes) for a session, given in the address's
//!   fragment (`/#<session>`: a browser never sends a fragment to any server) and taken out of
//!   it by the page, which keeps it for its tab: the link can't be used again. Not a cookie: a
//!   browser sends 127.0.0.1's cookies to every port of it, so any server here that the user's
//!   browser visits would read one. The page's files are no secret; the API is: `/api` takes
//!   the session (`?s=`) with the page's `Origin`.
//! - **`Host` must be 127.0.0.1 or localhost at this port** (a site whose name points here, DNS
//!   rebinding, is turned away) and a WebSocket's **`Origin` exactly this page's** (another
//!   site's page can't open one). No CORS headers; `frame-ancestors 'none'`.
//! - **`web_close` lets every page go**, connected ones too.
//! - **A page is who opened its link**: the window's link, the window's rights; the CLI's, the
//!   CLI's (so whatever passes for the CLI gains nothing by opening one).
//!
//! The page itself: the files in `LOOM_WEB_UI` (a built UI: index.html and what it loads), else
//! the UI compiled into the binary (`build.rs`, from `web/dist`), else `<data dir>/loom/web`,
//! else a small page built in that lists the agents (to see it work).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
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

struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    keys: Arc<Keys>,
}

#[derive(Default)]
struct Keys {
    /// Tokens not traded yet: (token, who it lets in, until).
    tokens: Mutex<Vec<(String, Principal, Instant)>>,
    /// Sessions given: their value, and who the page is.
    sessions: Mutex<HashMap<String, Principal>>,
    /// Pages connected: their number, session and socket (shut by `web_close`).
    live: Mutex<Vec<(u64, String, TcpStream)>>,
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
        // Connected pages too: their sockets shut, their connections end.
        for (_, _, live) in server.keys.live.lock().unwrap().drain(..) {
            let _ = live.shutdown(std::net::Shutdown::Both);
        }
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

}

fn read_head(reader: &mut BufReader<TcpStream>) -> Option<Request> {
    use std::io::Read;
    let mut head = Vec::new();
    let mut total = 0;
    loop {
        // Never more than what is left of HEAD, however long a line someone sends.
        let mut line = Vec::new();
        let n = reader.by_ref().take((HEAD - total) as u64).read_until(b'\n', &mut line).ok()?;
        total += n;
        if n == 0 || line.last() != Some(&b'\n') {
            return None;
        }
        let line = String::from_utf8_lossy(&line);
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
    // The link: its token, traded once for a session, given where no server sees it.
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
        let location = format!("Location: /#{session}");
        return answer(&mut stream, 303, "text/plain", b"", &[&location]);
    }
    if path == "/api" {
        let session = query.split('&').find_map(|p| p.strip_prefix("s=")).unwrap_or("").to_owned();
        let who = keys.sessions.lock().unwrap().get(&session).cloned();
        let Some(who) = who else {
            return answer(&mut stream, 403, "text/plain", b"Open loom's web page from loom (loom api web).\n", &[]);
        };
        // Only this page may open it (another site's page can't send this Origin).
        if request.header("origin") != format!("http://{host}") {
            return answer(&mut stream, 403, "text/plain", b"Not from this page.\n", &[]);
        }
        return websocket(stream, reader, &request, who, address, (keys, &session));
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
    if !embedded::EMBEDDED.is_empty() {
        return match embedded::EMBEDDED.iter().find(|(p, _)| *p == path) {
            Some((_, bytes)) => answer(stream, 200, mime(path), bytes, &[]),
            None => answer(stream, 404, "text/plain", b"Not found.\n", &[]),
        };
    }
    match path {
        "/index.html" => answer(stream, 200, "text/html; charset=utf-8", BUILT_IN_PAGE.as_bytes(), &[]),
        "/loom.js" => answer(stream, 200, "text/javascript; charset=utf-8", BUILT_IN_SCRIPT.as_bytes(), &[]),
        "/loom.css" => answer(stream, 200, "text/css; charset=utf-8", BUILT_IN_STYLE.as_bytes(), &[]),
        _ => answer(stream, 404, "text/plain", b"Not found.\n", &[]),
    }
}

/// The web page built into this binary (`build.rs`): empty when it was built without one.
mod embedded {
    include!(concat!(env!("OUT_DIR"), "/web_ui.rs"));
}

/// A built UI's folder to serve instead of the one built in: `LOOM_WEB_UI` (working on the UI),
/// else, with none built in, `<data dir>/loom/web`.
fn ui_dir() -> Option<PathBuf> {
    let dir = match std::env::var_os("LOOM_WEB_UI") {
        Some(dir) => PathBuf::from(dir),
        None if embedded::EMBEDDED.is_empty() => crate::paths::data_dir().join("loom/web"),
        None => return None,
    };
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
/// Inline styles are allowed (xterm.js draws its terminal with `<style>` elements); scripts
/// stay the page's own.
const SECURITY: &[&str] = &[
    "Content-Security-Policy: default-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' ws://127.0.0.1:* ws://localhost:*; img-src 'self' data: blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
    "X-Frame-Options: DENY",
    "X-Content-Type-Options: nosniff",
    "Referrer-Policy: no-referrer",
    "Cache-Control: no-store",
    "Cross-Origin-Opener-Policy: same-origin",
    "Cross-Origin-Resource-Policy: same-origin",
];

/// The WebSocket: the handshake, then each text message a message of the API, both ways. A
/// reader (this thread) and a writer (one of its own) block on their side, nothing polls;
/// `web_close` ends the connection by shutting its socket (`live`).
fn websocket(mut stream: TcpStream, reader: BufReader<TcpStream>, request: &Request, who: Principal, address: SocketAddr, (keys, session): (&Keys, &str)) -> std::io::Result<()> {
    let key = request.header("sec-websocket-key");
    if !request.header("upgrade").eq_ignore_ascii_case("websocket") || key.is_empty() {
        return answer(&mut stream, 400, "text/plain", b"A WebSocket only.\n", &[]);
    }
    let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
    let head = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    stream.set_read_timeout(None)?;
    // What the reader took past the head (a client may write at once), then the socket.
    let early = reader.buffer().to_vec();
    let (external, outgoing) = External::open(who, json!({ "transport": "web", "address": address.to_string() })).map_err(std::io::Error::other)?;
    let writer = Arc::new(Mutex::new(stream.try_clone()?));
    static PAGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let page = PAGES.fetch_add(1, Ordering::Relaxed);
    keys.live.lock().unwrap().push((page, session.to_owned(), stream.try_clone()?));
    // Let go meanwhile (web_close between the check and now): not let in.
    if !keys.sessions.lock().unwrap().contains_key(session) {
        let _ = stream.shutdown(std::net::Shutdown::Both);
        return Ok(());
    }
    let (sending, sender) = (writer.clone(), external.clone());
    std::thread::spawn(move || {
        for line in outgoing {
            if write_frame(&mut *sending.lock().unwrap(), TEXT, line.as_bytes()).is_err() {
                break;
            }
            sender.sent();
        }
    });
    let mut input = std::io::Read::chain(std::io::Cursor::new(early), stream.try_clone()?);
    let mut message: Vec<u8> = Vec::new();
    while let Ok((fin, op, data)) = read_frame(&mut input) {
        match op {
            TEXT | CONTINUATION => {
                if message.len() + data.len() > MESSAGE {
                    break;
                }
                message.extend(data);
                if fin {
                    let Ok(text) = String::from_utf8(std::mem::take(&mut message)) else { break };
                    if !external.line(text) {
                        break;
                    }
                }
            }
            PING => {
                if write_frame(&mut *writer.lock().unwrap(), PONG, &data).is_err() {
                    break;
                }
            }
            CLOSE => {
                let _ = write_frame(&mut *writer.lock().unwrap(), CLOSE, &data[..data.len().min(2)]);
                break;
            }
            _ => {}
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
    keys.live.lock().unwrap().retain(|(n, _, _)| *n != page);
    external.close();
    Ok(())
}

const CONTINUATION: u8 = 0x0;
const TEXT: u8 = 0x1;
const CLOSE: u8 = 0x8;
const PING: u8 = 0x9;
const PONG: u8 = 0xA;

/// One frame from the page (RFC 6455: a client's are masked): (final, opcode, payload).
fn read_frame(input: &mut impl Read) -> std::io::Result<(bool, u8, Vec<u8>)> {
    let bad = |why: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, why.to_owned());
    let mut head = [0u8; 2];
    input.read_exact(&mut head)?;
    let (fin, op, masked) = (head[0] & 0x80 != 0, head[0] & 0x0f, head[1] & 0x80 != 0);
    let len = match head[1] & 0x7f {
        126 => {
            let mut b = [0u8; 2];
            input.read_exact(&mut b)?;
            u64::from(u16::from_be_bytes(b))
        }
        127 => {
            let mut b = [0u8; 8];
            input.read_exact(&mut b)?;
            u64::from_be_bytes(b)
        }
        n => u64::from(n),
    };
    if !masked {
        return Err(bad("a page's frame is masked"));
    }
    if len > MESSAGE as u64 {
        return Err(bad("too long"));
    }
    let mut key = [0u8; 4];
    input.read_exact(&mut key)?;
    let mut data = vec![0u8; len as usize];
    input.read_exact(&mut data)?;
    for (i, byte) in data.iter_mut().enumerate() {
        *byte ^= key[i % 4];
    }
    Ok((fin, op, data))
}

/// One whole frame to the page (a server's aren't masked).
fn write_frame(out: &mut impl Write, op: u8, data: &[u8]) -> std::io::Result<()> {
    let mut head = vec![0x80 | op];
    match data.len() {
        n if n < 126 => head.push(n as u8),
        n if n <= u16::MAX as usize => {
            head.push(126);
            head.extend((n as u16).to_be_bytes());
        }
        n => {
            head.push(127);
            head.extend((n as u64).to_be_bytes());
        }
    }
    out.write_all(&head)?;
    out.write_all(data)?;
    out.flush()
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

const BUILT_IN_SCRIPT: &str = r#"// The session: in the address's fragment once (never sent to a server), then this tab's.
if (location.hash.length > 1) { sessionStorage.setItem("loom", location.hash.slice(1)); history.replaceState(null, "", "/"); }
const socket = new WebSocket(`ws://${location.host}/api?s=${encodeURIComponent(sessionStorage.getItem("loom") || "")}`);
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
