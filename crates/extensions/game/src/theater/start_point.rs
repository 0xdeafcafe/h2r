//! A start point: the arena and the extension blob at the end of a tick,
//! tagged so a joiner or a player can refuse a changed byte before it
//! inflates anything (docs/game/input-film.md, "Start points").
//!
//! Layout, little endian: magic `H2SP`, version (u8), tick (u32), arena
//! length (u32), extension length (u32), arena hash (u64), identifier length
//! (u8) and the per-game identifiers, stored arena length (u32), stored
//! extension length (u32), the SHA-256 of both stored frames, then the MAC
//! over everything before it, then the stored frames. The frames are zstd.
//!
//! Nothing here reads guest memory or loads an arena. The caller restores
//! through the engine's own path and then compares `lockstep::hash_sim`
//! with [`StartPoint::arena_hash`] ([`StartPoint::check_loaded`]).
use anyhow::{Result, bail, ensure};
use std::io::Read;

pub const MAGIC: [u8; 4] = *b"H2SP";
pub const VERSION: u8 = 1;
/// The most stored bytes of arena a start point holds.
pub const MAX_STORED_ARENA: usize = 1 << 20;
/// The most stored bytes of extension blob a start point holds.
pub const MAX_STORED_EXT: usize = 64 << 10;
/// The most inflated extension blob a start point holds.
pub const MAX_EXT: usize = 1 << 20;
/// The most bytes of per-game identifiers.
pub const MAX_IDS: usize = 64;

/// The state a film that begins mid-game starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartPoint {
    /// The tick whose end the state is.
    pub tick: u32,
    /// `lockstep::hash_sim` of the arena and the extension records.
    pub arena_hash: u64,
    /// Per-game identifiers; meaningless outside the game.
    pub ids: Vec<u8>,
    pub arena: Vec<u8>,
    pub ext: Vec<u8>,
}

impl StartPoint {
    /// The state loaded; its hash must be the one the tag names.
    pub fn check_loaded(&self, hash_after_load: u64) -> Result<()> {
        ensure!(
            hash_after_load == self.arena_hash,
            "start point arena hash {:#018x} differs from the loaded state's {hash_after_load:#018x}",
            self.arena_hash
        );
        Ok(())
    }

    /// Tag and compress. The arena must be `arena_len` bytes, which the
    /// reader checks against its own.
    pub fn encode(&self, key: &[u8]) -> Result<Vec<u8>> {
        ensure!(self.ids.len() <= MAX_IDS, "start point identifiers too long");
        ensure!(self.ext.len() <= MAX_EXT, "start point extension blob too large");
        let arena = zstd::bulk::compress(&self.arena, 3)?;
        let ext = zstd::bulk::compress(&self.ext, 3)?;
        ensure!(arena.len() <= MAX_STORED_ARENA, "start point arena stores too large");
        ensure!(ext.len() <= MAX_STORED_EXT, "start point extension blob stores too large");
        let mut out = Vec::with_capacity(96 + self.ids.len() + arena.len() + ext.len());
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.tick.to_le_bytes());
        out.extend_from_slice(&u32::try_from(self.arena.len())?.to_le_bytes());
        out.extend_from_slice(&u32::try_from(self.ext.len())?.to_le_bytes());
        out.extend_from_slice(&self.arena_hash.to_le_bytes());
        out.push(u8::try_from(self.ids.len())?);
        out.extend_from_slice(&self.ids);
        out.extend_from_slice(&u32::try_from(arena.len())?.to_le_bytes());
        out.extend_from_slice(&u32::try_from(ext.len())?.to_le_bytes());
        let mut frames = arena;
        frames.extend_from_slice(&ext);
        out.extend_from_slice(&sha256(&frames));
        let mac = hmac_sha256(key, &out);
        out.extend_from_slice(&mac);
        out.extend_from_slice(&frames);
        Ok(out)
    }

    /// Check the tag, then inflate into buffers of exactly the stated sizes.
    /// `arena_len` is the reader's own arena size and `film_hash` the arena
    /// hash the film names for this tick; either differing is a refusal.
    /// Nothing is inflated before the MAC and the digest are right.
    pub fn decode(bytes: &[u8], key: &[u8], arena_len: usize, film_hash: u64) -> Result<Self> {
        let mut at = Cursor { bytes, at: 0 };
        ensure!(at.take(4)? == MAGIC, "not a start point");
        ensure!(at.take(1)?[0] == VERSION, "start point version is not supported");
        let tick = at.u32()?;
        let arena_n = at.u32()? as usize;
        let ext_n = at.u32()? as usize;
        let arena_hash = at.u64()?;
        let ids_n = usize::from(at.take(1)?[0]);
        ensure!(ids_n <= MAX_IDS, "start point identifiers too long");
        let ids = at.take(ids_n)?.to_vec();
        let arena_stored = at.u32()? as usize;
        let ext_stored = at.u32()? as usize;
        let sha: [u8; 32] = at.take(32)?.try_into()?;
        let signed = at.at;
        let mac: [u8; 32] = at.take(32)?.try_into()?;
        ensure!(
            equal(&hmac_sha256(key, &bytes[..signed]), &mac),
            "start point tag does not match"
        );
        ensure!(arena_stored <= MAX_STORED_ARENA, "start point arena stores too large");
        ensure!(ext_stored <= MAX_STORED_EXT, "start point extension blob stores too large");
        ensure!(arena_n == arena_len, "start point arena is not this build's size");
        ensure!(ext_n <= MAX_EXT, "start point extension blob too large");
        let arena_frame = at.take(arena_stored)?;
        let ext_frame = at.take(ext_stored)?;
        ensure!(at.at == bytes.len(), "start point has trailing bytes");
        ensure!(
            equal(&sha256(&bytes[bytes.len() - arena_stored - ext_stored..]), &sha),
            "start point bytes differ from their digest"
        );
        ensure!(
            arena_hash == film_hash,
            "start point arena hash is not the film's"
        );
        Ok(Self {
            tick,
            arena_hash,
            ids,
            arena: inflate_exact(arena_frame, arena_n)?,
            ext: inflate_exact(ext_frame, ext_n)?,
        })
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let Some(end) = self.at.checked_add(n).filter(|&e| e <= self.bytes.len()) else {
            bail!("start point is cut short");
        };
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
}

