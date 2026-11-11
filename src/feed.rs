//! The window's copies of the core API's resources it draws (warp): subscribed once, kept up
//! to date by applying what comes; a gap asks for a fresh snapshot.

use std::cell::{Ref, RefCell};
use std::rc::Rc;
use std::path::PathBuf;

use loom_core::agent::Status;
use loom_core::api::{Applied, Mirror};

use crate::conn::Sub;
use serde_json::Value;

pub struct Feed {
    sub: Rc<Sub>,
    mirror: Rc<RefCell<Mirror>>,
}

impl Feed {
    /// Follows `resource`; `ready` is called when something new waits (call `update` then).
    pub fn new(resource: &str, ready: impl Fn() + 'static) -> Option<Feed> {
        let sub = Rc::new(crate::conn::subscribe(resource, ready)?);
        let feed = Feed { sub, mirror: Rc::default() };
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
            // What follows a gap is dropped by the copy until the snapshot asked for comes.
            let mut gap = false;
            {
                let mut mirror = self.mirror.borrow_mut();
                // Every one applied (not `any`: the copy takes each message's number).
                for m in &messages {
                    gap |= mirror.apply(m) == Applied::Gap;
                }
            }
            // Asked with the copy let go: asking may call back into `update`.
            if gap {
                self.sub.resync();
            }
        }
    }

    /// Whether its copy is whole (a snapshot came in full, no resync since).
    pub fn synced(&self) -> bool {
        self.mirror.borrow().synced
    }

    pub fn items(&self) -> Ref<'_, Vec<Value>> {
        Ref::map(self.mirror.borrow(), |m| &m.items)
    }
}

/// An agent as the sidebar draws it (the `agents` resource's item).
#[derive(Clone, PartialEq)]
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
    /// Why its last turn failed (empty: it didn't).
    pub failed: String,
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
            failed: item["failed"]["why"].as_str().unwrap_or_default().to_owned(),
        })
    }
}

/// A project as the sidebar draws it (the `projects` resource's item).
#[derive(Clone, PartialEq)]
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
