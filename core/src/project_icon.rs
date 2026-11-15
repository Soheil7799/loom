//! A project's icon, for the sidebar and its notifications: a picture the user chose
//! (`.loom/icon.svg` or `.png`), else the repository's own logo, else loom's fox in the
//! project's own colour (`.loom/fox`: a hue the first time it is asked for, or a colour the
//! user picked), with the project's first letter on it.

use std::path::{Path, PathBuf};

const APP_ICON: &str = include_str!("../../data/dev.shirvani.Loom.svg");
/// The hues a fox gets on its own: teal to pink. The warm half reads as loom's signals
/// (red: failed, gold: waits, green: done), a fox in it as one more of them.
const HUES: std::ops::RangeInclusive<u16> = 185..=340;
/// Where a repository usually keeps its logo, looked for in this order.
const LOGOS: [&str; 13] = [
    "logo.svg",
    "logo.png",
    "icon.svg",
    "icon.png",
    "assets/logo.svg",
    "assets/logo.png",
    "assets/icon.svg",
    "assets/icon.png",
    ".github/logo.svg",
    ".github/logo.png",
    "public/logo.svg",
    "public/favicon.svg",
    "static/favicon.svg",
];

/// The icon's file for `project`, in dark or light colours. `others` are the other projects:
/// a project without a colour yet takes the hue farthest from theirs.
pub fn file(project: &Path, others: &[PathBuf], dark: bool) -> PathBuf {
    if let Some(own) = chosen(project).or_else(|| logo(project)) {
        return own;
    }
    let colour = colour(project, others);
    let letter = project.file_name().and_then(|n| n.to_str()).and_then(|n| n.chars().find(|c| c.is_alphanumeric()));
    let svg = fox(colour, letter, dark);
    let path = crate::state::dir().join("icons").join(format!("{:016x}-{}.svg", hash(project), if dark { "dark" } else { "light" }));
    if std::fs::read_to_string(&path).ok().as_deref() != Some(&svg) {
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
        let _ = std::fs::write(&path, svg);
    }
    path
}

/// The picture the user chose for it, if any.
fn chosen(project: &Path) -> Option<PathBuf> {
    ["icon.svg", "icon.png"].iter().map(|f| project.join(".loom").join(f)).find(|p| p.is_file())
}

/// The repository's own logo: a usual name, or a desktop app's icon (named by its app id,
/// as `data/dev.example.App.svg`).
fn logo(project: &Path) -> Option<PathBuf> {
    LOGOS.iter().map(|f| project.join(f)).find(|p| p.is_file()).or_else(|| {
        ["data", "data/icons/hicolor/scalable/apps"].iter().find_map(|dir| {
            let mut found: Vec<PathBuf> = std::fs::read_dir(project.join(dir))
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "svg") && p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.matches('.').count() >= 2))
                .collect();
            found.sort();
            found.into_iter().next()
        })
    })
}

/// Use `picture` (an SVG or PNG) as the project's icon; `None`: back to its logo or fox.
pub fn choose(project: &Path, picture: Option<&Path>) -> std::io::Result<()> {
    for old in ["icon.svg", "icon.png"].map(|f| project.join(".loom").join(f)).into_iter().filter(|p| p.is_file()) {
        std::fs::remove_file(old)?;
    }
    let Some(picture) = picture else { return Ok(()) };
    let ext = if picture.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) { "svg" } else { "png" };
    std::fs::create_dir_all(project.join(".loom"))?;
    std::fs::copy(picture, project.join(".loom").join(format!("icon.{ext}"))).map(|_| ())
}

/// The fox's colour for `project`: `#rrggbb` picked by the user; `None`: one of its own again.
pub fn pick(project: &Path, colour: Option<&str>) -> std::io::Result<()> {
    let file = project.join(".loom").join("fox");
    match colour {
        Some(c) => {
            std::fs::create_dir_all(project.join(".loom"))?;
            std::fs::write(file, format!("{c}\n"))
        }
        None if file.exists() => std::fs::remove_file(file),
        None => Ok(()),
    }
}

