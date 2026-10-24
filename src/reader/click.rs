//! Where the player clicks for an advised move: the board cell, destination
//! card, button or pick card the move needs on the screen read, on the
//! reference frame and on the game's frame.

use super::layout::{Layout, frame_point};
use super::panels::{Screen, Spot};
use super::scene::portrait;
use crate::engine::actions::{Action, ChoiceId};

/// Ring radii in reference px: a cell portrait, a destination or
/// Interrogation card (330 x 170, rec1-t049; a ring of half its width
/// reaches the board), an instrument card (about 457 wide, reroll-2000x837),
/// a decision or Cancel button (about 140 across, rec1-t055 and rec1-t057)
/// and the Re-roll button (270 x 65).
const CELL_R: f32 = 60.0;
const DOOR_R: f32 = 110.0;
const PICK_R: f32 = 230.0;
const BUTTON_R: f32 = 70.0;
const REROLL_R: f32 = 90.0;

/// A place to click: its centre on the reference frame, the layout it was
/// read from, and a ring radius in reference px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Click {
    pub at: (i32, i32),
    pub layout: Layout,
    pub radius: f32,
}

/// What the player clicks for `action` on `screen`, given the screen's
/// [`super::panels::Panels::spots`]; None for a move with nothing to click
/// on that screen or whose card or button was not read.
pub fn target(screen: Screen, spots: &[(Spot, Layout, (i32, i32))], action: &Action) -> Option<Click> {
    let spot = |want: Spot, radius: f32| spots.iter().find(|s| s.0 == want).map(|&(_, layout, at)| Click { at, layout, radius });
    // Actions name model cells; the screen numbers them from 1.
    let cell = |n: usize| (n < 16).then(|| Click { at: portrait(n as u8 + 1), layout: Layout::Board, radius: CELL_R });
    let card = |key: &str, radius: f32| spot(Spot::Card(key.to_string()), radius);
    match (screen, action) {
        (Screen::Patrol, Action::Patrol { cell }) => spot(Spot::Door(u8::try_from(*cell + 1).ok()?), DOOR_R),
        (Screen::Manage, Action::Devour { .. }) => spot(Spot::Button("Consume"), BUTTON_R),
        (Screen::Manage, Action::Swap) => spot(Spot::Button("Replace"), BUTTON_R),
        (Screen::Manage, Action::Hunt { .. }) => spot(Spot::Button("Hunt"), BUTTON_R),
        (Screen::Manage, Action::End) => spot(Spot::Button("End"), BUTTON_R),
        (Screen::ChainTarget, Action::Devour { to } | Action::Chain { to } | Action::Choose { id: ChoiceId::Cell(to) }) => cell(*to),
        (Screen::ChainTarget, Action::EndChain) => spot(Spot::Button("Cancel"), BUTTON_R),
        (Screen::CellPick, Action::Choose { id: ChoiceId::Cell(n) }) => cell(*n),
        (Screen::InstrumentPick, Action::Choose { id: ChoiceId::Name(key) }) => card(key, PICK_R),
        (Screen::InstrumentPick, Action::Reroll) => spot(Spot::Button("Re-roll"), REROLL_R),
        (Screen::Interrogation, Action::Choose { id: ChoiceId::Name(key) }) => card(key, DOOR_R),
        _ => None,
    }
}

