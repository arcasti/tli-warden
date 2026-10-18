//! Replays seeded runs recorded from the earlier JS engine through the Rust engine and compares the
//! state after every move (tests/fixtures/traces.jsonl, 20 games).

use serde_json::Value;
use warden::engine::{Action, State, apply, new_game};

mod common;
use common::{first_diff, js_repeat_differs, normalize, traces};

/// How a state compared with the trace.
#[derive(Clone, Copy, PartialEq)]
enum Match {
    /// Equal as the Rust engine left it.
    Exact,
    /// Equal once the dice, the destinations and the rng were taken from a JS morning with a
    /// repeated step.
    Dice,
    /// A JS morning with a repeated step where the engine draws from the stream after its roll
    /// (newGame, a day start that places markers): the whole state is taken from the trace.
    Whole,
}

/// The Rust roll redraws a repeated step, so on mornings without a repeat both engines draw the
/// same dice from the same stream. Where the trace shows a repeated step and the Rust roll
/// differs, the replay goes on with the JS morning: the dice, the destinations and the rng first,
/// the whole state if the stream was used again after the roll.
fn compare(state: &mut State, js: &Value) -> Result<Match, String> {
    let Some(diff) = state_diff(state, js) else { return Ok(Match::Exact) };
    if !js_repeat_differs(state, js) {
        return Err(diff);
    }
    state.dice = serde_json::from_value(js["dice"].clone()).expect("dice");
    state.destinations = serde_json::from_value(js["destinations"].clone()).expect("destinations");
    state.rng = serde_json::from_value(js["rng"].clone()).expect("rng");
    if state_diff(state, js).is_none() {
        return Ok(Match::Dice);
    }
    *state = serde_json::from_value(js.clone()).expect("JS state");
    Ok(Match::Whole)
}

fn state_diff(state: &State, js: &Value) -> Option<String> {
    let rust = normalize(serde_json::to_value(state).expect("state to JSON"));
    first_diff("state", &rust, &normalize(js.clone()))
}

#[test]
fn replays_js_traces() {
    let (mut games, mut full, mut starts_matched, mut steps_matched) = (0, 0, 0, 0);
    let mut copied = [0usize; 3];
    let mut mismatches: Vec<String> = Vec::new();
    for game in traces() {
        games += 1;
        let seed = game["seed"].as_str().expect("seed");
        let mut state = new_game(seed);
        match compare(&mut state, &game["start"]) {
            Ok(m) => {
                starts_matched += 1;
                copied[m as usize] += 1;
            }
            Err(diff) => {
                mismatches.push(format!("{seed} newGame: {diff}"));
                continue;
            }
        }
        let steps = game["steps"].as_array().expect("steps");
        let mut complete = true;
        for (i, step) in steps.iter().enumerate() {
            let action: Action = serde_json::from_value(step["action"].clone()).expect("action");
            let out = apply(&mut state, &action);
            let result = if out.is_ok() != step["ok"].as_bool().expect("ok") {
                Err(format!("ok: rust {} / js {}", out.is_ok(), step["ok"]))
            } else {
                compare(&mut state, &step["state"])
            };
            match result {
                Ok(m) => copied[m as usize] += 1,
                Err(diff) => {
                    mismatches.push(format!("{seed} step {i} {}: {diff}", step["action"]));
                    complete = false;
                    break;
                }
            }
            steps_matched += 1;
        }
        if complete {
            full += 1;
        }
    }

    println!("games: {games}, start states matched: {starts_matched}, fully matched: {full}, steps matched: {steps_matched}");
    let [exact, dice, whole] = copied;
    println!("states exact: {exact}, with the JS dice: {dice}, taken whole from the JS: {whole}");
    println!("mismatches: {}", mismatches.len());
    for m in mismatches.iter().take(5) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty(), "{} games differ from the JS", mismatches.len());
}
