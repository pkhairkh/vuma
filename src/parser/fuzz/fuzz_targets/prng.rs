//! Hand-written xorshift128+ PRNG for the parser fuzz target.
//! Non-cryptographic — used only for fuzz input generation, not security.
//! A fixed seed makes fuzz runs reproducible (useful for fuzz baselines).
//! Replaces the `rand` crate so the whole repo stays dependency-free.

/// xorshift128+ state. Two 64-bit words; both must not be zero simultaneously
/// (the splitmix64 seeding in `from_seed` guarantees this).
pub struct Prng {
    state: [u64; 2],
}

impl Prng {
    /// Create a PRNG from a single 64-bit seed. Uses splitmix64 to expand
    /// the seed into the two 64-bit state words (which also avoids the
    /// all-zero degenerate state of xorshift128+).
    pub fn from_seed(seed: u64) -> Self {
        let mut sm = seed;
        let s0 = splitmix64(&mut sm);
        let s1 = splitmix64(&mut sm);
        // splitmix64 essentially never emits two consecutive zeros, but
        // defend explicitly: xorshift128+ is undefined at the all-zero
        // state, so fall back to a non-zero state if that ever happens.
        let state = if s0 == 0 && s1 == 0 {
            [0x9E37_79B9_7F4A_7C15, 0xBF58_476D_1CE4_E5B9]
        } else {
            [s0, s1]
        };
        Self { state }
    }

    /// Advance the generator and return the next 64-bit output (xorshift128+).
    pub fn next_u64(&mut self) -> u64 {
        let mut s1 = self.state[0];
        let s0 = self.state[1];
        let result = s0.wrapping_add(s1);
        self.state[0] = s0;
        s1 ^= s1 << 23; // a
        self.state[1] = s1 ^ s0 ^ (s1 >> 18) ^ (s0 >> 5); // b, c
        result
    }

    /// Return a `usize` in `[lo, hi)` — `lo` inclusive, `hi` exclusive.
    /// Degrades gracefully: returns `lo` when `hi <= lo` (empty/inverted range).
    pub fn gen_range(&mut self, lo: usize, hi: usize) -> usize {
        if hi <= lo {
            return lo;
        }
        let range = (hi - lo) as u64;
        let bits = self.next_u64();
        // xorshift128+'s high bits are its best-quality output, so prefer
        // drawing from the top of the word and reduce modulo `range`. Use
        // the full 64-bit word when `range` exceeds 2^32 to avoid bias.
        let v = if range > (1u64 << 32) {
            bits % range
        } else {
            (bits >> 32) % range
        };
        lo + v as usize
    }

    /// Fill `dst` with pseudorandom bytes: 8 bytes at a time via `next_u64`,
    /// then any trailing bytes individually (drawing one more `next_u64`).
    pub fn fill_bytes(&mut self, dst: &mut [u8]) {
        let mut chunks = dst.chunks_exact_mut(8);
        for chunk in chunks.by_ref() {
            let v = self.next_u64();
            chunk.copy_from_slice(&v.to_le_bytes());
        }
        let rem = chunks.into_remainder();
        if !rem.is_empty() {
            let v = self.next_u64();
            let bytes = v.to_le_bytes();
            rem.copy_from_slice(&bytes[..rem.len()]);
        }
    }
}

/// splitmix64 — used by `Prng::from_seed` to expand a single u64 seed into
/// two well-mixed u64 state words (Sebastiano Vigna, 2015).
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
