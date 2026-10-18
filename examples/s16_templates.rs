//! Builds assets/s16-templates.bin from the labeled S16 frames
//! (trainingdata/s16, data/scan-truth-s16.json).

use std::path::Path;
use warden::reader::scene::{build_templates, load_labeled};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let frames = load_labeled(&root.join("trainingdata/s16"), &root.join("data/scan-truth-s16.json"))
        .unwrap_or_else(|e| panic!("{e}"));
    if frames.is_empty() {
        eprintln!("no labeled frames in trainingdata/s16");
        std::process::exit(1);
    }
    let refs: Vec<_> = frames.iter().map(|(img, t)| (img, t)).collect();
    let bytes = build_templates(&refs).to_bytes();
    let out = root.join("assets").join("s16-templates.bin");
    std::fs::write(&out, &bytes).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
    println!("{} frames -> {} ({} bytes)", frames.len(), out.display(), bytes.len());
}
