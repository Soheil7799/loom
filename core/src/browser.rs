//! A browser for agents, one per project: a Chromium-based browser the user has (Settings →
//! Browser), with a profile of loom's own for the project (logins kept, never the user's own
//! profile), on a debugging port of the project's. It runs headed on a screen of its own
//! (Xvfb, when installed; headless otherwise), so it never opens a window on the desktop:
//! the window shows it in the project's tray and passes the user's clicks and keys to it
//! (`Screencast`). Agents drive the same browser with agent-browser (`--cdp <port>`), or
//! with Chrome's DevTools MCP when Settings says so.
//!
//! loomd starts them (`open`, for the `browser` tool and the window's `browser_open`) and
//! stops them (`browser_stop`, after `IDLE` unused, and with loomd: they die with it).

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Chromium-based browsers, by program name (what CDP, and so agent-browser, can drive).
const KNOWN: &[(&str, &str)] = &[
    ("helium-browser", "Helium"),
    ("chromium", "Chromium"),
    ("chromium-browser", "Chromium"),
    ("google-chrome-stable", "Google Chrome"),
    ("google-chrome", "Google Chrome"),
    ("brave-browser", "Brave"),
    ("brave", "Brave"),
    ("microsoft-edge-stable", "Microsoft Edge"),
    ("vivaldi-stable", "Vivaldi"),
    ("vivaldi", "Vivaldi"),
];

/// Unused this long (no agent asked, the window doesn't show it), a browser is stopped.
const IDLE: Duration = Duration::from_secs(30 * 60);

/// The screen a headed browser draws on, in pixels.
const SCREEN: (u32, u32) = (1366, 900);

/// The browsers found on this machine: (name, program), one per name.
pub fn found() -> Vec<(String, PathBuf)> {
    let mut all: Vec<(String, PathBuf)> = Vec::new();
    for (program, name) in KNOWN {
        if let Some(path) = crate::paths::find_program(program)
            && !all.iter().any(|(n, _)| n == name)
        {
            all.push(((*name).to_owned(), path));
        }
    }
    all
}

/// The browser agents get: the one Settings names, else the first found.
pub fn chosen() -> Option<PathBuf> {
    let set = crate::settings::get().browser;
    if !set.is_empty() {
        let path = PathBuf::from(&set);
        return if path.is_absolute() { path.is_file().then_some(path) } else { crate::paths::find_program(&set) };
    }
    found().into_iter().next().map(|(_, path)| path)
}

/// The project's debugging port: always the same, so the DevTools MCP can be told it before
/// the browser runs.
pub fn port_of(project: &Path) -> u16 {
    9300 + (crate::embed::hash(&project.to_string_lossy()) % 600) as u16
}

/// Does something answer on `port` on this machine?
fn listening(port: u16) -> bool {
    TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(200)).is_ok()
}

