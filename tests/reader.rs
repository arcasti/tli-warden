//! Held-out accuracy of the S16 board reader: templates built from the
//! even-numbered labeled frames are scored on the odd ones, then the reverse.
//! Skips when trainingdata/s16 (git-ignored) is missing.

use std::path::Path;
use warden::board::{CellModel, Model};
use warden::engine::data::Beast;
use warden::reader::model::{RUN_DAYS, TIER_CONF, to_model};
use warden::reader::panels::{Screen, read_panels};
use warden::reader::scene::{CELLS, FrameTruth, Scene, build_templates, load_labeled, read_scene, read_scene_with};
use warden::reader::{Rgba, load_png, read_model};

mod common;
use common::Tally;

const FIELDS: [&str; 7] = ["locked", "beast", "tier", "event", "warden", "jaws", "glow"];

#[test]
fn held_out_accuracy() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = common::frames();
    if !dir.is_dir() {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    let frames = load_labeled(&dir, &repo.join("data/scan-truth-s16.json")).unwrap();
    assert!(!frames.is_empty(), "no labeled frames in {}", dir.display());
    let mut tally: Vec<Tally> = FIELDS.iter().map(|_| Tally::default()).collect();
    let mut confusion = [[0usize; 5]; 5];
    let mut confident = Tally::default();
    for half in 0..2 {
        let (train, test): (Vec<_>, Vec<_>) = frames.iter().enumerate().partition(|(i, _)| i % 2 == half);
        let train: Vec<(&Rgba, &FrameTruth)> = train.iter().map(|(_, (img, t))| (img, t)).collect();
        let templates = build_templates(&train);
        for (_, (img, t)) in test {
            let scene = read_scene_with(img, &templates);
            let f = &t.name;
            assert_eq!(CELLS.filter(|&c| scene.cell(c).covered).count(), 0, "{f}: a tooltip read on a labeled frame");
            tally[4].check(scene.warden == t.warden, || format!("{f} warden read {:?} label {:?}", scene.warden, t.warden));
            for c in CELLS.filter(|c| !t.covered.contains(c)) {
                let (r, l) = (scene.cell(c), t.cell(c));
                if let Some(locked) = l.locked {
                    tally[0].check(r.locked == locked, || format!("{f} cell {c} locked read {} label {locked}", r.locked));
                    if locked && r.locked {
                        let glow = || format!("{f} cell {c} glow read {:?} label {:?} (conf {:.2})", r.glow, l.scratch, r.glow_conf);
                        tally[6].check(r.glow == l.scratch, glow);
                    }
                }
                let beast = r.inmate.map(|x| x.0);
                let label = l.inmate.map(|x| x.0);
                if label != Some(None) {
                    let label = label.flatten();
                    tally[1].check(beast == label, || format!("{f} cell {c} beast read {beast:?} label {label:?}"));
                }
                if let (Some((_, rt)), Some((_, lt))) = (r.inmate, l.inmate) {
                    tally[2].check(rt == lt, || format!("{f} cell {c} tier read {rt} label {lt} (conf {:.2})", r.tier_conf));
                    confusion[lt as usize][rt as usize] += 1;
                    if r.tier_conf >= TIER_CONF {
                        confident.check(rt == lt, String::new);
                    }
                }
                tally[3].check(r.event == l.event, || format!("{f} cell {c} event read {} label {}", r.event, l.event));
                if l.inmate.is_some() && t.screen == "board" {
                    tally[5].check(r.jaws == l.jaws, || format!("{f} cell {c} jaws read {} label {}", r.jaws, l.jaws));
                }
            }
        }
    }
    for (name, t) in FIELDS.iter().zip(&tally) {
        println!("{name:7} {:4}/{:4} {:5.1}%", t.ok, t.total, t.pct());
    }
    println!("tier confusion (rows label 0-4, columns read 0-4):");
    for (lt, row) in confusion.iter().enumerate() {
        println!("  {lt}: {row:?}");
    }
    let coverage = 100.0 * confident.total as f64 / tally[2].total.max(1) as f64;
    println!("tier at conf >= {TIER_CONF}: {}/{} {:.1}%, coverage {coverage:.1}%", confident.ok, confident.total, confident.pct());
    for (name, t) in FIELDS.iter().zip(&tally) {
        for m in &t.misses {
            println!("miss {name}: {m}");
        }
    }
    assert!(tally[0].pct() >= 98.0, "locked {:.1}%", tally[0].pct());
    assert!(tally[1].pct() >= 95.0, "beast {:.1}%", tally[1].pct());
    assert!(confident.pct() >= 90.0, "tier at conf >= {TIER_CONF}: {:.1}%", confident.pct());
    assert!(coverage >= 80.0, "tier coverage {coverage:.1}%");
    assert!(tally[4].pct() >= 90.0, "warden {:.1}%", tally[4].pct());
    assert!(tally[6].pct() >= 98.0, "glow {:.1}%", tally[6].pct());
}

