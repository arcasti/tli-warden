//! Valuing moves (js/solver.js 336-497): incident markers, patrol
//! destinations, prey marks, devours, swaps and the gift picks.

use super::chain::{best_meal, coin_of_fate, devour_delta, replace_odds};
use super::values::{activation, execution_value, fresh_value, stair_delta, swap_value};
use super::{Comp, Ctx, Rng32, TOP, beasts, composition, context, dist_worth, door_dist, seen_door, worth};
use crate::engine::data::{Beast, Card, RAINBOW, Tool, data};
use crate::engine::devour::{Bite, devour_options, hunt_targets, sure_gain};
use crate::engine::incidents::{CHAIN_REACTION, EXECUTION_EFFICIENCY, SEWER_RATWAY};
use crate::engine::rules::{clockwise_path, land, steps_between};
use crate::engine::state::{Cell, Inmate, State};
use crate::engine::text::{an, describe, incident_name, plural};
use crate::engine::{Action, ChoiceId, apply};

/// Sorts best first, keeping ties in order (the JS stable sort on b - a).
fn best_first(out: &mut [(usize, f64)]) {
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
}

/// incidentScore: the value of the incident marker on a destination. Only
/// the two incidents the engine simulates have a value.
pub fn incident_score(ctx: &Ctx, c: &Cell, comp: &Comp) -> f64 {
    // Behind a door the executioner is whoever comes out; a Chain Reaction
    // landing opens it from the side.
    let execution = || {
        if !c.sealed {
            return c.inmate.map_or(0.0, |i| execution_value(ctx, i.tier as usize, i.beast.index(), comp));
        }
        let (dist, beast) = door_dist(ctx.st, Some(c), !ctx.st.chain);
        let (beasts, share) = beasts(beast);
        let mut exec = 0.0;
        for (t, &p) in dist.iter().enumerate() {
            for b in beasts.clone() {
                exec += (p * share) * execution_value(ctx, t, b, comp);
            }
        }
        exec
    };
    match c.event {
        // A marker whose incident the player can't see is either, evenly.
        Some(_) if c.event_known == Some(false) => 0.5 * execution() + 0.5 * ctx.p.w_chain,
        Some(EXECUTION_EFFICIENCY) => execution(),
        Some(CHAIN_REACTION) => ctx.p.w_chain,
        _ => 0.0,
    }
}

/// Incidents whose effect rolls no dice, played once.
const SURE_INCIDENTS: [u32; 4] = [213, 231, 232, SEWER_RATWAY];

/// The seed of sample `k` of a copy landed on cell `n`: fixed, so a copy
/// never reads the game's own dice.
fn sample_seed(st: &State, n: usize, k: u32) -> u32 {
    st.day.wrapping_mul(0x9e37_79b9) ^ (n as u32).wrapping_mul(0x85eb_ca6b) ^ (k + 1).wrapping_mul(0xc2b2_ae35)
}

/// A copy of `st` with the warden landed on cell `n`, its marker's incident
/// pending. A Sewer Ratway choice in progress is this landing.
fn landed(st: &State, n: usize, seed: u32) -> State {
    let mut trial = st.clone();
    trial.rng = seed;
    trial.pending.clear();
    land(&mut trial, n);
    trial
}

/// The mean board gain, by tiers `(v, r)`, of `action` on a copy landed on
/// cell `n` over `samples` samples: a marker's incident (`Action::Event`) or
/// the Interrogation Room's Bestow. Instigate's marker adds no board value.
fn landing_gain(st: &State, n: usize, action: &Action, samples: u32, &(v, r): &([f64; 6], f64)) -> f64 {
    let mut gain = 0.0;
    for k in 0..samples {
        let mut trial = landed(st, n, sample_seed(st, n, k));
        let before = board_value(&trial, &v, r);
        if apply(&mut trial, action).is_ok() {
            gain += board_value(&trial, &v, r) - before;
        }
    }
    gain / f64::from(samples)
}

