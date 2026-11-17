//! Meaning search, optional (off by default): vectors from a local embedding model served by
//! Ollama, for loom_find (the code) and the note search. A question finds what it means, not
//! only what shares its words (on loom's, a Go/TSX and a Python repo, embeddinggemma found the
//! right definition first about twice as often as words did).
//!
//! Each piece's vector is kept on disk by model and by its text's hash, so it is embedded
//! once whichever agent's process asks; until all of a project's or vault's pieces have one,
//! a search uses words alone and says how far the indexing is. loomd starts what it needs
//! (`ollama serve` when nothing answers on this machine, the model pulled when missing) and
//! says how it is in the `embeddings` resource.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

/// Pieces sent to the model at once.
const BATCH: usize = 32;

/// Where the model is and which: what loomd's settings say (`embed_info` for an agent's tools).
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub url: String,
    pub name: String,
}

impl Model {
    pub fn from_settings(s: &crate::settings::Settings) -> Model {
        Model { url: s.embed_url.trim_end_matches('/').to_owned(), name: s.embed_model.clone() }
    }

    /// `text` as the model wants it said: each model was trained with its own wording for a
    /// question and for what is searched.
    fn worded(&self, text: &str, query: bool) -> String {
        if self.name.starts_with("embeddinggemma") {
            if query { format!("task: code retrieval | query: {text}") } else { format!("title: none | text: {text}") }
        } else if self.name.starts_with("nomic-embed") {
            format!("{}{text}", if query { "search_query: " } else { "search_document: " })
        } else if self.name.starts_with("qwen3-embedding") && query {
            format!("Instruct: Given a question, find the code or note that answers it\nQuery: {text}")
        } else {
            text.to_owned()
        }
    }

    /// Unit vectors for `texts` (questions when `query`), in their order.
    pub fn embed(&self, texts: &[String], query: bool) -> Result<Vec<Vec<f32>>, String> {
        let client = client(300);
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(BATCH) {
            let input: Vec<String> = chunk.iter().map(|t| self.worded(t, query)).collect();
            let answer = call(&client, &format!("{}/api/embed", self.url), Some(json!({ "model": self.name, "input": input, "truncate": true })))?;
            let vectors = answer["embeddings"].as_array().ok_or_else(|| format!("{} answered without vectors", self.name))?;
            for v in vectors {
                let mut v: Vec<f32> = v.as_array().into_iter().flatten().filter_map(|x| x.as_f64()).map(|x| x as f32).collect();
                let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(f32::EPSILON);
                v.iter_mut().for_each(|x| *x /= norm);
                out.push(v);
            }
        }
        if out.len() != texts.len() {
            return Err(format!("{} gave {} vectors for {} pieces", self.name, out.len(), texts.len()));
        }
        Ok(out)
    }
}

fn client(seconds: u64) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(seconds))).http_status_as_error(false).build().into()
}

/// A GET (no body) or POST to Ollama, its JSON answer.
fn call(client: &ureq::Agent, url: &str, body: Option<Value>) -> Result<Value, String> {
    let sent = match body {
        Some(body) => client.post(url).header("content-type", "application/json").send(body.to_string()),
        None => client.get(url).call(),
    };
    let mut response = sent.map_err(|e| format!("can't reach {url}: {e}"))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status >= 400 {
        return Err(value["error"].as_str().map(str::to_owned).unwrap_or_else(|| format!("{url} answered {status}")));
    }
    Ok(value)
}

/// A text's key in the vectors kept (FNV-1a: the same in every process and release).
pub fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// The vectors one model gave, kept in `~/.cache/loom/embeddings/<model>.vec`: records of
/// hash (u64), length (u32) and the floats, all little-endian, appended by any process (one
/// write each) and read by all. Only where each vector is (16 bytes a vector) is held in
/// memory: a vector is read from the file when a search needs it (the file runs to hundreds of
/// MB, and each agent's `loom mcp` and loomd would otherwise hold all of it).
pub struct Vectors {
    path: PathBuf,
    /// Each hash's vector: where its floats start in the file, and how many.
    at: HashMap<u64, (u64, u32)>,
    file: Option<std::fs::File>,
    /// How far the file is read (what other processes append after is read at the next look).
    read_to: u64,
}

