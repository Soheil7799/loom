use super::*;
use serde_json::json;
use std::path::PathBuf;
use std::rc::Rc;

#[test]
fn principals_read_what_they_may() {
    let hub = Hub::manual(Limits::default());
    hub.store("agents");
    hub.store("conversation/a1");
    hub.store("conversation/a2");
    let agent = hub.connect(Principal::Agent("a1".into()));
    assert!(agent.subscribe("agents", || {}).is_ok());
    assert!(agent.subscribe("conversation/a1", || {}).is_ok());
    assert_eq!(agent.subscribe("conversation/a2", || {}).err(), Some(Error::NotFound("conversation/a2".into())));
    // what the user types or queues for agents is theirs: no agent or bridge reads it
    hub.store("inputs");
    hub.store("overview");
    assert!(agent.subscribe("inputs", || {}).is_err());
    assert!(agent.subscribe("overview", || {}).is_err());
    assert!(hub.connect(Principal::Bridge("telegram".into())).subscribe("inputs", || {}).is_err());
    assert!(hub.connect(Principal::Window).subscribe("inputs", || {}).is_ok());
    let window = hub.connect(Principal::Window);
    assert!(window.subscribe("conversation/a2", || {}).is_ok());
    assert_eq!(window.subscribe("weather", || {}).err(), Some(Error::NotFound("weather".into())));
    let phone = hub.connect(Principal::Bridge("phone".into()));
    assert!(phone.subscribe("conversation/a2", || {}).is_ok());
}

#[test]
fn subscriptions_are_numbered_per_connection_and_closing_a_resource_ends_them() {
    let hub = Hub::manual(Limits::default());
    hub.store("agents").upsert(json!({"id": "a1"}));
    let window = hub.connect(Principal::Window);
    let first = window.subscribe("agents", || {}).unwrap();
    let second = window.subscribe("agents", || {}).unwrap();
    assert_eq!((first.id(), second.id()), ("s1", "s2"));
    first.drain();
    hub.close("agents");
    assert_eq!(first.drain().last().map(|m| m.body.clone()), Some(Body::Gone));
    assert!(window.subscribe("agents", || {}).is_err());
}

#[test]
fn changes_go_out_once_the_core_is_idle() {
    let queued: Rc<RefCell<Vec<Task>>> = Rc::default();
    let q = queued.clone();
    crate::rt::set_host(move |f| q.borrow_mut().push(f), |_, _| {});
    let hub = Hub::new();
    let store = hub.store("agents");
    let woke = Rc::new(Cell::new(0));
    let w = woke.clone();
    let sub = hub.connect(Principal::Window).subscribe("agents", move || w.set(w.get() + 1)).unwrap();
    sub.drain();
    store.upsert(json!({"id": "a"}));
    store.upsert(json!({"id": "b"}));
    assert_eq!(queued.borrow().len(), 1, "one flush for changes made together");
    let flush = queued.borrow_mut().pop().unwrap();
    run(flush);
    assert_eq!(woke.get(), 1);
    assert_eq!(sub.drain().len(), 1);
}

type Task = std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>;

/// Runs a future that never waits.
fn run(f: Task) {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let mut f = f;
    assert!(f.as_mut().poll(&mut cx).is_ready());
}

/// A scenario's messages, as one subscriber gets them; and the items it ends with.
fn transcript(name: &str) -> (Vec<Message>, Vec<Value>) {
    let hub = Hub::manual(Limits { page: 2, queue: 6 });
    let store = hub.store("conversation/a1");
    let window = hub.connect(Principal::Window);
    let mut got = Vec::new();
    match name {
        // A conversation: the snapshot in pages, a reply streaming in, a question answered.
        "conversation" => {
            store.upsert(json!({"id": "e1", "kind": "user", "status": "done", "turn_id": "t1", "text": "hello"}));
            store.upsert(json!({"id": "e2", "kind": "agent", "status": "done", "turn_id": "t1", "text": "Hi."}));
            store.upsert(json!({"id": "e3", "kind": "user", "status": "done", "turn_id": "t2", "text": "list the files"}));
            let sub = window.subscribe("conversation/a1", || {}).unwrap();
            got.extend(sub.drain());
            store.upsert(json!({"id": "e4", "kind": "agent", "status": "streaming", "turn_id": "t2", "text": ""}));
            store.append("e4", "text", "There are ");
            store.flush();
            store.append("e4", "text", "three");
            store.append("e4", "text", " files.");
            store.flush();
            store.upsert(json!({"id": "e5", "kind": "question", "status": "awaiting_user", "turn_id": "t2", "text": "Delete them?"}));
            store.upsert(json!({"id": "e4", "kind": "agent", "status": "done", "turn_id": "t2", "text": "There are three files."}));
            store.flush();
            store.upsert(json!({"id": "e5", "kind": "question", "status": "done", "turn_id": "t2", "text": "Delete them?", "answer": "No"}));
            store.flush();
            got.extend(sub.drain());
        }
        // A subscriber that fell behind: its queue is dropped for a resync and a new snapshot.
        "resync" => {
            store.upsert(json!({"id": "e1", "text": "a"}));
            let sub = window.subscribe("conversation/a1", || {}).unwrap();
            got.extend(sub.drain());
            for n in 2..=8 {
                store.upsert(json!({"id": format!("e{n}"), "text": "x"}));
                store.flush();
            }
            got.extend(sub.drain());
            store.remove("e1");
            store.order(vec!["e8".into()]);
            store.flush();
            got.extend(sub.drain());
        }
        _ => unreachable!("no scenario {name}"),
    }
    (got, store.items())
}

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/api/golden").join(name)
}

