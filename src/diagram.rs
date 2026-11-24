//! Diagrams in the chat: SVG drawn by GTK itself (librsvg, through gdk-pixbuf), Mermaid turned
//! into SVG first by a headless run of the Chromium loom finds (Mermaid's script ships inside
//! loom: data/mermaid, MIT, version in data/mermaid/VERSION). Nothing goes on the network: the
//! page is a local file and the browser resolves no host.

use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gtk::{gdk, gio, glib};

const MERMAID_JS: &[u8] = include_bytes!("../data/mermaid/mermaid.min.js");

/// A Mermaid render that takes longer than this is given up.
const LIMIT: Duration = Duration::from_secs(20);

/// The languages drawn as a picture, not shown as code.
pub fn is_diagram(lang: &str) -> bool {
    matches!(lang.to_ascii_lowercase().as_str(), "svg" | "mermaid")
}

/// `svg` as a picture at most `widest` pixels wide (its own size if smaller).
pub fn texture(svg: &[u8], widest: i32) -> Result<gdk::Texture, String> {
    let stream = |bytes: &[u8]| gio::MemoryInputStream::from_bytes(&glib::Bytes::from(bytes));
    let read = |e: glib::Error| format!("not a picture GTK can read ({e})");
    let natural = gtk::gdk_pixbuf::Pixbuf::from_stream(&stream(svg), gio::Cancellable::NONE).map_err(read)?;
    let pixbuf = if natural.width() > widest {
        gtk::gdk_pixbuf::Pixbuf::from_stream_at_scale(&stream(svg), widest, -1, true, gio::Cancellable::NONE).map_err(read)?
    } else {
        natural
    };
    #[allow(deprecated)]
    Ok(gdk::Texture::for_pixbuf(&pixbuf))
}

/// Mermaid source as SVG (from the cache when drawn before). Blocking: run it off the main
/// thread.
pub fn mermaid(code: &str, dark: bool) -> Result<String, String> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (code, dark, MERMAID_JS.len()).hash(&mut hasher);
    let dir = glib::user_cache_dir().join("loom").join("diagrams");
    let cached = dir.join(format!("{:016x}.svg", hasher.finish()));
    if let Ok(svg) = std::fs::read_to_string(&cached) {
        return Ok(svg);
    }
    let browser = loom_core::browser::chosen().ok_or("no Chromium-based browser found to draw Mermaid with")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let script = dir.join("mermaid.min.js");
    if std::fs::metadata(&script).map(|m| m.len()).ok() != Some(MERMAID_JS.len() as u64) {
        std::fs::write(&script, MERMAID_JS).map_err(|e| e.to_string())?;
    }
    let work = tempdir(&dir)?;
    let page = work.join("page.html");
    std::fs::write(&page, page_html(code, dark)).map_err(|e| e.to_string())?;
    let out = run(&browser, &work, &page);
    let _ = std::fs::remove_dir_all(&work);
    let svg = svg_from_dom(&out?)?;
    let _ = std::fs::write(&cached, &svg);
    Ok(svg)
}

/// The page that draws `code`: Mermaid's own labels as SVG text (GTK can't draw the HTML ones),
/// the result written into the body as text for `--dump-dom` to print.
fn page_html(code: &str, dark: bool) -> String {
    // A JSON string, with no "</script>" able to close the element early.
    let code = serde_json::to_string(code).unwrap_or_default().replace('<', "\\u003c");
    let theme = if dark { "dark" } else { "default" };
    format!(
        r#"<!doctype html><meta charset="utf-8"><body><script src="../mermaid.min.js"></script><script>
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
    fn mermaid_text_cannot_close_the_script() {
        let page = super::page_html("graph TD; A-->B</script><script>alert(1)", false);
        assert!(!page.contains("B</script>"));
    }
}
