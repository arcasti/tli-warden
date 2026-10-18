//! The held Torture Instruments, read from the icons in the instrument panel
//! at the top left of the board. Each slot's crop is matched against the
//! game's 31 instrument icons (assets/s16-instruments.bin, built by
//! `cargo run --release --example s16_instruments -- <NoAtlas dir>`).

use super::panels::luma_stats;
use super::{Rect, Rgba};
use crate::engine::data::{Tool, data};
use std::sync::OnceLock;

/// Slot centres at 1918x1078, in pick order; the game shows at most 2 icons.
const SLOTS: [(i32, i32); 2] = [(122, 391), (211, 391)];
/// The icon is drawn at 89x89 px around the slot centre.
const ICON_PX: i32 = 89;
/// Side of the grid both the icons and the frame crops are averaged down to.
pub const GRID: usize = 45;
/// Luma std over the 44x44 box below which a slot is empty: 4.4 measured on
/// an empty slot, >= 20 on filled ones.
const EMPTY_SPREAD: f32 = 12.0;
/// Least score of a named icon: the right icon scores 0.73-0.96.
const MIN_SCORE: f32 = 0.6;
/// Least lead over the runner-up: the best wrong icon scores <= 0.39.
const MIN_LEAD: f32 = 0.25;

const MAGIC: &[u8; 8] = b"S16INS01";

/// The instrument icons in data order, each GRID x GRID RGBA.
pub struct Icons {
    icons: Vec<Vec<u8>>,
}

impl Icons {
    pub fn to_bytes(icons: &[Vec<u8>]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.push(GRID as u8);
        out.push(icons.len() as u8);
        for icon in icons {
            out.extend_from_slice(icon);
        }
        out
    }

    fn from_bytes(b: &[u8]) -> Result<Icons, String> {
        if b.len() < 10 || &b[..8] != MAGIC {
            return Err("not an S16INS01 file".into());
        }
        let (side, count) = (b[8] as usize, b[9] as usize);
        if side != GRID {
            return Err(format!("grid side {side}, expected {GRID}"));
        }
        if count != data().tool_ids.len() {
            return Err(format!("{count} icons, expected {}", data().tool_ids.len()));
        }
        let size = GRID * GRID * 4;
        if b.len() != 10 + count * size {
            return Err(format!("{} bytes, expected {}", b.len(), 10 + count * size));
        }
        Ok(Icons { icons: b[10..].chunks(size).map(|c| c.to_vec()).collect() })
    }

    /// The icons exported from the game and embedded at compile time.
    pub fn embedded() -> &'static Icons {
        static I: OnceLock<Icons> = OnceLock::new();
        I.get_or_init(|| {
            Icons::from_bytes(include_bytes!("../../assets/s16-instruments.bin"))
                .expect("assets/s16-instruments.bin: run `cargo run --release --example s16_instruments -- <NoAtlas dir>`")
        })
    }
}

/// Area-averages a w x h RGBA image to side x side: an exact box filter with
/// fractional pixel coverage, colour weighted by alpha, then un-premultiplied.
pub fn box_downscale(src: &[u8], w: usize, h: usize, side: usize) -> Vec<u8> {
    // Per output index, the source pixels it covers with their coverage.
    let spans = |n: usize| -> Vec<Vec<(usize, f64)>> {
        let s = n as f64 / side as f64;
        (0..side)
            .map(|i| {
                let (a, b) = (i as f64 * s, (i + 1) as f64 * s);
                (a.floor() as usize..(b.ceil() as usize).min(n))
                    .map(|p| (p, b.min(p as f64 + 1.0) - a.max(p as f64)))
                    .collect()
            })
            .collect()
    };
    let (xs, ys) = (spans(w), spans(h));
    let mut out = Vec::with_capacity(side * side * 4);
    for yspan in &ys {
        for xspan in &xs {
            let (mut acc, mut alpha, mut area) = ([0.0f64; 3], 0.0f64, 0.0f64);
            for &(y, cy) in yspan {
                for &(x, cx) in xspan {
                    let i = (y * w + x) * 4;
                    let (c, a) = (cx * cy, src[i + 3] as f64 / 255.0);
                    for k in 0..3 {
                        acc[k] += c * a * src[i + k] as f64;
                    }
                    alpha += c * a;
                    area += c;
                }
            }
            for v in acc {
                out.push(if alpha > 0.0 { (v / alpha).round().clamp(0.0, 255.0) as u8 } else { 0 });
            }
            out.push((alpha / area * 255.0).round().clamp(0.0, 255.0) as u8);
        }
    }
    out
}

/// What one panel slot shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Slot {
    Empty,
    Named(Tool),
    Unsure,
}