/// The card under the mouse in rec4-t051 and rec4-t053 ("Go to Cell 11",
/// raised and dimmed, the cursor on its digits) reads with the other two.
/// Skips when the frames are missing.
#[test]
fn hovered_card_reads() {
    for name in ["rec4-t051.png", "rec4-t053.png"] {
        let path = common::frames().join(name);
        if !path.is_file() {
            println!("skipped: {} is missing", path.display());
            continue;
        }
        assert_eq!(read_panels(&load_png(&path).unwrap()).doors, [Some(8), Some(11), Some(13)], "{name}");
    }
}

/// trainingdata/reports/incident-cell8.png (outside s16, whose presence turns
/// on the labeled-frame tests): a player's screenshot (1911x1072) with the
/// Prison Incident "!" on the locked cell 8 and none on cell 7, the open
/// Normal Rat next to it. Skips when the frame is missing.
#[test]
fn side_cell_incident_reads() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("trainingdata/reports/incident-cell8.png");
    if !path.is_file() {
        println!("skipped: {} is missing", path.display());
        return;
    }
    let scene = read_scene(&load_png(&path).unwrap());
    assert!(scene.cell(8).event, "cell 8 incident");
    assert!(!scene.cell(7).event, "cell 7 has no incident");
}

/// trainingdata/reports/jaws-orange-*.png (outside s16): Legendary inmates
/// with the jaws crown on cells lit as reachable destinations (14 in the
/// first frame, 13 and 16 in the second) read Legendary, not Ultimate, like
/// the unlit ones next to them. Skips when a frame is missing.
#[test]
fn lit_destination_tiers() {
    let reports = Path::new(env!("CARGO_MANIFEST_DIR")).join("trainingdata/reports");
    let cases: [(&str, &[(u8, u8)]); 2] = [
        ("jaws-orange-cell14.png", &[(2, 4), (3, 2), (7, 1), (8, 4), (9, 1), (11, 3), (12, 0), (13, 1), (14, 3), (15, 3)]),
        ("jaws-orange-cells13-16.png", &[(5, 3), (6, 3), (13, 3), (16, 3)]),
    ];
    for (name, tiers) in cases {
        let path = reports.join(name);
        if !path.is_file() {
            println!("skipped: {} is missing", path.display());
            continue;
        }
        let scene = read_scene(&load_png(&path).unwrap());
        for &(c, tier) in tiers {
            assert_eq!(scene.cell(c).inmate.map(|i| i.1), Some(tier), "{name} cell {c} tier");
        }
    }
}

/// trainingdata/reports/chain-target.png (outside s16): a player's Consume
/// target pick with the eater, a Magic Lion, on cell 3 and the big red jaws
/// on its targets 4, 5, 12 and 13, not on the Magic Lion on 8 or the red glow
/// of the Ultimate on 16. Skips when the frame is missing.
#[test]
fn consume_targets_read() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("trainingdata/reports/chain-target.png");
    if !path.is_file() {
        println!("skipped: {} is missing", path.display());
        return;
    }
    let scene = read_scene(&load_png(&path).unwrap());
    let targets: Vec<u8> = CELLS.filter(|&c| scene.cell(c).target).collect();
    assert_eq!(targets, [4, 5, 12, 13]);
}

/// trainingdata/reports/tusk-cells7-9.png (outside s16): the Tusk of Iron
/// Tongs of Lust: Mammoth on the Mammoths in cells 7 and 9, not on the
/// unmarked Mammoth in cell 8. The board model carries it as ivory. Skips
/// when the frame is missing.
#[test]
fn tusk_reads() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("trainingdata/reports/tusk-cells7-9.png");
    if !path.is_file() {
        println!("skipped: {} is missing", path.display());
        return;
    }
    let scene = read_scene(&load_png(&path).unwrap());
    let tusks: Vec<u8> = CELLS.filter(|&c| scene.cell(c).tusk).collect();
    assert_eq!(tusks, [7, 9]);
    let (model, _) = read_model(&path, &[]).unwrap();
    let ivory: Vec<u8> = CELLS.filter(|&c| model.cells[c as usize - 1].ivory()).collect();
    assert_eq!(ivory, [7, 9]);
}

