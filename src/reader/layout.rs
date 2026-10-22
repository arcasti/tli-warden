//! Turns a game frame of any width into the 1918 x 1078 reference frame the
//! reader's regions are measured on.

use super::{REF_H, REF_W, Rect, Rgba};
use std::ops::Range;
use std::sync::OnceLock;

/// Where a zone of the game's screen keeps to along one axis: the frame's
/// start (left or top), centre or end (right or bottom).
#[derive(Clone, Copy)]
enum Anchor {
    Start,
    Centre,
    End,
}

impl Anchor {
    /// The anchor's coordinate on an axis `len` long.
    fn at(self, len: f32) -> f32 {
        match self {
            Anchor::Start => 0.0,
            Anchor::Centre => len / 2.0,
            Anchor::End => len,
        }
    }
}

/// A zone of the reference frame and the anchors it scales about.
struct Zone {
    rect: Rect,
    x: Anchor,
    y: Anchor,
    /// How far the point the zone scales about sits from its anchors, in
    /// reference px: at scale u the zone's content moves by (1 - u) times it.
    at: (f32, f32),
}

const fn zone(x: i32, y: i32, w: i32, h: i32, ax: Anchor, ay: Anchor) -> Zone {
    Zone { rect: Rect::new(x, y, w, h), x: ax, y: ay, at: (0.0, 0.0) }
}

use Anchor::{Centre, End, Start};

/// The zones of the reference frame that keep to an edge of the game's
/// window; everything else (the board and its cell numbers, Select a
/// Destination/Objective, the Go to Cell and Consume/End cards, the pick
/// cards and their names) keeps to the centre. At interface scale u every
/// zone shrinks by u about its anchor; a window wider than 16:9 moves each
/// zone with its horizontal anchor. A later zone wins where two overlap.
///
/// Horizontally measured on a 1716 x 720 (21:9) clip at u = 1 against 16:9
/// frames of the same screens: the board (rec1-t087), the Select a Torture
/// Instrument cards (rec1-t031, x 250-1670) and the Warden's Treasure cards
/// (rec1-t085). Vertically measured on a 1920 x 1084 run at u = 0.645 against
/// the same screens.
///
/// - The top left (left, top): the Tartarus Prison bar, the objective panel
///   and the Torture Instrument icons. It stops at x 400 above the
///   instrument cards' tops (y 180) and below at x 260, where the second
///   icon's crop ends (the first instrument card's left 10 px, art only, go
///   with it).
/// - The top right (right, top): Speed-up, the back arrow, View Prison
///   Status, the Chromatic Prisoner heading and the vault slots (from x 1680,
///   right of the instrument cards' names). The vault zone covers the right
///   part of the fourth Warden's Treasure card, which the reader does not
///   read, and the right end of the Prison Incident popup title region.
/// - The bottom right (right, centre): Quick Action and the Consume target
///   pick's Cancel.
/// - The top centre (centre, top): the days counter.
/// - The bottom centre (centre, bottom): Confirm.
/// - The Torture Instrument icons (left, top, about a point 3.6 px left of
///   and 7.3 px above the corner): at u = 0.645 the icons sit 2 reference px left
///   and 4 above where the top-left corner puts them (f_053, f_093, f_209
///   against rec1-t087: the first icon scores 0.85 here against 0.57 at the
///   corner, the second 0.83 against 0.47).
const ZONES: [Zone; 8] = [
    zone(0, 0, 400, 260, Start, Start),
    zone(0, 260, 260, 230, Start, Start),
    zone(1540, 0, 378, 200, End, Start),
    zone(1680, 200, 238, 520, End, Start),
    zone(1600, 720, 318, 358, End, Centre),
    zone(800, 0, 320, 60, Centre, Start),
    zone(700, 915, 518, 163, Centre, End),
    Zone { at: (-3.6, -7.3), ..zone(70, 340, 190, 100, Start, Start) },
];

/// The zones the title layout adds over [`ZONES`], the first two kept to the
/// top centre, while the board's top rows under them keep to the centre: a strip
/// over the panels reader's TITLE and RIOT regions (y 70-175) with the pick
/// titles and the riot title, and one over its INCIDENT region with the
/// Prison Incident popup's title (the popup's title, at the smallest
/// setting, sits at y 149 of 1084 where centre-anchoring would put it above
/// the frame: f_049, Sewer Ratway). A third, kept to the bottom centre as
/// Confirm is, covers the panels reader's REROLLS region: "Remaining attempts
/// N" over the instrument pick's Re-roll button (x 693-892, y 893-915 on
/// reroll-2000x837). In the board layout that strip keeps to the centre,
/// where the destination cards' text reaches down to y 906 (rec3-t058).
const TITLE_ZONES: [Zone; 3] =
    [zone(400, 70, 1140, 105, Centre, Start), zone(1100, 190, 700, 70, Centre, Start), zone(640, 880, 320, 45, Centre, End)];

