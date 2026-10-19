//! Valuing moves (js/solver.js 336-497): incident markers, patrol
//! destinations, prey marks, devours, swaps and the gift picks.

use super::chain::{coin_of_fate, devour_delta, replace_odds};
use super::values::{activation, execution_value, fresh_value, stair_delta, swap_value};
use super::{Comp, Ctx, Rng32, TOP, composition, context};
use crate::engine::data::{RAINBOW, Tool, data};
use crate::engine::devour::{Bite, devour_options, hunt_targets};
use crate::engine::incidents::{CHAIN_REACTION, EXECUTION_EFFICIENCY};
use crate::engine::rules::{clockwise_path, steps_between};
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
    // Behind a door the executioner is whoever comes out.
    let execution = || {
        if !c.sealed {
            return c.inmate.map_or(0.0, |i| execution_value(ctx, i.tier as usize, i.beast.index(), comp));
        }
        let mut exec = 0.0;
        for t in 0..ctx.open_dist.len() {
            for b in 0..4 {
                exec += (ctx.open_dist[t] / 4.0) * execution_value(ctx, t, b, comp);
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
    let mut fresh: Option<f64> = None;
    let mut fresh_now = |ctx: &mut Ctx, board: &mut Comp| {
        *fresh.get_or_insert_with(|| {
            let dist = ctx.open_dist.clone();
            fresh_value(ctx, &dist, board, crowd + 1, swap_left)
        })
    };
    let known = |ctx: &mut Ctx, cell: &Cell| {
        let (mut comp, crowd) = composition(st, Some(cell.n));
        let i = cell.inmate.expect("inmate");
        activation(ctx, i.tier as usize, i.beast.index(), &mut comp, crowd, swap_left)
    };
    let mut out = Vec::with_capacity(cells.len());
    for &n in cells {
        let c = &st.cells[n];
        let steps = steps_between(st.position, n);
        let mut score = p.w_step * steps as f64;
        // Landing on a marker fires its incident, which can't be declined.
        score += incident_score(ctx, c, &board);
        if st.chain {
            let sealed = clockwise_path(st, n).iter().filter(|&&k| st.cells[k].sealed).count() as i64;
            let here = if c.sealed {
                fresh_now(ctx, &mut board)
            } else if c.inmate.is_some() {
                p.w_walk * known(ctx, c)
            } else {
                0.0
            };
            score += p.w_open * (sealed - i64::from(c.sealed)) as f64 * ctx.open_value + here;
        } else if c.sealed && c.nest.is_some() {
            score += p.w_nest;
        } else if c.sealed {
            score += fresh_now(ctx, &mut board);
        } else if c.inmate.is_some() {
            score += p.w_walk * known(ctx, c);
        }
        out.push((n, score));
    }
    best_first(&mut out);
    out
}

/// linkValue: what a prey mark still promises; both inmates rise a tier each morning.
pub fn link_value(ctx: &Ctx, lion: u32, prey: u32, rounds: i32) -> f64 {
    let mut value = 0.0;
    for id in [lion, prey] {
        let Some(i) = ctx.st.cell_of(id).and_then(|n| ctx.st.cells[n].inmate) else { continue };
        let t = i.tier as usize;
        let to = TOP.min((i.tier + rounds) as usize);
        value += (if to >= TOP { ctx.r } else { ctx.v[to] }) - ctx.v[t];
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
    if here.ivory {
        let board: Vec<usize> =
            st.cells.iter().filter_map(|c| c.inmate.filter(|i| !c.sealed && i.tier < RAINBOW)).map(|i| i.tier as usize).collect();
        let lift = board.iter().fold(0.0, |sum, &t| sum + ctx.v[t + 1] - ctx.v[t]) / board.len().max(1) as f64;
        gain += (st.params.ivory_count as usize).min(board.len()) as f64 * lift;
        return gain;
    }
    if f64::from(inmate.tier) <= ctx.p.swap_max_tier { gain } else { f64::NEG_INFINITY }
}

/// boardValue: tier values plus vaulted Rainbows, with a bonus for inmates
/// that have something to eat.
pub fn board_value(ctx: &Ctx) -> f64 {
    let st = ctx.st;
    let mut value = st.vault.len() as f64 * ctx.r;
    for c in &st.cells {
        let Some(i) = c.inmate.filter(|_| !c.sealed) else { continue };
        value += ctx.v[i.tier as usize] * if devour_options(st, c.n).is_empty() { 1.0 } else { 1.25 };
    }
    value
}

/// bestByBoard: the option whose result has the highest board value; random
/// ones are sampled three times. Gifts and the Interrogation Room's cards use it.
pub fn best_by_board<'k>(ctx: &Ctx, options: &[&'k str], roll: &mut Rng32) -> &'k str {
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
            value += board_value(&context(&trial, ctx.p));
        }
        value /= f64::from(samples);
        if best.is_none_or(|(_, v)| value > v) {
            best = Some((id, value));
        }
    }
    best.map_or(options[0], |(id, _)| id)
}
