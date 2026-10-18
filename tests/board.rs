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
        if let Some(diff) = first_diff("state", &normalize(rust), &normalize(js["state"].clone())) {
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
