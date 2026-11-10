//! Training samples for a learned value function: the morning states of
//! runs played by the strategy or by a net's advice, each labelled with the
//! mean Riot loot of strategy playouts from it on re-rolled hidden information.

use super::bench::{advised, each_game, solve_seed, start};
use super::features::{INPUTS, Row, encode, is_morning};
use super::net::Net;
use super::solve::{candidates, hide_future, playout};
use super::{Rng32, TUNED, decide};
use crate::engine::state::{Phase, State};
use crate::engine::{Action, apply};

/// Moves a run may take before it is cut, as in `playout`.
const GUARD: u32 = 2000;

/// One morning state and the mean loot of strategy playouts from it.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub row: Row,
    pub label: f32,
}

/// The advice that plays the sampled games in place of the strategy: `rollouts` playouts per
/// option, valued by `net` at the next morning.
#[derive(Clone, Copy)]
pub struct Advice<'a> {
    pub net: &'a Net,
    pub rollouts: usize,
}

/// Games 0..`games` of `seed` on `threads` threads; every morning state gives one sample.
/// Game `k` has `k % 3` instrument rerolls. Each move is the strategy's pick, or the pick of
/// `advice` when one is given. With odds `explore`, a move with two or more candidates is a
/// uniformly random candidate instead. Each label is the mean loot of `label_playouts`
/// strategy playouts.
pub fn samples(games: u32, seed: u32, threads: usize, explore: f64, label_playouts: usize, advice: Option<Advice>) -> Vec<Sample> {
    each_game(games, threads, |k| game(seed, k, explore, label_playouts, advice)).into_iter().flatten().collect()
}

/// The samples of game `k` of `seed`.
fn game(seed: u32, k: u32, explore: f64, label_playouts: usize, advice: Option<Advice>) -> Vec<Sample> {
    let (mut st, mut rng) = start(seed, k, &[]);
    st.rerolls = (k % 3) as u8;
    let mut own = Rng32::new(seed.wrapping_mul(0x9e3779b9) ^ k.wrapping_mul(40503).wrapping_add(1));
    let mut out = Vec::new();
    for step in 0..GUARD {
        if st.phase == Phase::Ended {
            break;
        }
        if is_morning(&st) {
            let label = label(&st, label_playouts, &mut own);
            out.push(Sample { row: encode(&st), label });
        }
        let Some(mine) = decide(&st, &TUNED, &mut rng) else {
            break;
        };
        let base = match advice {
            Some(a) => advised(&st, &mine, a.rollouts, solve_seed(seed, k, step), Some(a.net)).unwrap_or(mine),
            None => mine,
        };
        let list = candidates(&st);
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

/// Mean loot of `playouts` strategy playouts from `st`, each on hidden information
/// re-rolled with a seed drawn from `own`.
fn label(st: &State, playouts: usize, own: &mut Rng32) -> f32 {
    let mut total = 0.0;
    for _ in 0..playouts {
        let s = (own.roll() * 4294967296.0) as u32;
        let mut clone = st.clone();
        hide_future(&mut clone, s);
        playout(&mut clone, &TUNED, &mut Rng32::new(s ^ 0x9e3779b9));
        total += clone.loot();
    }
    (total / playouts.max(1) as f64) as f32
}

/// Magic bytes at the start of a samples file.
const MAGIC: &[u8; 4] = b"WSMP";

/// Writes `samples` in the samples file layout, little endian: magic `WSMP`, u32 inputs,
/// u32 count, u32 nnz, f32 labels[count], u32 offsets[count+1], u16 indices[nnz],
/// f32 values[nnz]. Row `r` is entries `offsets[r]..offsets[r+1]`.
pub fn write_samples(samples: &[Sample], out: &mut impl std::io::Write) -> std::io::Result<()> {
    let nnz: usize = samples.iter().map(|s| s.row.len()).sum();
    let mut buf = Vec::with_capacity(16 + samples.len() * 8 + 4 + nnz * 6);
    buf.extend_from_slice(MAGIC);
    for n in [INPUTS, samples.len(), nnz] {
        buf.extend_from_slice(&(n as u32).to_le_bytes());
    }
    for s in samples {
        buf.extend_from_slice(&s.label.to_le_bytes());
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
    let (labels, offsets) = (16, 16 + count * 4);
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
        list.push(Sample { row, label: f32::from_bits(word(labels + r * 4)) });
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_do_not_depend_on_threads() {
        assert_eq!(samples(6, 1, 1, 0.2, 2, None), samples(6, 1, 3, 0.2, 2, None));
    }

    #[test]
    fn advised_samples_do_not_depend_on_threads() {
        let zero = crate::solver::solve::tests::zero_net();
        let advice = Some(Advice { net: &zero, rollouts: 8 });
        let one = samples(4, 1, 1, 0.0, 1, advice);
        assert_eq!(one, samples(4, 1, 3, 0.0, 1, advice));
        assert!(!one.is_empty() && one.iter().all(|s| s.label.is_finite()));
    }

    #[test]
    fn every_morning_gives_a_finite_label() {
        let list = samples(4, 1, 2, 0.0, 2, None);
        assert!((4..=4 * 21).contains(&list.len()), "{} samples", list.len());
        assert!(list.iter().all(|s| s.label.is_finite() && s.label >= 0.0));
    }

    #[test]
    fn exploring_changes_the_samples() {
        assert_ne!(samples(4, 1, 2, 0.5, 1, None), samples(4, 1, 2, 0.0, 1, None));
    }

    #[test]
    fn samples_file_round_trips() {
        let list = samples(3, 1, 1, 0.2, 1, None);
        let mut bytes = Vec::new();
        write_samples(&list, &mut bytes).unwrap();
        assert_eq!(read_samples(&bytes).unwrap(), list);
    }

    #[test]
    fn bad_samples_files_are_errors() {
        let mut bytes = Vec::new();
        write_samples(&samples(1, 1, 1, 0.0, 1, None), &mut bytes).unwrap();
        assert!(read_samples(&bytes[..bytes.len() - 1]).is_err());
        assert!(read_samples(&bytes[..10]).is_err());
        bytes[0] = b'X';
        assert!(read_samples(&bytes).is_err());
    }
}
