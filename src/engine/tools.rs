//! Growth and the Torture Instrument effects outside a consume (js/engine.js
//! promote 548-565, the "after opening" block of openCell, beginDay 884-951,
//! filthyClaws, coinOfFate, ravenousChains, shackles and scout).

use super::data::{Beast, RAINBOW, Tool, data};
use super::rng::{pick_index, pick_one, rand, shuffle};
use super::rules::{How, after_growth, check_prey, door_weights, empty_cell, fires, open_cell, open_random, roll_inmate, weights};
use super::state::{Inmate, Nest, State};

/// The cell `step` places from `n` on the ring.
fn ring(n: usize, step: isize) -> usize {
    let cells = data().cell_count as isize;
    ((n as isize + step + cells) % cells) as usize
}

pub fn board_inmates(s: &State) -> Vec<Inmate> {
    s.cells.iter().filter(|c| c.on_board()).filter_map(|c| c.inmate).collect()
}

/// promote: `who` rises `by` tiers; a consume re-rolls the beast below
/// Chromatic. The JS mutates the inmate object, so the one in its cell is the
/// current one; `who` stands in when it has left every cell. Returns it after.
pub fn promote(s: &mut State, who: Inmate, by: i32, reroll: bool) -> Inmate {
    let at = s.cell_of(who.id);
    let mut inmate = at.and_then(|n| s.cells[n].inmate).unwrap_or(who);
    inmate.tier = (inmate.tier + by).clamp(0, RAINBOW);
    if reroll && inmate.tier < RAINBOW {
        inmate.beast = Beast::ALL[pick_index(&mut s.rng, &weights(&s.params.beast_weights))];
    }
    if let Some(n) = at {
        s.cells[n].inmate = Some(inmate);
    }
    after_growth(s, inmate);
    inmate
}

/// hiddenOf: the occupant behind a sealed door is rolled the first time anyone looks.
pub fn hidden_of(s: &mut State, n: usize) -> Inmate {
    if let Some(h) = s.cells[n].hidden {
        return h;
    }
    let w = door_weights(s);
    let h = roll_inmate(s, &w);
    s.cells[n].hidden = Some(h);
    h
}

/// The "after opening" instruments on a patrolled door or one an instrument
/// opened; `id` is the inmate that came out of cell `n`.
pub fn after_opening(s: &mut State, n: usize, id: u32) {
    let d = data();
    let here = |s: &State| s.cells[n].inmate.filter(|i| i.id == id);
    // Felon's Dog Tag: the inmate behind the door rises.
    if fires(s, Tool::Dogtag)
        && let Some(i) = here(s)
    {
        promote(s, i, d.tool(Tool::Dogtag).tiers.unwrap_or(0), false);
    }
    // Executioner's Blacklist: with few unopened cells left, random inmates rise.
    let blacklist = d.tool(Tool::Blacklist);
    if s.has(Tool::Blacklist) && s.sealed_cells().len() <= blacklist.max_sealed.unwrap_or(0) {
        let mut picks = board_inmates(s);
        shuffle(&mut s.rng, &mut picks);
        picks.truncate(blacklist.count.unwrap_or(0));
        for i in picks {
            if s.board_cell_of(i.id).is_some() {
                promote(s, i, blacklist.tiers.unwrap_or(0), false);
            }
        }
    }
    // Searing Branding Iron: a copy of the inmate opens another unopened cell.
    if fires(s, Tool::Fire)
        && let Some(i) = here(s)
    {
        let sealed = s.sealed_cells();
        if !sealed.is_empty() {
            let target = sealed[pick_one(&mut s.rng, sealed.len())];
            open_cell(s, target, How::Side, Some((i.beast, i.tier)));
        }
    }
    // Lost Ring of Keys: the doors on both sides open too.
    if fires(s, Tool::Keys) {
        for step in [-1, 1] {
            open_cell(s, ring(n, step), How::Side, None);
        }
    }
    // Forged Key: one more random door opens with no other effect.
    if fires(s, Tool::Forgedkey) {
        open_random(s, d.tool(Tool::Forgedkey).count.unwrap_or(0), How::Side);
    }
}

/// The morning's prey marks: a Lion with several marks rises once, each prey once.
pub fn prey_rise(s: &mut State) {
    let mut risen: Vec<u32> = Vec::new();
    for link in s.prey.clone() {
        if link.next > s.day {
            continue;
        }
        let lion = s.board_cell_of(link.lion).and_then(|n| s.cells[n].inmate);
        let prey = s.board_cell_of(link.prey).and_then(|n| s.cells[n].inmate);
        let (Some(lion), Some(prey)) = (lion, prey) else { continue };
        let lion_rises = !risen.contains(&lion.id);
        risen.push(lion.id);
        if let Some(k) = s.prey.iter().position(|l| l.lion == link.lion && l.prey == link.prey) {
            s.prey[k].rounds -= 1;
            s.prey[k].next = s.day + 1;
            if s.prey[k].rounds <= 0 {
                s.prey.remove(k);
            }
        }
        if lion_rises {
            promote(s, lion, 1, false);
        }
        promote(s, prey, 1, false);
    }
}

/// Rat Nests that are due hatch a rat of the nest rarity.
pub fn hatch_nests(s: &mut State) {
    for n in 0..s.cells.len() {
        if s.cells[n].nest.is_some_and(|nest| nest.due <= s.day) {
            open_cell(s, n, How::Side, Some((Beast::Rat, s.params.nest_tier)));
        }
    }
}

