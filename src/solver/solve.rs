//! The Monte Carlo solver and the trainer helpers (js/solver.js 562-790):
//! playouts, the candidate moves with their labels, `solve`, `instant` and
//! the reasons behind the strategy's pick.

use super::chain::chain_odds;
use super::scores::patrol_why;
use super::{Params, Rng32, TOOL_RANK, TUNED, decide, decide_bite};
use crate::engine::data::{Beast, RAINBOW};
use crate::engine::devour::{can_devour_now, hunt_targets, start_options};
use crate::engine::incidents::{CHAIN_REACTION, EXECUTION_EFFICIENCY, execution_victims};
use crate::engine::rules::{clockwise_path, patrol_options};
use crate::engine::state::{Pending, Phase, State};
use crate::engine::text::{cell_label, describe, incident_name, plural, tier_name, tool_name};
use crate::engine::{Action, ChoiceId, apply, data::data};
use std::cmp::Ordering::Equal;

/// playout: plays the run to the end with the strategy. A failed move is
/// followed by an End Day; if that fails too, the playout stops.
pub fn playout(st: &mut State, p: &Params, roll: &mut Rng32) {
    for _ in 0..2000 {
        if st.phase == Phase::Ended {
            break;
        }
        let Some(action) = decide(st, p, roll) else { break };
        if !apply(st, &action).is_ok() && !(action != Action::End && apply(st, &Action::End).is_ok()) {
            break;
        }
    }
}

/// hideFuture: re-rolls everything the player can't see, the game's dice and
/// any occupant behind a door that hasn't been fully revealed.
pub fn hide_future(st: &mut State, seed: u32) {
    let mut random = Rng32::new(seed);
    st.rng = (random.roll() * 4294967296.0) as u32;
    for c in &mut st.cells {
        // A marker whose incident the player can't see is either simulated one, evenly.
        if c.event.is_some() && c.event_known == Some(false) {
            c.event = Some(if random.roll() < 0.5 { CHAIN_REACTION } else { EXECUTION_EFFICIENCY });
        }
        if !c.sealed || c.known >= 2 {
            continue;
        }
        let Some(hidden) = &mut c.hidden else { continue };
        if c.known == 1 {
            hidden.beast = Beast::ALL[(random.roll() * 4.0) as usize];
        } else {
            c.hidden = None;
        }
    }
}

/// A move worth comparing, with its label.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub action: Action,
    pub label: String,
}

fn candidate(action: Action, label: String) -> Candidate {
    Candidate { action, label }
}

/// candidates: the moves worth comparing at this point of the run.
pub fn candidates(st: &State) -> Vec<Candidate> {
    let mut out = Vec::new();
    if st.phase == Phase::Ended {
        return out;
    }
    if let Some(choice) = st.pending.first() {
        match choice {
            Pending::Chain { options, .. } => {
                for &n in options {
                    out.push(candidate(Action::Chain { to: n }, format!("Chain on into {}", cell_label(n))));
                }
                out.push(candidate(Action::EndChain, "End the chain".to_string()));
            }
            Pending::Cell { options, .. } => {
                for &n in options {
                    out.push(candidate(Action::Choose { id: ChoiceId::Cell(n) }, format!("Move to {}", cell_label(n))));
                }
            }
            Pending::Interrogation { options } => {
                for c in options {
                    out.push(candidate(Action::Choose { id: ChoiceId::Name(c.key().to_string()) }, tool_name(c.key())));
                }
            }
            Pending::Tool { options, .. } => {
                for t in options {
                    out.push(candidate(Action::Choose { id: ChoiceId::Name(t.key().to_string()) }, format!("Take {}", tool_name(t.key()))));
                }
            }
            Pending::Gift { options, .. } => {
                for g in options {
                    out.push(candidate(Action::Choose { id: ChoiceId::Name(g.key().to_string()) }, format!("Take {}", tool_name(g.key()))));
                }
            }
        }
        return out;
    }
    if let (Phase::Event, Some(event)) = (st.phase, st.event) {
        out.push(candidate(Action::Event, format!("Confirm {}", incident_name(event.id))));
        return out;
    }
    if st.phase == Phase::Patrol {
        for n in patrol_options(st) {
            let c = &st.cells[n];
            let what = if st.chain {
                let opens = clockwise_path(st, n).iter().filter(|&&k| st.cells[k].sealed).count();
                format!("the chain opens {}", plural(opens, "door"))
            } else if n == data().room {
                "Instigate or Bestow".to_string()
            } else if let Some(id) = c.event {
                if c.event_known == Some(false) { "a Prison Incident marker".to_string() } else { format!("the {} marker", incident_name(id)) }
            } else if c.sealed {
                (if c.nest.is_some() { "the Rat Nest" } else { "a locked door" }).to_string()
            } else if let Some(i) = c.inmate {
                format!("the {}", describe(&i))
            } else {
                "an empty cell".to_string()
            };
            let to = if n == data().room { "the Interrogation Room".to_string() } else { format!("cell {}", cell_label(n)) };
            out.push(candidate(Action::Patrol { cell: n }, format!("Go to {to} ({what})")));
        }
        return out;
    }
    let here = st.cells.get(st.position);
    for o in start_options(st) {
        let meal = if o.scratch {
            "the scratched door".to_string()
        } else {
            format!("the {}", describe(&st.cells[o.cell].inmate.expect("meal")))
        };
        out.push(candidate(Action::Devour { to: o.cell }, format!("Consume {meal} in {}", cell_label(o.cell))));
    }
    if let Some((c, i)) = here.and_then(|c| c.inmate.filter(|_| !c.sealed).map(|i| (c, i)))
        && st.used.swap < st.params.swaps_per_day
    {
        out.push(candidate(Action::Swap, format!("Replace the {} in {}", describe(&i), cell_label(c.n))));
    }
    let prey = hunt_targets(st);
    if !prey.is_empty() {
        out.push(candidate(Action::Hunt, format!("{}: mark {} as prey", data().hunt_name, plural(prey.len(), "Consumable target"))));
    }
    out.push(candidate(Action::End, (if st.day >= st.max_days { "Finish the run" } else { "End the day" }).to_string()));
    out
}