/// The `why` of incidentScore: what waits on the marker, if there is one.
pub fn incident_why(c: &Cell) -> Option<String> {
    let id = c.event?;
    Some(match id {
        _ if c.event_known == Some(false) => "a Prison Incident waits there".to_string(),
        EXECUTION_EFFICIENCY => "an Execution Efficiency waits there".to_string(),
        CHAIN_REACTION => "a Chain Reaction waits there".to_string(),
        _ => format!("{} waits there", an(incident_name(id))),
    })
}

/// The `why` patrolScores gives landing on cell `n`.
pub fn patrol_why(st: &State, n: usize) -> String {
    let c = &st.cells[n];
    if let Some(why) = incident_why(c) {
        return why;
    }
    if st.chain {
        let sealed = clockwise_path(st, n).iter().filter(|&&k| st.cells[k].sealed).count();
        format!("the chain opens {} on the way", plural(sealed, "door"))
    } else if let (true, Some(nest)) = (c.sealed, c.nest) {
        format!("it destroys the Rat Nest due on day {}", nest.due)
    } else if c.sealed {
        "a new Inmate, who may consume today".to_string()
    } else if let Some(i) = c.inmate {
        format!("its {} can act today", describe(&i))
    } else if n == data().room {
        "Instigate or Bestow waits there".to_string()
    } else {
        "an empty cell".to_string()
    }
}

/// patrolScores: scores landing on each of `cells`, as (cell, score), best first.
pub fn patrol_scores(ctx: &mut Ctx, cells: &[usize]) -> Vec<(usize, f64)> {
    let st = ctx.st;
    let p = ctx.p;
    let (mut board, crowd) = composition(st, None);
    let swap_left = st.used.swap < st.params.swaps_per_day;
    // The door in cell c, by whether its opening sets off the "after opening"
    // instruments; an unseen door is valued once per kind of opening.
    let mut fresh: [Option<f64>; 2] = [None; 2];
    // The scratch bite each cell's opening gives up.
    let loss: Vec<f64> = st.cells.iter().map(|c| scratch_loss(ctx, c)).collect();
    let mut door_now = |ctx: &mut Ctx, board: &mut Comp, c: &Cell, after: bool| {
        let seen = seen_door(c);
        if let (false, Some(value)) = (seen, fresh[usize::from(after)]) {
            return value;
        }
        let (dist, beast) = door_dist(st, Some(c), after);
        let value = fresh_value(ctx, &dist, beast, board, crowd + 1, swap_left) - p.w_open * loss[c.n];
        if !seen {
            fresh[usize::from(after)] = Some(value);
        }
        value
    };
    let known = |ctx: &mut Ctx, cell: &Cell| {
        let (mut comp, crowd) = composition(st, Some(cell.n));
        let i = cell.inmate.expect("inmate");
        activation(ctx, i.tier as usize, i.beast.index(), &mut comp, crowd, swap_left)
    };
    let held = HELD_TERMS.iter().any(|&t| st.has(t)).then(|| Held::of(ctx, &loss));
    let tiers = gift_tiers(ctx);
    let mut out = Vec::with_capacity(cells.len());
    for &n in cells {
        let c = &st.cells[n];
        let steps = steps_between(st.position, n);
        let mut score = p.w_step * steps as f64;
        // Landing on a marker fires its incident, which can't be declined.
        score += incident_score(ctx, c, &board);
        // Any other known marker, and the Interrogation Room, are played on a copy.
        if let Some(id) = c.event.filter(|&id| c.event_known != Some(false) && id != EXECUTION_EFFICIENCY && id != CHAIN_REACTION) {
            let samples = if SURE_INCIDENTS.contains(&id) { 1 } else { 3 };
            score += p.w_event * landing_gain(st, n, &Action::Event, samples, &tiers);
        } else if n == data().room {
            let bestow = Action::Choose { id: ChoiceId::Name(Card::Bestow.key().to_string()) };
            score += p.w_event * landing_gain(st, n, &bestow, 3, &tiers);
        }
        // The sealed doors left after this landing's own openings.
        let (mut left, mut left_worth) = held.as_ref().map_or((0, 0.0), |h| (h.sealed, h.side));
        if held.is_some() && c.sealed && !st.chain {
            left -= 1;
            left_worth -= side_worth(ctx, c, &loss);
        }
        if st.chain {
            // Chain Reaction opens every door on the walk from the side.
            let mut opened = 0.0;
            for &k in clockwise_path(st, n).iter().filter(|&&k| st.cells[k].sealed) {
                let d = &st.cells[k];
                if held.is_some() {
                    left -= 1;
                    left_worth -= side_worth(ctx, d, &loss);
                }
                if k == n {
                    continue;
                }
                opened += side_worth(ctx, d, &loss);
            }
            let here = if c.sealed {
                door_now(ctx, &mut board, c, false)
            } else if c.inmate.is_some() {
                p.w_walk * known(ctx, c)
            } else {
                0.0
            };
            score += p.w_open * opened + here;
        } else if c.sealed {
            score += door_now(ctx, &mut board, c, true);
            // Squirming Iron Box: the patrol opens the door's own inmate and
            // destroys the nest, whose Rare Rat hatches if it is due in the run.
            if c.nest.is_some_and(|nest| nest.due <= st.max_days) {
                score -= ctx.v[st.params.nest_tier as usize];
            }
        } else if c.inmate.is_some() {
            score += p.w_walk * known(ctx, c);
        }
        // Iron Tongs of Lust: a Replace left for the Tusk cell's inmate.
        if swap_left && c.ivory && c.on_board() {
            score += p.w_walk * tusk_gain(ctx);
        }
        if let Some(h) = &held {
            // A dice patrol onto a door sets off the "after opening" instruments.
            let opened = (c.sealed && !st.chain)
                .then(|| door_dist(st, Some(c), true).0);
            score += held_terms(ctx, h, n, steps, opened.as_deref(), left, left_worth);
        }
        out.push((n, score));
    }
    best_first(&mut out);
    out
}

