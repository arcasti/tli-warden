//! The ring drawn on the game around what to click for the advised move: a
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

/// The ring's width at 1080p, edges included, and each dark edge's.
pub const STROKE: f32 = 7.0;
const EDGE: f32 = 1.5;
/// The bright core, premultiplied alpha 255.
const CORE: [u8; 3] = [170, 255, 60];
/// The dark edge's opacity.
const EDGE_ALPHA: f32 = 180.0 / 255.0;

/// The ring window; hidden until [`Ring::show`], destroyed on drop.
pub struct Ring {
    hwnd: HWND,
    /// What the window shows: centre, radius and stroke in screen px.
    drawn: Option<[f32; 4]>,
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: the arguments are the ones Windows passed.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Ring {
    /// A hidden ring window; None when Windows refuses one.
    pub fn new() -> Option<Ring> {
        // SAFETY: plain Win32 calls; the class name is a static string.
        let hwnd = dpi_aware(|| unsafe {
            let instance = GetModuleHandleW(None).ok()?;
            let class = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance.into(), lpszClassName: w!("WardenRing"), ..Default::default() };
            // A second ring finds the class registered already.
            RegisterClassW(&class);
            let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            CreateWindowExW(ex, w!("WardenRing"), w!("Warden ring"), WS_POPUP, 0, 0, 1, 1, None, None, Some(instance.into()), None).ok()
        })?;
        Some(Ring { hwnd, drawn: None })
    }

    /// Shows a ring of radius `r` centred on (`x`, `y`) in physical screen
    /// px, `stroke` px wide; drawn again only when one of them changes.
    pub fn show(&mut self, x: f32, y: f32, r: f32, stroke: f32) {
        let want = [x, y, r, stroke];
        if self.drawn == Some(want) {
            return;
        }
        let side = (2.0 * (r + stroke / 2.0)).ceil() as i32 + 4;
        let (left, top) = ((x - side as f32 / 2.0).round() as i32, (y - side as f32 / 2.0).round() as i32);
        let pixels = paint(side as usize, x - left as f32, y - top as f32, r, stroke);
        if dpi_aware(|| self.update(&pixels, side, left, top)) {
            self.drawn = Some(want);
            // SAFETY: a window this ring made.
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
            // SAFETY: a window this ring made.
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    /// Puts the `side` x `side` premultiplied BGRA `pixels` on the window at
    /// (`left`, `top`); false when Windows refuses.
    fn update(&self, pixels: &[u8], side: i32, left: i32, top: i32) -> bool {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: side,
                // Negative: rows run top down.
                biHeight: -side,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: the DC and bitmap are made, used and freed here; the bitmap
        // holds side * side * 4 bytes, the length of `pixels`.
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
            let (at, size, from) = (POINT { x: left, y: top }, SIZE { cx: side, cy: side }, POINT::default());
            let done = UpdateLayeredWindow(self.hwnd, None, Some(&at), Some(&size), Some(dc), Some(&from), COLORREF(0), Some(&blend), ULW_ALPHA);
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            done.is_ok()
        }
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        // SAFETY: a window this ring made, destroyed once.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// A `side` x `side` premultiplied BGRA ring centred on (`cx`, `cy`):
/// a dark edge on each side of a bright core, anti-aliased.
fn paint(side: usize, cx: f32, cy: f32, r: f32, stroke: f32) -> Vec<u8> {
    let edge = EDGE * stroke / STROKE;
    let (outer, inner) = (r + stroke / 2.0, r - stroke / 2.0);
    // How much of a pixel at distance d lies in the band from a to b.
    let band = |d: f32, a: f32, b: f32| (d - a + 0.5).clamp(0.0, 1.0) * (b - d + 0.5).clamp(0.0, 1.0);
    let mut out = vec![0u8; side * side * 4];
    for (i, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = ((i % side) as f32 + 0.5 - cx, (i / side) as f32 + 0.5 - cy);
        let d = (x * x + y * y).sqrt();
        let core = band(d, inner + edge, outer - edge);
        let alpha = core + band(d, inner, outer) * EDGE_ALPHA * (1.0 - core);
        let [r, g, b] = CORE.map(|c| (c as f32 * core).round() as u8);
        px.copy_from_slice(&[b, g, r, (alpha * 255.0).round() as u8]);
    }
    out
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