/// pairedSe: the standard error of the mean difference between two options'
/// values on the same dice (rounds an option couldn't play are NaN, skipped).
fn paired_se(a: &[f64], b: &[f64]) -> f64 {
    let (mut n, mut sum, mut sq) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let d = x - y;
        if d.is_nan() {
            continue;
        }
        n += 1.0;
        sum += d;
        sq += d * d;
    }
    if n < 2.0 {
        return f64::INFINITY;
    }
    let m = sum / n;
    (f64::max(0.0, sq / n - m * m) / n).sqrt()
}

/// One ranked option of `solve`.
#[derive(Clone, Debug, PartialEq)]
pub struct SolveOption {
    pub label: String,
    pub action: Action,
    /// Rainbows vaulted per playout.
    pub mean: f64,
    pub se: f64,
    /// The paired standard error of the margin to the leader (0 for the
    /// leader, infinite with fewer than 2 paired rounds).
    pub gap_se: f64,
    /// Playouts that ran.
    pub n: usize,
    /// The best rarity reached, averaged.
    pub best: f64,
    /// The strategy's own pick.
    pub policy: bool,
}

/// solve: plays every candidate out `rollouts` times and ranks them by
/// Rainbows vaulted. Each round shares its dice across candidates (common
/// random numbers), so the differences between moves are measured, not the
/// luck.
pub fn solve(st: &State, rollouts: usize, seed: u32, p: &Params) -> Vec<SolveOption> {
    let mut r = Rollouts::new(st, seed, p);
    r.run(rollouts);
    r.ranking()
}

/// `solve` in steps: `run` adds rounds and `ranking` ranks the rounds so far,
/// so a caller can show partial rankings. Running N rounds in any number of
/// steps gives the same ranking as `solve` with N rollouts.
pub struct Rollouts<'a> {
    base: State,
    p: &'a Params,
    list: Vec<Candidate>,
    pick: Option<Action>,
    random: Rng32,
    vals: Vec<Vec<f64>>,
    best: Vec<f64>,
    rounds: usize,
}