/// The instruments `held_terms` prices on a patrol landing.
const HELD_TERMS: [Tool; 6] = [Tool::Keys, Tool::Forgedkey, Tool::Fire, Tool::Blacklist, Tool::Shortchain, Tool::Longchain];

/// The board before a patrol, as the held instruments see it: the sealed
/// doors and their worth opened from the side, the open inmates' tiers, and
/// the scratch bite each cell's opening gives up.
struct Held {
    sealed: usize,
    side: f64,
    board: Vec<usize>,
    loss: Vec<f64>,
}

impl Held {
    fn of(ctx: &Ctx, loss: &[f64]) -> Held {
        let st = ctx.st;
        let doors = st.cells.iter().filter(|c| c.sealed);
        Held {
            sealed: doors.clone().count(),
            side: doors.map(|c| side_worth(ctx, c, loss)).sum(),
            board: st.cells.iter().filter(|c| c.on_board()).filter_map(|c| c.inmate).map(|i| i.tier as usize).collect(),
            loss: loss.to_vec(),
        }
    }
}

/// The worth of sealed cell `c` opened from the side (no "after opening"
/// instrument), less the scratch bite its opening gives up (`loss` by cell).
fn side_worth(ctx: &Ctx, c: &Cell, loss: &[f64]) -> f64 {
    let worth = if seen_door(c) { dist_worth(&ctx.v, ctx.r, &door_dist(ctx.st, Some(c), false).0) } else { ctx.side_value };
    worth - loss[c.n]
}

/// The scratch bite given up by opening sealed cell `c`. With an open Wolf of
/// the door's tier: half the most it gains biting the door over its best
/// other meal (it may not get its turn first). Without one: the share of the
/// open inmates that are lower Wolves, times 3, times the scratch rise.
fn scratch_loss(ctx: &mut Ctx, c: &Cell) -> f64 {
    let Some(h) = c.hidden.filter(|h| c.sealed && c.scratch && h.tier < RAINBOW) else { return 0.0 };
    let st = ctx.st;
    let (t, wolf) = (h.tier as usize, Beast::Wolf.index());
    let open = || st.cells.iter().filter(|w| w.on_board()).filter_map(|w| w.inmate.map(|i| (w.n, i)));
    let wolves: Vec<usize> = open().filter(|(_, i)| i.beast == Beast::Wolf && i.tier == h.tier).map(|(n, _)| n).collect();
    if wolves.is_empty() {
        let lower = open().filter(|(_, i)| i.beast == Beast::Wolf && i.tier < h.tier).count();
        let share = lower as f64 / open().count().max(1) as f64;
        return 3.0 * share * rise_worth(ctx, t, 1 + ctx.scratch_extra as i32);
    }
    let mut loss = 0.0f64;
    for n in wolves {
        let (mut comp, crowd) = composition(st, Some(n));
        let bite = devour_delta(ctx, t, wolf, (t, wolf), true, &mut comp, crowd);
        let other = best_meal(ctx, t, wolf, &mut comp, crowd).0.max(0.0);
        loss = loss.max(0.5 * (bite - other));
    }
    loss
}

