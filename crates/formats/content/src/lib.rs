//! Map packages (docs/game/maps.md): a map is a base plus layers.
//!
//! - [`Manifest`]: a package's `manifest.toml`.
//! - [`MapId`]: the identity of a map, the SHA-256 of its manifest's
//!   identity in canonical JSON.
//! - [`patch`]: PPF 1/2/3, IPS and BPS readers; offset patches become
//!   [`patch::Overlay`]s laid over the base as it streams.
//! - [`Library`]: the packages under `<data>/maps/`, and the resolver that
//!   turns a map name into the bytes to serve.
//! - [`store`]: writing packages and `active.toml` (the editor's side).
//! - [`campaign_list`]: campaigns past Halo 2's own, as 1.0's campaign level
//!   list records (docs/halo1/campaign-picker.md).
//! - [`map_pack`]: the Multiplayer Map Pack's maps, which packages can
//!   take as their base.
//! - [`patch_lvl`] and [`map_list`]: 1.0's multiplayer map list entries,
//!   read from the Map Pack and built for new maps.
//!
//! Nothing here holds game data: packages name the user's disc maps by name
//! and hash, and the tests use synthetic maps.
pub mod campaign_list;
mod canonical;
mod hash;
mod id;
pub mod manifest;
pub mod map_list;
pub mod map_pack;
pub mod patch;
pub mod patch_lvl;
mod resolve;
pub mod store;

pub use canonical::{NotInteger, canonicalize};
pub use hash::{BadHex, Hasher, Sha256};
pub use id::MapId;
pub use manifest::{HudSwap, LevelHud, Manifest, ManifestError, Origin, Swap, Variant};
pub use resolve::{
    ACTIVE_FILE, BaseRef, CACHE_DIR, DEFAULT_CACHE_CAP, Data, DiscHashes, Library, Package,
    ResolveError, Resolved, repair_checksum,
};

#[cfg(test)]
pub(crate) mod testutil {
    /// A tiny deterministic generator (xorshift64*), so failures replay.
    pub(crate) struct Rng(u64);
    impl Rng {
        pub(crate) fn new(seed: u64) -> Self {
            Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
        }
        pub(crate) fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        pub(crate) fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }
        pub(crate) fn bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| self.next() as u8).collect()
        }
    }
}
