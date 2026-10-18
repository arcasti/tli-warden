//! Helpers shared by the tests: the recorded JS traces, the strategy games
//! and their snapshot fixtures, the fixture boards, JSON comparison of Rust
//! and JS states, and a hit count for the reader tests. Each test crate uses
//! only some of them.
#![allow(dead_code)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use warden::board::{Model, build_state};
use warden::engine::state::{Params, Phase};
use warden::engine::{Action, State, apply};
use warden::solver::{TUNED, bench, decide};

/// The saved frames, trainingdata/s16 (git-ignored).
pub fn frames() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("trainingdata/s16")
}

/// The fixture board tests/boards/`name` as JSON.
pub fn raw(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("boards").join(name);
    serde_json::from_str(&std::fs::read_to_string(path).expect("board file")).expect("board JSON")
}

/// The state built from the fixture board `name` (seed 1) and its problems.
pub fn build(name: &str) -> (State, Vec<String>) {
    let model: Model = serde_json::from_value(raw(name)).expect("model");
    build_state(&model, 1)
}

/// The recorded fixture tests/fixtures/`name`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(name)
}

/// Params for replaying a JS trace: the JS engine had no vault cap.
pub fn no_cap() -> Params {
    Params { vault_cap: None, ..Params::default() }
}

/// Continues a replay from the traced JS state `js` where the JS rolled a repeated step and the
/// Rust engine rolled other dice. The JS engine rolled independent dice; the Rust engine draws
/// distinct steps, as the game does (docs/s16-screen.md). The dice check skips the later steps of
/// a day already copied.
pub fn follow_js_repeat(state: &mut State, js: &Value) {
    let dice: Vec<u32> = serde_json::from_value(js["dice"].clone()).expect("dice");
    let mut distinct = dice.clone();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() < dice.len() && state.dice != dice {
        *state = serde_json::from_value(js.clone()).expect("JS state");
    }
}

/// The game traces recorded from the earlier JS engine
/// (seeds TRACE-0001 to TRACE-0020), one JSON value per game, from
/// tests/fixtures/traces.jsonl.
pub fn traces() -> Vec<Value> {
    let text = std::fs::read_to_string(fixture("traces.jsonl")).expect("traces.jsonl");
    text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).expect("trace line")).collect()
}

/// Plays game `k` of seed 1 as `bench::play_strategy` does (its strategy
/// dice, default Params) and returns every action `decide` picked, in order,
/// with the final state; `at(i, state)` sees the state before decision `i`.
pub fn strategy_game(k: u32, mut at: impl FnMut(usize, &State)) -> (Vec<Action>, State) {
    let (mut st, mut rng) = bench::start(1, k);
    let mut picks = Vec::new();
    for _ in 0..2000 {
        if st.phase == Phase::Ended {
            break;
        }
        at(picks.len(), &st);
        let Some(action) = decide(&st, &TUNED, &mut rng) else { break };
        picks.push(action.clone());
        if !apply(&mut st, &action).is_ok() && !(action != Action::End && apply(&mut st, &Action::End).is_ok()) {
            break;
        }
    }
    (picks, st)
}

/// Whether the snapshot tests record their fixtures instead of comparing
/// (the env var `WARDEN_RECORD` is set).
pub fn recording() -> bool {
    std::env::var_os("WARDEN_RECORD").is_some()
}

/// Writes `lines` to tests/fixtures/`name`, one JSON value per line.
pub fn record(name: &str, lines: &[Value]) {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(fixture(name), text).expect("write fixture");
    println!("recorded {} lines to tests/fixtures/{name}", lines.len());
}

/// The lines of the recorded fixture tests/fixtures/`name`.
pub fn recorded(name: &str) -> Vec<Value> {
    let text = std::fs::read_to_string(fixture(name)).unwrap_or_else(|_| panic!("no {name}: record it with WARDEN_RECORD=1"));
    text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).expect("fixture line")).collect()
}

/// Reads that matched out of those checked, with the misses described.
#[derive(Default)]
pub struct Tally {
    pub ok: usize,
    pub total: usize,
    pub misses: Vec<String>,
}

impl Tally {
    pub fn check(&mut self, ok: bool, miss: impl FnOnce() -> String) {
        self.total += 1;
        if ok {
            self.ok += 1;
        } else {
            self.misses.push(miss());
        }
    }

    pub fn pct(&self) -> f64 {
        100.0 * self.ok as f64 / self.total.max(1) as f64
    }
}

/// JS numbers are all doubles: compare every number as f64.
pub fn normalize(v: Value) -> Value {
    match v {
        Value::Number(n) => serde_json::Number::from_f64(n.as_f64().expect("number")).map_or(Value::Null, Value::Number),
        Value::Array(a) => Value::Array(a.into_iter().map(normalize).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, normalize(v))).collect()),
        other => other,
    }
}

/// The first JSON path where `a` (Rust) and `b` (JS) differ, with both values.
pub fn first_diff(path: &str, a: &Value, b: &Value) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            keys.into_iter().find_map(|k| match (x.get(k), y.get(k)) {
                (Some(p), Some(q)) => first_diff(&format!("{path}.{k}"), p, q),
                (p, q) => Some(format!("{path}.{k}: rust {p:?} / js {q:?}")),
            })
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            x.iter().zip(y).enumerate().find_map(|(i, (p, q))| first_diff(&format!("{path}[{i}]"), p, q))
        }
        _ if a == b => None,
        _ => Some(format!("{path}: rust {a} / js {b}")),
    }
}