impl<'a> Rollouts<'a> {
    pub fn new(st: &State, seed: u32, p: &'a Params) -> Rollouts<'a> {
        Rollouts::with(st, seed, p, false)
    }

    /// With `bite`, a Consume target pick: only today's first bites compete,
    /// and the strategy's pick is its best bite (see [`decide_bite`]).
    pub fn with(st: &State, seed: u32, p: &'a Params, bite: bool) -> Rollouts<'a> {
        let base = st.clone();
        let mut list = candidates(&base);
        if bite {
            list.retain(|c| matches!(c.action, Action::Devour { .. }));
        }
        let decider = if bite { decide_bite } else { decide };
        let pick = if list.is_empty() { None } else { decider(&base, p, &mut Rng32::new(seed ^ 0x5bd1e995)) };
        let n = list.len();
        Rollouts { base, p, list, pick, random: Rng32::new(seed), vals: vec![Vec::new(); n], best: vec![0.0; n], rounds: 0 }
    }

    /// Rounds played so far.
    pub fn rounds(&self) -> usize {
        self.rounds
    }

    /// Plays up to `rounds` more rounds and returns how many ran (none with
    /// fewer than two candidates: there is nothing to compare).
    pub fn run(&mut self, rounds: usize) -> usize {
        if self.list.len() < 2 {
            return 0;
        }
        for _ in 0..rounds {
            let seed = (self.random.roll() * 4294967296.0) as u32;
            for (k, c) in self.list.iter().enumerate() {
                let mut trial = self.base.clone();
                hide_future(&mut trial, seed);
                if !apply(&mut trial, &c.action).is_ok() {
                    self.vals[k].push(f64::NAN);
                    continue;
                }
                playout(&mut trial, self.p, &mut Rng32::new(seed ^ 0x9e3779b9));
                self.vals[k].push(trial.vault.len() as f64);
                // Second measure: the best rarity reached.
                let top = trial.cells.iter().filter_map(|c| c.inmate.filter(|_| !c.sealed)).map(|i| i.tier).max().unwrap_or(0);
                self.best[k] += f64::from(top.max(if trial.vault.is_empty() { 0 } else { RAINBOW }).max(0));
            }
            self.rounds += 1;
        }
        rounds
    }

    /// The candidates ranked on the rounds played so far.
    pub fn ranking(&self) -> Vec<SolveOption> {
        let (list, vals, best) = (&self.list, &self.vals, &self.best);
        let mut ranked: Vec<(usize, f64, f64, usize)> = vals
            .iter()
            .enumerate()
            .map(|(k, v)| {
                let (mut n, mut sum, mut sq) = (0usize, 0.0, 0.0);
                for &x in v.iter().filter(|x| !x.is_nan()) {
                    n += 1;
                    sum += x;
                    sq += x * x;
                }
                let count = n as f64;
                let mean = if n > 0 { sum / count } else { 0.0 };
                let sd = if n > 1 { f64::max(0.0, sq / count - mean * mean).sqrt() } else { 0.0 };
                (k, mean, if n > 1 { sd / count.sqrt() } else { 0.0 }, n)
            })
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Equal).then(best[b.0].partial_cmp(&best[a.0]).unwrap_or(Equal)));
        let top = ranked.first().map_or(0, |r| r.0);
        ranked
            .into_iter()
            .map(|(k, mean, se, n)| SolveOption {
                label: list[k].label.clone(),
                action: list[k].action.clone(),
                mean,
                se,
                gap_se: if k == top { 0.0 } else { paired_se(&vals[top], &vals[k]) },
                n,
                best: if n > 0 { best[k] / n as f64 } else { 0.0 },
                policy: self.pick.as_ref() == Some(&list[k].action),
            })
            .collect()
    }
}

/// The strategy's pick with its label and a short reason (instant: no playouts).
#[derive(Clone, Debug, PartialEq)]
pub struct Instant {
    pub action: Action,
    pub label: String,
    pub why: String,
}

pub fn instant(st: &State) -> Option<Instant> {
    labelled(st, decide(st, &TUNED, &mut Rng32::new(1))?)
}

/// `instant` on a Consume target pick: the strategy's best bite.
pub fn instant_bite(st: &State) -> Option<Instant> {
    labelled(st, decide_bite(st, &TUNED, &mut Rng32::new(1))?)
}

fn labelled(st: &State, action: Action) -> Option<Instant> {
    let label = match candidates(st).into_iter().find(|c| c.action == action) {
        Some(c) => c.label,
        None => serde_json::to_value(&action).ok().and_then(|v| v["type"].as_str().map(str::to_string)).unwrap_or_default(),
    };
    let why = reason(st, &action);
    Some(Instant { action, label, why })
}

/// The strategy's move after `action`: the same on every resample of what
/// the player can't see, or depending on it (what a door reveals, a roll).
#[derive(Clone, Debug, PartialEq)]
pub enum FollowUp {
    Move(Instant),
    Depends,
}

/// Seeds of the hidden information `follow_up` plays `action` under.
const FOLLOW_SEEDS: u32 = 8;

