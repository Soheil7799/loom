//! Icons for files by their kind: Catppuccin's file icons (MIT, data/icons/LICENSE-catppuccin),
//! built into loom so the desktop's icon theme doesn't change them. A file's icon comes from
//! its whole name (Cargo.toml, Dockerfile), else its extension, else the plain file icon;
//! every folder has the plain folder icon (its name says what it is).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use gtk::{gio, glib};
use serde_json::Value;

static SET: &str = include_str!("../data/icons/catppuccin.json");

/// Whole names (lowercase) with an icon of their own.
const NAMES: &[(&str, &str)] = &[
    ("cargo.toml", "cargo"),
    ("cargo.lock", "cargo-lock"),
    ("package.json", "package-json"),
    ("package-lock.json", "npm-lock"),
    ("pnpm-lock.yaml", "pnpm-lock"),
    ("yarn.lock", "yarn-lock"),
    ("bun.lockb", "bun-lock"),
    ("bun.lock", "bun-lock"),
    ("deno.lock", "deno-lock"),
    ("flake.lock", "nix-lock"),
    ("go.mod", "go-mod"),
    ("go.sum", "go-mod"),
    ("tsconfig.json", "typescript-config"),
    ("jsconfig.json", "javascript-config"),
    ("dockerfile", "docker"),
    ("containerfile", "docker"),
    ("docker-compose.yml", "docker-compose"),
    ("docker-compose.yaml", "docker-compose"),
    ("compose.yml", "docker-compose"),
    ("compose.yaml", "docker-compose"),
    (".dockerignore", "docker-ignore"),
    (".gitignore", "git"),
    (".gitattributes", "git"),
    (".gitmodules", "git"),
    (".editorconfig", "editorconfig"),
    (".envrc", "envrc"),
    ("makefile", "makefile"),
    ("gnumakefile", "makefile"),
    ("justfile", "just"),
    (".justfile", "just"),
    ("license", "license"),
    ("licence", "license"),
    ("copying", "license"),
    ("todo", "todo"),
    ("todo.md", "todo"),
    ("readme", "readme"),
    ("changelog", "changelog"),
    ("codeowners", "codeowners"),
    (".prettierrc", "prettier"),
    ("vite.config.ts", "vite"),
    ("vite.config.js", "vite"),
    ("tailwind.config.js", "tailwind"),
    ("tailwind.config.ts", "tailwind"),
    ("robots.txt", "robots"),
    ("favicon.ico", "favicon"),
];

