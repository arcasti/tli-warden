//! The app session on a saved frame: the reading matches `read_model`, the
//! solve run in rounds on the worker matches one `solve` call, and cell edits
//! and held instruments change the model and re-solve. Skips when
//! trainingdata/s16 (git-ignored) is missing.

use std::path::Path;
mod common;
use warden::app::runlog::RunLog;
use warden::app::session::{CellEdit, casket_meals, consume_targets, DaysClock, HELD_BLOCKS, MAX_WAIT, RECHECK, ROUND, Reading, SETTLE, Session, Watch, differs, fingerprint};
use warden::board::{CellModel, ChoiceKind, EventMark, Model, PreyMark, Step, build_state};
use warden::engine::data::{Beast, IRON_LAW, OPULENCE, Tool};
use warden::engine::devour::{devour_options, start_options};
use warden::engine::{Action, ChoiceId};
use warden::reader::{Rgba, load_png, read_model};
use warden::reader::model::WarningKind;
use warden::reader::panels::Screen;
use warden::solver::TUNED;
use warden::solver::solve::{Rollouts, solve};

const ROLLOUTS: usize = 400;

#[test]
fn session_on_t087() {
    let frame = common::frames().join("rec1-t087.png");
    if !frame.is_file() {
        println!("skipped: {} is missing", frame.display());
        return;
    }
    // The session starts with the list the instrument panel shows: its read adds no line.
    let mut s = Session::new(ROLLOUTS, vec![Tool::Coin], None, None);
    s.read_in_background(load_png(&frame).unwrap());
    s.wait();
    let (model, warnings) = read_model(&frame, &[]).unwrap();
    let reading = s.reading.clone().unwrap();
    assert_eq!(reading.model, model);
    assert_eq!(reading.warnings, warnings);
    assert!(s.problems.is_empty(), "{:?}", s.problems);
    assert!(s.pick.is_some());

    // The worker's rounds add up to one solve call with the same rollouts.
    s.wait();
    assert_eq!(s.rounds, ROLLOUTS);
    let (st, _) = build_state(&model, 1);
    let whole = solve(&st, ROLLOUTS, 1, &TUNED);
    assert!(whole.len() > 1);
    assert_eq!(s.ranking, whole);
    println!("ranking: {} options, leader {} {:.3}", whole.len(), whole[0].label, whole[0].mean);

    // A partial ranking ranks the rounds so far.
    let mut r = Rollouts::new(&st, 1, &TUNED);
    r.run(ROUND);
    assert!(r.ranking().iter().all(|o| o.n <= ROUND));
    assert_eq!(r.ranking(), solve(&st, ROUND, 1, &TUNED));

    // Edit a locked door into a Wolf.
    let n = model.cells.iter().position(|c| matches!(c, CellModel::Sealed { .. })).expect("a locked cell");
    s.edit_cell(n as u8 + 1, CellEdit::Inmate { beast: Beast::Wolf, tier: 2 }).unwrap();
    let edited = s.reading.clone().unwrap().model;
    assert_eq!(edited.cells[n], CellModel::Inmate { beast: Beast::Wolf, tier: 2, ivory: false, prey: None, event: model.cells[n].event() });
    assert_ne!(edited, model);
    s.wait();
    assert!(s.problems.is_empty(), "{:?}", s.problems);
    assert!(!s.ranking.is_empty());
    assert!(s.edit_cell(1, CellEdit::Empty).is_err(), "the Interrogation Room is not a cell to edit");

    // Held instruments go into the built state; the edit stays.
    let held = vec![Tool::ALL[0]];
    s.set_tools(held.clone());
    let with_tools = s.reading.clone().unwrap().model;
    assert_eq!(with_tools.tools, held);
    assert_eq!(with_tools.cells[n], edited.cells[n]);
    assert_ne!(build_state(&with_tools, 1).0.tools, build_state(&edited, 1).0.tools);
    s.wait();
    assert!(!s.ranking.is_empty());
    println!("warnings: {}", reading.warnings.len());

    // A read of the same scene keeps the edit and the solve: no restart.
    let at = reading.at;
    assert_eq!(s.reading.as_ref().unwrap().at, at, "corrections keep the read time");
    s.set_tools(model.tools.clone());
    s.wait();
    let (kept, rounds) = (s.ranking.clone(), s.rounds);
    s.read_in_background(load_png(&frame).unwrap());
    while s.scanning() {
        s.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!s.solving);
    assert_eq!((s.rounds, &s.ranking), (rounds, &kept));
    let same = s.reading.clone().unwrap();
    assert_eq!(same.model, edited);
    assert_eq!(same.warnings, warnings);
    assert!(same.at > at);

    // Another frame drops the edit and restarts the solve.
    let pick = frame.with_file_name("rec1-t031.png");
    s.read_in_background(load_png(&pick).unwrap());
    s.wait();
    assert_ne!(s.reading.as_ref().unwrap().model, edited);
    assert_ne!(s.ranking, kept);

    // Back on the board: the reading without the edit, solved again.
    let at = same.at;
    s.read_in_background(load_png(&frame).unwrap());
    assert!(s.scanning());
    s.wait();
    assert!(!s.scanning());
    // The panel's list stands over the pick's advised instrument; setting it
    // drops the panel's line, and the board is as before.
    assert_eq!(s.tools, model.tools);
    s.set_tools(model.tools.clone());
    s.wait();
    let again = s.reading.clone().unwrap();
    assert_eq!(again.model, model);
    assert_eq!(again.warnings, warnings);
    assert_eq!((again.screen, again.days_left), (reading.screen, reading.days_left));
    assert!(again.at > at);
    assert_eq!(s.rounds, ROLLOUTS);
    assert_eq!(s.ranking, whole);
}

/// The player adds a missed incident marker, keeps it through a kind
/// correction and takes it off again.
#[test]
fn marker_correction() {
    let frame = common::frames().join("rec1-t087.png");
    if !frame.is_file() {
        println!("skipped: {} is missing", frame.display());
        return;
    }
    let mut s = Session::new(20, Vec::new(), None, None);
    s.read_in_background(load_png(&frame).unwrap());
    s.wait();
    let model = s.reading.clone().unwrap().model;
    let n = model.cells.iter().position(|c| matches!(c, CellModel::Sealed { event: None })).expect("a locked cell without a marker");
    let cell = |s: &Session| s.reading.as_ref().unwrap().model.cells[n].clone();
    s.set_marker(n as u8 + 1, true).unwrap();
    assert_eq!(cell(&s), CellModel::Sealed { event: Some(EventMark::Unknown) });
    s.edit_cell(n as u8 + 1, CellEdit::Empty).unwrap();
    assert_eq!(cell(&s), CellModel::Empty { event: Some(EventMark::Unknown) });
    s.edit_cell(n as u8 + 1, CellEdit::Locked).unwrap();
    assert_eq!(cell(&s), CellModel::Sealed { event: Some(EventMark::Unknown) });
    s.set_marker(n as u8 + 1, false).unwrap();
    assert_eq!(cell(&s), CellModel::Sealed { event: None });
    assert!(s.set_marker(1, true).is_err(), "the Interrogation Room takes no marker");
}

/// A live frame of another 16:9 size reads as the reference size does; a
/// window narrower than 16:9 is not read and says why, then keeps the
/// reading on show with the same line.
#[test]
fn frame_size_is_scaled_or_refused() {
    let Some(reference) = read_all(Vec::new(), &["rec1-t087.png"]) else { return };
    let frame = load_png(&common::frames().join("rec1-t087.png")).unwrap();
    let mut s = Session::new(20, Vec::new(), None, None);
    let read = |s: &mut Session, w, h| {
        s.read_in_background(frame.resize(w, h));
        s.wait();
    };
    read(&mut s, 1440, 1080);
    let line = "The game window is 1440 x 1080, narrower than 16:9";
    assert!(s.reading.is_none());
    assert!(s.missed.as_deref().is_some_and(|m| m.starts_with(line)), "{:?}", s.missed);

    read(&mut s, 2560, 1440);
    let wide = s.reading.clone().unwrap();
    assert_eq!(wide.model.cells, reference.reading.as_ref().unwrap().model.cells);
    assert!(wide.warnings.iter().all(|w| !w.text.contains("16:9")), "{:?}", wide.warnings);
    assert!(wide.kept_lines().is_empty());

    read(&mut s, 1440, 1080);
    let kept = s.reading.clone().unwrap();
    assert_eq!(kept.model.cells, wide.model.cells);
    assert!(kept.warnings[0].text.starts_with(line), "{:?}", kept.warnings);
    let lines = kept.kept_lines();
    assert!(lines.len() == 1 && lines[0].starts_with(line) && lines[0].ends_with("that is not maximized."), "{lines:?}");

    // A frame that reads again drops the line.
    read(&mut s, 1920, 1080);
    assert!(s.reading.as_ref().unwrap().kept_lines().is_empty());
}

