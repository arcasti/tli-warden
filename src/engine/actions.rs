//! The public actions (js/engine.js lines 1144-1783), each wrapped like the
//! JS `bounded`: the incident markers are synced after every action.

use super::data::{Beast, Card, Gift, RAINBOW, Tool, data};
use super::devour::{devour, devour_next, end_chain, hunt, refresh_chain};
use super::gifts::{apply_gift, interrogate};
use super::incidents::{ratway_move, resolve_event};
use super::rng::{pick_index, shuffle};
use super::rules::{
    How, advance_day, after_growth, check_prey, destinations_from_dice, empty_cell, fires, land, make_inmate,
    open_random, roll_inmate, sync_markers, weights,
};
use super::state::{Inmate, Pending, Phase, State};
use super::tools::{board_inmates, promote, scout};
use serde::{Deserialize, Serialize};

/// What a choice names: an option key, or a cell for the cell choices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChoiceId {
    Cell(usize),
    Name(String),
}

/// A move as the JS solver writes it (js/solver.js apply).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    Choose { id: ChoiceId },
    Event,
    Patrol { cell: usize },
    Devour { to: usize },
    Chain { to: usize },
    EndChain,
    Swap,
    /// The Hunt marks the inmate in `cell` as the Lion's prey.
    Hunt { cell: usize },
    End,
}

/// The JS `{ ok }` result: a refused action changes nothing but the markers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Fail(&'static str),
}

impl Outcome {
    pub fn is_ok(self) -> bool {
        self == Outcome::Ok
    }
}

/// Runs one action, then syncs the markers as `bounded` does.
pub fn apply(s: &mut State, action: &Action) -> Outcome {
    let out = match action {
        Action::Choose { id } => choose(s, id),
        Action::Event => resolve_event(s),
        Action::Patrol { cell } => patrol(s, *cell),
        Action::Devour { to } => devour(s, *to),
        Action::Chain { to } => devour_next(s, *to),
        Action::EndChain => end_chain(s),
        Action::Swap => swap(s),
        Action::Hunt { cell } => hunt(s, *cell),
        Action::End => end_day(s),
    };
    sync_markers(s);
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Need {
    Patrol,
    Manage,
}

pub(crate) fn guard(s: &State, need: Need) -> Option<&'static str> {
    if s.phase == Phase::Ended {
        return Some("This run is over. Start a new run to keep playing.");
    }
    if !s.pending.is_empty() {
        return Some("Make your pending choice first.");
    }
    if s.phase == Phase::Event {
        return Some("Confirm the Prison Incident first.");
    }
    if need == Need::Manage && s.phase == Phase::Patrol {
        return Some("Move first: go to one of today’s destinations.");
    }
    if need == Need::Patrol && s.phase != Phase::Patrol {
        return Some("Today’s move is done.");
    }
    None
}

pub fn patrol(s: &mut State, n: usize) -> Outcome {
    if let Some(reason) = guard(s, Need::Patrol) {
        return Outcome::Fail(reason);
    }
    if !s.destinations.contains(&n) {
        return Outcome::Fail("Move to one of today’s destinations.");
    }
    s.stats.patrols += 1;
    land(s, n);
    Outcome::Ok
}

/// Replace: the inmate in the warden's cell makes way for a new one.
pub fn swap(s: &mut State) -> Outcome {
    if let Some(reason) = guard(s, Need::Manage) {
        return Outcome::Fail(reason);
    }
    let d = data();
    let n = s.position;
    let Some(old) = s.cells[n].inmate.filter(|_| !s.cells[n].sealed) else {
        return Outcome::Fail("The warden’s cell has no Inmate to replace.");
    };
    if s.used.swap >= s.params.swaps_per_day {
        return Outcome::Fail("No Replace left today.");
    }
    let mut weights = weights(&s.params.tier_weights);
    if s.has(Tool::Rope) {
        weights[0] = 0.0;
        weights[1] = 0.0;
    }
    if !weights.iter().any(|&w| w > 0.0) {
        return Outcome::Fail("No rarity is left to roll. Adjust the rarity weights in the Lab.");
    }
    s.cells[n].inmate = None;
    check_prey(s);
    // Scapegoat's Guillotine: with enough other inmates on the block, some are
    // removed and the new inmate is sure to be rarer than the old.
    let scapegoat = d.tool(Tool::Scapegoat);
    let mut others = s.board_cells();
    let sacrifice = s.has(Tool::Scapegoat) && others.len() >= scapegoat.min_others.unwrap_or(0);
    if sacrifice {
        shuffle(&mut s.rng, &mut others);
        others.truncate(scapegoat.count.unwrap_or(0));
        for c in others {
            empty_cell(s, c);
        }
        check_prey(s);
        for w in weights.iter_mut().take(old.tier.max(0) as usize + 1) {
            *w = 0.0;
        }
    }
    let mut fresh = if sacrifice && !weights.iter().any(|&w| w > 0.0) {
        let beast = Beast::ALL[pick_index(&mut s.rng, &self::weights(&s.params.beast_weights))];
        make_inmate(s, beast, old.tier + 1)
    } else {
        roll_inmate(s, &weights)
    };
    if s.has(Tool::Pestle) {
        fresh.beast = old.beast;
    }
    // Burning Stake: a replaced inmate below Legendary can bring in a Legendary.
    let stake = d.tool(Tool::Stake).tier.unwrap_or(RAINBOW);
    if old.tier < stake && fires(s, Tool::Stake) && fresh.tier < stake {
        fresh.tier = stake;
    }
    s.cells[n].inmate = Some(fresh);
    s.used.swap += 1;
    s.stats.swaps += 1;
    after_growth(s, fresh);
    // Severed Iron Maiden: as many rarer inmates as less rare ones on the block.
    if s.params.maiden && s.has(Tool::Maiden) && s.board_cell_of(fresh.id).is_some() {
        let rest: Vec<i32> = board_inmates(s).iter().filter(|i| i.id != fresh.id).map(|i| i.tier).collect();
        if rest.iter().filter(|&&t| t > fresh.tier).count() == rest.iter().filter(|&&t| t < fresh.tier).count() {
            fresh = promote(s, fresh, d.tool(Tool::Maiden).tiers.unwrap_or(0), false);
        }
    }
    if fires(s, Tool::Scales) {
        scout(s);
    }
    // Gaoler's Execution Axe: a swap that raised the rarity opens random cells.
    if s.has(Tool::Axe) && fresh.tier > old.tier {
        open_random(s, d.tool(Tool::Axe).count.unwrap_or(0), How::Tool);
    }
    // Tusk: inmates on the block each gain a tier.
    if s.cells[n].ivory && s.board_cell_of(fresh.id).is_some() {
        let mut pool: Vec<Inmate> = board_inmates(s).into_iter().filter(|i| s.params.ivory_includes_swapped || i.id != fresh.id).collect();
        shuffle(&mut s.rng, &mut pool);
        pool.truncate(s.params.ivory_count as usize);
        for i in pool {
            if s.board_cell_of(i.id).is_some() {
                promote(s, i, 1, false);
            }
        }
    }
    Outcome::Ok
}