/// What `by` tiers more are worth to an inmate of tier `t`; a Chromatic is
/// worth `r` and nothing rises past the top tier.
fn rise_worth(ctx: &Ctx, t: usize, by: i32) -> f64 {
    worth(ctx, (t as i32 + by).clamp(0, ctx.st.top_tier()) as usize) - worth(ctx, t)
}

/// What the held opening and movement instruments add to a dice landing on
/// cell `n`, `steps` away (engine after_opening and shackles). `opened` is the
/// tier odds of the inmate a patrol opening brings out, None when the landing
/// opens no door on the patrol; `left` doors stay sealed after the landing's
/// own openings, worth `left_worth` in all from the side.
fn held_terms(ctx: &Ctx, h: &Held, n: usize, steps: usize, opened: Option<&[f64]>, left: usize, left_worth: f64) -> f64 {
    let st = ctx.st;
    let d = data();
    let w_open = ctx.p.w_open;
    let chance = |t: Tool| if st.has(t) { d.tool(t).chance / 100.0 } else { 0.0 };
    // A random sealed door left, opened from the side.
    let random_door = if left > 0 { left_worth / left as f64 } else { 0.0 };
    // The open inmates after the landing, the opened one included, and the
    // sum of what `by` tiers more are worth to them.
    let board = h.board.len() + usize::from(opened.is_some());
    let rises = |by: i32| {
        let mut sum: f64 = h.board.iter().map(|&t| rise_worth(ctx, t, by)).sum();
        if let Some(dist) = opened {
            sum += dist.iter().enumerate().map(|(t, p)| p * rise_worth(ctx, t, by)).sum::<f64>();
        }
        sum
    };
    let mut gain = 0.0;
    if let Some(dist) = opened {
        // Executioner's Blacklist: with few doors left after this one (counted
        // before the Iron, Keys and Forged Key open theirs), random inmates rise.
        let black = d.tool(Tool::Blacklist);
        if st.has(Tool::Blacklist) && left <= black.max_sealed.unwrap_or(0) {
            let picks = black.count.unwrap_or(0).min(board);
            gain += picks as f64 / board as f64 * rises(black.tiers.unwrap_or(0));
        }
        // Searing Branding Iron: a copy of the opened inmate opens another door.
        if left > 0 {
            gain += chance(Tool::Fire) * w_open * dist_worth(&ctx.v, ctx.r, dist);
        }
        // Lost Ring of Keys: the doors on both sides open from the side.
        if st.has(Tool::Keys) {
            let cells = d.cell_count;
            let sides: f64 = [n + cells - 1, n + 1]
                .into_iter()
                .map(|k| &st.cells[k % cells])
                .filter(|c| c.sealed && c.n != d.room)
                .map(|c| side_worth(ctx, c, &h.loss))
                .sum();
            gain += chance(Tool::Keys) * w_open * sides;
        }
        // Forged Key: more random doors open from the side.
        let forged = d.tool(Tool::Forgedkey).count.unwrap_or(0).min(left);
        gain += chance(Tool::Forgedkey) * w_open * forged as f64 * random_door;
    }
    // Short-Chained Shackles: a short move opens random doors.
    let short = d.tool(Tool::Shortchain);
    if steps > 0 && steps <= short.max_steps.unwrap_or(0) {
        gain += chance(Tool::Shortchain) * w_open * short.count.unwrap_or(0).min(left) as f64 * random_door;
    }
    // Long-Chained Shackles: a long move raises a random open inmate.
    let long = d.tool(Tool::Longchain);
    if steps >= long.min_steps.unwrap_or(usize::MAX) && board > 0 {
        gain += chance(Tool::Longchain) * rises(long.tiers.unwrap_or(0)) / board as f64;
    }
    gain
}

