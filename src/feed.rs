//! The window's copies of the core API's resources it draws (warp): subscribed once, kept up
//! to date by applying what comes; a gap asks for a fresh snapshot.

use std::cell::{Ref, RefCell};
use std::path::PathBuf;

use loom_core::agent::Status;
use loom_core::api::{Applied, Client, Mirror, Subscription};
use serde_json::Value;

pub struct Feed {
    sub: Subscription,
    mirror: RefCell<Mirror>,
}

impl Feed {
    /// Follows `resource`; `ready` is called when something new waits (call `update` then).
    pub fn new(client: &Client, resource: &str, ready: impl Fn() + 'static) -> Option<Feed> {
        let sub = client.subscribe(resource, ready).ok()?;
        let feed = Feed { sub, mirror: RefCell::default() };
        feed.update();
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
