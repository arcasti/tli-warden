//! The board model: fixture boards in tests/boards build the same state as
//! the analyzer's buildState (tests/fixtures/board-states.jsonl, recorded from
//! the earlier JS engine), problems read as in the JS, and
//! the `warden-cli solve` command line.

use serde_json::Value;
use std::path::Path;
use std::process::Command;
use warden::board::{CellModel, ChoiceKind, EventMark, Model, ObjectiveModel, build_state};
use warden::engine::actions::Action;
use warden::engine::data::{Beast, Tool, data};
use warden::engine::devour::devour_options;
use warden::engine::incidents::{CHAIN_REACTION, EXECUTION_EFFICIENCY, SEWER_RATWAY};
use warden::engine::rules::steps_between;
use warden::engine::state::{ObjectiveState, Pending, Phase, ToolReason, Used};
use warden::engine::apply;
use warden::solver::chain::{chain_odds, devour_delta};
use warden::solver::solve::reason;
use warden::solver::{Rng32, TUNED, decide};

mod common;
use common::{build, first_diff, fixture, normalize, raw};

const GOOD: [&str; 4] = ["patrol.json", "manage.json", "choice.json", "ratway.json"];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn problem_boards_read_as_in_the_js() {
    let cases = [
        ("problem-prey.json", "The prey mark from the Lion in cell 03 needs a same-rarity meal in cell 02."),
        (
            "problem-doors.json",
            "Mark today's doors: tick them under \u{201c}Select a Destination\u{201d}, or select a cell and press Today's door.",
        ),
        (
            "problem-event.json",
            "A Prison Incident needs the Inmate that came out of the door in the warden's cell: put the warden there and set its Inmate.",
        ),
    ];
    for (name, text) in cases {
        assert_eq!(build(name).1, vec![text.to_string()], "{name}");
    }
}

#[test]
fn good_boards_build_the_model() {
    let d = data();
    for name in GOOD {
        let m = raw(name);
        let (st, problems) = build(name);
        assert!(problems.is_empty(), "{name}: {problems:?}");
        for (n, c) in m["cells"].as_array().unwrap().iter().enumerate() {
            let inmate = st.cells[n].inmate;
            if c["kind"] == "inmate" {
                let i = inmate.unwrap_or_else(|| panic!("{name} cell {n}: no inmate"));
                assert!(!st.cells[n].sealed);
                assert_eq!(serde_json::to_value(i.beast).unwrap(), c["beast"], "{name} cell {n}");
                assert_eq!(i64::from(i.tier), c["tier"].as_i64().unwrap(), "{name} cell {n}");
            } else {
                assert!(inmate.is_none(), "{name} cell {n}");
            }
        }
        assert_eq!(st.position as i64, m["warden"].as_i64().unwrap(), "{name}");
        let phase = match m["step"].as_str().unwrap() {
            "patrol" => Phase::Patrol,
            "event" => Phase::Event,
            _ => Phase::Manage,
        };
        assert_eq!(st.phase, phase, "{name}");
        assert!(st.destinations.windows(2).all(|w| steps_between(st.position, w[0]) <= steps_between(st.position, w[1])), "{name}");
        let tools: Vec<Tool> = m["tools"].as_array().unwrap().iter().map(|t| Tool::from_key(t.as_str().unwrap()).unwrap()).collect();
        let order: Vec<Tool> = d.tool_ids.iter().copied().filter(|t| tools.contains(t)).collect();
        assert_eq!(st.tools, order, "{name}");
        if phase != Phase::Patrol {
            let used = Used { swap: u32::from(m["swapUsed"] == true), devour: u32::from(m["devourUsed"] == true), hunt: 0 };
            assert_eq!(st.used, used, "{name}");
        }
        let action = decide(&st, &TUNED, &mut Rng32::new(1)).unwrap_or_else(|| panic!("{name}: no action"));
        assert!(apply(&mut st.clone(), &action).is_ok(), "{name}: {action:?} refused");
    }
    let (patrol, _) = build("patrol.json");
    assert_eq!(patrol.destinations, vec![5, 9, 14]);
    assert_eq!(patrol.vault.iter().map(|v| v.beast).collect::<Vec<_>>(), vec![Beast::Wolf]);
    assert_eq!(patrol.prey.len(), 1);
    assert_eq!(patrol.cells[5].event_known, Some(false));
}

