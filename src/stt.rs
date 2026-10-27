//! Speech to text, through an endpoint the user sets (Settings → Advanced): the OpenAI-style
//! `/audio/transcriptions` that Groq, OpenAI and local Whisper servers (speaches, LocalAI,
//! whisper.cpp's server) answer. Loom bundles no model: the user's choice of service, or their
//! own server. Used by the composer's microphone and for Telegram voice messages.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gtk::{gio, glib};
use serde_json::Value;

use crate::settings;

/// Switched on, with somewhere to send the audio.
pub fn configured() -> bool {
    let s = settings::get();
    s.stt_enabled && !s.stt_url.trim().is_empty()
}

/// `audio` (a file named `name`, of type `mime`) as text, to `done` on the main loop.
pub fn transcribe(audio: Vec<u8>, name: &str, mime: &str, done: impl FnOnce(Result<String, String>) + 'static) {
    let s = settings::get();
    let (url, key, model, language) = (s.stt_url.trim().to_owned(), s.stt_key.trim().to_owned(), s.stt_model.trim().to_owned(), s.stt_language.trim().to_owned());
    let (name, mime) = (name.to_owned(), mime.to_owned());
    let size = audio.len();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || request(&url, &key, &model, &language, &audio, &name, &mime))
            .await
            .unwrap_or_else(|_| Err("the request stopped".to_owned()));
        crate::log::debug!("stt", "{size} bytes → {:?}", result.as_ref().map(|t| t.chars().count()));
        done(result);
    });
}

/// The endpoint: the address as given if it names the transcriptions path, else that path
/// under it ("https://api.groq.com/openai/v1" → …/v1/audio/transcriptions).
fn endpoint(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    if url.ends_with("/transcriptions") { url.to_owned() } else { format!("{url}/audio/transcriptions") }
}

fn request(url: &str, key: &str, model: &str, language: &str, audio: &[u8], name: &str, mime: &str) -> Result<String, String> {
    if url.is_empty() {
        return Err("no speech-to-text address in the settings".to_owned());
    }
    let boundary = format!("loom-{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    let mut body = Vec::with_capacity(audio.len() + 1024);
    let mut field = |name: &str, value: &str| {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    };
    // None given: none sent (a local Whisper server picks its own; Groq and OpenAI say they
    // need one).
    if !model.is_empty() {
        field("model", model);
    }
    field("response_format", "json");
    if !language.is_empty() {
        field("language", language);
    }
    // A name with quotes or line breaks would break the form.
    let name: String = name.chars().map(|c| if c == '"' || c == '\\' || c.is_control() { '_' } else { c }).collect();
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
    body.extend_from_slice(audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let client: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(120))).http_status_as_error(false).build().into();
    let mut call = client.post(&endpoint(url)).header("content-type", &format!("multipart/form-data; boundary={boundary}"));
    if !key.is_empty() {
        call = call.header("authorization", &format!("Bearer {key}"));
    }
    let hide = |e: String| if key.is_empty() { e } else { e.replace(key, "…") };
    let mut response = call.send(&body[..]).map_err(|e| hide(format!("can't reach it: {e}")))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().map_err(|e| hide(e.to_string()))?;
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status != 200 {
        let why = value["error"]["message"].as_str().or_else(|| value["error"].as_str()).or_else(|| value["detail"].as_str()).map(str::to_owned);
        return Err(format!("it answered {status}: {}", why.unwrap_or_else(|| text.chars().take(200).collect())));
    }
    value["text"].as_str().map(|t| t.trim().to_owned()).ok_or_else(|| "it answered without text".to_owned())
}

/// A second of silence as WAV (16 kHz mono): for the settings' Test.
pub fn silence() -> Vec<u8> {
    let samples = 16_000u32;
    let data = samples * 2;
    let mut wav = Vec::with_capacity(44 + data as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    wav.resize(44 + data as usize, 0);
    wav
}

/// The microphone, recording to a file until finished.
pub struct Recording {
    child: Child,
    path: PathBuf,
}

/// Recordings made so far (each to a file of its own).
static TAKES: AtomicU32 = AtomicU32::new(0);

/// Starts recording from the default microphone (PipeWire, else ALSA): 16 kHz mono WAV, to
/// a file of its own. The recorder dies with loom.
pub fn record() -> Result<Recording, String> {
    let dir = crate::state::dir().join("voice");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // What a loom that ended mid-recording left behind: another process's, over an hour old
    // (another loom recording now keeps its file).
    let me = format!("{}-", std::process::id());
    let hour = Duration::from_secs(3600);
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > hour);
        if !entry.file_name().to_string_lossy().starts_with(&me) && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let path = dir.join(format!("{me}{}.wav", TAKES.fetch_add(1, Ordering::SeqCst)));
    let file = path.to_string_lossy().into_owned();
    let tries: [(&str, Vec<&str>); 2] = [
        ("pw-record", vec!["--rate", "16000", "--channels", "1", "--format", "s16", &file]),
        ("arecord", vec!["-q", "-f", "S16_LE", "-r", "16000", "-c", "1", &file]),
    ];
    for (program, args) in tries {
        if glib::find_program_in_path(program).is_none() {
            continue;
        }
        let mut command = Command::new(program);
        command.args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        // SAFETY: prctl only, between fork and exec: loom gone, the recorder goes too.
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        let child = command.spawn();
        if let Ok(child) = child {
            return Ok(Recording { child, path });
        }
    }
    Err("no recorder found (pw-record or arecord)".to_owned())
}

impl Recording {
    /// Stops it and gives the WAV (the recorder writes its header when told to stop). Blocks
    /// up to 3 s: run off the main loop.
    pub fn finish(mut self) -> Result<Vec<u8>, String> {
        // SAFETY: plain kill(2) on our own child, not yet waited for (its pid is not reused).
        unsafe { libc::kill(self.child.id() as i32, libc::SIGINT) };
        let until = Instant::now() + Duration::from_secs(3);
        while matches!(self.child.try_wait(), Ok(None)) {
            if Instant::now() > until {
                // Stuck (a device gone): stopped hard; what was written may still read.
                let _ = self.child.kill();
                let _ = self.child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let audio = std::fs::read(&self.path).map_err(|e| format!("the recording could not be read: {e}"))?;
        let _ = std::fs::remove_file(&self.path);
        if audio.len() <= 44 {
            return Err("nothing was recorded (is a microphone connected?)".to_owned());
        }
        Ok(audio)
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        // Dropped without finishing (the window closed): the recorder stops too.
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        // Not kept: what was said goes nowhere unless sent.
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_endpoint_is_found_under_a_base_address() {
        assert_eq!(super::endpoint("https://api.groq.com/openai/v1/"), "https://api.groq.com/openai/v1/audio/transcriptions");
        assert_eq!(super::endpoint("http://127.0.0.1:8000/v1/audio/transcriptions"), "http://127.0.0.1:8000/v1/audio/transcriptions");
    }

    #[test]
    fn a_second_of_silence_is_a_wav() {
        let wav = super::silence();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 32_000);
    }
}
