//! Tiny deterministic PRNG used by tests and the host benchmark so that
//! randomised allocation sequences are reproducible from a seed.

#[derive(Clone, Debug)]
pub struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    pub const fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9e37_79b9_7f4a_7c15
            } else {
                seed
            },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    pub fn next_usize(&mut self, bound: usize) -> usize {
        debug_assert!(bound > 0);
        (self.next_u64() % bound as u64) as usize
    }

    pub fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_deterministic_for_a_seed() {
        let mut a = XorShift64::new(42);
        let mut b = XorShift64::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn never_returns_zero_state() {
        let mut rng = XorShift64::new(0);
        for _ in 0..16 {
            assert_ne!(rng.next_u64(), 0);
        }
    }

    #[test]
    fn bounds_are_respected() {
        let mut rng = XorShift64::new(7);
        for _ in 0..1000 {
            assert!(rng.next_usize(13) < 13);
        }
    }
}
