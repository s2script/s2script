//! A cheap hasher for maps whose keys the host makes up itself: plugin ids, small integers, enum
//! tags. The standard library's default resists hash flooding, which costs a few hundred
//! instructions a probe. These maps are probed several times on every dispatch, and nothing a
//! client sends chooses their keys (a plugin id comes from an installed plugin's manifest).
//!
//! Engine-generic: nothing here knows a game.
use std::hash::{BuildHasherDefault, Hasher};

#[derive(Default)]
pub(crate) struct WordHasher(u64);

const K: u64 = 0x9e37_79b9_7f4a_7c15;

impl Hasher for WordHasher {
    fn finish(&self) -> u64 {
        // The table takes its bucket from the low bits and its tag from the high ones.
        (self.0 ^ (self.0 >> 32)).wrapping_mul(K)
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            self.write_u64(u64::from_le_bytes(chunk.try_into().unwrap()));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut word = [0u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.write_u64(u64::from_le_bytes(word) ^ ((rest.len() as u64) << 56));
        }
    }
    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }
    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }
    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(K);
    }
    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
}

pub(crate) type FastState = BuildHasherDefault<WordHasher>;
pub(crate) type FastMap<K, V> = std::collections::HashMap<K, V, FastState>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_that_differ_only_in_their_tail_or_length_land_apart() {
        let hash = |s: &str| {
            let mut h = WordHasher::default();
            std::hash::Hash::hash(s, &mut h);
            h.finish()
        };
        let ids = ["a", "b", "ab", "ba", "basechat", "basecomm", "basecommands", "basecommandsx", "@edgegamers/ttt", "@edgegamers/jb", ""];
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert_ne!(hash(a), hash(b), "{a} and {b}");
            }
        }
    }

    #[test]
    fn a_map_of_plugin_ids_finds_every_key() {
        let mut map: FastMap<String, usize> = FastMap::default();
        for i in 0..500 {
            map.insert(format!("@scope/plugin-{i}"), i);
        }
        for i in 0..500 {
            assert_eq!(map.get(format!("@scope/plugin-{i}").as_str()), Some(&i));
        }
        assert_eq!(map.get("@scope/plugin-500"), None);
    }
}