/// The Sewer Ratway cell pick: every cell but the room is an option, the
/// solver moves the warden to one, and markers on open cells reach the state.
#[test]
fn ratway_board_picks_a_cell() {
    let d = data();
    let (st, problems) = build("ratway.json");
    assert!(problems.is_empty(), "{problems:?}");
    let all: Vec<usize> = (0..d.cell_count).filter(|&n| n != d.room).collect();
    assert_eq!(st.pending, vec![Pending::Cell { id: SEWER_RATWAY, options: all }]);
    assert_eq!((st.cells[3].event, st.cells[3].event_known), (Some(CHAIN_REACTION), Some(false)));
    assert_eq!((st.cells[5].event, st.cells[5].sealed), (Some(EXECUTION_EFFICIENCY), false));
    assert_eq!(st.cells[12].event, None);
    let pick = warden::solver::solve::instant(&st).expect("pick");
    assert!(pick.label.starts_with("Move to "), "{}", pick.label);
    let options = warden::solver::solve::solve(&st, 20, 1, &TUNED);
    assert!(!options.is_empty() && options.iter().all(|o| o.label.starts_with("Move to ")));
}

/// The new fields survive a JSON round trip, and an open cell without an
/// `event` (older board files) has none.
#[test]
fn open_cell_markers_round_trip() {
    let model: Model = serde_json::from_value(raw("ratway.json")).expect("model");
    assert_eq!(model.choice.kind, ChoiceKind::Cell);
    assert_eq!(model.cells[5], CellModel::Empty { event: Some(EventMark::Execution) });
    assert_eq!(model.cells[12], CellModel::Empty { event: None });
    assert!(matches!(model.cells[3], CellModel::Inmate { event: Some(EventMark::Unknown), .. }));
    assert!(matches!(model.cells[1], CellModel::Inmate { event: None, .. }));
    let json = serde_json::to_value(&model).unwrap();
    assert_eq!(json["choice"]["kind"], "cell");
    assert_eq!(json["cells"][5]["event"], "execution");
    let back: Model = serde_json::from_value(json).unwrap();
    assert_eq!(back, model);
}

/// The JS pending choice has no `reason` or `picks` and its `used` no
/// `hunt`; the engine reads those absences as Start, 1 and 0.
fn drop_rust_only(rust: &mut Value, js: &Value) {
    if js["used"].get("hunt").is_none() {
        rust["used"].as_object_mut().unwrap().remove("hunt");
    }
    if js["params"].get("vaultCap").is_none() {
        rust["params"].as_object_mut().unwrap().remove("vaultCap");
    }
    for (r, j) in rust["pending"].as_array_mut().unwrap().iter_mut().zip(js["pending"].as_array().unwrap()) {
        for key in ["reason", "picks"] {
            if j.get(key).is_none() {
                r.as_object_mut().unwrap().remove(key);
            }
        }
    }
}

#[test]
fn builds_like_the_js() {
    let text = std::fs::read_to_string(fixture("board-states.jsonl")).expect("board-states.jsonl");
    let mut count = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let js: Value = serde_json::from_str(line).expect("board line");
        let name = js["board"].as_str().unwrap();
        let (st, problems) = build(name);
        assert_eq!(serde_json::to_value(&problems).unwrap(), js["problems"], "{name} problems");
        let mut rust = serde_json::to_value(&st).unwrap();
        drop_rust_only(&mut rust, &js["state"]);
        // The analyzer's leftover dice and incident counts come from the JS newGame: its roll of
        // independent dice repeats a step (4, 3, 3), and the Rust engine draws distinct steps
        // (docs/s16-screen.md), so the markers newGame places after the roll differ too. The board
        // sets the cells itself, so those two fields are left out of the comparison.
        let mut js_state = js["state"].clone();
        assert_eq!(js_state["dice"], serde_json::json!([4, 3, 3]), "{name} JS dice");
        for v in [&mut rust, &mut js_state] {
            for key in ["dice", "incidentCounts"] {
                v.as_object_mut().unwrap().remove(key);
            }
        }
        // The JS marks every "scratch" door scratched; the Rust only with Filthy Claws.
        if !st.has(Tool::Claws) {
            for cell in js_state["cells"].as_array_mut().unwrap() {
                cell["scratch"] = Value::Bool(false);
            }
        }
        if let Some(diff) = first_diff("state", &normalize(rust), &normalize(js_state)) {
            panic!("{name}: {diff}");
        }
        count += 1;
    }
    assert_eq!(count, 7, "fixtures in board-states.jsonl");
    println!("{count} boards match the JS buildState");
}

fn warden(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_warden-cli")).args(args).current_dir(root()).output().expect("warden-cli");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn cli_solves_a_board() {
    let (code, out) = warden(&["solve", "tests/boards/patrol.json", "--rollouts", "50"]);
    assert_eq!(code, 0, "{out}");
    let (st, _) = build("patrol.json");
    let pick = warden::solver::solve::instant(&st).expect("pick");
    assert!(out.contains(&format!("Pick: {}", pick.label)), "{out}");
    let rows: Vec<&str> = out.lines().filter(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit())).collect();
    for c in warden::solver::solve::candidates(&st) {
        assert!(rows.iter().any(|r| r.contains(&c.label)), "{} missing:\n{out}", c.label);
    }
    assert_eq!(rows.iter().filter(|r| r.ends_with(" *")).count(), 1, "{out}");

    let (code, out) = warden(&["solve", "tests/boards/problem-doors.json"]);
    assert_eq!(code, 2);
    assert!(out.contains("Mark today's doors"), "{out}");

    assert_eq!(warden(&["solve"]).0, 1);
    assert_eq!(warden(&["solve", "tests/boards/missing.json"]).0, 1);
}

