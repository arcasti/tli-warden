//! The board scene of the S16 prison screen: per cell the padlock (with the
//! rarity glow of a "?" portrait behind it), the inmate
//! (beast and rarity tier), the Prison Incident "!", the jaws crown, and the
//! warden's gold arrow. Coordinates are pixels of the 1918x1078 reference
//! frame (docs/s16-screen.md). Templates and colour centroids are built from
//! labeled frames by [`build_templates`] and embedded from
//! `assets/s16-templates.bin` (`cargo run --release --example s16_templates`).

use super::Rgba;
use crate::engine::data::Beast;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Cell 1 is the Interrogation Room; cells 2-16 hold inmates.
pub const CELLS: std::ops::RangeInclusive<u8> = 2..=16;

/// Portrait centres of cells 1-16, clockwise from the Interrogation Room.
const PORTRAIT: [(i32, i32); 16] = [
    (520, 140), (698, 140), (871, 140), (1045, 140), (1220, 140), (1395, 140),
    (1400, 270), (1405, 420),
    (1416, 560), (1233, 560), (1052, 560), (869, 560), (686, 560), (504, 560),
    (512, 420), (515, 280),
];

/// Centres of the warden's arrow for cells 1-16 (measured on the labeled
/// frames for 1, 5-8, 10, 12, 14-16; the rest follow their row).
const ARROW: [(i32, i32); 16] = [
    (614, 219), (698, 228), (871, 228), (1045, 228), (1217, 228), (1290, 225),
    (1290, 276), (1293, 416),
    (1293, 470), (1230, 466), (1052, 466), (866, 466), (686, 466), (601, 478),
    (610, 419), (615, 278),
];

/// Where the padlock sits for cells 2-16 (index = cell - 1).
fn padlock_spot(cell: u8) -> (i32, i32) {
    let (x, y) = PORTRAIT[cell as usize - 1];
    match cell {
        2..=6 => (x, 195),
        7 => (x, y + 58),
        8 => (x + 2, y + 50),
        9..=14 => (x, 620),
        _ => (x, y + 52),
    }
}

/// The Prison Incident "!" at the cell's bottom right: measured at cells 6,
/// 9, 13 and 14, and at cell 8, where the right column's frame puts it
/// further out and higher (cell 7 is assumed to match cell 8; one frame,
/// trainingdata/reports/incident-cell8.png).
fn event_spot(cell: u8) -> (i32, i32) {
    let (x, y) = PORTRAIT[cell as usize - 1];
    match cell {
        // Cell 7's marker sits where the other cells' do (measured on a
        // recorded frame); cell 8's has not been seen on a frame yet.
        8 => (x + 80, y + 25),
        _ => (x + 71, y + 33),
    }
}

fn portrait(cell: u8) -> (i32, i32) {
    PORTRAIT[cell as usize - 1]
}

// ---------- Features ----------

/// Box-averaged RGB patch of `bw` x `bh` blocks of `block` pixels centred on (cx, cy).
fn patch(img: &Rgba, cx: i32, cy: i32, bw: i32, bh: i32, block: i32) -> Vec<f32> {
    let (x0, y0) = (cx - bw * block / 2, cy - bh * block / 2);
    let mut out = Vec::with_capacity((bw * bh * 3) as usize);
    for by in 0..bh {
        for bx in 0..bw {
            let mut s = [0f32; 3];
            for dy in 0..block {
                for dx in 0..block {
                    let p = img.rgb(x0 + bx * block + dx, y0 + by * block + dy);
                    for c in 0..3 {
                        s[c] += p[c] as f32;
                    }
                }
            }
            let n = (block * block) as f32;
            out.extend(s.iter().map(|v| v / n));
        }
    }
    out
}

/// Per-channel mean removal and scaling, then unit length, so a dot product
/// is a normalized cross-correlation that ignores a colour tint.
fn standardize(v: &mut [f32]) {
    for c in 0..3 {
        let vals: Vec<f32> = v.iter().skip(c).step_by(3).copied().collect();
        let n = vals.len() as f32;
        let mean = vals.iter().sum::<f32>() / n;
        let sd = (vals.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n).sqrt().max(4.0);
        for x in v.iter_mut().skip(c).step_by(3) {
            *x = (*x - mean) / sd;
        }
    }
    normalize(v);
}

/// Grey patch with the mean removed, at unit length.
fn luma(v: &[f32]) -> Vec<f32> {
    let mut g: Vec<f32> = v.as_chunks::<3>().0.iter().map(|p| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]).collect();
    let mean = g.iter().sum::<f32>() / g.len() as f32;
    g.iter_mut().for_each(|x| *x -= mean);
    normalize(&mut g);
    g
}

