//! Chain math (js/solver.js 113-251): the exact expected outcome of a devour,
//! including the random chain after it.

use super::{Comp, Ctx, TOP, composition, eats_table, food_chain, normalize};
use crate::engine::data::{Beast, RAINBOW, Tool, data};
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

/// replaceOdds: tier odds (Normal to Rainbow) for the inmate a Replace brings
/// in for one of tier `tier`, after every instrument that changes its rarity
/// (engine swap), and the expected worth of the inmates the Replace removes,
/// with `others` other open inmates on the block (those below Rainbow counted
/// in `comp`).
/// - Hangman's Noose: no Normal or Magic roll (`swap_dist`).
/// - Scapegoat's Guillotine: with at least min_others others, it removes count
///   of them at random and the new inmate is sure to be rarer than the old.
/// - Burning Stake: replacing an inmate below Legendary, a roll below
///   Legendary becomes Legendary at the data chance.
/// - Severed Iron Maiden: a new inmate on the block with as many rarer open
///   inmates as less rare ones rises its tiers.
pub fn replace_odds(ctx: &Ctx, tier: usize, comp: &Comp, others: i64) -> (Vec<f64>, f64) {
    let st = ctx.st;
    let top = st.top_tier() as usize;
    let mut dist = swap_dist(ctx);
    dist.resize(TOP + 1, 0.0);
    let def = data().tool(Tool::Scapegoat);
    let removed = if st.has(Tool::Scapegoat) && others >= def.min_others.unwrap_or(0) as i64 && others > 0 {
        (def.count.unwrap_or(0) as i64).min(others)
    } else {
        0
    };
    let mut loss = 0.0;
    if removed > 0 {
        for p in dist.iter_mut().take(tier + 1) {
            *p = 0.0;
        }
        if dist.iter().any(|&p| p > 0.0) {
            dist = normalize(&dist);
        } else {
            // Nothing rarer left to roll: the engine makes one a tier up.
            dist = vec![0.0; TOP + 1];
            dist[(tier + 1).min(top)] = 1.0;
        }
        let worth = comp.iter().enumerate().fold(0.0, |sum, (i, &k)| sum + f64::from(k) * ctx.v[i / 4]);
        loss = removed as f64 / others as f64 * worth;
    }
    let stake = data().tool(Tool::Stake);
    let legendary = stake.tier.unwrap_or(RAINBOW) as usize;
    if st.has(Tool::Stake) && tier < legendary {
        let c = stake.chance / 100.0;
        let below: f64 = dist[..legendary].iter().sum();
        for p in &mut dist[..legendary] {
            *p *= 1.0 - c;
        }
        dist[legendary] += c * below;
    }
    if st.has(Tool::Maiden) {
        // Open inmates by tier; the Guillotine's removals come first, so the
        // Maiden's odds at each tier average over the removed inmates.
        let mut counts = [0i64; TOP + 1];
        for (i, &k) in comp.iter().enumerate() {
            counts[i / 4] += i64::from(k);
        }
        let n: i64 = counts.iter().sum();
        let k = removed.min(n);
        let to = |t: usize| (t + tiers(Tool::Maiden)).min(top);
        for t in 0..TOP {
            let mass = dist[t];
            if mass == 0.0 || to(t) == t {
                continue;
            }
            let above: i64 = counts[t + 1..].iter().sum();
            let below: i64 = counts[..t].iter().sum();
            let level = n - above - below;
            let mut fire = 0.0;
            for a in 0..=k {
                for b in 0..=k - a {
                    if above - a == below - b {
                        fire += choose(above, a) * choose(below, b) * choose(level, k - a - b);
                    }
                }
            }
            let moved = mass * fire / choose(n, k);
            dist[t] -= moved;
            dist[to(t)] += moved;
        }
    }
    (dist, loss)
}

/// n choose k, 0 when k is out of range.
fn choose(n: i64, k: i64) -> f64 {
    if k < 0 || k > n {
        return 0.0;
    }
    (0..k).fold(1.0, |c, i| c * (n - i) as f64 / (i + 1) as f64)
}

