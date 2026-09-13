//! Deterministic seeded PRNG (SplitMix64/xoshiro256**) for tests, simulation and fuzzing.
//!
//! No OS entropy is ever read here; every consumer must pass an explicit seed
//! (SPEC-010 §6: all simulated nondeterminism enters through an explicit environment).

#[derive(Debug, Clone)]
pub struct DetRng {
    s: [u64; 4],
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl DetRng {
    pub fn new(seed: u64) -> Self {
        let mut st = seed;
        let s = [
            splitmix64(&mut st),
            splitmix64(&mut st),
            splitmix64(&mut st),
            splitmix64(&mut st),
        ];
        DetRng { s }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in `0..n` (n > 0).
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0);
        // rejection sampling to avoid modulo bias
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let v = self.next_u64();
            if v < zone {
                return v % n;
            }
        }
    }

    pub fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        assert!(lo <= hi);
        let span = (hi as i128 - lo as i128 + 1) as u64;
        lo.wrapping_add(self.below(span) as i64)
    }

    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.below(den) < num
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(n);
        while v.len() < n {
            let x = self.next_u64().to_le_bytes();
            for b in x {
                if v.len() < n {
                    v.push(b);
                }
            }
        }
        v
    }

    pub fn fork(&mut self) -> DetRng {
        DetRng::new(self.next_u64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic() {
        let mut a = DetRng::new(42);
        let mut b = DetRng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut c = DetRng::new(43);
        assert_ne!(a.next_u64(), c.next_u64());
    }
    #[test]
    fn below_in_range() {
        let mut r = DetRng::new(1);
        for _ in 0..1000 {
            assert!(r.below(7) < 7);
            let x = r.range_i64(-3, 3);
            assert!((-3..=3).contains(&x));
        }
    }
}
