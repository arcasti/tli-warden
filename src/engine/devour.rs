//! Consume, chain consumes and Hunt (js/engine.js canEat 451, devourOptions
//! 1015-1069, huntTargets and hunt 1138-1154, doDevour to endChain 1473-1597).

use super::actions::{Need, Outcome, guard};
use super::data::{Beast, CAGE_FIGHT, ObjectiveKind, RAINBOW, Tool, data};
use super::rng::{pick_one, rand};
use super::rules::{How, advance_objective, check_prey, empty_cell, fires, open_cell};
use super::state::{Inmate, Pending, Phase, PreyLink, State};
use super::tools::promote;

/// A target of a consume: an open cell, or a scratched door a Wolf eats through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bite {
    pub cell: usize,
    pub scratch: bool,
}

pub fn eats(beast: Beast) -> &'static [Beast] {
    &data().food_chain[beast.index()]
}

/// The species instruments widen the chain: the Fanged Guillotine lets a Wolf
/// eat lower rarities, the Elephant-Foot Maul a Mammoth higher ones, the Iron
/// Crown a Lion any same-rarity inmate, the Skewered Rat Heads a Rat a Rat.
pub fn can_eat(s: &State, eater: &Inmate, meal: Option<&Inmate>) -> bool {
    let Some(meal) = meal else { return false };
    if eater.id == meal.id || eater.tier >= RAINBOW {
        return false;
    }
    let counters = eats(eater.beast).contains(&meal.beast);
    if eater.tier > meal.tier {
        return counters && ((eater.beast == Beast::Wolf && s.has(Tool::Guillotine)) || s.theme == Some(CAGE_FIGHT));
    }
    if eater.tier < meal.tier {
        return counters && eater.beast == Beast::Mammoth && s.has(Tool::Maul);
    }
    counters
        || (eater.beast == Beast::Lion && s.has(Tool::Crown))
        || (eater.beast == Beast::Rat && meal.beast == Beast::Rat && s.has(Tool::Skewer))
}

pub fn devour_options(s: &State, n: usize) -> Vec<Bite> {
    bites(s, n).collect()
}

/// The consumes open to the inmate in cell n, in cell order, without collecting them.
pub fn bites(s: &State, n: usize) -> impl Iterator<Item = Bite> {
    let eater = s.cells[n].inmate.filter(|_| !s.cells[n].sealed);
    eater.into_iter().flat_map(move |eater| {
        s.cells.iter().filter(move |c| c.n != n).filter_map(move |c| {
            if !c.sealed && can_eat(s, &eater, c.inmate.as_ref()) {
                Some(Bite { cell: c.n, scratch: false })
            } else if c.sealed
                && c.scratch
                && eater.beast == Beast::Wolf
                && c.hidden.is_some_and(|h| h.tier == eater.tier)
                && eater.tier < RAINBOW
            {
                Some(Bite { cell: c.n, scratch: true })
            } else {
                None
            }
        })
    })
}

/// One consume a day (canDevourNow); a chain's later bites don't count.
pub fn can_devour_now(s: &State) -> bool {
    s.used.devour < 1
}

/// Today's first-bite options for the inmate in the warden's cell (startOptions).
pub fn start_options(s: &State) -> Vec<Bite> {
    let cell = &s.cells[s.position];
    if !can_devour_now(s) || s.phase != Phase::Manage || !cell.on_board() {
        return Vec::new();
    }
    devour_options(s, cell.n)
}

fn option_cells(s: &State, n: usize) -> Vec<usize> {
    devour_options(s, n).iter().map(|o| o.cell).collect()
}

/// Oppress (Fanged Guillotine): a Wolf consuming a lower rarity loses upgrade
/// chance per tier of difference. From 0 to 1.
pub fn upgrade_chance(s: &State, eater: &Inmate, meal: &Inmate) -> f64 {
    let diff = eater.tier - meal.tier;
    if diff <= 0 || eater.beast != Beast::Wolf || !s.has(Tool::Guillotine) {
        return 1.0;
    }
    (100.0 - data().oppress_per_tier * f64::from(diff)).max(0.0) / 100.0
}

