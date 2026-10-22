//! `warden-cli bench`: full runs played by the strategy alone and by the
//! app's advice (playouts at every decision, the strategy's pick unless the
//! leader beats it by more than its paired margin) on the same seeds, so the
//! paired difference measures what following the advice is worth.
//! `warden-cli bench --strategy-only` plays the strategy alone, on the tuned
//! weights or changed ones, to measure a change of weights.

use super::advise::{Pick, advise};
use super::solve::{Rollouts, playout};
use super::{Params, Rng32, TUNED, decide};
use crate::engine::actions::take_tool;
use crate::engine::data::Tool;
use crate::engine::state::{Phase, State, Stats};
use crate::engine::{Action, apply, new_game};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// Moves a run may take before it is cut, as in `playout` and the JS lab.
const GUARD: u32 = 2000;

/// Game `k` of `seed` with the strategy's dice for it, as
/// the JS strategy simulator's newDrill and playGames start one (rewards on),
/// holding `held` as if picked before the first move.
pub fn start(seed: u32, k: u32, held: &[Tool]) -> (State, Rng32) {
    let mut st = new_game(&format!("{seed}-{k}"));
    for &tool in held {
        take_tool(&mut st, tool);
    }
    (st, Rng32::new(k.wrapping_mul(2654435761).wrapping_add(7)))
}

/// One seed played by both players.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Game {
    /// Chromatics vaulted by the strategy alone.
    pub strategy: usize,
    /// Chromatics vaulted following the advice.
    pub advice: usize,
    /// Riot loot of the strategy alone, in Chromatics (`State::loot`).
    pub strategy_loot: f64,
    /// Riot loot following the advice.
    pub advice_loot: f64,
    /// Advice decisions with two or more options.
    pub decisions: usize,
    /// Those where the advice was not the strategy's move.
    pub overruled: usize,
}

/// Plays game `k` both ways; the advice runs `rollouts` playouts per option.
pub fn play(seed: u32, k: u32, rollouts: usize) -> Game {
    let Solo { chromatics: strategy, loot: strategy_loot, .. } = play_strategy(seed, k, &[], 0, &TUNED);

    let (mut st, mut rng) = start(seed, k, &[]);
    let (mut decisions, mut overruled) = (0, 0);
    for step in 0..GUARD {
        if st.phase == Phase::Ended {
            break;
        }
        let Some(mine) = decide(&st, &TUNED, &mut rng) else { break };
        let solve_seed = k.wrapping_mul(7919).wrapping_add(step.wrapping_mul(104729)).wrapping_add(seed);
        let mut r = Rollouts::new(&st, solve_seed, &TUNED);
        let action = if r.run(rollouts) == 0 {
            mine
        } else {
            decisions += 1;
            let ranking = r.ranking();
            let pick = Pick { action: mine.clone(), label: String::new(), why: String::new() };
            match advise(Some(&pick), &ranking, true).and_then(|a| a.option) {
                Some(i) if ranking[i].action != mine => {
                    overruled += 1;
                    ranking[i].action.clone()
                }
                _ => mine,
            }
        };
        if !apply(&mut st, &action).is_ok() && !(action != Action::End && apply(&mut st, &Action::End).is_ok()) {
            break;
        }
    }
    Game { strategy, advice: st.vault.len(), strategy_loot, advice_loot: st.loot(), decisions, overruled }
}

/// Games 0..`games` of `seed` on `threads` threads, in game order.
pub fn bench(games: u32, rollouts: usize, seed: u32, threads: usize) -> Vec<Game> {
    each_game(games, threads, |k| play(seed, k, rollouts))
}

/// A run played by the strategy alone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Solo {
    /// Chromatics vaulted.
    pub chromatics: usize,
    /// Riot loot, in Chromatics (`State::loot`).
    pub loot: f64,
    pub stats: Stats,
    /// Whether the run completed its objective.
    pub objective: bool,
}

/// Plays game `k` of `seed` with the strategy alone on weights `p`, holding
/// `held`, with `rerolls` instrument rerolls for the run.
pub fn play_strategy(seed: u32, k: u32, held: &[Tool], rerolls: u8, p: &Params) -> Solo {
    let (mut st, mut rng) = start(seed, k, held);
    st.rerolls = rerolls;
    playout(&mut st, p, &mut rng);
    Solo { chromatics: st.vault.len(), loot: st.loot(), stats: st.stats, objective: st.objective.is_some_and(|o| o.done) }
}

/// Games 0..`games` of `seed` played by the strategy alone on weights `p`,
/// holding `held`, with `rerolls` rerolls, on `threads` threads, in game order.
pub fn strategy_bench(games: u32, seed: u32, threads: usize, held: &[Tool], rerolls: u8, p: &Params) -> Vec<Solo> {
    each_game(games, threads, |k| play_strategy(seed, k, held, rerolls, p))
}

/// `play` on games 0..`games` on `threads` threads, in game order.
fn each_game<T: Send>(games: u32, threads: usize, play: impl Fn(u32) -> T + Sync) -> Vec<T> {
    let next = AtomicU32::new(0);
    let done = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    if k >= games {
                        break;
                    }
                    let game = play(k);
                    done.lock().unwrap().push((k, game));
                }
            });
        }
    });
    let mut done = done.into_inner().unwrap();
    done.sort_by_key(|g| g.0);
    done.into_iter().map(|g| g.1).collect()
}

