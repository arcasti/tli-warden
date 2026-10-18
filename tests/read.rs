//! The S16 panel reader (Windows OCR) on the labeled frames: screen, doors,
//! decision buttons, days and offers against data/scan-truth-s16.json, then
//! every board frame with cards or buttons through `to_model`, `build_state`
//! and a short solve. Skips when trainingdata/s16 (git-ignored) is missing.

use serde_json::Value;
use std::path::Path;
use warden::board::build_state;
use warden::engine::data::Tool;
use warden::reader::load_png;
use warden::reader::model::to_model;
use warden::reader::panels::read_panels;
use warden::reader::scene::read_scene;
use warden::solver::TUNED;
use warden::solver::solve::solve;

mod common;
use common::Tally;

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

#[test]
fn panels_against_labels() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = common::frames();
    if !dir.is_dir() {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    let labels: Value = serde_json::from_str(&std::fs::read_to_string(repo.join("data/scan-truth-s16.json")).unwrap()).unwrap();
    let names = ["screen", "doors", "decision", "days", "offers", "chromatic", "board days", "board doors"];
    let mut tally: Vec<Tally> = names.iter().map(|_| Tally::default()).collect();
    let mut solved = 0;
    for (f, l) in labels.as_object().unwrap().iter().filter(|(k, _)| k.ends_with(".png")) {
        let img = load_png(&dir.join(f)).unwrap();
        let p = read_panels(&img);
        let screen = l["screen"].as_str().unwrap();
        let board = screen == "board";
        tally[0].check(p.screen.label() == screen, || format!("{f} screen read {:?} label {screen}", p.screen));

        let doors: Vec<u8> = l["doors"].as_array().unwrap().iter().map(|d| d.as_u64().unwrap() as u8).collect();
        let read_doors: Vec<Option<u8>> = p.doors.clone();
        let ok = read_doors == doors.iter().map(|&d| Some(d)).collect::<Vec<_>>();
        tally[1].check(ok, || format!("{f} doors read {read_doors:?} label {doors:?}"));
        if board && !doors.is_empty() {
            tally[7].check(ok, String::new);
        }

        let decision = strings(&l["decision"]);
        tally[2].check(p.decision == decision, || format!("{f} decision read {:?} label {decision:?}", p.decision));

        if let Some(days) = l["days"].as_u64() {
            let ok = p.days == Some(days as u32);
            tally[3].check(ok, || format!("{f} days read {:?} label {days}", p.days));
            if board {
                tally[6].check(ok, || format!("{f} days read {:?} label {days}", p.days));
            }
        }

        let offers = strings(&l["offers"]);
        tally[4].check(p.offers == offers, || format!("{f} offers read {:?} label {offers:?}", p.offers));

        let chromatic = l["chromatic"].as_u64().unwrap() as usize;
        tally[5].check(p.vault.len() == chromatic, || format!("{f} chromatic read {} label {chromatic}", p.vault.len()));

        // End to end: a board frame with cards or buttons, or a Sewer Ratway
        // cell pick, is solvable.
        let phase = l["phase"].as_str().unwrap();
        let cell_pick = screen == "cellPick";
        if board && (phase == "manage" || !doors.is_empty()) || cell_pick {
            let held: Vec<Tool> = strings(&l["tools"]).iter().filter_map(|t| Tool::from_key(t)).collect();
            let (model, warnings) = to_model(&read_scene(&img), &p, &held, None);
            let (st, problems) = build_state(&model, 1);
            assert!(problems.is_empty(), "{f}: {problems:?} (warnings {warnings:?})");
            let options = solve(&st, 20, 1, &TUNED);
            assert!(!options.is_empty(), "{f}: no option");
            if cell_pick {
                assert!(options.iter().all(|o| o.label.starts_with("Move to ")), "{f}: {:?}", options[0].label);
            }
            solved += 1;
        }
    }

    for (name, t) in names.iter().zip(&tally) {
        println!("{name:<12} {:>3}/{:>3} {:5.1}%", t.ok, t.total, t.pct());
    }
    for t in &tally[..6] {
        for m in &t.misses {
            println!("  miss: {m}");
        }
    }
    println!("end to end: {solved} board frames read, built and solved");
    assert_eq!(tally[6].ok, tally[6].total, "days on board frames");
    assert_eq!(tally[7].ok, tally[7].total, "doors on board frames");
    assert!(tally[0].pct() >= 95.0, "screen {:.1}%", tally[0].pct());
    assert!(solved > 0);
}