/// The tiers a bite adds before any roll; crowd counts the inmates on the
/// block before the meal leaves it.
pub fn sure_gain(s: &State, eater: &Inmate, meal: &Inmate, scratch: bool, crowd: usize) -> i32 {
    let d = data();
    let mut gain = if scratch { 2 + i32::from(s.params.scratch_stacks_base) } else { 1 };
    let tiers = |t: Tool| d.tool(t).tiers.unwrap_or(0);
    if s.has(Tool::Maul) && eater.beast == Beast::Mammoth {
        gain = gain.max(meal.tier + d.tool(Tool::Maul).above.unwrap_or(0) - eater.tier);
    }
    if s.has(Tool::Muzzle) && crowd <= d.tool(Tool::Muzzle).max_inmates.unwrap_or(0) {
        gain += tiers(Tool::Muzzle);
    }
    if s.has(Tool::Skewer) && eater.beast == Beast::Rat {
        gain += tiers(Tool::Skewer);
    }
    if s.has(Tool::Crown) && eater.beast == Beast::Lion {
        gain += tiers(Tool::Crown);
    }
    gain
}

/// One bite without the phase checks; `chained` marks a bite after the first
/// of a chain (Maddened Beast's Barbed Collar).
pub fn do_devour(s: &mut State, from: usize, option: Bite, chained: bool) {
    let d = data();
    let (Some(eater), Some(meal)) = (s.cells[from].inmate, {
        let c = &s.cells[option.cell];
        if c.sealed { c.hidden } else { c.inmate }
    }) else {
        return;
    };
    let crowd = s.board_cells().len();
    let chance = upgrade_chance(s, &eater, &meal);
    let mut gain = sure_gain(s, &eater, &meal, option.scratch, crowd);
    empty_cell(s, option.cell);
    if chance < 1.0 && rand(&mut s.rng) >= chance {
        gain = 0;
    } else {
        let collar: &[Tool] = if chained { &[Tool::Collar] } else { &[] };
        for &tool in [Tool::Saw, Tool::Whip].iter().chain(collar) {
            if fires(s, tool) {
                gain += d.tool(tool).tiers.unwrap_or(0);
            }
        }
    }
    let revenge = fires(s, Tool::Revenge);
    let cage = fires(s, Tool::Beastcage);
    // Cage Fight: consuming does not re-roll the eater's beast.
    let eater = promote(s, eater, gain, s.theme != Some(CAGE_FIGHT));
    s.stats.devours += 1;
    advance_objective(s, ObjectiveKind::DevourCount, 1);
    check_prey(s);
    // Crowded Beastcage: the meal's cell opens again at once, with no other effect.
    if cage && s.cells[option.cell].sealed {
        open_cell(s, option.cell, How::Side, None);
    }
    // Avenger's Casket: a random door opens with a meal for the eater.
    if revenge && s.board_cell_of(eater.id).is_some() {
        let sealed = s.sealed_cells();
        if !sealed.is_empty() {
            let target = sealed[pick_one(&mut s.rng, sealed.len())];
            let food = eats(eater.beast);
            let food = food[pick_one(&mut s.rng, food.len())];
            open_cell(s, target, How::Side, Some((food, eater.tier)));
        }
    }
}

/// The day's consume, started by the inmate in the warden's cell.
pub fn devour(s: &mut State, to: usize) -> Outcome {
    if let Some(reason) = guard(s, Need::Manage) {
        return Outcome::Fail(reason);
    }
    if s.used.devour >= 1 {
        return Outcome::Fail("Today’s Consume is used. End the day for another.");
    }
    let from = s.position;
    let Some(option) = devour_options(s, from).into_iter().find(|o| o.cell == to) else {
        return Outcome::Fail("That Inmate cannot consume that target.");
    };
    let eater_id = s.cells[from].inmate.map_or(0, |i| i.id);
    s.used.devour += 1;
    do_devour(s, from, option, false);
    offer_chain(s, eater_id);
    Outcome::Ok
}

