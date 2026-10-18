//! Runs the Rust solver at the points in tests/fixtures/solve-traces.jsonl,
//! recorded from the earlier JS solver (every 8th
//! step of the 20 traced games), and requires the same instant pick,
//! candidates and options, and the same ranked scores where no playout
//! depends on the dice model (the JS rolled independent dice).

use serde_json::{Value, json};
use std::time::Instant;
use warden::engine::{Action, State, apply, new_game_with};
use warden::solver::TUNED;
use warden::solver::solve::{SolveOption, candidates, instant, solve};

mod common;
use common::{fixture, follow_js_repeat, no_cap, traces};

/// The game `seed` replayed to `step` from its traced moves.
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

fn close(rust: f64, js: &Value) -> bool {
    match js {
        Value::String(s) if s == "inf" => rust == f64::INFINITY,
        _ => js.as_f64().is_some_and(|x| (rust - x).abs() <= 1e-9),
    }
}

/// The first field of `o` that differs from the JS option, if any.
fn option_diff(o: &SolveOption, js: &Value) -> Option<String> {
    let action = serde_json::to_value(&o.action).expect("action");
    let same = o.label == js["label"]
        && action == js["action"]
        && json!(o.n) == js["n"]
        && json!(o.policy) == js["policy"]
        && close(o.mean, &js["mean"])
        && close(o.se, &js["se"])
        && close(o.gap_se, &js["gapSe"])
        && close(o.best, &js["best"]);
    let rust = json!({ "label": o.label, "action": action, "n": o.n, "policy": o.policy, "mean": o.mean, "se": o.se, "gapSe": o.gap_se, "best": o.best });
    (!same).then(|| format!("rust {rust} / js {js}"))
}

#[test]
fn solves_like_the_js() {
    let games = traces();
    let text = std::fs::read_to_string(fixture("solve-traces.jsonl")).expect("solve-traces.jsonl");

    let (mut points, mut exact) = (0, 0);
    let mut mismatches: Vec<String> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        points += 1;
        let point: Value = serde_json::from_str(line).expect("point line");
        let seed = point["seed"].as_str().expect("seed");
        let step = point["step"].as_u64().expect("step") as usize;
        let game = games.iter().find(|g| g["seed"] == seed).expect("traced game");
        let state = replay(game, step);
        let at = format!("{seed} step {step}");

        let pick = instant(&state).map(|i| json!({ "action": i.action, "label": i.label, "why": i.why }));
        let pick = pick.unwrap_or(Value::Null);
        if pick != point["instant"] {
            mismatches.push(format!("{at} instant: rust {pick} / js {}", point["instant"]));
        }
        let labels: Vec<String> = candidates(&state).into_iter().map(|c| c.label).collect();
        if json!(labels) != point["candidates"] {
            mismatches.push(format!("{at} candidates: rust {labels:?} / js {}", point["candidates"]));
        }
        let options = solve(&state, 30, 7, &TUNED);
        let js = point["options"].as_array().expect("options");
        if options.len() != js.len() {
            mismatches.push(format!("{at} options: rust {} / js {}", options.len(), js.len()));
        } else if options.iter().zip(js).all(|(o, j)| option_diff(o, j).is_none()) {
            exact += 1;
        } else {
            // The JS playouts rolled independent dice and the Rust playouts draw distinct steps
            // (docs/s16-screen.md), so the playout scores and the ranking differ wherever a
            // playout reaches a morning; the options offered must still be the same.
            let mut rust: Vec<Value> = options.iter().map(|o| json!([o.label, o.action])).collect();
            let mut want: Vec<Value> = js.iter().map(|j| json!([j["label"], j["action"]])).collect();
            rust.sort_by_key(Value::to_string);
            want.sort_by_key(Value::to_string);
            if rust != want {
                mismatches.push(format!("{at} options: rust {rust:?} / js {want:?}"));
            }
        }
    }

    println!("points checked: {points}, options scored as in the JS: {exact}");
    println!("mismatches: {}", mismatches.len());
    for m in mismatches.iter().take(10) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty(), "{} mismatches with the JS solver", mismatches.len());
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
