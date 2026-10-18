//! Chain math (js/solver.js 113-251): the exact expected outcome of a devour,
//! including the random chain after it.

use super::{Comp, Ctx, TOP, composition, eats_table, normalize};
use crate::engine::data::{Beast, Tool, data};
use crate::engine::state::State;
use std::collections::HashMap;

fn tiers(t: Tool) -> usize {
    data().tool(t).tiers.unwrap_or(0) as usize
}

/// swapDist: tier odds for a swapped-in inmate, after Hangman's Noose.
pub fn swap_dist(ctx: &Ctx) -> Vec<f64> {
    let mut w: Vec<f64> = ctx.st.params.tier_weights.iter().map(|&w| f64::from(w)).collect();
    if ctx.st.has(Tool::Rope) {
        w[0] = 0.0;
        w[1] = 0.0;
    }
    normalize(&w)
}

/// rise: value from here on for an eater that just reached tier t; its beast
/// re-rolls, then it chains into a random valid meal if it has one.
pub fn rise(ctx: &mut Ctx, t: usize, comp: &mut Comp, crowd: i64) -> f64 {
    if t >= TOP {
        return ctx.r;
    }
    let mut sum = 0.0;
    for b in 0..4 {
        if ctx.revenge {
            // Avenger's Casket: 30% of the time a meal for the new form appears.
            let foods = ctx.eats[b].clone();
            let mut spawned = 0.0;
            for &f in &foods {
                comp[t * 4 + f] += 1;
                spawned += ctx.v[t] + feed(ctx, t, b, comp, crowd + 1);
                comp[t * 4 + f] -= 1;
            }
            sum += (0.3 * spawned) / foods.len() as f64 + 0.7 * feed(ctx, t, b, comp, crowd);
        } else {
            sum += feed(ctx, t, b, comp, crowd);
        }
    }
    sum / 4.0
}

/// The memo key of feed: tier, beast, the Muzzle crowd bucket and the counts
/// from tier t up, as the JS string key tells positions apart.
fn feed_key(ctx: &Ctx, t: usize, b: usize, comp: &Comp, crowd: i64) -> (u32, u128) {
    let mut head = t as u32 | (b as u32) << 3;
    if ctx.muzzle {
        let bucket = crowd.min(data().tool(Tool::Muzzle).max_inmates.unwrap_or(0) as i64 + 1);
        head |= 1 << 5 | ((bucket + 1024) as u32) << 6;
    }
    let mut counts = 0u128;
    for (i, &k) in comp.iter().enumerate().take(20).skip(t * 4) {
        counts |= ((k + 2) as u128 & 63) << (6 * i);
    }
    (head, counts)
}

/// feed: an eater of beast b at tier t takes a random valid meal; with none,
/// the chain ends and it keeps its tier.
pub fn feed(ctx: &mut Ctx, t: usize, b: usize, comp: &mut Comp, crowd: i64) -> f64 {
    let key = feed_key(ctx, t, b, comp, crowd);
    if let Some(&hit) = ctx.memo.get(&key) {
        return hit;
    }
    let foods = ctx.eats[b].clone();
    let total: i32 = foods.iter().map(|&f| comp[t * 4 + f]).sum();
    let mut out = ctx.v[t];
    if total != 0 {
        let mut sum = 0.0;
        for &f in &foods {
            let k = comp[t * 4 + f];
            if k == 0 {
                continue;
            }
            comp[t * 4 + f] -= 1;
            sum += f64::from(k) * bite(ctx, t, b, comp, crowd, 0);
            comp[t * 4 + f] += 1;
        }
        out = sum / f64::from(total);
    }
    ctx.memo.insert(key, out);
    out
}

/// bite: one bite at tier t; the meal is gone and the eater rises.
pub fn bite(ctx: &mut Ctx, t: usize, b: usize, comp: &mut Comp, crowd: i64, extra: usize) -> f64 {
    let mut gain = 1 + extra;
    if ctx.muzzle && crowd <= data().tool(Tool::Muzzle).max_inmates.unwrap_or(0) as i64 {
        gain += tiers(Tool::Muzzle);
    }
    if ctx.skewer && b == Beast::Rat.index() {
        gain += tiers(Tool::Skewer);
    }
    if ctx.crown && b == Beast::Lion.index() {
        gain += tiers(Tool::Crown);
    }
    let mut after = |ctx: &mut Ctx, g: usize| rise(ctx, TOP.min(t + g), comp, crowd - 1);
    let rest = if ctx.saw { 0.75 * after(ctx, gain) + 0.25 * after(ctx, gain + 1) } else { after(ctx, gain) };
    -ctx.v[t] + rest
}

