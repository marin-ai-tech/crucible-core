//! Reproducible simulation randomness with a seed and named ChaCha8 streams.

use std::collections::HashMap;

pub use rand_chacha::ChaCha8Rng;
pub use rand_chacha::rand_core::{RngCore, SeedableRng};

/// World resource providing named streams to simulation systems.
/// ([`WorldTimeExt::rng_stream`](crate::WorldTimeExt::rng_stream)).
///
/// Stream `name` uses ChaCha8 with the simulation seed and the FNV-1a hash of the name
/// as its ChaCha stream number. Streams are independent and unaffected by their creation order.
/// The same seed and calls to a stream reproduce the same sequence.
pub struct SimRng {
    seed: u64,
    streams: HashMap<String, ChaCha8Rng>,
}

impl SimRng {
    /// Randomness initialized with an explicit seed.
    pub fn new(seed: u64) -> Self {
        SimRng {
            seed,
            streams: HashMap::new(),
        }
    }

    /// Simulation seed.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Named random stream.
    pub fn stream(&mut self, name: &str) -> &mut ChaCha8Rng {
        let seed = self.seed;
        self.streams
            .entry(name.to_owned())
            .or_insert_with(|| derive_stream(seed, name))
    }
}

fn derive_stream(seed: u64, name: &str) -> ChaCha8Rng {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    rng.set_stream(fnv1a64(name.as_bytes()));
    rng
}

/// FNV-1a with 64-bit output is stable across compiler versions and platforms (unlike
/// `DefaultHasher`).
pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// If configuration omits the seed, derive it from system time and process ID.
pub(crate) fn fresh_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut bytes = nanos.to_le_bytes().to_vec();
    bytes.extend_from_slice(&std::process::id().to_le_bytes());
    fnv1a64(&bytes)
}