impl Vectors {
    pub fn open(model: &str) -> Vectors {
        let file = model.replace(['/', ':'], "-");
        let dir = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".cache"));
        Vectors::at(dir.join("loom").join("embeddings").join(format!("{file}.vec")))
    }

    fn at(path: PathBuf) -> Vectors {
        let mut vectors = Vectors { path, at: HashMap::new(), file: None, read_to: 0 };
        vectors.reload();
        vectors
    }

    /// Where the vectors appended since the last look are (a record still being written
    /// waits): only their headers are read, the floats skipped.
    pub fn reload(&mut self) {
        if self.file.is_none() {
            self.file = std::fs::File::open(&self.path).ok();
        }
        let Some(file) = &self.file else { return };
        let Ok(length) = file.metadata().map(|m| m.len()) else { return };
        let mut reader = std::io::BufReader::with_capacity(1 << 16, file);
        if reader.seek(SeekFrom::Start(self.read_to)).is_err() {
            return;
        }
        let mut header = [0u8; 12];
        while length.saturating_sub(self.read_to) >= 12 && reader.read_exact(&mut header).is_ok() {
            let hash = u64::from_le_bytes(header[..8].try_into().unwrap_or_default());
            let n = u32::from_le_bytes(header[8..].try_into().unwrap_or_default());
            let floats = self.read_to + 12;
            if floats + u64::from(n) * 4 > length {
                break;
            }
            self.at.insert(hash, (floats, n));
            self.read_to = floats + u64::from(n) * 4;
            if reader.seek_relative(i64::from(n) * 4).is_err() {
                break;
            }
        }
    }

    pub fn has(&self, hash: u64) -> bool {
        self.at.contains_key(&hash)
    }

    /// The vector of `hash`, read from the file.
    pub fn get(&self, hash: u64) -> Option<Vec<f32>> {
        use std::os::unix::fs::FileExt;
        let (offset, n) = *self.at.get(&hash)?;
        let mut bytes = vec![0u8; n as usize * 4];
        self.file.as_ref()?.read_exact_at(&mut bytes, offset).ok()?;
        Some(bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect())
    }

    /// New vectors kept, for every process.
    pub fn add(&mut self, items: Vec<(u64, Vec<f32>)>) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut bytes = Vec::new();
        for (hash, v) in &items {
            bytes.extend(hash.to_le_bytes());
            bytes.extend((v.len() as u32).to_le_bytes());
            v.iter().for_each(|x| bytes.extend(x.to_le_bytes()));
        }
        // shortcut: two processes appending at once rely on one O_APPEND write staying whole
        // (true for local files on Linux); a lost record is only embedded again.
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        self.reload();
        Ok(())
    }
}

/// One process's meaning search over one set of pieces (a project's code, a vault): vectors
/// for what lacks one are made in the background; a question is answered once all have one.
pub struct Meaning {
    model: Model,
    vectors: Arc<Mutex<Vectors>>,
    busy: Arc<AtomicBool>,
    failed: Arc<Mutex<Option<String>>>,
}

