//! The mark drawn on the game on what to click for the advised move: corner
//! brackets around the target and an arrow above it pointing down, on a
//! layered window over the game that never takes focus and lets clicks
//! through, drawn from a premultiplied BGRA bitmap.

use super::capture::{dpi_aware, ours};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow, ULW_ALPHA,
    UpdateLayeredWindow, WNDCLASSW, WindowFromPoint, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::w;

/// Sizes below are reference px (1080p): the brackets' line width, edges
/// included, and each dark edge's.
const STROKE: f32 = 5.0;
const EDGE: f32 = 1.5;
/// How far outside the target's box the brackets sit.
const PAD: f32 = 8.0;
/// The longest bracket arm.
const ARM: f32 = 70.0;
/// The arrow's width and height, and the gap from its tip down to the
/// brackets' top edge.
const ARROW_W: f32 = 46.0;
const ARROW_H: f32 = 34.0;
const ARROW_GAP: f32 = 10.0;
/// The bright core, premultiplied alpha 255.
const GOLD: [u8; 3] = [255, 205, 110];
/// The dark edge's opacity.
const EDGE_ALPHA: f32 = 180.0 / 255.0;

/// The mark window; hidden until [`Mark::show`], destroyed on drop.
pub struct Mark {
    hwnd: HWND,
    /// What the window shows: centre, half width, half height and unit in
    /// screen px.
    drawn: Option<[f32; 5]>,
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: the arguments are the ones Windows passed.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Mark {
    /// A hidden mark window; None when Windows refuses one.
    pub fn new() -> Option<Mark> {
        // SAFETY: plain Win32 calls; the class name is a static string.
        let hwnd = dpi_aware(|| unsafe {
            let instance = GetModuleHandleW(None).ok()?;
            let class = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance.into(), lpszClassName: w!("WardenMark"), ..Default::default() };
            // A second mark finds the class registered already.
            RegisterClassW(&class);
            let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            CreateWindowExW(ex, w!("WardenMark"), w!("Warden mark"), WS_POPUP, 0, 0, 1, 1, None, None, Some(instance.into()), None).ok()
        })?;
        Some(Mark { hwnd, drawn: None })
    }

    /// Marks a target of half size `half_w` x `half_h` centred on (`x`, `y`)
    /// in physical screen px, `unit` screen px per reference px; drawn again
    /// only when one of them changes.
    pub fn show(&mut self, x: f32, y: f32, half_w: f32, half_h: f32, unit: f32) {
        let want = [x, y, half_w, half_h, unit];
        if self.drawn == Some(want) {
            return;
        }
        // The padded box plus the stroke on every side, the arrow above, and
        // 2 px slack.
        let side = (PAD + STROKE / 2.0) * unit + 2.0;
        let (right, below) = (half_w + side, half_h + side);
        let above = below + (ARROW_GAP + ARROW_H) * unit;
        let (width, height) = ((2.0 * right).ceil() as i32, (above + below).ceil() as i32);
        let (left, top) = ((x - right).round() as i32, (y - above).round() as i32);
        let pixels = paint((width as usize, height as usize), (x - left as f32, y - top as f32), (half_w, half_h), unit);
        if dpi_aware(|| self.update(&pixels, width, height, left, top)) {
            self.drawn = Some(want);
            // SAFETY: a window this mark made.
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
        }
    }

    pub fn shown(&self) -> bool {
        self.drawn.is_some()
    }

    pub fn hide(&mut self) {
        if self.drawn.take().is_some() {
            // SAFETY: a window this mark made.
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    /// Puts the `width` x `height` premultiplied BGRA `pixels` on the window
    /// at (`left`, `top`); false when Windows refuses.
    fn update(&self, pixels: &[u8], width: i32, height: i32, left: i32, top: i32) -> bool {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: rows run top down.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: the DC and bitmap are made, used and freed here; the bitmap
        // holds width * height * 4 bytes, the length of `pixels`.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let mut bits = std::ptr::null_mut();
            let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return false;
            };
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
            let old = SelectObject(dc, bitmap.into());
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let (at, size, from) = (POINT { x: left, y: top }, SIZE { cx: width, cy: height }, POINT::default());
            let done = UpdateLayeredWindow(self.hwnd, None, Some(&at), Some(&size), Some(dc), Some(&from), COLORREF(0), Some(&blend), ULW_ALPHA);
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            done.is_ok()
        }
    }
}

