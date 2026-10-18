//! Replays seeded runs recorded from the earlier JS engine and requires the Rust strategy to pick every
//! traced move the JS strategy picked (tests/fixtures/traces.jsonl, 20 games).

use warden::engine::rng::hash_seed;
use warden::engine::{Action, apply, new_game};
use warden::engine::state::State;
use warden::solver::solve::{FollowUp, follow_up};
use warden::solver::{Rng32, TUNED, decide};

mod common;

#[test]
fn decides_like_the_js() {
    let (mut games, mut full, mut compared) = (0, 0, 0);
    let mut mismatches: Vec<String> = Vec::new();
    for game in common::traces() {
        games += 1;
        let seed = game["seed"].as_str().expect("seed");
        let mut state = new_game(seed);
        common::follow_js_repeat(&mut state, &game["start"]);
        let mut roll = Rng32::new(hash_seed(seed));
        let steps = game["steps"].as_array().expect("steps");
        let mut complete = true;
        for (i, step) in steps.iter().enumerate() {
            let action: Action = serde_json::from_value(step["action"].clone()).expect("action");
            // The End Day after a failed move is the playout's fallback, not a decide pick.
            let fallback = i > 0 && action == Action::End && !steps[i - 1]["ok"].as_bool().unwrap_or(true) && steps[i - 1]["action"]["type"] != "end";
            if !fallback {
                let pick = decide(&state, &TUNED, &mut roll);
                let rust = serde_json::to_value(&pick).expect("pick to JSON");
                compared += 1;
                if rust != step["action"] {
                    mismatches.push(format!("{seed} step {i}: rust {rust} / js {}", step["action"]));
                    complete = false;
                    break;
                }
            }
            apply(&mut state, &action);
            common::follow_js_repeat(&mut state, &step["state"]);
        }
        if complete {
            full += 1;
        }
    }

    println!("games: {games}, steps compared: {compared}, fully matched: {full}");
    println!("mismatches: {}", mismatches.len());
    for m in mismatches.iter().take(10) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty(), "{} games differ from the JS", mismatches.len());
}

fn board(name: &str) -> State {
    let (st, problems) = common::build(name);
    assert!(problems.is_empty(), "{problems:?}");
    st
}

#[test]
fn follow_up_after_a_known_marker() {
    // Cell 15's Chain Reaction marker is known: its Confirm follows the patrol on every resample.
    let st = board("patrol.json");
    let Some(FollowUp::Move(next)) = follow_up(&st, &Action::Patrol { cell: 14 }) else { panic!("expected a move") };
    assert_eq!((next.action, next.label.as_str()), (Action::Event, "Confirm Chain Reaction"));
}

#[test]
fn follow_up_when_only_the_end_is_left() {
    // Consume used: after the Replace, End the day is the only move.
    let mut st = board("manage.json");
    (st.used.devour, st.used.swap) = (1, 0);
    let Some(FollowUp::Move(next)) = follow_up(&st, &Action::Swap) else { panic!("expected a move") };
    assert_eq!((next.action, next.why.as_str()), (Action::End, "It's the only move left."));
}

#[test]
fn follow_up_behind_a_door_depends() {
    let st = board("patrol.json");
    // A locked door: what it holds decides the next move.
    assert!(matches!(follow_up(&st, &Action::Patrol { cell: 9 }), Some(FollowUp::Depends)));
    // An unseen incident: the Confirm would name one the player can't see yet.
    assert!(matches!(follow_up(&st, &Action::Patrol { cell: 5 }), Some(FollowUp::Depends)));
}

#[test]
fn axe_doors_count_toward_a_swap() {
    use warden::engine::data::Tool;
    use warden::solver::chain::swap_dist;
    use warden::solver::context;
    use warden::solver::scores::swap_gain_here;
    // The warden stands on a Rare Lion with a Replace left and 7 sealed cells
    // (scratch cells and the nest count); with the Noose the new inmate is Rare
    // or rarer, and a rarer one opens min(2, sealed) of them.
    let mut st = board("manage.json");
    st.used.swap = 0;
    let axe_term = |st: &mut State| {
        st.tools = vec![Tool::Rope];
        let rope = swap_gain_here(&context(st, &TUNED));
        st.tools.push(Tool::Axe);
        let ctx = context(st, &TUNED);
        let rise = 1.0 - swap_dist(&ctx)[2];
        (swap_gain_here(&ctx) - rope) / (rise * TUNED.w_open * ctx.open_value)
    };
    assert_eq!(st.sealed_cells().len(), 7);
    assert!((axe_term(&mut st) - 2.0).abs() < 1e-9);
    for n in st.sealed_cells().into_iter().skip(1) {
        st.cells[n].sealed = false;
    }
    assert!((axe_term(&mut st) - 1.0).abs() < 1e-9);
}