/// coinOfFate: the expected worth of Kin's Coin of Fate for the open inmates
/// below Rainbow in `comp`: the odds it fires on one of the mornings left
/// times one firing's average gain, a random match's partner rising its tiers
/// less the matched inmate it removes. 0 without the Coin, a match or a morning.
pub fn coin_of_fate(ctx: &Ctx, comp: &Comp) -> f64 {
    let st = ctx.st;
    let mornings = st.max_days.saturating_sub(st.day);
    if !st.has(Tool::Coin) || mornings == 0 {
        return 0.0;
    }
    let (mut matched, mut gain) = (0, 0.0);
    for (i, &k) in comp.iter().enumerate().filter(|&(_, &k)| k >= 2) {
        let t = i / 4;
        let to = TOP.min(t + tiers(Tool::Coin));
        matched += k;
        gain += f64::from(k) * ((if to >= TOP { ctx.r } else { ctx.v[to] }) - 2.0 * ctx.v[t]);
    }
    if matched == 0 {
        return 0.0;
    }
    let miss = 1.0 - data().tool(Tool::Coin).chance / 100.0;
    (1.0 - miss.powi(mornings as i32)) * gain / f64::from(matched)
}

/// A meal an eater can take: its tier and beast, the tiers the bite adds
/// before Muzzle, Skewer, Crown and the rolls, and the odds it upgrades at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meal {
    pub tier: usize,
    pub beast: usize,
    pub base: usize,
    pub chance: f64,
}

/// The terms of a bite by an eater of beast b at tier t on a meal of tier mt
/// and beast mb, or None when it can't eat it (engine `can_eat`).
fn eat_terms(ctx: &Ctx, t: usize, b: usize, mt: usize, mb: usize) -> Option<Meal> {
    let (lower, higher) = reach(ctx, b);
    let ok = match mt.cmp(&t) {
        std::cmp::Ordering::Less => lower && ctx.chain[b].contains(&mb),
        std::cmp::Ordering::Greater => higher && ctx.chain[b].contains(&mb),
        std::cmp::Ordering::Equal => ctx.eats[b].contains(&mb),
    };
    ok.then(|| terms(ctx, t, b, mt, mb))
}

/// Whether an eater of beast b eats countered inmates of lower tiers (Fanged
/// Guillotine for a Wolf, Cage Fight for any) and of higher ones (the Maul
/// for a Mammoth).
fn reach(ctx: &Ctx, b: usize) -> (bool, bool) {
    ((b == Beast::Wolf.index() && ctx.fangs) || ctx.cage_fight, b == Beast::Mammoth.index() && ctx.maul)
}

/// The terms of a bite it can make. The base is `sure_gain`'s before the
/// bonuses: the Maul lands a Mammoth `above` tiers over the meal, at least +1.
/// The chance is `upgrade_chance`: Oppress costs a Wolf with the Fanged
/// Guillotine `oppress_per_tier` percent per tier below it.
fn terms(ctx: &Ctx, t: usize, b: usize, mt: usize, mb: usize) -> Meal {
    let mut base = 1;
    if b == Beast::Mammoth.index() && ctx.maul {
        let above = data().tool(Tool::Maul).above.unwrap_or(0) as i64;
        base = base.max(mt as i64 + above - t as i64);
    }
    let mut chance = 1.0;
    if mt < t && b == Beast::Wolf.index() && ctx.fangs {
        chance = (100.0 - data().oppress_per_tier * (t - mt) as f64).max(0.0) / 100.0;
    }
    Meal { tier: mt, beast: mb, base: base as usize, chance }
}

/// The tiers an eater of beast b at tier t takes meals from: its own, then
/// the lower and higher ones it reaches.
fn meal_tiers(ctx: &Ctx, t: usize, b: usize) -> impl Iterator<Item = usize> {
    let (lower, higher) = reach(ctx, b);
    std::iter::once(t).chain(if lower { 0 } else { t }..t).chain(t + 1..if higher { TOP } else { t + 1 })
}

