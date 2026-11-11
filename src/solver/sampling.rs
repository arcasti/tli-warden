//! Training samples for a learned value function: the morning states of
//! runs played by the strategy or by a net's advice, each labelled with the
//! mean Riot loot of playouts from it on re-rolled hidden information,
//! and sibling groups: the next-morning states of the options of one decision
//! under one hidden future, each labelled with the mean loot of playouts that
//! share their futures across the group. The playouts that give the labels
//! follow the strategy, or the net's advice when `Advice::label_rollouts` is set.

use super::bench::{advised, each_game, solve_seed, start};
use super::features::{INPUTS, Row, encode, is_morning};
use super::net::Net;
use super::solve::{Candidate, candidates, hide_future, playout, to_morning};
use super::{Rng32, TUNED, decide};
use crate::engine::state::{Phase, State};
use crate::engine::{Action, apply};

/// Moves a run may take before it is cut, as in `playout`.
const GUARD: u32 = 2000;

/// One state and the loot of playouts from it. Samples with the same
/// `group` are the options of one decision under one hidden future; a morning
/// sample is alone in its group.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub row: Row,
    pub label: f32,
    pub group: u32,
}

/// The advice that plays the sampled games in place of the strategy: `rollouts` playouts per
/// option, valued by `net` at the `depth`-th morning they meet (1 = the next morning). With
/// `label_rollouts` set, the playouts that give the labels follow this advice at that many
/// playouts per option, with the same net and depth, instead of the strategy.
#[derive(Clone, Copy)]
pub struct Advice<'a> {
    pub net: &'a Net,
    pub rollouts: usize,
    pub depth: usize,
    pub label_rollouts: Option<usize>,
}

/// Games 0..`games` of `seed` on `threads` threads; every morning state gives one sample.
/// Game `k` has `k % 3` instrument rerolls. Each move is the strategy's pick, or the pick of
/// `advice` when one is given. With odds `explore`, a move with two or more candidates is a
/// uniformly random candidate instead. Each label is the mean loot of `label_playouts`
/// playouts that follow the strategy, or `advice` when its `label_rollouts` is set. With odds
/// `siblings`, a decision with two or more candidates also gives a sibling group (see
/// `sibling_group`), its labels also the mean of `label_playouts` playouts. Group ids are
/// 0, 1, 2, ... in the order of the result, distinct across games.
pub fn samples(
    games: u32,
    seed: u32,
    threads: usize,
    explore: f64,
    label_playouts: usize,
    advice: Option<Advice>,
    siblings: f64,
) -> Vec<Sample> {
    let mut out = Vec::new();
    let mut next = 0;
    for list in each_game(games, threads, |k| game(seed, k, explore, label_playouts, advice, siblings)) {
        let base = next;
        next += list.last().map_or(0, |s| s.group + 1);
        out.extend(list.into_iter().map(|s| Sample { group: base + s.group, ..s }));
    }
    out
}

/// The samples of game `k` of `seed`, with group ids 0, 1, 2, ... within the game.
fn game(seed: u32, k: u32, explore: f64, label_playouts: usize, advice: Option<Advice>, siblings: f64) -> Vec<Sample> {
    let (mut st, mut rng) = start(seed, k, &[]);
    st.rerolls = (k % 3) as u8;
    let mut own = Rng32::new(seed.wrapping_mul(0x9e3779b9) ^ k.wrapping_mul(40503).wrapping_add(1));
    let labels = advice.and_then(|a| a.label_rollouts.map(|n| Advice { rollouts: n, ..a }));
    let mut out = Vec::new();
    let mut group = 0;
    for step in 0..GUARD {
        if st.phase == Phase::Ended {
            break;
        }
        if is_morning(&st) {
            let label = label(&st, label_playouts, &mut own, labels);
            out.push(Sample { row: encode(&st), label, group });
            group += 1;
        }
        let list = candidates(&st);
        if siblings > 0.0 && list.len() >= 2 && own.roll() < siblings {
            let members = sibling_group(&st, &list, (own.roll() * 4294967296.0) as u32, label_playouts, labels);
            if members.len() >= 2 {
                out.extend(members.into_iter().map(|(row, label)| Sample { row, label, group }));
                group += 1;
            }
        }
        let Some(mine) = decide(&st, &TUNED, &mut rng) else {
            break;
        };
        let base = match advice {
            Some(a) => advised(&st, &mine, a.rollouts, solve_seed(seed, k, step), Some((a.net, a.depth))).unwrap_or(mine),
            None => mine,
        };
        let action = if list.len() >= 2 && own.roll() < explore {
            list[((own.roll() * list.len() as f64) as usize).min(list.len() - 1)].action.clone()
        } else {
            base
        };
        if !apply(&mut st, &action).is_ok() && !(action != Action::End && apply(&mut st, &Action::End).is_ok()) {
            break;
        }
    }
    out
}

