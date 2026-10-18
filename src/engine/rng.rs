//! Seeded randomness, bit for bit as js/engine.js (lines 314-352).

/// FNV-1a over the code points of `text` (hashSeed).
pub fn hash_seed(text: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for ch in text.chars() {
        h ^= ch as u32;
        h = h.wrapping_mul(16777619);
    }
    h
}

/// mulberry32 (rand): advances `stream` and returns a number in [0, 1).
pub fn rand(stream: &mut u32) -> f64 {
    *stream = stream.wrapping_add(0x6d2b79f5);
    let mut t = *stream;
    t = (t ^ (t >> 15)).wrapping_mul(t | 1);
    t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
    f64::from(t ^ (t >> 14)) / 4294967296.0
}

/// pickIndex: an index drawn by weight; negative weights count as 0.
pub fn pick_index(stream: &mut u32, weights: &[f64]) -> usize {
    let total: f64 = weights.iter().fold(0.0, |sum, w| sum + w.max(0.0));
    if total <= 0.0 {
        return 0;
    }
    let mut r = rand(stream) * total;
    for (i, w) in weights.iter().enumerate() {
        r -= w.max(0.0);
        if r < 0.0 {
            return i;
        }
    }
    weights.len() - 1
}

/// pickOne, as an index into a list of `len` items.
pub fn pick_one(stream: &mut u32, len: usize) -> usize {
    (rand(stream) * len as f64).floor() as usize
}

/// shuffle: Fisher-Yates from the back, in place.
pub fn shuffle<T>(stream: &mut u32, list: &mut [T]) {
    for i in (1..list.len()).rev() {
        let j = (rand(stream) * (i + 1) as f64).floor() as usize;
        list.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_stream_match_js() {
        // Values from the JS hashSeed and rng32 under Node.
        let mut s = hash_seed("TRACE-0001|empty");
        assert_eq!(s, 3278519699);
        assert_eq!(rand(&mut s), 0.18879732862114906);
        assert_eq!(rand(&mut s), 0.5664459567051381);
        assert_eq!(rand(&mut s), 0.7377160913310945);
    }
}