/// manage.json with `objective` set as the board JSON carries it.
fn with_objective(objective: Value) -> Model {
    let mut m = raw("manage.json");
    m["objective"] = objective;
    serde_json::from_value(m).expect("model")
}

/// An objective one consume short (Survival of the Fittest, 3 of 4) is in the
/// state, and the consume completes it and queues the instrument pick.
#[test]
fn objective_in_the_model_reaches_the_state() {
    let model = with_objective(serde_json::json!({ "id": 2, "progress": 3 }));
    assert_eq!(model.objective, Some(ObjectiveModel { id: 2, progress: 3 }));
    let back: Model = serde_json::from_value(serde_json::to_value(&model).unwrap()).unwrap();
    assert_eq!(back, model);
    let (mut st, problems) = build_state(&model, 1);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(st.objective, Some(ObjectiveState { id: 2, progress: 3, done: false }));
    let bite = devour_options(&st, st.position).into_iter().next().expect("a consume");
    assert!(apply(&mut st, &Action::Devour { to: bite.cell }).is_ok());
    assert_eq!(st.objective, Some(ObjectiveState { id: 2, progress: 4, done: true }));
    assert!(st.pending.iter().any(|p| matches!(p, Pending::Tool { reason: ToolReason::Objective, .. })), "{:?}", st.pending);
}

/// A glowing "?" door of the eater's rarity is a Wolf's target only with
/// Filthy Claws: without it the glow is an incident showing the rarity.
#[test]
fn glowing_door_is_scratched_only_with_filthy_claws() {
    let mut model: Model = serde_json::from_value(raw("manage.json")).expect("model");
    model.warden = 9;
    let door = |st: &warden::engine::state::State| devour_options(st, 9).iter().any(|b| b.cell == 8 && b.scratch);
    let (st, _) = build_state(&model, 1);
    assert!(!st.cells[8].scratch && st.cells[8].known == 1);
    assert!(!door(&st));
    model.tools.push(Tool::Claws);
    let (st, _) = build_state(&model, 1);
    assert!(st.cells[8].scratch);
    assert!(door(&st));
}

#[test]
fn unknown_objective_is_a_problem() {
    let (st, problems) = build_state(&with_objective(serde_json::json!({ "id": 99, "progress": 0 })), 1);
    assert_eq!(st.objective, None);
    assert_eq!(problems, vec!["There is no objective with id 99.".to_string()]);
}

#[test]
fn board_without_objective_has_none() {
    for objective in [Value::Null, serde_json::json!("x")] {
        let (st, problems) = build_state(&with_objective(objective), 1);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(st.objective, None);
    }
    let model: Model = serde_json::from_value(raw("manage.json")).expect("model");
    assert_eq!(model.objective, None);
    assert!(serde_json::to_value(&model).unwrap().get("objective").is_none());
    assert_eq!(build_state(&model, 1).0.objective, None);
}

#[test]
fn guillotine_on_the_last_day_keeps_the_block() {
    // Replace with Scapegoat's Guillotine removes 2 of the 3 others, and no
    // Chromatic can be made on the last day; with Avenger's Casket it pays.
    for (tools, want) in [(serde_json::json!(["scapegoat", "rope"]), Action::End), (serde_json::json!(["revenge", "rope"]), Action::Swap)] {
        let st = with_tools("guillotine-last-day.json", tools.clone());
        assert_eq!(decide(&st, &TUNED, &mut Rng32::new(1)), Some(want), "{tools}");
    }
}

/// The board of `name` with these instruments held.
fn with_tools(name: &str, tools: Value) -> warden::engine::state::State {
    let mut m = raw(name);
    m["tools"] = tools;
    let model: Model = serde_json::from_value(m).expect("model");
    build_state(&model, 1).0
}

#[test]
fn the_end_reason_names_the_instrument_that_decides_it() {
    let st = with_tools("coin-pair-day15.json", serde_json::json!(["coin", "dogtag"]));
    let why = reason(&st, &Action::End);
    assert!(why.contains("Kin's Coin of Fate") && why.contains("16") && why.contains("10"), "{why}");
    let st = with_tools("guillotine-last-day.json", serde_json::json!(["scapegoat", "rope"]));
    let why = reason(&st, &Action::End);
    assert!(why.contains("Scapegoat's Guillotine"), "{why}");
}

