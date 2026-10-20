//! Text from a region of the frame through the OCR engine built into Windows
//! (Windows.Media.Ocr). The game text is English, so the engine reads in the
//! first English OCR language installed, else in the user's profile languages
//! (one of another script, such as Russian, misreads the game).

use super::{Rect, Rgba};

/// The language tag the engine reads in ("en-US"); None when Windows has
/// no OCR language installed.
pub fn language() -> Option<String> {
    imp::language()
}

/// Why the game can't be read: Windows has no English OCR language. None
/// when it has one.
pub fn problem() -> Option<String> {
    // Settings can show English with its OCR feature removed (debloated Windows 10).
    const ADD: &str = "in PowerShell as administrator run Add-WindowsCapability -Online -Name Language.OCR~~~en-US~0.0.1.0, then restart the app";
    match language() {
        Some(tag) if is_english(&tag) => None,
        Some(tag) => Some(format!("Windows reads text in {tag} only, which misreads the game: {ADD}.")),
        None => Some(format!("Windows has no text recognition (OCR) language, so nothing can be read: {ADD}.")),
    }
}

/// "en", "en-US", "en-GB"...
fn is_english(tag: &str) -> bool {
    tag.to_ascii_lowercase().starts_with("en")
}

/// One line of text and its box in frame pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub rect: Rect,
}

/// The lines of text inside `rect`, read from the crop upscaled by `scale`
/// (small game text reads better at 2-3x). Empty when the region is empty
/// or OCR is not available.
pub fn ocr(img: &Rgba, rect: Rect, scale: f32) -> Vec<Line> {
    ocr_all(img, &[(rect, scale)]).pop().unwrap_or_default()
}

/// The lines of each `(rect, scale)` region as `ocr` reads them, in order,
/// with the regions read at the same time.
pub fn ocr_all(img: &Rgba, regions: &[(Rect, f32)]) -> Vec<Vec<Line>> {
    let inputs: Vec<Option<Rgba>> = regions.iter().map(|&(rect, scale)| prepare(img, rect, scale)).collect();
    regions
        .iter()
        .zip(imp::recognize_all(inputs))
        .map(|(&(rect, scale), lines)| {
            lines
                .into_iter()
                .map(|(text, [x, y, lw, lh])| Line {
                    text,
                    rect: Rect::new(
                        rect.x + (x / scale).round() as i32,
                        rect.y + (y / scale).round() as i32,
                        (lw / scale).round() as i32,
                        (lh / scale).round() as i32,
                    ),
                })
                .collect()
        })
        .collect()
}

/// The OCR input for `rect`: the crop upscaled by `scale` with its contrast
/// stretched. None when the region is empty.
fn prepare(img: &Rgba, rect: Rect, scale: f32) -> Option<Rgba> {
    let crop = img.crop(rect);
    if crop.w == 0 || crop.h == 0 {
        return None;
    }
    let (w, h) = ((crop.w as f32 * scale).round() as usize, (crop.h as f32 * scale).round() as usize);
    Some(stretch(&crop.resize(w.max(1), h.max(1))))
}

/// Stretches the contrast so the 1st to 99th percentile of luma spans 0-255:
/// faded-in buttons and text under the red target-pick tint read better.
fn stretch(img: &Rgba) -> Rgba {
    let mut hist = [0usize; 256];
    for p in img.data.as_chunks::<4>().0 {
        hist[luma(p) as usize] += 1;
    }
    let n = img.w * img.h;
    let percentile = |q: usize| {
        let mut acc = 0;
        hist.iter().position(|&c| {
            acc += c;
            acc * 100 >= n * q
        })
    };
    let (lo, hi) = (percentile(1).unwrap_or(0) as f32, percentile(99).unwrap_or(255) as f32);
    if hi - lo < 8.0 {
        return img.clone();
    }
    let k = 255.0 / (hi - lo);
    let table: [u8; 256] = std::array::from_fn(|v| ((v as f32 - lo) * k).clamp(0.0, 255.0) as u8);
    let mut data = img.data.clone();
    for p in data.as_chunks_mut::<4>().0 {
        for v in &mut p[..3] {
            *v = table[*v as usize];
        }
    }
    Rgba { w: img.w, h: img.h, data }
}

fn luma(p: &[u8; 4]) -> u8 {
    ((u32::from(p[0]) * 77 + u32::from(p[1]) * 150 + u32::from(p[2]) * 29) >> 8) as u8
}

#[cfg(windows)]
mod imp {
    use super::Rgba;
    use std::sync::{Arc, Mutex, OnceLock, mpsc};
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::{OcrEngine, OcrResult};
    use windows::Storage::Streams::DataWriter;
    use windows::core::Result;

