//! loom: a light, keyboard-first home for coding agents. Each agent is a Claude Code
//! session over ACP (claude-agent-acp), shown as a native chat; commands land in a job tray,
//! files in a side editor, and a scratch terminal is one key away.

mod acp;
mod agent;
mod app;
mod chat;
mod devhooks;
mod files;
mod jobs;
mod markdown;
mod quickopen;
mod scratch;
mod state;
mod term;

use gtk::{gdk, gio, glib, prelude::*};

const APP_ID: &str = "dev.shirvani.Loom";

/// The window in the GTK theme's colours (what noctalia or matugen write), so it follows
/// the wallpaper like every other app. Status colours are loom's own.
const CSS: &str = r#"
@define-color loom_ok #87d96c;
@define-color loom_warn #ffcc66;
@define-color loom_err #f28779;

window.loom { font-family: "Inter Variable", Inter, Vazirmatn, sans-serif; background: @window_bg_color; color: @window_fg_color; }
.sidebar-bottom button:not(:hover):not(:checked) { background: none; box-shadow: none; }
.mono, .tool-title, .chip, .code-block, .diff label, .help { font-family: "JetBrains Mono", Vazirmatn, monospace; }
.dim { opacity: 0.6; }
.small { font-size: 0.85em; }
.title { font-weight: 700; }

.sidebar { background: @sidebar_bg_color; border-right: 1px solid alpha(@window_fg_color, 0.07); }
.sidebar-top { padding: 12px 12px 6px 16px; }
.brand { font-weight: 800; letter-spacing: 0.04em; opacity: 0.8; }
.sidebar-bottom { padding: 6px; border-top: 1px solid alpha(@window_fg_color, 0.07); }
.agents { background: transparent; }
.agents row { border-radius: 8px; margin: 1px 6px; padding: 6px 10px; }
.agents row:selected { background: alpha(@accent_color, 0.18); color: @window_fg_color; }
.agents row.agent-row:hover:not(:selected) { background: alpha(@window_fg_color, 0.05); }
.project-header { font-size: 0.75em; font-weight: 700; letter-spacing: 0.08em; opacity: 0.5; margin: 10px 4px 2px 4px; }
.inbox-button { color: @loom_warn; font-weight: 700; }
.inbox { padding: 6px; }
.inbox-agent { font-weight: 700; }

.dot { min-width: 9px; min-height: 9px; border-radius: 99px; background: alpha(@window_fg_color, 0.25); }
.dot.working, .dot.starting { background: @accent_color; animation: pulse 1.4s ease-in-out infinite; }
.dot.blocked { background: @loom_warn; }
.dot.done { background: @loom_ok; }
.dot.failed { background: @loom_err; }
.dot.suspended { background: transparent; box-shadow: inset 0 0 0 1.5px alpha(@window_fg_color, 0.35); }
@keyframes pulse { 0% { opacity: 1; } 50% { opacity: 0.35; } 100% { opacity: 1; } }

.chat { background: @view_bg_color; }
.header { padding: 10px 18px; border-bottom: 1px solid alpha(@window_fg_color, 0.06); }
.pill { font-size: 0.8em; padding: 2px 10px; min-height: 0; border-radius: 99px; background: alpha(@window_fg_color, 0.08); }
.pill.plan-mode { background: alpha(@accent_color, 0.25); }
.pill.accept-mode { background: alpha(@loom_warn, 0.22); }
.pill.bypass-mode { background: alpha(@loom_err, 0.3); }
.messages { padding: 18px 28px 24px 28px; }
.msg-user { background: alpha(@accent_color, 0.12); border-radius: 12px; padding: 10px 14px; }
.msg-agent { padding: 0 2px; }
.thought { opacity: 0.6; }
.notice { font-style: italic; opacity: 0.7; }
.notice.error { color: @loom_err; opacity: 1; }
.code-block { background: alpha(@window_fg_color, 0.06); border-radius: 8px; padding: 8px 10px; font-size: 0.92em; }
.tool-card { border-radius: 8px; background: alpha(@window_fg_color, 0.035); }
.tool-toggle { padding: 4px 10px; border-radius: 8px; }
.tool-title { font-size: 0.9em; }
.kind { font-size: 0.75em; font-weight: 700; opacity: 0.55; text-transform: uppercase; }
.glyph.done { color: @loom_ok; }
.glyph.failed { color: @loom_err; }
.tool-detail { padding: 4px 12px 10px 12px; }
.diff-add { color: @loom_ok; }
.diff-del { color: @loom_err; }
.permission-card { border: 1px solid alpha(@loom_warn, 0.55); background: alpha(@loom_warn, 0.07); border-radius: 10px; padding: 12px; }
button.suggested { background: alpha(@accent_color, 0.85); color: @accent_fg_color; }
.plan { padding: 8px 28px; border-top: 1px solid alpha(@window_fg_color, 0.06); font-size: 0.9em; }
.plan .done { opacity: 0.45; }
.plan .current { font-weight: 700; }
.input-box { padding: 10px 18px 14px 18px; }
textview.input { border-radius: 10px; background: alpha(@window_fg_color, 0.05); font-size: 1.02em; }
textview.input text { background: transparent; }
.completion row { padding: 3px 8px; }