/// Mean loot of `playouts` playouts from `st`, each on hidden information re-rolled with a
/// seed drawn from `own`; they follow the strategy, or `labels` when given (see `finish`).
fn label(st: &State, playouts: usize, own: &mut Rng32, labels: Option<Advice>) -> f32 {
    let mut total = 0.0;
    for _ in 0..playouts {
        total += replay(st, (own.roll() * 4294967296.0) as u32, labels);
    }
    (total / playouts.max(1) as f64) as f32
}

/// Loot of one playout from `st` on hidden information re-rolled with seed `s` (see `finish`).
fn replay(st: &State, s: u32, labels: Option<Advice>) -> f64 {
    let mut clone = st.clone();
    hide_future(&mut clone, s);
    finish(&mut clone, &mut Rng32::new(s ^ 0x9e3779b9), labels, s ^ 0x85ebca6b);
    clone.loot()
}

/// Plays `st` to the end of the run: a strategy playout drawing from `rng`, or, with `labels`,
/// the moves of that advice at `labels.rollouts` playouts per option, the strategy's picks
/// drawing from `rng`. The advice's playouts at step `step` are seeded by
/// `solve_seed(seed, 0, step)`, so the result depends only on the arguments.
fn finish(st: &mut State, rng: &mut Rng32, labels: Option<Advice>, seed: u32) {
    let Some(a) = labels else {
        playout(st, &TUNED, rng);
        return;
    };
    for step in 0..GUARD {
        if st.phase == Phase::Ended {
            break;
        }
        let Some(mine) = decide(st, &TUNED, rng) else {
            break;
        };
        let action = advised(st, &mine, a.rollouts, solve_seed(seed, 0, step), Some((a.net, a.depth))).unwrap_or(mine);
        if !apply(st, &action).is_ok() && !(action != Action::End && apply(st, &Action::End).is_ok()) {
            break;
        }
    }
}

/// The sibling group of the decision at `st`: for each candidate of `list` that applies on
/// `st` with its hidden future re-rolled by `h`, the next-morning state the strategy reaches
/// and the mean loot of `playouts` playouts from that morning. The first continues the run
/// under `h`, as the advice's rounds with a net see them (`Rollouts::round` then `settle`);
/// playout `j` re-rolls the hidden information of the morning with a seed derived from `h`
/// and `j`. The playouts follow the strategy, or `labels` when given, with the same seeds
/// for every member. A candidate whose run ends before a morning gives none.
fn sibling_group(st: &State, list: &[Candidate], h: u32, playouts: usize, labels: Option<Advice>) -> Vec<(Row, f32)> {
    let mut hidden = st.clone();
    hide_future(&mut hidden, h);
    let mut members = Vec::new();
    for c in list {
        let mut t = hidden.clone();
        if !apply(&mut t, &c.action).is_ok() {
            continue;
        }
        let mut roll = Rng32::new(h ^ 0x9e3779b9);
        if !to_morning(&mut t, &TUNED, &mut roll, 1) {
            continue;
        }
        let row = encode(&t);
        let rest: f64 = (1..playouts).map(|j| replay(&t, h.wrapping_add((j as u32).wrapping_mul(0x9e3779b9)), labels)).sum();
        finish(&mut t, &mut roll, labels, h ^ 0x85ebca6b);
        members.push((row, ((t.loot() + rest) / playouts as f64) as f32));
    }
    members
}

/// Magic bytes at the start of a samples file.
const MAGIC: &[u8; 4] = b"WSM2";

