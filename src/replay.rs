//! Replay: checks the engine against real run logs. Each pair of consecutive
//! board reads is a transition; the engine tries every candidate move from
//! the first board and the move whose result reads closest to the second
//! board explains it, or shows what the engine gets wrong.

use crate::board::{Model, build_state};
use crate::engine::data::{Beast, RAINBOW, data};
use crate::engine::rules::weights;
use crate::engine::state::State;
use crate::engine::text::tier_name;
use crate::engine::{Action, apply};
use crate::solver::solve::{candidates, hide_future};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One `read` line of a run log.
#[derive(Debug, PartialEq)]
pub struct Read {
    pub at: String,
    pub model: Model,
}

/// The `read` lines of a log, in order, without a read whose model equals
/// the previous one kept. Lines that do not parse are skipped.
pub fn parse_log(text: &str) -> Vec<Read> {
    let mut out: Vec<Read> = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("kind").and_then(Value::as_str) != Some("read") {
            continue;
        }
        let Some(model) = v.get("model") else { continue };
        let model = Model::from(model.clone());
        if out.last().is_some_and(|r| r.model == model) {
            continue;
        }
        let at = v.get("at").and_then(Value::as_str).unwrap_or("").to_string();
        out.push(Read { at, model });
    }
    out
}

/// The `*.jsonl` run logs under each path (a file, or a directory searched
/// recursively).
pub fn log_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else if is_log(&p) {
                out.push(p);
            }
        }
    }
    fn is_log(p: &Path) -> bool {
        p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jsonl"))
    }
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, &mut out);
        } else {
            out.push(p.clone());
        }
    }
    out
}

/// One cell as the reader sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CellView {
    sealed: bool,
    inmate: Option<(Beast, i32)>,
    known: u8,
    /// The rarity behind a door the player knows something about.
    hidden: Option<i32>,
    scratch: bool,
    nest: bool,
    marker: bool,
}

/// What the reader sees of a state.
#[derive(Clone, Debug, PartialEq, Eq)]
struct View {
    day: u32,
    position: usize,
    vault: usize,
    tools: Vec<&'static str>,
    cells: Vec<CellView>,
}

fn view(st: &State) -> View {
    let mut tools: Vec<&'static str> = st.tools.iter().map(|t| t.key()).collect();
    tools.sort_unstable();
    let cells = st
        .cells
        .iter()
        .map(|c| CellView {
            sealed: c.sealed,
            inmate: c.inmate.filter(|_| !c.sealed).map(|i| (i.beast, i.tier)),
            known: c.known,
            hidden: c.hidden.filter(|_| c.known >= 1).map(|i| i.tier),
            scratch: c.scratch,
            nest: c.nest.is_some(),
            marker: c.event.is_some(),
        })
        .collect();
    View { day: st.day, position: st.position, vault: st.vault.len(), tools, cells }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Field {
    Sealed,
    Inmate,
    Beast,
    Tier,
    Known,
    Hidden,
    Scratch,
    Nest,
    Marker,
}

impl Field {
    fn name(self) -> &'static str {
        match self {
            Field::Sealed => "sealed",
            Field::Inmate => "inmate",
            Field::Beast => "beast",
            Field::Tier => "tier",
            Field::Known => "known",
            Field::Hidden => "hidden tier",
            Field::Scratch => "scratch",
            Field::Nest => "nest",
            Field::Marker => "marker",
        }
    }
}

/// Where the engine's result and the read board differ.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "what", rename_all = "lowercase")]
pub enum Mismatch {
    Cell { cell: usize, field: Field, engine: String, read: String },
    Day { engine: u32, read: u32 },
    Position { engine: usize, read: usize },
    Vault { engine: usize, read: usize },
    Tools { engine: Vec<&'static str>, read: Vec<&'static str> },
}

impl Mismatch {
    /// A short name to group mismatches by.
    pub fn kind(&self) -> String {
        match self {
            Mismatch::Cell { field, .. } => format!("cell {}", field.name()),
            Mismatch::Day { .. } => "day".to_string(),
            Mismatch::Position { .. } => "position".to_string(),
            Mismatch::Vault { .. } => "vault".to_string(),
            Mismatch::Tools { .. } => "tools".to_string(),
        }
    }
}