/// Pearson correlation between the icon's opaque pixels and the same pixels
/// of the crop, over their RGB values taken together.
fn score(icon: &[u8], crop: &[u8]) -> f32 {
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for i in (0..icon.len()).step_by(4) {
        if icon[i + 3] >= 128 {
            a.extend(icon[i..i + 3].iter().map(|&v| v as f64));
            b.extend(crop[i..i + 3].iter().map(|&v| v as f64));
        }
    }
    let standardize = |v: &mut Vec<f64>| {
        let n = v.len() as f64;
        let mean = v.iter().sum::<f64>() / n;
        let std = (v.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n).sqrt().max(1e-9);
        v.iter_mut().for_each(|x| *x = (*x - mean) / std);
    };
    if a.is_empty() {
        return 0.0;
    }
    standardize(&mut a);
    standardize(&mut b);
    (a.iter().zip(&b).map(|(x, y)| x * y).sum::<f64>() / a.len() as f64) as f32
}

/// The best and runner-up icon scores of a slot, as (data index, score).
fn ranked(img: &Rgba, (cx, cy): (i32, i32)) -> [(usize, f32); 2] {
    let half = ICON_PX / 2;
    let crop = img.crop(Rect::new(cx - half, cy - half, ICON_PX, ICON_PX));
    let small = box_downscale(&crop.data, crop.w, crop.h, GRID);
    let mut s: Vec<(usize, f32)> = Icons::embedded().icons.iter().map(|icon| score(icon, &small)).enumerate().collect();
    s.sort_by(|x, y| y.1.total_cmp(&x.1));
    [s[0], s[1]]
}

fn slot(img: &Rgba, at: (i32, i32)) -> Slot {
    if luma_stats(img, at, 22).1 < EMPTY_SPREAD {
        return Slot::Empty;
    }
    let [best, second] = ranked(img, at);
    if best.1 >= MIN_SCORE && best.1 - second.1 >= MIN_LEAD {
        Slot::Named(data().tool_ids[best.0])
    } else {
        Slot::Unsure
    }
}

/// The instruments the panel icons name, in pick order; None when an icon
/// does not match one instrument clearly.
pub fn read_held(img: &Rgba) -> Option<Vec<Tool>> {
    match (slot(img, SLOTS[0]), slot(img, SLOTS[1])) {
        (Slot::Empty, _) => Some(vec![]),
        (Slot::Named(a), Slot::Empty) => Some(vec![a]),
        (Slot::Named(a), Slot::Named(b)) => Some(vec![a, b]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::load_png;
    use serde_json::Value;
    use std::path::Path;

    fn labels(v: &Value) -> Vec<Tool> {
        v.as_array().unwrap().iter().map(|k| Tool::from_key(k.as_str().unwrap()).unwrap()).collect()
    }

    /// The frames whose read differs from the labels, with each slot's
    /// spread, best and runner-up scores.
    fn misses(dir: &Path, truth: &[(String, Vec<Tool>)]) -> Vec<String> {
        let d = data();
        let mut bad = Vec::new();
        for (name, want) in truth {
            let img = load_png(&dir.join(name)).unwrap();
            let got = read_held(&img);
            if got.as_ref() != Some(want) {
                let detail: Vec<String> = SLOTS
                    .iter()
                    .map(|&at| {
                        let [b, s] = ranked(&img, at);
                        let spread = luma_stats(&img, at, 22).1;
                        format!("spread {spread:.1} best {} {:.2} second {} {:.2}", d.tool_ids[b.0].key(), b.1, d.tool_ids[s.0].key(), s.1)
                    })
                    .collect();
                bad.push(format!("{name}: got {got:?}, want {want:?} ({})", detail.join(" | ")));
            }
        }
        bad
    }

    #[test]
    fn held_frames_read() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = root.join("trainingdata/s16/held");
        if !dir.exists() {
            eprintln!("skipped: {} is missing", dir.display());
            return;
        }
        let v: Value = serde_json::from_str(&std::fs::read_to_string(root.join("data/held-truth-s16.json")).unwrap()).unwrap();
        let truth: Vec<(String, Vec<Tool>)> =
            v.as_object().unwrap().iter().filter(|(k, _)| k.ends_with(".png")).map(|(k, t)| (k.clone(), labels(t))).collect();
        let bad = misses(&dir, &truth);
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn labeled_board_frames_read() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = root.join("trainingdata/s16");
        if !dir.exists() {
            eprintln!("skipped: {} is missing", dir.display());
            return;
        }
        let v: Value = serde_json::from_str(&std::fs::read_to_string(root.join("data/scan-truth-s16.json")).unwrap()).unwrap();
        let truth: Vec<(String, Vec<Tool>)> = v
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, f)| {
                k.ends_with(".png") && f["tools"].is_array() && matches!(f["screen"].as_str(), Some("board" | "cellPick"))
            })
            .map(|(k, f)| (k.clone(), labels(&f["tools"])))
            .collect();
        assert!(!truth.is_empty());
        let bad = misses(&dir, &truth);
        assert!(bad.is_empty(), "{bad:#?}");
    }
}
