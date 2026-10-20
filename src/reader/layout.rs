//! Turns a game frame of any width into the 1918 x 1078 reference frame the
//! reader's regions are measured on.

use super::{REF_H, REF_W, Rect, Rgba};

/// Which edge of a wider window a zone of the game's screen keeps to.
#[derive(Clone, Copy)]
enum Pin {
    Left,
    Right,
}

/// The zones of the reference frame that a window wider than 16:9 keeps at
/// its left or right edge; everything else (the board, the top counter, the
/// pick cards, the headings and the decision cards) stays centred. Every
/// zone scales by the frame height, as the centre does.
///
/// Measured on a 1716 x 720 (21:9) clip against 16:9 frames of the same
/// screens: the board (rec1-t087), the Select a Torture Instrument cards
/// (rec1-t031, x 250-1670) and the Warden's Treasure cards (rec1-t085).
/// The left zones hold the objective panel and the Torture Instrument icons;
/// they stop at x 400 above the instrument cards' tops (y 180) and below
/// at x 260, where the second icon's crop ends (the first instrument card's
/// left 10 px, art only, go with it). The right zones hold Speed-up, View
/// Prison Status and the Chromatic Prisoner heading, the vault slots (from
/// x 1680, right of the instrument cards' names) and the Consume target
/// pick's Cancel. The vault zone covers the right part of the fourth
/// Warden's Treasure card, which the reader does not read; it also covers
/// the right end of the Prison Incident popup title region, which no wide
/// frame has shown yet.
const ZONES: [(Rect, Pin); 5] = [
    (Rect::new(0, 0, 400, 260), Pin::Left),
    (Rect::new(0, 260, 260, 230), Pin::Left),
    (Rect::new(1540, 0, 378, 200), Pin::Right),
    (Rect::new(1680, 200, 238, 520), Pin::Right),
    (Rect::new(1600, 720, 318, 358), Pin::Right),
];

/// Whether a `w` x `h` frame is 16:9 within 1%.
pub fn is_16_9(w: usize, h: usize) -> bool {
    (w * 9).abs_diff(h * 16) * 100 <= h * 16
}

/// The frame at the reference size. A 16:9 frame (within 1%) is resized.
/// Any other frame is scaled to the reference height and laid out by
/// [`ZONES`]: a pinned zone takes the pixels at the same distance from its
/// edge of the frame, the rest those at the same distance from the centre.
/// Pixels past the frame's edges are black.
pub fn normalise(frame: &Rgba) -> Rgba {
    if is_16_9(frame.w, frame.h) {
        return frame.resize(REF_W, REF_H);
    }
    let w = (frame.w * REF_H + frame.h / 2) / frame.h;
    let scaled = frame.resize(w, REF_H);
    let extra = w as i64 - REF_W as i64;
    let mut data = [0, 0, 0, 255].repeat(REF_W * REF_H);
    let mut copy = |y: usize, x0: usize, x1: usize, shift: i64| {
        let s0 = (x0 as i64 + shift).clamp(0, w as i64) as usize;
        let s1 = (x1 as i64 + shift).clamp(0, w as i64) as usize;
        if s0 < s1 {
            let d0 = (s0 as i64 - shift) as usize;
            let src = &scaled.data[(y * w + s0) * 4..(y * w + s1) * 4];
            data[(y * REF_W + d0) * 4..][..src.len()].copy_from_slice(src);
        }
    };
    for y in 0..REF_H {
        copy(y, 0, REF_W, extra / 2);
        for (r, pin) in ZONES.iter().filter(|(r, _)| (r.y..r.y + r.h).contains(&(y as i32))) {
            let shift = match pin {
                Pin::Left => 0,
                Pin::Right => extra,
            };
            copy(y, r.x as usize, (r.x + r.w) as usize, shift);
        }
    }
    Rgba { w: REF_W, h: REF_H, data }
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
        assert_eq!(normalise(&frame).data, frame.resize(REF_W, REF_H).data);
        let near = gradient(1902, 1072);
        assert_eq!(normalise(&near).data, near.resize(REF_W, REF_H).data);
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
        let out = normalise(&frame);
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
}