impl Meaning {
    pub fn new(model: Model) -> Meaning {
        let vectors = Arc::new(Mutex::new(Vectors::open(&model.name)));
        Meaning { model, vectors, busy: Arc::default(), failed: Arc::default() }
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    /// Each piece's likeness to `query` (cosine, -1 to 1), in the pieces' order; Err says why
    /// not yet (indexing, so many done) or not at all. Starts the indexing when pieces lack
    /// a vector.
    pub fn scores(&self, pieces: &[(u64, String)], query: &str) -> Result<Vec<f32>, String> {
        let missing: Vec<(u64, String)> = {
            let mut vectors = self.vectors.lock().unwrap_or_else(|e| e.into_inner());
            vectors.reload();
            pieces.iter().filter(|(h, _)| !vectors.has(*h)).cloned().collect()
        };
        if !missing.is_empty() {
            if let Some(why) = self.failed.lock().unwrap_or_else(|e| e.into_inner()).take() {
                return Err(format!("meaning search failed ({why}); words alone this time, it tries again at the next search"));
            }
            let total = missing.len();
            if !self.busy.swap(true, Ordering::SeqCst) {
                let (model, vectors, busy, failed) = (self.model.clone(), self.vectors.clone(), self.busy.clone(), self.failed.clone());
                std::thread::spawn(move || {
                    for chunk in missing.chunks(BATCH * 4) {
                        let texts: Vec<String> = chunk.iter().map(|(_, t)| t.clone()).collect();
                        let made = model.embed(&texts, false).and_then(|vs| vectors.lock().unwrap_or_else(|e| e.into_inner()).add(chunk.iter().map(|(h, _)| *h).zip(vs).collect()));
                        if let Err(why) = made {
                            *failed.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
                            break;
                        }
                    }
                    busy.store(false, Ordering::SeqCst);
                });
            }
            return Err(format!("meaning search is indexing: {total} pieces left to embed (once); words alone until then"));
        }
        let q = self.model.embed(&[query.to_owned()], true)?.remove(0);
        let vectors = self.vectors.lock().unwrap_or_else(|e| e.into_inner());
        Ok(pieces.iter().map(|(h, _)| vectors.get(*h).map_or(-1.0, |v| v.iter().zip(&q).map(|(a, b)| a * b).sum())).collect())
    }
}

/// Scores by meaning for `pieces`, from the search `held` (made anew for another model): Err
/// says why words alone answer this time.
pub fn scores_in(held: &Mutex<Option<Meaning>>, model: Model, pieces: &[(u64, String)], query: &str) -> Result<Vec<f32>, String> {
    let mut held = held.lock().unwrap_or_else(|e| e.into_inner());
    if held.as_ref().is_none_or(|m| *m.model() != model) {
        *held = Some(Meaning::new(model));
    }
    held.as_ref().ok_or("no meaning search")?.scores(pieces, query)
}

/// The model for the notes when meaning search for them is on and ready (loomd's last check),
/// for Scribe, which works off the thread the settings are on.
static NOTES_READY: Mutex<Option<Model>> = Mutex::new(None);

pub fn notes_model() -> Option<Model> {
    NOTES_READY.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn notes_ready(model: Option<Model>) {
    *NOTES_READY.lock().unwrap_or_else(|e| e.into_inner()) = model;
}

/// The order `scores` put the pieces in, best first.
pub fn order(scores: &[f32]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..scores.len()).collect();
    order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
    order
}

/// How meaning search stands, for the `embeddings` resource: `state` (off, checking,
/// starting, pulling, ready, failed) and a sentence for people.
pub fn status(state: &str, detail: &str) -> Value {
    json!({ "id": "status", "state": state, "detail": detail })
}

/// Is `url` on this machine (where loom may start Ollama itself)?
fn local(url: &str) -> bool {
    let host = url.split("://").nth(1).unwrap_or(url).split(['/', ':']).next().unwrap_or("");
    matches!(host, "127.0.0.1" | "localhost" | "[" | "::1")
}

/// Gets meaning search ready (off the main thread: it waits on the network): Ollama started
/// when nothing answers on this machine, the model checked. Ok("pull"): it lacks the model.
pub fn ensure(model: &Model) -> Result<&'static str, String> {
    let quick = client(3);
    let version = format!("{}/api/version", model.url);
    if call(&quick, &version, None).is_err() {
        if !local(&model.url) {
            return Err(format!("nothing answers at {}", model.url));
        }
        let program = crate::paths::find_program("ollama").ok_or("Ollama isn't installed (on Arch: pacman -S ollama-cuda, or ollama without a GPU)")?;
        start_ollama(&program, &model.url)?;
        let answered = (0..30).any(|_| {
            std::thread::sleep(Duration::from_millis(500));
            call(&quick, &version, None).is_ok()
        });
        if !answered {
            return Err("Ollama was started but doesn't answer yet; its log: ~/.local/state/loom/ollama.log".into());
        }
    }
    let tags = call(&quick, &format!("{}/api/tags", model.url), None)?;
    let has = tags["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str()).any(|n| n == model.name || n.strip_suffix(":latest") == Some(&model.name));
    if !has {
        return Ok("pull");
    }
    model.embed(&["loom".to_owned()], true).map(|_| "ready")
}

/// The model downloaded (minutes: hundreds of MB).
pub fn pull(model: &Model) -> Result<(), String> {
    call(&client(3600), &format!("{}/api/pull", model.url), Some(json!({ "model": model.name, "stream": false })))
        .map(|_| ())
        .map_err(|why| format!("couldn't download {} (is that a model's name in Ollama's library?): {why}", model.name))
}

/// `ollama serve` in the background, answering where `url` says, its output in loom's state
/// folder. It ends with loomd (quit, killed or crashed): an Ollama loomd didn't start is never
/// touched.
fn start_ollama(program: &std::path::Path, url: &str) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let dir = crate::paths::home().join(".local/state/loom");
    let _ = std::fs::create_dir_all(&dir);
    let log = std::fs::File::create(dir.join("ollama.log")).map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(program);
    command
        .arg("serve")
        .env("OLLAMA_HOST", url.split("://").nth(1).unwrap_or(url).trim_end_matches('/'))
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    let loomd = libc::pid_t::try_from(std::process::id()).unwrap_or(0);
    // SAFETY: prctl and getppid only, between fork and exec.
    unsafe {
        command.pre_exec(move || {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            // loomd ended before the death signal was set: it would never come.
            if libc::getppid() != loomd {
                libc::_exit(0);
            }
            Ok(())
        });
    }
    // The death signal comes when the *thread* that started it ends: started from a thread
    // that lives as long as Ollama (waiting on it), not from a pool thread that may end first.
    let (sent, started) = std::sync::mpsc::channel();
    std::thread::spawn(move || match command.spawn() {
        Ok(mut child) => {
            let _ = sent.send(Ok(()));
            let _ = child.wait();
        }
        Err(e) => {
            let _ = sent.send(Err(format!("can't start Ollama: {e}")));
        }
    });
    started.recv().map_err(|e| e.to_string())?
}