/// linkValue: what a prey mark still promises; both inmates rise a tier each morning.
pub fn link_value(ctx: &Ctx, lion: u32, prey: u32, rounds: i32) -> f64 {
    let mut value = 0.0;
    for id in [lion, prey] {
        let Some(i) = ctx.st.cell_of(id).and_then(|n| ctx.st.cells[n].inmate) else { continue };
        let t = i.tier as usize;
        let to = TOP.min((i.tier + rounds) as usize);
        value += worth(ctx, to) - ctx.v[t];
    }
    value
}

/// linkLoss: the prey mark an action would end, valued (0 if none).
pub fn link_loss(ctx: &Ctx, inmate: Option<Inmate>, certain: bool) -> f64 {
    let Some(inmate) = inmate else { return 0.0 };
    let Some(k) = ctx.st.link_of(inmate.id) else { return 0.0 };
    let link = ctx.st.prey[k];
    // A Lion that devours re-rolls its beast; 1 in 4 times it stays a Lion.
    link_value(ctx, link.lion, link.prey, link.rounds) * if certain || link.prey == inmate.id { 1.0 } else { 0.75 }
}

/// coinLoss: the worth of Kin's Coin of Fate lost when `gone` leave the board;
/// `board` holds every open inmate below Rainbow.
fn coin_loss(ctx: &Ctx, board: &Comp, gone: &[Inmate]) -> f64 {
    if !ctx.st.has(Tool::Coin) {
        return 0.0;
    }
    let mut after = *board;
    for i in gone.iter().filter(|i| i.tier < RAINBOW) {
        after[i.tier as usize * 4 + i.beast.index()] -= 1;
    }
    coin_of_fate(ctx, board) - coin_of_fate(ctx, &after)
}

/// The open inmates' tiers by cell, as Ravenous Chains reads them, -1 where
/// none stands; empty without the Chains or a morning left.
fn chains_ring(st: &State) -> Vec<i8> {
    if !st.has(Tool::Chains) || st.max_days <= st.day {
        return Vec::new();
    }
    st.cells.iter().map(|c| c.inmate.filter(|_| !c.sealed).map_or(-1, |i| i.tier as i8)).collect()
}

/// chainsLoss: what Ravenous Chains takes on the next morning from the board
/// `ring` (`chains_ring`, 0 when empty): over the tied top inmates, the worth
/// of the open ring neighbours it removes less the expected worth of the
/// centre's rise, one roll per removal.
fn chains_loss(ctx: &Ctx, ring: &[i8]) -> f64 {
    let Some(&top) = ring.iter().max().filter(|&&t| t >= 0) else { return 0.0 };
    let def = data().tool(Tool::Chains);
    let (p, by) = (def.chance / 100.0, def.tiers.unwrap_or(0));
    let n = ring.len();
    let (mut tops, mut loss) = (0, 0.0);
    for c in (0..n).filter(|&c| ring[c] == top) {
        tops += 1;
        let sides = [ring[(c + n - 1) % n], ring[(c + 1) % n]];
        let removed = sides.iter().filter(|&&t| t >= 0).count();
        loss += sides.iter().filter(|&&t| t >= 0).map(|&t| worth(ctx, t as usize)).sum::<f64>();
        // At most two removals: j of them land in [1, removed, 1] ways.
        for j in 0..=removed {
            let odds = [1.0, removed as f64, 1.0][j] * p.powi(j as i32) * (1.0 - p).powi((removed - j) as i32);
            loss -= odds * rise_worth(ctx, top as usize, j as i32 * by);
        }
    }
    loss / f64::from(tops)
}

