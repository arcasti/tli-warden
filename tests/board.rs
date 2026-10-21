//! The board model: fixture boards in tests/boards build the same state as
//! the analyzer's buildState (tests/fixtures/board-states.jsonl, recorded from
//! the earlier JS engine), problems read as in the JS, and
//! the `warden-cli solve` command line.

use serde_json::Value;
use std::path::Path;
use std::process::Command;
use warden::board::{CellModel, ChoiceKind, EventMark, Model, ObjectiveModel, build_state};
use warden::engine::actions::{Action, ChoiceId};
use warden::engine::data::{Beast, CAGE_FIGHT, Gift, Tool, data};
use warden::engine::devour::{bites, devour_options, eater_mask};
use warden::engine::incidents::{CHAIN_REACTION, EXECUTION_EFFICIENCY, SEWER_RATWAY};
use warden::engine::rules::steps_between;
use warden::engine::state::{ObjectiveState, Pending, Phase, ToolReason, Used};
use warden::engine::apply;
use warden::solver::chain::{Meal, bite, chain_odds, devour_delta, replace_odds, rise};
use warden::solver::scores::{patrol_scores, swap_gain_here};
use warden::solver::values::swap_value;
use warden::solver::solve::reason;
use warden::solver::bench::start;
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
            let used = Used { swap: u32::from(m["swapUsed"] == true), devour: u32::from(m["devourUsed"] == true), hunt: u32::from(m["huntUsed"] == true) };
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

/// `huntUsed`: absent from older board files, where it reads false; it
/// survives a JSON round trip and goes into the built state's day's uses.
#[test]
fn hunt_used_round_trips() {
    let mut json = raw("manage.json");
    assert!(json.get("huntUsed").is_none());
    let model: Model = serde_json::from_value(json.clone()).expect("model");
    assert!(!model.hunt_used);
    assert_eq!(build_state(&model, 1).0.used.hunt, 0);
    json["huntUsed"] = Value::Bool(true);
    let model: Model = serde_json::from_value(json).expect("model");
    assert!(model.hunt_used);
    let back = serde_json::to_value(&model).unwrap();
    assert_eq!(back["huntUsed"], true);
    assert_eq!(serde_json::from_value::<Model>(back).unwrap(), model);
    let (st, _) = build_state(&model, 1);
    assert_eq!((st.phase, st.used.hunt), (Phase::Manage, 1));
}

/// hunt-pick: the Hunt's "Select an Objective" with the Lion in cell 3 and
/// two Rare targets, the Rat in cell 2 and the Wolf in cell 6. The pick
/// marks only the chosen one.
#[test]
fn hunt_pick_board_offers_the_lion_targets() {
    let (st, problems) = build("hunt-pick.json");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(st.pending, vec![Pending::Prey { options: vec![1, 5] }]);
    let pick = warden::solver::solve::instant(&st).expect("pick");
    assert!(pick.label.starts_with(&format!("{}: mark the ", data().hunt_name)), "{}", pick.label);
    let mut picked = st.clone();
    assert!(apply(&mut picked, &Action::Choose { id: ChoiceId::Cell(5) }).is_ok());
    assert!(picked.pending.is_empty());
    let marked: Vec<u32> = picked.prey.iter().map(|l| l.prey).collect();
    assert_eq!(marked, vec![st.cells[5].inmate.unwrap().id]);
    let mut json = raw("hunt-pick.json");
    json["huntUsed"] = Value::Bool(true);
    let (_, problems) = build_state(&serde_json::from_value(json).expect("model"), 1);
    assert_eq!(problems.len(), 1, "{problems:?}");
}

