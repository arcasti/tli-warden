//! Valuing positions (js/solver.js 255-334): chain readiness, swaps, what an
//! inmate can do once the warden stands with it, and Executions.

use super::chain::{best_meal, fed, replace_odds};
use super::{Comp, Ctx, TOP};
use crate::engine::data::data;

/// stair: how ready the block is for chains; at each tier, the share of beasts
/// an eater could re-roll into and still find a meal there, weighted by tier.
pub fn stair(ctx: &Ctx, comp: &Comp) -> f64 {
    let mut value = 0.0;
    for t in 0..TOP {
        let fed = (0..4).filter(|&b| fed(ctx, t, b, comp)).count();
        value += (fed as f64 / 4.0) * ctx.v[t];
    }
    value
}

/// stairDelta: change in readiness from adding (+1) or removing (-1) one inmate.
pub fn stair_delta(ctx: &Ctx, comp: &mut Comp, tier: usize, beast: usize, sign: i32) -> f64 {
    if ctx.p.w_stair == 0.0 || tier >= TOP {
        return 0.0;
    }
    let before = stair(ctx, comp);
    comp[tier * 4 + beast] += sign;
    let after = stair(ctx, comp);
    comp[tier * 4 + beast] -= sign;
    ctx.p.w_stair * (after - before)
}

/// stairFresh: expected readiness change from a new inmate with tier odds `dist`.
pub fn stair_fresh(ctx: &Ctx, dist: &[f64], comp: &mut Comp) -> f64 {
    if ctx.p.w_stair == 0.0 {
        return 0.0;
    }
    let mut sum = 0.0;
    for (t, &p) in dist.iter().enumerate() {
        for b in 0..4 {
            if p != 0.0 {
                sum += (p / 4.0) * stair_delta(ctx, comp, t, b, 1);
            }
        }
    }
    sum
}

/// swapValue: what swapping an inmate of this tier is worth, on average, with
/// `others` other open inmates (`comp` below Rainbow) that Scapegoat's
/// Guillotine may remove.
pub fn swap_value(ctx: &Ctx, tier: usize, comp: &Comp, others: i64) -> f64 {
    let (dist, loss) = replace_odds(ctx, tier, comp, others);
    dist.iter().enumerate().fold(0.0, |sum, (t, p)| sum + p * if t >= TOP { ctx.r } else { ctx.v[t] }) - ctx.v[tier] - loss
}

/// activation: the inmate's best devour or a swap, whichever is worth more,
/// and never below zero (it can just wait).
pub fn activation(ctx: &mut Ctx, tier: usize, beast: usize, comp: &mut Comp, crowd: i64, swap_left: bool) -> f64 {
    let mut value: f64 = 0.0;
    if tier < TOP {
        value = value.max(best_meal(ctx, tier, beast, comp, crowd).0);
    }
    if swap_left && tier as f64 <= ctx.p.swap_max_tier {
        value = value.max(swap_value(ctx, tier, comp, crowd - 1));
    }
    value
}

/// freshActivation: what a brand-new inmate (tier odds `dist`) could do today,
/// of beast `beast` when it is known (the Iron Pestle keeps the replaced
/// inmate's), else any of the 4 at even odds.
pub fn fresh_activation(ctx: &mut Ctx, dist: &[f64], beast: Option<usize>, comp: &mut Comp, crowd: i64, swap_left: bool) -> f64 {
    let beasts = beast.map_or(0..4, |b| b..b + 1);
    let share = 1.0 / beasts.len() as f64;
    let mut sum = 0.0;
    for (t, &p) in dist.iter().enumerate() {
        if p == 0.0 {
            continue;
        }
        for b in beasts.clone() {
            sum += p * share * activation(ctx, t, b, comp, crowd, swap_left);
        }
    }
    sum
}

/// freshValue: a new inmate behind a door, its own worth plus what it could
/// do today from the warden's cell.
pub fn fresh_value(ctx: &mut Ctx, dist: &[f64], comp: &mut Comp, crowd: i64, swap_left: bool) -> f64 {
    let worth = dist.iter().enumerate().fold(0.0, |sum, (t, p)| sum + p * ctx.v[t]);
    ctx.p.w_open * worth + ctx.p.w_now * fresh_activation(ctx, dist, None, comp, crowd, swap_left) + stair_fresh(ctx, dist, comp)
}

/// executionValue: an Execution by an inmate of this tier and beast.
pub fn execution_value(ctx: &Ctx, tier: usize, beast: usize, comp: &Comp) -> f64 {
    let prey = &data().food_chain[beast];
    let mut count = 0;
    let mut lost = 0.0;
    for t in 0..tier {
        for f in prey {
            let k = comp[t * 4 + f.index()];
            count += k;
            lost += f64::from(k) * ctx.v[t];
        }
    }
    if count == 0 {
        return 0.0;
    }
    let to = TOP.min((tier as i32 + count) as usize);
    (if to >= TOP { ctx.r } else { ctx.v[to] }) - ctx.v[tier] - lost
}