/// meals: the meals in `comp` an eater of beast b at tier t can take, in a
/// fixed buffer (one slot per tier and beast) and their count: the same-tier
/// ones first in `ctx.eats` order, then the lower and higher tiers it reaches.
pub fn meals(ctx: &Ctx, t: usize, b: usize, comp: &Comp) -> ([Meal; 20], usize) {
    let mut out = [Meal { tier: 0, beast: 0, base: 0, chance: 0.0 }; 20];
    let mut n = 0;
    for mt in meal_tiers(ctx, t, b) {
        for &mb in if mt == t { &ctx.eats[b] } else { &ctx.chain[b] } {
            if comp[mt * 4 + mb] > 0 {
                out[n] = terms(ctx, t, b, mt, mb);
                n += 1;
            }
        }
    }
    (out, n)
}

/// Whether an eater of beast b at tier t has a meal in `comp`.
pub fn fed(ctx: &Ctx, t: usize, b: usize, comp: &Comp) -> bool {
    meal_tiers(ctx, t, b).any(|mt| (if mt == t { &ctx.eats[b] } else { &ctx.chain[b] }).iter().any(|&mb| comp[mt * 4 + mb] > 0))
}

/// The worth of an inmate of tier t.
fn worth(ctx: &Ctx, t: usize) -> f64 {
    if t >= TOP { ctx.r } else { ctx.v[t] }
}

/// rise: value from here on for an eater of beast b that just reached tier t;
/// its beast re-rolls (kept under Cage Fight), then it chains into a random
/// valid meal if it has one. With the Crowded Beastcage the meal's re-sealed
/// cell may open with a fresh door inmate (engine do_devour); only its worth
/// is counted, not the chain it could feed.
pub fn rise(ctx: &mut Ctx, t: usize, b: usize, comp: &mut Comp, crowd: i64) -> f64 {
    let cage = ctx.beastcage * ctx.open_value;
    if t >= TOP {
        return ctx.r + cage;
    }
    if ctx.cage_fight {
        return cage + land(ctx, t, b, comp, crowd);
    }
    let mut sum = 0.0;
    for b in 0..4 {
        sum += land(ctx, t, b, comp, crowd);
    }
    cage + sum / 4.0
}

/// land: value from here on for an eater of beast b settled at tier t.
fn land(ctx: &mut Ctx, t: usize, b: usize, comp: &mut Comp, crowd: i64) -> f64 {
    if !ctx.revenge {
        return feed(ctx, t, b, comp, crowd);
    }
    // Avenger's Casket: with its chance a door opens with a plain counter of
    // the new form at its tier (engine do_devour), whatever the tools widen.
    let casket = data().tool(Tool::Revenge).chance / 100.0;
    let n = ctx.chain[b].len();
    let mut spawned = 0.0;
    for i in 0..n {
        let f = ctx.chain[b][i];
        comp[t * 4 + f] += 1;
        spawned += ctx.v[t] + feed(ctx, t, b, comp, crowd + 1);
        comp[t * 4 + f] -= 1;
    }
    casket * spawned / n as f64 + (1.0 - casket) * feed(ctx, t, b, comp, crowd)
}

