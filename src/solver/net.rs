//! A learned value function: a small net trained by tools/train_net.py on the
//! samples of `sampling`, read from its weights file. It maps the inputs of
//! `features::encode` to Riot loot in Chromatics.

use std::sync::OnceLock;

use super::features::{INPUTS, MORNING_INPUTS};

/// Magic bytes at the start of a weights file.
const MAGIC: &[u8; 4] = b"WNET";

/// A net inputs -> h1 -> h2 -> 1 with a clipped ReLU, `clamp(x, 0, 1)`, after each
/// hidden layer and a linear output. `w1[i * h1 + j]` is input `i` to hidden `j`,
/// `w2[j * h2 + k]` is hidden `j` to hidden `k`.
#[derive(Clone, Debug)]
pub struct Net {
    inputs: usize,
    h1: usize,
    h2: usize,
    w1: Vec<f32>,
    b1: Vec<f32>,
    w2: Vec<f32>,
    b2: Vec<f32>,
    w3: Vec<f32>,
    b3: f32,
}

impl Net {
    /// Reads a weights file, little endian: magic `WNET`, u32 inputs, u32 h1, u32 h2,
    /// f32 w1[inputs*h1], b1[h1], w2[h1*h2], b2[h2], w3[h2], b3. `inputs` has to be
    /// `features::INPUTS`, or `features::MORNING_INPUTS` for a net that values mornings only.
    pub fn from_bytes(bytes: &[u8]) -> Result<Net, String> {
        Self::parse(bytes, &[INPUTS, MORNING_INPUTS])
    }

    /// `from_bytes` for a net of one of the `accepted` input counts.
    fn parse(bytes: &[u8], accepted: &[usize]) -> Result<Net, String> {
        if bytes.len() < 16 || &bytes[..4] != MAGIC {
            return Err("not a weights file".into());
        }
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let (inputs, h1, h2) = (word(4) as usize, word(8) as usize, word(12) as usize);
        if !accepted.contains(&inputs) {
            let counts: Vec<String> = accepted.iter().map(|n| n.to_string()).collect();
            return Err(format!("weights file has {inputs} inputs, expected {}", counts.join(" or ")));
        }
        let floats = inputs * h1 + h1 + h1 * h2 + h2 + h2 + 1;
        if bytes.len() != 16 + floats * 4 {
            return Err(format!("weights file is {} bytes, expected {}", bytes.len(), 16 + floats * 4));
        }
        let mut at = 16;
        let mut take = |n: usize| {
            let v: Vec<f32> = (0..n).map(|e| f32::from_bits(word(at + e * 4))).collect();
            at += n * 4;
            v
        };
        let (w1, b1, w2, b2, w3) = (take(inputs * h1), take(h1), take(h1 * h2), take(h2), take(h2));
        let b3 = take(1)[0];
        Ok(Net { inputs, h1, h2, w1, b1, w2, b2, w3, b3 })
    }

    /// The net's input count.
    pub fn inputs(&self) -> usize {
        self.inputs
    }

    /// The net's value of one row of active inputs, in Chromatics.
    pub fn value(&self, row: &[(u16, f32)]) -> f32 {
        let mut a1 = self.b1.clone();
        for &(i, v) in row {
            let i = i as usize;
            debug_assert!(i < self.inputs);
            for (a, w) in a1.iter_mut().zip(&self.w1[i * self.h1..(i + 1) * self.h1]) {
                *a += v * w;
            }
        }
        let mut a2 = self.b2.clone();
        for (j, &a) in a1.iter().enumerate() {
            let a = a.clamp(0.0, 1.0);
            for (b, w) in a2.iter_mut().zip(&self.w2[j * self.h2..(j + 1) * self.h2]) {
                *b += a * w;
            }
        }
        self.b3 + a2.iter().zip(&self.w3).map(|(a, w)| a.clamp(0.0, 1.0) * w).sum::<f32>()
    }
}

/// The net the app's playouts use, trained by tools/train_net.py and embedded at
/// compile time.
pub fn embedded() -> &'static Net {
    static N: OnceLock<Net> = OnceLock::new();
    N.get_or_init(|| {
        Net::from_bytes(include_bytes!("../../assets/s16-net.bin")).expect("assets/s16-net.bin is a weights file")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3 -> 2 -> 2 -> 1 net in the weights file layout.
    fn small() -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        for n in [3u32, 2, 2] {
            bytes.extend_from_slice(&n.to_le_bytes());
        }
        let floats = [
            1.0, -1.0, 0.5, 2.0, -2.0, 0.25, // w1
            0.1, -0.2, // b1
            1.0, -1.0, 0.5, 3.0, // w2
            0.0, 0.1, // b2
            2.0, -1.0,   // w3
            0.5f32, // b3
        ];
        for x in floats {
            bytes.extend_from_slice(&x.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn value_is_the_forward_pass() {
        let net = Net::parse(&small(), &[3]).unwrap();
        // Hidden 1: 0.2, 0.2. Hidden 2: 0.3, 0.5. Output 0.5 + 0.6 - 0.5.
        assert!((net.value(&[(1, 0.2)]) - 0.6).abs() < 1e-6);
        // Hidden 1: 1.3 clamped to 1, -0.4 clamped to 0. Hidden 2: 1, -0.9 clamped to 0.
        assert!((net.value(&[(0, 1.0), (1, 0.4)]) - 2.5).abs() < 1e-6);
        // Hidden 1: 0.1, -0.2 clamped to 0. Hidden 2: 0.1, 0.0. Output 0.5 + 0.2.
        assert!((net.value(&[]) - 0.7).abs() < 1e-6);
    }

    #[test]
    fn bad_weights_files_are_errors() {
        let bytes = small();
        assert!(Net::parse(&bytes[..bytes.len() - 4], &[3]).is_err());
        assert!(Net::parse(&[bytes.clone(), vec![0; 4]].concat(), &[3]).is_err());
        assert!(Net::parse(&bytes, &[4]).is_err());
        assert!(Net::from_bytes(&bytes).is_err());
        let mut wrong = bytes;
        wrong[3] = b'X';
        assert!(Net::parse(&wrong, &[3]).is_err());
    }

    #[test]
    fn embedded_net_parses() {
        assert!(embedded().value(&[]).is_finite());
    }
}