/// While the eater has a target, a chain choice queues behind any pick the bite earned.
pub fn offer_chain(s: &mut State, eater_id: u32) {
    let Some(n) = s.board_cell_of(eater_id) else { return };
    let options = option_cells(s, n);
    if !options.is_empty() {
        s.pending.push(Pending::Chain { from: n, eater_id, options });
    }
}

/// Re-reads queued chain choices after another pick changed the board; drops
/// those whose eater has no target left.
pub fn refresh_chain(s: &mut State) {
    let mut pending = std::mem::take(&mut s.pending);
    pending.retain_mut(|p| {
        let Pending::Chain { from, eater_id, options } = p else { return true };
        let cell = s.board_cell_of(*eater_id);
        *options = cell.map(|n| option_cells(s, n)).unwrap_or_default();
        if let Some(n) = cell {
            *from = n;
        }
        !options.is_empty()
    });
    s.pending = pending;
}

/// The next bite of a chain: the eater consumes the chosen target.
pub fn devour_next(s: &mut State, to: usize) -> Outcome {
    let Some(&Pending::Chain { from, eater_id, ref options }) = s.pending.first() else {
        return Outcome::Fail("There is no Chain Consume to continue.");
    };
    let option = devour_options(s, from).into_iter().find(|o| o.cell == to);
    let Some(option) = option.filter(|_| options.contains(&to)) else {
        return Outcome::Fail("That Inmate cannot consume that target.");
    };
    s.pending.remove(0);
    do_devour(s, from, option, true);
    s.stats.chains += 1;
    offer_chain(s, eater_id);
    Outcome::Ok
}

pub fn end_chain(s: &mut State) -> Outcome {
    if !matches!(s.pending.first(), Some(Pending::Chain { .. })) {
        return Outcome::Fail("There is no Chain Consume to end.");
    }
    s.pending.remove(0);
    Outcome::Ok
}

/// Hunt targets: with the Blood-Stained Trap, a Lion in the warden's cell
/// marks every open Consumable target that has no prey mark yet.
pub fn hunt_targets(s: &State) -> Vec<usize> {
    let n = s.position;
    let lion = s.cells[n].inmate.filter(|i| !s.cells[n].sealed && i.beast == Beast::Lion);
    if s.phase != Phase::Manage || !s.has(Tool::Trap) || s.used.hunt >= 1 || lion.is_none() {
        return Vec::new();
    }
    let linked = |id: u32| s.prey.iter().any(|l| l.lion == id || l.prey == id);
    devour_options(s, n)
        .into_iter()
        .filter(|o| !o.scratch && !s.cells[o.cell].inmate.is_some_and(|i| linked(i.id)))
        .map(|o| o.cell)
        .collect()
}

pub fn hunt(s: &mut State) -> Outcome {
    if let Some(reason) = guard(s, Need::Manage) {
        return Outcome::Fail(reason);
    }
    let targets = hunt_targets(s);
    let Some(lion) = s.cells[s.position].inmate.filter(|_| !targets.is_empty()) else {
        return Outcome::Fail(
            "Hunt needs the Blood-Stained Trap, a Berserk Lion in the warden's cell and a Consumable target without a Prey mark.",
        );
    };
    for n in targets {
        if let Some(prey) = s.cells[n].inmate {
            s.prey.push(PreyLink { lion: lion.id, prey: prey.id, prey_beast: prey.beast, rounds: s.params.prey_rounds as i32, next: s.day + 1 });
        }
    }
    s.used.hunt += 1;
    Outcome::Ok
}