.tray { padding: 4px 8px; border-top: 1px solid alpha(@window_fg_color, 0.07); background: @sidebar_bg_color; }
.tray .hint { font-size: 0.78em; opacity: 0.4; margin-right: 6px; }
.chip { padding: 2px 10px; min-height: 0; border-radius: 99px; font-size: 0.82em; }
.dock { padding: 6px; background: @sidebar_bg_color; }
.docked, .peek-frame { padding: 4px; }
button.small { padding: 0 8px; min-height: 22px; font-size: 0.85em; }

.files { background: @sidebar_bg_color; border-left: 1px solid alpha(@window_fg_color, 0.07); }
.panel-header { padding: 8px 12px; }
.file-list { background: transparent; }
.file-row { padding: 4px 6px; }
.badge { font-size: 0.72em; font-weight: 700; padding: 0 6px; border-radius: 99px; }
.badge.new { background: alpha(@loom_ok, 0.22); }
.badge.edited { background: alpha(@accent_color, 0.22); }
.dirty { color: @loom_warn; }
.editor { font-family: "JetBrains Mono", Vazirmatn, monospace; font-size: 0.95em; }

.scratch { background: @window_bg_color; border-radius: 12px; padding: 8px; border: 1px solid alpha(@accent_color, 0.45); box-shadow: 0 10px 30px rgba(0, 0, 0, 0.45); }
window.loom paned > separator { background: rgba(128, 128, 128, 0.18); min-width: 1px; min-height: 1px; }
.prefix-hint { background: alpha(@window_bg_color, 0.95); border: 1px solid alpha(@accent_color, 0.6); border-radius: 99px; padding: 6px 16px; font-size: 0.88em; box-shadow: 0 6px 20px rgba(0, 0, 0, 0.35); }
.tab { padding: 2px 10px; min-height: 0; border-radius: 99px; font-weight: 600; }
.tab:checked { background: alpha(@accent_color, 0.22); }
.tree row { padding: 1px 6px; }
.tree .folder { font-weight: 600; }
.editor-message { padding: 2px 12px 6px 12px; color: @loom_warn; font-size: 0.88em; }
.switcher-list row { padding: 6px 10px; border-radius: 6px; }
.help { padding: 6px; font-size: 0.9em; }
"#;

fn main() -> glib::ExitCode {
    // LOOM_APP_ID: a separate instance (tests), with its own saved agents.
    let app_id = std::env::var("LOOM_APP_ID").unwrap_or_else(|_| APP_ID.to_owned());
    let app = gtk::Application::builder().application_id(app_id).flags(gio::ApplicationFlags::empty()).build();
    app.connect_startup(|_| {
        let css = gtk::CssProvider::new();
        css.load_from_string(CSS);
        if let Some(display) = gdk::Display::default() {
            // Above the user's GTK stylesheet (which it reads colours from), like nevis's chrome.
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_USER + 2);
        }
    });
    app.connect_activate(|app| {
        // A second launch brings the window back rather than opening another.
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        let loom = app::App::new(app);
        loom.window.present();
        devhooks::watch(&loom);
        // The window owns everything through its widgets; keep the app state alive with it.
        // SAFETY: the key stores the App and is read by nothing; it drops with the window.
        unsafe { loom.window.set_data("loom", loom.clone()) };
    });
    app.run()
}