fn normalize(v: &mut [f32]) {
    let len = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if len > 1e-6 {
        v.iter_mut().for_each(|x| *x /= len);
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The face inside a portrait frame centred at `(x, y)`, below the jaws crown.
fn face(img: &Rgba, x: i32, y: i32) -> Vec<f32> {
    let mut v = patch(img, x, y + 12, 13, 13, 4);
    standardize(&mut v);
    v
}

fn beast_feature(img: &Rgba, cell: u8) -> Vec<f32> {
    let (x, y) = portrait(cell);
    face(img, x, y)
}

/// The faces at the spots within +-`r` px of `(x, y)`, 2 px apart.
fn faces_around(img: &Rgba, x: i32, y: i32, r: i32) -> Vec<Vec<f32>> {
    let mut out = Vec::new();
    for dy in (-r..=r).step_by(2) {
        for dx in (-r..=r).step_by(2) {
            out.push(face(img, x + dx, y + dy));
        }
    }
    out
}

fn padlock_feature(img: &Rgba, x: i32, y: i32) -> Vec<f32> {
    luma(&patch(img, x, y, 14, 14, 2))
}

fn event_feature(img: &Rgba, x: i32, y: i32) -> Vec<f32> {
    let mut v = patch(img, x, y, 10, 18, 2);
    standardize(&mut v);
    v
}

/// Best template score within +-4 px of the spot.
fn search(img: &Rgba, (x, y): (i32, i32), tpl: &[f32], feature: fn(&Rgba, i32, i32) -> Vec<f32>) -> f32 {
    let mut best = f32::MIN;
    for dy in (-4..=4).step_by(2) {
        for dx in (-4..=4).step_by(2) {
            best = best.max(dot(&feature(img, x + dx, y + dy), tpl));
        }
    }
    best
}

/// Mean RGB over the given pixels.
fn mean_rgb(img: &Rgba, pts: impl Iterator<Item = (i32, i32)>) -> [f32; 3] {
    let (mut m, mut n) = ([0f32; 3], 0f32);
    for (x, y) in pts {
        let p = img.rgb(x, y);
        for c in 0..3 {
            m[c] += p[c] as f32;
        }
        n += 1.0;
    }
    m.map(|v| v / n.max(1.0))
}

/// Glow colour relative to the local light. The rarity glow hugs the portrait
/// frame (32-42 px from the centre at the sides, 24-38 px above it, the jaws
/// crown left out); the wall 54-64 px out at the same height sees the same
/// lantern, red target tint or day-banner dimming. Their per-channel ratio
/// cancels such a tint: its chromaticity gives the hue (white, blue, purple,
/// amber, red) and its mean how much the glow stands out.
///
/// A cell lit as a reachable destination shows pale stone that adds a
/// roughly grey light to the dark stone, but not evenly: on cell 14 of
/// jaws-orange-cell14 it adds about 27 under the ring on both sides, while
/// the wall sample catches a brown wooden bar on the left and a bluish
/// [69, 74, 79] on the right (40-50 over the far stone). Subtracting that
/// wall took the green out of an orange glow and read it as red. A lit cell
/// is measured against the dark far stone plus a grey offset instead: the
/// wall's least channel lift, capped per segment by the segment's least
/// channel excess (an orange or red glow adds almost no blue, a blue glow
/// almost no red).
fn tier_feature(img: &Rgba, cell: u8, lantern: bool) -> (Vec<f32>, Vec<Vec<f32>>) {
    let (cx, cy) = portrait(cell);
    let side = |y0: i32, y1: i32, d0: i32, d1: i32, s: i32| -> Vec<(i32, i32)> {
        (y0..y1).flat_map(|y| (d0..d1).map(move |d| (cx + s * d, y))).collect()
    };
    let wall = median_rgb(img, side(cy - 20, cy + 40, 54, 64, -1).into_iter().chain(side(cy - 20, cy + 40, 54, 64, 1)));
    let top = |x0: i32| -> Vec<(i32, i32)> { (cy - 38..cy - 24).flat_map(|y| (x0..x0 + 20).map(move |x| (x, y))).collect() };
    let segments = [
        side(cy - 20, cy + 10, 32, 42, -1),
        side(cy + 10, cy + 40, 32, 42, -1),
        side(cy - 20, cy + 10, 32, 42, 1),
        side(cy + 10, cy + 40, 32, 42, 1),
        top(cx - 40),
        top(cx + 20),
    ];
    let far = median_rgb(img, side(cy - 20, cy + 40, 70, 90, -1).into_iter().chain(side(cy - 20, cy + 40, 70, 90, 1)));
    let lift = (0..3).map(|c| wall[c] - far[c]).fold(f32::MAX, f32::min);
    let lit = !lantern && lift > LIT_LIFT;
    let grey = (wall[0] + wall[1] + wall[2]) / 3.0;
    let scale = if lantern {
        [grey; 3]
    } else if lit {
        far
    } else {
        wall
    };
    let excess: Vec<[f32; 3]> = segments
        .into_iter()
        .map(|s| {
            let g = mean_rgb(img, s.into_iter());
            let base = if lit {
                let o = (0..3).map(|c| g[c] - far[c]).fold(lift, f32::min).max(0.0);
                far.map(|f| f + o)
            } else {
                wall
            };
            [0, 1, 2].map(|c| (g[c] - base[c]).max(0.0) / scale[c].max(4.0))
        })
        .collect();
    let e = [0, 1, 2].map(|c| median(excess.iter().map(|x| x[c]).collect()));
    (glow_point(e), excess.into_iter().map(glow_point).collect())
}

/// Least lift of the wall over the far stone (70-90 px out, outside the
/// cell) that marks a cell lit as a reachable destination. Unlit cells of the
/// labeled frames stay below 5, lit ones measure 13 to 50.
const LIT_LIFT: f32 = 10.0;

/// Chromaticity of a glow excess (pulled to grey when faint) and its strength.
fn glow_point(e: [f32; 3]) -> Vec<f32> {
    let sum = e[0] + e[1] + e[2];
    let k = 3.0 / (sum + 0.3);
    vec![k * e[0], k * e[1], k * e[2], sum / 12.0]
}

fn median(mut v: Vec<f32>) -> f32 {
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Per-channel median RGB over the given pixels.
fn median_rgb(img: &Rgba, pts: impl Iterator<Item = (i32, i32)>) -> [f32; 3] {
    let mut ch: [Vec<f32>; 3] = Default::default();
    for (x, y) in pts {
        let p = img.rgb(x, y);
        (0..3).for_each(|c| ch[c].push(p[c] as f32));
    }
    ch.map(median)
}

/// Gold arrow pixels around each of the 16 warden spots.
fn arrow_scores(img: &Rgba) -> [u32; 16] {
    let mut out = [0u32; 16];
    for (i, &(ax, ay)) in ARROW.iter().enumerate() {
        let mut n = 0;
        for y in ay - 22..ay + 22 {
            for x in ax - 22..ax + 22 {
                let [r, g, b] = img.rgb(x, y).map(|v| v as i32);
                if r > 60 && r >= g && g > b && (r - b) * 2 > r && g * 20 > r * 11 {
                    n += 1;
                }
            }
        }
        out[i] = n;
    }
    out
}

/// Red pixels of the small jaws crown above the portrait.
fn crown_score(img: &Rgba, cell: u8) -> f32 {
    let (cx, cy) = portrait(cell);
    let mut n = 0;
    for y in cy - 40..cy - 15 {
        for x in cx - 20..cx + 20 {
            let [r, g, b] = img.rgb(x, y).map(|v| v as i32);
            if r > 140 && g * 2 < r && b * 2 < r + 40 {
                n += 1;
            }
        }
    }
    n as f32 / 1000.0
}

/// Share of saturated red pixels that marks the big jaws of a Consume target
/// pick on a cell: the score is the smallest share of the three bands the
/// jaws fill (left and right of the portrait, under it). Measured over 81
/// trainingdata frames and 44 run-log frames: the cells of every other screen,
/// and the cells a target pick does not offer, score 0.000 (an Ultimate's red
/// glow included); the offered targets score 0.086 to 0.317.
const TARGET_MIN: f32 = 0.04;

/// Whether the big jaws surround a cell's portrait: see [`TARGET_MIN`].
fn has_target_jaws(img: &Rgba, cell: u8) -> bool {
    let (cx, cy) = portrait(cell);
    let red = |r: i32, g: i32, b: i32| r > 150 && g * 2 < r && b * 2 < r;
    let bands = [
        ((cx - 40, cx + 40), (cy + 50, cy + 80)),
        ((cx - 58, cx - 38), (cy - 30, cy + 50)),
        ((cx + 38, cx + 58), (cy - 30, cy + 50)),
    ];
    bands.into_iter().all(|(xs, ys)| colour_share(img, xs, ys, red) >= TARGET_MIN)
}

/// Share of the pixels in a box whose colour passes `pick`.
fn colour_share(img: &Rgba, (x0, x1): (i32, i32), (y0, y1): (i32, i32), pick: impl Fn(i32, i32, i32) -> bool) -> f32 {
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let [r, g, b] = img.rgb(x, y).map(|v| v as i32);
            if pick(r, g, b) {
                n += 1;
            }
        }
    }
    n as f32 / ((x1 - x0) * (y1 - y0)) as f32
}

/// Share of cream pixels that marks the Tusk of Iron Tongs of Lust: Mammoth,
/// a curved tusk drawn over the bottom of the portrait. Measured on a 62-frame
/// run: 150 marked Mammoth cells score 0.063 to 0.094, unmarked Mammoths
/// 0.000, every other beast and empty cell at most 0.013 (one frame in the
/// middle of a chain animation aside); the cells of the 32 labeled
/// trainingdata/s16 frames at most 0.010. The upper bounds on r and r - b
/// leave out the warden's gold arrow, brighter and more saturated than the
/// Tusk, which scored 0.040 where it crossed a Rat's box.
const TUSK_MIN: f32 = 0.03;

/// Whether the Tusk mark is on a cell: see [`TUSK_MIN`].
fn has_tusk(img: &Rgba, cell: u8) -> bool {
    let (cx, cy) = portrait(cell);
    let cream = |r: i32, g: i32, b: i32| r > 170 && r < 248 && g > 140 && r - b > 45 && r - b <= 100 && r - g < 45;
    colour_share(img, (cx - 20, cx + 25), (cy + 22, cy + 50), cream) >= TUSK_MIN
}

/// Half the width and height of the box around a portrait that a tooltip
/// panel is looked for in.
const TIP_BOX: (i32, i32) = (70, 45);
/// Sample step in that box, both ways.
const TIP_STEP: usize = 3;
/// Fewest pixels in a horizontal run of panel colour: the tooltip panel is a
/// flat fill, the board's dark stone breaks such runs within a few pixels.
const TIP_RUN: i32 = 40;
/// Share of the box in such runs that marks the cell covered. Measured on the
/// Rusty Saw tooltip (rec3-t161): 0.68 and 0.94 on the cells under it, 0.44 on
/// cell 14 it only clips (read right); on the Agonizing Muzzle tooltip
/// (rec4-t412) 0.61 and 0.79 on cells 15 and 16 and 0.59 on cell 14, whose
/// portrait it hides down to the padlock; boards without a tooltip stay at or
/// below 0.30.
const TIP_SHARE: f32 = 0.5;

/// The tooltip panel colour: a dark grey (22-48 green), red within 2 of green
/// and blue 0-4 above it. A recorded frame's compression tints the flat panel
/// in blocks, some with blue 3 above green, some with blue equal to green and
/// red 2 below (rec4-t412). The board's stone is warm, red above blue.
fn tip_colour(img: &Rgba, x: i32, y: i32) -> bool {
    let [r, g, b] = img.rgb(x, y).map(i32::from);
    (22..=48).contains(&g) && (-2..=2).contains(&(r - g)) && (0..=4).contains(&(b - g))
}

/// Whether a tooltip panel (an instrument's name and text drawn when the
/// mouse rests on its icon) covers the cell: the share of the box around the
/// portrait in long horizontal runs of panel colour.
fn tooltip_covers(img: &Rgba, cell: u8) -> bool {
    let (cx, cy) = portrait(cell);
    let (bw, bh) = TIP_BOX;
    let x0 = cx - bw - TIP_RUN;
    let (mut hit, mut total) = (0, 0);
    for y in (cy - bh..=cy + bh).step_by(TIP_STEP) {
        // The row reaches TIP_RUN past the box so a run that crosses its edge counts.
        let panel: Vec<bool> = (x0..=cx + bw + TIP_RUN).map(|x| tip_colour(img, x, y)).collect();
        let mut long = vec![false; panel.len()];
        let mut i = 0;
        while i < panel.len() {
            let start = i;
            while i < panel.len() && panel[i] {
                i += 1;
            }
            if i - start >= TIP_RUN as usize {
                long[start..i].fill(true);
            }
            i = i.max(start + 1);
        }
        for x in (cx - bw..=cx + bw).step_by(TIP_STEP) {
            total += 1;
            hit += usize::from(long[(x - x0) as usize]);
        }
    }
    hit as f32 >= TIP_SHARE * total as f32
}

// ---------- Templates ----------

/// Beast classes: the four beasts, then an open empty cell and a closed one.
const CLASSES: [Option<Beast>; 6] = [
    Some(Beast::Mammoth), Some(Beast::Lion), Some(Beast::Wolf), Some(Beast::Rat), None, None,
];

/// A two-class score split learned from labeled samples.
#[derive(Clone, Debug, Default)]
pub struct Split {
    pub threshold: f32,
    pub scale: f32,
}

impl Split {
    fn learn(pos: &[f32], neg: &[f32]) -> Split {
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
        if pos.is_empty() || neg.is_empty() {
            return Split { threshold: 0.5, scale: 0.25 };
        }
        let lo = pos.iter().copied().fold(f32::MAX, f32::min);
        let hi = neg.iter().copied().fold(f32::MIN, f32::max);
        let threshold = if lo > hi { (lo + hi) / 2.0 } else { (mean(pos) + mean(neg)) / 2.0 };
        Split { threshold, scale: ((mean(pos) - mean(neg)) / 2.0).abs().max(1e-3) }
    }

    /// The decision and a 0-1 confidence.
    fn decide(&self, score: f32) -> (bool, f32) {
        (score > self.threshold, ((score - self.threshold).abs() / self.scale).min(1.0))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Templates {
    /// One standardized face patch per entry of `CLASSES` (empty when unseen).
    pub beast: Vec<Vec<f32>>,
    pub padlock: Vec<f32>,
    pub padlock_split: Split,
    pub event: Vec<f32>,
    pub event_split: Split,
    pub crown_split: Split,
    /// Glow centroid per tier 0-4 (empty when unseen).
    pub tier: Vec<Vec<f32>>,
}

const MAGIC: &[u8; 8] = b"S16TPL01";

impl Templates {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        let mut put = |v: &[f32]| {
            out.extend((v.len() as u32).to_le_bytes());
            v.iter().for_each(|x| out.extend(x.to_le_bytes()));
        };
        self.beast.iter().for_each(|v| put(v));
        put(&self.padlock);
        put(&[self.padlock_split.threshold, self.padlock_split.scale]);
        put(&self.event);
        put(&[self.event_split.threshold, self.event_split.scale]);
        put(&[self.crown_split.threshold, self.crown_split.scale]);
        self.tier.iter().for_each(|v| put(v));
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Templates, String> {
        let rest = bytes.strip_prefix(MAGIC.as_slice()).ok_or("not an S16 template file")?;
        let mut pos = 0;
        let mut take = || -> Result<Vec<f32>, String> {
            let word = |p: usize| rest.get(p..p + 4).map(|b| [b[0], b[1], b[2], b[3]]).ok_or("truncated template file");
            let n = u32::from_le_bytes(word(pos)?) as usize;
            pos += 4;
            let v = (0..n).map(|i| word(pos + 4 * i).map(f32::from_le_bytes)).collect::<Result<_, _>>()?;
            pos += 4 * n;
            Ok(v)
        };
        let split = |v: Vec<f32>| Split { threshold: v[0], scale: v[1] };
        let beast = (0..CLASSES.len()).map(|_| take()).collect::<Result<_, _>>()?;
        let padlock = take()?;
        let padlock_split = split(take()?);
        let event = take()?;
        let event_split = split(take()?);
        let crown_split = split(take()?);
        let tier = (0..5).map(|_| take()).collect::<Result<_, _>>()?;
        Ok(Templates { beast, padlock, padlock_split, event, event_split, crown_split, tier })
    }

    /// The templates built from the labeled frames and embedded at compile time.
    pub fn embedded() -> &'static Templates {
        static T: OnceLock<Templates> = OnceLock::new();
        T.get_or_init(|| {
            Templates::from_bytes(include_bytes!("../../assets/s16-templates.bin"))
                .expect("assets/s16-templates.bin: run `cargo run --release --example s16_templates`")
        })
    }
}

// ---------- Labels ----------

/// What the labels say about one cell.
#[derive(Clone, Debug, Default)]
pub struct CellTruth {
    /// None when the frame's padlocks were not labeled (day banner).
    pub locked: Option<bool>,
    /// Beast (None when labeled "?") and tier of the inmate.
    pub inmate: Option<(Option<Beast>, u8)>,
    /// The glow tier of a "?" portrait on a locked cell (label "scratch").
    pub scratch: Option<u8>,
    pub event: bool,
    pub jaws: bool,
}

/// One frame of data/scan-truth-s16.json.
#[derive(Clone, Debug)]
pub struct FrameTruth {
    pub name: String,
    pub screen: String,
    pub warden: Option<u8>,
    pub covered: Vec<u8>,
    /// Cells 2-16 at index cell - 2.
    pub cells: Vec<CellTruth>,
}

fn beast_named(s: &str) -> Option<Beast> {
    Beast::ALL.into_iter().find(|b| format!("{b:?}").eq_ignore_ascii_case(s))
}

impl FrameTruth {
    /// Every frame of the label file, sorted by name.
    pub fn parse_all(json: &Value) -> Vec<FrameTruth> {
        let obj = json.as_object().expect("label file is an object");
        let mut out: Vec<FrameTruth> =
            obj.iter().filter(|(k, _)| !k.starts_with('_')).map(|(k, v)| FrameTruth::parse(k, v)).collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    pub fn parse(name: &str, v: &Value) -> FrameTruth {
        let nums = |key: &str| -> Vec<u8> {
            v[key].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).map(|x| x as u8).collect()).unwrap_or_default()
        };
        let screen = v["screen"].as_str().unwrap_or("").to_string();
        let locked = nums("locked");
        let locked_known = !(screen == "dayBanner" && locked.is_empty());
        let (events, jaws) = (nums("events"), nums("jaws"));
        let mut inmates: HashMap<u8, (Option<Beast>, u8)> = HashMap::new();
        if let Some(o) = v["inmates"].as_object() {
            for (k, s) in o {
                let (b, t) = s.as_str().and_then(|s| s.split_once('/')).expect("inmate as beast/tier");
                inmates.insert(k.parse().expect("cell number"), (beast_named(b), t.parse().expect("tier")));
            }
        }
        let scratch: HashMap<u8, u8> = v["scratch"]
            .as_object()
            .map(|o| o.iter().map(|(k, t)| (k.parse().expect("cell number"), t.as_u64().expect("tier") as u8)).collect())
            .unwrap_or_default();
        let cells = CELLS
            .map(|c| CellTruth {
                locked: locked_known.then(|| locked.contains(&c)),
                inmate: inmates.get(&c).copied(),
                scratch: scratch.get(&c).copied(),
                event: events.contains(&c),
                jaws: jaws.contains(&c),
            })
            .collect();
        FrameTruth {
            name: name.to_string(),
            warden: v["warden"].as_u64().map(|w| w as u8),
            covered: nums("covered"),
            screen,
            cells,
        }
    }

    /// Whether the frame shows the board to read.
    pub fn has_board(&self) -> bool {
        matches!(self.screen.as_str(), "board" | "chainTarget" | "dayBanner")
    }

    pub fn cell(&self, cell: u8) -> &CellTruth {
        &self.cells[cell as usize - 2]
    }
}

/// The labeled frames found in `dir` (sorted by name) with their labels from
/// `labels`; frames without a board are left out.
pub fn load_labeled(dir: &std::path::Path, labels: &std::path::Path) -> Result<Vec<(Rgba, FrameTruth)>, String> {
    let text = std::fs::read_to_string(labels).map_err(|e| format!("{}: {e}", labels.display()))?;
    let json: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", labels.display()))?;
    let mut out = Vec::new();
    for t in FrameTruth::parse_all(&json).into_iter().filter(FrameTruth::has_board) {
        let path = dir.join(&t.name);
        if path.exists() {
            out.push((super::load_png(&path)?, t));
        }
    }
    Ok(out)
}

// ---------- Building ----------

fn mean_of(vs: &[Vec<f32>]) -> Vec<f32> {
    let Some(first) = vs.first() else { return Vec::new() };
    let mut m = vec![0f32; first.len()];
    for v in vs {
        m.iter_mut().zip(v).for_each(|(a, b)| *a += b / vs.len() as f32);
    }
    m
}

/// Builds the templates from labeled frames with a board, as
/// [`load_labeled`] gives them (covered cells left out).
pub fn build_templates(frames: &[(&Rgba, &FrameTruth)]) -> Templates {
    let mut beast: Vec<Vec<Vec<f32>>> = vec![Vec::new(); CLASSES.len()];
    let mut tier: Vec<Vec<Vec<f32>>> = vec![Vec::new(); 5];
    let (mut locks, mut events) = (Vec::new(), Vec::new());
    for (img, t) in frames {
        for c in CELLS.filter(|c| !t.covered.contains(c)) {
            let ct = t.cell(c);
            let class = match (ct.inmate, ct.locked) {
                (Some((Some(b), _)), _) => Some(b.index()),
                (Some((None, _)), _) => None,
                (None, Some(true)) => Some(5),
                (None, Some(false)) => Some(4),
                (None, None) => None,
            };
            if let Some(k) = class {
                beast[k].push(beast_feature(img, c));
            }
            if let Some((_, tr)) = ct.inmate
                && t.warden != Some(c)
                && t.screen == "board"
            {
                tier[tr as usize].push(tier_feature(img, c, false).0);
            }
            let (px, py) = padlock_spot(c);
            if ct.locked == Some(true) {
                locks.push(padlock_feature(img, px, py));
            }
            if ct.event {
                let (ex, ey) = event_spot(c);
                events.push(event_feature(img, ex, ey));
            }
        }
    }
    let unit = |mut v: Vec<f32>| {
        normalize(&mut v);
        v
    };
    let padlock = unit(mean_of(&locks));
    let event = unit(mean_of(&events));
    // Second pass: the score splits, with the same search the reader uses.
    let (mut lp, mut ln, mut ep, mut en, mut jp, mut jn) = (vec![], vec![], vec![], vec![], vec![], vec![]);
    for (img, t) in frames {
        for c in CELLS.filter(|c| !t.covered.contains(c)) {
            let ct = t.cell(c);
            if let Some(l) = ct.locked {
                let s = search(img, padlock_spot(c), &padlock, padlock_feature);
                if l { lp.push(s) } else { ln.push(s) }
            }
            let s = search(img, event_spot(c), &event, event_feature);
            if ct.event { ep.push(s) } else { en.push(s) }
            if ct.inmate.is_some() && t.screen == "board" {
                let s = crown_score(img, c);
                if ct.jaws { jp.push(s) } else { jn.push(s) }
            }
        }
    }
    Templates {
        beast: beast.iter().map(|v| unit(mean_of(v))).collect(),
        padlock,
        padlock_split: Split::learn(&lp, &ln),
        event,
        event_split: Split::learn(&ep, &en),
        crown_split: Split::learn(&jp, &jn),
        tier: tier.iter().map(|v| mean_of(v)).collect(),
    }
}

// ---------- Reading ----------

/// One cell as read, each field with a 0-1 confidence.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellRead {
    pub locked: bool,
    pub locked_conf: f32,
    /// The rarity glow around the "?" portrait of a locked cell (a Scratch
    /// mark): the tier of the inmate behind the door.
    pub glow: Option<u8>,
    pub glow_conf: f32,
    /// Beast and rarity tier (0 Normal to 4 Ultimate).
    pub inmate: Option<(Beast, u8)>,
    pub beast_conf: f32,
    pub tier_conf: f32,
    /// The orange Prison Incident "!".
    pub event: bool,
    pub event_conf: f32,
    /// The small red crown: the inmate has something to consume. Weak on a
    /// red-tinted target pick and next to a red (Ultimate) glow.
    pub jaws: bool,
    pub jaws_conf: f32,
    /// The big red jaws a Consume target pick draws around each inmate the
    /// eater can consume.
    pub target: bool,
    /// The Tusk of Iron Tongs of Lust: Mammoth. Read on any inmate, since
    /// whether a Replace keeps the mark on the cell is not known.
    pub tusk: bool,
    /// A tooltip panel covers the cell, so the reads above are of the panel.
    pub covered: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    /// Cells 2-16 at index cell - 2.
    pub cells: Vec<CellRead>,
    /// The cell (1-16) the warden's arrow points at.
    pub warden: Option<u8>,
    pub warden_conf: f32,
}

