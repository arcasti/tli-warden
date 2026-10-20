//! The run log: per run, a folder of JSON lines (log.jsonl) with what the
//! session read, advised and was told, and the frames read (frame-NNNN.jpg),
//! so a run can be reviewed later. A writer thread does the file work, so a
//! JPEG encode never stalls the window.

use crate::board::Model;
use crate::reader::Rgba;
use crate::reader::panels::Screen;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Run folders kept; older ones go when a folder starts.
const KEEP: usize = 10;

/// Reads that install nothing on one screen in a row save a frame this often,
/// so a board the reader misses still shows in the log.
const MISS_FRAME_EVERY: Duration = Duration::from_secs(60);

/// JPEG quality of the saved frames: a 1920 x 1080 frame takes about 420 KB,
/// against 2.3 MB as a PNG. The reader reads the captured frame, never these.
const FRAME_QUALITY: u8 = 92;

enum Job {
    /// A write into a run's folder.
    Write(PathBuf, Item),
    Flush(Sender<()>),
}

enum Item {
    Line(String),
    Jpeg(String, Rgba),
}

/// The log of the run on show. A folder is made with its first line, so an
/// app opened and closed without a read leaves none.
pub struct RunLog {
    root: PathBuf,
    /// The run's folder.
    dir: PathBuf,
    /// Frames saved in `dir`.
    frames: usize,
    /// The screen and board of the last frame saved, and when; no board for
    /// a read that installed none.
    last: Option<(Screen, Option<Model>, Instant)>,
    tx: Option<Sender<Job>>,
    writer: Option<JoinHandle<()>>,
}

impl RunLog {
    /// A log in a folder per run under `root`, the first one starting now.
    pub fn new(root: PathBuf) -> RunLog {
        let (tx, rx) = channel::<Job>();
        let keep_root = root.clone();
        let writer = std::thread::spawn(move || {
            let mut made: Option<PathBuf> = None;
            for job in rx {
                let (dir, item) = match job {
                    Job::Write(dir, item) => (dir, item),
                    Job::Flush(done) => {
                        let _ = done.send(());
                        continue;
                    }
                };
                if made.as_ref() != Some(&dir) {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = prune(&keep_root, KEEP);
                    made = Some(dir.clone());
                }
                // Errors are dropped: the log must not get in the player's way.
                let _ = match item {
                    Item::Line(line) => append(&dir.join("log.jsonl"), &line),
                    Item::Jpeg(name, frame) => save_jpeg(&dir.join(name), &frame),
                };
            }
        });
        let dir = root.join(folder_name(SystemTime::now()));
        RunLog { root, dir, frames: 0, last: None, tx: Some(tx), writer: Some(writer) }
    }

    /// A log under %APPDATA%\warden\runs; None when APPDATA is not set.
    pub fn in_appdata() -> Option<RunLog> {
        super::settings::app_dir().map(|d| RunLog::new(d.join("runs")))
    }

    /// The folder of the run on show.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Starts a new run's folder, named from now; a second one in the same
    /// second gets a suffix.
    pub fn new_run(&mut self) {
        let name = folder_name(SystemTime::now());
        let mut dir = self.root.join(&name);
        let mut n = 2;
        while dir == self.dir || dir.is_dir() {
            dir = self.root.join(format!("{name}-{n}"));
            n += 1;
        }
        self.dir = dir;
        self.frames = 0;
        self.last = None;
    }

    /// Writes one line: `at`, `kind`, then `fields` (a JSON object).
    pub fn event(&self, kind: &str, at: SystemTime, fields: Value) {
        let mut line = Map::new();
        line.insert("at".into(), Value::String(timestamp(at)));
        line.insert("kind".into(), Value::String(kind.into()));
        if let Value::Object(fields) = fields {
            line.extend(fields);
        }
        self.send(Job::Write(self.dir.clone(), Item::Line(Value::Object(line).to_string())));
    }

    /// Saves `frame` when its screen or board differs from the last frame
    /// saved, and gives the file's name. A frame without a board (a read that
    /// installed none) differs by its screen, or by [`MISS_FRAME_EVERY`]
    /// since the last frame saved.
    pub fn frame(&mut self, screen: Screen, model: Option<&Model>, frame: Rgba) -> Option<String> {
        if self.last.as_ref().is_some_and(|(s, m, at)| (*s, m.as_ref()) == (screen, model) && (model.is_some() || at.elapsed() < MISS_FRAME_EVERY)) {
            return None;
        }
        self.last = Some((screen, model.cloned(), Instant::now()));
        self.frames += 1;
        let name = frame_name(self.frames);
        self.send(Job::Write(self.dir.clone(), Item::Jpeg(name.clone(), frame)));
        Some(name)
    }

    /// Blocks until everything sent so far is written.
    pub fn wait(&self) {
        let _ = self.flushed().recv();
    }

    /// A receiver that gets a message, or is dropped, once everything sent
    /// so far is written.
    pub fn flushed(&self) -> Receiver<()> {
        let (done, rx) = channel();
        self.send(Job::Flush(done));
        rx
    }

    fn send(&self, job: Job) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(job);
        }
    }
}

impl Drop for RunLog {
    /// Finishes the writes sent so far.
    fn drop(&mut self) {
        self.tx = None;
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
    }
}

fn append(path: &Path, line: &str) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| e.to_string())
}

