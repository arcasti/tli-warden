//! `warden-cli bench`: full runs played by the strategy alone and by the
//! app's advice (playouts at every decision, the strategy's pick unless the
//! leader beats it by more than its paired margin) on the same seeds, so the
//! paired difference measures what following the advice is worth.

use super::advise::{Pick, advise};
use super::solve::{Rollouts, playout};
use super::{Rng32, TUNED, decide};
use crate::engine::state::{Phase, State};
use crate::engine::{Action, apply, new_game};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// Moves a run may take before it is cut, as in `playout` and the JS lab.
const GUARD: u32 = 2000;

/// Game `k` of `seed` with the strategy's dice for it, as
/// the JS strategy simulator's newDrill and playGames start one (rewards on).
pub fn start(seed: u32, k: u32) -> (State, Rng32) {
    (new_game(&format!("{seed}-{k}")), Rng32::new(k.wrapping_mul(2654435761).wrapping_add(7)))
}

/// One seed played by both players.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Game {
    /// Chromatics vaulted by the strategy alone.
    pub strategy: usize,
    /// Chromatics vaulted following the advice.
    pub advice: usize,
    /// Advice decisions with two or more options.
    pub decisions: usize,
    /// Those where the advice was not the strategy's move.
    pub overruled: usize,
}

/// Plays game `k` both ways; the advice runs `rollouts` playouts per option.
pub fn play(seed: u32, k: u32, rollouts: usize) -> Game {
    let (mut st, mut rng) = start(seed, k);
    playout(&mut st, &TUNED, &mut rng);
    let strategy = st.vault.len();

    let (mut st, mut rng) = start(seed, k);
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
            let row = ranking.iter().position(|o| o.action == mine);
            let pick = Pick { action: mine.clone(), label: String::new(), why: String::new() };
            match advise(Some(&pick), row, &ranking, true).and_then(|a| a.option) {
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
    Game { strategy, advice: st.vault.len(), decisions, overruled }
}

/// Games 0..`games` of `seed` on `threads` threads, in game order.
pub fn bench(games: u32, rollouts: usize, seed: u32, threads: usize) -> Vec<Game> {
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
                    let game = play(seed, k, rollouts);
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

/// Chromatics per run, each as (mean, standard error); `gain` is advice
/// minus strategy, paired by seed.
pub struct Summary {
    pub strategy: (f64, f64),
    pub advice: (f64, f64),
    pub gain: (f64, f64),
    pub decisions: usize,
    pub overruled: usize,
}

pub fn summarize(games: &[Game]) -> Summary {
    let of = |f: fn(&Game) -> f64| mean_se(&games.iter().map(f).collect::<Vec<_>>());
    Summary {
        strategy: of(|g| g.strategy as f64),
        advice: of(|g| g.advice as f64),
        gain: of(|g| g.advice as f64 - g.strategy as f64),
        decisions: games.iter().map(|g| g.decisions).sum(),
        overruled: games.iter().map(|g| g.overruled).sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strategy_player_replays_plain_games() {
        let games = bench(2, 20, 1, 2);
        assert_eq!(games.len(), 2);
        for (k, g) in games.iter().enumerate() {
            let (mut st, mut rng) = start(1, k as u32);
            for _ in 0..GUARD {
                let Some(action) = decide(&st, &TUNED, &mut rng) else { break };
                if !apply(&mut st, &action).is_ok() && !(action != Action::End && apply(&mut st, &Action::End).is_ok()) {
                    break;
                }
            }
            assert_eq!(st.phase, Phase::Ended);
            assert_eq!(g.strategy, st.vault.len(), "game {k}");
            assert!(g.decisions > 0 && g.overruled <= g.decisions);
        }
        let s = summarize(&games);
        for x in [s.strategy.0, s.strategy.1, s.advice.0, s.advice.1, s.gain.0, s.gain.1] {
            assert!(x.is_finite());
        }
    }
}