/// Whether the icon is the user's doing (a picture or a colour), so it can be undone.
pub fn custom(project: &Path) -> bool {
    chosen(project).is_some() || matches!(read(project), Some(Colour::Picked(_)))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Colour {
    Hue(u16),
    /// sRGB, 0 to 1.
    Picked([f64; 3]),
}

fn read(project: &Path) -> Option<Colour> {
    let text = std::fs::read_to_string(project.join(".loom").join("fox")).ok()?;
    let text = text.trim();
    match text.strip_prefix('#') {
        Some(hex) if hex.len() == 6 => {
            let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|b| f64::from(b) / 255.0);
            Some(Colour::Picked([byte(0)?, byte(2)?, byte(4)?]))
        }
        Some(_) => None,
        None => text.parse().ok().map(Colour::Hue),
    }
}

fn colour(project: &Path, others: &[PathBuf]) -> Colour {
    if let Some(c) = read(project) {
        return c;
    }
    let used: Vec<u16> = others.iter().filter(|p| *p != project).filter_map(|p| match read(p) {
        Some(Colour::Hue(h)) => Some(h),
        _ => None,
    }).collect();
    let hue = farthest(&used);
    // Kept, so it stays this project's colour when others come and go.
    if project.is_dir() {
        let _ = std::fs::create_dir_all(project.join(".loom")).and_then(|()| std::fs::write(project.join(".loom").join("fox"), format!("{hue}\n")));
    }
    Colour::Hue(hue)
}

/// The hue in `HUES` farthest from every used one; the first is violet, away from loom's blue.
fn farthest(used: &[u16]) -> u16 {
    let dist = |a: u16, b: u16| {
        let d = a.abs_diff(b) % 360;
        d.min(360 - d)
    };
    HUES.rev().max_by_key(|&h| used.iter().map(|&u| dist(h, u)).min().unwrap_or(360 - dist(h, 290))).unwrap_or(290)
}

fn hash(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    h.finish()
}

/// The fox on its tile, as the app icon is drawn (`data/dev.shirvani.Loom.svg`).
fn fox(colour: Colour, letter: Option<char>, dark: bool) -> String {
    let (tile, fox) = match colour {
        Colour::Hue(h) => {
            let h = f64::from(h);
            if dark {
                ([oklch(0.33, 0.07, h), oklch(0.24, 0.05, h)], [oklch(0.93, 0.07, h), oklch(0.74, 0.15, h)])
            } else {
                ([oklch(0.95, 0.03, h), oklch(0.89, 0.05, h)], [oklch(0.55, 0.16, h), oklch(0.40, 0.14, h)])
            }
        }
        Colour::Picked(rgb) => picked(rgb),
    };
    let start = APP_ICON.find("<svg x=").unwrap_or(0);
    let end = APP_ICON.rfind("</svg>").unwrap_or(APP_ICON.len());
    let mark = APP_ICON[start..end].replace("url(#agile-l-dark-blue)", "url(#f)");
    let badge = letter.map_or(String::new(), |l| {
        let back = if dark { "#051122" } else { "#ffffff" };
        format!(
            r##"<rect x="68" y="68" width="56" height="56" rx="14" fill="{back}"/><text x="96" y="110" text-anchor="middle" font-family="Onest, sans-serif" font-size="40" font-weight="500" fill="{}">{}</text>"##,
            hex(fox[1]),
            l.to_uppercase()
        )
    });
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128"><defs><linearGradient id="t" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient><linearGradient id="f" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient></defs><rect x="6" y="6" width="116" height="116" rx="28" fill="url(#t)"/>{mark}{badge}</svg>
"##,
        hex(tile[0]),
        hex(tile[1]),
        hex(fox[0]),
        hex(fox[1])
    )
}

