//! The screenshot reader for the S16 prison screen (docs/s16-screen.md).
//! The board scene comes from pixels (`scene`), the text panels from the
//! Windows OCR engine (`ocr`, `panels`), and `model` joins them into a board
//! model.

pub mod instruments;
pub mod layout;
pub mod model;
pub mod ocr;
pub mod panels;
pub mod scene;

use std::path::Path;

/// Width and height of the reference frame every coordinate refers to.
pub const REF_W: usize = 1918;
pub const REF_H: usize = 1078;

/// A pixel rectangle: left, top, width, height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn centre(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

/// An 8-bit RGBA image, rows top to bottom.
#[derive(Clone, Debug)]
pub struct Rgba {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

impl Rgba {
    /// The RGB of pixel (x, y), clamped to the image.
    pub fn rgb(&self, x: i32, y: i32) -> [u8; 3] {
        let x = x.clamp(0, self.w as i32 - 1) as usize;
        let y = y.clamp(0, self.h as i32 - 1) as usize;
        let i = (y * self.w + x) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// The part of the image inside `r`, clipped to the image.
    pub fn crop(&self, r: Rect) -> Rgba {
        let x0 = r.x.clamp(0, self.w as i32) as usize;
        let y0 = r.y.clamp(0, self.h as i32) as usize;
        let x1 = (r.x + r.w).clamp(x0 as i32, self.w as i32) as usize;
        let y1 = (r.y + r.h).clamp(y0 as i32, self.h as i32) as usize;
        let mut data = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
        for y in y0..y1 {
            data.extend_from_slice(&self.data[(y * self.w + x0) * 4..(y * self.w + x1) * 4]);
        }
        Rgba { w: x1 - x0, h: y1 - y0, data }
    }

    /// Bilinear resize to `w` x `h`.
    pub fn resize(&self, w: usize, h: usize) -> Rgba {
        if w == self.w && h == self.h {
            return self.clone();
        }
        let mut data = vec![0u8; w * h * 4];
        let sx = self.w as f32 / w as f32;
        let sy = self.h as f32 / h as f32;
        // Each output column's two source byte offsets in a row and its weight, worked out once.
        let cols: Vec<(usize, usize, f32)> = (0..w)
            .map(|x| {
                let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (self.w - 1) as f32);
                let x0 = fx as usize;
                (x0 * 4, (x0 + 1).min(self.w - 1) * 4, fx - x0 as f32)
            })
            .collect();
        let row_len = self.w * 4;
        for (y, out) in data.chunks_exact_mut(w * 4).enumerate() {
            let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (self.h - 1) as f32);
            let y0 = fy as usize;
            let y1 = (y0 + 1).min(self.h - 1);
            let ty = fy - y0 as f32;
            let (top_row, bot_row) = (&self.data[y0 * row_len..][..row_len], &self.data[y1 * row_len..][..row_len]);
            for (px, &(x0, x1, tx)) in out.as_chunks_mut::<4>().0.iter_mut().zip(&cols) {
                for c in 0..4 {
                    let top = top_row[x0 + c] as f32 * (1.0 - tx) + top_row[x1 + c] as f32 * tx;
                    let bot = bot_row[x0 + c] as f32 * (1.0 - tx) + bot_row[x1 + c] as f32 * tx;
                    px[c] = (top * (1.0 - ty) + bot * ty).round() as u8;
                }
            }
        }
        Rgba { w, h, data }
    }
}

/// Decodes a PNG into RGBA at the reference size, in the board layout and,
/// when it differs, the title layout; see [`layout::lay_out_scaled`].
fn decode_png(bytes: &[u8]) -> Result<(Rgba, Option<Rgba>), String> {
    decode_png_raw(bytes).map(|img| {
        let (board, title, _) = layout::lay_out_scaled(&img, None);
        (board, title)
    })
}

/// Decodes a PNG into RGBA at its own size.
fn decode_png_raw(bytes: &[u8]) -> Result<Rgba, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let size = reader.output_buffer_size().ok_or("PNG too large")?;
    let mut buf = vec![0u8; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width as usize, info.height as usize);
    let src = &buf[..info.buffer_size()];
    let data = match info.color_type {
        png::ColorType::Rgba => src.to_vec(),
        png::ColorType::Rgb => src.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect(),
        png::ColorType::GrayscaleAlpha => src.as_chunks::<2>().0.iter().flat_map(|&[g, a]| [g, g, g, a]).collect(),
        png::ColorType::Grayscale => src.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("indexed PNG left unexpanded".into()),
    };
    Ok(Rgba { w, h, data })
}

/// Loads a PNG file in the board layout; see [`decode_png`].
pub fn load_png(path: &Path) -> Result<Rgba, String> {
    load_layouts(path).map(|(board, _)| board)
}

/// Loads a PNG file in both layouts; see [`decode_png`].
fn load_layouts(path: &Path) -> Result<(Rgba, Option<Rgba>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode_png(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes an image as an 8-bit RGBA PNG file.
pub fn save_png(path: &Path, img: &Rgba) -> Result<(), String> {
    let at = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let file = std::fs::File::create(path).map_err(|e| at(&e))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), img.w as u32, img.h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| at(&e))?;
    writer.write_image_data(&img.data).map_err(|e| at(&e))?;
    writer.finish().map_err(|e| at(&e))
}

/// Reads a screenshot into a board model and the warnings to check;
/// `held_tools` are the instruments the player holds, or when empty those the
/// instrument panel's icons name.
pub fn read_model(path: &Path, held_tools: &[crate::engine::data::Tool]) -> Result<(crate::board::Model, Vec<model::Warning>), String> {
    let (img, title) = load_layouts(path)?;
    let panels = panels::read_panels_titled(&img, title.as_ref().unwrap_or(&img));
    let held = if held_tools.is_empty() { panels.held.clone().unwrap_or_default() } else { held_tools.to_vec() };
    Ok(model::to_model(&scene::read_scene(&img), &panels, &held, None))
}
