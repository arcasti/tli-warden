//! Prison Incident effects (js/engine.js chainReaction to resolveEvent): the
//! confirm, each incident's effect and the Sewer Ratway move.

use super::actions::Outcome;
use super::data::{Beast, ObjectiveKind, data};
use super::devour::{eats, refresh_chain};
use super::rng::{pick_index, pick_one, rand, shuffle};
use super::rules::{How, advance_objective, after_growth, check_prey, door_weights, empty_cell, incident_weight, land, open_cell, open_random};
use super::state::{Inmate, Pending, Phase, State};
use super::tools::{board_inmates, hidden_of, promote};

pub const EXECUTION_EFFICIENCY: u32 = 222;
pub const SEWER_RATWAY: u32 = 241;
pub const CHAIN_REACTION: u32 = 242;

/// The incident popup's one Confirm button: there is no decline.
pub fn resolve_event(s: &mut State) -> Outcome {
    let Some(event) = s.event.filter(|_| s.phase == Phase::Event) else {
        return Outcome::Fail("There is no Prison Incident to confirm.");
    };
    if !s.pending.is_empty() {
        return Outcome::Fail("Make your pending choice first.");
    }
    s.event = None;
    s.phase = Phase::Manage;
    incident_effect(s, event.id, event.cell);
    advance_objective(s, ObjectiveKind::EventTriggerCount, 1);
    Outcome::Ok
}

/// INCIDENT_EFFECTS: the effect of incident `id` fired on cell `n`.
fn incident_effect(s: &mut State, id: u32, n: usize) {
    match id {
        201 => high_risk_convict(s),
        202 => open_random(s, data().incident(202).count, How::Side),
        203 => behind_closed_doors(s, n),
        211 => ravenous_appetite(s),
        212 => extra_portion(s),
        213 => shared_fortune(s, n),
        221 => archenemy(s, n),
        EXECUTION_EFFICIENCY => execution_efficiency(s, n),
        231 => crow_whispers(s),
        232 => treasure_hunt(s),
        SEWER_RATWAY => sewer_ratway(s),
        // The next move opens every unopened cell on its path (see land).
        CHAIN_REACTION => s.chain = true,
        251 => transfer_of_convicts(s, n),
        252..=255 => usurpation(s, id),
        other => panic!("incident {other} has no effect"),
    }
}

/// The inmate in the incident's cell, if the cell is open.
fn incident_inmate(s: &State, n: usize) -> Option<Inmate> {
    s.cells[n].inmate.filter(|_| !s.cells[n].sealed)
}

fn random_sealed(s: &mut State) -> Option<usize> {
    let sealed = s.sealed_cells();
    if sealed.is_empty() {
        return None;
    }
    Some(sealed[pick_one(&mut s.rng, sealed.len())])
}

/// Execution Efficiency: the lower-tier inmates the cell's inmate counters.
pub fn execution_victims(s: &State, n: usize) -> Vec<usize> {
    let Some(exec) = incident_inmate(s, n) else { return Vec::new() };
    let prey = eats(exec.beast);
    s.board_cells()
        .into_iter()
        .filter(|&c| c != n && s.cells[c].inmate.is_some_and(|i| prey.contains(&i.beast) && i.tier < exec.tier))
        .collect()
}

/// Execution Efficiency (EV 222): the inmate removes every victim and rises
/// one tier per removal.
fn execution_efficiency(s: &mut State, n: usize) {
    let Some(exec) = incident_inmate(s, n) else { return };
    let victims = execution_victims(s, n);
    for &c in &victims {
        empty_cell(s, c);
    }
    s.stats.executions += 1;
    check_prey(s);
    if !victims.is_empty() {
        promote(s, exec, victims.len() as i32, s.params.execution_rerolls);
    }
}

/// High-Risk Convict (EV 201): a random unopened cell opens with an inmate of
/// the text's rarity or better, rolled by the door odds above that floor.
fn high_risk_convict(s: &mut State) {
    let Some(cell) = random_sealed(s) else { return };
    let h = hidden_of(s, cell);
    let floor = data().incident(201).tier;
    let mut tier = h.tier;
    if tier < floor {
        let w: Vec<f64> = door_weights(s).into_iter().enumerate().map(|(t, x)| if t as i32 >= floor { x } else { 0.0 }).collect();
        tier = if w.iter().any(|&x| x > 0.0) { pick_index(&mut s.rng, &w) as i32 } else { floor };
    }
    open_cell(s, cell, How::Side, Some((h.beast, tier)))
}

/// Behind Closed Doors (EV 203): the cell's inmate takes the beast with the
/// fewest inmates on the board (ties drawn) and the text's rarity.
fn behind_closed_doors(s: &mut State, n: usize) {
    let Some(mut inmate) = incident_inmate(s, n) else { return };
    let board = board_inmates(s);
    let counts = Beast::ALL.map(|b| board.iter().filter(|i| i.beast == b).count());
    let fewest = counts.iter().copied().min().unwrap_or(0);
    let pool: Vec<Beast> = Beast::ALL.into_iter().zip(counts).filter(|&(_, c)| c == fewest).map(|(b, _)| b).collect();
    inmate.beast = pool[pick_one(&mut s.rng, pool.len())];
    inmate.tier = data().incident(203).tier;
    s.cells[n].inmate = Some(inmate);
    check_prey(s);
    after_growth(s, inmate);
}