/// A mean and its standard error (0 with fewer than two values).
pub fn mean_se(xs: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    if xs.is_empty() {
        return (0.0, 0.0);
    }
    let mean = xs.iter().sum::<f64>() / n;
    if xs.len() < 2 {
        return (mean, 0.0);
    }
    let var = xs.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / (n - 1.0);
    (mean, (var / n).sqrt())
}

/// Chromatics and loot per run, each as (mean, standard error); `gain` is
/// advice minus strategy in loot, paired by seed.
pub struct Summary {
    pub strategy: (f64, f64),
    pub advice: (f64, f64),
    pub strategy_loot: (f64, f64),
    pub advice_loot: (f64, f64),
    pub gain: (f64, f64),
    pub decisions: usize,
    pub overruled: usize,
}

pub fn summarize(games: &[Game]) -> Summary {
    let of = |f: fn(&Game) -> f64| mean_se(&games.iter().map(f).collect::<Vec<_>>());
    Summary {
        strategy: of(|g| g.strategy as f64),
        advice: of(|g| g.advice as f64),
        strategy_loot: of(|g| g.strategy_loot),
        advice_loot: of(|g| g.advice_loot),
        gain: of(|g| g.advice_loot - g.strategy_loot),
        decisions: games.iter().map(|g| g.decisions).sum(),
        overruled: games.iter().map(|g| g.overruled).sum(),
    }
}

/// Runs with at most this many Chromatics count as bad runs in `Spread`.
pub const LOW: usize = 2;

/// Whether a run with `chromatics` is a bad run: see [`LOW`].
pub fn is_low(chromatics: usize) -> bool {
    chromatics <= LOW
}

/// Percentiles `Spread` reports, in percent.
const PERCENTILES: [usize; 5] = [10, 25, 50, 75, 90];

/// How Chromatics per run spread: the share of runs with at most `LOW` (in %),
/// the `PERCENTILES` by nearest rank (the smallest value with at least p% of
/// the runs at or below it), and the most.
#[derive(Clone, Debug, PartialEq)]
pub struct Spread {
    pub low: f64,
    pub percentiles: [usize; PERCENTILES.len()],
    pub max: usize,
}

pub fn spread(runs: &[usize]) -> Spread {
    if runs.is_empty() {
        return Spread { low: 0.0, percentiles: [0; PERCENTILES.len()], max: 0 };
    }
    let mut sorted = runs.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    let low = 100.0 * sorted.iter().filter(|&&c| is_low(c)).count() as f64 / n as f64;
    let percentiles = PERCENTILES.map(|p| sorted[(p * n).div_ceil(100).max(1) - 1]);
    Spread { low, percentiles, max: sorted[n - 1] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strategy_player_replays_plain_games() {
        let games = bench(2, 20, 1, 2);
        assert_eq!(games.len(), 2);
        for (k, g) in games.iter().enumerate() {
            let (mut st, mut rng) = start(1, k as u32, &[]);
            for _ in 0..GUARD {
                let Some(action) = decide(&st, &TUNED, &mut rng) else { break };
                if !apply(&mut st, &action).is_ok() && !(action != Action::End && apply(&mut st, &Action::End).is_ok()) {
                    break;
                }
            }
            assert_eq!(st.phase, Phase::Ended);
            assert_eq!(g.strategy, st.vault.len(), "game {k}");
            assert!(g.decisions > 0 && g.overruled <= g.decisions);
            assert_eq!(play_strategy(1, k as u32, &[], 0, &TUNED), Solo { chromatics: g.strategy, loot: st.loot(), stats: st.stats, objective: st.objective.is_some_and(|o| o.done) }, "game {k}");
        }
        let s = summarize(&games);
        for x in [s.strategy.0, s.strategy.1, s.advice.0, s.advice.1, s.gain.0, s.gain.1] {
            assert!(x.is_finite());
        }
    }

    #[test]
    fn changed_weights_change_the_strategy_runs() {
        let mut p = TUNED.clone();
        *p.weight_mut("w_step").unwrap() = 0.0;
        assert!(p.weight_mut("w_steps").is_none());
        let (tuned, changed) = (strategy_bench(20, 1, 2, &[], 0, &TUNED), strategy_bench(20, 1, 2, &[], 0, &p));
        assert_ne!(tuned, changed);
        assert_eq!(changed[7], play_strategy(1, 7, &[], 0, &p));
    }

    #[test]
    fn held_instruments_start_in_the_tools() {
        let (plain, _) = start(1, 3, &[]);
        assert_eq!(plain.tools, new_game("1-3").tools);
        let (held, _) = start(1, 3, &[Tool::Whip]);
        assert!(held.has(Tool::Whip) && !plain.has(Tool::Whip));
        // The Whip's "-1 destination" drops a die as on a pick.
        assert_eq!(held.dice.len(), (plain.dice.len() - 1).max(1));
    }

    #[test]
    fn spread_takes_nearest_rank_percentiles() {
        // Ten runs with ties: p10 is the 1st, p25 the 3rd, p50 the 5th, p75 the 8th, p90 the 9th.
        let s = spread(&[5, 0, 2, 2, 3, 3, 3, 7, 9, 4]);
        assert_eq!(s, Spread { low: 30.0, percentiles: [0, 2, 3, 5, 7], max: 9 });
        // An even count of four: ranks 1, 1, 2, 3, 4.
        assert_eq!(spread(&[4, 1, 3, 2]), Spread { low: 50.0, percentiles: [1, 1, 2, 3, 4], max: 4 });
        assert_eq!(spread(&[]), Spread { low: 0.0, percentiles: [0; 5], max: 0 });
    }
}
