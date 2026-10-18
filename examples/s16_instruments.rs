//! Builds assets/s16-instruments.bin from the game's instrument icons
//! (UI_S16Gameplay_RelicIcon_NN.png in the NoAtlas texture folder; icon NN is
//! the NN-th instrument in data order).

use std::path::Path;
use warden::engine::data::data;
use warden::reader::instruments::{GRID, Icons, box_downscale};

/// One icon area-averaged to GRID x GRID RGBA.
fn icon(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba {
        return Err(format!("{:?}, expected RGBA", info.color_type));
    }
    Ok(box_downscale(&buf[..info.buffer_size()], info.width as usize, info.height as usize, GRID))
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: s16_instruments <NoAtlas dir>");
        std::process::exit(1);
    });
    let icons: Vec<Vec<u8>> = (1..=data().tool_ids.len())
        .map(|n| {
            let path = Path::new(&dir).join(format!("UI_S16Gameplay_RelicIcon_{n:02}.png"));
            icon(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        })
        .collect();
    let bytes = Icons::to_bytes(&icons);
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("s16-instruments.bin");
    std::fs::write(&out, &bytes).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
    println!("{} icons -> {} ({} bytes)", icons.len(), out.display(), bytes.len());
}
