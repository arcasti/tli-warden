//! Where the player clicks for an advised move: the board cell, destination
//! card, button or pick card the move needs on the screen read, on the
//! reference frame and on the game's frame.

use super::layout::{Layout, frame_point};
use super::panels::{Screen, Spot};
use super::scene::portrait;
use crate::engine::actions::{Action, ChoiceId};

/// Target boxes in reference px, width by height: a cell portrait
/// (portraits at a 175 px pitch, scene.rs PORTRAIT), a destination or
/// Interrogation card (rec1-t049), an instrument card (y 187-870,
/// reroll-2000x837), a Warden's Treasure card (rec1-t085), a decision or
/// round Cancel button (rec1-t055 and rec1-t057) and the Re-roll button.
const CELL: (f32, f32) = (130.0, 130.0);
const DOOR: (f32, f32) = (330.0, 170.0);
const PICK: (f32, f32) = (457.0, 683.0);
const TREASURE: (f32, f32) = (420.0, 665.0);
const BUTTON: (f32, f32) = (140.0, 140.0);
const REROLL: (f32, f32) = (270.0, 65.0);

/// A place to click: its centre on the reference frame, the layout it was
/// read from, and its half width and half height in reference px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Click {
    pub at: (i32, i32),
    pub layout: Layout,
    pub half: (f32, f32),
}

/// What the player clicks for `action` on `screen`, given the screen's
/// [`super::panels::Panels::spots`]; None for a move with nothing to click
/// on that screen or whose card or button was not read.
pub fn target(screen: Screen, spots: &[(Spot, Layout, (i32, i32))], action: &Action) -> Option<Click> {
    let half = |(w, h): (f32, f32)| (w / 2.0, h / 2.0);
    let spot = |want: Spot, size| spots.iter().find(|s| s.0 == want).map(|&(_, layout, at)| Click { at, layout, half: half(size) });
    // Actions name model cells; the screen numbers them from 1.
    let cell = |n: usize| (n < 16).then(|| Click { at: portrait(n as u8 + 1), layout: Layout::Board, half: half(CELL) });
    let card = |key: &str, size| spot(Spot::Card(key.to_string()), size);
    match (screen, action) {
        (Screen::Patrol, Action::Patrol { cell }) => spot(Spot::Door(u8::try_from(*cell + 1).ok()?), DOOR),
        (Screen::Manage, Action::Devour { .. }) => spot(Spot::Button("Consume"), BUTTON),
        (Screen::Manage, Action::Swap) => spot(Spot::Button("Replace"), BUTTON),
        (Screen::Manage, Action::Hunt { .. }) => spot(Spot::Button("Hunt"), BUTTON),
        (Screen::Manage, Action::End) => spot(Spot::Button("End"), BUTTON),
        (Screen::ChainTarget, Action::Devour { to } | Action::Chain { to } | Action::Choose { id: ChoiceId::Cell(to) }) => cell(*to),
        (Screen::ChainTarget, Action::EndChain) => spot(Spot::Button("Cancel"), BUTTON),
        (Screen::CellPick, Action::Choose { id: ChoiceId::Cell(n) }) => cell(*n),
        (Screen::InstrumentPick, Action::Choose { id: ChoiceId::Name(key) }) => card(key, PICK),
        (Screen::InstrumentPick, Action::Reroll) => spot(Spot::Button("Re-roll"), REROLL),
        (Screen::TreasurePick, Action::Choose { id: ChoiceId::Name(key) }) => card(key, TREASURE),
        (Screen::Interrogation, Action::Choose { id: ChoiceId::Name(key) }) => card(key, DOOR),
        _ => None,
    }
}

/// Where `click` sits on a `w` x `h` frame at interface scale `u`: its
/// centre, half width and half height in frame px.
pub fn to_frame(click: &Click, w: usize, h: usize, u: f32) -> (f32, f32, f32, f32) {
    let ((x, y), scale) = frame_point((click.at.0 as f32, click.at.1 as f32), click.layout, w, h, u);
    (x, y, click.half.0 * scale, click.half.1 * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::data::{Gift, Tool};
    use crate::reader::panels::{Panels, read_panels_titled};
    use std::path::Path;

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
        let Some(dir) = super::super::training("s16") else { return };
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
        let Some(dir) = super::super::training("s16") else { return };
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
        let Some(dir) = super::super::training("s16") else { return };
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
        let (x, y, hw, hh) = to_frame(&click, frame.w, frame.h, u);
        // The button is 210 x 55 px on this frame.
        assert!(inside((x as i32, y as i32), (760, 715, 970, 770)) && (95.0..115.0).contains(&hw) && (22.0..30.0).contains(&hh), "({x}, {y}) half {hw} x {hh}");
    }

    /// The four Warden's Treasure cards, about 420 px across, centre at
    /// x 310, 747, 1185 and 1623 in y 190-855 (rec1-t085).
    #[test]
    fn treasure_cards() {
        let Some(dir) = super::super::training("s16") else { return };
        let pick = read(&dir, "rec1-t085.png");
        assert_eq!(pick.screen, Screen::TreasurePick);
        for (gift, x) in Gift::ALL.iter().zip([310, 747, 1185, 1623]) {
            let action = Action::Choose { id: ChoiceId::Name(gift.key().to_string()) };
            assert!(inside(at(&pick, action).unwrap(), (x - 60, 470, x + 60, 580)), "{gift:?}");
        }
    }
}