/// The auto read fingerprint: the same frame matches, a changed region or
/// another grid size differs.
#[test]
fn fingerprint_differs() {
    let (w, h) = (1280, 720);
    let mut data = vec![0u8; w * h * 4];
    for (i, p) in data.chunks_mut(4).enumerate() {
        let (x, y) = (i % w, i / w);
        p.copy_from_slice(&[(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255]);
    }
    let frame = Rgba { w, h, data };
    let print = fingerprint(&frame);
    assert_eq!(print.len(), 64 * 36);
    assert!(!differs(&print, &fingerprint(&frame.clone())));

    // A 120 x 90 white patch, about the size of a cell.
    let mut changed = frame.clone();
    for y in 300..390 {
        for x in 600..720 {
            changed.data[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&[255, 255, 255]);
        }
    }
    assert!(differs(&print, &fingerprint(&changed)));
    assert!(differs(&print, &print[..print.len() - 1]));
}

/// A print of all `gray`, with the first `changed` blocks at 255.
fn print(gray: u8, changed: usize) -> Vec<u8> {
    let mut p = vec![gray; 64 * 36];
    p[..changed].fill(255);
    p
}

/// Auto read waits for [`SETTLE`] matching samples, reads a screen once and
/// ignores a hover-sized change.
#[test]
fn watch_reads_settled_screens_once() {
    let mut w = Watch::default();
    // The screen keeps changing: no read.
    for g in [0, 50, 100, 150, 200] {
        assert!(!w.sample(print(g, 0)));
    }
    // It settles: read after SETTLE samples matching the one before.
    for _ in 0..SETTLE - 1 {
        assert!(!w.sample(print(200, 0)));
    }
    assert!(w.sample(print(200, 0)));
    // A hover glow under the threshold: no second read.
    for _ in 0..5 {
        assert!(!w.sample(print(200, 0)));
        assert!(!w.sample(print(200, 2)));
    }
    // The same screen again, past RECHECK samples: never read again.
    assert!((0..RECHECK + 20).all(|_| !w.sample(print(200, 0))));
    // Another screen: read once it settles, after SETTLE samples.
    let reads: Vec<bool> = (0..SETTLE + 3).map(|_| w.sample(print(10, 0))).collect();
    assert_eq!(reads.iter().position(|&r| r), Some(SETTLE), "{reads:?}");
    assert_eq!(reads.iter().filter(|&&r| r).count(), 1);
}

/// A lasting change under the threshold, such as an incident marker on a cell,
/// is read once [`RECHECK`] samples passed since the last read, then not again.
#[test]
fn watch_rechecks_a_small_change() {
    let mut w = Watch::default();
    assert_eq!((0..SETTLE + 1).filter(|_| w.sample(print(200, 0))).count(), 1);
    assert!(!differs(&print(200, 0), &print(200, 2)));
    let reads: Vec<bool> = (0..RECHECK).map(|_| w.sample(print(200, 2))).collect();
    assert_eq!(reads.iter().position(|&r| r), Some(RECHECK - 1), "{reads:?}");
    assert_eq!(reads.iter().filter(|&&r| r).count(), 1);
    assert!((0..40).all(|_| !w.sample(print(200, 2))));
}

/// A screen marked read, or the sample after `adopt_next`, is not read again.
#[test]
fn watch_mark_read_suppresses() {
    let mut w = Watch::default();
    w.mark_read(print(80, 0));
    assert!((0..SETTLE + 3).all(|_| !w.sample(print(80, 0))));

    let mut w = Watch::default();
    w.adopt_next();
    assert!((0..SETTLE + 3).all(|_| !w.sample(print(30, 0))));
    assert_eq!((0..SETTLE + 3).filter(|_| w.sample(print(90, 0))).count(), 1);
}

/// The sample after `read_next` is read whatever it shows, once.
#[test]
fn watch_read_next_reads_once() {
    let mut w = Watch::default();
    w.mark_read(print(80, 0));
    assert!(!w.sample(print(80, 0)));
    w.read_next();
    assert!(w.sample(print(80, 0)));
    assert!((0..SETTLE + 3).all(|_| !w.sample(print(80, 0))));
}

/// A print of all `gray`, with `n` blocks at 255 starting at block `from`.
fn noisy(gray: u8, from: usize, n: usize) -> Vec<u8> {
    let mut p = vec![gray; 64 * 36];
    p[from..from + n].fill(255);
    p
}

/// A new screen with an animation that moves up to [`HELD_BLOCKS`] blocks a
/// sample settles and is read; the next screen change is read too.
#[test]
fn watch_reads_a_noisy_screen() {
    // The animation's 12 blocks make two prints differ but hold.
    assert!(differs(&noisy(120, 0, 6), &noisy(120, 6, 6)));
    const { assert!(12 <= HELD_BLOCKS) };
    let mut w = Watch::default();
    assert_eq!((0..SETTLE + 1).filter(|_| w.sample(print(40, 0))).count(), 1);
    // A screen change, then an animation that moves 2 x 6 blocks every sample.
    assert!(!w.sample(print(120, 0)));
    let reads: Vec<bool> = (0..SETTLE).map(|k| w.sample(noisy(120, 6 * (k % 2), 6))).collect();
    assert_eq!(reads.last(), Some(&true), "{reads:?}");
    // Another screen, with the same animation: read once it settles.
    assert!(!w.sample(noisy(200, 0, 6)));
    let reads: Vec<bool> = (0..SETTLE).map(|k| w.sample(noisy(200, 6 * (k % 2), 6))).collect();
    assert_eq!(reads.last(), Some(&true), "{reads:?}");
}

/// A screen whose edges pulse by more than [`HELD_BLOCKS`] blocks every
/// sample never settles: it is read after [`MAX_WAIT`] samples anyway.
#[test]
fn watch_reads_a_pulsing_screen_after_max_wait() {
    let mut w = Watch::default();
    w.mark_read(print(40, 0));
    let reads: Vec<bool> = (0..MAX_WAIT + 1).map(|k| w.sample(noisy(120, 40 * (k % 2), 40))).collect();
    assert_eq!(reads.iter().position(|&r| r), Some(MAX_WAIT), "{reads:?}");
}

/// An instrument pick covers the board: its cells come from the last board
/// read, corrections included, and never from the card art.
#[test]
fn pick_keeps_the_board_under_it() {
    let dir = common::frames();
    let (board, pick) = (dir.join("rec1-t087.png"), dir.join("rec1-t031.png"));
    if !board.is_file() || !pick.is_file() {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    // Without a board read first: the run start, every cell locked.
    let (alone, _) = read_model(&pick, &[]).unwrap();
    assert!(alone.cells.iter().skip(1).all(|c| matches!(c, CellModel::Sealed { .. })), "{:?}", alone.cells);

    let mut s = Session::new(20, Vec::new(), None, None);
    s.read_in_background(load_png(&board).unwrap());
    s.wait();
    let n = s.reading.as_ref().unwrap().model.cells.iter().position(|c| matches!(c, CellModel::Sealed { .. })).expect("a locked cell");
    s.edit_cell(n as u8 + 1, CellEdit::Inmate { beast: Beast::Wolf, tier: 2 }).unwrap();
    let under = s.reading.clone().unwrap().model;

    s.read_in_background(load_png(&pick).unwrap());
    s.wait();
    let r = s.reading.clone().unwrap();
    assert_eq!(r.model.cells, under.cells);
    assert_eq!((r.model.warden, r.model.day), (under.warden, under.day));
    assert_eq!(r.model.choice.options.len(), 3);
    assert!(r.warnings.iter().all(|w| w.cell.is_none()), "{:?}", r.warnings);
    assert!(s.problems.is_empty(), "{:?}", s.problems);
}

/// The days clock: the counter only counts down, so a lone higher read is a
/// misread and the same higher value on two readings in a row is a new run.
#[test]
fn days_clock() {
    let mut c = DaysClock::default();
    assert_eq!(c.update(None), None, "nothing read yet");
    assert_eq!(c.update(Some(14)), Some(14));
    assert_eq!(c.update(None), Some(14), "a hidden counter keeps the count");
    assert_eq!(c.update(Some(14)), Some(14));
    assert_eq!(c.update(Some(12)), Some(12));
    assert_eq!(c.update(Some(15)), Some(12), "a lone higher read");
    assert_eq!(c.update(Some(11)), Some(11));
    assert_eq!(c.update(Some(20)), Some(11));
    assert_eq!(c.update(None), Some(11), "a missing read breaks the pair");
    assert_eq!(c.update(Some(20)), Some(11));
    assert_eq!(c.update(Some(5)), Some(5), "a different value breaks the pair");
    assert_eq!(c.update(Some(20)), Some(5));
    assert_eq!(c.update(Some(20)), Some(20), "the same higher read twice: a new run");
    assert_eq!(c.update(Some(0)), Some(0));
}

/// A Consume target pick hides the days counter and does not replace the
/// board; the treasure pick after it keeps the day of the last board read
/// and covers the board as the chain left it.
#[test]
fn hidden_counter_keeps_the_day() {
    let dir = common::frames();
    let frames = ["rec1-t071.png", "rec1-t073.png", "rec1-t085.png"].map(|f| dir.join(f));
    if frames.iter().any(|f| !f.is_file()) {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    let mut s = Session::new(20, Vec::new(), None, None);
    let mut read = |f: &Path| {
        s.read_in_background(load_png(f).unwrap());
        s.wait();
        s.reading.clone().unwrap()
    };
    let board = read(&frames[0]);
    assert_eq!((board.screen, board.days_left), (Screen::Manage, Some(16)));
    for f in &frames[1..] {
        let r = read(f);
        assert_eq!((r.days_left, r.model.day), (Some(16), board.model.day), "{}", f.display());
        // The Consume target screen always hides the counter, so only the
        // treasure pick says so.
        let kept = r.warnings.iter().any(|w| w.text.contains("16 day(s) until Riot kept from the last read"));
        assert_eq!(kept, r.screen != Screen::ChainTarget, "{:?}", r.warnings);
        assert!(r.warnings.iter().all(|w| !w.text.contains("day 1 assumed")), "{:?}", r.warnings);
    }
    let pick = s.reading.clone().unwrap();
    assert_eq!(pick.screen, Screen::TreasurePick);
    // The chain emptied the eater's cell 05 (Chromatic) and cell 15 (bitten,
    // fading on the Consume target screen): both lock again.
    let mut cells = board.model.cells.clone();
    let CellModel::Inmate { beast, .. } = cells[4] else { panic!("{:?}", cells[4]) };
    cells[4] = CellModel::Sealed { event: None };
    cells[14] = CellModel::Sealed { event: None };
    assert_eq!(pick.model.cells, cells);
    assert_eq!(pick.model.vault, [board.model.vault.clone(), vec![beast]].concat());
}

/// A pick read before any board is the run start's, its hidden counter too: a
/// stale count saved at 0 days left gives way to day 1, and the board read
/// after it is the same run.
#[test]
fn hidden_counter_on_the_run_start_pick_is_day_one() {
    let dir = common::frames();
    let frames = ["rec1-t031.png", "rec1-t071.png"].map(|f| dir.join(f));
    if frames.iter().any(|f| !f.is_file()) {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    let mut s = Session::new(20, vec![Tool::Chains], None, Some(0));
    let mut read = |f: &Path| {
        s.read_in_background(load_png(f).unwrap());
        s.wait();
        s.reading.clone().unwrap()
    };
    let pick = read(&frames[0]);
    assert_eq!(pick.screen, Screen::InstrumentPick);
    assert_eq!((pick.days_left, pick.model.day), (Some(20), 1));
    assert!(pick.warnings.iter().all(|w| !w.text.contains("kept from the last read")), "{:?}", pick.warnings);
    // The count 0 kept from the settings was the last run's: its Chains go.
    assert!(pick.warnings.iter().any(|w| w.text == "A new run: the held instruments were cleared."), "{:?}", pick.warnings);
    for _ in 0..2 {
        let board = read(&frames[1]);
        assert_eq!((board.screen, board.days_left), (Screen::Manage, Some(16)));
        assert!(board.warnings.iter().all(|w| !w.text.starts_with("A new run")), "{:?}", board.warnings);
    }
}

/// View Prison Status over a treasure pick (rec5-t018) shows the real board:
/// the pick stays on show, solved on that board in place of the inferred one
/// (rec5-t012), and the pick shown again (rec5-t028) keeps it.
#[test]
fn prison_status_gives_the_board_under_the_pick() {
    let tools = vec![Tool::Muzzle, Tool::Saw];
    let Some(status) = read_all(tools.clone(), &["rec5-t018.png"]) else { return };
    let status = status.reading.unwrap();
    assert_eq!(status.screen, Screen::PrisonStatus);
    let Some(mut s) = read_all(tools, &["rec5-t012.png"]) else { return };
    assert_eq!(s.reading.as_ref().unwrap().screen, Screen::TreasurePick);
    for f in ["rec5-t018.png", "rec5-t028.png"] {
        s.read_in_background(load_png(&common::frames().join(f)).unwrap());
        s.wait();
        let r = s.reading.clone().unwrap();
        assert_eq!(r.screen, Screen::TreasurePick, "{f}");
        assert_eq!(r.model.cells, status.model.cells, "{f}");
        assert_eq!(r.model.vault, status.model.vault, "{f}");
        let patrol = |w: &str| w.contains("Mark today's doors") || w.contains("No destination cards");
        assert!(r.warnings.iter().all(|w| !patrol(&w.text)), "{f}: {:?}", r.warnings);
        assert!(s.problems.iter().all(|p| !patrol(p)), "{f}: {:?}", s.problems);
        assert_eq!(r.model.choice.kind, ChoiceKind::Gift, "{f}");
        assert!(matches!(s.advice().map(|a| a.action), Some(Action::Choose { .. })), "{f}: {:?}", s.advice());
    }
}

/// A Consume target screen right after the manage screen picks the day's
/// first bite among today's targets; once the screen shows a bite (the eater
/// grew, its meal's cell locked), it picks the chain's next meal.
#[test]
fn consume_target_picks_a_bite() {
    let frames = ["rec3-t113.png", "rec3-t114.png", "rec3-t116.png"];
    let Some(mut s) = read_all(Vec::new(), &frames[..2]) else { return };
    let r = s.reading.clone().unwrap();
    assert_eq!(r.screen, Screen::ChainTarget);
    assert!(r.warnings.iter().all(|w| !w.text.contains("can't be solved")), "{:?}", r.warnings);
    let (st, _) = build_state(&r.model, 1);
    let targets: Vec<usize> = start_options(&st).iter().map(|o| o.cell).collect();
    assert!(!targets.is_empty());
    let advice = s.advice().expect("advice");
    assert!(matches!(advice.action, Action::Devour { to } if targets.contains(&to)), "{advice:?} {targets:?}");
    assert!(advice.label.starts_with("Consume the "), "{}", advice.label);
    assert!(!s.ranking.is_empty());
    assert!(s.ranking.iter().all(|o| matches!(o.action, Action::Devour { .. })), "{:?}", s.ranking);

    s.read_in_background(load_png(&common::frames().join(frames[2])).unwrap());
    s.wait();
    let r = s.reading.clone().unwrap();
    assert_eq!(r.screen, Screen::ChainTarget);
    let (st, _) = build_state(&r.model, 1);
    let options: Vec<usize> = devour_options(&st, st.position).iter().map(|o| o.cell).collect();
    let advice = s.advice().expect("advice");
    match advice.action {
        Action::Chain { to } => assert!(options.contains(&to), "{advice:?} {options:?}"),
        Action::EndChain => {}
        _ => panic!("{advice:?}"),
    }
    assert!(s.ranking.iter().all(|o| matches!(o.action, Action::Chain { .. } | Action::EndChain)), "{:?}", s.ranking);
}

/// The game opens a chain's Consume target screen only while the eater has a
/// target, so a board that gives it none (an eater read before its rarity
/// rose, a misread target) is a misread: no advice, never a Replace or End
/// the day, and a problem naming the eater's cell.
#[test]
fn consume_target_with_no_target_read_gets_no_advice() {
    let Some(mut s) = read_all(Vec::new(), &["rec3-t113.png", "rec3-t114.png", "rec3-t116.png"]) else { return };
    // A Normal Rat on the eater's cell: no Mammoth on the board to eat.
    s.edit_cell(16, CellEdit::Inmate { beast: Beast::Rat, tier: 0 }).unwrap();
    s.wait();
    let r = s.reading.clone().unwrap();
    assert_eq!(r.screen, Screen::ChainTarget);
    let (st, _) = build_state(&r.model, 1);
    assert!(devour_options(&st, st.position).is_empty(), "the edited eater has no target");
    assert_eq!(s.advice(), None, "{:?}", s.ranking);
    assert!(s.ranking.is_empty(), "{:?}", s.ranking);
    assert!(s.problems.iter().any(|p| p.contains("eater in cell 16")), "{:?}", s.problems);
}

/// The red jaws over the targets of the Consume target screen rec4-t110
/// spoil the read of cell 03: read alone it warns. After the board rec4-t070
/// of the day before, where 03 is locked, it gives no warning: a jawed cell
/// the board holds locked is the meal Avenger's Casket let in, here a Magic
/// Rat for the Magic Lion on 10, as the screen shows. The hidden days
/// counter, which this screen always hides, gives no line either.
#[test]
fn consume_target_keeps_unsure_cells() {
    let Some(alone) = read_all(Vec::new(), &["rec4-t110.png"]) else { return };
    let r = alone.reading.clone().unwrap();
    assert_eq!(r.screen, Screen::ChainTarget);
    assert!(r.warnings.iter().any(|w| w.text.starts_with("Check cell 03")), "{:?}", r.warnings);

    let Some(s) = read_all(Vec::new(), &["rec4-t070.png", "rec4-t110.png"]) else { return };
    let r = s.reading.clone().unwrap();
    assert_eq!(r.screen, Screen::ChainTarget);
    assert!(r.warnings.iter().all(|w| !w.text.starts_with("Check cell ") && !w.text.contains("days counter")), "{:?}", r.warnings);
    assert_eq!(r.model.cells[2], CellModel::Inmate { beast: Beast::Rat, tier: 1, ivory: false, prey: None, event: None });
}

/// The player's Consume target pick trainingdata/reports/chain-target.png
/// (outside s16): the eater on cell 03 has the big jaws on 04, 05, 12 and 13.
/// The advice bites one of those or bites nothing, never the Magic Lion on
/// 08; when the board read allows other targets, a warning on the eater's
/// cell names both sets. Skips when the frame is missing.
#[test]
fn consume_target_follows_the_jaws() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("trainingdata/reports/chain-target.png");
    if !path.is_file() {
        println!("skipped: {} is missing", path.display());
        return;
    }
    let mut s = Session::new(20, Vec::new(), None, None);
    s.read_in_background(load_png(&path).unwrap());
    s.wait();
    let r = s.reading.clone().unwrap();
    assert_eq!(r.screen, Screen::ChainTarget);
    let jawed = [3, 4, 11, 12];
    if let Some(advice) = s.advice() {
        match advice.action {
            Action::Devour { to } | Action::Chain { to } => assert!(jawed.contains(&to), "{advice:?}"),
            Action::EndChain | Action::End => {}
            _ => panic!("{advice:?}"),
        }
    }
    let (st, _) = build_state(&r.model, 1);
    let options: Vec<usize> = start_options(&st).iter().map(|o| o.cell).collect();
    let mut sorted = options.clone();
    sorted.sort();
    if sorted != jawed {
        let w = r.warnings.iter().find(|w| w.text.starts_with("The Consume targets on screen are cells 04, 05, 12 and 13"));
        assert_eq!(w.and_then(|w| w.cell), Some(2), "{:?}", r.warnings);
    }
}

/// Avenger's Casket's meal on a Consume target screen: a jawed cell the board
/// holds as locked takes the eater's rarity and a beast it eats. The player's
/// report trainingdata/reports/consume-target-cell2.png: an Ultimate Rat in
/// 07 bit, the Casket opened 02, and the meal (an Ultimate Mammoth, what a
/// Rat eats) stayed locked. A Mammoth eats Lions and Wolves: the read beast
/// when it is one of them, else a Lion with a warning on the cell.
#[test]
fn casket_meal_on_a_locked_cell() {
    let inmate = |beast, tier| CellModel::Inmate { beast, tier, ivory: false, prey: None, event: None };
    let mut board = Model::blank();
    board.warden = 6;
    board.cells[6] = inmate(Beast::Rat, 4);
    board.cells[2] = inmate(Beast::Mammoth, 1);
    let mut read = board.clone();
    read.cells[1] = inmate(Beast::Wolf, 0);
    read.cells[2] = inmate(Beast::Lion, 0);

    let mut m = board.clone();
    assert!(casket_meals(&mut m, &read, &[1, 2]).is_empty());
    assert_eq!(m.cells[1], inmate(Beast::Mammoth, 4));
    assert_eq!(m.cells[2], board.cells[2], "a jawed inmate stays as the board has it");

    board.cells[6] = inmate(Beast::Mammoth, 3);
    read.cells[1] = inmate(Beast::Wolf, 0);
    let mut m = board.clone();
    assert!(casket_meals(&mut m, &read, &[1]).is_empty());
    assert_eq!(m.cells[1], inmate(Beast::Wolf, 3));

    read.cells[1] = inmate(Beast::Rat, 0);
    let mut m = board.clone();
    let warnings = casket_meals(&mut m, &read, &[1]);
    assert_eq!(m.cells[1], inmate(Beast::Lion, 3));
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].cell, Some(1));
    assert!(warnings[0].text.starts_with("Check cell 02: a new Inmate from "), "{}", warnings[0].text);
}

/// The manage board rec1-t143 (day 16) has 2 Chromatic Prisoners. The same
/// frame with the vault panel blacked out, as when its slots do not read,
/// keeps them: Chromatics never leave the vault in a run.
#[test]
fn unread_vault_keeps_the_board_vault() {
    let Some(mut s) = read_all(Vec::new(), &["rec1-t143.png"]) else { return };
    let vault = s.reading.clone().unwrap().model.vault;
    assert_eq!(vault.len(), 2);
    let mut img = frame("rec1-t143.png");
    for y in 180..680 {
        for x in 1660..1870 {
            let i = (y * img.w + x) * 4;
            img.data[i..i + 3].fill(0);
        }
    }
    s.read_in_background(img);
    s.wait();
    let r = s.reading.clone().unwrap();
    assert_eq!(r.screen, Screen::Manage);
    assert_eq!(r.model.vault, vault);
}

/// A treasure pick (rec5-t012) over the day-16 board rec1-t143 has its 2
/// Chromatic Prisoners and the eater gone to the vault. View Prison Status
/// over it (rec5-t018, day 16) shows an empty vault panel, as when the panel
/// does not read; the pick it stays on and the pick shown again (rec5-t028)
/// keep the run's 3, and so does the riot screen's line.
#[test]
fn pick_over_an_unread_vault_keeps_the_run_vault() {
    if !have(&["rec5-t018.png", "rec5-t028.png", "rec4-t422.png"]) {
        return;
    }
    let Some(mut s) = read_all(Vec::new(), &["rec1-t143.png", "rec5-t012.png"]) else { return };
    let vault = s.reading.clone().unwrap().model.vault;
    assert_eq!(vault.len(), 3);
    for f in ["rec5-t018.png", "rec5-t028.png"] {
        s.read_in_background(frame(f));
        s.wait();
        let r = s.reading.clone().unwrap();
        assert_eq!((r.screen, r.model.day), (Screen::TreasurePick, 16), "{f}");
        assert_eq!(r.model.vault, vault, "{f}");
    }
    s.read_in_background(frame("rec4-t422.png"));
    s.wait();
    assert_eq!(s.run_over.as_deref(), Some("The run is over: 3 Chromatic Prisoners vaulted."));
}

/// The targets of a Consume target pick: the jawed cells among the board
/// read's, with a warning on the eater's cell when the two differ; the
/// board read's when no jaws read or none of them is among its own.
#[test]
fn consume_targets_restrict_and_warn() {
    assert_eq!(consume_targets(&[3, 7], &[], 2), (vec![3, 7], None));
    assert_eq!(consume_targets(&[7, 3], &[3, 7], 2), (vec![7, 3], None));

    let (targets, w) = consume_targets(&[3, 7], &[3, 4], 2);
    assert_eq!(targets, [3]);
    let w = w.expect("a warning");
    assert_eq!(w.cell, Some(2));
    assert_eq!(w.kind, WarningKind::Read);
    assert_eq!(
        w.text,
        "The Consume targets on screen are cells 04 and 05 and the board read allows cells 04 and 08: check the eater in cell 03 and those cells."
    );

    let (targets, w) = consume_targets(&[7], &[3, 4, 11], 2);
    assert_eq!(targets, [7]);
    assert!(w.expect("a warning").text.starts_with("The Consume targets on screen are cells 04, 05 and 12 and the board read allows cell 08:"));

    let (targets, w) = consume_targets(&[], &[3], 2);
    assert!(targets.is_empty());
    assert!(w.expect("a warning").text.contains("cell 04 and the board read allows none:"));
}

/// A loading screen (rec4-t424) after a board keeps the board's reading and
/// solve with a line that says so; the riot screen (rec4-t422) then ends the
/// run: no reading, no advice, no held instruments, one line.
#[test]
fn off_board_frame_then_run_end() {
    let Some(mut s) = read_all(vec![Tool::Saw], &["rec4-t070.png"]) else { return };
    let dir = common::frames();
    let (loading, riot) = (dir.join("rec4-t424.png"), dir.join("rec4-t422.png"));
    if !loading.is_file() || !riot.is_file() {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    let before = s.reading.clone().unwrap();
    assert_eq!(before.screen, Screen::Manage);
    let ranking = s.ranking.clone();
    s.read_in_background(load_png(&loading).unwrap());
    s.wait();
    let after = s.reading.clone().unwrap();
    assert_eq!((&after.model, after.screen, after.at), (&before.model, before.screen, before.at));
    assert_eq!(after.warnings[0].text, "The prison board is not on screen: showing the last reading.");
    assert_eq!(after.warnings[1..], before.warnings[..]);
    assert_eq!(s.ranking, ranking);

    s.read_in_background(load_png(&riot).unwrap());
    s.wait();
    let vaulted = before.model.vault.len();
    let line = if vaulted == 1 { "The run is over: 1 Chromatic Prisoner vaulted.".to_string() } else { format!("The run is over: {vaulted} Chromatic Prisoners vaulted.") };
    assert_eq!(s.run_over.as_deref(), Some(line.as_str()));
    assert!(s.reading.is_none());
    assert!(s.pick.is_none() && s.ranking.is_empty() && s.problems.is_empty() && !s.solving);
    assert!(s.advice().is_none());
    assert!(s.edit_cell(2, CellEdit::Empty).is_err());
    assert!(s.tools.is_empty() && s.take_tools_changed(), "{:?}", s.tools);
    assert_eq!(s.days_left(), None);

    // A mid-run board between runs is a screen after the run: kept.
    s.read_in_background(load_png(&dir.join("rec4-t070.png")).unwrap());
    s.wait();
    assert_eq!(s.run_over.as_deref(), Some(line.as_str()));
    assert!(s.reading.is_none());
    assert_eq!(s.missed.as_deref(), Some("The run is over: waiting for the next run."));

    // A board with 20 days left (rec1-t029) starts the next run.
    let start = dir.join("rec1-t029.png");
    if !start.is_file() {
        println!("skipped: {} is missing", start.display());
        return;
    }
    s.read_in_background(load_png(&start).unwrap());
    s.wait();
    assert!(s.run_over.is_none());
    assert_eq!(s.reading.as_ref().unwrap().days_left, Some(20));
}

/// The riot screen after the last day (rec4-t412, day 20) logs one run_end
/// and starts the next run's folder; a day-1 board read after it (rec1-t055)
/// and the riot screen again show the line but log no second run_end and
/// start no folder.
#[test]
fn run_end_logs_once() {
    if !have(&["rec4-t412.png", "rec4-t422.png", "rec1-t055.png"]) {
        return;
    }
    let root = std::env::temp_dir().join(format!("warden-run-end-once-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut s = Session::new(20, Vec::new(), None, None);
    s.log = Some(RunLog::new(root.clone()));
    let last = read_next(&mut s, "rec4-t412.png");
    assert_eq!((last.model.day, last.model.max_days), (20, 20));
    let first = s.log.as_ref().unwrap().dir().to_path_buf();
    s.read_in_background(frame("rec4-t422.png"));
    s.wait();
    assert!(s.run_over.is_some());
    let next = s.log.as_ref().unwrap().dir().to_path_buf();
    assert_ne!(next, first);

    let day1 = read_next(&mut s, "rec1-t055.png");
    assert_eq!(day1.model.day, 1);
    s.read_in_background(frame("rec4-t422.png"));
    s.wait();
    assert_eq!(s.run_over.as_deref(), Some("The run is over: 0 Chromatic Prisoners vaulted."));
    assert!(s.reading.is_none());
    assert_eq!(s.log.as_ref().unwrap().dir(), next);
    drop(s);

    let run_ends = |dir: &Path| {
        let text = std::fs::read_to_string(dir.join("log.jsonl")).unwrap_or_default();
        text.lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()).filter(|l| l["kind"] == "run_end").map(|l| l["vaulted"].clone()).collect::<Vec<_>>()
    };
    assert_eq!(run_ends(&first), vec![serde_json::json!(last.model.vault.len())]);
    assert!(run_ends(&next).is_empty());
    std::fs::remove_dir_all(&root).unwrap();
}

/// The riot screen missed: on the last day (rec4-t412, 0 days left), a
/// board with the counter hidden and far from the board on show (rec1-t073,
/// another run's Consume target screen) is a screen after the run: it ends
/// the run and is not installed.
#[test]
fn far_board_on_the_last_day_ends_the_run() {
    if !have(&["rec4-t412.png", "rec1-t073.png"]) {
        return;
    }
    let mut s = Session::new(20, Vec::new(), None, None);
    let last = read_next(&mut s, "rec4-t412.png");
    assert_eq!(last.days_left, Some(0));
    s.read_in_background(frame("rec1-t073.png"));
    s.wait();
    assert!(s.run_over.is_some());
    assert!(s.reading.is_none());
}

/// A game dialog keeps the last reading and its solve on show.
#[test]
fn popup_keeps_the_last_reading() {
    let dir = common::frames();
    let (board, popup) = (dir.join("rec1-t087.png"), dir.join("rec2-t109.png"));
    if !board.is_file() || !popup.is_file() {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    let mut s = Session::new(20, Vec::new(), None, None);
    s.read_in_background(load_png(&board).unwrap());
    s.wait();
    let before = s.reading.clone().unwrap();
    let ranking = s.ranking.clone();
    s.read_in_background(load_png(&popup).unwrap());
    s.wait();
    let after = s.reading.clone().unwrap();
    assert_eq!((&after.model, after.screen, after.at), (&before.model, before.screen, before.at));
    assert_eq!(after.warnings[0].text, "A popup covers the game: showing the last reading.");
    assert_eq!(after.warnings[1..], before.warnings[..]);
    assert_eq!(after.kept_lines(), ["A popup covers the game: showing the last reading."]);
    assert!(before.kept_lines().is_empty());
    assert_eq!(s.ranking, ranking);
}

/// A black frame: no board, no pick, no days counter.
fn black() -> Rgba {
    Rgba { w: 1920, h: 1080, data: [0, 0, 0, 255].repeat(1920 * 1080) }
}

/// A read without the board while no reading is on show says so in
/// `missed`; a board read clears it.
#[test]
fn off_board_read_without_a_reading_is_missed() {
    let mut s = Session::new(20, Vec::new(), None, None);
    s.read_in_background(black());
    s.wait();
    assert!(s.reading.is_none());
    assert_eq!(s.missed.as_deref(), Some("The last read found no board or pick on screen."));

    let board = common::frames().join("rec1-t087.png");
    if !board.is_file() {
        println!("skipped: {} is missing", board.display());
        return;
    }
    s.read_in_background(load_png(&board).unwrap());
    s.wait();
    assert!(s.reading.is_some() && s.missed.is_none());
}

/// The run log gets a "miss" line for a read that installed nothing, with
/// no frame for one off the board, and the "window" line names the OCR
/// language.
#[test]
fn run_log_logs_a_miss() {
    let root = std::env::temp_dir().join(format!("warden-run-log-miss-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut s = Session::new(20, Vec::new(), None, None);
    s.log = Some(RunLog::new(root.clone()));
    s.log_window("\"Torchlight: Infinite\" (1920 x 1080)");
    for _ in 0..2 {
        s.read_in_background(black());
        s.wait();
    }
    let run = s.log.as_ref().unwrap().dir().to_path_buf();
    let text = std::fs::read_to_string(run.join("log.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines[0]["kind"], "window");
    assert_eq!(lines[0]["window"], "\"Torchlight: Infinite\" (1920 x 1080)");
    assert_eq!(lines[0]["ocr"].as_str(), warden::reader::ocr::language().as_deref());
    let misses: Vec<&serde_json::Value> = lines.iter().filter(|l| l["kind"] == "miss").collect();
    assert_eq!(misses.len(), 2);
    assert_eq!(misses[0]["screen"], "Unknown");
    assert_eq!(misses[0]["line"], "The prison board is not on screen: showing the last reading.");
    assert_eq!(misses[0]["size"], serde_json::json!([1920, 1080]));
    // Off the board, the frame shows another game screen: none is saved.
    assert!(misses.iter().all(|m| m["frame"].is_null()));
    assert!(!run.join("frame-0001.jpg").exists());
    drop(s);
    std::fs::remove_dir_all(&root).unwrap();
}

/// The Sewer Ratway's "Select an Objective" reads the board under the corner
/// brackets as a cell choice, picks the cell to move to, and is the board
/// under the next instrument pick.
#[test]
fn sewer_ratway_cell_pick() {
    let dir = common::frames();
    let (ratway, pick) = (dir.join("rec2-t340.png"), dir.join("rec1-t031.png"));
    if !ratway.is_file() || !pick.is_file() {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    let mut s = Session::new(20, Vec::new(), None, None);
    s.read_in_background(load_png(&ratway).unwrap());
    s.wait();
    let r = s.reading.clone().unwrap();
    assert_eq!((r.screen, r.model.step, r.model.choice.kind), (Screen::CellPick, Step::Choice, ChoiceKind::Cell));
    assert!(r.model.cells.iter().filter(|c| matches!(c, CellModel::Inmate { .. })).count() >= 4, "{:?}", r.model.cells);
    assert!(s.problems.is_empty(), "{:?}", s.problems);
    let label = &s.pick.as_ref().expect("a pick").label;
    assert!(label.starts_with("Move to "), "{label}");

    s.read_in_background(load_png(&pick).unwrap());
    s.wait();
    assert_eq!(s.reading.clone().unwrap().model.cells, r.model.cells);
}

/// The Interrogation Room's Instigate and Bestow cards after a patrol are a
/// choice between the two, solved on the board read behind them.
#[test]
fn interrogation_room_cards() {
    for card in ["rec3-ir08.png", "rec3-ir12.png"] {
        let Some(s) = read_all(Vec::new(), &["rec3-t291.png", card]) else { return };
        let r = s.reading.clone().unwrap();
        assert_eq!((r.screen, r.model.step, r.model.choice.kind), (Screen::Interrogation, Step::Choice, ChoiceKind::Interrogation), "{card}");
        assert!(s.problems.is_empty(), "{card}: {:?}", s.problems);
        let Some(Action::Choose { id: ChoiceId::Name(key) }) = s.advice().map(|a| a.action) else { panic!("{card}: {:?}", s.advice()) };
        assert!(key == "instigate" || key == "bestow", "{card}: {key}");
        for name in ["instigate", "bestow"] {
            assert!(s.ranking.iter().any(|o| o.action == Action::Choose { id: ChoiceId::Name(name.to_string()) }), "{card}: no {name} in {:?}", s.ranking);
        }
    }
}

/// An incident marker on an open cell is read and stays through a correction.
#[test]
fn open_cell_marker() {
    let frame = common::frames().join("rec2-t100.png");
    if !frame.is_file() {
        println!("skipped: {} is missing", frame.display());
        return;
    }
    let mut s = Session::new(20, Vec::new(), None, None);
    s.read_in_background(load_png(&frame).unwrap());
    s.wait();
    let cell = &s.reading.as_ref().unwrap().model.cells[12];
    assert!(matches!(cell, CellModel::Inmate { beast: Beast::Wolf, event: Some(EventMark::Unknown), .. }), "{cell:?}");
    s.edit_cell(13, CellEdit::Empty).unwrap();
    let model = s.reading.clone().unwrap().model;
    assert_eq!(model.cells[12], CellModel::Empty { event: Some(EventMark::Unknown) });
    let (st, _) = build_state(&model, 1);
    assert!(!st.cells[12].sealed && st.cells[12].event.is_some() && st.cells[12].event_known == Some(false));
}

/// A low-confidence cell read warns on the first two reads that give it and
/// not on a third in a row.
#[test]
fn steady_read_stops_warning() {
    let frame = "rec3-t058.png";
    let Some(mut s) = read_all(Vec::new(), &[frame]) else { return };
    let check = |s: &Session| s.reading.as_ref().unwrap().warnings.iter().filter(|w| w.text.starts_with("Check cell 08: rarity")).count();
    assert_eq!(check(&s), 1, "{:?}", s.reading.as_ref().unwrap().warnings);
    let mut counts = Vec::new();
    for _ in 0..2 {
        s.read_in_background(load_png(&common::frames().join(frame)).unwrap());
        s.wait();
        counts.push(check(&s));
    }
    assert_eq!(counts, [1, 0]);
    assert!(s.reading.as_ref().unwrap().warnings.iter().all(|w| !w.text.starts_with("Check cell")), "{:?}", s.reading.as_ref().unwrap().warnings);
}

/// A treasure pick after a consume chain that reached Chromatic: the board
/// under it is the last Consume target screen's (earlier bites locked), with
/// the eater gone from the warden's cell to the vault and its cell and its
/// last meal's locked.
#[test]
fn treasure_after_a_chain() {
    let dir = common::frames();
    let frames = ["rec2-t045.png", "rec2-t051.png", "rec2-t063.png"].map(|f| dir.join(f));
    if !frames.iter().all(|f| f.is_file()) {
        println!("skipped: a frame in {} is missing", dir.display());
        return;
    }
    let mut s = Session::new(20, Vec::new(), None, None);
    let [board, chain, pick] = frames.map(|f| {
        s.read_in_background(load_png(&f).unwrap());
        s.wait();
        s.reading.clone().unwrap()
    });
    assert_eq!((board.screen, chain.screen, pick.screen), (Screen::Manage, Screen::ChainTarget, Screen::TreasurePick));
    // The eater sits in the warden's cell, 02.
    let n = chain.model.warden as usize;
    assert_eq!(n, 1);
    let CellModel::Inmate { beast, .. } = chain.model.cells[n] else { panic!("{:?}", chain.model.cells[n]) };
    assert!(matches!(pick.model.cells[n], CellModel::Sealed { .. }), "{:?}", pick.model.cells[n]);
    // Cell 03, the chain's last meal, locks too.
    assert!(matches!((&chain.model.cells[2], &pick.model.cells[2]), (CellModel::Inmate { .. }, CellModel::Sealed { .. })));
    for k in (0..chain.model.cells.len()).filter(|&k| k != n && k != 2) {
        assert_eq!(pick.model.cells[k], chain.model.cells[k], "cell {}", k + 1);
    }
    // Cell 10, open on the manage board, was the first bite.
    assert!(matches!(board.model.cells[9], CellModel::Inmate { .. }) && matches!(pick.model.cells[9], CellModel::Sealed { .. }));
    let mut vault = board.model.vault.clone();
    vault.push(beast);
    assert_eq!(pick.model.vault, vault);
    assert_eq!((pick.model.day, pick.model.step, pick.model.choice.kind), (board.model.day, Step::Choice, ChoiceKind::Gift));
    assert!(pick.model.devour_used);
    assert!(pick.warnings.iter().any(|w| w.text.ends_with("cell 02 left for the vault after consuming cell 03.")), "{:?}", pick.warnings);
    assert!(s.problems.is_empty(), "{:?}", s.problems);
}

/// The last Consume target screen before a treasure pick offers one target
/// (cell 09): the pick's board has that cell locked, as well as the eater's.
#[test]
fn treasure_seals_the_last_meal() {
    let frames = ["rec3-t113.png", "rec3-t114.png", "rec3-t116.png", "rec3-t117.png"];
    let Some(mut s) = read_all(Vec::new(), &frames) else { return };
    let chain = s.reading.clone().unwrap();
    assert_eq!(chain.screen, Screen::ChainTarget);
    assert!(matches!(chain.model.cells[8], CellModel::Inmate { .. }), "{:?}", chain.model.cells[8]);
    let treasure = common::frames().join("rec3-t121.png");
    if !treasure.is_file() {
        println!("skipped: {} is missing", treasure.display());
        return;
    }
    s.read_in_background(load_png(&treasure).unwrap());
    s.wait();
    let pick = s.reading.clone().unwrap();
    assert_eq!(pick.screen, Screen::TreasurePick);
    for n in [8, 15] {
        assert!(matches!(pick.model.cells[n], CellModel::Sealed { .. }), "cell {}: {:?}", n + 1, pick.model.cells[n]);
    }
    let text = "The Warden's Treasure follows a Chromatic Prisoner: the inmate in cell 16 left for the vault after consuming cell 09.";
    assert!(pick.warnings.iter().any(|w| w.text == text), "{:?}", pick.warnings);
    assert!(s.problems.is_empty(), "{:?}", s.problems);
}

/// The Consume target screen rec3-t117 with the Shackles and the Saw held,
/// then the day 9 patrol board rec3-t161 (`_` for its panel read empty, as
/// while the board fades in): the session and the reading on show.
fn chain_then_board(board: &str) -> Option<(Session, Reading)> {
    let frames = ["rec3-t113.png", "rec3-t114.png", "rec3-t116.png", "rec3-t117.png"];
    if !have(&[board]) {
        return None;
    }
    let mut s = read_all(Vec::new(), &frames)?;
    let chain = s.reading.clone().unwrap();
    assert_eq!(chain.screen, Screen::ChainTarget);
    assert_eq!(s.tools, [Tool::Longchain, Tool::Saw]);
    s.read_in_background(frame(board));
    s.wait();
    Some((s, chain))
}

/// A board read with an empty instrument panel right after a Consume target
/// screen is kept once; a second such read in a row is taken, the held
/// instruments kept.
#[test]
fn fading_board_after_a_chain_is_kept_once() {
    let Some((mut s, chain)) = chain_then_board("_rec3-t161.png") else { return };
    let kept = s.reading.clone().unwrap();
    assert_eq!((&kept.model, kept.screen, kept.at), (&chain.model, chain.screen, chain.at));
    assert_eq!(kept.kept_lines(), ["The board is still coming in: showing the last reading."]);
    assert_eq!(s.tools, [Tool::Longchain, Tool::Saw]);

    s.read_in_background(frame("_rec3-t161.png"));
    s.wait();
    let r = s.reading.clone().unwrap();
    assert_eq!(r.screen, Screen::Patrol);
    assert!(r.kept_lines().is_empty(), "{:?}", r.warnings);
    assert_eq!(s.tools, [Tool::Longchain, Tool::Saw]);
}

/// An instrument panel read empty while instruments are held (rec1-t087's
/// panel shows the Coin) is a misread: the held list stays.
#[test]
fn empty_panel_read_keeps_the_held_instruments() {
    let Some(s) = read_all(vec![Tool::Coin], &["_rec1-t087.png"]) else { return };
    assert_eq!(s.reading.as_ref().unwrap().screen, Screen::Patrol);
    assert_eq!(s.tools, [Tool::Coin]);
}

/// The day-16 manage board rec1-t143 read, and the frame with cell 3's
/// portrait painted over (a board on the same screen and day that reads
/// another model).
fn blip() -> Option<(Session, Rgba)> {
    let s = read_all(Vec::new(), &["rec1-t143.png"])?;
    let mut img = frame("rec1-t143.png");
    for y in 90..190 {
        for x in 815..925 {
            let i = (y * img.w + x) * 4;
            img.data[i..i + 3].fill(0);
        }
    }
    Some((s, img))
}

/// An opened screenshot of a changed board is taken at once.
#[test]
fn changed_board_from_a_file_is_taken_at_once() {
    let Some((mut s, changed)) = blip() else { return };
    let before = s.reading.clone().unwrap();
    s.read_in_background(changed);
    s.wait();
    let r = s.reading.clone().unwrap();
    assert_eq!(r.screen, Screen::Manage);
    assert_ne!(r.model, before.model);
    assert!(!s.wants_reread());
}

/// A live read of a changed board on the same screen and day is held, with
/// no line on show, and asks for one more read.
#[test]
fn changed_board_is_held_for_one_more_read() {
    let Some((mut s, changed)) = blip() else { return };
    let before = s.reading.clone().unwrap();
    s.read_live(changed);
    s.wait();
    let r = s.reading.clone().unwrap();
    assert_eq!((&r.model, r.at), (&before.model, before.at));
    assert!(r.kept_lines().is_empty(), "{:?}", r.warnings);
    assert!(s.wants_reread());
    assert!(!s.wants_reread());
}

/// A blip (the old board read again after a held read) keeps the reading
/// and the corrections made before it.
#[test]
fn blip_keeps_the_corrections() {
    let Some((mut s, changed)) = blip() else { return };
    let model = s.reading.clone().unwrap().model;
    let n = model.cells.iter().enumerate().position(|(i, c)| i != 2 && matches!(c, CellModel::Sealed { .. } | CellModel::Inmate { .. })).expect("a cell to edit");
    s.edit_cell(n as u8 + 1, CellEdit::Inmate { beast: Beast::Wolf, tier: 2 }).unwrap();
    let edited = s.reading.clone().unwrap().model;
    assert_ne!(edited, model);
    s.read_live(changed);
    s.wait();
    assert!(s.wants_reread());
    s.read_live(frame("rec1-t143.png"));
    s.wait();
    assert_eq!(s.reading.clone().unwrap().model, edited);
    assert!(!s.wants_reread());
}

/// A changed board read again after it was held is taken.
#[test]
fn confirmed_change_is_taken() {
    let Some((mut s, changed)) = blip() else { return };
    let mut direct = read_all(Vec::new(), &["rec1-t143.png"]).unwrap();
    direct.read_in_background(changed.clone());
    direct.wait();
    let want = direct.reading.clone().unwrap().model;
    s.read_live(changed.clone());
    s.wait();
    assert!(s.wants_reread());
    s.read_live(changed);
    s.wait();
    assert_eq!(s.reading.clone().unwrap().model, want);
    assert!(!s.wants_reread());
}

/// A board whose panel reads the held instruments is taken at once after a
/// Consume target screen.
#[test]
fn board_after_a_chain_is_taken() {
    let Some((s, _)) = chain_then_board("rec3-t161.png") else { return };
    let r = s.reading.clone().unwrap();
    assert_eq!(r.screen, Screen::Patrol);
    assert!(r.kept_lines().is_empty(), "{:?}", r.warnings);
    assert_eq!(s.tools, [Tool::Longchain, Tool::Saw]);
}

/// Saved frame `name`; a leading `~` covers its first instrument slot with
/// noise, so the panel's read is unsure, and a leading `_` with one colour,
/// so the panel reads empty.
fn frame(name: &str) -> Rgba {
    let (noise, name) = match name.strip_prefix('~') {
        Some(rest) => (true, rest),
        None => match name.strip_prefix('_') {
            Some(rest) => (false, rest),
            None => return load_png(&common::frames().join(name)).unwrap(),
        },
    };
    let mut img = load_png(&common::frames().join(name)).unwrap();
    // The slot's 89 px icon around (122, 391); frames load at 1918x1078.
    let mut seed = 1u32;
    for y in 346..436 {
        for x in 77..167 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let i = (y * img.w + x) * 4;
            img.data[i..i + 3].fill(if noise { (seed >> 24) as u8 } else { 20 });
        }
    }
    img
}

/// Reads `frames` (see [`frame`]) in order and gives the session; None
/// (skipped) when one is missing.
fn read_all(tools: Vec<Tool>, frames: &[&str]) -> Option<Session> {
    if !have(frames) {
        return None;
    }
    let mut s = Session::new(20, tools, None, None);
    for f in frames {
        s.read_in_background(frame(f));
        s.wait();
    }
    Some(s)
}

/// The instrument the session advises on the rec1-t031 pick after `board`
/// (rec1-t087, see [`frame`]) with `held`, and the pick's options.
fn advised_on_t031(held: Vec<Tool>, board: &str) -> Option<(Tool, Vec<Tool>)> {
    let s = read_all(held, &[board, "rec1-t031.png"])?;
    let r = s.reading.as_ref().unwrap();
    assert_eq!(r.screen, Screen::InstrumentPick);
    let Some(Action::Choose { id: ChoiceId::Name(key) }) = s.advice().map(|a| a.action) else { panic!("{:?}", s.advice()) };
    Some((Tool::from_key(&key).expect("an instrument"), r.model.choice.options.clone()))
}

/// A board frame's sure panel read names the held list over the instrument
/// advised on the pick before it: rec1-t087 shows the Coin.
#[test]
fn panel_read_wins_over_the_advised_instrument() {
    let Some((t, _)) = advised_on_t031(Vec::new(), "rec1-t087.png") else { return };
    assert_ne!(t, Tool::Coin);
    let s = read_all(Vec::new(), &["rec1-t087.png", "rec1-t031.png", "rec1-t087.png"]).unwrap();
    assert_eq!(s.tools, vec![Tool::Coin]);
}

/// With the panel's read unsure, the board after an instrument pick holds the
/// advised instrument without a line about it, and the solve plays with it.
#[test]
fn advised_instrument_is_held_after_the_pick() {
    let Some((t, _)) = advised_on_t031(Vec::new(), "~rec1-t087.png") else { return };
    let mut s = read_all(Vec::new(), &["~rec1-t087.png", "rec1-t031.png", "~rec1-t087.png"]).unwrap();
    assert_eq!(s.tools, vec![t]);
    let r = s.reading.clone().unwrap();
    assert!(r.warnings.iter().all(|w| !w.text.contains("held instruments")), "{:?}", r.warnings);
    assert!(s.take_tools_changed());
    assert!(!s.take_tools_changed());
    let held = read_all(vec![t], &["~rec1-t087.png"]).unwrap();
    assert_eq!(r.model, held.reading.unwrap().model);
    assert_eq!(s.ranking, held.ranking);
}

/// An advised instrument already held is not added again. The advice never
/// names a held instrument, so the player adds it on a screen the reader
/// doesn't know (the board between moves of rec1-t059 here), which keeps the
/// advised instrument.
#[test]
fn held_advised_instrument_is_not_duplicated() {
    let Some((t, _)) = advised_on_t031(Vec::new(), "~rec1-t087.png") else { return };
    let Some(mut s) = read_all(Vec::new(), &["~rec1-t087.png", "rec1-t031.png", "rec1-t059.png"]) else { return };
    assert_eq!(s.reading.as_ref().unwrap().screen, Screen::Unknown);
    s.set_tools(vec![t]);
    s.read_in_background(frame("~rec1-t087.png"));
    s.wait();
    assert_eq!(s.tools, vec![t]);
    assert!(!s.take_tools_changed());
}

/// A player's list stands while reads give the panel read it was set over
/// (rec1-t087's Coin), and gives way to another (the rec2-t340 cell pick's Crown and Stake).
#[test]
fn set_instruments_stand_until_the_panel_changes() {
    let Some(mut s) = read_all(Vec::new(), &["rec1-t087.png"]).filter(|_| have(&["rec2-t340.png"])) else { return };
    s.set_tools(vec![Tool::Saw]);
    s.read_in_background(frame("rec1-t087.png"));
    s.wait();
    assert_eq!(s.tools, vec![Tool::Saw]);
    s.read_in_background(frame("rec2-t340.png"));
    s.wait();
    assert_eq!(s.tools, vec![Tool::Crown, Tool::Stake]);
}

/// A game dialog between the pick and the board keeps the advised instrument.
#[test]
fn popup_keeps_the_advised_instrument() {
    let Some((t, _)) = advised_on_t031(Vec::new(), "~rec1-t087.png") else { return };
    let Some(mut s) = read_all(Vec::new(), &["~rec1-t087.png", "rec1-t031.png", "rec2-t109.png"]) else { return };
    assert!(s.tools.is_empty());
    s.read_in_background(frame("~rec1-t087.png"));
    s.wait();
    assert_eq!(s.tools, vec![t]);
}

/// An instrument advised at a new run's start is kept when the second
/// higher read confirms the new run: the board after the rec1-t031 pick
/// (rec1-t049, 19 days, over rec1-t087's 14) takes the advised instrument
/// while its count waits, and reading it again clears only the old Saw.
/// The panel's reads are unsure throughout.
#[test]
fn new_run_keeps_the_instrument_picked_at_its_start() {
    let Some((t, _)) = advised_on_t031(vec![Tool::Saw], "~rec1-t087.png") else { return };
    assert_ne!(t, Tool::Saw);
    let Some(mut s) = read_all(vec![Tool::Saw], &["~rec1-t087.png", "rec1-t031.png", "~rec1-t049.png"]) else { return };
    assert_eq!(s.reading.as_ref().map(|r| (r.days_left, r.screen)), Some((Some(14), Screen::Patrol)), "the higher count waits");
    assert_eq!(s.tools, vec![Tool::Saw, t]);
    s.read_in_background(frame("~rec1-t049.png"));
    s.wait();
    let r = s.reading.clone().unwrap();
    assert_eq!(r.days_left, Some(19));
    assert_eq!(s.tools, vec![t]);
    assert!(r.warnings.iter().any(|w| w.text == "A new run: the held instruments were cleared."), "{:?}", r.warnings);
}

/// Held instruments saved at a days count (an app restart) are cleared by a
/// higher count read twice (rec1-t029, 20 days); with no saved count the
/// first count is taken and they are kept.
#[test]
fn saved_instruments_clear_on_a_higher_count() {
    let dir = common::frames();
    let frame = dir.join("rec1-t029.png");
    if !frame.is_file() {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    for (days, held) in [(Some(11), vec![]), (None, vec![Tool::Muzzle])] {
        let mut s = Session::new(20, vec![Tool::Muzzle], None, days);
        for _ in 0..2 {
            s.read_in_background(load_png(&frame).unwrap());
            s.wait();
        }
        assert_eq!(s.days_left(), Some(20));
        assert_eq!(s.tools, held, "saved at {days:?}");
    }
}

/// Under Iron Law the run lasts 30 days, so 20 days left (rec1-t029) is
/// day 10; without a theme it is day 1 of 20.
#[test]
fn iron_law_run_reads_at_30_days() {
    let dir = common::frames();
    let frame = dir.join("rec1-t029.png");
    if !frame.is_file() {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    for (theme, day, max_days) in [(Some(IRON_LAW), 10, 30), (None, 1, 20)] {
        let mut s = Session::new(20, vec![], theme, None);
        s.read_in_background(load_png(&frame).unwrap());
        s.wait();
        let m = &s.reading.as_ref().expect("a reading").model;
        assert_eq!((m.day, m.max_days), (day, max_days), "theme {theme:?}");
    }
}

/// An instrument the player adds on the pick is the one taken: the next
/// advice on that pick is not added on top (the panel's reads unsure).
#[test]
fn instrument_named_on_the_pick_is_kept() {
    let Some((t, offers)) = advised_on_t031(Vec::new(), "~rec1-t087.png") else { return };
    let other = *offers.iter().find(|&&o| o != t).expect("another offer");
    let Some(mut s) = read_all(Vec::new(), &["~rec1-t087.png", "rec1-t031.png"]) else { return };
    s.set_tools(vec![other]);
    s.wait();
    s.read_in_background(frame("~rec1-t087.png"));
    s.wait();
    assert_eq!(s.tools, vec![other]);
}

/// A higher days count read twice is a new run: the held instruments go
/// (the panel's reads unsure).
#[test]
fn new_run_clears_the_held_instruments() {
    let Some(mut s) = read_all(vec![Tool::ALL[0], Tool::ALL[1]], &["~rec1-t087.png", "~rec1-t071.png"]) else { return };
    assert_eq!(s.tools.len(), 2, "a lone higher read");
    s.read_in_background(frame("~rec1-t071.png"));
    s.wait();
    assert!(s.tools.is_empty());
    let r = s.reading.clone().unwrap();
    assert!(r.warnings.iter().any(|w| w.text == "A new run: the held instruments were cleared."), "{:?}", r.warnings);
    assert!(s.take_tools_changed());
}

/// rec4-t074: a Warden's Treasure pick after the rec4-t070 manage board with
/// no Consume target screen between them, the Ravenous Chains held. The
/// Chromatic is the board's top inmate, the Legendary Wolf in cell 07 (the
/// Chains removed the Mammoth in the warden's cell 08 beside it at the start
/// of a day), not the inmate in the warden's cell.
#[test]
fn treasure_after_the_ravenous_chains() {
    let Some(mut s) = read_all(vec![Tool::Chains], &["rec4-t070.png"]) else { return };
    let board = s.reading.clone().unwrap();
    s.read_in_background(load_png(&common::frames().join("rec4-t074.png")).unwrap());
    s.wait();
    let pick = s.reading.clone().unwrap();
    assert_eq!((board.screen, pick.screen), (Screen::Manage, Screen::TreasurePick));
    assert!(matches!(board.model.cells[6], CellModel::Inmate { beast: Beast::Wolf, tier: 4, .. }), "{:?}", board.model.cells[6]);
    for n in [6, 7] {
        assert!(matches!(pick.model.cells[n], CellModel::Sealed { .. }), "cell {}: {:?}", n + 1, pick.model.cells[n]);
    }
    assert_eq!(pick.model.vault, [board.model.vault.clone(), vec![Beast::Wolf]].concat());
    let text = "The Warden's Treasure follows a Chromatic Prisoner: the inmate in cell 07 left for the vault, lifted by the Ferocious Beast's Ravenous Chains at the start of the day, which removed the inmate in cell 08: check them with View Prison Status.";
    assert!(pick.warnings.iter().any(|w| w.text == text), "{:?}", pick.warnings);
}

/// A frame sent while a read runs waits for it: the first read is not dropped.
#[test]
fn a_frame_during_a_read_waits_for_it() {
    let dir = common::frames();
    let frames = ["rec5-t012.png", "rec5-t018.png"];
    if frames.iter().any(|f| !dir.join(f).is_file()) {
        println!("skipped: {} is missing", dir.display());
        return;
    }
    let mut s = Session::new(20, Vec::new(), None, None);
    for f in frames {
        s.read_in_background(load_png(&dir.join(f)).unwrap());
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let poll_until = |s: &mut Session, done: fn(&mut Session) -> bool| {
        while !done(s) {
            assert!(std::time::Instant::now() < until, "no read finished in 60 s");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    poll_until(&mut s, |s| s.poll());
    assert_eq!(s.reading.as_ref().unwrap().screen, Screen::TreasurePick);
    assert!(s.scanning());
    poll_until(&mut s, |s| {
        s.poll();
        !s.scanning()
    });
    // View Prison Status over the pick keeps the pick on show, on its board.
    let r = s.reading.clone().unwrap();
    let status = read_all(Vec::new(), &frames[1..]).unwrap().reading.unwrap();
    assert_eq!((status.screen, r.screen), (Screen::PrisonStatus, Screen::TreasurePick));
    assert_eq!(r.model.cells, status.model.cells);
}

/// The run log: a line per reading, and a frame only when the screen or
/// board differs from the last frame saved.
#[test]
fn run_log_saves_a_frame_per_board() {
    let dir = common::frames();
    let names = ["rec1-t087.png", "rec1-t031.png", "rec1-t031.png"];
    if let Some(f) = names.iter().find(|f| !dir.join(f).is_file()) {
        println!("skipped: {f} is missing");
        return;
    }
    let root = std::env::temp_dir().join(format!("warden-run-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut s = Session::new(ROUND, Vec::new(), None, None);
    s.log = Some(RunLog::new(root.clone()));
    for f in names {
        s.read_in_background(load_png(&dir.join(f)).unwrap());
        s.wait();
    }
    let run = s.log.as_ref().unwrap().dir().to_path_buf();
    let text = std::fs::read_to_string(run.join("log.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let reads: Vec<&serde_json::Value> = lines.iter().filter(|l| l["kind"] == "read").collect();
    assert_eq!(reads.len(), 3);
    assert_eq!(reads[0]["screen"], "Patrol");
    assert_eq!(reads[1]["screen"], "InstrumentPick");
    assert_eq!(reads[0]["frame"], "frame-0001.jpg");
    assert_eq!(reads[1]["frame"], "frame-0002.jpg");
    assert!(reads[2]["frame"].is_null());
    assert!(reads[0]["model"]["cells"].is_array() && reads[0]["at"].is_string());
    assert!(lines.iter().any(|l| l["kind"] == "advice" && l["pick"].is_string()));
    let frames = std::fs::read_dir(&run).unwrap().filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "jpg")).count();
    assert_eq!(frames, 2);
    drop(s);
    std::fs::remove_dir_all(&root).unwrap();
}

/// True when the saved frames hold every one of `names`; says which is missing otherwise.
fn have(names: &[&str]) -> bool {
    let missing = names.iter().find(|f| !common::frames().join(f.trim_start_matches(['~', '_'])).is_file());
    if let Some(f) = missing {
        println!("skipped: {f} is missing");
    }
    missing.is_none()
}

/// Reads `name` of the saved frames in `s` and gives the reading on show.
fn read_next(s: &mut Session, name: &str) -> Reading {
    s.read_in_background(load_png(&common::frames().join(name)).unwrap());
    s.wait();
    s.reading.clone().unwrap()
}

/// The lobby's run theme scroll (rec6-t003, Opulence) sets the theme and
/// keeps the last board on show; the riot screen clears it.
#[test]
fn the_theme_scroll_sets_the_theme() {
    if !have(&["rec6-t181.png", "rec6-t003.png", "rec4-t422.png"]) {
        return;
    }
    let mut s = Session::new(20, Vec::new(), None, None);
    let board = read_next(&mut s, "rec6-t181.png");
    let r = read_next(&mut s, "rec6-t003.png");
    assert_eq!(s.theme, Some(OPULENCE));
    assert!(s.take_tools_changed());
    assert_eq!((board.screen, r.screen), (Screen::Manage, Screen::Manage));
    assert_eq!(r.model, board.model);
    assert!(r.warnings.iter().any(|w| w.text.starts_with("The run's theme: ")), "{:?}", r.warnings);
    s.read_in_background(load_png(&common::frames().join("rec4-t422.png")).unwrap());
    s.wait();
    assert!(s.run_over.is_some());
    assert_eq!(s.theme, None);
}

/// An Opulence treasure in rec6 (day 13): the manage board, the pick, View
/// Prison Status over it, then the board shown after each pick before the
/// next pick screen.
const OPULENCE_FLOW: [&str; 8] = ["rec6-t181.png", "rec6-t191.png", "rec6-t206.png", "rec6-t225.png", "rec6-t227.png", "rec6-t230.png", "rec6-t231.png", "rec6-t236.png"];

/// The gifts offered and the picks left on the treasure pick being solved.
fn treasure_pick(s: &Session) -> Option<(usize, u32)> {
    s.treasure_pick().map(|(options, picks)| (options.len(), picks))
}

#[test]
fn opulence_counts_the_picks_of_a_treasure() {
    if !have(&OPULENCE_FLOW) {
        return;
    }
    let mut s = Session::new(20, Vec::new(), Some(OPULENCE), None);
    assert_eq!(read_next(&mut s, "rec6-t181.png").screen, Screen::Manage);
    assert_eq!(read_next(&mut s, "rec6-t191.png").screen, Screen::TreasurePick);
    assert_eq!(treasure_pick(&s), Some((4, 3)));
    assert_eq!(read_next(&mut s, "rec6-t206.png").screen, Screen::TreasurePick);
    assert_eq!(treasure_pick(&s), Some((4, 3)));
    // The board after the first pick stays under the pick on show.
    let r = read_next(&mut s, "rec6-t225.png");
    let after = read_all(Vec::new(), &["rec6-t225.png"]).unwrap().reading.unwrap();
    assert_eq!((r.screen, after.screen), (Screen::TreasurePick, Screen::Unknown));
    assert_eq!(r.model.cells, after.model.cells);
    assert_eq!(read_next(&mut s, "rec6-t227.png").screen, Screen::TreasurePick);
    assert_eq!(treasure_pick(&s), Some((4, 2)));
    assert_eq!(read_next(&mut s, "rec6-t230.png").screen, Screen::TreasurePick);
    assert_eq!(read_next(&mut s, "rec6-t231.png").screen, Screen::TreasurePick);
    assert_eq!(treasure_pick(&s), Some((4, 1)));
    // No pick left: the board after the last one is installed as read.
    assert_eq!(read_next(&mut s, "rec6-t236.png").screen, Screen::Unknown);
    assert_eq!(treasure_pick(&s), None);
}

/// Without a theme a treasure gives one pick: the board after it is
/// installed as read.
#[test]
fn a_treasure_without_a_theme_has_one_pick() {
    let Some(mut s) = read_all(Vec::new(), &OPULENCE_FLOW[..2]) else { return };
    assert_eq!(s.reading.as_ref().unwrap().screen, Screen::TreasurePick);
    assert_eq!(treasure_pick(&s), Some((4, 1)));
    assert_eq!(read_next(&mut s, "rec6-t225.png").screen, Screen::Unknown);
    assert_eq!(treasure_pick(&s), None);
}

#[test]
fn rollouts_same_on_any_thread_count() {
    let (st, problems) = common::build("ratway.json");
    assert!(problems.is_empty(), "{problems:?}");
    let ranked = |threads: usize| {
        let mut r = Rollouts::new(&st, 7, &TUNED);
        assert_eq!(r.run_on(60, threads), 60);
        r.ranking()
    };
    let one = ranked(1);
    assert!(one.len() > 2, "{} candidates", one.len());
    assert_eq!(ranked(4), one);
    // In steps, with a thread count that does not divide the rounds.
    let mut r = Rollouts::new(&st, 7, &TUNED);
    r.run(25);
    r.run_on(35, 3);
    assert_eq!(r.ranking(), one);
}

/// A Prey mark loses a round each day after it is first read: the crosshair
/// on cell 4 of the Trap run reads 3 rounds on day 3 (hunt-run-0008) and 2
/// on day 4 (hunt-run-0012). Day 3 read twice after it is a new run's, at 3
/// again (a lone higher days counter is taken as a misread). Skips when a
/// frame is missing.
#[test]
fn prey_mark_rounds_count_down() {
    let dir = common::frames();
    let frames = ["hunt-run-0008.png", "hunt-run-0012.png", "hunt-run-0008.png", "hunt-run-0008.png"].map(|f| dir.join(f));
    if !frames.iter().all(|f| f.is_file()) {
        println!("skipped: a hunt-run frame in {} is missing", dir.display());
        return;
    }
    let mut s = Session::new(20, Vec::new(), None, None);
    for (f, (day, rounds)) in frames.iter().zip([(3, 3), (4, 2), (4, 2), (3, 3)]) {
        s.read_in_background(load_png(f).unwrap());
        s.wait();
        let model = &s.reading.as_ref().unwrap().model;
        let marks: Vec<PreyMark> = model.cells.iter().filter_map(|c| match c {
            CellModel::Inmate { prey: Some(mark), .. } => Some(*mark),
            _ => None,
        }).collect();
        assert_eq!((model.day, marks), (day, vec![PreyMark { cell: Some(3), rounds }]), "{}", f.display());
    }
}

/// The Hunt's target pick (hunt-run-0006) shows no cards: it keeps the day's
/// used Consume from the manage screen before it (hunt-run-0005), and its
/// cells, since the white glow on the lone target, the Normal Wolf in cell 4,
/// spoils that cell's read. The pick marks that Wolf. Skips when a frame is
/// missing.
#[test]
fn hunt_pick_keeps_manage_read() {
    let dir = common::frames();
    let (manage, hunt) = (dir.join("hunt-run-0005.png"), dir.join("hunt-run-0006.png"));
    if !manage.is_file() || !hunt.is_file() {
        println!("skipped: a hunt-run frame in {} is missing", dir.display());
        return;
    }
    let mut s = Session::new(20, Vec::new(), None, None);
    s.read_in_background(load_png(&manage).unwrap());
    s.wait();
    let before = s.reading.clone().unwrap().model;
    assert_eq!((before.step, before.devour_used, before.swap_used), (Step::Manage, false, false));
    s.read_in_background(load_png(&hunt).unwrap());
    s.wait();
    let r = s.reading.clone().unwrap();
    assert_eq!((r.screen, r.model.step, r.model.choice.kind), (Screen::CellPick, Step::Choice, ChoiceKind::Prey));
    assert_eq!((r.model.devour_used, r.model.swap_used, r.model.hunt_used), (false, false, false));
    assert_eq!(r.model.cells, before.cells);
    assert!(s.problems.is_empty(), "{:?}", s.problems);
    let label = &s.pick.as_ref().expect("a pick").label;
    assert!(label.starts_with(&format!("{}: mark", warden::engine::data::data().hunt_name)) && label.contains(" in 04 "), "{label}");
}
