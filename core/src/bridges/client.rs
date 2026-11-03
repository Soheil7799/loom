//! A bridge's side of its start: what loom said on stdin (its app id, this bridge's own
//! settings, where its API connection is) and that connection, which loom made and handed
//! down. From then on the bridge ends when stdin closes: loom has gone.

use std::io::Read;
use std::os::unix::net::UnixStream;

use serde_json::Value;

/// The fd loom handed this bridge its API connection on (-1: none, or taken).
static API_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// The line loom wrote on stdin: (its app id, this bridge's settings).
pub fn from_stdin() -> Result<(String, Value), String> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
    let hello: Value = serde_json::from_str(&line).map_err(|_| "a bridge is started by loom, not by hand".to_owned())?;
    if let Some(fd) = hello["api_fd"].as_i64().and_then(|fd| i32::try_from(fd).ok()) {
        API_FD.store(fd, std::sync::atomic::Ordering::Relaxed);
    }
    std::thread::spawn(|| {
        let _ = std::io::stdin().read_to_end(&mut Vec::new());
        std::process::exit(0);
    });
    Ok((hello["app"].as_str().unwrap_or("").to_owned(), hello["settings"].clone()))
}

/// This bridge's connection to the core API, initialized (once).
pub fn api(name: &str) -> Option<crate::api::cli::Connection> {
    crate::api::cli::Connection::over(handed()?, &format!("loom bridge {name}")).ok()
}

/// The connection itself, as loom handed it down (once; None after, or if none was).
pub fn handed() -> Option<UnixStream> {
    use std::os::fd::FromRawFd;
    let fd = API_FD.swap(-1, std::sync::atomic::Ordering::Relaxed);
    if fd < 0 {
        return None;
    }
    // SAFETY: loom left `fd` open for this process, and it is taken once.
    Some(unsafe { UnixStream::from_raw_fd(fd) })
}