/// Writes `samples` in the samples file layout, little endian: magic `WSM2`, u32 inputs,
/// u32 count, u32 nnz, f32 labels[count], u32 groups[count], u32 offsets[count+1],
/// u16 indices[nnz], f32 values[nnz]. Row `r` is entries `offsets[r]..offsets[r+1]`. The
/// groups are renumbered 0, 1, 2, ... in file order: adjacent samples with equal `group`
/// share a number.
pub fn write_samples(samples: &[Sample], out: &mut impl std::io::Write) -> std::io::Result<()> {
    let nnz: usize = samples.iter().map(|s| s.row.len()).sum();
    let mut buf = Vec::with_capacity(16 + samples.len() * 12 + 4 + nnz * 6);
    buf.extend_from_slice(MAGIC);
    for n in [INPUTS, samples.len(), nnz] {
        buf.extend_from_slice(&(n as u32).to_le_bytes());
    }
    for s in samples {
        buf.extend_from_slice(&s.label.to_le_bytes());
    }
    let mut group = 0u32;
    for (r, s) in samples.iter().enumerate() {
        if r > 0 && s.group != samples[r - 1].group {
            group += 1;
        }
        buf.extend_from_slice(&group.to_le_bytes());
    }
    let mut offset = 0u32;
    buf.extend_from_slice(&offset.to_le_bytes());
    for s in samples {
        offset += s.row.len() as u32;
        buf.extend_from_slice(&offset.to_le_bytes());
    }
    for s in samples {
        for &(i, _) in &s.row {
            buf.extend_from_slice(&i.to_le_bytes());
        }
    }
    for s in samples {
        for &(_, v) in &s.row {
            buf.extend_from_slice(&v.to_le_bytes());
        }
    }
    out.write_all(&buf)
}