impl Drop for Mark {
    fn drop(&mut self) {
        // SAFETY: a window this mark made, destroyed once.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// A `width` x `height` premultiplied BGRA mark for a target of half size
/// `half_w` x `half_h` centred on (`cx`, `cy`), `unit` px per reference px:
/// corner brackets and an arrow, a bright core in a dark edge, anti-aliased.
fn paint((width, height): (usize, usize), (cx, cy): (f32, f32), (half_w, half_h): (f32, f32), unit: f32) -> Vec<u8> {
    let (stroke, edge) = (STROKE * unit, EDGE * unit);
    // The padded box's corner, and the arms running in from it.
    let (bx, by) = (half_w + PAD * unit, half_h + PAD * unit);
    let arm = (ARM * unit).min(0.6 * half_w.min(half_h));
    // The arrow's tip, below the centre of its top side.
    let (tip, arrow_w, arrow_h) = (-by - stroke / 2.0 - ARROW_GAP * unit, ARROW_W * unit, ARROW_H * unit);
    let side_len = (arrow_h * arrow_h + arrow_w * arrow_w / 4.0).sqrt();
    let mut out = vec![0u8; width * height * 4];
    for (i, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = ((i % width) as f32 + 0.5 - cx, (i / width) as f32 + 0.5 - cy);
        // The four corners mirror each other; arms stop short of the axes.
        let (ax, ay) = (x.abs(), y.abs());
        let d =segment(ax, ay, (bx - arm, by), (bx, by)).min(segment(ax, ay, (bx, by - arm), (bx, by)));
        // Signed distance to the triangle: the largest of its sides'.
        let q = y - tip;
        let s = (-q - arrow_h).max((ax * arrow_h + q * arrow_w / 2.0) / side_len);
        let core = (stroke / 2.0 - edge - d + 0.5).clamp(0.0, 1.0).max((-s - edge + 0.5).clamp(0.0, 1.0));
        let rim = (stroke / 2.0 - d + 0.5).clamp(0.0, 1.0).max((-s + 0.5).clamp(0.0, 1.0));
        let alpha = core + rim * EDGE_ALPHA * (1.0 - core);
        let [r, g, b] = GOLD.map(|c| (c as f32 * core).round() as u8);
        px.copy_from_slice(&[b, g, r, (alpha * 255.0).round() as u8]);
    }
    out
}

/// The distance from (`x`, `y`) to the segment from `a` to `b`.
fn segment(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy).max(f32::EPSILON)).clamp(0.0, 1.0);
    let (ex, ey) = (x - a.0 - t * dx, y - a.1 - t * dy);
    (ex * ex + ey * ey).sqrt()
}

/// Clears the left button's pressed-since-last-call bit, so the next
/// [`clicked_in`] sees only clicks after this call.
pub fn forget_clicks() {
    // SAFETY: a plain Win32 call.
    unsafe {
        GetAsyncKeyState(VK_LBUTTON.0 as i32);
    }
}

/// Whether the left button is down, or was pressed since the last call,
/// with the cursor in `rect` (left, top, width, height in physical screen px)
/// and not on this app's own window over the game.
pub fn clicked_in([left, top, width, height]: [i32; 4]) -> bool {
    // SAFETY: plain Win32 calls; the point is a valid out pointer.
    let (state, cursor) = dpi_aware(|| unsafe {
        let mut p = POINT::default();
        (GetAsyncKeyState(VK_LBUTTON.0 as i32), GetCursorPos(&mut p).ok().map(|_| (p, WindowFromPoint(p))))
    });
    let pressed = state as u16 & 0x8001 != 0;
    pressed && cursor.is_some_and(|(p, under)| (left..left + width).contains(&p.x) && (top..top + height).contains(&p.y) && !ours(under))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At 1080p, a 200 x 200 target centred on (200.5, 250.5): its padded
    /// corner sits on the centre of pixel (308, 358), the arrow's tip at
    /// y 130 and its top side at y 96.
    #[test]
    fn brackets_and_arrow_are_gold_and_the_target_clear() {
        let (w, h) = (400, 400);
        let out = paint((w, h), (200.5, 250.5), (100.0, 100.0), 1.0);
        let at = |x: usize, y: usize| <[u8; 4]>::try_from(&out[(y * w + x) * 4..][..4]).unwrap();
        let gold = [GOLD[2], GOLD[1], GOLD[0], 255];
        assert_eq!(at(200, 107), gold, "the arrow's middle");
        assert_eq!(at(308, 358), gold, "a bracket corner");
        assert_eq!(at(92, 142), gold, "the opposite corner");
        assert_eq!(at(200, 250), [0; 4], "the target's centre");
    }
}
