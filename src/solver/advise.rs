//! The move to recommend from a ranking: the strategy's pick until the
//! playouts count, then the playouts' best. The app shows it, and `bench`
//! plays it to measure what following the advice is worth.

use super::solve::SolveOption;
use crate::engine::Action;

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