impl Scene {
    pub fn cell(&self, cell: u8) -> &CellRead {
        &self.cells[cell as usize - 2]
    }
}

/// The nearest beast class of the best of the face features (one per spot)
/// and the margin to the next one.
fn beast_class(fs: &[Vec<f32>], t: &Templates) -> (usize, f32) {
    let mut scores: Vec<(f32, usize)> = t
        .beast
        .iter()
        .enumerate()
        .filter(|(_, tpl)| !tpl.is_empty())
        .map(|(k, tpl)| (fs.iter().map(|f| dot(f, tpl)).fold(f32::MIN, f32::max), k))
        .collect();
    scores.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (top, k) = scores[0];
    let next = scores.get(1).map_or(0.0, |s| s.0);
    (k, ((top - next) / 0.2).min(1.0))
}

/// How far from its spot a vault portrait's face is searched. The slots sit
/// at the frame's right edge, where a frame scaled from another size moves
/// them most: at 2560x1440 a lion's face sat 8 px above the spot, and in a
/// 1895x1058 screenshot 12 px right of it. At the spot itself the vault's
/// flame then scores the lion under the mammoth.
const VAULT_SEARCH: i32 = 12;

/// The beast of a portrait centred at `(x, y)` outside the board (a
/// Chromatic Prisoner slot), with the embedded templates; None when the
/// nearest class is an empty or closed cell.
pub fn beast_at(img: &Rgba, (x, y): (i32, i32)) -> (Option<Beast>, f32) {
    let (k, conf) = beast_class(&faces_around(img, x, y, VAULT_SEARCH), Templates::embedded());
    (CLASSES[k], conf)
}