/// The memo key of feed: tier, beast, the exact crowd under the Muzzle (its
/// bonus starts once the chain has shrunk the crowd to max_inmates) and the
/// counts an eater at tier t can reach.
fn feed_key(ctx: &Ctx, t: usize, b: usize, comp: &Comp, crowd: i64) -> (u32, u128) {
    let mut head = t as u32 | (b as u32) << 3;
    if ctx.muzzle {
        head |= 1 << 5 | ((crowd + 1024) as u32) << 6;
    }
    let mut counts = 0u128;
    for (i, &k) in comp.iter().enumerate().take(20) {
        // Below tier t only the beasts the chain can eat down.
        if i >= t * 4 || ctx.down[b] >> (i % 4) & 1 == 1 {
            counts |= ((k + 2) as u128 & 63) << (6 * i);
        }
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
    let (options, n) = meals(ctx, t, b, comp);
    let total: i32 = options[..n].iter().map(|m| comp[m.tier * 4 + m.beast]).sum();
    let mut out = ctx.v[t];
    if total != 0 {
        let mut sum = 0.0;
        for &m in &options[..n] {
            let i = m.tier * 4 + m.beast;
            let k = comp[i];
            comp[i] -= 1;
            sum += f64::from(k) * bite(ctx, t, b, m, true, comp, crowd);
            comp[i] += 1;
        }
        out = sum / f64::from(total);
    }
    ctx.memo.insert(key, out);
    out
}

/// bite: one bite by an eater at tier t; the meal is gone and the eater rises
/// (engine do_devour). `chained` marks a bite after the day's first, which
/// the Barbed Collar rolls on. A failed Oppress roll adds nothing, the rolled
/// tiers included, and the eater stays at tier t.
pub fn bite(ctx: &mut Ctx, t: usize, b: usize, meal: Meal, chained: bool, comp: &mut Comp, crowd: i64) -> f64 {
    let mut gain = meal.base;
    if ctx.muzzle && crowd <= data().tool(Tool::Muzzle).max_inmates.unwrap_or(0) as i64 {
        gain += tiers(Tool::Muzzle);
    }
    if ctx.skewer && b == Beast::Rat.index() {
        gain += tiers(Tool::Skewer);
    }
    if ctx.crown && b == Beast::Lion.index() {
        gain += tiers(Tool::Crown);
    }
    let mut after = |ctx: &mut Ctx, g: usize| rise(ctx, TOP.min(t + g), b, comp, crowd - 1);
    let extra = ctx.extra[usize::from(chained)];
    let mut won = 0.0;
    for (k, &p) in extra.iter().enumerate().filter(|&(_, &p)| p > 0.0) {
        won += p * after(ctx, gain + k);
    }
    let rest = if meal.chance < 1.0 { meal.chance * won + (1.0 - meal.chance) * after(ctx, 0) } else { won };
    -worth(ctx, meal.tier) + rest
}

/// devourDelta: expected change in value from a devour, the chosen first meal
/// (tier, beast) then the random chain. `comp` leaves out the eater and still
/// holds the meal.
pub fn devour_delta(ctx: &mut Ctx, tier: usize, beast: usize, meal: (usize, usize), scratch: bool, comp: &mut Comp, crowd: i64) -> f64 {
    let (mt, mb) = meal;
    let i = mt * 4 + mb;
    let terms = if scratch {
        Meal { tier, beast: mb, base: 1 + ctx.scratch_extra, chance: 1.0 }
    } else {
        // devour_options offers only the bites can_eat allows, which eat_terms mirrors.
        eat_terms(ctx, tier, beast, mt, mb).unwrap_or(Meal { tier: mt, beast: mb, base: 1, chance: 1.0 })
    };
    if !scratch {
        comp[i] -= 1;
    }
    let value = bite(ctx, tier, beast, terms, false, comp, crowd) - ctx.v[tier];
    if !scratch {
        comp[i] += 1;
    }
    value
}

/// bestMeal: the best first meal for an eater of this tier and beast, as
/// (value, (meal tier, meal beast)); (-inf, None) when it has none.
pub fn best_meal(ctx: &mut Ctx, tier: usize, beast: usize, comp: &mut Comp, crowd: i64) -> (f64, Option<(usize, usize)>) {
    let mut best = (f64::NEG_INFINITY, None);
    let (options, n) = meals(ctx, tier, beast, comp);
    for &m in &options[..n] {
        let value = devour_delta(ctx, tier, beast, (m.tier, m.beast), false, comp, crowd);
        if value > best.0 {
            best = (value, Some((m.tier, m.beast)));
        }
    }
    best
}

/// chainOdds: exact odds that a devour from cell `from` into cell `to` chains
/// up to Rainbow, and its expected bites, by the rules of `rise` with Avenger's
/// Casket (engine do_devour: after each bite, while a door is left, its chance
/// to open one with a meal for the eater's new tier and beast) and no other tools.
pub fn chain_odds(st: &State, from: usize, to: usize) -> (f64, f64) {
    let target = &st.cells[to];
    let meal = if target.sealed { target.hidden } else { target.inmate };
    let (Some(eater), Some(meal)) = (st.cells[from].inmate, meal) else { return (0.0, 0.0) };
    let (mut comp, _) = composition(st, Some(from));
    if !target.sealed {
        comp[meal.tier as usize * 4 + meal.beast.index()] -= 1;
    }
    // With reusable cells a meal's cell seals again and a scratched door stays sealed.
    let reseal = st.params.reusable_cells;
    let doors = st.sealed_cells().len() + usize::from(reseal && !target.sealed) - usize::from(!reseal && target.sealed);
    let casket = if st.has(Tool::Revenge) { data().tool(Tool::Revenge).chance / 100.0 } else { 0.0 };
    let mut odds = Odds { eats: eats_table(st), food: food_chain(), casket, reseal, doors, comp, memo: HashMap::new() };
    let gain = if target.sealed && target.scratch { 2 + usize::from(st.params.scratch_stacks_base) } else { 1 };
    let (p, n) = odds.at_tier(TOP.min(eater.tier as usize + gain));
    (p, 1.0 + n)
}

/// (tier, beast, doors, the tiers' composition from tier up) to the odds.
type Memo = HashMap<(usize, usize, usize, Vec<i32>), (f64, f64)>;

struct Odds {
    eats: [Vec<usize>; 4],
    /// Who each beast eats without tools: the meals the Casket brings.
    food: [Vec<usize>; 4],
    /// Avenger's Casket's chance to fire after a bite; 0 without it.
    casket: f64,
    /// A meal's cell seals again (reusable cells).
    reseal: bool,
    doors: usize,
    comp: Comp,
    memo: Memo,
}

impl Odds {
    /// (P(reach Rainbow), expected further bites) from tier t, beast re-rolling.
    fn at_tier(&mut self, t: usize) -> (f64, f64) {
        if t >= TOP {
            return (1.0, 0.0);
        }
        let (mut p, mut n) = (0.0, 0.0);
        for b in 0..4 {
            let (mut pb, mut nb) = self.eat(t, b);
            if self.casket > 0.0 && self.doors > 0 {
                let foods = self.food[b].clone();
                let (mut ps, mut ns) = (0.0, 0.0);
                self.doors -= 1;
                for &f in &foods {
                    self.comp[t * 4 + f] += 1;
                    let (pf, nf) = self.eat(t, b);
                    self.comp[t * 4 + f] -= 1;
                    ps += pf / foods.len() as f64;
                    ns += nf / foods.len() as f64;
                }
                self.doors += 1;
                pb = self.casket * ps + (1.0 - self.casket) * pb;
                nb = self.casket * ns + (1.0 - self.casket) * nb;
            }
            p += pb / 4.0;
            n += nb / 4.0;
        }
        (p, n)
    }

    fn eat(&mut self, t: usize, b: usize) -> (f64, f64) {
        // The doors only change the odds through the Casket.
        let doors = if self.casket > 0.0 { self.doors } else { 0 };
        let key = (t, b, doors, self.comp[t * 4..20].to_vec());
        if let Some(&hit) = self.memo.get(&key) {
            return hit;
        }
        let foods = self.eats[b].clone();
        let total: i32 = foods.iter().map(|&f| self.comp[t * 4 + f]).sum();
        let mut out = (0.0, 0.0);
        if total != 0 {
            let (mut p, mut n) = (0.0, 0.0);
            let reseal = usize::from(self.reseal);
            for &f in &foods {
                let k = self.comp[t * 4 + f];
                if k == 0 {
                    continue;
                }
                self.comp[t * 4 + f] -= 1;
                self.doors += reseal;
                let (pf, nf) = self.at_tier(t + 1);
                self.doors -= reseal;
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
