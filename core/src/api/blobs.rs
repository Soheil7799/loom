//! Large things (pictures) kept apart from the state (research/core-api §4): an entry names a
//! blob by id, and a client fetches it once (`blob {id}` → `{data}`, base64). Ids are opaque:
//! the same bytes always get the same one. Kept in memory up to `KEEP`, the oldest dropped
//! first (a client that asks for one gone gets `NotFound`, and draws a placeholder).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use serde_json::json;

use super::{Error, Principal, wire};

/// How many bytes of blobs are kept.
const KEEP: usize = 128 << 20;

type Blob = Arc<[u8]>;

thread_local! {
    static BLOBS: RefCell<VecDeque<(String, Blob)>> = RefCell::default();
}

/// Keeps `bytes`: their id.
pub fn put(bytes: Blob) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let id = format!("{:016x}-{}", hasher.finish(), bytes.len());
    BLOBS.with_borrow_mut(|blobs| {
        if blobs.iter().any(|(had, _)| *had == id) {
            return;
        }
        blobs.push_back((id.clone(), bytes));
        let mut total: usize = blobs.iter().map(|(_, b)| b.len()).sum();
        while total > KEEP && blobs.len() > 1 {
            if let Some((_, gone)) = blobs.pop_front() {
                total -= gone.len();
            }
        }
    });
    if !SERVED.with(|served| served.replace(true)) {
        super::hub().handle("blob", |who, params| {
            if matches!(who, Principal::Agent(_)) {
                return Err(Error::Forbidden("blobs".into()));
            }
            let id = params["id"].as_str().ok_or_else(|| Error::Invalid("which blob?".into()))?;
            let bytes = get(id).ok_or_else(|| Error::NotFound(format!("blob {id}")))?;
            Ok(json!({ "data": wire::base64(&bytes) }))
        });
    }
    id
}

thread_local! {
    static SERVED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The bytes kept as `id`, if they still are.
pub fn get(id: &str) -> Option<Blob> {
    BLOBS.with_borrow(|blobs| blobs.iter().find(|(had, _)| had == id).map(|(_, b)| b.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_bytes_are_kept_once_and_fetched_by_id() {
        let a = put(Blob::from(&b"picture"[..]));
        assert_eq!(put(Blob::from(&b"picture"[..])), a);
        assert_ne!(put(Blob::from(&b"another"[..])), a);
        assert_eq!(get(&a).as_deref(), Some(&b"picture"[..]));
        let window = super::super::hub().connect(Principal::Window);
        assert_eq!(window.command("b1", "blob", json!({ "id": a })), Ok(json!({ "data": "cGljdHVyZQ==" })));
        assert!(matches!(window.command("b2", "blob", json!({ "id": "nope" })), Err(Error::NotFound(_))));
    }
}