/// Reads the board with the embedded templates.
pub fn read_scene(img: &Rgba) -> Scene {
    read_scene_with(img, Templates::embedded())
}

/// Nearest tier centroid and its distance margin to the next one.
fn nearest_tier(g: &[f32], t: &Templates) -> (u8, f32) {
    let mut d: Vec<(f32, u8)> = t
        .tier
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.is_empty())
        .map(|(i, m)| (m.iter().zip(g).map(|(a, b)| (a - b) * (a - b)).sum::<f32>(), i as u8))
        .collect();
    d.sort_by(|a, b| a.0.total_cmp(&b.0));
    let tier = d.first().map_or(0, |x| x.1);
    let margin = d.get(1).map_or(0.0, |n| (n.0 - d[0].0) / (n.0 + d[0].0).max(1e-6));
    (tier, margin)
}

/// The rarity glow around a portrait: the tier, a 0-1 confidence and the
/// glow strength. The confidence is the margin of the median glow, scaled
/// down when fewer than 4 of the 6 ring segments agree on the tier (a jaws
/// overlay or a lit frame on part of the ring). A white glow is grey in
/// every segment, so a Normal read loses its confidence when the segment
/// hues scatter (red jaws and a blue glow whose median lands on grey). The
/// strength is the weaker of the median glow and the glow above the
/// portrait: a glow rises above the frame, while the light of a Prison
/// Incident "!" next to a plain padlock brightens only the sides.
fn glow_tier(img: &Rgba, cell: u8, lantern: bool, t: &Templates) -> (u8, f32, f32) {
    let (g, segments) = tier_feature(img, cell, lantern);
    let (tier, margin) = nearest_tier(&g, t);
    let agree = segments.iter().filter(|s| nearest_tier(s, t).0 == tier).count() as f32;
    let mut conf = margin * (agree / 4.0).min(1.0);
    if tier == 0 {
        let spread = median(segments.iter().map(|s| (0..3).map(|i| (s[i] - g[i]).powi(2)).sum()).collect());
        conf *= ((0.12 - spread) / 0.06).clamp(0.0, 1.0);
    }
    let top = (segments[4][3] + segments[5][3]) / 2.0;
    (tier, conf, g[3].min(top))
}