/// Whether a `w` x `h` frame is 16:9 within 1%.
pub fn is_16_9(w: usize, h: usize) -> bool {
    (w * 9).abs_diff(h * 16) * 100 <= h * 16
}

/// One axis of a zone: per reference pixel of the zone's span, the two
/// source pixels it samples and the second's weight in 256ths; None outside
/// the source.
type Taps = Vec<Option<(usize, usize, u32)>>;

/// The taps of reference pixels `span` on an axis `len` long, the zone's
/// anchor `a` moved by `off` reference px, sampled from a source axis `src`
/// px long at `k` source px per reference px and scale `u`.
fn taps(span: Range<usize>, len: usize, a: Anchor, off: f32, u: f32, k: f32, src: usize) -> Taps {
    let (at_ref, at_src) = (a.at(len as f32) + off, a.at(src as f32 / k) + off);
    span.map(|i| {
        let c = (at_src + (i as f32 + 0.5 - at_ref) * u) * k - 0.5;
        (c > -1.0 && c < src as f32).then(|| {
            let c = c.clamp(0.0, (src - 1) as f32);
            let c0 = c as usize;
            (c0, (c0 + 1).min(src - 1), ((c - c0 as f32) * 256.0).round() as u32)
        })
    })
    .collect()
}

/// Paints zone `z` of `out`, a reference-size image, from `frame` at
/// interface scale `u`.
fn paint(out: &mut [u8], frame: &Rgba, u: f32, z: &Zone) {
    let k = frame.h as f32 / REF_H as f32;
    let r = z.rect;
    let (x0, x1, y0, y1) = (r.x as usize, (r.x + r.w) as usize, r.y as usize, (r.y + r.h) as usize);
    let cols = taps(x0..x1, REF_W, z.x, z.at.0, u, k, frame.w);
    let rows = taps(y0..y1, REF_H, z.y, z.at.1, u, k, frame.h);
    let cols: Vec<Option<(usize, usize, u32)>> = cols.into_iter().map(|c| c.map(|(a, b, w)| (a * 4, b * 4, w))).collect();
    let row_len = frame.w * 4;
    for (y, row) in (y0..y1).zip(rows) {
        let px = out[(y * REF_W + x0) * 4..(y * REF_W + x1) * 4].as_chunks_mut::<4>().0;
        let Some((r0, r1, wy)) = row else {
            px.fill([0, 0, 0, 255]);
            continue;
        };
        let (top, bot) = (&frame.data[r0 * row_len..][..row_len], &frame.data[r1 * row_len..][..row_len]);
        for (p, c) in px.iter_mut().zip(&cols) {
            let Some((c0, c1, wx)) = *c else {
                *p = [0, 0, 0, 255];
                continue;
            };
            for i in 0..4 {
                let t = top[c0 + i] as u32 * (256 - wx) + top[c1 + i] as u32 * wx;
                let b = bot[c0 + i] as u32 * (256 - wx) + bot[c1 + i] as u32 * wx;
                p[i] = ((t * (256 - wy) + b * wy + (1 << 15)) >> 16) as u8;
            }
        }
    }
}

/// `frame` at the reference size in the board layout at interface scale `u`
/// (see [`ui_scale`]). A 16:9 frame (within 1%) at u = 1 is resized. Any
/// other frame is laid out by [`ZONES`]: a reference pixel (x, y) in a zone
/// anchored at (ax, ay) on the reference frame and at (ax', ay') on the
/// frame scaled to the reference height takes the pixel at
/// (ax' + (x - ax) u, ay' + (y - ay) u) there, sampled bilinearly from
/// `frame` itself; pixels outside every zone keep to the centre. Pixels past
/// the frame's edges are black.
fn lay_out(frame: &Rgba, u: f32) -> Rgba {
    if u == 1.0 && is_16_9(frame.w, frame.h) {
        return frame.resize(REF_W, REF_H);
    }
    let mut data = vec![0u8; REF_W * REF_H * 4];
    paint(&mut data, frame, u, &zone(0, 0, REF_W as i32, REF_H as i32, Centre, Centre));
    for z in &ZONES {
        paint(&mut data, frame, u, z);
    }
    Rgba { w: REF_W, h: REF_H, data }
}

