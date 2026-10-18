//! At every 8th decision of the strategy games of tests/decide.rs (games 0
//! to 19 of seed 1), runs the Rust solver and requires the instant pick, the
//! candidates and the options with their scores recorded in
//! tests/fixtures/strategy-solves.jsonl, so an accidental change of the
//! strategy or the playouts fails. After a deliberate change, re-record the
//! fixture with `WARDEN_RECORD=1 cargo test --profile fast-test --test decide --test solve`
//! and review its diff.

use serde_json::{Value, json};
use std::time::Instant;
use warden::engine::{Action, State, apply, new_game_with};
use warden::solver::TUNED;
use warden::solver::solve::{SolveOption, candidates, instant, solve};

mod common;
use common::{follow_js_repeat, no_cap, traces};

const SOLVES: &str = "strategy-solves.jsonl";

/// The traced JS game `seed` replayed to `step` from its traced moves.
fn replay(game: &Value, step: usize) -> State {
    let mut state = new_game_with(game["seed"].as_str().expect("seed"), no_cap());
    follow_js_repeat(&mut state, &game["start"]);
    for s in &game["steps"].as_array().expect("steps")[..step] {
        let action: Action = serde_json::from_value(s["action"].clone()).expect("action");
        apply(&mut state, &action);
        follow_js_repeat(&mut state, &s["state"]);
    }
    state
}

/// `x` as JSON; JSON has no infinity, so a non-finite score is a string.
fn score(x: f64) -> Value {
    if x.is_finite() { json!(x) } else { json!(x.to_string()) }
}

fn close(now: &Value, was: &Value) -> bool {
    match (now.as_f64(), was.as_f64()) {
        (Some(a), Some(b)) => (a - b).abs() <= 1e-9,
        _ => now == was,
    }
}

fn option(o: &SolveOption) -> Value {
    json!({ "label": o.label, "action": o.action, "n": o.n, "policy": o.policy, "mean": score(o.mean), "se": score(o.se), "gapSe": score(o.gap_se), "best": score(o.best) })
}

fn same_option(now: &Value, was: &Value) -> bool {
    ["label", "action", "n", "policy"].iter().all(|f| now[f] == was[f]) && ["mean", "se", "gapSe", "best"].iter().all(|f| close(&now[f], &was[f]))
}

/// The solver's view of the state before decision `step` of game `k`.
fn point(k: u32, step: usize, state: &State) -> Value {
    let pick = instant(state).map_or(Value::Null, |i| json!({ "action": i.action, "label": i.label, "why": i.why }));
    let labels: Vec<String> = candidates(state).into_iter().map(|c| c.label).collect();
    let options: Vec<Value> = solve(state, 30, 7, &TUNED).iter().map(option).collect();
    json!({ "k": k, "step": step, "instant": pick, "candidates": labels, "options": options })
}

#[test]
fn solves_as_recorded() {
    let mut points: Vec<Value> = Vec::new();
    for k in 0..20 {
        common::strategy_game(k, |step, state| {
            if step % 8 == 0 {
                points.push(point(k, step, state));
            }
        });
    }
    if common::recording() {
        common::record(SOLVES, &points);
        return;
    }

    let want = common::recorded(SOLVES);
    let mut mismatches: Vec<String> = Vec::new();
    if want.len() != points.len() {
        mismatches.push(format!("points: now {} / recorded {}", points.len(), want.len()));
    }
    // The first differing point of each game.
    let mut reported = Value::Null;
    for (now, was) in points.iter().zip(&want) {
        if now["k"] != reported
            && let Some(diff) = point_diff(now, was)
        {
            reported = now["k"].clone();
            mismatches.push(format!("game {} step {} {diff}", now["k"], now["step"]));
        }
    }

    println!("points: {}, mismatches: {}", points.len(), mismatches.len());
    for m in mismatches.iter().take(10) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty(), "{} mismatches with {SOLVES}", mismatches.len());
}

/// The first part of point `now` that differs from the recorded `was`, if any.
fn point_diff(now: &Value, was: &Value) -> Option<String> {
    if (&now["k"], &now["step"]) != (&was["k"], &was["step"]) {
        return Some(format!("point: recorded game {} step {}", was["k"], was["step"]));
    }
    if let Some(field) = ["instant", "candidates"].into_iter().find(|&f| now[f] != was[f]) {
        return Some(format!("{field}: now {} / recorded {}", now[field], was[field]));
    }
    let (a, b) = (now["options"].as_array().expect("options"), was["options"].as_array().expect("options"));
    if a.len() != b.len() {
        return Some(format!("options: now {} / recorded {}", a.len(), b.len()));
    }
    let (o, w) = a.iter().zip(b).find(|(o, w)| !same_option(o, w))?;
    Some(format!("option: now {o} / recorded {w}"))
}

#[test]
#[ignore]
fn solve_timing() {
    let games = traces();
    let game = &games[0];
    let step = game["steps"].as_array().expect("steps").len() / 2;
    let state = replay(game, step);
    let start = Instant::now();
    let options = solve(&state, 400, 1, &TUNED);
    let wall = start.elapsed();
    println!("{} step {step}: 400 rollouts in {:.3} s", game["seed"].as_str().expect("seed"), wall.as_secs_f64());
    println!("candidates: {}", options.len());
    if let Some(top) = options.first() {
        println!("top: {} (mean {:.3}, se {:.3}, n {})", top.label, top.mean, top.se, top.n);
    }
}