/// Glow strength that marks a "?" portrait with a rarity glow on a locked
/// cell. Measured on locked cells of both recordings: plain padlocks and
/// unglowing "?" stay at or below 0.07, glowing "?" measure 0.12
/// (a lit cell) to 0.8.
const GLOW_MIN: f32 = 0.1;

/// Fewest gold pixels that make an arrow.
const ARROW_MIN: u32 = 40;

pub fn read_scene_with(img: &Rgba, t: &Templates) -> Scene {
    let arrows = arrow_scores(img);
    let mut order: Vec<usize> = (0..16).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(arrows[i]));
    let (best, second) = (arrows[order[0]], arrows[order[1]]);
    let warden = (best >= ARROW_MIN).then_some(order[0] as u8 + 1);
    let warden_conf = if warden.is_some() { 1.0 - second as f32 / best as f32 } else { 0.0 };

    let cells = CELLS
        .map(|c| {
            let mut r = CellRead { covered: tooltip_covers(img, c), ..CellRead::default() };
            let s = search(img, padlock_spot(c), &t.padlock, padlock_feature);
            (r.locked, r.locked_conf) = t.padlock_split.decide(s);
            let s = search(img, event_spot(c), &t.event, event_feature);
            (r.event, r.event_conf) = t.event_split.decide(s);

            // The face at the spots within +-4 px of the usual one: on a lit
            // cell with a path line through it the face has been seen 4-8 px higher.
            let (x, y) = portrait(c);
            let (k, conf) = beast_class(&faces_around(img, x, y, 4), t);
            r.beast_conf = conf;
            if let (Some(beast), false) = (CLASSES[k], r.locked) {
                let (tier, conf, _) = glow_tier(img, c, warden == Some(c), t);
                r.tier_conf = conf;
                r.inmate = Some((beast, tier));
                let (jaws, conf) = t.crown_split.decide(crown_score(img, c));
                (r.jaws, r.jaws_conf) = (jaws, conf);
                r.target = has_target_jaws(img, c);
                r.tusk = has_tusk(img, c);
            } else if r.locked {
                r.beast_conf = r.locked_conf;
                let (tier, conf, strength) = glow_tier(img, c, false, t);
                if strength >= GLOW_MIN {
                    r.glow = Some(tier);
                    r.glow_conf = conf * ((strength - GLOW_MIN) / 0.02).min(1.0);
                }
            }
            r
        })
        .collect();
    Scene { cells, warden, warden_conf }
}
