//! The strategy, a port of js/solver.js lines 16-546: it scores every legal
//! move from what the player can see and plays the best one. Sums run in the
//! JS order and sorts are stable, so a position gets the same pick as the JS.

pub mod advise;
pub mod bench;
pub mod chain;
pub mod scores;
pub mod solve;
pub mod values;

use crate::engine::data::{Beast, RAINBOW, Tool, data};
use crate::engine::devour::{can_devour_now, devour_options, start_options};
use crate::engine::rng::rand;
use crate::engine::state::{Pending, Phase, State};
use crate::engine::{Action, ChoiceId};
use chain::guillotine;
use scores::{best_by_board, devour_scores, hunt_gain, patrol_scores, swap_gain_here};
use std::collections::HashMap;
use values::fresh_activation;

/// Torture Instruments by Chromatics per run (TOOL_RANK).
pub const TOOL_RANK: [Tool; 31] = [
    Tool::Crown, Tool::Trap, Tool::Longchain, Tool::Claws, Tool::Rope, Tool::Coin, Tool::Skewer, Tool::Chains,
    Tool::Muzzle, Tool::Maul, Tool::Revenge, Tool::Tongs, Tool::Stake, Tool::Saw, Tool::Whip, Tool::Maiden,
    Tool::Hook, Tool::Collar, Tool::Guillotine, Tool::Dogtag, Tool::Blacklist, Tool::Scales, Tool::Pestle,
    Tool::Shortchain, Tool::Keys, Tool::Fire, Tool::Beastcage, Tool::Forgedkey, Tool::Axe, Tool::Ironbox,
    Tool::Scapegoat,
];

/// The top tier (Rainbow) as an index.
const TOP: usize = RAINBOW as usize;

/// Strategy weights in tier-value units; see js/solver.js 29-43.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    pub tier_base: f64,
    pub rainbow_mult: f64,
    pub w_open: f64,
    pub w_now: f64,
    pub w_walk: f64,
    pub w_step: f64,
    pub w_nest: f64,
    pub w_chain: f64,
    pub w_stair: f64,
    pub min_devour: f64,
    pub swap_max_tier: f64,
}

impl Params {
    /// The weight with field name `name` (such as `w_step`), for the
    /// strategy bench's `--set`.
    pub fn weight_mut(&mut self, name: &str) -> Option<&mut f64> {
        Some(match name {
            "tier_base" => &mut self.tier_base,
            "rainbow_mult" => &mut self.rainbow_mult,
            "w_open" => &mut self.w_open,
            "w_now" => &mut self.w_now,
            "w_walk" => &mut self.w_walk,
            "w_step" => &mut self.w_step,
            "w_nest" => &mut self.w_nest,
            "w_chain" => &mut self.w_chain,
            "w_stair" => &mut self.w_stair,
            "min_devour" => &mut self.min_devour,
            "swap_max_tier" => &mut self.swap_max_tier,
            _ => return None,
        })
    }
}

/// The tuned weights (js/solver.js withDefaults(TUNED)), what `decide(st, null)` plays.
pub static TUNED: Params = Params {
    tier_base: 2.75,
    rainbow_mult: 1.0,
    w_open: 0.5,
    w_now: 1.5,
    w_walk: 0.75,
    w_step: 1.5,
    w_nest: -6.0,
    w_chain: 16.0,
    w_stair: 1.0,
    min_devour: -2.0,
    swap_max_tier: 2.0,
};

/// mulberry32 (rng32), for the solver's own dice (never the game's).
pub struct Rng32(u32);

impl Rng32 {
    pub fn new(seed: u32) -> Rng32 {
        Rng32(seed)
    }

    pub fn roll(&mut self) -> f64 {
        rand(&mut self.0)
    }
}

/// Open inmates by tier and beast (index tier * 4 + beast).
pub type Comp = [i32; 24];

/// composition: the open inmates below Rainbow leaving out cell `exclude`,
/// plus how many are on the block in all.
pub fn composition(st: &State, exclude: Option<usize>) -> (Comp, i64) {
    let mut comp = [0; 24];
    let mut crowd = 0;
    for c in &st.cells {
        let Some(i) = c.inmate.filter(|_| !c.sealed) else { continue };
        crowd += 1;
        if Some(c.n) == exclude || i.tier >= RAINBOW {
            continue;
        }
        comp[i.tier as usize * 4 + i.beast.index()] += 1;
    }
    (comp, crowd)
}

/// normalize: negative weights count as 0; all zero leaves all zero.
pub fn normalize(weights: &[f64]) -> Vec<f64> {
    let total = weights.iter().fold(0.0, |sum, w| sum + w.max(0.0));
    let total = if total == 0.0 { 1.0 } else { total };
    weights.iter().map(|w| w.max(0.0) / total).collect()
}

/// Who each beast eats, by index, without tools.
pub fn food_chain() -> [Vec<usize>; 4] {
    Beast::ALL.map(|b| data().food_chain[b.index()].iter().map(|f| f.index()).collect())
}

/// Who each beast eats, by index, with the Skewered Rat Heads and the Iron Crown.
pub fn eats_table(st: &State) -> [Vec<usize>; 4] {
    let mut eats = food_chain();
    if st.has(Tool::Skewer) {
        eats[Beast::Rat.index()].push(Beast::Rat.index());
    }
    if st.has(Tool::Crown) {
        eats[Beast::Lion.index()] = (0..4).collect();
    }
    eats
}