/// The Tusk's gain on a Replace in a marked cell: random open inmates below
/// Rainbow rise a tier each.
fn tusk_gain(ctx: &Ctx) -> f64 {
    let st = ctx.st;
    let board: Vec<usize> =
        st.cells.iter().filter_map(|c| c.inmate.filter(|i| !c.sealed && i.tier < RAINBOW)).map(|i| i.tier as usize).collect();
    let lift = board.iter().fold(0.0, |sum, &t| sum + ctx.v[t + 1] - ctx.v[t]) / board.len().max(1) as f64;
    (st.params.ivory_count as usize).min(board.len()) as f64 * lift
}

/// huntGain: what a Hunt now adds, the value of the prey marks it would place.
pub fn hunt_gain(ctx: &Ctx) -> f64 {
    let st = ctx.st;
    let targets = hunt_targets(st);
    let Some(lion) = st.cells[st.position].inmate.filter(|_| !targets.is_empty()) else { return 0.0 };
    targets.iter().fold(0.0, |sum, &n| {
        sum + link_value(ctx, lion.id, st.cells[n].inmate.expect("prey").id, st.params.prey_rounds as i32)
    })
}

/// devourScores: the given first or chain bites for the warden's inmate, as
/// (cell, score), best first.
pub fn devour_scores(ctx: &mut Ctx, options: Vec<Bite>) -> Vec<(usize, f64)> {
    let st = ctx.st;
    let here = &st.cells[st.position];
    let Some(eater) = here.inmate.filter(|_| !here.sealed) else { return Vec::new() };
    let (mut comp, crowd) = composition(st, Some(st.position));
    let board = composition(st, None).0;
    let tier = eater.tier as usize;
    let beast = eater.beast.index();
    let own = link_loss(ctx, Some(eater), false);
    let mut ring = chains_ring(st);
    let chains_before = chains_loss(ctx, &ring);
    let block = if ring.is_empty() { 0 } else { st.board_cells().len() };
    let mut out: Vec<(usize, f64)> = options
        .into_iter()
        .map(|o| {
            let target = &st.cells[o.cell];
            let meal = if o.scratch { target.hidden } else { target.inmate }.expect("meal");
            let meal_link = if o.scratch { 0.0 } else { link_loss(ctx, Some(meal), true) };
            let same_link = own != 0.0 && meal_link != 0.0 && st.link_of(eater.id) == st.link_of(meal.id);
            let mut loss = if same_link { own.max(meal_link) } else { own + meal_link };
            // The eater leaves its tier and the meal the board (the chain after is not priced).
            let gone = [eater, meal];
            loss += coin_loss(ctx, &board, &gone[..if o.scratch { 1 } else { 2 }]);
            // Ravenous Chains: the meal's cell empties and the eater lands
            // its sure gain up (a Chromatic leaves for the vault).
            if !ring.is_empty() {
                let (e, m) = (ring[st.position], ring[o.cell]);
                let landing = (eater.tier + sure_gain(st, &eater, &meal, o.scratch, block)).min(st.top_tier());
                ring[o.cell] = -1;
                ring[st.position] = if landing >= RAINBOW { -1 } else { landing as i8 };
                loss += chains_loss(ctx, &ring) - chains_before;
                ring[o.cell] = m;
                ring[st.position] = e;
            }
            let stair_loss = if o.scratch { 0.0 } else { stair_delta(ctx, &mut comp, meal.tier as usize, meal.beast.index(), -1) };
            let delta = devour_delta(ctx, tier, beast, (meal.tier as usize, meal.beast.index()), o.scratch, &mut comp, crowd);
            (o.cell, delta - loss + stair_loss)
        })
        .collect();
    best_first(&mut out);
    out
}