/// The mismatches between the engine's result and the read board, and the
/// rarities seen behind the doors the engine opened. `before` is the view of
/// the first board: a door it had sealed that both the engine and the read
/// show open matches whatever is inside, since door contents are random.
fn diff(before: &View, engine: &View, read: &View) -> (Vec<Mismatch>, Vec<i32>) {
    let mut out = Vec::new();
    let mut reveals = Vec::new();
    if engine.day != read.day {
        out.push(Mismatch::Day { engine: engine.day, read: read.day });
    }
    if engine.position != read.position {
        out.push(Mismatch::Position { engine: engine.position, read: read.position });
    }
    if engine.vault != read.vault {
        out.push(Mismatch::Vault { engine: engine.vault, read: read.vault });
    }
    if engine.tools != read.tools {
        out.push(Mismatch::Tools { engine: engine.tools.clone(), read: read.tools.clone() });
    }
    for (n, ((b, e), r)) in before.cells.iter().zip(&engine.cells).zip(&read.cells).enumerate() {
        let mut cell = |field: Field, engine: String, read: String| out.push(Mismatch::Cell { cell: n, field, engine, read });
        if e.sealed != r.sealed {
            cell(Field::Sealed, e.sealed.to_string(), r.sealed.to_string());
            continue;
        }
        if b.sealed && !e.sealed {
            if let Some((_, tier)) = r.inmate {
                reveals.push(tier);
            }
            continue;
        }
        match (e.inmate, r.inmate) {
            (Some((eb, et)), Some((rb, rt))) => {
                if eb != rb {
                    cell(Field::Beast, format!("{eb:?}"), format!("{rb:?}"));
                }
                if et != rt {
                    cell(Field::Tier, tier_name(et).to_string(), tier_name(rt).to_string());
                }
            }
            (None, None) => {}
            (ei, ri) => cell(Field::Inmate, format!("{ei:?}"), format!("{ri:?}")),
        }
        if e.known != r.known {
            cell(Field::Known, e.known.to_string(), r.known.to_string());
        }
        if e.hidden != r.hidden {
            cell(Field::Hidden, format!("{:?}", e.hidden), format!("{:?}", r.hidden));
        }
        if e.scratch != r.scratch {
            cell(Field::Scratch, e.scratch.to_string(), r.scratch.to_string());
        }
        if e.nest != r.nest {
            cell(Field::Nest, e.nest.to_string(), r.nest.to_string());
        }
        if e.marker != r.marker {
            cell(Field::Marker, e.marker.to_string(), r.marker.to_string());
        }
    }
    (out, reveals)
}

/// The candidate move whose result reads closest to the second board.
#[derive(Debug, PartialEq)]
pub struct Explanation {
    pub label: String,
    /// Whether the move, or the first move of a pair, is End.
    pub end: bool,
    pub mismatches: Vec<Mismatch>,
    pub reveals: Vec<i32>,
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// The two boards read the same: reader noise between identical boards.
    Unchanged,
    /// The first board offers no move.
    NoCandidates,
    Best(Explanation),
}

/// Tries every candidate from `a` on `seeds` hidden futures and keeps the
/// result with the fewest mismatches against `b` (ties: the first).
pub fn explain(a: &Model, b: &Model, seeds: u32) -> Verdict {
    let (st, _) = build_state(a, 1);
    let before = view(&st);
    let read = view(&build_state(b, 1).0);
    if before == read {
        return Verdict::Unchanged;
    }
    let mut best: Option<Explanation> = None;
    // Keeps `s` as the best when it reads closer to `read`; true once it matches.
    let consider = |best: &mut Option<Explanation>, s: &State, label: &dyn Fn() -> String, end: bool| {
        let (mismatches, reveals) = diff(&before, &view(s), &read);
        if best.as_ref().is_some_and(|b| mismatches.len() >= b.mismatches.len()) {
            return false;
        }
        let done = mismatches.is_empty();
        *best = Some(Explanation { label: label(), end, mismatches, reveals });
        done
    };
    'moves: for c in candidates(&st) {
        for seed in 0..seeds {
            let mut s = st.clone();
            hide_future(&mut s, seed);
            apply(&mut s, &c.action);
            if consider(&mut best, &s, &|| c.label.clone(), c.action == Action::End) {
                break 'moves;
            }
        }
    }
    if best.as_ref().is_some_and(|b| !b.mismatches.is_empty()) {
        // Reads can miss a move: try every pair of moves too.
        'pairs: for c in candidates(&st) {
            for seed in 0..seeds {
                let mut s = st.clone();
                hide_future(&mut s, seed);
                apply(&mut s, &c.action);
                for c2 in candidates(&s) {
                    let mut s2 = s.clone();
                    apply(&mut s2, &c2.action);
                    if consider(&mut best, &s2, &|| format!("{}, then {}", c.label, c2.label), c.action == Action::End) {
                        break 'pairs;
                    }
                }
            }
        }
    }
    best.map_or(Verdict::NoCandidates, Verdict::Best)
}

