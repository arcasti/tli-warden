//! Helpers shared by the tests: the recorded JS traces, the fixture boards,
//! JSON comparison of Rust and JS states, and a hit count for the reader
//! tests. Each test crate uses only some of them.
#![allow(dead_code)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use warden::board::{Model, build_state};
use warden::engine::State;

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

/// The game traces recorded from the earlier JS engine
/// (seeds TRACE-0001 to TRACE-0020), one JSON value per game, from
/// tests/fixtures/traces.jsonl.
pub fn traces() -> Vec<Value> {
    let text = std::fs::read_to_string(fixture("traces.jsonl")).expect("traces.jsonl");
    text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).expect("trace line")).collect()
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