    thread_local! {
        static ENGINE: Option<OcrEngine> = engine();
    }

    /// The first English OCR language installed, else the profile languages.
    fn engine() -> Option<OcrEngine> {
        let english = OcrEngine::AvailableRecognizerLanguages()
            .ok()
            .and_then(|all| all.into_iter().find(|l| l.LanguageTag().is_ok_and(|t| super::is_english(&t.to_string()))));
        english.map_or_else(OcrEngine::TryCreateFromUserProfileLanguages, |l| OcrEngine::TryCreateFromLanguage(&l)).ok()
    }

    pub fn language() -> Option<String> {
        ENGINE.with(|e| Some(e.as_ref()?.RecognizerLanguage().ok()?.LanguageTag().ok()?.to_string()))
    }

    type Lines = Vec<(String, [f32; 4])>;
    type Job = (Rgba, mpsc::Sender<(usize, Lines)>, usize);

    /// Reader threads, each with its own engine: one engine given several
    /// reads at once returns some of them empty.
    const WORKERS: usize = 4;

    /// The queue the reader threads take images from, started on first use.
    fn pool() -> &'static mpsc::Sender<Job> {
        static POOL: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
        POOL.get_or_init(|| {
            let (jobs, queue) = mpsc::channel::<Job>();
            let queue = Arc::new(Mutex::new(queue));
            for _ in 0..WORKERS {
                let queue = Arc::clone(&queue);
                std::thread::spawn(move || {
                    while let Ok((img, back, i)) = queue.lock().map_err(drop).and_then(|q| q.recv().map_err(drop)) {
                        let _ = back.send((i, recognize(&img)));
                    }
                });
            }
            jobs
        })
    }

    /// The lines of each image with their boxes (x, y, w, h) in its pixels;
    /// none for a missing image or a failed read. Several images are read on
    /// the reader threads at once.
    pub fn recognize_all(imgs: Vec<Option<Rgba>>) -> Vec<Lines> {
        let mut out = vec![Vec::new(); imgs.len()];
        let (back, done) = mpsc::channel();
        for (i, img) in imgs.into_iter().enumerate() {
            if let Some(img) = img {
                let _ = pool().send((img, back.clone(), i));
            }
        }
        drop(back);
        for (i, lines) in done {
            out[i] = lines;
        }
        out
    }

    /// The lines of `img` read on this thread's engine; none when the read fails.
    fn recognize(img: &Rgba) -> Lines {
        let read = || lines(&ENGINE.with(|e| e.as_ref().map(|e| e.RecognizeAsync(&bitmap(img)?))).ok_or_else(windows::core::Error::empty)??.join()?);
        read().unwrap_or_default()
    }

    /// `img` as a Bgra8 bitmap, shrunk to the engine's largest size.
    fn bitmap(img: &Rgba) -> Result<SoftwareBitmap> {
        let max = OcrEngine::MaxImageDimension()? as usize;
        let img = if img.w > max || img.h > max {
            let k = max as f32 / img.w.max(img.h) as f32;
            img.resize(((img.w as f32 * k) as usize).max(1), ((img.h as f32 * k) as usize).max(1))
        } else {
            img.clone()
        };
        let bgra: Vec<u8> = img.data.as_chunks::<4>().0.iter().flat_map(|&[r, g, b, _]| [b, g, r, 255]).collect();
        let writer = DataWriter::new()?;
        writer.WriteBytes(&bgra)?;
        SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &writer.DetachBuffer()?,
            BitmapPixelFormat::Bgra8,
            img.w as i32,
            img.h as i32,
            BitmapAlphaMode::Ignore,
        )
    }

    /// The lines of a result with their boxes (x, y, w, h).
    fn lines(result: &OcrResult) -> Result<Vec<(String, [f32; 4])>> {
        let mut out = Vec::new();
        for line in result.Lines()? {
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for word in line.Words()? {
                let r = word.BoundingRect()?;
                (x0, y0) = (x0.min(r.X), y0.min(r.Y));
                (x1, y1) = (x1.max(r.X + r.Width), y1.max(r.Y + r.Height));
            }
            if x0 <= x1 {
                out.push((line.Text()?.to_string(), [x0, y0, x1 - x0, y1 - y0]));
            }
        }
        Ok(out)
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Rgba;

    pub fn recognize_all(imgs: Vec<Option<Rgba>>) -> Vec<Vec<(String, [f32; 4])>> {
        vec![Vec::new(); imgs.len()]
    }

    pub fn language() -> Option<String> {
        None
    }
}