/// Everything a decision needs, computed once per position (context).
pub struct Ctx<'a> {
    pub st: &'a State,
    pub p: &'a Params,
    pub eats: [Vec<usize>; 4],
    pub v: [f64; 6],
    pub r: f64,
    pub saw: bool,
    pub muzzle: bool,
    pub skewer: bool,
    pub crown: bool,
    pub revenge: bool,
    pub scratch_extra: usize,
    pub open_dist: Vec<f64>,
    pub open_value: f64,
    pub memo: HashMap<(u32, u128), f64>,
}

pub fn context<'a>(st: &'a State, p: &'a Params) -> Ctx<'a> {
    let v = [0, 1, 2, 3, 4, 5].map(|t| p.tier_base.powi(t));
    let open_dist = normalize(&st.params.tier_weights.iter().map(|&w| f64::from(w)).collect::<Vec<_>>());
    let open_value = open_dist.iter().enumerate().fold(0.0, |sum, (t, p)| sum + p * v[t]);
    Ctx {
        st,
        p,
        eats: eats_table(st),
        v,
        // A full vault stops inmates at Ultimate: the top tier is worth no more.
        r: if st.top_tier() < RAINBOW { v[RAINBOW as usize - 1] } else { v[RAINBOW as usize] * p.rainbow_mult },
        saw: st.has(Tool::Saw),
        muzzle: st.has(Tool::Muzzle),
        skewer: st.has(Tool::Skewer),
        crown: st.has(Tool::Crown),
        revenge: st.has(Tool::Revenge) && st.cells.iter().any(|c| c.sealed),
        scratch_extra: 1 + usize::from(st.params.scratch_stacks_base),
        open_dist,
        open_value,
        memo: HashMap::new(),
    }
}

/// decide: the move the strategy would play now, or None once the run is over.
pub fn decide(st: &State, p: &Params, roll: &mut Rng32) -> Option<Action> {
    let mut ctx = context(st, p);
    if st.phase == Phase::Ended {
        return None;
    }
    let name = |key: &str| Some(Action::Choose { id: ChoiceId::Name(key.to_string()) });
    if let Some(choice) = st.pending.first() {
        return match choice {
            Pending::Chain { from, options, .. } => {
                let bites = devour_options(st, *from).into_iter().filter(|o| options.contains(&o.cell)).collect();
                let next = devour_scores(&mut ctx, bites);
                Some(match next.first() {
                    Some(&(to, score)) if score >= p.min_devour => Action::Chain { to },
                    _ => Action::EndChain,
                })
            }
            Pending::Gift { options, .. } => {
                let keys: Vec<&str> = options.iter().map(|g| g.key()).collect();
                name(best_by_board(&ctx, &keys, roll))
            }
            Pending::Interrogation { options } => {
                let keys: Vec<&str> = options.iter().map(|c| c.key()).collect();
                name(best_by_board(&ctx, &keys, roll))
            }
            // Sewer Ratway: the cell a patrol would score best.
            Pending::Cell { options, .. } => {
                Some(Action::Choose { id: ChoiceId::Cell(patrol_scores(&mut ctx, options)[0].0) })
            }
            Pending::Tool { options, .. } => {
                name(TOOL_RANK.iter().find(|t| options.contains(t)).unwrap_or(&options[0]).key())
            }
        };
    }
    // An incident is confirmed, never declined (S16BoardEventPop.lua:17-38).
    if st.phase == Phase::Event && st.event.is_some() {
        return Some(Action::Event);
    }
    if st.phase == Phase::Patrol {
        let scores = patrol_scores(&mut ctx, &st.destinations);
        return Some(scores.first().map_or(Action::End, |&(cell, _)| Action::Patrol { cell }));
    }
    // Manage: a Hunt goes first, as a devour after it already counts the
    // prey marks it would end.
    if hunt_gain(&ctx) > 0.0 {
        return Some(Action::Hunt);
    }
    let swap_gain = swap_gain_here(&ctx);
    if can_devour_now(st) {
        let devours = devour_scores(&mut ctx, start_options(st));
        if let Some(&(to, best)) = devours.first().filter(|d| d.1 >= p.min_devour) {
            // A swap first can be worth more than a weak devour: the new
            // inmate may still devour today.
            if swap_gain > f64::NEG_INFINITY {
                let (mut comp, crowd) = composition(st, Some(st.position));
                let dist = guillotine(&ctx, st.cells[st.position].inmate.map_or(0, |i| i.tier as usize), &comp, crowd - 1).0;
                let swap_first = swap_gain + p.w_now * fresh_activation(&mut ctx, &dist, &mut comp, crowd, false);
                if swap_first > best {
                    return Some(Action::Swap);
                }
            }
            return Some(Action::Devour { to });
        }
    }
    Some(if swap_gain > 0.0 { Action::Swap } else { Action::End })
}

/// The strategy's bite once Consume is pressed: decide's own, else the best
/// scored of today's targets; None when the inmate has none. Only bites
/// into the cells of `only` count.
pub fn decide_bite(st: &State, p: &Params, roll: &mut Rng32, only: &[usize]) -> Option<Action> {
    if let Some(bite @ Action::Devour { to }) = decide(st, p, roll)
        && only.contains(&to)
    {
        return Some(bite);
    }
    let mut ctx = context(st, p);
    let options = start_options(st).into_iter().filter(|o| only.contains(&o.cell)).collect();
    devour_scores(&mut ctx, options).first().map(|&(to, _)| Action::Devour { to })
}