/// follow_up: plays `action` on resamples of the hidden information and
/// asks the strategy for the next move on each. None when `action` can't
/// be played or the run ends with it.
pub fn follow_up(st: &State, action: &Action) -> Option<FollowUp> {
    let mut next: Vec<Instant> = Vec::new();
    for seed in 1..=FOLLOW_SEEDS {
        let mut trial = st.clone();
        hide_future(&mut trial, seed);
        if !apply(&mut trial, action).is_ok() {
            return None;
        }
        let only = candidates(&trial).len() == 1;
        next.extend(instant(&trial).map(|mut i| {
            if only {
                i.why = "It's the only move left.".to_string();
            }
            i
        }));
    }
    let first = next.first()?;
    // The label too: a Confirm names the incident, which may be one the player can't see yet.
    if next.len() == FOLLOW_SEEDS as usize && next.iter().all(|i| i.action == first.action && i.label == first.label) {
        return Some(FollowUp::Move(next.swap_remove(0)));
    }
    Some(FollowUp::Depends)
}

/// toFixed: like Rust's `{:.d$}`, but a tie rounds up as in the JS.
fn to_fixed(x: f64, d: usize) -> String {
    let wide = format!("{:.*}", d + 30, x);
    let tail = &wide[wide.len() - 30..];
    if x > 0.0 && tail.starts_with('5') && tail[1..].bytes().all(|b| b == b'0') {
        return format!("{:.*}", d, f64::from_bits(x.to_bits() + 1));
    }
    format!("{:.*}", d, x)
}

/// reason: why the strategy plays `action`, in a sentence.
pub fn reason(st: &State, action: &Action) -> String {
    let here = st.cells.get(st.position);
    match action {
        Action::Patrol { cell } => {
            if !patrol_options(st).contains(cell) {
                return String::new();
            }
            let why = patrol_why(st, *cell);
            let mut chars = why.chars();
            chars.next().map_or(String::new(), |f| format!("{}{}.", f.to_uppercase(), chars.as_str()))
        }
        Action::Devour { to } => {
            let (rainbow, bites) = chain_odds(st, st.position, *to);
            format!(
                "The best first meal: its chain averages {} bites, with {}% to reach Chromatic.",
                to_fixed(bites, 1),
                to_fixed(100.0 * rainbow, 0)
            )
        }
        Action::Swap => {
            let here = here.expect("warden's cell");
            if here.ivory {
                return "A Tusk cell: replacing it raises other Inmates a tier.".to_string();
            }
            format!("{} Inmates are worth less than a fresh roll on average.", tier_name(here.inmate.expect("inmate").tier))
        }
        Action::End => {
            let starts = start_options(st);
            if !can_devour_now(st) || starts.is_empty() {
                return "Nothing left today is worth doing.".to_string();
            }
            // A devour is skipped only when it costs more than it gains:
            // nearly always because it would end a prey mark.
            let linked = |id: u32| st.link_of(id).is_some();
            let marked = here.and_then(|c| c.inmate).is_some_and(|i| linked(i.id))
                || starts.iter().any(|o| st.cells[o.cell].inmate.is_some_and(|i| linked(i.id)));
            if marked {
                return "Skip today\u{2019}s consume: it would end a prey mark, and the free tier each morning is worth more than one bite."
                    .to_string();
            }
            "Every consume here would cost more than it gains today.".to_string()
        }
        Action::Event => {
            let event = st.event.expect("incident");
            if event.id == CHAIN_REACTION {
                return "Your next walk opens every sealed door on the way.".to_string();
            }
            let exec = if event.id == EXECUTION_EFFICIENCY { st.cells[event.cell].inmate } else { None };
            let Some(exec) = exec else { return "An incident can only be confirmed.".to_string() };
            let victims = execution_victims(st, event.cell).len();
            format!("The {} removes {} and rises as many tiers.", describe(&exec), plural(victims, "inmate"))
        }
        Action::Chain { .. } => "This bite is worth more than stopping here.".to_string(),
        Action::EndChain => "Every next bite would cost more than it gains.".to_string(),
        Action::Hunt => format!(
            "{}: the Berserk Lion and each prey rise a tier on each of the next {}.",
            plural(hunt_targets(st).len(), "new prey mark"),
            plural(st.params.prey_rounds as usize, "morning")
        ),
        Action::Choose { id } => match st.pending.first() {
            Some(Pending::Gift { .. }) => "Worth the most on this board right now.".to_string(),
            Some(Pending::Cell { .. }) => "The best landing by the patrol scores.".to_string(),
            Some(Pending::Interrogation { .. }) => {
                let ChoiceId::Name(key) = id else { return String::new() };
                format!("{} leaves this board worth more than the other card, on average.", tool_name(key))
            }
            _ => {
                let rank = match id {
                    ChoiceId::Name(key) => TOOL_RANK.iter().position(|t| t.key() == key),
                    ChoiceId::Cell(_) => None,
                };
                rank.map_or(String::new(), |r| format!("Ranked #{} of {} Torture Instruments in training.", r + 1, TOOL_RANK.len()))
            }
        },
    }
}