/// Reads a samples file written by `write_samples`.
pub fn read_samples(bytes: &[u8]) -> Result<Vec<Sample>, String> {
    if bytes.len() < 16 || &bytes[..4] != MAGIC {
        return Err("not a samples file".into());
    }
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let (inputs, count, nnz) = (word(4) as usize, word(8) as usize, word(12) as usize);
    if inputs != INPUTS {
        return Err(format!("samples file has {inputs} inputs, expected {INPUTS}"));
    }
    let (labels, groups, offsets) = (16, 16 + count * 4, 16 + count * 8);
    let indices = offsets + (count + 1) * 4;
    let values = indices + nnz * 2;
    if bytes.len() != values + nnz * 4 {
        return Err(format!("samples file is {} bytes, expected {}", bytes.len(), values + nnz * 4));
    }
    let mut list = Vec::with_capacity(count);
    for r in 0..count {
        let (a, b) = (word(offsets + r * 4) as usize, word(offsets + r * 4 + 4) as usize);
        if a > b || b > nnz {
            return Err(format!("samples file row {r} has offsets {a}..{b} of {nnz}"));
        }
        let row = (a..b)
            .map(|e| {
                let i = u16::from_le_bytes(bytes[indices + e * 2..indices + e * 2 + 2].try_into().unwrap());
                (i, f32::from_bits(word(values + e * 4)))
            })
            .collect();
        let group = word(groups + r * 4);
        if r > 0 && group < word(groups + r * 4 - 4) {
            return Err(format!("samples file row {r} has group {group}, below the group before it"));
        }
        list.push(Sample { row, label: f32::from_bits(word(labels + r * 4)), group });
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_do_not_depend_on_threads() {
        assert_eq!(samples(6, 1, 1, 0.2, 2, None, 0.0), samples(6, 1, 3, 0.2, 2, None, 0.0));
    }

    #[test]
    fn advised_samples_do_not_depend_on_threads() {
        let zero = crate::solver::solve::tests::zero_net();
        let advice = Some(Advice { net: &zero, rollouts: 8, depth: 1, label_rollouts: None });
        let one = samples(4, 1, 1, 0.0, 1, advice, 0.0);
        assert_eq!(one, samples(4, 1, 3, 0.0, 1, advice, 0.0));
        assert!(!one.is_empty() && one.iter().all(|s| s.label.is_finite()));
    }

    #[test]
    fn advised_labels_do_not_depend_on_threads() {
        let zero = crate::solver::solve::tests::zero_net();
        let advice = Some(Advice { net: &zero, rollouts: 4, depth: 2, label_rollouts: Some(2) });
        let one = samples(2, 1, 1, 0.2, 1, advice, 0.3);
        assert_eq!(one, samples(2, 1, 3, 0.2, 1, advice, 0.3));
        assert!(one.windows(2).any(|w| w[0].group == w[1].group), "no sibling group");
        assert!(one.iter().all(|s| s.label.is_finite() && s.label >= 0.0));
    }

    #[test]
    fn advised_labels_keep_the_rows_and_groups() {
        let zero = crate::solver::solve::tests::zero_net();
        let strategy = Advice { net: &zero, rollouts: 4, depth: 1, label_rollouts: None };
        let advised = Advice { label_rollouts: Some(2), ..strategy };
        let a = samples(2, 1, 2, 0.2, 1, Some(strategy), 0.3);
        let b = samples(2, 1, 2, 0.2, 1, Some(advised), 0.3);
        let shape = |l: &[Sample]| l.iter().map(|s| (s.row.clone(), s.group)).collect::<Vec<_>>();
        assert_eq!(shape(&a), shape(&b));
    }

    #[test]
    fn every_morning_gives_a_finite_label() {
        let list = samples(4, 1, 2, 0.0, 2, None, 0.0);
        assert!((4..=4 * 21).contains(&list.len()), "{} samples", list.len());
        assert!(list.iter().all(|s| s.label.is_finite() && s.label >= 0.0));
    }

    #[test]
    fn exploring_changes_the_samples() {
        assert_ne!(samples(4, 1, 2, 0.5, 1, None, 0.0), samples(4, 1, 2, 0.0, 1, None, 0.0));
    }

    #[test]
    fn samples_file_round_trips() {
        let list = samples(3, 1, 1, 0.2, 1, None, 0.5);
        let mut bytes = Vec::new();
        write_samples(&list, &mut bytes).unwrap();
        assert_eq!(read_samples(&bytes).unwrap(), list);
    }

    #[test]
    fn bad_samples_files_are_errors() {
        let mut bytes = Vec::new();
        write_samples(&samples(1, 1, 1, 0.0, 1, None, 0.0), &mut bytes).unwrap();
        assert!(read_samples(&bytes[..bytes.len() - 1]).is_err());
        assert!(read_samples(&bytes[..10]).is_err());
        bytes[0] = b'X';
        assert!(read_samples(&bytes).is_err());
    }

    #[test]
    fn without_siblings_every_sample_is_alone() {
        let list = samples(4, 1, 2, 0.2, 1, None, 0.0);
        assert_eq!(list, samples(4, 1, 2, 0.2, 1, None, 0.0));
        assert!(list.iter().enumerate().all(|(r, s)| s.group == r as u32));
    }

    #[test]
    fn siblings_form_groups() {
        let list = samples(2, 1, 1, 0.2, 1, None, 1.0);
        assert_eq!(list, samples(2, 1, 3, 0.2, 1, None, 1.0));
        assert!(list.windows(2).all(|w| w[0].group <= w[1].group));
        let mut sizes = vec![0; list.last().map_or(0, |s| s.group as usize + 1)];
        for s in &list {
            sizes[s.group as usize] += 1;
        }
        assert!(sizes.iter().all(|&n| n >= 1) && sizes.iter().any(|&n| n >= 2), "{sizes:?}");
        for s in &list {
            assert!(s.label.is_finite() && s.label >= 0.0);
            assert!(s.row.windows(2).all(|w| w[0].0 < w[1].0) && s.row.iter().all(|&(i, _)| (i as usize) < INPUTS));
        }
    }

    #[test]
    fn sibling_playouts_share_the_rows_and_average_the_loot() {
        let (mut st, mut rng) = start(1, 0, &[]);
        let mut list = candidates(&st);
        while list.len() < 2 {
            let action = decide(&st, &TUNED, &mut rng).expect("a decision with two candidates");
            assert!(apply(&mut st, &action).is_ok());
            list = candidates(&st);
        }
        let one = sibling_group(&st, &list, 12345, 1, None);
        let many = sibling_group(&st, &list, 12345, 16, None);
        assert!(one.len() >= 2 && one.len() == many.len());
        assert!(one.iter().zip(&many).all(|(a, b)| a.0 == b.0));
        assert_eq!(many, sibling_group(&st, &list, 12345, 16, None));
        assert!(one.iter().zip(&many).any(|(a, b)| a.1 != b.1));
    }

    #[test]
    fn samples_file_keeps_the_groups() {
        let row = vec![(1u16, 1.0f32)];
        let list: Vec<Sample> = [7, 7, 3, 9, 9, 9].into_iter().map(|group| Sample { row: row.clone(), label: 0.5, group }).collect();
        let mut bytes = Vec::new();
        write_samples(&list, &mut bytes).unwrap();
        let back: Vec<u32> = read_samples(&bytes).unwrap().iter().map(|s| s.group).collect();
        assert_eq!(back, [0, 0, 1, 2, 2, 2]);
        // A decreasing group array is rejected.
        bytes[16 + 6 * 4 + 4..16 + 6 * 4 + 8].copy_from_slice(&5u32.to_le_bytes());
        assert!(read_samples(&bytes).is_err());
    }
}
