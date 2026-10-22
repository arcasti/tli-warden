//! The move to recommend from a ranking: the strategy's pick until the
//! playouts count, then the playouts' best. The app shows it, and `bench`
//! plays it to measure what following the advice is worth.

use super::solve::SolveOption;
use crate::engine::data::Gift;
use crate::engine::state::{Pending, State};
use crate::engine::text::tool_name;
use crate::engine::gifts::apply_gift;
use crate::engine::{Action, ChoiceId};

/// The strategy's own pick and why, before any playout.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub action: Action,
    pub label: String,
    pub why: String,
}

/// The move to recommend; `option` is its row in the ranking.
#[derive(Clone, Debug, PartialEq)]
pub struct Advice {
    pub action: Action,
    pub label: String,
    pub why: String,
    pub option: Option<usize>,
}

/// The strategy's pick until the playouts count (`live`), then the ranking's
/// first row.
pub fn advise(pick: Option<&Pick>, ranking: &[SolveOption], live: bool) -> Option<Advice> {
    let mine = pick.and_then(|p| ranking.iter().position(|o| o.action == p.action));
    if !live || ranking.len() < 2 {
        return match (pick, ranking) {
            (Some(p), _) => Some(Advice { action: p.action.clone(), label: p.label.clone(), why: p.why.clone(), option: mine }),
            (None, [only]) => Some(Advice { action: only.action.clone(), label: only.label.clone(), why: "It's the only move here.".to_string(), option: Some(0) }),
            _ => None,
        };
    }
    let best = &ranking[0];
    let why = match pick {
        Some(p) if mine == Some(0) => p.why.clone(),
        // Within the margin of error of the first row, the pick is as good.
        Some(p) if mine.is_some_and(|k| also_good(ranking, Some(0)).contains(&k)) => {
            format!("The playouts rate it as good as the strategy's pick ({}).", p.label)
        }
        Some(p) => format!("The playouts found a better move than the strategy's pick ({}).", p.label),
        None => "The playouts rate it best.".to_string(),
    };
    Some(Advice { action: best.action.clone(), label: best.label.clone(), why, option: Some(0) })
}

/// The rows of the ranking other than the advised one that are within the
/// margin of error of it, in ranking order.
pub fn also_good(ranking: &[SolveOption], advised: Option<usize>) -> Vec<usize> {
    let Some(best) = advised.and_then(|k| ranking.get(k)) else { return Vec::new() };
    ranking
        .iter()
        .enumerate()
        .filter(|&(k, o)| {
            // gap_se is the margin to the first row; the first row's margin
            // to the advised row is the advised row's.
            let se = if k == 0 { best.gap_se } else { o.gap_se };
            Some(k) != advised && (best.mean - o.mean).abs() <= 1.96 * se
        })
        .map(|(k, _)| k)
        .collect()
}

/// What the window shows for the advice: a headline replacing the move's name, and the line under the board.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub headline: Option<String>,
    pub why: String,
}

/// The advice as the window shows it. On a named pick (treasure, card,
/// instrument) where every option ties, the headline says so instead of
/// naming one; on a treasure pick, the why names the treasures that change
/// nothing on the board.
pub fn shown(st: &State, ranking: &[SolveOption], advice: &Advice) -> Shown {
    let others = also_good(ranking, advice.option);
    let tie = ranking.len() >= 2 && others.len() == ranking.len() - 1;
    let alike = "The playouts can't tell them apart.";
    let plain = Shown { headline: None, why: advice.why.clone() };
    match st.pending.first() {
        Some(Pending::Gift { options, .. }) => {
            let (noops, rest): (Vec<Gift>, Vec<Gift>) = options.iter().partition(|&&g| changes_nothing(st, g));
            if tie {
                let why = match (noops.len(), rest.len()) {
                    (0, _) => alike.to_string(),
                    (_, 0) => "No treasure changes anything on this board.".to_string(),
                    (n, r) => format!(
                        "{} {} nothing on this board. {} {} no better.",
                        names(&noops),
                        change(n),
                        names(&rest),
                        if r == 1 { "scores" } else { "score" },
                    ),
                };
                return Shown { headline: Some("Take any treasure".to_string()), why };
            }
            match advice.option.filter(|&k| gift_at(options, ranking, k).is_some_and(|g| noops.contains(&g))) {
                Some(k) => {
                    let idle = idle_tied(st, options, ranking, k);
                    Shown {
                        headline: None,
                        why: format!("{} {} nothing on this board, and the other treasures score lower.", names(&idle), change(idle.len())),
                    }
                }
                None => plain,
            }
        }
        Some(Pending::Interrogation { .. }) if tie => Shown { headline: Some("Take either card".to_string()), why: alike.to_string() },
        Some(Pending::Tool { .. }) if tie => Shown { headline: Some("Take any instrument".to_string()), why: alike.to_string() },
        _ => plain,
    }
}

