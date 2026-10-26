//! The debug log (Settings → General → Debug log): what loom does, in detail, to find out
//! what went wrong. Off unless switched on; then ~/.local/state/loom/debug.log (readable by
//! its owner only: it holds parts of conversations), cut to a new file past 10 MB. Crashes
//! are written there whether it is on or not.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use gtk::glib;

static ON: AtomicBool = AtomicBool::new(false);
static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// Biggest the file gets before it is moved to debug.log.1 (the one before is dropped).
const MOST: u64 = 10 * 1024 * 1024;
/// Longest one entry is kept (a message with an image in it is megabytes).
const LONGEST: usize = 4000;

pub fn path() -> PathBuf {
    crate::state::dir().join("debug.log")
}

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

pub fn set(on: bool) {
    if self::on() == on {
        return;
    }
    if on {
        ON.store(true, Ordering::SeqCst);
        write("loom", &format!("debug log on (loom {})", env!("CARGO_PKG_VERSION")));
    } else {
        // Written while still on: the log shows where it stops.
        write("loom", "debug log off");
        ON.store(false, Ordering::SeqCst);
        if let Ok(mut file) = FILE.lock() {
            file.take();
        }
    }
}

/// At most what one entry keeps of `text` (cut before formatting a megabyte for nothing).
pub fn cut(text: &str) -> &str {
    let mut end = text.len().min(LONGEST);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `area`: what it is about ("acp 1234", "agent 3", "telegram"…).
pub fn write(area: &str, text: &str) {
    if !on() && area != "crash" {
        return;
    }
    let text = if text.len() > LONGEST && area != "memory" {
        let mut cut = LONGEST;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}… [{} bytes in all]", &text[..cut], text.len())
    } else {
        text.to_owned()
    };
    let time = glib::DateTime::now_local().ok().and_then(|t| t.format("%F %T.%f").ok()).map(|t| t.to_string()).unwrap_or_default();
    let line = format!("{time} [{area}] {text}\n");
    let Ok(mut file) = FILE.lock() else { return };
    if file.as_ref().and_then(|f| f.metadata().ok()).is_some_and(|m| m.len() > MOST) {
        file.take();
        let _ = std::fs::rename(path(), path().with_extension("log.1"));
    }
    if file.is_none() {
        let _ = std::fs::create_dir_all(crate::state::dir());
        use std::os::unix::fs::OpenOptionsExt;
        *file = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path()).ok();
    }
    if let Some(file) = file.as_mut() {
        let _ = file.write_all(line.as_bytes());
    }
}

/// From here on: crashes and GTK's own warnings go into the log too.
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write("crash", &format!("{info}\n{}", std::backtrace::Backtrace::force_capture()));
        previous(info);
    }));
    glib::log_set_writer_func(|level, fields| {
        if on() {
            let message = fields.iter().find(|f| f.key() == "MESSAGE").and_then(|f| f.value_str()).unwrap_or("");
            let domain = fields.iter().find(|f| f.key() == "GLIB_DOMAIN").and_then(|f| f.value_str()).unwrap_or("glib");
            write(&format!("{domain} {level:?}"), message);
        }
        glib::log_writer_default(level, fields)
    });
}

/// `debug!("area", "format", args…)`: written only while the log is on (nothing is
/// formatted otherwise).
macro_rules! debug {
    ($area:expr, $($arg:tt)*) => {
        if $crate::log::on() {
            $crate::log::write(&$area, &format!($($arg)*));
        }
    };
}
pub(crate) use debug;