/// The start-of-day instruments, in the beginDay order.
pub fn morning_tools(s: &mut State) {
    if s.has(Tool::Tongs) {
        let mammoths: Vec<usize> = s
            .cells
            .iter()
            .filter(|c| c.on_board() && !c.ivory && c.inmate.is_some_and(|i| i.beast == Beast::Mammoth))
            .map(|c| c.n)
            .collect();
        if !mammoths.is_empty() {
            let n = mammoths[pick_one(&mut s.rng, mammoths.len())];
            s.cells[n].ivory = true;
        }
    }
    if s.has(Tool::Ironbox) {
        let free: Vec<usize> = s.cells.iter().filter(|c| c.sealed && c.nest.is_none()).map(|c| c.n).collect();
        if !free.is_empty() {
            let n = free[pick_one(&mut s.rng, free.len())];
            s.cells[n].nest = Some(Nest { due: s.day + s.params.nest_days });
        }
    }
    if s.has(Tool::Claws) {
        filthy_claws(s);
    }
    if fires(s, Tool::Coin) {
        coin_of_fate(s);
    }
    if s.has(Tool::Chains) {
        ravenous_chains(s);
    }
}

/// Filthy Claws: a random unopened cell gets a Scratch mark.
fn filthy_claws(s: &mut State) {
    let free: Vec<usize> = s.cells.iter().filter(|c| c.sealed && !c.scratch && c.nest.is_none()).map(|c| c.n).collect();
    if free.is_empty() {
        return;
    }
    let n = free[pick_one(&mut s.rng, free.len())];
    hidden_of(s, n);
    let cell = &mut s.cells[n];
    cell.scratch = true;
    cell.known = cell.known.max(1);
}

/// Kin's Coin of Fate: one of a matching pair is removed and a match rises.
fn coin_of_fate(s: &mut State) {
    let same = |a: &Inmate, b: &Inmate| a.beast == b.beast && a.tier == b.tier;
    let board = board_inmates(s);
    let matched: Vec<Inmate> = board.iter().copied().filter(|i| board.iter().any(|o| o.id != i.id && same(o, i))).collect();
    if matched.is_empty() {
        return;
    }
    let gone = matched[pick_one(&mut s.rng, matched.len())];
    let kin: Vec<Inmate> = matched.iter().copied().filter(|i| i.id != gone.id && same(i, &gone)).collect();
    let lucky = kin[pick_one(&mut s.rng, kin.len())];
    if let Some(n) = s.cell_of(gone.id) {
        empty_cell(s, n);
    }
    check_prey(s);
    promote(s, lucky, data().tool(Tool::Coin).tiers.unwrap_or(0), false);
}

/// Ravenous Chains: the neighbours of the highest-rarity inmate are removed,
/// and it rolls its chance to rise once per removal.
fn ravenous_chains(s: &mut State) {
    let cells = s.board_cells();
    let tier = |s: &State, n: usize| s.cells[n].inmate.map_or(0, |i| i.tier);
    let Some(top) = cells.iter().map(|&n| tier(s, n)).max() else { return };
    let tops: Vec<usize> = cells.iter().copied().filter(|&n| tier(s, n) == top).collect();
    let center = tops[pick_one(&mut s.rng, tops.len())];
    let Some(inmate) = s.cells[center].inmate else { return };
    let sides: Vec<usize> = [-1, 1].into_iter().map(|step| ring(center, step)).filter(|&n| s.cells[n].on_board()).collect();
    if sides.is_empty() {
        return;
    }
    for &n in &sides {
        empty_cell(s, n);
    }
    check_prey(s);
    let chains = data().tool(Tool::Chains);
    let mut gain = 0;
    for _ in &sides {
        if rand(&mut s.rng) * 100.0 < chains.chance {
            gain += chains.tiers.unwrap_or(0);
        }
    }
    if gain != 0 {
        promote(s, inmate, gain, false);
    }
}

/// The shackles roll on a move of at least one tile.
pub fn shackles(s: &mut State, steps: usize) {
    let d = data();
    let short = d.tool(Tool::Shortchain);
    if steps > 0 && steps <= short.max_steps.unwrap_or(0) && fires(s, Tool::Shortchain) {
        open_random(s, short.count.unwrap_or(0), How::Side);
    }
    let long = d.tool(Tool::Longchain);
    if steps >= long.min_steps.unwrap_or(usize::MAX) && fires(s, Tool::Longchain) {
        let board = board_inmates(s);
        if board.is_empty() {
            return;
        }
        let lucky = board[pick_one(&mut s.rng, board.len())];
        promote(s, lucky, long.tiers.unwrap_or(0), false);
    }
}

/// Warden's Scales: the best inmate behind the locked doors is shown.
pub fn scout(s: &mut State) {
    let doors: Vec<usize> = s.cells.iter().filter(|c| c.sealed && c.nest.is_none()).map(|c| c.n).collect();
    if doors.is_empty() {
        return;
    }
    let tiers: Vec<i32> = doors.iter().map(|&n| hidden_of(s, n).tier).collect();
    let top = tiers.iter().copied().max().unwrap_or(0);
    let best: Vec<usize> = doors.iter().zip(&tiers).filter(|(_, t)| **t == top).map(|(&n, _)| n).collect();
    let n = best[pick_one(&mut s.rng, best.len())];
    s.cells[n].known = 2;
}