/// The ring for `click` on a `w` x `h` frame at interface scale `u`: its
/// centre and radius in frame px.
pub fn to_frame(click: &Click, w: usize, h: usize, u: f32) -> (f32, f32, f32) {
    let ((x, y), scale) = frame_point((click.at.0 as f32, click.at.1 as f32), click.layout, w, h, u);
    (x, y, click.radius * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::data::Tool;
    use crate::reader::panels::{Panels, read_panels_titled};
    use std::path::{Path, PathBuf};

    /// trainingdata/s16, None (and a note) when it is missing.
    fn frames() -> Option<PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("trainingdata/s16");
        dir.is_dir().then_some(dir.clone()).or_else(|| {
            println!("skipped: {} is missing", dir.display());
            None
        })
    }

    fn read(dir: &Path, name: &str) -> Panels {
        let (board, title) = super::super::load_layouts(&dir.join(name)).unwrap();
        read_panels_titled(&board, title.as_ref().unwrap_or(&board))
    }

    fn inside((x, y): (i32, i32), (x0, y0, x1, y1): (i32, i32, i32, i32)) -> bool {
        (x0..x1).contains(&x) && (y0..y1).contains(&y)
    }

    fn at(p: &Panels, action: Action) -> Option<(i32, i32)> {
        target(p.screen, &p.spots, &action).map(|c| c.at)
    }

    #[test]
    fn board_cells_on_target_and_cell_picks() {
        let Some(dir) = frames() else { return };
        let chain = read(&dir, "rec1-t057.png");
        assert_eq!(chain.screen, Screen::ChainTarget);
        for to in 0..16 {
            assert_eq!(at(&chain, Action::Devour { to }), Some(portrait(to as u8 + 1)));
            assert_eq!(at(&chain, Action::Chain { to }), Some(portrait(to as u8 + 1)));
        }
        assert_eq!(at(&chain, Action::Devour { to: 16 }), None);
        // The Cancel region of the panels reader.
        assert!(inside(at(&chain, Action::EndChain).unwrap(), (1600, 740, 1918, 880)));
        let pick = read(&dir, "rec2-t340.png");
        assert_eq!(pick.screen, Screen::CellPick);
        for n in 0..16 {
            assert_eq!(at(&pick, Action::Choose { id: ChoiceId::Cell(n) }), Some(portrait(n as u8 + 1)));
        }
        assert_eq!(at(&pick, Action::Event), None);
    }

    /// Each door, as data/scan-truth-s16.json labels it, lands on its card:
    /// three 330 px cards centred 345 apart in y 750-920 (rec1-t049).
    #[test]
    fn doors_and_buttons() {
        let Some(dir) = frames() else { return };
        let patrol = read(&dir, "rec1-t049.png");
        assert_eq!(patrol.doors, [Some(3), Some(6), Some(7)]);
        for (i, door) in [3usize, 6, 7].into_iter().enumerate() {
            let x = 615 + 345 * i as i32;
            assert!(inside(at(&patrol, Action::Patrol { cell: door - 1 }).unwrap(), (x - 165, 750, x + 165, 920)), "door {door}");
        }
        assert_eq!(at(&patrol, Action::Patrol { cell: 3 }), None);
        assert_eq!(at(&patrol, Action::End), None);
        // Consume, Replace and End, icon and name, about 140 px across round x 614, 960 and 1304.
        let manage = read(&dir, "rec1-t055.png");
        assert_eq!(manage.screen, Screen::Manage);
        for (action, x) in [(Action::Devour { to: 4 }, 614), (Action::Swap, 960), (Action::End, 1304)] {
            assert!(inside(at(&manage, action.clone()).unwrap(), (x - 70, 775, x + 70, 915)), "{action:?}");
        }
        assert_eq!(at(&manage, Action::Hunt { cell: 4 }), None);
    }

    /// The three instrument cards span x 250-1670 (rec1-t031); the Re-roll
    /// button sits at x 760-970, y 715-770 of reroll-2000x837.
    #[test]
    fn instrument_cards_and_reroll() {
        let Some(dir) = frames() else { return };
        let pick = read(&dir, "rec1-t031.png");
        assert_eq!(pick.screen, Screen::InstrumentPick);
        assert_eq!(pick.instruments.len(), 3);
        for (i, tool) in pick.instruments.iter().enumerate() {
            let x = 250 + 473 * i as i32;
            let action = Action::Choose { id: ChoiceId::Name(tool.key().to_string()) };
            assert!(inside(at(&pick, action).unwrap(), (x, 187, x + 473, 870)), "{tool:?}");
        }
        let missing = Tool::ALL.iter().find(|t| !pick.instruments.contains(t)).unwrap();
        assert_eq!(at(&pick, Action::Choose { id: ChoiceId::Name(missing.key().to_string()) }), None);
        let reroll = read(&dir, "reroll-2000x837.png");
        assert_eq!(reroll.rerolls, 1);
        let click = target(reroll.screen, &reroll.spots, &Action::Reroll).unwrap();
        let png = std::fs::read(dir.join("reroll-2000x837.png")).unwrap();
        let frame = super::super::decode_png_raw(&png).unwrap();
        let (_, _, u) = super::super::layout::lay_out_scaled(&frame, None);
        let (x, y, r) = to_frame(&click, frame.w, frame.h, u);
        assert!(inside((x as i32, y as i32), (760, 715, 970, 770)) && (60.0..80.0).contains(&r), "({x}, {y}) r {r}");
    }
}