/// A transition no candidate explains.
#[derive(Debug, PartialEq, Serialize)]
pub struct Unexplained {
    pub file: String,
    pub a_at: String,
    pub b_at: String,
    /// None when the first board offers no move.
    pub label: Option<String>,
    pub mismatches: Vec<Mismatch>,
}

/// The tallies over every transition replayed.
#[derive(Default)]
pub struct Report {
    pub transitions: usize,
    /// A new run started, or a board read blank.
    pub skipped: usize,
    pub unchanged: usize,
    pub explained: usize,
    pub unexplained: Vec<Unexplained>,
    /// Rarities seen behind the doors the engine opened, Normal to Ultimate.
    pub doors: [u32; RAINBOW as usize],
    /// By the first board's step and whether the best move was End: the
    /// transitions, and the markers that appeared on cells without one.
    pub markers: BTreeMap<(String, bool), (u32, u32)>,
}

impl Report {
    /// Replays the reads of one log.
    pub fn add_log(&mut self, file: &str, reads: &[Read], seeds: u32) {
        let blank = Model::blank();
        for pair in reads.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            self.transitions += 1;
            if b.model.day < a.model.day || a.model == blank || b.model == blank {
                self.skipped += 1;
                continue;
            }
            let verdict = explain(&a.model, &b.model, seeds);
            let end = matches!(&verdict, Verdict::Best(e) if e.end);
            if verdict != Verdict::Unchanged {
                let spawned = a.model.cells.iter().zip(&b.model.cells).filter(|(ca, cb)| ca.event().is_none() && cb.event().is_some()).count();
                let row = self.markers.entry((format!("{:?}", a.model.step).to_lowercase(), end)).or_default();
                row.0 += 1;
                row.1 += spawned as u32;
            }
            let (label, mismatches) = match verdict {
                Verdict::Unchanged => {
                    self.unchanged += 1;
                    continue;
                }
                Verdict::NoCandidates => (None, Vec::new()),
                Verdict::Best(e) => {
                    if e.mismatches.is_empty() {
                        // An inmate may rise after its door opens.
                        for &t in &e.reveals {
                            if let Some(n) = self.doors.get_mut(t as usize) {
                                *n += 1;
                            }
                        }
                        self.explained += 1;
                        continue;
                    }
                    (Some(e.label), e.mismatches)
                }
            };
            self.unexplained.push(Unexplained { file: file.to_string(), a_at: a.at.clone(), b_at: b.at.clone(), label, mismatches });
        }
    }

    /// The summary for the terminal.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "transitions {}, skipped {}, unchanged {}, explained {}, unexplained {}",
            self.transitions,
            self.skipped,
            self.unchanged,
            self.explained,
            self.unexplained.len()
        );
        let mut kinds: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
        for u in &self.unexplained {
            let mut seen: Vec<String> = if u.label.is_none() { vec!["no candidates".to_string()] } else { u.mismatches.iter().map(Mismatch::kind).collect() };
            seen.sort_unstable();
            seen.dedup();
            for k in seen {
                let entry = kinds.entry(k).or_default();
                entry.0 += 1;
                if entry.1.len() < 3 {
                    entry.1.push(format!("{}:{}", u.file, u.a_at));
                }
            }
        }
        let mut kinds: Vec<_> = kinds.into_iter().collect();
        kinds.sort_by_key(|k| std::cmp::Reverse(k.1.0));
        let _ = writeln!(s, "\nunexplained by mismatch kind:");
        for (k, (n, examples)) in &kinds {
            let _ = writeln!(s, "  {k:<18} {n:>6}  {}", examples.join(" "));
        }
        let w = weights(&data().tier_weights);
        let total_w: f64 = w.iter().sum();
        let total: u32 = self.doors.iter().sum();
        let _ = writeln!(s, "\ndoor reveals ({total}):  count  observed  engine");
        for (t, &n) in self.doors.iter().enumerate() {
            let observed = if total == 0 { 0.0 } else { 100.0 * f64::from(n) / f64::from(total) };
            let _ = writeln!(s, "  {:<10} {n:>8} {observed:>8.1}% {:>6.1}%", tier_name(t as i32), 100.0 * w[t] / total_w);
        }
        let _ = writeln!(s, "\nnew markers by step: transitions  markers  per transition");
        for ((step, end), (n, m)) in &self.markers {
            let how = if *end { "end" } else { "other" };
            let _ = writeln!(s, "  {step:<8} {how:<6} {n:>8} {m:>8} {:>8.3}", f64::from(*m) / f64::from(*n));
        }
        s
    }

    /// One JSON line per unexplained transition.
    pub fn unexplained_lines(&self) -> String {
        self.unexplained.iter().map(|u| json!(u).to_string() + "\n").collect()
    }
}
