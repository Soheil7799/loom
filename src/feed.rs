//! The window's copies of the core API's resources it draws (warp): subscribed once, kept up
//! to date by applying what comes; a gap asks for a fresh snapshot.

use std::cell::{Ref, RefCell};
use std::rc::{Rc, Weak};
use std::path::PathBuf;

use loom_core::agent::Status;
use loom_core::api::{Applied, Client, Mirror, Subscription};
use serde_json::Value;

pub struct Feed {
    sub: Rc<Subscription>,
    mirror: Rc<RefCell<Mirror>>,
}

/// A feed as the self-check sees it (dev builds).
struct Watched {
    resource: String,
    sub: Weak<Subscription>,
    mirror: Weak<RefCell<Mirror>>,
}

thread_local! {
    static FEEDS: RefCell<Vec<Watched>> = const { RefCell::new(Vec::new()) };
}

impl Feed {
    /// Follows `resource`; `ready` is called when something new waits (call `update` then).
    pub fn new(client: &Client, resource: &str, ready: impl Fn() + 'static) -> Option<Feed> {
        let sub = Rc::new(client.subscribe(resource, ready).ok()?);
        let feed = Feed { sub, mirror: Rc::default() };
        feed.update();
        if cfg!(debug_assertions) {
            FEEDS.with_borrow_mut(|feeds| {
                feeds.retain(|w| w.sub.strong_count() > 0);
                feeds.push(Watched { resource: resource.to_owned(), sub: Rc::downgrade(&feed.sub), mirror: Rc::downgrade(&feed.mirror) });
            });
        }
        Some(feed)
    }

    /// Applies what waits.
    pub fn update(&self) {
        loop {
            let messages = self.sub.drain();
            if messages.is_empty() {
                return;
            }
            let mut mirror = self.mirror.borrow_mut();
            for m in &messages {
                if mirror.apply(m) == Applied::Gap {
                    // What follows the gap is dropped: the snapshot asked for brings it.
                    self.sub.resync();
                    break;
                }
            }
        }
    }

    pub fn items(&self) -> Ref<'_, Vec<Value>> {
        Ref::map(self.mirror.borrow(), |m| &m.items)
    }
}

/// Dev builds: where the window's copies differ from the core's resources (each a line
/// saying which and how; empty: they agree). Copies with messages still on their way, or
/// whose resource has changes not yet sent, are skipped (they are behind, not wrong).
pub fn self_check() -> Vec<String> {
    let hub = loom_core::api::hub();
    let mut wrong = Vec::new();
    FEEDS.with_borrow(|feeds| {
        for watched in feeds {
            let (Some(sub), Some(mirror)) = (watched.sub.upgrade(), watched.mirror.upgrade()) else { continue };
            let mirror = mirror.borrow();
            let Some(store) = hub.existing(&watched.resource) else { continue };
            if !mirror.synced || mirror.gone || sub.waiting() || store.unsent() {
                continue;
            }
            let core = store.items();
            if core != mirror.items {
                let at = core.iter().zip(&mirror.items).position(|(a, b)| a != b).unwrap_or(core.len().min(mirror.items.len()));
                wrong.push(format!(
                    "{}: the window has {} items, the core {}; first difference at {at}: window {} / core {}",
                    watched.resource,
                    mirror.items.len(),
                    core.len(),
                    mirror.items.get(at).map_or("-".into(), Value::to_string),
                    core.get(at).map_or("-".into(), Value::to_string),
                ));
            }
        }
    });
    wrong
}

/// Dev builds: checks the window's copies against the core every few seconds, and answers
/// `_selfcheck` (the end-to-end tests ask it last), so a copy that drifts is said at once.
pub fn watch_copies() {
    if !cfg!(debug_assertions) {
        return;
    }
    loom_core::rt::every(std::time::Duration::from_secs(5), || {
        for line in self_check() {
            eprintln!("loom self-check: {line}");
        }
        true
    });
    loom_core::api::hub().handle("_selfcheck", |who, _| {
        if !matches!(who, loom_core::api::Principal::Window | loom_core::api::Principal::Cli) {
            return Err(loom_core::api::Error::Forbidden("_selfcheck".into()));
        }
        let wrong = self_check();
        let feeds = FEEDS.with_borrow(|feeds| feeds.iter().filter(|w| w.sub.strong_count() > 0).map(|w| w.resource.clone()).collect::<Vec<_>>());
        Ok(serde_json::json!({ "checked": feeds, "wrong": wrong }))
    });
}

/// `resource` as it is now: subscribed, read and let go (a window built once from it).
pub fn snapshot(resource: &str) -> Vec<Value> {
    let client = loom_core::api::hub().connect(loom_core::api::Principal::Window);
    let Ok(sub) = client.subscribe(resource, || {}) else { return Vec::new() };
    let mut mirror = Mirror::default();
    for message in sub.drain() {
        mirror.apply(&message);
    }
    mirror.items
}

/// An agent as the sidebar draws it (the `agents` resource's item).
pub struct SideAgent {
    pub id: u32,
    pub title: String,
    pub project: PathBuf,
    pub project_name: String,
    pub status: Status,
    pub unseen: bool,
    /// When it was last used (Unix seconds; 0: never).
    pub used: u64,
    pub place: String,
    pub edited: usize,
    pub awake: bool,
}

impl SideAgent {
    pub fn from(item: &Value) -> Option<SideAgent> {
        Some(SideAgent {
            id: item["id"].as_str()?.parse().ok()?,
            title: item["title"].as_str().unwrap_or_default().to_owned(),
            project: PathBuf::from(item["project_id"].as_str()?),
            project_name: item["project"].as_str().unwrap_or_default().to_owned(),
            status: Status::from_css(item["status"].as_str().unwrap_or_default()),
            unseen: item["unseen"].as_bool().unwrap_or(false),
            used: loom_core::api::seconds(&item["used"]).map_or(0, |s| s.max(0) as u64),
            place: item["place"].as_str().unwrap_or_default().to_owned(),
            edited: item["edited"].as_u64().unwrap_or(0) as usize,
            awake: item["awake"].as_bool().unwrap_or(false),
        })
    }
}

/// A project as the sidebar draws it (the `projects` resource's item).
pub struct SideProject {
    pub path: PathBuf,
    pub name: String,
    pub collapsed: bool,
}

impl SideProject {
    pub fn from(item: &Value) -> Option<SideProject> {
        Some(SideProject {
            path: PathBuf::from(item["id"].as_str()?),
            name: item["name"].as_str().unwrap_or_default().to_owned(),
            collapsed: item["collapsed"].as_bool().unwrap_or(false),
        })
    }
}
