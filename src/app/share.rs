//! Run log sharing: with the player's consent, each finished run's log.jsonl
//! goes to warden.arcasti.dev, gzipped, once. Window titles are replaced by
//! the window's size first; frames are never sent. A run folder gets an
//! `uploaded` file once the server has answered for good.

use super::http;
use super::runlog::is_run_folder;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

/// Where run logs go.
pub const ENDPOINT: &str = "https://warden.arcasti.dev/api/runs";

/// The file a run folder gets once its log is sent or refused for good.
const UPLOADED: &str = "uploaded";

/// A random install id, as a lowercase UUID: "0f8fad5b-d9cb-469f-a165-70867728950e".
pub fn new_install_id() -> Result<String, String> {
    let g = windows::core::GUID::new().map_err(|e| e.message())?;
    let tail: String = g.data4[2..].iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{tail}", g.data1, g.data2, g.data3, g.data4[0], g.data4[1]))
}

/// The log with each window line's `window` title replaced by `size`, the
/// [w, h] its "(W x H)" suffix gives (no size when it has none); other
/// lines as they are.
fn sanitize(log: &str) -> String {
    log.split_inclusive('\n')
        .map(|line| {
            let body = line.trim_end_matches(['\r', '\n']);
            if !body.contains("\"kind\":\"window\"") {
                return line.to_string();
            }
            let Ok(serde_json::Value::Object(mut event)) = serde_json::from_str(body) else { return line.to_string() };
            if event.get("kind").and_then(|k| k.as_str()) != Some("window") {
                return line.to_string();
            }
            let size = event.remove("window").and_then(|w| w.as_str().and_then(window_size));
            if let Some([w, h]) = size {
                event.insert("size".to_string(), serde_json::json!([w, h]));
            }
            format!("{}{}", serde_json::Value::Object(event), &line[body.len()..])
        })
        .collect()
}

/// The size in a window title's "(W x H)" suffix.
fn window_size(title: &str) -> Option<[u32; 2]> {
    let inner = title.trim_end().strip_suffix(')')?;
    let (w, h) = inner[inner.rfind('(')? + 1..].split_once(" x ")?;
    Some([w.trim().parse().ok()?, h.trim().parse().ok()?])
}

/// The run folders under `root` still to send, oldest first: not `current`,
/// named at or after `shared_from`, with no `uploaded` file and a log.jsonl
/// that holds a read.
fn pending(root: &Path, current: &Path, shared_from: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut runs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && e.file_name().to_str().is_some_and(|n| is_run_folder(n) && n >= shared_from))
        .map(|e| e.path())
        .filter(|dir| dir != current && !dir.join(UPLOADED).exists())
        .filter(|dir| std::fs::read_to_string(dir.join("log.jsonl")).is_ok_and(|log| log.contains("\"kind\":\"read\"")))
        .collect();
    runs.sort();
    runs
}

/// Gzips `bytes`.
fn gzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).and_then(|()| encoder.finish()).map_err(|e| e.to_string())
}

/// Sends the pending runs under `root` to `url`; the count sent. Stops at
/// the first 429, 5xx or failed request, so the rest waits for the next try.
pub fn upload_pending(url: &str, root: &Path, current: &Path, shared_from: &str, install_id: &str) -> Result<usize, String> {
    upload_with(root, current, shared_from, |run, body| {
        let headers = [("X-Warden-Version", env!("CARGO_PKG_VERSION")), ("X-Warden-Install", install_id), ("X-Warden-Run", run)];
        http::post(url, "application/gzip", &headers, &body)
    })
}