/// The title layout of `frame` at interface scale `u`: `board`, its board
/// layout, with the [`TITLE_ZONES`] laid out over it.
fn lay_out_title(frame: &Rgba, u: f32, board: &Rgba) -> Rgba {
    let mut title = board.clone();
    for z in &TITLE_ZONES {
        paint(&mut title.data, frame, u, z);
    }
    title
}

/// `frame` in the board layout and, when it differs (u != 1), the title
/// layout, with the interface scale they were laid out at: measured on the
/// frame (see [`ui_scale`], searching near `hint` first), else `hint`, else 1.
pub fn lay_out_scaled(frame: &Rgba, hint: Option<f32>) -> (Rgba, Option<Rgba>, f32) {
    let u = ui_scale(frame, hint).or(hint).unwrap_or(1.0);
    let board = lay_out(frame, u);
    let title = (u != 1.0).then(|| lay_out_title(frame, u, &board));
    (board, title, u)
}

/// Where `assets/s16-back-arrow.png` was cut from the reference frame
/// rec1-t087: the back arrow at the top right with a few px of background.
const ARROW_AT: Rect = Rect::new(1812, 15, 68, 43);

/// Where `assets/s16-gate.png` was cut from rec1-t087: the Tartarus Prison
/// gate icon at the top left with a few px of background.
const GATE_AT: Rect = Rect::new(54, 8, 68, 60);

/// The lowest normalised cross-correlation that counts as either mark: true
/// matches score 0.54 and up unless a popup dims the gate icon to 0.3 or
/// less; misses score up to 0.31 (a run1414 frame whose right side is
/// shifted against its left).
const MIN_SCORE: f32 = 0.45;

/// How far the two marks may disagree in y, in frame px.
const MAX_DY: f32 = 3.0;

/// How far a mark may sit from where the scale puts it, in frame px.
const SLACK_X: f32 = 12.0;
const SLACK_Y: f32 = 8.0;

/// A template's grey levels, row by row.
fn grey(png: &[u8], at: Rect) -> Vec<f32> {
    let img = super::decode_png_raw(png).expect("mark template");
    assert_eq!((img.w, img.h), (at.w as usize, at.h as usize), "mark template size");
    img.data.as_chunks::<4>().0.iter().map(|&[r, g, b, _]| luma(r, g, b)).collect()
}

fn arrow() -> &'static [f32] {
    static A: OnceLock<Vec<f32>> = OnceLock::new();
    A.get_or_init(|| grey(include_bytes!("../../assets/s16-back-arrow.png"), ARROW_AT))
}

fn gate() -> &'static [f32] {
    static G: OnceLock<Vec<f32>> = OnceLock::new();
    G.get_or_init(|| grey(include_bytes!("../../assets/s16-gate.png"), GATE_AT))
}

fn luma(r: u8, g: u8, b: u8) -> f32 {
    0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
}

/// The grey levels of a frame's top rows.
struct Strip {
    w: usize,
    h: usize,
    grey: Vec<f32>,
}

/// A mark's template and where the reference frame has it.
struct Mark {
    t: &'static [f32],
    at: Rect,
    /// Whether the mark keeps to the right edge (else the left).
    right: bool,
}

/// A mark found on the strip: its correlation and its box's top-left corner.
#[derive(Clone, Copy)]
struct Found {
    score: f32,
    x: f32,
    y: f32,
}