/// On a treasure pick, when the advised treasure changes nothing and a tied
/// one changes something, the best-ranked such one, since the playouts can't
/// rank them apart; otherwise the advice as it is.
pub fn prefer_change(st: &State, ranking: &[SolveOption], advice: Advice) -> Advice {
    let (Some(Pending::Gift { options, .. }), Some(k)) = (st.pending.first(), advice.option) else { return advice };
    let idle = idle_tied(st, options, ranking, k);
    let acts = |j: usize| gift_at(options, ranking, j).is_some_and(|g| !idle.contains(&g));
    if acts(k) {
        return advice;
    }
    let Some(best) = also_good(ranking, Some(k)).into_iter().find(|&j| acts(j)) else { return advice };
    let row = &ranking[best];
    Advice {
        action: row.action.clone(),
        label: row.label.clone(),
        why: format!("The playouts can't tell it from {}, which {} nothing on this board.", names(&idle), change(idle.len())),
        option: Some(best),
    }
}

/// The treasures of ranking row `advised` and of the rows tied with it that
/// change nothing on the board, in ranking order.
fn idle_tied(st: &State, options: &[Gift], ranking: &[SolveOption], advised: usize) -> Vec<Gift> {
    let mut rows = also_good(ranking, Some(advised));
    rows.push(advised);
    rows.sort_unstable();
    rows.into_iter().filter_map(|k| gift_at(options, ranking, k)).filter(|&g| changes_nothing(st, g)).collect()
}

/// The treasure that ranking row `k` takes, if it is one of `options`.
fn gift_at(options: &[Gift], ranking: &[SolveOption], k: usize) -> Option<Gift> {
    match &ranking.get(k)?.action {
        Action::Choose { id: ChoiceId::Name(key) } => options.iter().copied().find(|g| g.key() == key.as_str()),
        _ => None,
    }
}

/// Whether the treasure's own effect leaves the cells and the vault as they
/// are, over three dice. The pick's full action carries the game on past it
/// (new incident markers spawn), so only the effect is tried.
fn changes_nothing(st: &State, gift: Gift) -> bool {
    [0x9e37_79b9u32, 0x85eb_ca6b, 0xc2b2_ae35].iter().all(|&rng| {
        let mut trial = st.clone();
        trial.rng = rng;
        apply_gift(&mut trial, gift);
        trial.cells == st.cells && trial.vault == st.vault
    })
}

/// The treasures' names joined as "A, B and C".
fn names(gifts: &[Gift]) -> String {
    join(&gifts.iter().map(|g| tool_name(g.key())).collect::<Vec<_>>())
}

/// The verb for `n` treasures that change nothing.
fn change(n: usize) -> &'static str {
    if n == 1 { "changes" } else { "change" }
}

/// Names joined as "A, B and C".
fn join(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(cell: usize, mean: f64, gap_se: f64, policy: bool) -> SolveOption {
        SolveOption { label: format!("Go to cell {cell}"), action: Action::Patrol { cell }, mean, se: 0.1, gap_se, n: 400, best: 0.0, policy }
    }

    #[test]
    fn advice_takes_the_playouts_best() {
        let pick = Pick { action: Action::Patrol { cell: 8 }, label: "Go to cell 8".to_string(), why: "A new Inmate.".to_string() };
        // The strategy's pick ranks second, 0.040 behind the leader, outside its 1.96 * 0.01 margin.
        let tie = [option(7, 4.07, 0.0, false), option(8, 4.03, 0.01, true)];
        let a = advise(Some(&pick), &tie, true).unwrap();
        assert_eq!((a.label.as_str(), a.option), ("Go to cell 7", Some(0)));
        assert_eq!(a.action, Action::Patrol { cell: 7 });
        assert_eq!(a.why, "The playouts found a better move than the strategy's pick (Go to cell 8).");

        let led = [option(8, 4.07, 0.0, true), option(7, 4.03, 0.06, false)];
        let a = advise(Some(&pick), &led, true).unwrap();
        assert_eq!((a.label.as_str(), a.why.as_str()), ("Go to cell 8", "A new Inmate."));
        // Before enough playouts, the strategy's pick stands.
        assert_eq!(advise(Some(&pick), &tie, false).unwrap().option, Some(1));
    }

    #[test]
    fn advice_in_a_tie_rates_the_pick_as_good() {
        let pick = Pick { action: Action::Patrol { cell: 8 }, label: "Go to cell 8".to_string(), why: "A new Inmate.".to_string() };
        let tie = [option(7, 0.0, 0.0, false), option(8, 0.0, 0.0, true), option(9, 0.0, 0.0, false)];
        let a = advise(Some(&pick), &tie, true).unwrap();
        // The move stays the first row; only the why changes.
        assert_eq!((a.action, a.option), (Action::Patrol { cell: 7 }, Some(0)));
        assert_eq!(a.why, "The playouts rate it as good as the strategy's pick (Go to cell 8).");
    }
}