pub fn end_day(s: &mut State) -> Outcome {
    if let Some(reason) = guard(s, Need::Manage) {
        return Outcome::Fail(reason);
    }
    advance_day(s);
    Outcome::Ok
}

enum Picked {
    Tool(Tool),
    Gift(Gift),
    Card(Card),
}

/// Picks an option of the first pending choice.
pub fn choose(s: &mut State, id: &ChoiceId) -> Outcome {
    let Some(choice) = s.pending.first() else {
        return Outcome::Fail("There is nothing to choose right now.");
    };
    // A chain choice's options are target cells: picking one is the next bite.
    if let Pending::Chain { .. } = choice {
        return match id {
            ChoiceId::Cell(to) => devour_next(s, *to),
            ChoiceId::Name(_) => Outcome::Fail("That Inmate cannot consume that target."),
        };
    }
    // A cell choice (Sewer Ratway) moves the warden to the picked cell.
    if let Pending::Cell { .. } = choice {
        return match id {
            ChoiceId::Cell(n) => ratway_move(s, *n),
            ChoiceId::Name(_) => Outcome::Fail("The Sewer Ratway cannot lead there."),
        };
    }
    // The Hunt's target pick marks the picked cell's inmate as prey.
    if let Pending::Prey { .. } = choice {
        return match id {
            ChoiceId::Cell(n) => {
                let n = *n;
                s.pending.remove(0);
                hunt(s, n)
            }
            ChoiceId::Name(_) => Outcome::Fail("The Hunt cannot mark that target."),
        };
    }
    let picked = match (choice, id) {
        (Pending::Tool { options, .. }, ChoiceId::Name(key)) => Tool::from_key(key).filter(|t| options.contains(t)).map(Picked::Tool),
        (Pending::Gift { options, .. }, ChoiceId::Name(key)) => Gift::from_key(key).filter(|g| options.contains(g)).map(Picked::Gift),
        (Pending::Interrogation { options }, ChoiceId::Name(key)) => Card::from_key(key).filter(|c| options.contains(c)).map(Picked::Card),
        _ => None,
    };
    let Some(picked) = picked else {
        return Outcome::Fail("That option is not on offer.");
    };
    let choice = s.pending.remove(0);
    match picked {
        Picked::Card(card) => interrogate(s, card),
        Picked::Gift(gift) => {
            // Opulence: the next pick of the same Warden's Treasure, without this reward.
            if let Pending::Gift { options, picks } = choice {
                let rest: Vec<Gift> = options.into_iter().filter(|&g| g != gift).collect();
                if picks > 1 && !rest.is_empty() {
                    s.pending.insert(0, Pending::Gift { options: rest, picks: picks - 1 });
                }
            }
            apply_gift(s, gift);
        }
        Picked::Tool(tool) => take_tool(s, tool),
    }
    refresh_chain(s);
    Outcome::Ok
}

/// Adds `tool` to the held instruments with the effects of picking it.
pub fn take_tool(s: &mut State, tool: Tool) {
    s.tools.push(tool);
    // A "-1 daily destination option" instrument taken before today's move drops a die now.
    let fewer = data().tool(tool).fewer_dice;
    if fewer > 0 && s.phase == Phase::Patrol && s.dice.len() > 1 {
        let keep = (s.dice.len() as i32 - fewer).max(1) as usize;
        s.dice.truncate(keep);
        destinations_from_dice(s);
    }
    // A later tool pick drawn before this one may offer the same tool.
    for next in &mut s.pending {
        if let Pending::Tool { options, .. } = next {
            options.retain(|&t| t != tool);
        }
    }
    s.pending.retain(|p| !p.is_empty());
}