/// One zstd frame that inflates to exactly `len` bytes and ends with its
/// input.
fn inflate_exact(frame: &[u8], len: usize) -> Result<Vec<u8>> {
    let mut decoder = zstd::stream::read::Decoder::new(frame)?.single_frame();
    let mut out = Vec::with_capacity(len);
    decoder.by_ref().take(len as u64 + 1).read_to_end(&mut out)?;
    ensure!(out.len() == len, "start point frame does not inflate to its stated size");
    let rest = decoder.finish();
    ensure!(
        rest.buffer().is_empty() && rest.get_ref().is_empty(),
        "start point frame has extra bytes"
    );
    Ok(out)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let hex = film::xbe_digest(bytes);
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("sha-256 hex");
    }
    out
}

/// RFC 2104 over SHA-256.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&sha256(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = block.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(message);
    let mut outer: Vec<u8> = block.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&sha256(&inner));
    sha256(&outer)
}

fn equal(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"synthetic tunnel key";

    fn point() -> StartPoint {
        StartPoint {
            tick: 1234,
            arena_hash: 0x1122_3344_5566_7788,
            ids: vec![7; 12],
            arena: (0..4096u32).map(|i| (i / 7) as u8).collect(),
            ext: b"synthetic extension records".to_vec(),
        }
    }

    #[test]
    fn hmac_matches_rfc_4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn round_trips() {
        let p = point();
        let bytes = p.encode(KEY).unwrap();
        let back = StartPoint::decode(&bytes, KEY, 4096, p.arena_hash).unwrap();
        assert_eq!(back, p);
        back.check_loaded(p.arena_hash).unwrap();
        assert!(back.check_loaded(p.arena_hash ^ 1).is_err());
    }

    #[test]
    fn any_changed_byte_is_refused() {
        let p = point();
        let bytes = p.encode(KEY).unwrap();
        for i in 0..bytes.len() {
            let mut bad = bytes.clone();
            bad[i] ^= 1;
            assert!(
                StartPoint::decode(&bad, KEY, 4096, p.arena_hash).is_err(),
                "byte {i} changed and was accepted"
            );
        }
    }

    #[test]
    fn truncation_and_trailing_bytes_are_refused() {
        let p = point();
        let bytes = p.encode(KEY).unwrap();
        for n in 0..bytes.len() {
            assert!(StartPoint::decode(&bytes[..n], KEY, 4096, p.arena_hash).is_err());
        }
        let mut long = bytes;
        long.push(0);
        assert!(StartPoint::decode(&long, KEY, 4096, p.arena_hash).is_err());
    }

    #[test]
    fn another_key_size_or_film_hash_is_refused() {
        let p = point();
        let bytes = p.encode(KEY).unwrap();
        assert!(StartPoint::decode(&bytes, b"other", 4096, p.arena_hash).is_err());
        assert!(StartPoint::decode(&bytes, KEY, 4095, p.arena_hash).is_err());
        let err = StartPoint::decode(&bytes, KEY, 4096, p.arena_hash ^ 1).unwrap_err();
        assert!(err.to_string().contains("not the film's"));
    }

    #[test]
    fn a_tagged_frame_that_inflates_wrong_is_refused() {
        // A holder of the key lying about the length is still caught.
        let mut p = point();
        p.arena.truncate(100);
        let bytes = p.encode(KEY).unwrap();
        assert!(StartPoint::decode(&bytes, KEY, 4096, p.arena_hash).is_err());
        assert!(inflate_exact(&zstd::bulk::compress(&[1; 10], 3).unwrap(), 9).is_err());
        assert!(inflate_exact(&zstd::bulk::compress(&[1; 10], 3).unwrap(), 11).is_err());
    }

    #[test]
    fn oversized_identifiers_and_blobs_are_not_written() {
        let mut p = point();
        p.ids = vec![0; MAX_IDS + 1];
        assert!(p.encode(KEY).is_err());
        let mut p = point();
        p.ext = vec![0; MAX_EXT + 1];
        assert!(p.encode(KEY).is_err());
    }
}
