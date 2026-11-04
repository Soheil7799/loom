//! The web page's built files (`../web/dist`, from `npm run build` in `web/`) compiled into
//! the binary, so one package has them (`api/web.rs` serves them). Without a build there, none:
//! loom then serves `LOOM_WEB_UI`, `<data dir>/loom/web` or its small page built in.

use std::path::{Path, PathBuf};

fn main() {
    let dist = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../web/dist");
    // Cargo scans a watched folder whole, and takes a missing path as changed on every build:
    // the build when there is one, else `web/` (where one would appear), else nothing.
    let mut files = Vec::new();
    if dist.join("index.html").is_file() {
        println!("cargo::rerun-if-changed={}", dist.display());
        collect(&dist, &dist, &mut files);
    } else if dist.parent().is_some_and(Path::is_dir) {
        println!("cargo::rerun-if-changed={}", dist.parent().unwrap().display());
    } else {
        println!("cargo::rerun-if-changed=build.rs");
    }
    files.sort();
    let mut code = String::from("/// The web page's built files: (`/` + path in `web/dist`, contents).\npub static EMBEDDED: &[(&str, &[u8])] = &[\n");
    for (path, file) in &files {
        code.push_str(&format!("    ({path:?}, include_bytes!({:?})),\n", file.display().to_string()));
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("web_ui.rs");
    std::fs::write(out, code).unwrap();
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files);
        } else if let Ok(relative) = path.strip_prefix(root) {
            let Some(relative) = relative.to_str() else { continue };
            files.push((format!("/{}", relative.replace('\\', "/")), path.canonicalize().unwrap_or(path.clone())));
        }
    }
}