/// A Hunt marks the one target it names, once a day, and only a target.
#[test]
fn hunt_marks_one_prey() {
    let (mut st, _) = build("hunt-pick.json");
    st.pending.clear();
    let fresh = st.clone();
    assert!(!apply(&mut st, &Action::Hunt { cell: 9 }).is_ok());
    assert!(!apply(&mut st, &Action::Hunt { cell: 6 }).is_ok());
    assert_eq!(st.prey, fresh.prey);
    assert!(apply(&mut st, &Action::Hunt { cell: 1 }).is_ok());
    let rat = st.cells[1].inmate.unwrap();
    let lion = st.cells[2].inmate.unwrap();
    assert_eq!(st.prey.len(), 1);
    assert_eq!((st.prey[0].lion, st.prey[0].prey, st.used.hunt), (lion.id, rat.id, 1));
    assert!(!apply(&mut st, &Action::Hunt { cell: 5 }).is_ok());
    assert_eq!(st.prey.len(), 1);
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
    if js["params"].get("maiden").is_none() {
        rust["params"].as_object_mut().unwrap().remove("maiden");
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

/// Landing on a scratched door gives up the bite of an open Wolf of its
/// rarity, so the landing scores lower; with no such Wolf the scratch costs nothing.
#[test]
fn opening_a_scratched_door_gives_up_the_wolf_bite() {
    let score = |beast: Beast, claws: bool| {
        let mut model: Model = serde_json::from_value(raw("manage.json")).expect("model");
        model.cells[9] = CellModel::Inmate { beast, tier: 0, ivory: false, prey: None, event: None };
        if claws {
            model.tools.push(Tool::Claws);
        }
        let (st, _) = build_state(&model, 1);
        assert_eq!(st.cells[8].scratch, claws);
        let mut ctx = warden::solver::context(&st, &TUNED);
        patrol_scores(&mut ctx, &[8])[0].1
    };
    assert!(score(Beast::Wolf, true) < score(Beast::Wolf, false));
    assert_eq!(score(Beast::Rat, true), score(Beast::Rat, false));
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
fn guillotine_on_the_last_day_still_replaces() {
    // Replace with Scapegoat's Guillotine removes 2 of the 3 others, but on
    // the last day they are worth only their Riot loot, so the Replace pays
    // as with Avenger's Casket (loot playouts: 2.063 against 2.050 for
    // finishing, paired error 0.001).
    for (tools, want) in [(serde_json::json!(["scapegoat", "rope"]), Action::Swap), (serde_json::json!(["revenge", "rope"]), Action::Swap)] {
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

/// The value of a bite by the Rare eater of a duel on its Rare meal, as the
/// day's first bite or a chained one, with these instruments held.
fn duel_bite(tools: Value, eater: Beast, chained: bool) -> f64 {
    let st = duel(tools, (eater, 2), 2);
    let mut ctx = warden::solver::context(&st, &TUNED);
    let (mut comp, crowd) = warden::solver::composition(&st, Some(5));
    let meal = st.cells[9].inmate.expect("meal");
    let i = 2 * 4 + meal.beast.index();
    comp[i] -= 1;
    bite(&mut ctx, 2, eater.index(), Meal { tier: 2, beast: meal.beast.index(), base: 1, chance: 1.0 }, chained, &mut comp, crowd)
}

#[test]
fn the_barbed_collar_rolls_on_chained_bites_only() {
    let plain = duel_bite(serde_json::json!([]), Beast::Wolf, false);
    assert!((duel_bite(serde_json::json!(["collar"]), Beast::Wolf, false) - plain).abs() < 1e-12);
    assert!(duel_bite(serde_json::json!(["collar"]), Beast::Wolf, true) > plain + 1e-6);
}

#[test]
fn the_barbed_whip_raises_a_chained_bite() {
    let plain = duel_bite(serde_json::json!([]), Beast::Wolf, true);
    let whip = duel_bite(serde_json::json!(["whip"]), Beast::Wolf, true);
    assert!(whip > plain + 1e-6, "{whip} {plain}");
}

#[test]
fn the_avengers_casket_spawns_a_meal_for_the_new_form() {
    // An Ultimate Lion on an empty block, with the Iron Crown: the Casket's
    // counter is always eaten and lands it at Chromatic.
    let st = duel(serde_json::json!(["revenge", "crown"]), (Beast::Lion, 4), 0);
    let mut ctx = warden::solver::context(&st, &TUNED);
    let c = data().tool(Tool::Revenge).chance / 100.0;
    let value = rise(&mut ctx, 4, Beast::Lion.index(), &mut [0; 24], 1);
    let want = c * ctx.r + (1.0 - c) * ctx.v[4];
    assert!((value - want).abs() < 1e-9, "{value} {want}");
}

#[test]
fn the_crowded_beastcage_counts_the_reopened_cell_with_reusable_cells() {
    let mut st = duel(serde_json::json!(["beastcage"]), (Beast::Wolf, 2), 2);
    let gain = |st: &warden::engine::state::State| {
        let mut ctx = warden::solver::context(st, &TUNED);
        let mut plain = warden::solver::context(st, &TUNED);
        plain.beastcage = 0.0;
        rise(&mut ctx, 3, Beast::Wolf.index(), &mut [0; 24], 1) - rise(&mut plain, 3, Beast::Wolf.index(), &mut [0; 24], 1)
    };
    let c = data().tool(Tool::Beastcage).chance / 100.0;
    let side_value = warden::solver::context(&st, &TUNED).side_value;
    assert!((gain(&st) - c * side_value).abs() < 1e-9 && gain(&st) > 0.0);
    st.params.reusable_cells = false;
    assert_eq!(gain(&st), 0.0);
}

/// The guillotine-last-day board (a Rare Wolf in 02, a Legendary Lion in 08,
/// a Normal Lion in 11, the warden in 12) with the warden's inmate and these
/// open inmates set as (cell, tier), and these instruments held.
fn replace_board(tools: Value, warden_tier: i32, set: &[(usize, i32)]) -> warden::engine::state::State {
    let mut m = raw("guillotine-last-day.json");
    m["tools"] = tools;
    m["cells"][12] = serde_json::json!({"kind": "inmate", "beast": "wolf", "tier": warden_tier});
    for &(n, tier) in set {
        m["cells"][n]["tier"] = serde_json::json!(tier);
    }
    let model: Model = serde_json::from_value(m).expect("model");
    build_state(&model, 1).0
}

/// The tier odds of a Replace in the warden's cell, and its value.
fn replace(st: &warden::engine::state::State) -> (warden::solver::Dist, f64) {
    let ctx = warden::solver::context(st, &TUNED);
    let (comp, crowd) = warden::solver::composition(st, Some(12));
    let tier = st.cells[12].inmate.expect("inmate").tier as usize;
    let dist = replace_odds(&ctx, tier, &comp, crowd - 1).0;
    (dist, swap_value(&ctx, tier, &comp, crowd - 1))
}

#[test]
fn the_burning_stake_moves_low_rolls_to_legendary() {
    let (plain, _) = replace(&replace_board(serde_json::json!([]), 0, &[]));
    let (stake, _) = replace(&replace_board(serde_json::json!(["stake"]), 0, &[]));
    let c = data().tool(Tool::Stake).chance / 100.0;
    assert!((c - 0.4).abs() < 1e-12);
    let low: f64 = plain[..3].iter().sum();
    assert!((stake[3] - (plain[3] + c * low)).abs() < 1e-12, "{stake:?}");
    assert!((stake.iter().sum::<f64>() - 1.0).abs() < 1e-12);
}

#[test]
fn the_severed_iron_maiden_lifts_a_rare_among_rares() {
    // Every other open inmate is Rare: a Rare roll has none above and none
    // below, so it rises 3 tiers to Chromatic.
    let rares = [(2, 2), (8, 2), (11, 2)];
    let (plain, plain_value) = replace(&replace_board(serde_json::json!([]), 1, &rares));
    // A read board gives the Maiden no rise; price it as the engine rule has it.
    let mut st = replace_board(serde_json::json!(["maiden"]), 1, &rares);
    st.params.maiden = true;
    let (maiden, maiden_value) = replace(&st);
    assert_eq!(maiden[2], 0.0);
    assert!((maiden[5] - plain[2]).abs() < 1e-12, "{maiden:?}");
    assert!(maiden_value > plain_value + 1e-6, "{maiden_value} {plain_value}");
}

#[test]
fn a_read_board_gives_the_maiden_no_rise() {
    let rares = [(2, 2), (8, 2), (11, 2)];
    let (plain, plain_value) = replace(&replace_board(serde_json::json!([]), 1, &rares));
    let (maiden, maiden_value) = replace(&replace_board(serde_json::json!(["maiden"]), 1, &rares));
    assert_eq!(maiden, plain);
    assert!((maiden_value - plain_value).abs() < 1e-12, "{maiden_value} {plain_value}");
}

#[test]
fn the_execution_axe_counts_the_burning_stake() {
    // The Axe's part of a Replace's gain grows with the Stake's Legendaries;
    // before the last day, when the doors it opens still count.
    let gain = |tools: Value| {
        let mut st = replace_board(tools, 0, &[]);
        st.day = 10;
        swap_gain_here(&warden::solver::context(&st, &TUNED))
    };
    let axe = gain(serde_json::json!(["axe"])) - gain(serde_json::json!([]));
    let axe_with_stake = gain(serde_json::json!(["axe", "stake"])) - gain(serde_json::json!(["stake"]));
    assert!(axe > 0.0 && axe_with_stake > axe + 1e-6, "{axe_with_stake} {axe}");
}

#[test]
fn the_sorting_hook_and_the_dog_tag_raise_an_unseen_door() {
    let ctx_of = |tools: Value| {
        let st = duel(tools, (Beast::Wolf, 2), 2);
        let ctx = warden::solver::context(&st, &TUNED);
        (warden::solver::door_dist(&st, None, true).0, ctx.open_value)
    };
    let (plain_dist, plain) = ctx_of(serde_json::json!([]));
    let (hook_dist, hook) = ctx_of(serde_json::json!(["hook"]));
    assert!(plain_dist[0] > 0.0 && hook_dist[0] == 0.0, "{hook_dist:?}");
    assert!(hook > plain + 1e-6, "{hook} {plain}");
    // With its chance the Dog Tag lifts the opened inmate its tiers.
    let (tag_dist, tag) = ctx_of(serde_json::json!(["dogtag"]));
    let def = data().tool(Tool::Dogtag);
    let (c, by) = (def.chance / 100.0, def.tiers.unwrap_or(0) as usize);
    for t in 0..tag_dist.len() {
        let want = (1.0 - c) * plain_dist[t] + if t >= by { c * plain_dist[t - by] } else { 0.0 };
        assert!((tag_dist[t] - want).abs() < 1e-12, "{tag_dist:?}");
    }
    assert!(tag > plain + 1e-6, "{tag} {plain}");
}

#[test]
fn a_revealed_legendary_door_outscores_an_unseen_one() {
    let mut st = duel(serde_json::json!([]), (Beast::Wolf, 2), 2);
    let door = 7;
    let score = |st: &warden::engine::state::State| {
        let mut ctx = warden::solver::context(st, &TUNED);
        warden::solver::scores::patrol_scores(&mut ctx, &[door])[0].1
    };
    // The solver stays blind to the inmate behind an unseen door.
    st.cells[door].hidden = Some(warden::engine::state::Inmate { id: 999, beast: Beast::Lion, tier: 3 });
    let unseen = score(&st);
    st.cells[door].hidden = None;
    assert_eq!(score(&st), unseen);
    st.cells[door].hidden = Some(warden::engine::state::Inmate { id: 999, beast: Beast::Lion, tier: 3 });
    st.cells[door].known = 1;
    let rarity = score(&st);
    st.cells[door].known = 2;
    let both = score(&st);
    assert!(rarity > unseen + 1e-6 && both > unseen + 1e-6, "{rarity} {both} {unseen}");
}

#[test]
fn the_ring_of_keys_prices_the_sealed_doors_beside_a_patrolled_one() {
    // Door 7 sits between two sealed cells; door 12's neighbours are open.
    let mut st = duel(serde_json::json!([]), (Beast::Wolf, 2), 2);
    for n in [11, 13] {
        st.cells[n].sealed = false;
    }
    let gap = |st: &warden::engine::state::State| {
        let mut ctx = warden::solver::context(st, &TUNED);
        let scores = warden::solver::scores::patrol_scores(&mut ctx, &[7, 12]);
        let of = |n: usize| scores.iter().find(|s| s.0 == n).expect("scored").1;
        (of(7) - of(12), ctx.side_value)
    };
    let (plain, _) = gap(&st);
    st.tools = vec![Tool::Keys];
    let (keys, side) = gap(&st);
    // The Keys open their doors from the side: no "after opening" instrument.
    let want = 2.0 * data().tool(Tool::Keys).chance / 100.0 * TUNED.w_open * side;
    assert!((keys - plain - want).abs() < 1e-9, "{keys} {plain} {want}");
}

#[test]
fn the_short_shackles_price_a_short_move_only() {
    let mut st = duel(serde_json::json!([]), (Beast::Wolf, 2), 2);
    let short = data().tool(Tool::Shortchain);
    let max = short.max_steps.unwrap_or(0);
    assert!((2..5).contains(&max), "{max}");
    let at = |steps: usize| {
        st.sealed_cells().into_iter().find(|&n| steps_between(st.position, n) == steps).expect("a door that far")
    };
    let (near, far) = (at(2), at(5));
    let score = |st: &warden::engine::state::State, n: usize| {
        let mut ctx = warden::solver::context(st, &TUNED);
        (warden::solver::scores::patrol_scores(&mut ctx, &[n])[0].1, ctx.side_value)
    };
    let (near_plain, far_plain) = (score(&st, near).0, score(&st, far).0);
    st.tools = vec![Tool::Shortchain];
    let (near_held, side) = score(&st, near);
    let doors = short.count.unwrap_or(0).min(st.sealed_cells().len() - 1);
    let want = short.chance / 100.0 * TUNED.w_open * doors as f64 * side;
    assert!(want > 0.0 && (near_held - near_plain - want).abs() < 1e-9, "{near_held} {near_plain} {want}");
    assert!((score(&st, far).0 - far_plain).abs() < 1e-12);
}

#[test]
fn ravenous_chains_price_a_meal_beside_the_top_inmate() {
    // A Magic Wolf in cell 5 eats Magic meals in cells 9, 11 and 13; the
    // Legendary in cell 10 is the top inmate the Chains feed with 9 and 11.
    let mut st = duel(serde_json::json!([]), (Beast::Wolf, 1), 1);
    let meal = st.cells[9].inmate.expect("meal");
    for (n, id, tier) in [(10, 900, 3), (11, 901, 1), (13, 902, 1)] {
        st.cells[n].sealed = false;
        st.cells[n].inmate = Some(warden::engine::state::Inmate { id, tier, ..meal });
    }
    st.position = 5;
    assert_eq!(devour_options(&st, 5).iter().map(|o| o.cell).collect::<Vec<_>>(), [9, 11, 13]);
    let scores = |st: &warden::engine::state::State| {
        let mut ctx = warden::solver::context(st, &TUNED);
        warden::solver::scores::devour_scores(&mut ctx, devour_options(st, 5))
    };
    let plain = scores(&st);
    st.tools = vec![Tool::Chains];
    let held = scores(&st);
    let loss = |n: usize| {
        let of = |s: &[(usize, f64)]| s.iter().find(|s| s.0 == n).expect("scored").1;
        of(&plain) - of(&held)
    };
    // Eating cell 9 leaves the Legendary one Magic neighbour to remove and one
    // roll, against two of each; the meal in cell 13 changes nothing.
    let ctx = warden::solver::context(&st, &TUNED);
    let (v, r, p) = (ctx.v, ctx.r, data().tool(Tool::Chains).chance / 100.0);
    let one = p * (v[4] - v[3]);
    let two = 2.0 * p * (1.0 - p) * (v[4] - v[3]) + p * p * (r - v[3]);
    let want = two - one - v[1];
    assert!(loss(13).abs() < 1e-9 && (loss(9) - loss(11)).abs() < 1e-9, "{} {}", loss(9), loss(13));
    assert!((loss(9) - want).abs() < 1e-9, "{} {want}", loss(9));
}

#[test]
fn a_tusk_cell_outscores_the_same_cell_unmarked() {
    let mut st = duel(serde_json::json!(["tongs"]), (Beast::Wolf, 2), 2);
    let score = |st: &warden::engine::state::State| {
        let mut ctx = warden::solver::context(st, &TUNED);
        warden::solver::scores::patrol_scores(&mut ctx, &[9])[0].1
    };
    let plain = score(&st);
    st.cells[9].ivory = true;
    assert!(score(&st) > plain + 1e-6, "{plain}");
}

#[test]
fn a_rat_nest_due_in_the_run_scores_below_a_plain_door() {
    let mut st = duel(serde_json::json!(["ironbox"]), (Beast::Wolf, 2), 2);
    let score = |st: &warden::engine::state::State| {
        let mut ctx = warden::solver::context(st, &TUNED);
        warden::solver::scores::patrol_scores(&mut ctx, &[7])[0].1
    };
    let plain = score(&st);
    assert!(st.day < st.max_days);
    st.cells[7].nest = Some(warden::engine::state::Nest { due: st.day + 1 });
    assert!(score(&st) < plain - 1e-6, "{plain}");
}

#[test]
fn the_last_day_keeps_a_legendary_over_a_long_shot_chromatic() {
    // The warden's Legendary Wolf has no meal, and an Ultimate Lion sits in
    // 08: a new inmate may bite it into a Chromatic today. The Legendary's
    // Riot loot outweighs that chance on both days (loot playouts on day 20:
    // 2.250 for finishing against 2.214 for the Replace, paired error 0.001).
    let pick = |day: u32| {
        let mut st = replace_board(serde_json::json!([]), 3, &[(8, 4)]);
        st.day = day;
        assert!(devour_options(&st, 12).is_empty());
        decide(&st, &TUNED, &mut Rng32::new(1))
    };
    assert_eq!(pick(19), Some(Action::End));
    assert_eq!(pick(20), Some(Action::End));
}

#[test]
fn an_early_gift_opens_doors_over_raising_one_rare() {
    // Day 5, a Rare Wolf in the warden's cell with no target and every other
    // door sealed. Under 2.75^t tiers Wine (the Rare rises to Epic) outweighed
    // three doors; under 2^t the doors win.
    let mut m = raw("guillotine-last-day.json");
    m["day"] = serde_json::json!(5);
    m["tools"] = serde_json::json!([]);
    m["vault"] = serde_json::json!([]);
    for n in 1..16 {
        m["cells"][n] = serde_json::json!({"kind": "sealed", "event": null});
    }
    m["cells"][12] = serde_json::json!({"kind": "inmate", "beast": "wolf", "tier": 2});
    let model: Model = serde_json::from_value(m).expect("model");
    let mut st = build_state(&model, 1).0;
    assert!(devour_options(&st, 12).is_empty());
    st.pending = vec![Pending::Gift { options: vec![Gift::Ring, Gift::Wine], picks: 1 }];
    let ring = Action::Choose { id: ChoiceId::Name(Gift::Ring.key().to_string()) };
    assert_eq!(decide(&st, &TUNED, &mut Rng32::new(1)), Some(ring));
}

#[test]
fn an_extra_portion_marker_beats_a_plain_door() {
    // Day 5, three Rares on the block and every other door sealed. The
    // patrol picks a door over an empty cell; an Extra Portion there, which
    // raises inmates, turns the pick around.
    let d = data();
    let mut m = raw("guillotine-last-day.json");
    m["day"] = serde_json::json!(5);
    m["tools"] = serde_json::json!([]);
    m["vault"] = serde_json::json!([]);
    for n in 1..16 {
        m["cells"][n] = serde_json::json!({"kind": "sealed", "event": null});
    }
    for n in [2, 3, 4] {
        m["cells"][n] = serde_json::json!({"kind": "inmate", "beast": "wolf", "tier": 2});
    }
    let model: Model = serde_json::from_value(m).expect("model");
    let mut st = build_state(&model, 1).0;
    let free: Vec<usize> = (5..16).filter(|&n| n != d.room).take(2).collect();
    let (marker, door) = (free[0], free[1]);
    st.cells[marker].sealed = false;
    st.cells[marker].inmate = None;
    st.pending.clear();
    st.event = None;
    st.phase = Phase::Patrol;
    st.destinations = vec![marker, door];
    let plain = decide(&st, &TUNED, &mut Rng32::new(1));
    assert_eq!(plain, Some(Action::Patrol { cell: door }));
    st.cells[marker].event = Some(212);
    st.cells[marker].event_known = Some(true);
    assert_eq!(decide(&st, &TUNED, &mut Rng32::new(1)), Some(Action::Patrol { cell: marker }));
}

/// `eater_mask` agrees with `bites` on every cell of every board a few hundred
/// strategy games pass through, with the species instruments and Cage Fight.
#[test]
fn eater_mask_matches_bites() {
    let species = [Tool::Crown, Tool::Skewer, Tool::Guillotine, Tool::Maul];
    for k in 0..300 {
        let held: &[Tool] = if k % 3 == 0 { &[] } else { &species };
        let (mut st, mut roll) = start(11, k, held);
        if k % 5 == 0 {
            st.theme = Some(CAGE_FIGHT);
        }
        for _ in 0..2000 {
            let mask = eater_mask(&st);
            for n in 0..st.cells.len() {
                assert_eq!(mask & (1 << n) != 0, bites(&st, n).next().is_some(), "game {k} cell {n}");
            }
            if st.phase == Phase::Ended {
                break;
            }
            let Some(action) = decide(&st, &TUNED, &mut roll) else { break };
            if !apply(&mut st, &action).is_ok() && !(action != Action::End && apply(&mut st, &Action::End).is_ok()) {
                break;
            }
        }
    }
}