/// The golden transcripts are the API's examples, for every client to replay: a change to
/// what goes on the wire fails here first. `LOOM_BLESS=1 cargo test` writes them anew.
#[test]
fn transcripts_match_the_golden_ones_and_replay_to_the_same_items() {
    for name in ["conversation", "resync"] {
        let (messages, items) = transcript(name);
        let lines: String = messages.iter().map(|m| serde_json::to_string(m).unwrap() + "\n").collect();
        let ends = serde_json::to_string_pretty(&items).unwrap() + "\n";
        let (file, end) = (golden(&format!("{name}.ndjson")), golden(&format!("{name}.items.json")));
        if std::env::var_os("LOOM_BLESS").is_some() {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, &lines).unwrap();
            std::fs::write(&end, &ends).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), lines, "{name}: run with LOOM_BLESS=1 if the change is meant");
        assert_eq!(std::fs::read_to_string(&end).unwrap(), ends);

        let mut mirror = Mirror::default();
        for line in lines.lines() {
            let m: Message = serde_json::from_str(line).unwrap();
            assert_ne!(mirror.apply(&m), Applied::Gap, "{name}: seq {}", m.seq);
        }
        assert!(mirror.synced);
        assert_eq!(mirror.items, items, "{name}: the replay ends where the core does");
    }
}

#[test]
fn a_mirror_notices_a_gap() {
    let mut mirror = Mirror::default();
    let at = |seq| Message { sub: "s1".into(), seq, body: Body::Synced };
    assert_eq!(mirror.apply(&at(1)), Applied::Changed);
    assert_eq!(mirror.apply(&at(3)), Applied::Gap);
    assert!(!mirror.synced);
    // Said once: what follows is dropped until the resync asked for, whose snapshot counts.
    assert_eq!(mirror.apply(&at(4)), Applied::Nothing);
    assert_eq!(mirror.apply(&at(9)), Applied::Nothing);
    let resync = Message { sub: "s1".into(), seq: 10, body: Body::Resync { reason: "asked".into() } };
    assert_eq!(mirror.apply(&resync), Applied::Nothing);
    let page = Message { sub: "s1".into(), seq: 11, body: Body::Page { items: vec![json!({ "id": "a" })] } };
    assert_eq!(mirror.apply(&page), Applied::Nothing);
    assert_eq!(mirror.apply(&at(12)), Applied::Changed);
    assert!(mirror.synced);
    assert_eq!(mirror.items, vec![json!({ "id": "a" })]);
}

#[test]
fn a_command_sent_twice_is_done_once_and_the_first_answer_wins() {
    let hub = Hub::manual(Limits::default());
    let answered: Rc<RefCell<Option<String>>> = Rc::default();
    let a = answered.clone();
    hub.handle("answer", move |who, params| {
        if matches!(who, Principal::Agent(_)) {
            return Err(Error::Forbidden("answering".into()));
        }
        let mut answered = a.borrow_mut();
        if answered.is_some() {
            return Err(Error::Conflict("answered already".into()));
        }
        *answered = Some(params["option"].as_str().unwrap_or_default().to_string());
        Ok(json!({}))
    });
    let window = hub.connect(Principal::Window);
    let phone = hub.connect(Principal::Bridge("phone".into()));
    let agent = hub.connect(Principal::Agent("a1".into()));
    assert_eq!(agent.command("c1", "answer", json!({"option": "allow"})), Err(Error::Forbidden("answering".into())));
    assert_eq!(phone.command("c1", "answer", json!({"option": "deny"})), Ok(json!({})));
    // The same command again (a retry after a lost reply): not done again, same result.
    assert_eq!(phone.command("c1", "answer", json!({"option": "deny"})), Ok(json!({})));
    assert_eq!(window.command("c1", "answer", json!({"option": "allow"})), Err(Error::Conflict("answered already".into())));
    assert_eq!(answered.borrow().as_deref(), Some("deny"));
    assert_eq!(window.command("c2", "fly", json!({})), Err(Error::NotFound("command fly".into())));
    assert_eq!(window.command("c3", "answer", json!([1])), Err(Error::Invalid("params must be an object".into())));
    assert!(window.command("", "answer", json!({})).is_err());
}

#[test]
fn a_resource_made_when_asked_goes_once_nobody_follows_it() {
    let hub = Hub::manual(Limits::default());
    hub.provide("note/", |store, path| {
        store.upsert(json!({ "id": "note", "path": path }));
        true
    });
    let window = hub.connect(Principal::Window);
    let first = window.subscribe("note/a.md", || {}).unwrap();
    let kept = window.subscribe("note/b.md", || {}).unwrap();
    drop(first);
    // Another made: the one nobody follows goes, the followed one stays.
    let _third = window.subscribe("note/c.md", || {}).unwrap();
    assert!(hub.existing("note/a.md").is_none());
    assert!(hub.existing("note/b.md").is_some());
    // Asked for again: made again.
    assert!(window.subscribe("note/a.md", || {}).is_ok());
    drop(kept);
}
