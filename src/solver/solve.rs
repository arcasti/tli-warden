//! The Monte Carlo solver and the trainer helpers (js/solver.js 562-790):
//! playouts, the candidate moves with their labels, `solve`, `instant` and
//! the reasons behind the strategy's pick.

use super::bench::follow;
use super::chain::chain_odds;
use super::features::{encode, is_morning};
use super::net::Net;
use super::scores::patrol_why;
use super::{Params, Rng32, TOOL_RANK, TUNED, composition, decide, decide_bite};
use crate::engine::data::{Beast, RAINBOW, Tool};
use crate::engine::devour::{can_devour_now, can_swap, hunt_targets, start_options};
use crate::engine::incidents::{CHAIN_REACTION, EXECUTION_EFFICIENCY, execution_victims};
use crate::engine::rules::{clockwise_path, draw_incident, marker_seen, patrol_options};
use crate::engine::state::{Inmate, Pending, Phase, State};
use crate::engine::text::{cell_label, describe, incident_name, plural, tier_name, tool_name};
use crate::engine::{Action, ChoiceId, apply, data::data};
use std::cmp::Ordering::Equal;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

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

/// `playout` up to the `depth`-th morning it meets, valued there by `net`; a
/// run that ends first, or a move that fails with its End Day, is valued by
/// its Riot loot. A state that is already a morning counts as the first. With
/// depth 0 `st` is valued as it stands, by `net` or by its loot once ended,
/// without a move, so `net` has to read the day block of `features`.
pub fn settle(st: &mut State, p: &Params, roll: &mut Rng32, net: &Net, depth: usize) -> f64 {
    if depth == 0 {
        return if st.phase == Phase::Ended { st.loot() } else { f64::from(net.value(&encode(st))) };
    }
    if to_morning(st, p, roll, depth) { f64::from(net.value(&encode(st))) } else { st.loot() }
}

/// `playout` up to the `depth`-th morning it meets: true when it is reached,
/// false when the run ends first or a move fails with its End Day. A state
/// that is already a morning counts as the first, so with depth 1 it is left
/// as it stands. The strategy is asked at the same states as in `playout`.
pub(super) fn to_morning(st: &mut State, p: &Params, roll: &mut Rng32, depth: usize) -> bool {
    let mut met = 0;
    for _ in 0..2000 {
        if st.phase == Phase::Ended {
            return false;
        }
        if is_morning(st) {
            met += 1;
            if met >= depth {
                return true;
            }
        }
        let Some(action) = decide(st, p, roll) else { return false };
        if !apply(st, &action).is_ok() && !(action != Action::End && apply(st, &Action::End).is_ok()) {
            return false;
        }
    }
    false
}