/// Waits (up to `within`) until the browser on `port` answers. For the MCP server's process,
/// not loomd's main loop.
pub fn wait_ready(port: u16, within: Duration) -> bool {
    let until = Instant::now() + within;
    while Instant::now() < until {
        if ureq::get(&format!("http://127.0.0.1:{port}/json/version")).call().is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

struct Running {
    port: u16,
    name: String,
    headed: bool,
    browser: Child,
    screen: Option<Child>,
    used: Instant,
    /// Times an agent asked the user to look (a login): the window brings it forward.
    shown: u64,
}

thread_local! {
    static RUNNING: RefCell<HashMap<PathBuf, Running>> = RefCell::default();
}

/// The project's browser, started if it doesn't run (`show`: the user is asked to look at
/// it). Its port, whether it is headed, and its name.
pub fn open(project: &Path, show: bool) -> Result<Value, String> {
    let alive = RUNNING.with(|r| r.borrow_mut().get_mut(project).is_some_and(|b| matches!(b.browser.try_wait(), Ok(None))));
    if !alive {
        stop(project);
        let started = start(project)?;
        RUNNING.with(|r| r.borrow_mut().insert(project.to_owned(), started));
    }
    let item = RUNNING.with(|r| {
        let mut running = r.borrow_mut();
        let b = running.get_mut(project).expect("started above");
        b.used = Instant::now();
        if show {
            b.shown += 1;
        }
        item(project, b)
    });
    crate::api::hub().store("browsers").upsert(item.clone());
    Ok(item)
}

/// Stops the project's browser (and its screen).
pub fn stop(project: &Path) {
    if let Some(b) = RUNNING.with(|r| r.borrow_mut().remove(project)) {
        std::thread::spawn(move || end(b.browser, Some(b.port), b.screen));
        crate::api::remove_item("browsers", &project.to_string_lossy());
    }
}

/// Stops every browser before loomd ends (waiting for them: they save their logins as they
/// close).
pub fn stop_all() {
    let all: Vec<Running> = RUNNING.with(|r| r.borrow_mut().drain().map(|(_, b)| b).collect());
    let ends: Vec<_> = all.into_iter().map(|b| std::thread::spawn(move || end(b.browser, Some(b.port), b.screen))).collect();
    for end in ends {
        let _ = end.join();
    }
}

/// Closes the browser as its window's close button would, by CDP's `Browser.close` on `port`:
/// SIGTERM cuts its last save of the cookies (the logins) short. SIGTERM if that fails or it
/// is still there in 10 s, killed 10 s later. Then its screen (`after`), only once it closed:
/// without one it would die before saving.
fn end(browser: Child, port: Option<u16>, after: Option<Child>) {
    for (mut child, port) in std::iter::once((browser, port)).chain(after.map(|s| (s, None))) {
        let pid = child.id();
        let ended = |child: &mut Child| {
            (0..100).any(|_| {
                std::thread::sleep(Duration::from_millis(100));
                matches!(child.try_wait(), Ok(Some(_)) | Err(_))
            })
        };
        if port.is_some_and(close) && ended(&mut child) {
            continue;
        }
        // SAFETY: a signal to a child of ours, by its pid.
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        if !ended(&mut child) {
            let _ = child.kill();
            let _ = child.wait();
            crate::log::write("browser", &format!("{pid} killed: it didn't stop in 10 s"));
        }
    }
}

/// Asks the browser on `port` to close (`Browser.close`); whether it was asked.
fn close(port: u16) -> bool {
    let asked = || -> Result<(), String> {
        let text = ureq::get(&format!("http://127.0.0.1:{port}/json/version")).call().map_err(|e| e.to_string())?.body_mut().read_to_string().map_err(|e| e.to_string())?;
        let version: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let url = version["webSocketDebuggerUrl"].as_str().ok_or("no browser socket")?;
        let (mut socket, _) = tungstenite::connect(url).map_err(|e| e.to_string())?;
        socket.send(tungstenite::Message::text(json!({ "id": 1, "method": "Browser.close" }).to_string())).map_err(|e| e.to_string())?;
        // Its answer: it is closing.
        socket.read().map(|_| ()).map_err(|e| e.to_string())
    };
    asked().map_err(|e| crate::log::write("browser", &format!("Browser.close on port {port}: {e}"))).is_ok()
}

fn item(project: &Path, b: &Running) -> Value {
    json!({ "id": project.to_string_lossy(), "port": b.port, "headed": b.headed, "name": b.name, "shown": b.shown, "devtools": crate::settings::get().browser_devtools })
}

/// The command line a browser is started with: its profile, its port, headed on `display`
/// or headless.
fn arguments(profile: &Path, port: u16, display: Option<u32>) -> Vec<String> {
    let mut args = vec![
        format!("--user-data-dir={}", profile.display()),
        format!("--remote-debugging-port={port}"),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        // no keyring asked for on the user's desktop
        "--password-store=basic".into(),
        format!("--window-size={},{}", SCREEN.0, SCREEN.1),
        "--window-position=0,0".into(),
    ];
    match display {
        Some(_) => args.push("--ozone-platform=x11".into()),
        None => args.push("--headless=new".into()),
    }
    args.push("about:blank".into());
    args
}

/// A display number nobody uses (no socket, no lock).
fn free_display() -> Option<u32> {
    (90..190).find(|n| !Path::new(&format!("/tmp/.X11-unix/X{n}")).exists() && !Path::new(&format!("/tmp/.X{n}-lock")).exists())
}

/// Dies with loomd (the signal it gets when loomd ends).
fn with_loomd(command: &mut Command) {
    // SAFETY: prctl only, between fork and exec.
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
}

fn start(project: &Path) -> Result<Running, String> {
    let program = chosen().ok_or("no Chromium-based browser was found: install Chromium (or Chrome, Brave, Helium), then pick it in loom's Settings → Browser")?;
    let name = found().into_iter().find(|(_, p)| *p == program).map(|(n, _)| n).unwrap_or_else(|| program.file_name().unwrap_or_default().to_string_lossy().into_owned());
    let port = port_of(project);
    if listening(port) {
        return Err(format!("port {port}, this project's browser port, is taken by another program"));
    }
    let profile = crate::state::dir().join("browser").join(format!("{:016x}", crate::embed::hash(&project.to_string_lossy())));
    std::fs::create_dir_all(&profile).map_err(|e| format!("can't make the browser's profile folder: {e}"))?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(crate::state::dir().join("browser.log")).map_err(|e| e.to_string())?;
    // Its own screen: headed (as sites expect a browser to be), and nowhere on the desktop.
    let mut screen = None;
    let display = match (crate::paths::find_program("Xvfb"), free_display()) {
        (Some(xvfb), Some(n)) => {
            let mut command = Command::new(xvfb);
            command.args([format!(":{n}"), "-screen".into(), "0".into(), format!("{}x{}x24", SCREEN.0, SCREEN.1), "-nolisten".into(), "tcp".into(), "-terminate".into()]);
            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(log.try_clone().map_err(|e| e.to_string())?);
            // Not ended with loomd (the browser is, and closes its windows first): it ends by
            // itself once the browser is gone (-terminate), its cookies saved.
            match command.spawn() {
                Ok(child) => {
                    screen = Some(child);
                    Some(n)
                }
                Err(err) => {
                    crate::log::write("browser", &format!("Xvfb didn't start ({err}): headless"));
                    None
                }
            }
        }
        _ => None,
    };
    // The browser waits for its screen (up to 5 s) before it starts.
    let mut command = Command::new("sh");
    command.arg("-c").arg(r#"s=$1; shift; i=0; while [ -n "$s" ] && [ ! -S "$s" ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i+1)); done; exec "$@""#).arg("sh");
    command.arg(display.map(|n| format!("/tmp/.X11-unix/X{n}")).unwrap_or_default());
    command.arg(&program).args(arguments(&profile, port, display));
    match display {
        Some(n) => {
            command.env("DISPLAY", format!(":{n}")).env_remove("WAYLAND_DISPLAY");
        }
        None => {
            command.env_remove("DISPLAY").env_remove("WAYLAND_DISPLAY");
        }
    }
    command.current_dir(project).stdin(Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
    with_loomd(&mut command);
    let browser = match command.spawn() {
        Ok(browser) => browser,
        Err(e) => {
            if let Some(screen) = screen {
                std::thread::spawn(move || end(screen, None, None));
            }
            return Err(format!("{} didn't start: {e}", program.display()));
        }
    };
    crate::log::write("browser", &format!("{name} for {} on port {port}{}", project.display(), if display.is_some() { ", headed" } else { ", headless" }));
    Ok(Running { port, name, headed: display.is_some(), browser, screen, used: Instant::now(), shown: 0 })
}

/// loomd's part: the `browsers` resource (one item per running browser), `browser_open` and
/// `browser_stop` for the window, and the idle check.
pub fn serve() {
    use crate::api::{Error, Principal};
    let hub = crate::api::hub();
    hub.store("browsers").set(Vec::new());
    let project = |params: &Value| params["project"].as_str().filter(|p| !p.is_empty()).map(PathBuf::from).ok_or_else(|| Error::Invalid("which project?".into()));
    hub.handle("browser_open", move |who, params| {
        if !matches!(who, Principal::Window | Principal::Cli) {
            return Err(Error::Forbidden("a project's browser, but from loom's window or the CLI".into()));
        }
        open(&project(params)?, false).map_err(Error::Failed)
    });
    hub.handle("browser_stop", move |who, params| {
        if !matches!(who, Principal::Window | Principal::Cli) {
            return Err(Error::Forbidden("a project's browser, but from loom's window or the CLI".into()));
        }
        stop(&project(params)?);
        Ok(json!({}))
    });
    // Ended by itself (crashed, closed), or unused for long: stopped, out of `browsers`.
    let _ = crate::rt::every(Duration::from_secs(60), || {
        let done: Vec<PathBuf> = RUNNING.with(|r| r.borrow_mut().iter_mut().filter_map(|(p, b)| (b.used.elapsed() > IDLE || !matches!(b.browser.try_wait(), Ok(None))).then(|| p.clone())).collect());
        for project in done {
            stop(&project);
        }
        true
    });
}

// ---- the window's view of it ------------------------------------------------------------

/// What a `Screencast` hears from the browser.
pub enum Shown {
    /// A frame (JPEG) and the page's size in CSS pixels it shows.
    Frame { jpeg: Vec<u8>, width: f64, height: f64 },
    /// The page now shown: its title and address.
    Page { title: String, url: String },
    /// The connection ended (the browser stopped): why.
    Ended(String),
}

/// A live view of the project's browser (the window's tray): its frames as they are drawn
/// (CDP's screencast), and the user's input passed to it. One thread per view.
pub struct Screencast {
    send: mpsc::Sender<Value>,
}

impl Screencast {
    /// Follows the page in front in the browser on `port`; `shown` gets what it hears (on the
    /// view's thread: hand it to the main loop).
    pub fn start(port: u16, shown: impl Fn(Shown) + Send + 'static) -> Screencast {
        let (send, outgoing) = mpsc::channel::<Value>();
        std::thread::spawn(move || {
            let why = follow(port, &outgoing, &shown).err().unwrap_or_else(|| "the view closed".into());
            shown(Shown::Ended(why));
        });
        Screencast { send }
    }

    /// A CDP command for the page (`Input.dispatchMouseEvent`…).
    pub fn send(&self, method: &str, params: Value) {
        let _ = self.send.send(json!({ "method": method, "params": params }));
    }
}

/// The page in front: the first page target the browser lists (the one used last).
fn front_page(port: u16) -> Result<(String, String, String, String), String> {
    let text = ureq::get(&format!("http://127.0.0.1:{port}/json/list")).call().map_err(|e| e.to_string())?.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let list: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let page = list.as_array().into_iter().flatten().find(|t| t["type"] == "page").ok_or("the browser has no page open")?;
    let s = |k: &str| page[k].as_str().unwrap_or_default().to_owned();
    Ok((s("id"), s("webSocketDebuggerUrl"), s("title"), s("url")))
}

/// Screencasts the page in front until the outgoing side closes; switches when another page
/// comes in front.
fn follow(port: u16, outgoing: &mpsc::Receiver<Value>, shown: &impl Fn(Shown)) -> Result<(), String> {
    let mut checked = Instant::now();
    loop {
        let (id, url, title, address) = front_page(port)?;
        shown(Shown::Page { title, url: address });
        let (mut socket, _) = tungstenite::connect(url.as_str()).map_err(|e| format!("can't reach the browser: {e}"))?;
        if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_ref() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(15)));
        }
        let mut next = 1u64;
        let mut call = |socket: &mut tungstenite::WebSocket<_>, method: &str, params: Value| {
            next += 1;
            socket.send(tungstenite::Message::text(json!({ "id": next, "method": method, "params": params }).to_string())).map_err(|e| e.to_string())
        };
        call(&mut socket, "Page.enable", json!({}))?;
        call(&mut socket, "Page.startScreencast", json!({ "format": "jpeg", "quality": 70, "maxWidth": SCREEN.0, "maxHeight": SCREEN.1, "everyNthFrame": 1 }))?;
        loop {
            // what the user did, to the page
            loop {
                match outgoing.try_recv() {
                    Ok(command) => call(&mut socket, command["method"].as_str().unwrap_or_default(), command["params"].clone())?,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                }
            }
            match socket.read() {
                Ok(tungstenite::Message::Text(text)) => {
                    let message: Value = serde_json::from_str(&text).unwrap_or_default();
                    match message["method"].as_str() {
                        Some("Page.screencastFrame") => {
                            let p = &message["params"];
                            let jpeg = crate::api::wire::bytes_of(p["data"].as_str().unwrap_or_default());
                            if !jpeg.is_empty() {
                                shown(Shown::Frame { jpeg, width: p["metadata"]["deviceWidth"].as_f64().unwrap_or(0.0), height: p["metadata"]["deviceHeight"].as_f64().unwrap_or(0.0) });
                            }
                            call(&mut socket, "Page.screencastFrameAck", json!({ "sessionId": p["sessionId"] }))?;
                        }
                        Some("Page.frameNavigated") if message["params"]["frame"]["parentId"].is_null() => {
                            let frame = &message["params"]["frame"];
                            shown(Shown::Page { title: String::new(), url: frame["url"].as_str().unwrap_or_default().to_owned() });
                        }
                        _ => {}
                    }
                }
                Ok(tungstenite::Message::Close(_)) => return Err("the browser closed the page".into()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) => return Err(format!("the browser went away: {e}")),
            }
            // Another page in front (an agent opened a tab): follow that one.
            // shortcut: polls the page list every 2 s; Target events would say it at once.
            if checked.elapsed() > Duration::from_secs(2) {
                checked = Instant::now();
                if front_page(port).map(|(front, ..)| front != id).unwrap_or(false) {
                    let _ = socket.close(None);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_has_one_port_and_a_browser_runs_headed_only_on_a_screen() {
        let project = Path::new("/work/shop");
        assert_eq!(port_of(project), port_of(project));
        assert!((9300..9900).contains(&port_of(project)));
        let headed = arguments(Path::new("/p"), 9333, Some(91));
        assert!(headed.contains(&"--remote-debugging-port=9333".to_owned()) && headed.contains(&"--ozone-platform=x11".to_owned()));
        assert!(!headed.iter().any(|a| a.starts_with("--headless")));
        assert!(arguments(Path::new("/p"), 9333, None).contains(&"--headless=new".to_owned()));
        assert!(headed.contains(&"--user-data-dir=/p".to_owned()), "a profile of loom's, never the user's own");
    }
}
