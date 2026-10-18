//! The Interrogation Room cards and the Warden's Treasure rewards
//! (js/engine.js interrogationRoomStop and interrogate 1094-1131, applyGift 1729-1765).

use super::data::{Beast, Card, Gift, RAINBOW, TYRANNY, data};
use super::devour::{can_eat, devour_options, do_devour};
use super::rng::{pick_one, shuffle};
use super::rules::{How, add_marker, check_prey, open_cell};
use super::state::{Inmate, Pending, State};
use super::tools::{board_inmates, promote};

/// A stop in the Interrogation Room offers Instigate or Bestow. Tyranny runs
/// both cards its stated number of extra times first.
pub fn interrogation_room_stop(s: &mut State) {
    let extra = if s.theme == Some(TYRANNY) { data().theme(TYRANNY).and_then(|t| t.value).unwrap_or(0) } else { 0 };
    for _ in 0..extra {
        interrogate(s, Card::Instigate);
        interrogate(s, Card::Bestow);
    }
    s.pending.push(Pending::Interrogation { options: Card::ALL.to_vec() });
}

/// Instigate: a drawn incident marker goes on a fitting free cell. Bestow: a
/// random inmate on the block below Chromatic rises a tier.
pub fn interrogate(s: &mut State, card: Card) {
    if card == Card::Instigate {
        add_marker(s, None);
        return;
    }
    let board: Vec<Inmate> = board_inmates(s).into_iter().filter(|i| i.tier < RAINBOW).collect();
    if board.is_empty() {
        return;
    }
    let lucky = board[pick_one(&mut s.rng, board.len())];
    promote(s, lucky, 1, false);
}

fn hungry(s: &State, id: u32) -> bool {
    s.cell_of(id).is_none_or(|n| devour_options(s, n).is_empty())
}

pub fn apply_gift(s: &mut State, gift: Gift) {
    match gift {
        // Three random doors open with no other effect.
        Gift::Ring => {
            let mut doors = s.sealed_cells();
            shuffle(&mut s.rng, &mut doors);
            doors.truncate(3);
            for n in doors {
                open_cell(s, n, How::Side, None);
            }
        }
        // Every inmate with a target consumes a random open one.
        Gift::Horn => {
            let eaters: Vec<u32> = board_inmates(s).into_iter().filter(|i| !hungry(s, i.id)).map(|i| i.id).collect();
            for id in eaters {
                let Some(n) = s.board_cell_of(id) else { continue };
                let options: Vec<_> = devour_options(s, n).into_iter().filter(|o| !o.scratch).collect();
                if options.is_empty() {
                    continue;
                }
                let option = options[pick_one(&mut s.rng, options.len())];
                do_devour(s, n, option, false);
            }
        }
        // Everyone below Epic rises a tier.
        Gift::Wine => {
            let low: Vec<Inmate> = board_inmates(s).into_iter().filter(|i| i.tier < 3).collect();
            for i in low {
                if s.board_cell_of(i.id).is_some() {
                    promote(s, i, 1, false);
                }
            }
        }
        // Up to four inmates without a target become a beast that has one.
        Gift::Brand => {
            let mut picks: Vec<u32> = board_inmates(s).into_iter().filter(|i| hungry(s, i.id)).map(|i| i.id).collect();
            shuffle(&mut s.rng, &mut picks);
            picks.truncate(4);
            for id in picks {
                let Some(n) = s.board_cell_of(id) else { continue };
                let Some(me) = s.cells[n].inmate else { continue };
                if !devour_options(s, n).is_empty() {
                    continue;
                }
                let board = board_inmates(s);
                let possible: Vec<Beast> = Beast::ALL
                    .into_iter()
                    .filter(|&beast| board.iter().any(|o| can_eat(s, &Inmate { beast, ..me }, Some(o))))
                    .collect();
                if !possible.is_empty() {
                    let beast = possible[pick_one(&mut s.rng, possible.len())];
                    s.cells[n].inmate = Some(Inmate { beast, ..me });
                }
            }
            check_prey(s);
        }
    }
}