/// devourDelta: expected change in value from a devour, the chosen first meal
/// then the random chain. `comp` leaves out the eater and still holds the meal.
pub fn devour_delta(ctx: &mut Ctx, tier: usize, beast: usize, meal: usize, scratch: bool, comp: &mut Comp, crowd: i64) -> f64 {
    let i = tier * 4 + meal;
    if !scratch {
        comp[i] -= 1;
    }
    let extra = if scratch { ctx.scratch_extra } else { 0 };
    let value = bite(ctx, tier, beast, comp, crowd, extra) - ctx.v[tier];
    if !scratch {
        comp[i] += 1;
    }
    value
}

/// bestMeal: the best first meal for an eater of this tier and beast, as
/// (value, meal); (-inf, None) when it has none.
pub fn best_meal(ctx: &mut Ctx, tier: usize, beast: usize, comp: &mut Comp, crowd: i64) -> (f64, Option<usize>) {
    let mut best = (f64::NEG_INFINITY, None);
    for f in ctx.eats[beast].clone() {
        if comp[tier * 4 + f] == 0 {
            continue;
        }
        let value = devour_delta(ctx, tier, beast, f, false, comp, crowd);
        if value > best.0 {
            best = (value, Some(f));
        }
    }
    best
}

/// chainOdds: exact odds that a devour from cell `from` into cell `to` chains
/// up to Rainbow, and its expected bites, by the rules of `rise` without tools.
pub fn chain_odds(st: &State, from: usize, to: usize) -> (f64, f64) {
    let target = &st.cells[to];
    let meal = if target.sealed { target.hidden } else { target.inmate };
    let (Some(eater), Some(meal)) = (st.cells[from].inmate, meal) else { return (0.0, 0.0) };
    let (mut comp, _) = composition(st, Some(from));
    if !target.sealed {
        comp[meal.tier as usize * 4 + meal.beast.index()] -= 1;
    }
    let mut odds = Odds { eats: eats_table(st), comp, memo: HashMap::new() };
    let gain = if target.sealed && target.scratch { 2 + usize::from(st.params.scratch_stacks_base) } else { 1 };
    let (p, n) = odds.at_tier(TOP.min(eater.tier as usize + gain));
    (p, 1.0 + n)
}

struct Odds {
    eats: [Vec<usize>; 4],
    comp: Comp,
    memo: HashMap<(usize, usize, Vec<i32>), (f64, f64)>,
}

impl Odds {
    /// (P(reach Rainbow), expected further bites) from tier t, beast re-rolling.
    fn at_tier(&mut self, t: usize) -> (f64, f64) {
        if t >= TOP {
            return (1.0, 0.0);
        }
        let (mut p, mut n) = (0.0, 0.0);
        for b in 0..4 {
            let (pb, nb) = self.eat(t, b);
            p += pb / 4.0;
            n += nb / 4.0;
        }
        (p, n)
    }

    fn eat(&mut self, t: usize, b: usize) -> (f64, f64) {
        let key = (t, b, self.comp[t * 4..20].to_vec());
        if let Some(&hit) = self.memo.get(&key) {
            return hit;
        }
        let foods = self.eats[b].clone();
        let total: i32 = foods.iter().map(|&f| self.comp[t * 4 + f]).sum();
        let mut out = (0.0, 0.0);
        if total != 0 {
            let (mut p, mut n) = (0.0, 0.0);
            for &f in &foods {
                let k = self.comp[t * 4 + f];
                if k == 0 {
                    continue;
                }
                self.comp[t * 4 + f] -= 1;
                let (pf, nf) = self.at_tier(t + 1);
                self.comp[t * 4 + f] += 1;
                p += (f64::from(k) * pf) / f64::from(total);
                n += (f64::from(k) * (nf + 1.0)) / f64::from(total);
            }
            out = (p, n);
        }
        self.memo.insert(key, out);
        out
    }
}
