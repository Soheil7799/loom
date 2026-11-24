//! Diagrams in the chat: SVG drawn by GTK itself (librsvg, through gdk-pixbuf), Mermaid turned
//! into SVG first by a headless run of the Chromium loom finds (Mermaid's script ships inside
//! loom: data/mermaid, MIT, version in data/mermaid/VERSION). Nothing goes on the network: the
//! page is a local file, the browser resolves no host and goes through a proxy that isn't
//! there (an address typed in would reach nothing either).

use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

const MERMAID_JS: &[u8] = include_bytes!("../data/mermaid/mermaid.min.js");

/// A Mermaid render that takes longer than this is given up.
const LIMIT: Duration = Duration::from_secs(20);

/// A diagram is drawn at most this tall (an SVG can say any size it likes).
const TALLEST: i32 = 1600;

/// One Mermaid render at a time: a reply with many diagrams starts one browser, not many.
static RENDERING: Mutex<()> = Mutex::new(());

/// The languages drawn as a picture, not shown as code.
pub fn is_diagram(lang: &str) -> bool {
    matches!(lang.to_ascii_lowercase().as_str(), "svg" | "mermaid")
}

/// A drawn diagram's pixels (they cross threads; a texture is made of them on the main one).
pub struct Pixels {
    bytes: glib::Bytes,
    width: i32,
    height: i32,
    stride: usize,
    alpha: bool,
}

impl Pixels {
    /// As a texture, to show (on the main thread).
    pub fn texture(&self) -> gtk::gdk::Texture {
        let format = if self.alpha { gtk::gdk::MemoryFormat::R8g8b8a8 } else { gtk::gdk::MemoryFormat::R8g8b8 };
        gtk::gdk::MemoryTexture::new(self.width, self.height, format, &self.bytes, self.stride).upcast()
    }
}

/// A diagram's picture: SVG as it is, Mermaid turned into SVG first, at most `widest` wide and
/// `TALLEST` tall. Blocking (a browser may run): off the main thread.
pub fn picture(mermaid_code: bool, code: &str, widest: i32, dark: bool) -> Result<Pixels, String> {
    let svg = if mermaid_code { mermaid(code, dark)? } else { code.to_owned() };
    let pixbuf = decode(svg.as_bytes(), widest, TALLEST)?;
    Ok(Pixels { bytes: pixbuf.read_pixel_bytes(), width: pixbuf.width(), height: pixbuf.height(), stride: pixbuf.rowstride() as usize, alpha: pixbuf.has_alpha() })
}

/// `svg` drawn within `widest` x `tallest`, its shape kept: the size is set before anything
/// is drawn, so an SVG saying 30000 x 30000 costs no more than a small one.
fn decode(svg: &[u8], widest: i32, tallest: i32) -> Result<gtk::gdk_pixbuf::Pixbuf, String> {
    let loader = gtk::gdk_pixbuf::PixbufLoader::new();
    loader.connect_size_prepared(move |loader, width, height| {
        let scale = (f64::from(widest) / f64::from(width.max(1))).min(f64::from(tallest) / f64::from(height.max(1))).min(1.0);
        loader.set_size(((f64::from(width) * scale) as i32).max(1), ((f64::from(height) * scale) as i32).max(1));
    });
    let read = |e: glib::Error| format!("not a picture GTK can read ({e})");
    loader.write(svg).map_err(read)?;
    loader.close().map_err(read)?;
    loader.pixbuf().ok_or_else(|| "not a picture GTK can read".to_owned())
}

/// Mermaid's script's own hash (its file name, and part of every drawing's key: a new Mermaid
/// draws everything anew).
fn script_hash() -> u64 {
    static HASH: OnceLock<u64> = OnceLock::new();
    *HASH.get_or_init(|| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        MERMAID_JS.hash(&mut hasher);
        hasher.finish()
    })
}

/// Mermaid source as SVG (from the cache when drawn before). Blocking: run it off the main
/// thread.
pub fn mermaid(code: &str, dark: bool) -> Result<String, String> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (code, dark, script_hash()).hash(&mut hasher);
    let dir = glib::user_cache_dir().join("loom").join("diagrams");
    let cached = dir.join(format!("{:016x}.svg", hasher.finish()));
    if let Ok(svg) = std::fs::read_to_string(&cached) {
        return Ok(svg);
    }
    let browser = loom_core::browser::chosen().ok_or("no Chromium-based browser found to draw Mermaid with")?;
    let _one = RENDERING.lock().unwrap_or_else(|e| e.into_inner());
    // (drawn by another meanwhile, while this one waited its turn)
    if let Ok(svg) = std::fs::read_to_string(&cached) {
        return Ok(svg);
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let script = dir.join(format!("mermaid-{:016x}.min.js", script_hash()));
    // Folders of runs that never finished (loom stopped mid-render: only one runs at a time),
    // and the scripts of Mermaids loom no longer ships.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("run-") {
                let _ = std::fs::remove_dir_all(entry.path());
            } else if name.starts_with("mermaid") && name.ends_with(".js") && entry.path() != script {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    if !script.is_file() {
        write_whole(&script, MERMAID_JS)?;
    }
    let work = tempdir(&dir)?;
    let page = work.join("page.html");
    std::fs::write(&page, page_html(code, dark, &script)).map_err(|e| e.to_string())?;
    let out = run(&browser, &work, &page);
    let _ = std::fs::remove_dir_all(&work);
    let svg = svg_from_dom(&out?)?;
    let _ = write_whole(&cached, svg.as_bytes());
    Ok(svg)
}

