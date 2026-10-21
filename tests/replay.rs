//! Replay on hand-built boards: a deterministic move explains the next
//! board, a changed tier does not, and the log reader keeps only new reads.

use warden::board::{CellModel, EventMark, Model, Step};
use warden::engine::data::Beast;
use warden::replay::{Field, Mismatch, Report, Verdict, explain, parse_log};

/// Day 3 on patrol from the office, today's door cell 03 with a Rare Mammoth,
/// and an incident marker on the sealed door at cell 11: the engine keeps at
/// least one marker on the board.
fn before() -> Model {
    let mut m = Model::blank();
    m.day = 3;
    m.step = Step::Patrol;
    m.dest = vec![2];
    m.cells[2] = CellModel::Inmate { beast: Beast::Mammoth, tier: 1, ivory: false, prey: None, event: None };
    m.cells[10] = CellModel::Sealed { event: Some(EventMark::Chain) };
    m
}

/// The board after the patrol to cell 03.
fn after() -> Model {
    let mut m = before();
    m.step = Step::Manage;
    m.dest = Vec::new();
    m.warden = 2;
    m
}

#[test]
fn a_patrol_explains_the_next_board() {
    let Verdict::Best(e) = explain(&before(), &after(), 4) else { panic!("no explanation") };
    assert!(e.mismatches.is_empty(), "{:?}", e.mismatches);
    assert!(e.label.starts_with("Go to cell 03"), "{}", e.label);
}

#[test]
fn a_door_opens_onto_anyone() {
    let mut a = before();
    a.dest = vec![5];
    let mut b = a.clone();
    b.step = Step::Manage;
    b.dest = Vec::new();
    b.warden = 5;
    b.cells[5] = CellModel::Inmate { beast: Beast::Lion, tier: 3, ivory: false, prey: None, event: None };
    let Verdict::Best(e) = explain(&a, &b, 4) else { panic!("no explanation") };
    assert!(e.mismatches.is_empty(), "{:?}", e.mismatches);
    assert_eq!(e.reveals, vec![3]);
}

#[test]
fn a_changed_tier_is_unexplained() {
    let mut b = after();
    b.cells[2] = CellModel::Inmate { beast: Beast::Mammoth, tier: 2, ivory: false, prey: None, event: None };
    let Verdict::Best(e) = explain(&before(), &b, 4) else { panic!("no explanation") };
    assert_eq!(e.mismatches.len(), 1, "{:?}", e.mismatches);
    assert!(matches!(&e.mismatches[0], Mismatch::Cell { cell: 2, field: Field::Tier, .. }));
    assert_eq!(e.mismatches[0].kind(), "cell tier");
}

#[test]
fn identical_boards_are_unchanged() {
    assert_eq!(explain(&before(), &before(), 4), Verdict::Unchanged);
}

fn line(kind: &str, at: &str, m: &Model) -> String {
    serde_json::json!({ "at": at, "kind": kind, "screen": "Manage", "model": m }).to_string()
}

#[test]
fn the_log_keeps_new_reads_only() {
    let text = [
        line("read", "t1", &before()),
        line("miss", "t2", &after()),
        line("read", "t3", &before()),
        "not json".to_string(),
        line("read", "t4", &after()),
        line("read", "t5", &after()),
    ]
    .join("\n");
    let reads = parse_log(&text);
    let ats: Vec<&str> = reads.iter().map(|r| r.at.as_str()).collect();
    assert_eq!(ats, ["t1", "t4"]);
    assert_eq!(reads[1].model, after());

    let mut report = Report::default();
    report.add_log("log.jsonl", &reads, 4);
    assert_eq!((report.transitions, report.skipped, report.explained, report.unexplained.len()), (1, 0, 1, 0));
}