/// trainingdata/reports/consume-target-cell2.png (outside s16): a player's
/// Consume target pick on day 7 with the big jaws on the Ultimate Mammoth
/// Avenger's Casket let into cell 2: the cell reads open and a target.
/// Skips when the frame is missing.
#[test]
fn consume_target_reads_open() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("trainingdata/reports/consume-target-cell2.png");
    if !path.is_file() {
        println!("skipped: {} is missing", path.display());
        return;
    }
    let scene = read_scene(&load_png(&path).unwrap());
    let cell = scene.cell(2);
    assert!(cell.target, "cell 2 is not read as a target");
    assert!(!cell.locked, "cell 2 read as locked");
}

/// The big jaws read only on a Consume target pick: every other screen of
/// trainingdata/s16 has none. Skips when the frames are missing.
#[test]
fn consume_targets_only_on_target_pick() {
    let dir = common::frames();
    if !dir.is_dir() {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "png") {
            continue;
        }
        let img = load_png(&path).unwrap();
        if read_panels(&img).screen == Screen::ChainTarget {
            continue;
        }
        let scene = read_scene(&img);
        let targets: Vec<u8> = CELLS.filter(|&c| scene.cell(c).target).collect();
        assert!(targets.is_empty(), "{}: targets {targets:?}", path.display());
        seen += 1;
    }
    assert!(seen > 0, "no frames in {}", dir.display());
}

/// rec6-t003: the run theme scroll in the lobby names Opulence; the manage
/// board of rec6-t181 and the treasure pick of rec6-t191 name no theme.
/// Skips when a frame is missing.
#[test]
fn theme_scroll_reads() {
    for (name, screen, theme) in [("rec6-t003.png", Screen::Theme, Some(302)), ("rec6-t181.png", Screen::Manage, None), ("rec6-t191.png", Screen::TreasurePick, None)] {
        let path = common::frames().join(name);
        if !path.is_file() {
            println!("skipped: {} is missing", path.display());
            continue;
        }
        let p = read_panels(&load_png(&path).unwrap());
        assert_eq!((p.screen, p.theme), (screen, theme), "{name}");
    }
}

/// The objective panel: Survival of the Fittest "Completed" in
/// trainingdata/reports/objective-completed.png (a 1911x1072 frame), and
/// Undercurrents at 0/2 on the rec1-t123 board. Skips when a frame is missing.
#[test]
fn objective_panel_reads() {
    let reports = Path::new(env!("CARGO_MANIFEST_DIR")).join("trainingdata/reports");
    for (path, objective) in [(reports.join("objective-completed.png"), (2, 4)), (common::frames().join("rec1-t123.png"), (4, 0))] {
        if !path.is_file() {
            println!("skipped: {} is missing", path.display());
            continue;
        }
        assert_eq!(read_panels(&load_png(&path).unwrap()).objective, Some(objective), "{}", path.display());
    }
}

/// The Rusty Saw tooltip of rec3-t161 lies over cells 15 and 16 (16 empty, 15
/// a Legendary Rat): both are marked covered and taken from the last board
/// read when there is one. Skips when the frame is missing.
#[test]
fn tooltip_covers_cells() {
    let dir = common::frames();
    let (tip, plain) = (dir.join("rec3-t161.png"), dir.join("rec1-t029.png"));
    if !tip.is_file() || !plain.is_file() {
        println!("skipped: {} or {} is missing", tip.display(), plain.display());
        return;
    }
    let covered = |scene: &Scene| CELLS.filter(|&c| scene.cell(c).covered).collect::<Vec<_>>();
    assert_eq!(covered(&read_scene(&load_png(&plain).unwrap())), Vec::<u8>::new());

    let img = load_png(&tip).unwrap();
    let scene = read_scene(&img);
    assert_eq!(covered(&scene), vec![15, 16]);

    let panels = read_panels(&img);
    let (_, warnings) = to_model(&scene, &panels, &[], None, RUN_DAYS);
    let texts: Vec<&str> = warnings.iter().map(|w| w.text.as_str()).collect();
    assert!(texts.contains(&"Check cell 15: a tooltip covers it."), "{texts:?}");
    assert!(texts.contains(&"Check cell 16: a tooltip covers it."), "{texts:?}");

    let mut under = Model::blank();
    under.cells[14] = CellModel::Inmate { beast: Beast::Rat, tier: 3, ivory: false, prey: None, event: None };
    under.cells[15] = CellModel::Empty { event: None };
    let (m, warnings) = to_model(&scene, &panels, &[], Some(&under), RUN_DAYS);
    assert_eq!(m.cells[14..16], under.cells[14..16]);
    let texts: Vec<&str> = warnings.iter().map(|w| w.text.as_str()).collect();
    assert!(texts.contains(&"A tooltip covers cells 15 and 16: they are kept from the last board read."), "{texts:?}");
    assert!(!texts.iter().any(|t| t.starts_with("Check cell 15") || t.starts_with("Check cell 16")), "{texts:?}");
}