/// [`upload_pending`] with `post` sending a run's name and gzipped log. A
/// 2xx marks the run sent, and a 4xx other than 429 marks it too, with the
/// server's answer: the server refuses that log for good.
fn upload_with(root: &Path, current: &Path, shared_from: &str, mut post: impl FnMut(&str, Vec<u8>) -> Result<(u16, String), String>) -> Result<usize, String> {
    let mut sent = 0;
    for dir in pending(root, current, shared_from) {
        let run = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let log_path = dir.join("log.jsonl");
        let log = std::fs::read_to_string(&log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
        let (status, text) = post(&run, gzip(sanitize(&log).as_bytes())?)?;
        let marker = match status {
            200..=299 => {
                sent += 1;
                status.to_string()
            }
            429 => return Err(format!("{run}: HTTP 429, too many uploads")),
            400..=499 => format!("{status} {text}"),
            _ => return Err(format!("{run}: HTTP {status}")),
        };
        let marker_path = dir.join(UPLOADED);
        std::fs::write(&marker_path, marker).map_err(|e| format!("{}: {e}", marker_path.display()))?;
    }
    Ok(sent)
}

/// A sync: send the finished runs.
struct Request {
    /// The run folder on show, still being written.
    current: PathBuf,
    shared_from: String,
    install_id: String,
    /// Done once the run log has written all it was sent.
    flushed: Receiver<()>,
}

/// One thread that sends the pending runs on each request, one sync at a
/// time, so the window never waits on the network.
pub struct Uploader {
    tx: Sender<Request>,
    rx: Receiver<Result<usize, String>>,
}

impl Uploader {
    /// An uploader for the run folders under `root`; `done` runs after each sync.
    pub fn start(root: PathBuf, done: impl Fn() + Send + 'static) -> Uploader {
        let (tx, requests) = channel::<Request>();
        let (results, rx) = channel();
        std::thread::spawn(move || {
            while let Ok(mut request) = requests.recv() {
                // Requests that came meanwhile need one sync, with the latest.
                while let Ok(newer) = requests.try_recv() {
                    request = newer;
                }
                let _ = request.flushed.recv();
                let result = upload_pending(ENDPOINT, &root, &request.current, &request.shared_from, &request.install_id);
                let _ = results.send(result);
                done();
            }
        });
        Uploader { tx, rx }
    }

    /// Asks for a sync of the runs before `current`, once `flushed` (the run
    /// log's) is done.
    pub fn sync(&self, current: PathBuf, shared_from: String, install_id: String, flushed: Receiver<()>) {
        let _ = self.tx.send(Request { current, shared_from, install_id, flushed });
    }

    /// The last sync's outcome, once it comes: the count sent, or why it stopped.
    pub fn poll(&self) -> Option<Result<usize, String>> {
        self.rx.try_iter().last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the temp folder.
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("warden-share-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const READ: &str = "{\"at\":\"2026-10-20T16:13:30.114Z\",\"kind\":\"read\",\"model\":null}\n";

    /// A run folder `name` under `root` with `log` as its log.jsonl.
    fn run(root: &Path, name: &str, log: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("log.jsonl"), log).unwrap();
        dir
    }

    #[test]
    fn sanitize_window_lines() {
        let window = "{\"at\":\"2026-10-20T16:13:28.906Z\",\"kind\":\"window\",\"ocr\":\"en-US\",\"window\":\"\\\"Torchlight: Infinite\\\" (1920 x 1080)\"}\n";
        let odd = "{\"at\":\"2026-10-20T16:13:29.000Z\",\"kind\":\"window\",\"ocr\":\"en-US\",\"window\":\"Notepad\"}\r\n";
        let out = sanitize(&format!("{window}{READ}{odd}"));
        let lines: Vec<&str> = out.split_inclusive('\n').collect();
        assert_eq!(lines[0], "{\"at\":\"2026-10-20T16:13:28.906Z\",\"kind\":\"window\",\"ocr\":\"en-US\",\"size\":[1920,1080]}\n");
        assert_eq!(lines[1], READ);
        assert_eq!(lines[2], "{\"at\":\"2026-10-20T16:13:29.000Z\",\"kind\":\"window\",\"ocr\":\"en-US\"}\r\n");
        assert_eq!(window_size("A (2560 x 1440) (800 x 600)"), Some([800, 600]));
        assert_eq!((window_size("A (wide x 600)"), window_size("A 800 x 600")), (None, None));
    }

    #[test]
    fn pending_runs() {
        let root = temp("pending");
        let old = run(&root, "2026-10-18_01-00-00", READ);
        let a = run(&root, "2026-10-19_01-00-00", READ);
        let b = run(&root, "2026-10-19_02-00-00-2", READ);
        let sent = run(&root, "2026-10-19_03-00-00", READ);
        std::fs::write(sent.join(UPLOADED), "201").unwrap();
        run(&root, "2026-10-19_04-00-00", "{\"kind\":\"window\"}\n");
        run(&root, "notes", READ);
        let current = run(&root, "2026-10-19_05-00-00", READ);
        assert_eq!(pending(&root, &current, "2026-10-19_00-00-00"), vec![a.clone(), b.clone()]);
        assert_eq!(pending(&root, &current, ""), vec![old, a, b]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn upload_markers() {
        let root = temp("upload");
        let current = root.join("2026-10-20_00-00-00");
        let runs: Vec<PathBuf> = (1..=3).map(|n| run(&root, &format!("2026-10-19_0{n}-00-00"), READ)).collect();
        let marker = |dir: &Path| std::fs::read_to_string(dir.join(UPLOADED)).ok();
        // 201, then 400 (marked: refused for good), then a 429 leaves the third unmarked and stops.
        let mut answers = vec![Ok((201, "{\"ok\":true}".to_string())), Ok((400, "line 1: bad".to_string())), Ok((429, String::new()))].into_iter();
        let mut names = Vec::new();
        let result = upload_with(&root, &current, "", |name, body| {
            assert_eq!(&body[..2], &[0x1f, 0x8b]);
            names.push(name.to_string());
            answers.next().unwrap()
        });
        assert!(result.is_err());
        assert_eq!(names, ["2026-10-19_01-00-00", "2026-10-19_02-00-00", "2026-10-19_03-00-00"]);
        assert_eq!((marker(&runs[0]).as_deref(), marker(&runs[1]).as_deref(), marker(&runs[2])), (Some("201"), Some("400 line 1: bad"), None));
        // Only the third is left: a 5xx and a failed request leave it too.
        let result = upload_with(&root, &current, "", |_, _| Ok((503, String::new())));
        assert!(result.is_err() && marker(&runs[2]).is_none());
        let result = upload_with(&root, &current, "", |_, _| Err("no network".to_string()));
        assert_eq!(result, Err("no network".to_string()));
        assert!(marker(&runs[2]).is_none());
        assert_eq!(upload_with(&root, &current, "", |_, _| Ok((201, String::new()))), Ok(1));
        assert_eq!(marker(&runs[2]).as_deref(), Some("201"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The live server, for manual runs: `cargo test --release live_upload -- --ignored`,
    /// with `WARDEN_UPLOAD_URL` for another server, such as a local `wrangler dev`.
    /// Sends the largest run log in %APPDATA%\warden\runs as a test install's.
    #[test]
    #[ignore]
    fn live_upload() {
        let runs = crate::app::settings::app_dir().unwrap().join("runs");
        let largest = std::fs::read_dir(&runs)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|d| d.file_name().and_then(|n| n.to_str()).is_some_and(is_run_folder))
            .filter_map(|d| Some((std::fs::metadata(d.join("log.jsonl")).ok()?.len(), d)))
            .max()
            .unwrap()
            .1;
        let root = temp("live");
        let name = largest.file_name().unwrap().to_str().unwrap();
        let dir = run(&root, name, &std::fs::read_to_string(largest.join("log.jsonl")).unwrap());
        let url = std::env::var("WARDEN_UPLOAD_URL").unwrap_or_else(|_| ENDPOINT.to_string());
        let result = upload_pending(&url, &root, &root.join("none"), "", "00000000-0000-4000-8000-000000000001");
        let marker = std::fs::read_to_string(dir.join(UPLOADED));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(result, Ok(1), "marker: {marker:?}");
        assert_eq!(marker.unwrap(), "201");
    }
}