/// loomd's part: the `embeddings` resource, `embeddings_check` (Settings' Refresh), and a
/// check at the start and whenever meaning search's settings change.
pub fn serve() {
    use crate::api::{Error, Principal};
    let hub = crate::api::hub();
    hub.store("embeddings").set(vec![status("off", "Off: searches go by words.")]);
    hub.handle_later("embeddings_check", |who, _, reply| {
        if !matches!(who, Principal::Window | Principal::Cli) {
            return reply(Err(Error::Forbidden("meaning search, but from loom's window or the CLI".into())));
        }
        check(Some(reply));
    });
    let key = |s: &crate::settings::Settings| (s.embed_code || s.embed_notes, Model::from_settings(s));
    let last = std::cell::RefCell::new(key(&crate::settings::get()));
    crate::settings::on_change(move |s| {
        if *last.borrow() != key(s) {
            *last.borrow_mut() = key(s);
            check(None);
        }
    });
    check(None);
}

fn publish(state: &str, detail: &str) -> Value {
    let item = status(state, detail);
    crate::api::hub().store("embeddings").upsert(item.clone());
    item
}

/// Meaning search made ready (Ollama started, the model pulled) and said, then `reply`.
fn check(reply: Option<crate::api::Reply>) {
    let s = crate::settings::get();
    let model = Model::from_settings(&s);
    notes_ready(None);
    if !(s.embed_code || s.embed_notes) {
        let item = publish("off", "Off: searches go by words.");
        if let Some(reply) = reply {
            reply(Ok(item));
        }
        return;
    }
    publish("checking", &format!("Looking for {} at {}…", model.name, model.url));
    crate::rt::spawn_local(async move {
        let asked = model.clone();
        let mut done = crate::rt::spawn_blocking(move || ensure(&asked)).await.unwrap_or_else(|_| Err("the check stopped".into()));
        if done == Ok("pull") {
            publish("pulling", &format!("Downloading {} (once: a few hundred MB)…", model.name));
            let asked = model.clone();
            done = crate::rt::spawn_blocking(move || pull(&asked).and_then(|_| ensure(&asked))).await.unwrap_or_else(|_| Err("the download stopped".into()));
        }
        let item = match done {
            Ok("ready") => {
                if crate::settings::get().embed_notes {
                    notes_ready(Some(model.clone()));
                }
                publish("ready", &format!("{} answers at {}: searches go by meaning.", model.name, model.url))
            }
            Ok(_) => publish("failed", &format!("Ollama has no model named {}, even after the download.", model.name)),
            Err(why) => publish("failed", &why),
        };
        if let Some(reply) = reply {
            reply(Ok(item));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_kept_by_one_process_are_read_by_another() {
        let path = std::env::temp_dir().join(format!("loom-vectors-{}.vec", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut one = Vectors::at(path.clone());
        let mut other = Vectors::at(path.clone());
        one.add(vec![(hash("fn a"), vec![0.6, 0.8]), (hash("fn b"), vec![1.0, 0.0])]).unwrap();
        assert_eq!(one.get(hash("fn a")), Some(vec![0.6, 0.8]));
        assert_eq!(other.get(hash("fn b")), None);
        other.reload();
        assert_eq!(other.get(hash("fn b")), Some(vec![1.0, 0.0]));
        // a record half written (another process mid-write) waits for the rest
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&hash("fn c").to_le_bytes()).unwrap();
        other.reload();
        assert_eq!(other.get(hash("fn c")), None);
        file.write_all(&1u32.to_le_bytes()).unwrap();
        file.write_all(&0.5f32.to_le_bytes()).unwrap();
        other.reload();
        assert_eq!(other.get(hash("fn c")), Some(vec![0.5]));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn each_model_is_asked_in_its_own_words_and_loom_starts_ollama_only_here() {
        let m = |name: &str| Model { url: String::new(), name: name.into() };
        assert_eq!(m("embeddinggemma").worded("x", true), "task: code retrieval | query: x");
        assert_eq!(m("embeddinggemma:latest").worded("x", false), "title: none | text: x");
        assert_eq!(m("nomic-embed-text").worded("x", true), "search_query: x");
        assert_eq!(m("other").worded("x", true), "x");
        assert!(local("http://127.0.0.1:11434") && local("http://localhost:11434/") && !local("http://192.168.1.5:11434"));
        assert_ne!(hash("a"), hash("b"));
    }
}