/// A picked fox colour keeps its colour; its tile takes the same hue, darker or lighter until
/// the fox stands out at 3:1 at least.
fn picked(rgb: [f64; 3]) -> ([[f64; 3]; 2], [[f64; 3]; 2]) {
    let (l, c, h) = to_oklch(rgb);
    let dark_tile = l > 0.6;
    let mut tl = if dark_tile { 0.33 } else { 0.95 };
    while contrast(rgb, oklch(tl, 0.06, h)) < 3.0 && (0.12..=0.99).contains(&tl) {
        tl += if dark_tile { -0.02 } else { 0.02 };
    }
    ([oklch(tl, 0.06, h), oklch(tl - 0.07, 0.05, h)], [oklch((l + 0.12).min(0.97), c * 0.6, h), rgb])
}

/// OKLCH to sRGB (0 to 1), the chroma lowered until it fits.
fn oklch(l: f64, c: f64, h: f64) -> [f64; 3] {
    let mut c = c;
    loop {
        let (a, b) = (c * h.to_radians().cos(), c * h.to_radians().sin());
        let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
        let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
        let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548 * b).powi(3);
        let lin = [
            4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_,
            -1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_,
            -0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701 * s_,
        ];
        if c <= 0.0 || lin.iter().all(|v| (-1e-4..=1.0001).contains(v)) {
            return lin.map(|v| {
                let v = v.clamp(0.0, 1.0);
                if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
            });
        }
        c -= 0.005;
    }
}

fn linear(v: f64) -> f64 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn to_oklch(rgb: [f64; 3]) -> (f64, f64, f64) {
    let [r, g, b] = rgb.map(linear);
    let l = (0.412_221_470_8 * r + 0.536_337_155_8 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    let big_l = 0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s;
    let a = 1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s;
    let b = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s;
    (big_l, a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0))
}

fn contrast(x: [f64; 3], y: [f64; 3]) -> f64 {
    let lum = |c: [f64; 3]| {
        let [r, g, b] = c.map(linear);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    };
    let (a, b) = (lum(x), lum(y));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn hex(rgb: [f64; 3]) -> String {
    let [r, g, b] = rgb.map(|v| (v * 255.0).round() as u8);
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hues_spread_and_stay_cool() {
        let mut used = Vec::new();
        for _ in 0..12 {
            let h = farthest(&used);
            assert!(HUES.contains(&h), "{h}");
            assert!(!used.contains(&h));
            used.push(h);
        }
        assert_eq!(used[0], 290);
        // the first six far apart (both ends of the range count)
        let mut six = used[..6].to_vec();
        six.sort();
        assert!(six.windows(2).all(|w| w[1] - w[0] >= 25), "{six:?}");
    }

    #[test]
    fn a_picked_fox_stands_out() {
        for rgb in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [1.0, 0.56, 0.85], [0.1, 0.1, 0.4], [0.5, 0.5, 0.5]] {
            let (tile, fox) = picked(rgb);
            assert_eq!(fox[1], rgb);
            assert!(contrast(rgb, tile[0]) >= 3.0, "{rgb:?} on {}", hex(tile[0]));
        }
    }

    #[test]
    fn logo_then_chosen_then_fox() {
        let project = std::env::temp_dir().join(format!("loom-icon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(project.join("data")).unwrap();
        assert_eq!(logo(&project), None);
        std::fs::write(project.join("data/dev.example.App.svg"), "<svg/>").unwrap();
        assert_eq!(logo(&project), Some(project.join("data/dev.example.App.svg")));
        let picture = project.join("mine.png");
        std::fs::write(&picture, "png").unwrap();
        choose(&project, Some(&picture)).unwrap();
        assert_eq!(file(&project, &[], true), project.join(".loom/icon.png"));
        assert!(custom(&project));
        choose(&project, None).unwrap();
        assert!(!custom(&project));
        pick(&project, Some("#ff8fd8")).unwrap();
        assert_eq!(read(&project), Some(Colour::Picked([1.0, 143.0 / 255.0, 216.0 / 255.0])));
        std::fs::remove_dir_all(&project).unwrap();
    }

    #[test]
    fn the_fox_is_recoloured() {
        let svg = fox(Colour::Hue(290), Some('l'), true);
        assert!(svg.contains("fill=\"url(#f)\""));
        assert!(svg.contains(">L</text>"));
        assert_eq!(svg.matches("<svg").count(), 2);
    }
}