/// Extensions (lowercase) whose icon has another name than themselves.
const EXTENSIONS: &[(&str, &str)] = &[
    ("rs", "rust"),
    ("md", "markdown"),
    ("markdown", "markdown"),
    ("mdx", "markdown-mdx"),
    ("py", "python"),
    ("pyc", "python-compiled"),
    ("ts", "typescript"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("tsx", "typescript-react"),
    ("js", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("jsx", "javascript-react"),
    ("sh", "bash"),
    ("zsh", "bash"),
    ("fish", "bash"),
    ("yml", "yaml"),
    ("h", "c-header"),
    ("hh", "cpp-header"),
    ("hpp", "cpp-header"),
    ("cc", "cpp"),
    ("cxx", "cpp"),
    ("scss", "sass"),
    ("htm", "html"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("rb", "ruby"),
    ("ex", "elixir"),
    ("exs", "elixir"),
    ("hs", "haskell"),
    ("cs", "csharp"),
    ("ps1", "powershell"),
    ("bat", "batch"),
    ("cmd", "batch"),
    ("sql", "database"),
    ("db", "database"),
    ("sqlite", "database"),
    ("txt", "text"),
    ("env", "env"),
    ("ini", "config"),
    ("conf", "config"),
    ("cfg", "config"),
    ("tex", "latex"),
    ("typ", "typst"),
    ("wasm", "web-assembly"),
    ("patch", "diff"),
    ("pem", "certificate"),
    ("crt", "certificate"),
    ("key", "key"),
    ("pub", "key"),
    ("doc", "ms-word"),
    ("docx", "ms-word"),
    ("odt", "ms-word"),
    ("xls", "ms-excel"),
    ("xlsx", "ms-excel"),
    ("ods", "ms-excel"),
    ("ppt", "ms-powerpoint"),
    ("pptx", "ms-powerpoint"),
    ("exe", "exe"),
    ("bin", "binary"),
    ("so", "binary"),
    ("o", "binary"),
    ("a", "binary"),
    ("jar", "java-jar"),
    ("ttf", "font"),
    ("otf", "font"),
    ("woff", "font"),
    ("woff2", "font"),
    ("png", "image"),
    ("jpg", "image"),
    ("jpeg", "image"),
    ("gif", "image"),
    ("webp", "image"),
    ("avif", "image"),
    ("bmp", "image"),
    ("ico", "image"),
    ("mp3", "audio"),
    ("wav", "audio"),
    ("ogg", "audio"),
    ("flac", "audio"),
    ("opus", "audio"),
    ("m4a", "audio"),
    ("mp4", "video"),
    ("mkv", "video"),
    ("webm", "video"),
    ("mov", "video"),
    ("avi", "video"),
    ("tar", "zip"),
    ("gz", "zip"),
    ("tgz", "zip"),
    ("xz", "zip"),
    ("zst", "zip"),
    ("bz2", "zip"),
    ("7z", "zip"),
    ("rar", "zip"),
    ("jsonl", "json"),
    ("jsonc", "json"),
    ("json5", "json"),
    ("lock", "lock"),
    ("vim", "vim"),
];

thread_local! {
    /// Each icon's SVG body and size, read from the set when first wanted.
    static BODIES: HashMap<String, (String, u64, u64)> = bodies();
    static MADE: RefCell<HashMap<&'static str, gio::Icon>> = RefCell::default();
}

fn bodies() -> HashMap<String, (String, u64, u64)> {
    let Ok(set) = serde_json::from_str::<Value>(SET) else { return HashMap::new() };
    let (width, height) = (set["width"].as_u64().unwrap_or(16), set["height"].as_u64().unwrap_or(16));
    let Some(icons) = set["icons"].as_object() else { return HashMap::new() };
    icons
        .iter()
        .filter_map(|(name, icon)| Some((name.clone(), (icon["body"].as_str()?.to_owned(), icon["width"].as_u64().unwrap_or(width), icon["height"].as_u64().unwrap_or(height)))))
        .collect()
}

/// The icon's name in the set for a file or a folder.
fn name_for(path: &Path, is_dir: bool) -> String {
    if is_dir {
        return "folder".to_owned();
    }
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if let Some((_, icon)) = NAMES.iter().find(|(n, _)| *n == name) {
        return (*icon).to_owned();
    }
    // README.md, LICENSE-MIT, CHANGELOG.txt, .env.local: by what the name starts with.
    let stem = name.split('.').find(|s| !s.is_empty()).unwrap_or("");
    for (start, icon) in [("readme", "readme"), ("license", "license"), ("licence", "license"), ("changelog", "changelog"), ("contributing", "contributing"), ("dockerfile", "docker"), ("env", "env")] {
        if stem == start || (start != "env" && stem.starts_with(start)) {
            return icon.to_owned();
        }
    }
    let extension = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    if let Some((_, icon)) = EXTENSIONS.iter().find(|(e, _)| *e == extension) {
        return (*icon).to_owned();
    }
    // Many icons are named as their extension (go, lua, toml, json, zig…).
    if !extension.is_empty() && BODIES.with(|b| b.contains_key(extension)) {
        return extension.to_owned();
    }
    "file".to_owned()
}

/// The icon for a file (by its name) or a folder.
pub fn for_file(path: &Path, is_dir: bool) -> gio::Icon {
    let name = name_for(path, is_dir);
    // Made once for each kind: a leaked name is a few bytes, there are a few hundred kinds.
    if let Some(icon) = MADE.with(|m| m.borrow().get(name.as_str()).cloned()) {
        return icon;
    }
    let svg = BODIES.with(|b| b.get(&name).map(|(body, w, h)| format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{body}</svg>"#)));
    let icon: gio::Icon = match svg {
        Some(svg) => gio::BytesIcon::new(&glib::Bytes::from_owned(svg.into_bytes())).into(),
        None => gio::ThemedIcon::new(if is_dir { "folder-symbolic" } else { "text-x-generic-symbolic" }).into(),
    };
    MADE.with(|m| m.borrow_mut().insert(Box::leak(name.into_boxed_str()), icon.clone()));
    icon
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_get_their_kind_of_icon() {
        let icon = |name: &str| name_for(Path::new(name), false);
        assert_eq!(icon("Cargo.toml"), "cargo");
        assert_eq!(icon("main.rs"), "rust");
        assert_eq!(icon("README.md"), "readme");
        assert_eq!(icon("LICENSE-MIT"), "license");
        assert_eq!(icon(".env.local"), "env");
        assert_eq!(icon("environment.ts"), "typescript");
        assert_eq!(icon("x.toml"), "toml");
        assert_eq!(icon("x.go"), "go");
        assert_eq!(icon("photo.JPG"), "image");
        assert_eq!(icon("mystery.qqq"), "file");
        assert_eq!(name_for(Path::new("src"), true), "folder");
        // Every icon named in the tables is in the set.
        for (_, name) in NAMES.iter().chain(EXTENSIONS) {
            assert!(BODIES.with(|b| b.contains_key(*name)), "{name}");
        }
    }
}