impl Strip {
    /// The correlation of every `step`th template pixel with the strip
    /// sampled there, the template scaled by `s` with its top-left at (x, y).
    fn ncc(&self, m: &Mark, s: f32, x: f32, y: f32, step: usize) -> f32 {
        let (tw, th) = (m.at.w as usize, m.at.h as usize);
        // A coordinate's pixel and the weight of the next one, clamped to `len`.
        let axis = |v: f32, len: usize| {
            let v = v.clamp(0.0, len as f32 - 1.001);
            (v as usize, v - v.floor())
        };
        let (mut n, mut sp, mut spp, mut st, mut stt, mut spt) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for j in (0..th).step_by(step) {
            let (yi, ty) = axis(y + (j as f32 + 0.5) * s - 0.5, self.h);
            let (r0, r1) = (&self.grey[yi * self.w..][..self.w], &self.grey[(yi + 1) * self.w..][..self.w]);
            for i in (0..tw).step_by(step) {
                let (xi, tx) = axis(x + (i as f32 + 0.5) * s - 0.5, self.w);
                let top = r0[xi] + (r0[xi + 1] - r0[xi]) * tx;
                let bot = r1[xi] + (r1[xi + 1] - r1[xi]) * tx;
                let (p, t) = ((top + (bot - top) * ty) as f64, m.t[j * tw + i] as f64);
                n += 1.0;
                sp += p;
                spp += p * p;
                st += t;
                stt += t * t;
                spt += p * t;
            }
        }
        let (vp, vt) = (spp - sp * sp / n, stt - st * st / n);
        if vp > 1e-6 { ((spt - sp * st / n) / (vp * vt).sqrt()) as f32 } else { 0.0 }
    }

    /// Where `m` sits at scale `s` (frame px per reference px) with no shift.
    fn predicted(&self, m: &Mark, s: f32) -> (f32, f32) {
        let x = if m.right { self.w as f32 - (REF_W as f32 - m.at.x as f32) * s } else { m.at.x as f32 * s };
        (x, m.at.y as f32 * s)
    }

    /// The best match of `m` at scale `s` within (rx, ry) of (x0, y0), in
    /// shifts of `d` px, sampling every `step`th template pixel.
    fn locate(&self, m: &Mark, s: f32, (x0, y0): (f32, f32), (rx, ry): (f32, f32), d: f32, step: usize) -> Found {
        let mut best = Found { score: f32::MIN, x: x0, y: y0 };
        let (nx, ny) = ((rx / d).round() as i32, (ry / d).round() as i32);
        for iy in -ny..=ny {
            for ix in -nx..=nx {
                let (x, y) = (x0 + ix as f32 * d, y0 + iy as f32 * d);
                let score = self.ncc(m, s, x, y, step);
                if score > best.score {
                    best = Found { score, x, y };
                }
            }
        }
        best
    }

    /// `m` located to a quarter px around `f`.
    fn sharpen(&self, m: &Mark, s: f32, f: Found, r: f32) -> Found {
        let f = self.locate(m, s, (f.x, f.y), (r, r), 0.5, 1);
        self.locate(m, s, (f.x, f.y), (0.25, 0.25), 0.25, 1)
    }
}

/// The game's interface scale on `frame`, a frame of any size: 1.0 at the
/// slider setting the reference frame was measured at, about 0.645 at the
/// smallest setting. None when the back arrow or the gate icon is not
/// found, or the two disagree in y.
///
/// At scale u every UI element and the board shrink by u about an anchor:
/// the board and the pick cards about the screen centre, the top bar, the
/// gate icon and the objective panel about the top-left corner, Speed-up,
/// the back arrow and the vault about the top-right corner, Confirm about
/// the bottom centre. With the frame scaled to height 1078 and width W', a
/// capture shifted by s puts the back arrow's box at
/// x = W' - (1918 - x_arrow) * u + s and the gate icon's at
/// x = x_gate * u + s, both at y = y_ref * u, their sizes times u. Each
/// mark is found by normalised cross-correlation (so the marks dimmed under
/// the pick screens still match) within 12 px x and 8 px y of where a
/// coarse u in 0.5..=1.3 (by 0.02) puts it; u then comes from the gap between the two,
/// which the shift leaves unchanged, and the marks are found again at that
/// u twice over. A u within 0.025 of 1 gives exactly 1.0.
///
/// With a `hint` (the last u measured) the coarse u is searched within
/// [`HINT_SPAN`] of it first, by 0.01; the full search runs when either mark
/// scores below [`MIN_SCORE`] there.
///
/// Measured on the 16:9 frames of trainingdata/s16 and a 21:9 run (u = 1)
/// and on a 1920 x 1084 run at the smallest setting (u about 0.645).
fn ui_scale(frame: &Rgba, hint: Option<f32>) -> Option<f32> {
    let near = hint.map(|h| (-3..=3).map(|n| h + HINT_SPAN / 3.0 * n as f32).collect::<Vec<_>>());
    let (u, a, g) = match near.map(|us| measure(frame, &us)) {
        Some(m) if m.1.score.min(m.2.score) >= MIN_SCORE => m,
        _ => measure(frame, &(0..=40).map(|n| 0.5 + 0.02 * n as f32).collect::<Vec<_>>()),
    };
    let ok = a.score.min(g.score) >= MIN_SCORE;
    let k = frame.h as f32 / REF_H as f32;
    // The y shift each mark was found at.
    let (dya, dyg) = (a.y - ARROW_AT.y as f32 * u * k, g.y - GATE_AT.y as f32 * u * k);
    (ok && (dya - dyg).abs() <= MAX_DY).then_some(if (u - 1.0).abs() <= 0.025 { 1.0 } else { u })
}

