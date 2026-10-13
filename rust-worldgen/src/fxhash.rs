//! A fast non-cryptographic hasher (rustc's FxHash) for hash sets that are
//! only used for membership. Iteration order differs from the standard
//! hasher's, so it must not be used where a set or map is iterated.
use std::hash::{BuildHasherDefault, Hasher};

#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) { self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(SEED); }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks { self.add(u64::from_le_bytes(c.try_into().unwrap())); }
        for &b in chunks.remainder() { self.add(b as u64); }
    }
    #[inline] fn write_u8(&mut self, i: u8) { self.add(i as u64); }
    #[inline] fn write_u32(&mut self, i: u32) { self.add(i as u64); }
    #[inline] fn write_i32(&mut self, i: i32) { self.add(i as u32 as u64); }
    #[inline] fn write_u64(&mut self, i: u64) { self.add(i); }
    #[inline] fn write_i64(&mut self, i: i64) { self.add(i as u64); }
    #[inline] fn write_usize(&mut self, i: usize) { self.add(i as u64); }
    #[inline] fn finish(&self) -> u64 { self.0 }
}

/// A membership-only hash set (see the module documentation).
pub type FxHashSet<T> = std::collections::HashSet<T, BuildHasherDefault<FxHasher>>;
