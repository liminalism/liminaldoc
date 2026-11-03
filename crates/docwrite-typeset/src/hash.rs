//! A multiply-and-rotate hasher for small integer keys.
//!
//! Glyph atlas keys and paragraph ids are looked up thousands of times per
//! keystroke; SipHash on a key of a few integers was a measurable share of
//! both painting a page and matching paragraphs, and neither map is ever
//! exposed to hostile keys.

use std::collections::HashMap;
use std::hash::BuildHasherDefault;

/// The hasher.
#[derive(Debug, Default, Clone, Copy)]
pub struct SmallKeyHasher(u64);

impl std::hash::Hasher for SmallKeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_u64(u64::from(*byte));
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    fn write_u16(&mut self, value: u16) {
        self.write_u64(u64::from(value));
    }

    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
}

/// A map keyed by small integers.
pub type SmallKeyMap<K, V> = HashMap<K, V, BuildHasherDefault<SmallKeyHasher>>;
