//! Utilities for generating pseudo-random data.

use rand_pcg::Pcg32;

/// Create a new pseudo-random number generator with a fixed seed.
pub fn new_prng() -> Pcg32 {
    Pcg32::new(1337, 420)
}