/// rec4-t412: the Raging Beast's Agonizing Muzzle tooltip over the left
/// column, its flat grey tinted in blocks by the recording's compression,
/// hides the Lions of cells 15 and 16 and cell 14's portrait above its
/// padlock: the three are covered and kept from the last board read, so no
/// cell reads empty under it.
#[test]
fn tooltip_in_a_recorded_frame() {
    let tip = common::frames().join("rec4-t412.png");
    if !tip.is_file() {
        println!("skipped: {} is missing", tip.display());
        return;
    }
    let img = load_png(&tip).unwrap();
    let scene = read_scene(&img);
    assert_eq!(CELLS.filter(|&c| scene.cell(c).covered).collect::<Vec<_>>(), vec![14, 15, 16]);

    let lion = CellModel::Inmate { beast: Beast::Lion, tier: 0, ivory: false, prey: None, event: None };
    let mut under = Model::blank();
    under.cells[14] = lion.clone();
    under.cells[15] = lion;
    let (m, warnings) = to_model(&scene, &read_panels(&img), &[], Some(&under), RUN_DAYS);
    assert_eq!(m.cells[13..16], under.cells[13..16]);
    let texts: Vec<&str> = warnings.iter().map(|w| w.text.as_str()).collect();
    assert!(texts.contains(&"A tooltip covers cells 14, 15 and 16: they are kept from the last board read."), "{texts:?}");
}

/// The day counts down from the run's length, and a counter above a 20-day
/// run's length is an Iron Law run of 30 days (rec1-t029's board, its
/// counter overridden).
#[test]
fn iron_law_days() {
    let path = common::frames().join("rec1-t029.png");
    if !path.is_file() {
        println!("skipped: {} is missing", path.display());
        return;
    }
    let img = load_png(&path).unwrap();
    let scene = read_scene(&img);
    let mut panels = read_panels(&img);
    for (run_days, left, day, max_days) in [(20, 29, 1, 30), (20, 25, 5, 30), (30, 12, 18, 30), (20, 12, 8, 20)] {
        panels.days = Some(left);
        let (m, _) = to_model(&scene, &panels, &[], None, run_days);
        assert_eq!((m.day, m.max_days), (day, max_days), "run of {run_days}, {left} days left");
    }
}

/// Every saved frame: only the loading screen of rec4-t424 and the day
/// banners of rec1-t095 and rec4-t340 are off the board, and only the riot
/// screen of rec4-t422 ends the run. The red Consume target screens of rec4
/// (the days counter hidden) are not off the board. Only rec5-t018,
/// rec5-t047 and rec6-t206 are View Prison Status; the treasure picks around
/// them are not. Frames that are missing are skipped.
#[test]
fn off_board_and_run_end() {
    let dir = common::frames();
    if !dir.is_dir() {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.ends_with(".png")).collect();
    names.sort();
    let (mut off, mut ended, mut status) = (Vec::new(), Vec::new(), Vec::new());
    for name in &names {
        let p = read_panels(&load_png(&dir.join(name)).unwrap());
        if p.off_board() {
            off.push(name.as_str());
        }
        if p.screen == Screen::RunEnd {
            ended.push(name.as_str());
        }
        if p.screen == Screen::PrisonStatus {
            status.push(name.as_str());
        }
        if ["rec5-t012.png", "rec5-t028.png", "rec5-t045.png"].contains(&name.as_str()) {
            assert_eq!(p.screen, Screen::TreasurePick, "{name}");
        }
        if ["rec4-t110.png", "rec4-t148.png", "rec4-t166.png", "rec4-t258.png", "rec4-t390.png"].contains(&name.as_str()) {
            assert_eq!(p.screen, Screen::ChainTarget, "{name}");
        }
    }
    println!("off the board: {off:?}");
    let present = |list: &[&'static str]| list.iter().copied().filter(|n| names.iter().any(|m| m == n)).collect::<Vec<_>>();
    assert_eq!(off, present(&["rec1-t095.png", "rec4-t340.png", "rec4-t424.png"]));
    assert_eq!(ended, present(&["rec4-t422.png"]));
    assert_eq!(status, present(&["rec5-t018.png", "rec5-t047.png", "rec6-t206.png"]));
}