fn save_jpeg(path: &Path, img: &Rgba) -> Result<(), String> {
    let (w, h) = (u16::try_from(img.w).map_err(|e| e.to_string())?, u16::try_from(img.h).map_err(|e| e.to_string())?);
    let encoder = jpeg_encoder::Encoder::new_file(path, FRAME_QUALITY).map_err(|e| e.to_string())?;
    encoder.encode(&img.data, w, h, jpeg_encoder::ColorType::Rgba).map_err(|e| e.to_string())
}

/// "frame-0007.jpg".
fn frame_name(n: usize) -> String {
    format!("frame-{n:04}.jpg")
}

/// The UTC date and time of `t`: (year, month, day, hour, minute, second, millisecond).
fn civil(t: SystemTime) -> (i64, u32, u32, u32, u32, u32, u32) {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400) as u32);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, rem / 3600, rem / 60 % 60, rem % 60, d.subsec_millis())
}

/// A run folder's name, UTC, so names sort by time: "2026-10-18_01-23-45".
pub(super) fn folder_name(t: SystemTime) -> String {
    let (y, mo, d, h, mi, s, _) = civil(t);
    format!("{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{s:02}")
}

/// A line's time, UTC: "2026-10-18T01:23:45.678Z".
pub fn timestamp(t: SystemTime) -> String {
    let (y, mo, d, h, mi, s, ms) = civil(t);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z")
}

/// True for a name [`folder_name`] gives, with or without a suffix.
pub(super) fn is_run_folder(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() >= 19
        && b[..19].iter().enumerate().all(|(k, c)| match k {
            4 | 7 => *c == b'-',
            10 => *c == b'_',
            13 | 16 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// Deletes the run folders under `root` but the newest `keep`.
fn prune(root: &Path, keep: usize) -> std::io::Result<()> {
    let mut runs: Vec<PathBuf> = std::fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && e.file_name().to_str().is_some_and(is_run_folder))
        .map(|e| e.path())
        .collect();
    runs.sort();
    let old = runs.len().saturating_sub(keep);
    for dir in &runs[..old] {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("warden-runlog-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn names_sort_by_time() {
        let at = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
        assert_eq!(folder_name(at(0)), "1970-01-01_00-00-00");
        assert_eq!(folder_name(at(1_792_286_625)), "2026-10-18_01-23-45");
        assert_eq!(timestamp(at(1_792_286_625) + Duration::from_millis(7)), "2026-10-18T01:23:45.007Z");
        assert_eq!(folder_name(at(951_782_400)), "2000-02-29_00-00-00");
        let times = [9, 10, 59, 3599, 86_399, 86_400, 31_535_999, 31_536_000, 1_792_286_625];
        let names: Vec<String> = times.iter().map(|&s| folder_name(at(s))).collect();
        assert!(names.windows(2).all(|w| w[0] < w[1]), "{names:?}");
        assert!(names.iter().all(|n| is_run_folder(n)));
        assert!(!is_run_folder("notes") && is_run_folder("2026-10-18_01-23-45-2"));
    }

    #[test]
    fn prune_keeps_the_newest() {
        let root = temp("prune");
        for k in 0..12 {
            std::fs::create_dir(root.join(format!("2026-10-{:02}_00-00-00", k + 1))).unwrap();
        }
        std::fs::create_dir(root.join("mine")).unwrap();
        prune(&root, KEEP).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left.len(), KEEP + 1);
        assert_eq!(left[0], "2026-10-03_00-00-00");
        assert_eq!(left[KEEP - 1], "2026-10-12_00-00-00");
        assert_eq!(left[KEEP], "mine");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn frames_are_numbered_once_per_board() {
        let root = temp("frames");
        let mut log = RunLog::new(root.clone());
        let pixel = || Rgba { w: 1, h: 1, data: vec![0, 0, 0, 255] };
        let board = Model::blank();
        let mut other = board.clone();
        other.day += 1;
        assert_eq!(log.frame(Screen::Patrol, Some(&board), pixel()).as_deref(), Some("frame-0001.jpg"));
        assert_eq!(log.frame(Screen::Patrol, Some(&board), pixel()), None);
        assert_eq!(log.frame(Screen::Manage, Some(&board), pixel()).as_deref(), Some("frame-0002.jpg"));
        assert_eq!(log.frame(Screen::Manage, Some(&other), pixel()).as_deref(), Some("frame-0003.jpg"));
        // A frame without a board is saved once per screen.
        assert_eq!(log.frame(Screen::Unknown, None, pixel()).as_deref(), Some("frame-0004.jpg"));
        assert_eq!(log.frame(Screen::Unknown, None, pixel()), None);
        assert_eq!(log.frame(Screen::Popup, None, pixel()).as_deref(), Some("frame-0005.jpg"));
        log.event("read", SystemTime::now(), serde_json::json!({ "screen": "Patrol" }));
        log.wait();
        let first = log.dir().to_path_buf();
        assert!(first.join("frame-0005.jpg").is_file() && first.join("log.jsonl").is_file());

        // A new run numbers from 1 again, in another folder.
        log.new_run();
        assert_ne!(log.dir(), first);
        assert_eq!(log.frame(Screen::Manage, Some(&other), pixel()).as_deref(), Some("frame-0001.jpg"));
        drop(log);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