/// swapGainHere: a swap in the warden's cell, the new inmate's worth over the
/// old one less any prey mark it ends, plus the Tusk bonus if the cell has one.
pub fn swap_gain_here(ctx: &Ctx) -> f64 {
    let st = ctx.st;
    let here = &st.cells[st.position];
    let Some(inmate) = here.inmate.filter(|_| !here.sealed && st.used.swap < st.params.swaps_per_day) else {
        return f64::NEG_INFINITY;
    };
    let (comp, crowd) = composition(st, Some(st.position));
    let mut gain = swap_value(ctx, inmate.tier as usize, &comp, crowd - 1)
        - link_loss(ctx, Some(inmate), true)
        - coin_loss(ctx, &composition(st, None).0, &[inmate]);
    // Gaoler's Execution Axe: a swap that raises the rarity, counting the
    // Stake's and the Maiden's rises, opens random sealed cells.
    if st.has(Tool::Axe) {
        let doors = data().tool(Tool::Axe).count.unwrap_or(0).min(st.sealed_cells().len());
        let rise: f64 = replace_odds(ctx, inmate.tier as usize, &comp, crowd - 1).0.iter().skip(inmate.tier as usize + 1).sum();
        gain += rise * doors as f64 * ctx.p.w_open * ctx.open_value;
    }
    // Ravenous Chains: the new inmate's tier follows the Replace odds.
    let mut ring = chains_ring(st);
    if !ring.is_empty() {
        let before = chains_loss(ctx, &ring);
        let mut after = 0.0;
        for (t, &p) in replace_odds(ctx, inmate.tier as usize, &comp, crowd - 1).0.iter().enumerate().filter(|&(_, &p)| p > 0.0) {
            ring[st.position] = if t >= TOP { -1 } else { t as i8 };
            after += p * chains_loss(ctx, &ring);
        }
        gain -= after - before;
    }
    if here.ivory {
        return gain + tusk_gain(ctx);
    }
    if ctx.last_day || f64::from(inmate.tier) <= ctx.p.swap_max_tier { gain } else { f64::NEG_INFINITY }
}

/// boardValue: tier values `v` plus vaulted Rainbows worth `r` each, with a
/// bonus for inmates that have something to eat.
fn board_value(st: &State, v: &[f64; 6], r: f64) -> f64 {
    let mut value = st.vault.len() as f64 * r;
    for c in &st.cells {
        let Some(i) = c.inmate.filter(|_| !c.sealed) else { continue };
        value += v[i.tier as usize] * if devour_options(st, c.n).is_empty() { 1.0 } else { 1.25 };
    }
    value
}

/// The tier values and Rainbow worth a gift pick weighs the board with. Two
/// inmates of a tier make one of the next, so early on an inmate counts as the
/// inmates it took to make: 2^t. With fewer than 4 days left the tiers still
/// to climb weigh more than the inmates spent, 3^t. The last day keeps the
/// strategy's own values, where only a Chromatic counts.
fn gift_tiers(ctx: &Ctx) -> ([f64; 6], f64) {
    let st = ctx.st;
    if ctx.last_day {
        return (ctx.v, ctx.r);
    }
    let base: f64 = if st.max_days - st.day < 4 { 3.0 } else { 2.0 };
    let v = [0, 1, 2, 3, 4, 5].map(|t| base.powi(t));
    // A full vault stops inmates at Ultimate, as in `context`.
    (v, if st.top_tier() < RAINBOW { v[TOP - 1] } else { v[TOP] * ctx.p.rainbow_mult })
}

/// bestByBoard: the option whose result has the highest board value; random
/// ones are sampled three times. Gifts (`gift`) weigh tiers by `gift_tiers`;
/// the Interrogation Room's cards by the strategy's tier values.
pub fn best_by_board<'k>(ctx: &Ctx, options: &[&'k str], gift: bool, roll: &mut Rng32) -> &'k str {
    let mut best: Option<(&str, f64)> = None;
    for &id in options {
        let mut value = 0.0;
        let samples = if ["ring", "horn", "instigate", "bestow"].contains(&id) { 3 } else { 1 };
        for _ in 0..samples {
            let mut trial = ctx.st.clone();
            trial.rng = (roll.roll() * 4294967296.0) as u32;
            if !apply(&mut trial, &Action::Choose { id: ChoiceId::Name(id.to_string()) }).is_ok() {
                continue;
            }
            let after = context(&trial, ctx.p);
            let (v, r) = if gift { gift_tiers(&after) } else { (after.v, after.r) };
            value += board_value(&trial, &v, r);
        }
        value /= f64::from(samples);
        if best.is_none_or(|(_, v)| value > v) {
            best = Some((id, value));
        }
    }
    best.map_or(options[0], |(id, _)| id)
}
