//! Plays games 0 to 19 of seed 1 with the strategy alone, as the bench plays
//! them, and requires every decided action, the final vault and the run's
//! stats recorded in tests/fixtures/strategy-games.jsonl, so an accidental
//! change of the strategy fails. After a deliberate change, re-record the
//! fixture with `WARDEN_RECORD=1 cargo test --profile fast-test --test decide --test solve`
//! and review its diff.

use serde_json::{Value, json};
use warden::engine::data::Tool;
use warden::engine::state::State;
use warden::solver::{TUNED, bench};

mod common;

const GAMES: &str = "strategy-games.jsonl";

#[test]
fn decides_as_recorded() {
    let games: Vec<Value> = (0..20)
        .map(|k| {
            let (actions, st) = common::strategy_game(k, |_, _| {});
            // The test's game loop must stay the bench's.
            assert_eq!(bench::play_strategy(1, k, &[], &TUNED), bench::Solo { chromatics: st.vault.len(), loot: st.loot(), stats: st.stats, objective: st.objective.is_some_and(|o| o.done) }, "game {k}");
            json!({ "k": k, "actions": actions, "vault": st.vault.len(), "stats": st.stats })
        })
        .collect();
    if common::recording() {
        common::record(GAMES, &games);
        return;
    }

    let want = common::recorded(GAMES);
    let mut mismatches: Vec<String> = Vec::new();
    if want.len() != games.len() {
        mismatches.push(format!("games: now {} / recorded {}", games.len(), want.len()));
    }
    for (now, was) in games.iter().zip(&want) {
        let k = &now["k"];
        let (a, b) = (now["actions"].as_array().expect("actions"), was["actions"].as_array().expect("actions"));
        if let Some(i) = (0..a.len().max(b.len())).find(|&i| a.get(i) != b.get(i)) {
            let show = |v: Option<&Value>| v.map_or("none".to_string(), Value::to_string);
            mismatches.push(format!("game {k} step {i}: now {} / recorded {}", show(a.get(i)), show(b.get(i))));
        } else if now["vault"] != was["vault"] || now["stats"] != was["stats"] {
            mismatches.push(format!("game {k} end: now vault {} {} / recorded vault {} {}", now["vault"], now["stats"], was["vault"], was["stats"]));
        }
    }

    println!("games: {}, mismatches: {}", games.len(), mismatches.len());
    for m in mismatches.iter().take(10) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty(), "{} games differ from {GAMES}", mismatches.len());
}

fn board(name: &str) -> State {
    let (st, problems) = common::build(name);
    assert!(problems.is_empty(), "{problems:?}");
    st
}

#[test]
fn axe_doors_count_toward_a_swap() {
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
