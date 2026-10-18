//! Replays seeded runs recorded from the earlier JS engine through the Rust engine and compares the
//! state after every move (tests/fixtures/traces.jsonl, 20 games).

use serde_json::Value;
use warden::engine::{Action, apply, new_game_with};

mod common;
use common::{first_diff, follow_js_repeat, no_cap, normalize, traces};

fn state_diff(state: &warden::engine::State, js: &Value) -> Option<String> {
    let rust = normalize(serde_json::to_value(state).expect("state to JSON"));
    first_diff("state", &rust, &normalize(js.clone()))
}

#[test]
fn replays_js_traces() {
    let (mut games, mut full, mut steps_matched, mut starts_matched) = (0, 0, 0, 0);
    let mut mismatches: Vec<String> = Vec::new();
    for game in traces() {
        games += 1;
        let seed = game["seed"].as_str().expect("seed");
        let mut state = new_game_with(seed, no_cap());
        follow_js_repeat(&mut state, &game["start"]);
        if let Some(diff) = state_diff(&state, &game["start"]) {
            mismatches.push(format!("{seed} newGame: {diff}"));
            continue;
        }
        starts_matched += 1;
        let steps = game["steps"].as_array().expect("steps");
        let mut complete = true;
        for (i, step) in steps.iter().enumerate() {
            let action: Action = serde_json::from_value(step["action"].clone()).expect("action");
            let out = apply(&mut state, &action);
            let diff = if out.is_ok() != step["ok"].as_bool().expect("ok") {
                Some(format!("ok: rust {} / js {}", out.is_ok(), step["ok"]))
            } else {
                follow_js_repeat(&mut state, &step["state"]);
                state_diff(&state, &step["state"])
            };
            if let Some(diff) = diff {
                mismatches.push(format!("{seed} step {i} {}: {diff}", step["action"]));
                complete = false;
                break;
            }
            steps_matched += 1;
        }
        if complete {
            full += 1;
        }
    }

    println!("games: {games}, start states matched: {starts_matched}, fully matched: {full}, steps matched: {steps_matched}");
    println!("mismatches: {}", mismatches.len());
    for m in mismatches.iter().take(5) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty(), "{} games differ from the JS", mismatches.len());
}