/// How far from the hint [`ui_scale`] searches first.
const HINT_SPAN: f32 = 0.03;

/// The scale the two marks' gap gives on `frame`, starting from the best of
/// the coarse scales `us`, and the two marks found at it: the back arrow,
/// then the gate icon.
fn measure(frame: &Rgba, us: &[f32]) -> (f32, Found, Found) {
    let k = frame.h as f32 / REF_H as f32;
    let h = ((GATE_AT.y + GATE_AT.h).max(ARROW_AT.y + ARROW_AT.h) as f32 * 1.3 * k + SLACK_Y + 4.0) as usize;
    let h = h.min(frame.h);
    let strip = Strip { w: frame.w, h, grey: frame.data[..frame.w * h * 4].as_chunks::<4>().0.iter().map(|&[r, g, b, _]| luma(r, g, b)).collect() };
    let marks = [Mark { t: arrow(), at: ARROW_AT, right: true }, Mark { t: gate(), at: GATE_AT, right: false }];
    // Coarse: every other template pixel, shifts by 2 px.
    let mut coarse = (f32::MIN, 1.0, marks.each_ref().map(|_| Found { score: 0.0, x: 0.0, y: 0.0 }));
    for &u in us {
        let found = marks.each_ref().map(|m| strip.locate(m, u * k, strip.predicted(m, u * k), (SLACK_X, SLACK_Y), 2.0, 2));
        let score = found[0].score + found[1].score;
        if score > coarse.0 {
            coarse = (score, u, found);
        }
    }
    let (_, mut u, mut found) = coarse;
    // The gap between the two boxes' left edges, in frame px, gives u.
    let span = (REF_W as f32 - ARROW_AT.x as f32 + GATE_AT.x as f32) * k;
    let mut r = 2.0;
    for _ in 0..3 {
        let s = u * k;
        found = [0, 1].map(|i| strip.sharpen(&marks[i], s, found[i], r));
        let next = (frame.w as f32 - (found[0].x - found[1].x)) / span;
        // Carry each mark's shift over to the new scale.
        for (f, m) in found.iter_mut().zip(&marks) {
            let (old, new) = (strip.predicted(m, s), strip.predicted(m, next * k));
            f.x += new.0 - old.0;
            f.y += new.1 - old.1;
        }
        u = next;
        r = 1.0;
    }
    let s = u * k;
    found = [0, 1].map(|i| strip.sharpen(&marks[i], s, found[i], 0.5));
    (u, found[0], found[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> Rgba {
        let data = (0..w * h).flat_map(|i| [(i % w % 256) as u8, (i / w % 256) as u8, 7, 255]).collect();
        Rgba { w, h, data }
    }

    #[test]
    fn sixteen_nine_is_resized() {
        let frame = gradient(1280, 720);
        assert_eq!(lay_out(&frame, 1.0).data, frame.resize(REF_W, REF_H).data);
        let near = gradient(1902, 1072);
        assert_eq!(lay_out(&near, 1.0).data, near.resize(REF_W, REF_H).data);
    }

    /// A 1716 x 720 frame (2.38:1) with white blocks at the first
    /// instrument icon, the board's middle and the first vault slot.
    #[test]
    fn wide_frame_keeps_edges_pinned() {
        let (w, h) = (1716, 720);
        let mut frame = Rgba { w, h, data: [0, 0, 0, 255].repeat(w * h) };
        for (cx, cy) in [(82, 261), (858, 200), (1584, 160)] {
            for y in cy - 12..cy + 12 {
                for x in cx - 12..cx + 12 {
                    frame.data[(y * w + x) * 4..][..3].copy_from_slice(&[255, 255, 255]);
                }
            }
        }
        let out = lay_out(&frame, 1.0);
        assert_eq!((out.w, out.h), (REF_W, REF_H));
        // The scaled frame is 2569 wide: 651 more than the reference.
        for (x, y) in [(122, 390), (960, 300), (1720, 240)] {
            assert_eq!(out.rgb(x, y), [255, 255, 255], "({x}, {y})");
        }
        // Centred, the edge blocks would land 325 px further in.
        for (x, y) in [(122 + 325, 390), (1720 - 325, 240)] {
            assert_eq!(out.rgb(x, y), [0, 0, 0], "({x}, {y})");
        }
    }

    /// A black 1920 x 1084 frame at u = 0.645 with a white 7 x 7 block about
    /// where each kind of anchor puts a reference point (worked out by hand
    /// from the zone formula): each lands on its reference point.
    #[test]
    fn scaled_frame_lands_on_reference_points() {
        let (w, h) = (1920, 1084);
        let mut frame = Rgba { w, h, data: [0, 0, 0, 255].repeat(w * h) };
        // (reference point, block centre on the frame, what anchors it).
        let marks = [
            ((960, 600), (960, 581), "centre, centre (outside every zone)"),
            ((100, 100), (65, 65), "left, top"),
            ((1800, 100), (1843, 65), "right, top"),
            ((1724, 1003), (1794, 843), "right, centre"),
            ((959, 30), (960, 19), "centre, top"),
            ((959, 1000), (960, 1033), "centre, bottom"),
            ((122, 391), (78, 251), "left, top about a point off the corner"),
            ((959, 120), (960, 78), "centre, top in the title layout only"),
        ];
        for (_, (cx, cy), _) in marks {
            for y in cy - 3..=cy + 3 {
                for x in cx - 3..=cx + 3 {
                    frame.data[(y * w + x) * 4..][..3].copy_from_slice(&[255, 255, 255]);
                }
            }
        }
        let board = lay_out(&frame, 0.645);
        let title = lay_out_title(&frame, 0.645, &board);
        for ((x, y), _, what) in marks {
            assert_eq!(title.rgb(x, y), [255, 255, 255], "({x}, {y}) {what}");
        }
        for ((x, y), _, what) in &marks[..7] {
            assert_eq!(board.rgb(*x, *y), [255, 255, 255], "({x}, {y}) {what}");
        }
        assert_eq!(board.rgb(959, 120), [0, 0, 0], "the title point keeps to the centre on the board");
    }

    /// Near the right u, or far from it, a hint gives the full search's u.
    #[test]
    fn hint_gives_the_full_search_scale() {
        let Some(frame) = reference() else { return };
        let full = ui_scale(&frame, None);
        assert_eq!(full, Some(1.0));
        for hint in [1.0, 0.98, 0.7] {
            assert_eq!(ui_scale(&frame, Some(hint)), full, "hint {hint}");
        }
    }

    /// The reference frame rec1-t087 at its own size, None (and a note)
    /// when trainingdata/s16 is missing.
    fn reference() -> Option<Rgba> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("trainingdata/s16");
        if !dir.is_dir() {
            println!("skipped: {} is missing", dir.display());
            return None;
        }
        Some(super::super::decode_png_raw(&std::fs::read(dir.join("rec1-t087.png")).unwrap()).unwrap())
    }

    /// The reference frame moved 6 px right, black on the left.
    #[test]
    fn shifted_frame_has_unit_scale() {
        let Some(frame) = reference() else { return };
        let mut data = [0, 0, 0, 255].repeat(frame.w * frame.h);
        for (out, row) in data.chunks_exact_mut(frame.w * 4).zip(frame.data.chunks_exact(frame.w * 4)) {
            out[6 * 4..].copy_from_slice(&row[..(frame.w - 6) * 4]);
        }
        assert_eq!(ui_scale(&Rgba { data, ..frame }, None), Some(1.0));
    }

    /// The reference frame shrunk by 0.645, its left half about the
    /// top-left corner and its right half about the top-right corner.
    #[test]
    fn shrunk_frame_has_its_scale() {
        let Some(frame) = reference() else { return };
        let (w, h) = (frame.w, frame.h);
        let small = frame.resize((w as f32 * 0.645).round() as usize, (h as f32 * 0.645).round() as usize);
        let mut data = [0, 0, 0, 255].repeat(w * h);
        for y in 0..small.h {
            for x in 0..w {
                let sx = if x < w / 2 { x } else { x + small.w - w };
                if sx < small.w {
                    data[(y * w + x) * 4..][..4].copy_from_slice(&small.data[(y * small.w + sx) * 4..][..4]);
                }
            }
        }
        let u = ui_scale(&Rgba { w, h, data }, None).expect("a scale");
        assert!((u - 0.645).abs() <= 0.01, "{u}");
    }
}