/// hideFuture: re-rolls everything the player can't see, the game's dice and
/// any occupant behind a door that hasn't been fully revealed.
pub fn hide_future(st: &mut State, seed: u32) {
    let mut random = Rng32::new(seed);
    st.rng = (random.roll() * 4294967296.0) as u32;
    // A marker whose incident the player can't see is any that fits its
    // cell, drawn by weight.
    for n in 0..st.cells.len() {
        if st.cells[n].event.is_none() || marker_seen(st) {
            continue;
        }
        if let Some(id) = draw_incident(st, &[n]) {
            st.cells[n].event = Some(id);
        }
    }
    for c in &mut st.cells {
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

/// Why a Hunt on the target in cell `n`: the prey and its Lion rise each morning.
fn hunt_reason(st: &State, n: usize) -> String {
    format!(
        "The {} in {} and the Berserk Lion rise a tier on each of the next {}.",
        describe(&st.cells[n].inmate.expect("prey")),
        cell_label(n),
        plural(st.params.prey_rounds as usize, "morning")
    )
}

/// The label of a Hunt on the target in cell `n`.
fn hunt_label(st: &State, n: usize) -> String {
    format!("{}: mark the {} in {} as prey", data().hunt_name, describe(&st.cells[n].inmate.expect("prey")), cell_label(n))
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
            Pending::Prey { options } => {
                for &n in options {
                    out.push(candidate(Action::Choose { id: ChoiceId::Cell(n) }, hunt_label(st, n)));
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
                if st.rerolls > 0 {
                    out.push(candidate(Action::Reroll, "Re-roll".to_string()));
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
                if !marker_seen(st) { "a Prison Incident marker".to_string() } else { format!("the {} marker", incident_name(id)) }
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
        && can_swap(st)
    {
        out.push(candidate(Action::Swap, format!("Replace the {} in {}", describe(&i), cell_label(c.n))));
    }
    for n in hunt_targets(st) {
        out.push(candidate(Action::Hunt { cell: n }, hunt_label(st, n)));
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
    /// Riot loot per playout, in Chromatics (`State::loot`).
    pub mean: f64,
    pub se: f64,
    /// The paired standard error of the margin to the leader (0 for the
    /// leader, infinite with fewer than 2 paired rounds).
    pub gap_se: f64,
    /// Playouts that ran: fewer than the rounds for a move the playouts
    /// dropped early (see [`Rollouts`]) or one that could not apply.
    pub n: usize,
    /// The best rarity reached, averaged.
    pub best: f64,
    /// The strategy's own pick.
    pub policy: bool,
}

/// solve: plays every candidate out `rollouts` times and ranks them by
/// Riot loot. Each round shares its dice across candidates (common
/// random numbers), so the differences between moves are measured, not the
/// luck. Past `RACE_FROM` rounds, moves that clearly trail stop early and
/// the solve ends once one move is left (see [`Rollouts`]).
pub fn solve(st: &State, rollouts: usize, seed: u32, p: &Params) -> Vec<SolveOption> {
    let mut r = Rollouts::new(st, seed, p);
    r.run(rollouts);
    r.ranking()
}

/// `solve` in steps: `run` adds rounds and `ranking` ranks the rounds so far,
/// so a caller can show partial rankings. Running N rounds in any number of
/// steps, on any number of threads, gives the same ranking as `solve` with N
/// rollouts.
///
/// Racing: at round `RACE_FROM` and every `RACE_EVERY` rounds after, a
/// candidate whose mean trails the leader's (the best of those still
/// running) by more than `RACE_Z` paired standard errors plays no more
/// rounds and keeps the values it has. The checkpoints fall on fixed round
/// counts, so racing keeps the ranking independent of steps and threads.
/// With one candidate left the solve is over and `run` plays nothing more.
///
/// With a net (see [`Rollouts::net`]), a playout stops at the morning of its
/// depth and takes the net's value there instead of playing the run to the end.
pub struct Rollouts<'a> {
    base: State,
    p: &'a Params,
    /// The net and the depth of the morning it values (see [`settle`]).
    net: Option<(&'a Net, usize)>,
    /// The advice that plays the playouts' days up to that morning.
    inner: Option<Inner<'a>>,
    list: Vec<Candidate>,
    pick: Option<Action>,
    random: Rng32,
    vals: Vec<Vec<f64>>,
    best: Vec<f64>,
    /// The candidates still playing rounds.
    live: Vec<bool>,
    rounds: usize,
}

/// The advice that plays a playout's days: `rollouts` playouts per option,
/// valued by `net` at the `depth`-th morning (see [`Rollouts::inner`]).
#[derive(Clone, Copy)]
pub struct Inner<'a> {
    pub net: &'a Net,
    pub rollouts: usize,
    /// Where `net` values the inner playouts, as in [`settle`]: 1 at the next
    /// morning, 0 right after the action.
    pub depth: usize,
}

/// The first round count where racing drops trailing candidates.
const RACE_FROM: usize = 200;
/// Rounds between racing checkpoints after [`RACE_FROM`].
const RACE_EVERY: usize = 100;
/// The paired standard errors a candidate may trail the leader by.
const RACE_Z: f64 = 3.0;

/// The playouts that count (NaN skipped), their mean and its standard error.
fn stats(v: &[f64]) -> (usize, f64, f64) {
    let (mut n, mut sum, mut sq) = (0usize, 0.0, 0.0);
    for &x in v.iter().filter(|x| !x.is_nan()) {
        n += 1;
        sum += x;
        sq += x * x;
    }
    let count = n as f64;
    let mean = if n > 0 { sum / count } else { 0.0 };
    let sd = if n > 1 { f64::max(0.0, sq / count - mean * mean).sqrt() } else { 0.0 };
    (n, mean, if n > 1 { sd / count.sqrt() } else { 0.0 })
}

impl<'a> Rollouts<'a> {
    pub fn new(st: &State, seed: u32, p: &'a Params) -> Rollouts<'a> {
        Rollouts::with(st, seed, p, None)
    }

    /// With `bite`, a Consume target pick: only today's first bites into
    /// those cells compete, and the strategy's pick is its best bite (see
    /// [`decide_bite`]).
    pub fn with(st: &State, seed: u32, p: &'a Params, bite: Option<&[usize]>) -> Rollouts<'a> {
        let base = st.clone();
        let mut list = candidates(&base);
        if let Some(only) = bite {
            list.retain(|c| matches!(c.action, Action::Devour { to } if only.contains(&to)));
        }
        let roll = &mut Rng32::new(seed ^ 0x5bd1e995);
        let pick = match bite {
            _ if list.is_empty() => None,
            Some(only) => decide_bite(&base, p, roll, only),
            None => decide(&base, p, roll),
        };
        let n = list.len();
        Rollouts { base, p, net: None, inner: None, list, pick, random: Rng32::new(seed), vals: vec![Vec::new(); n], best: vec![0.0; n], live: vec![true; n], rounds: 0 }
    }

    /// Values the playouts with `net` at the `depth`-th morning they meet
    /// (see [`settle`]).
    pub fn net(mut self, net: &'a Net, depth: usize) -> Rollouts<'a> {
        self.net = Some((net, depth));
        self
    }

    /// Plays the playouts' days with `inner` instead of the strategy up to
    /// the morning the net values; without a net, or with a net at depth 0,
    /// it has no effect.
    pub fn inner(mut self, inner: Inner<'a>) -> Rollouts<'a> {
        self.inner = Some(inner);
        self
    }

    /// Rounds played so far.
    pub fn rounds(&self) -> usize {
        self.rounds
    }

    /// Plays up to `rounds` more rounds and returns how many ran (none with
    /// fewer than two candidates running: there is nothing to compare).
    pub fn run(&mut self, rounds: usize) -> usize {
        self.run_on(rounds, 1)
    }

    /// `run` on up to `threads` threads: the seeds are drawn in order first
    /// and the results kept in seed order, so the ranking does not depend on
    /// the thread count. On Windows the extra threads take the caller's
    /// priority.
    pub fn run_on(&mut self, rounds: usize, threads: usize) -> usize {
        let mut ran = 0;
        while ran < rounds && self.running().len() >= 2 {
            // A call stops at each racing checkpoint on its way.
            let checkpoint = RACE_FROM.max((self.rounds / RACE_EVERY + 1) * RACE_EVERY);
            let step = (rounds - ran).min(checkpoint - self.rounds);
            self.play(step, threads);
            ran += step;
            if self.rounds == checkpoint {
                self.race();
            }
        }
        ran
    }

    /// The indices of the candidates still playing rounds.
    fn running(&self) -> Vec<usize> {
        (0..self.list.len()).filter(|&k| self.live[k]).collect()
    }

    /// Drops the running candidates that trail the leader by more than
    /// [`RACE_Z`] paired standard errors.
    fn race(&mut self) {
        let means: Vec<f64> = self.vals.iter().map(|v| stats(v).1).collect();
        let live = self.running();
        let lead = live.iter().copied().fold(live[0], |lead, k| if means[k] > means[lead] { k } else { lead });
        for k in live {
            if k != lead && means[lead] - means[k] > RACE_Z * paired_se(&self.vals[lead], &self.vals[k]) {
                self.live[k] = false;
            }
        }
    }

    /// Plays `rounds` rounds of the running candidates on up to `threads`
    /// threads.
    fn play(&mut self, rounds: usize, threads: usize) {
        let seeds: Vec<u32> = (0..rounds).map(|_| (self.random.roll() * 4294967296.0) as u32).collect();
        let threads = threads.clamp(1, rounds);
        let results: Vec<Vec<(f64, f64)>> = if threads == 1 {
            seeds.iter().map(|&seed| self.round(seed)).collect()
        } else {
            let (this, priority) = (&*self, priority());
            // Rounds are handed out one at a time, so no thread sits idle
            // while another works through a long chunk.
            let (seeds, next) = (&seeds, &AtomicUsize::new(0));
            let mut results: Vec<Option<Vec<(f64, f64)>>> = vec![None; rounds];
            std::thread::scope(|s| {
                let workers: Vec<_> = (0..threads)
                    .map(|_| {
                        s.spawn(move || {
                            set_priority(priority);
                            let mut done = Vec::new();
                            loop {
                                let i = next.fetch_add(1, Relaxed);
                                let Some(&seed) = seeds.get(i) else { break done };
                                done.push((i, this.round(seed)));
                            }
                        })
                    })
                    .collect();
                for w in workers {
                    for (i, round) in w.join().expect("a playout thread panicked") {
                        results[i] = Some(round);
                    }
                }
            });
            results.into_iter().map(|r| r.expect("every round ran")).collect()
        };
        let live = self.running();
        for round in results {
            for (&k, (val, best)) in live.iter().zip(round) {
                self.vals[k].push(val);
                self.best[k] += best;
            }
            self.rounds += 1;
        }
    }

    /// One round on `seed`: per running candidate, the Riot loot (NaN when
    /// the move does not apply) and the best rarity reached (0 then). With a
    /// net, the loot is the net's value at the morning of its depth and the
    /// best rarity is the one reached by then.
    fn round(&self, seed: u32) -> Vec<(f64, f64)> {
        // The hidden future is the same for every candidate of the round.
        let mut hidden = self.base.clone();
        hide_future(&mut hidden, seed);
        self.list
            .iter()
            .zip(&self.live)
            .filter(|&(_, &on)| on)
            .map(|(c, _)| {
                let mut trial = hidden.clone();
                if !apply(&mut trial, &c.action).is_ok() {
                    return (f64::NAN, 0.0);
                }
                let roll = &mut Rng32::new(seed ^ 0x9e3779b9);
                let value = match (self.net, self.inner) {
                    (Some((net, depth)), Some(inner)) if depth > 0 => {
                        if follow(&mut trial, roll, inner.rollouts, (inner.net, inner.depth), seed ^ 0x85ebca6b, depth) { f64::from(net.value(&encode(&trial))) } else { trial.loot() }
                    }
                    (Some((net, depth)), _) => settle(&mut trial, self.p, roll, net, depth),
                    (None, _) => {
                        playout(&mut trial, self.p, roll);
                        trial.loot()
                    }
                };
                // Second measure: the best rarity reached.
                let top = trial.cells.iter().filter_map(|c| c.inmate.filter(|_| !c.sealed)).map(|i| i.tier).max().unwrap_or(0);
                (value, f64::from(top.max(if trial.vault.is_empty() { 0 } else { RAINBOW }).max(0)))
            })
            .collect()
    }

    /// The candidates ranked on the rounds played so far: those still
    /// running first, so the first row has played every round.
    pub fn ranking(&self) -> Vec<SolveOption> {
        let (list, vals, best, live) = (&self.list, &self.vals, &self.best, &self.live);
        let mut ranked: Vec<(usize, (usize, f64, f64))> = vals.iter().map(|v| stats(v)).enumerate().collect();
        ranked.sort_by(|a, b| {
            live[b.0].cmp(&live[a.0]).then(b.1.1.partial_cmp(&a.1.1).unwrap_or(Equal)).then(best[b.0].partial_cmp(&best[a.0]).unwrap_or(Equal))
        });
        let top = ranked.first().map_or(0, |r| r.0);
        ranked
            .into_iter()
            .map(|(k, (n, mean, se))| SolveOption {
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

/// The calling thread's priority, for threads that work on its behalf.
#[cfg(windows)]
fn priority() -> i32 {
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadPriority};
    unsafe { GetThreadPriority(GetCurrentThread()) }
}

#[cfg(not(windows))]
fn priority() -> i32 {
    0
}

/// Gives the current thread a priority [`priority`] returned.
#[cfg(windows)]
pub(crate) fn set_priority(priority: i32) {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY};
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY(priority)) };
}

#[cfg(not(windows))]
pub(crate) fn set_priority(_: i32) {}

/// The strategy's pick with its label and a short reason (instant: no playouts).
#[derive(Debug)]
pub struct Instant {
    pub action: Action,
    pub label: String,
    pub why: String,
}

pub fn instant(st: &State) -> Option<Instant> {
    labelled(st, decide(st, &TUNED, &mut Rng32::new(1))?)
}

/// `instant` on a Consume target pick: the strategy's best bite into one of
/// the cells of `only`.
pub fn instant_bite(st: &State, only: &[usize]) -> Option<Instant> {
    labelled(st, decide_bite(st, &TUNED, &mut Rng32::new(1), only)?)
}

fn labelled(st: &State, action: Action) -> Option<Instant> {
    let label = match candidates(st).into_iter().find(|c| c.action == action) {
        Some(c) => c.label,
        None => serde_json::to_value(&action).ok().and_then(|v| v["type"].as_str().map(str::to_string)).unwrap_or_default(),
    };
    let why = reason(st, &action);
    Some(Instant { action, label, why })
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

/// coinReason: with Kin's Coin of Fate and a morning left, the match a
/// Consume or Replace available now would break (the warden's inmate or a
/// meal in it), as the reason to end the day instead.
fn coin_reason(st: &State) -> Option<String> {
    if !st.has(Tool::Coin) || st.day >= st.max_days {
        return None;
    }
    let (board, _) = composition(st, None);
    let matched = |i: &Inmate| i.tier < RAINBOW && board[i.tier as usize * 4 + i.beast.index()] >= 2;
    let here = st.cells.get(st.position).and_then(|c| c.inmate.filter(|_| !c.sealed));
    let starts = start_options(st);
    let acts = !starts.is_empty() || can_swap(st);
    let mut meals = starts.iter().filter(|o| !o.scratch).filter_map(|o| st.cells[o.cell].inmate);
    let kept = here.filter(|i| acts && matched(i)).or_else(|| meals.find(matched))?;
    let mut cells: Vec<usize> = st
        .cells
        .iter()
        .filter(|c| c.on_board() && c.inmate.is_some_and(|i| i.beast == kept.beast && i.tier == kept.tier))
        .map(|c| c.n)
        .collect();
    cells.sort_by_key(|&n| n != st.position);
    let mut labels: Vec<String> = cells.iter().map(|&n| cell_label(n)).collect();
    let last = labels.pop().expect("a matched cell");
    let coin = data().tool(Tool::Coin);
    Some(format!(
        "Keep the {} {} in {} and {last}: {} has a {}% chance each morning to remove one and raise {} to {}.",
        describe(&kept),
        if cells.len() == 2 { "pair" } else { "match" },
        labels.join(", "),
        tool_name(Tool::Coin.key()),
        to_fixed(coin.chance, 0),
        if cells.len() == 2 { "the other" } else { "another" },
        tier_name(kept.tier + coin.tiers.unwrap_or(0))
    ))
}

/// guillotineCut: with Scapegoat's Guillotine armed for a Replace in the
/// warden's cell, how many of the other inmates on the block it removes, and
/// how many there are.
fn guillotine_cut(st: &State) -> Option<(usize, usize)> {
    let def = data().tool(Tool::Scapegoat);
    if !st.has(Tool::Scapegoat) || !st.cells.get(st.position).is_some_and(|c| c.on_board()) {
        return None;
    }
    let others = st.board_cells().len() - 1;
    (others > 0 && others >= def.min_others.unwrap_or(0)).then(|| (def.count.unwrap_or(0).min(others), others))
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
            if let Some((removed, others)) = guillotine_cut(st) {
                return format!(
                    "{} removes {removed} of the {others} other Inmates, and the new Inmate is sure to be rarer than {}.",
                    tool_name(Tool::Scapegoat.key()),
                    tier_name(here.inmate.expect("inmate").tier)
                );
            }
            if here.ivory {
                return "A Tusk cell: replacing it raises other Inmates a tier.".to_string();
            }
            if st.day >= st.max_days {
                return "The last day: only a Chromatic counts now, and the new Inmate may still consume its way to one today.".to_string();
            }
            format!("{} Inmates are worth less than a fresh roll on average.", tier_name(here.inmate.expect("inmate").tier))
        }
        Action::End => {
            if let Some(why) = coin_reason(st) {
                return why;
            }
            if let Some((removed, others)) = guillotine_cut(st).filter(|_| can_swap(st)) {
                return format!(
                    "Skip the Replace: {} would remove {removed} of the {others} other Inmates.",
                    tool_name(Tool::Scapegoat.key())
                );
            }
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
        Action::Hunt { cell } => hunt_reason(st, *cell),
        Action::Reroll => "A new offer is likely to beat every instrument here.".to_string(),
        Action::Choose { id } => match st.pending.first() {
            Some(Pending::Gift { .. }) => "Worth the most on this board right now.".to_string(),
            Some(Pending::Cell { .. }) => "The best landing by the patrol scores.".to_string(),
            Some(Pending::Prey { .. }) => {
                let ChoiceId::Cell(n) = id else { return String::new() };
                hunt_reason(st, *n)
            }
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::engine::new_game;
    use crate::solver::features::INPUTS;

    /// A net of `INPUTS` -> 2 -> 2 -> 1 with zero weights and output bias 2.5.
    pub(crate) fn zero_net() -> Net {
        let mut bytes = b"WNET".to_vec();
        for n in [INPUTS as u32, 2, 2] {
            bytes.extend_from_slice(&n.to_le_bytes());
        }
        // w1, b1, w2, b2, w3.
        for _ in 0..INPUTS * 2 + 2 + 4 + 2 + 2 {
            bytes.extend_from_slice(&0f32.to_le_bytes());
        }
        bytes.extend_from_slice(&2.5f32.to_le_bytes());
        Net::from_bytes(&bytes).unwrap()
    }

    #[test]
    fn settle_values_a_morning_by_the_net_and_an_ended_run_by_its_loot() {
        let net = zero_net();
        let (mut st, roll) = (new_game("1-0"), &mut Rng32::new(1));
        while !is_morning(&st) {
            let action = decide(&st, &TUNED, roll).unwrap();
            assert!(apply(&mut st, &action).is_ok());
        }
        let mut morning = st.clone();
        assert_eq!(settle(&mut morning, &TUNED, roll, &net, 1), 2.5);
        assert!(morning == st, "a morning is valued as it stands");
        playout(&mut st, &TUNED, &mut Rng32::new(1));
        assert_eq!(st.phase, Phase::Ended);
        assert_eq!(settle(&mut st.clone(), &TUNED, &mut Rng32::new(1), &net, 1), st.loot());
    }

    #[test]
    fn settle_at_depth_zero_values_the_state_as_it_stands() {
        let net = zero_net();
        // A fresh game waits on its starting instrument pick.
        let st = new_game("1-0");
        assert!(!is_morning(&st) && st.phase != Phase::Ended);
        let mut now = st.clone();
        assert_eq!(settle(&mut now, &TUNED, &mut Rng32::new(1), &net, 0), 2.5);
        assert!(now == st, "a state at depth 0 is not played");
        let mut ended = st.clone();
        playout(&mut ended, &TUNED, &mut Rng32::new(1));
        assert_eq!(ended.phase, Phase::Ended);
        assert_eq!(settle(&mut ended.clone(), &TUNED, &mut Rng32::new(1), &net, 0), ended.loot());
    }

    #[test]
    fn settle_at_depth_two_values_the_next_morning() {
        let net = zero_net();
        let (mut st, roll) = (new_game("1-0"), &mut Rng32::new(1));
        while !is_morning(&st) {
            let action = decide(&st, &TUNED, roll).unwrap();
            assert!(apply(&mut st, &action).is_ok());
        }
        // By hand: the strategy's move at this morning, then on to the next.
        let (mut next, roll) = (st.clone(), &mut Rng32::new(5));
        let action = decide(&next, &TUNED, roll).unwrap();
        assert!(apply(&mut next, &action).is_ok() || apply(&mut next, &Action::End).is_ok());
        let want = if to_morning(&mut next, &TUNED, roll, 1) { f64::from(net.value(&encode(&next))) } else { next.loot() };
        let mut deep = st.clone();
        assert_eq!(settle(&mut deep, &TUNED, &mut Rng32::new(5), &net, 2), want);
        assert!(deep == next, "the playout stops at the next morning");
        assert!(deep != st, "a morning at depth 2 is played past");
    }

    #[test]
    fn settle_past_every_morning_is_the_plain_playout() {
        let net = zero_net();
        for seed in [1, 2, 3, 4] {
            let st = new_game(&format!("{seed}-0"));
            let mut plain = st.clone();
            playout(&mut plain, &TUNED, &mut Rng32::new(seed));
            let mut deep = st.clone();
            assert_eq!(settle(&mut deep, &TUNED, &mut Rng32::new(seed), &net, usize::MAX), plain.loot());
            assert!(deep == plain, "seed {seed}");
        }
    }

    #[test]
    fn rollouts_with_a_net_rank_the_same_on_any_thread_count() {
        let net = zero_net();
        let st = new_game("1-0");
        assert!(candidates(&st).len() >= 2);
        let mut one = Rollouts::new(&st, 7, &TUNED).net(&net, 1);
        one.run(16);
        let mut three = Rollouts::new(&st, 7, &TUNED).net(&net, 1);
        three.run_on(16, 3);
        let key = |r: &Rollouts| r.ranking().into_iter().map(|o| (o.action, o.mean.to_bits(), o.n)).collect::<Vec<_>>();
        assert_eq!(key(&one), key(&three));
        assert!(one.ranking().iter().filter(|o| o.n > 0).all(|o| o.mean.is_finite()));
    }
}