/// Ravenous Appetite (EV 211): a random lowest-rarity inmate gets some tries
/// at a chance to rise one tier each.
fn ravenous_appetite(s: &mut State) {
    let board = board_inmates(s);
    let Some(low) = board.iter().map(|i| i.tier).min() else { return };
    let pool: Vec<Inmate> = board.into_iter().filter(|i| i.tier == low).collect();
    let who = pool[pick_one(&mut s.rng, pool.len())];
    let e = data().incident(211);
    let mut rose = 0;
    for _ in 0..e.count {
        if rand(&mut s.rng) * 100.0 < e.chance {
            rose += 1;
        }
    }
    if rose > 0 {
        promote(s, who, rose, false);
    }
}

/// Raises each of `picks` by `tiers`, skipping any that left the board on the way.
fn raise_all(s: &mut State, picks: &[Inmate], tiers: i32) {
    for &i in picks {
        if s.board_cell_of(i.id).is_some() {
            promote(s, i, tiers, false);
        }
    }
}

/// Extra Portion (EV 212): up to a few random inmates rise.
fn extra_portion(s: &mut State) {
    let e = data().incident(212);
    let mut picks = board_inmates(s);
    shuffle(&mut s.rng, &mut picks);
    picks.truncate(e.count);
    raise_all(s, &picks, e.tiers);
}

/// Shared Fortune (EV 213): the cell's inmate rises per other inmate on the
/// board with its beast.
fn shared_fortune(s: &mut State, n: usize) {
    let Some(inmate) = incident_inmate(s, n) else { return };
    let same = board_inmates(s).iter().filter(|i| i.id != inmate.id && i.beast == inmate.beast).count() as i32;
    let by = same * data().incident(213).tiers;
    if by > 0 {
        promote(s, inmate, by, false);
    }
}

/// Archenemy (EV 221): a random unopened cell opens on a same-rarity inmate
/// the cell's inmate counters; after-opening instruments fire.
fn archenemy(s: &mut State, n: usize) {
    let Some(inmate) = incident_inmate(s, n) else { return };
    let Some(cell) = random_sealed(s) else { return };
    let prey = eats(inmate.beast);
    let beast = prey[pick_one(&mut s.rng, prey.len())];
    open_cell(s, cell, How::Tool, Some((beast, inmate.tier)))
}

/// Shows the rarity behind each door in `cells` (Scratch-mark knowledge).
fn reveal_rarity(s: &mut State, cells: &[usize]) {
    for &c in cells {
        s.cells[c].known = s.cells[c].known.max(1);
    }
}

/// Crow Whispers (EV 231): the rarity behind the next unopened cells
/// clockwise from the warden; rat nests are skipped.
fn crow_whispers(s: &mut State) {
    let d = data();
    let count = d.incident(231).count;
    let mut cells = Vec::new();
    for k in 1..d.cell_count {
        if cells.len() >= count {
            break;
        }
        let c = (s.position + k) % d.cell_count;
        if s.cells[c].sealed && s.cells[c].nest.is_none() {
            cells.push(c);
        }
    }
    for &c in &cells {
        hidden_of(s, c);
    }
    reveal_rarity(s, &cells);
}

/// "Treasure" Hunt (EV 232): every unopened cell below the text's rarity shows it.
fn treasure_hunt(s: &mut State) {
    let doors: Vec<usize> = s.cells.iter().filter(|c| c.sealed && c.nest.is_none()).map(|c| c.n).collect();
    for &c in &doors {
        hidden_of(s, c);
    }
    let below = data().incident(232).tier;
    let low: Vec<usize> = doors.into_iter().filter(|&c| s.cells[c].hidden.is_some_and(|h| h.tier < below)).collect();
    reveal_rarity(s, &low);
}

/// Sewer Ratway (EV 241): the player picks any cell but the Interrogation
/// Room; choose() then moves the warden there.
fn sewer_ratway(s: &mut State) {
    let room = data().room;
    let options = (0..s.cells.len()).filter(|&n| n != room).collect();
    s.pending.push(Pending::Cell { id: SEWER_RATWAY, options });
}

/// The Sewer Ratway move lands like a patrol: door, shackles, marker.
pub fn ratway_move(s: &mut State, n: usize) -> Outcome {
    if !matches!(s.pending.first(), Some(Pending::Cell { options, .. }) if options.contains(&n)) {
        return Outcome::Fail("The Sewer Ratway cannot lead there.");
    }
    s.pending.remove(0);
    land(s, n);
    refresh_chain(s);
    Outcome::Ok
}

/// Transfer of Convicts (EV 251): the cell's inmate is copied into a random
/// unopened cell, which opens; after-opening instruments fire.
fn transfer_of_convicts(s: &mut State, n: usize) {
    let Some(inmate) = incident_inmate(s, n) else { return };
    let Some(cell) = random_sealed(s) else { return };
    open_cell(s, cell, How::Tool, Some((inmate.beast, inmate.tier)))
}

/// Usurpation I-IV (EV 252-255): random inmates rise, then the stage leaves
/// the pool and the next stage takes its weight; markers on the board turn
/// into the next stage (or go, after IV).
fn usurpation(s: &mut State, id: u32) {
    let e = data().incident(id);
    let mut picks = board_inmates(s);
    if e.count != usize::MAX {
        shuffle(&mut s.rng, &mut picks);
        picks.truncate(e.count);
    }
    raise_all(s, &picks, e.tiers);
    let weight = incident_weight(s, id);
    s.incident_weights.insert(id, 0);
    if let Some(next) = e.next {
        s.incident_weights.insert(next, weight);
    }
    for c in &mut s.cells {
        if c.event == Some(id) {
            c.event = e.next;
        }
    }
}
