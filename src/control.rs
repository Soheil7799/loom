//! loom's control socket (`loom_core::control`), answered on the main thread.

use gtk::glib;
use serde_json::Value;

pub use loom_core::control::*;

/// Listens on `path`; `handle` answers each request on the main thread.
pub fn serve(path: std::path::PathBuf, handle: impl Fn(u32, &str, &Value) -> Result<Value, String> + 'static) {
    let Some(requests) = loom_core::control::serve(path) else { return };
    glib::spawn_future_local(async move {
        while let Ok(request) = requests.recv().await {
            let answer = handle(request.agent, &request.method, &request.params);
            request.answer(answer);
        }
    });
}