/// `bytes` into `path` whole or not at all (written beside it, then moved over it): a crash
/// mid-write leaves no half file to be read as a whole one.
fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let part = path.with_extension(format!("part-{}-{nanos}", std::process::id()));
    std::fs::write(&part, bytes).and_then(|()| std::fs::rename(&part, path)).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        e.to_string()
    })
}

/// The page that draws `code`: Mermaid's own labels as SVG text (GTK can't draw the HTML ones),
/// the result written into the body as text for `--dump-dom` to print.
fn page_html(code: &str, dark: bool, script: &Path) -> String {
    // A JSON string, with no "</script>" able to close the element early.
    let code = serde_json::to_string(code).unwrap_or_default().replace('<', "\\u003c");
    let theme = if dark { "dark" } else { "default" };
    let script = script.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    format!(
        r#"<!doctype html><meta charset="utf-8"><body><script src="../{script}"></script><script>
mermaid.initialize({{ startOnLoad: false, securityLevel: "strict", theme: "{theme}", htmlLabels: false, flowchart: {{ htmlLabels: false }}, themeVariables: {{ fontFamily: "sans-serif" }} }});
mermaid.render("d", {code}).then(({{ svg }}) => {{
  document.body.innerHTML = svg;
  const el = document.querySelector("svg");
  const box = el.viewBox.baseVal;
  if (box && box.width) {{ el.setAttribute("width", Math.ceil(box.width)); el.setAttribute("height", Math.ceil(box.height)); }}
  el.removeAttribute("style");
  document.body.textContent = new XMLSerializer().serializeToString(el);
}}, (e) => {{ document.body.textContent = "ERROR " + (e && e.message ? e.message : e); }});
</script>"#
    )
}

/// Runs the browser headless on `page` and returns the DOM it printed.
fn run(browser: &std::path::Path, work: &std::path::Path, page: &std::path::Path) -> Result<String, String> {
    let mut child = Command::new(browser)
        .args(["--headless", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--mute-audio"])
        // No host resolves: the page can reach nothing off this machine.
        .arg("--host-resolver-rules=MAP * ~NOTFOUND")
        // (and an address typed in, which no resolver sees, goes to a proxy that isn't there)
        .arg("--proxy-server=127.0.0.1:9")
        .arg("--proxy-bypass-list=<-loopback>")
        .arg(format!("--user-data-dir={}", work.join("profile").display()))
        .arg("--virtual-time-budget=10000")
        .arg("--dump-dom")
        .arg(format!("file://{}", page.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't start the browser: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > LIMIT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Mermaid took too long to draw".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
    reader.join().map_err(|_| "no output".to_owned())
}

/// The SVG in a dumped page (the body's text), or the error Mermaid gave.
fn svg_from_dom(dom: &str) -> Result<String, String> {
    let body = dom.split_once("<body>").map_or("", |(_, b)| b);
    let body = body.rsplit_once("</body>").map_or(body, |(b, _)| b);
    // The body's text, as the page serialized it: back from HTML's escaping.
    let text = crate::rich::unescape(&body.replace("&nbsp;", "\u{a0}"));
    if let Some(error) = text.trim().strip_prefix("ERROR ") {
        return Err(format!("Mermaid: {}", error.lines().next().unwrap_or(error)));
    }
    let start = text.find("<svg").ok_or("Mermaid drew nothing")?;
    let end = text.rfind("</svg>").ok_or("Mermaid drew nothing")? + "</svg>".len();
    Ok(text[start..end].to_owned())
}

/// A new empty folder in `dir`, for one render.
fn tempdir(dir: &std::path::Path) -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let work = dir.join(format!("run-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&work).map_err(|e| e.to_string())?;
    Ok(work)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_svg_comes_back_out_of_the_dumped_page() {
        let dom = "<html><head></head><body>&lt;svg width=\"10\"&gt;&lt;text&gt;a &amp;amp; b&nbsp;c&lt;/text&gt;&lt;/svg&gt;</body></html>";
        assert_eq!(super::svg_from_dom(dom).unwrap(), "<svg width=\"10\"><text>a &amp; b\u{a0}c</text></svg>");
        assert_eq!(super::svg_from_dom("<body>ERROR Parse error on line 2</body>").unwrap_err(), "Mermaid: Parse error on line 2");
    }

    #[test]
    fn an_svg_of_any_size_is_drawn_within_the_bounds() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="30000" height="30000"><rect width="30000" height="30000" fill="red"/></svg>"#;
        let picture = super::decode(svg, 600, 1600).unwrap();
        assert_eq!((picture.width(), picture.height()), (600, 600));
        let tall = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="40000"><rect width="200" height="40000"/></svg>"#;
        let picture = super::decode(tall, 600, 1600).unwrap();
        assert!(picture.height() <= 1600 && picture.width() <= 8, "{}x{}", picture.width(), picture.height());
    }

    #[test]
    fn mermaid_text_cannot_close_the_script() {
        let page = super::page_html("graph TD; A-->B</script><script>alert(1)", false, std::path::Path::new("mermaid-0.min.js"));
        assert!(!page.contains("B</script>"));
    }
}