#[test]
fn chain_odds_count_the_avengers_casket() {
    // The warden's inmate made a Rare Berserk Lion so it has a meal, the Rare
    // Alpha Wolf in 03; no chain on the open block reaches Chromatic.
    let odds = |tools: Value| {
        let mut m = raw("guillotine-last-day.json");
        m["tools"] = tools;
        m["cells"][12] = serde_json::json!({"kind": "inmate", "beast": "lion", "tier": 2});
        let model: Model = serde_json::from_value(m).expect("model");
        let st = build_state(&model, 1).0;
        assert_eq!(devour_options(&st, 12).iter().map(|o| o.cell).collect::<Vec<_>>(), [2]);
        chain_odds(&st, 12, 2)
    };
    assert_eq!(odds(serde_json::json!(["rope"])), (0.0, 1.25));
    let (rainbow, _) = odds(serde_json::json!(["revenge", "rope"]));
    assert!(rainbow > 0.0, "{rainbow}");
}

#[test]
fn coin_of_fate_keeps_the_last_pair() {
    // Two Legendary Lions with five mornings left: Kin's Coin of Fate makes a
    // Chromatic of the pair, and a Consume by the warden's Lion breaks it.
    for (tools, want) in [(serde_json::json!(["coin", "dogtag"]), Action::End), (serde_json::json!(["dogtag"]), Action::Devour { to: 11 })] {
        let st = with_tools("coin-pair-day15.json", tools.clone());
        assert_eq!(decide(&st, &TUNED, &mut Rng32::new(1)), Some(want), "{tools}");
    }
}

/// A block of two open inmates, the eater in cell 5 and its meal in cell 9,
/// every other cell sealed, with these instruments held; the meal is the
/// first beast the eater counters, at `meal_tier`.
fn duel(tools: Value, eater: (Beast, i32), meal_tier: i32) -> warden::engine::state::State {
    let mut m = raw("coin-pair-day15.json");
    m["tools"] = tools;
    let meal = data().food_chain[eater.0.index()][0];
    for (n, cell) in m["cells"].as_array_mut().expect("cells").iter_mut().enumerate().skip(1) {
        *cell = serde_json::json!({ "kind": "sealed", "event": null });
        if n == 5 || n == 9 {
            let (beast, tier) = if n == 5 { eater } else { (meal, meal_tier) };
            *cell = serde_json::json!({ "kind": "inmate", "beast": beast, "tier": tier });
        }
    }
    let model: Model = serde_json::from_value(m).expect("model");
    build_state(&model, 1).0
}

#[test]
fn the_maul_lands_a_mammoth_two_tiers_above_its_meal() {
    // A Normal Mammoth eats a Rare inmate it counters: the meal leaves the
    // Rare tier and the Mammoth lands at Ultimate, with nothing left to eat.
    let st = duel(serde_json::json!(["maul"]), (Beast::Mammoth, 0), 2);
    assert_eq!(devour_options(&st, 5).iter().map(|o| o.cell).collect::<Vec<_>>(), [9]);
    let mut ctx = warden::solver::context(&st, &TUNED);
    let (mut comp, crowd) = warden::solver::composition(&st, Some(5));
    let before = comp;
    let meal = st.cells[9].inmate.expect("meal");
    let delta = devour_delta(&mut ctx, 0, Beast::Mammoth.index(), (2, meal.beast.index()), false, &mut comp, crowd);
    assert!((delta - (ctx.v[4] - ctx.v[2] - ctx.v[0])).abs() < 1e-9, "{delta}");
    assert_eq!(comp, before);
}

#[test]
fn the_fanged_guillotine_prices_oppress() {
    // An Ultimate Wolf eats a Normal Rat: Oppress lets it rise to Chromatic
    // 80% of the time (5% per tier below); else it stays Ultimate.
    let st = duel(serde_json::json!(["guillotine"]), (Beast::Wolf, 4), 0);
    assert_eq!(devour_options(&st, 5).iter().map(|o| o.cell).collect::<Vec<_>>(), [9]);
    let p = 1.0 - 4.0 * data().oppress_per_tier / 100.0;
    assert!((p - 0.8).abs() < 1e-12);
    let mut ctx = warden::solver::context(&st, &TUNED);
    let (mut comp, crowd) = warden::solver::composition(&st, Some(5));
    assert_eq!(st.cells[9].inmate.map(|i| i.beast), Some(Beast::Rat));
    let delta = devour_delta(&mut ctx, 4, Beast::Wolf.index(), (0, Beast::Rat.index()), false, &mut comp, crowd);
    let want = -ctx.v[0] + p * ctx.r + (1.0 - p) * ctx.v[4] - ctx.v[4];
    assert!((delta - want).abs() < 1e-9, "{delta} {want}");
}
