//! Gestalt merge: a `snd!` moved from one map to another takes its `ugh!`
//! entries with it.
//!
//! A Halo 2 `snd!` is 20 bytes of indices into the map's one sound gestalt.
//! [`merge_sound`] reads the entries a source `snd!` names in the source
//! gestalt and appends them to the destination one, the way
//! [`crate::ConvertedSound::add_to`] appends a converted sound:
//!
//! | `snd!` field | Copied |
//! |---|---|
//! | playback, scale | deduplicated against the destination |
//! | pitch ranges | appended in order, with their name and parameters (deduplicated) and a fresh run of runtime permutation flag bits |
//! | permutations | appended in order, with their name and chunks |
//! | chunks | appended; the caller maps each pointer (copies the raw) |
//! | custom playback, extra info | appended whole (nested blocks); the extra info's owner becomes the caller's datum |
//! | promotion | refused: its rules hold indices this module doesn't map |
//!
//! Import names are string IDs; the caller maps them from the source map's
//! string table into the destination's.

use crate::convert::snd;
use crate::gestalt::{Chunk, ChunkPointer, SoundGestalt, blocks};
use crate::{Error, Result};

/// An extra info's `snd!` owner datum.
const EXTRA_OWNER: usize = 0x20;
/// An extra info's raw pointer.
const EXTRA_RAW: usize = 8;

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn put_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn index16(what: &str, n: usize) -> Result<u16> {
    u16::try_from(n)
        .ok()
        .filter(|&v| v != u16::MAX)
        .ok_or_else(|| Error::Unsupported(format!("{what} index {n} doesn't fit 16 bits")))
}

fn index8(what: &str, n: usize) -> Result<u8> {
    u8::try_from(n)
        .ok()
        .filter(|&v| v != u8::MAX)
        .ok_or_else(|| Error::Unsupported(format!("{what} index {n} doesn't fit 8 bits")))
}

fn get<'a>(g: &'a SoundGestalt, i: usize, n: usize, what: &str) -> Result<&'a [u8]> {
    if n >= g.count(i) {
        return Err(Error::Invalid(format!(
            "snd!: {what} {n} past the source gestalt's {}",
            g.count(i)
        )));
    }
    Ok(g.element(i, n))
}

/// How [`merge_sound`] maps what lives outside the gestalt.
pub trait MergeMap {
    /// A source import name (string ID) as a destination one.
    fn name(&mut self, id: u32) -> Result<u32>;
    /// A source chunk or extra-info raw pointer of `size` bytes (flag bit
    /// cleared) as a destination one; usually copies the raw.
    fn raw(&mut self, pointer: ChunkPointer, size: u32) -> Result<ChunkPointer>;
}

/// Appends the gestalt entries `header` (a source `snd!`, at least
/// [`snd::LEN`] bytes) names in `src` to `dst`, and returns the `snd!`
/// with its indices rewritten for `dst`. `owner` is the datum the moved
/// `snd!` has in the destination (written into its extra info).
pub fn merge_sound(
    src: &SoundGestalt,
    header: &[u8],
    dst: &mut SoundGestalt,
    owner: u32,
    map: &mut dyn MergeMap,
) -> Result<Vec<u8>> {
    if header.len() < snd::LEN {
        return Err(Error::Invalid(format!("snd!: {} bytes", header.len())));
    }
    let mut out = header.to_vec();
    if header[snd::PROMOTION] != 0xff {
        return Err(Error::Unsupported(
            "snd!: sound promotions are not merged".into(),
        ));
    }

    let playback = u16_at(header, snd::PLAYBACK) as usize;
    let e = get(src, blocks::PLAYBACKS, playback, "playback")?.to_vec();
    let n = dst.push(blocks::PLAYBACKS, &e, true);
    put_u16(&mut out, snd::PLAYBACK, index16("playback", n)?);

    let scale = header[snd::SCALE] as usize;
    let e = get(src, blocks::SCALES, scale, "scale")?.to_vec();
    let n = dst.push(blocks::SCALES, &e, true);
    out[snd::SCALE] = index8("scale", n)?;

    let name = |dst: &mut SoundGestalt, map: &mut dyn MergeMap, n: u16| -> Result<u16> {
        let id = u32_at(
            get(src, blocks::IMPORT_NAMES, n as usize, "import name")?,
            0,
        );
        let id = map.name(id)?;
        index16(
            "import name",
            dst.push(blocks::IMPORT_NAMES, &id.to_le_bytes(), true),
        )
    };
    let first = u16_at(header, snd::FIRST_PITCH_RANGE) as usize;
    let count = header[snd::PITCH_RANGE_COUNT] as usize;
    let first_out = dst.count(blocks::PITCH_RANGES);
    for r in first..first + count {
        let range = get(src, blocks::PITCH_RANGES, r, "pitch range")?.to_vec();
        let mut e = range.clone();
        put_u16(&mut e, 0, name(dst, map, u16_at(&range, 0))?);
        let params = get(
            src,
            blocks::PITCH_RANGE_PARAMETERS,
            u16_at(&range, 2) as usize,
            "pitch range parameters",
        )?
        .to_vec();
        let params = dst.push(blocks::PITCH_RANGE_PARAMETERS, &params, true);
        put_u16(&mut e, 2, index16("pitch range parameters", params)?);
        let (perm0, perms) = (u16_at(&range, 8) as usize, u16_at(&range, 10) as usize);
        if u16_at(&range, 4) == 0xffff {
            // A run of flag bits, one per permutation, from a bit offset.
            let from = u16_at(&range, 6) as usize;
            let flags = &src.flat[blocks::RUNTIME_PERMUTATION_FLAGS];
            if (from + perms).div_ceil(8) > flags.len() {
                return Err(Error::Invalid(format!(
                    "snd!: runtime permutation flags {from}+{perms} past the source's"
                )));
            }
            let at = dst.flat[blocks::RUNTIME_PERMUTATION_FLAGS].len();
            let mut bits = vec![0u8; perms.div_ceil(8)];
            for b in 0..perms {
                let bit = flags[(from + b) / 8] >> ((from + b) % 8) & 1;
                bits[b / 8] |= bit << (b % 8);
            }
            dst.flat[blocks::RUNTIME_PERMUTATION_FLAGS].extend(bits);
            put_u16(&mut e, 6, index16("runtime permutation flag", at * 8)?);
        }
        put_u16(
            &mut e,
            8,
            index16("permutation", dst.count(blocks::PERMUTATIONS))?,
        );
        for p in perm0..perm0 + perms {
            let perm = get(src, blocks::PERMUTATIONS, p, "permutation")?.to_vec();
            let mut pe = perm.clone();
            put_u16(&mut pe, 0, name(dst, map, u16_at(&perm, 0))?);
            let (chunk0, chunks) = (u16_at(&perm, 12) as usize, u16_at(&perm, 14) as usize);
            put_u16(&mut pe, 12, index16("chunk", dst.chunks.len())?);
            for c in chunk0..chunk0 + chunks {
                let chunk = *src.chunks.get(c).ok_or_else(|| {
                    Error::Invalid(format!("snd!: chunk {c} past the source gestalt's"))
                })?;
                let pointer = map.raw(chunk.pointer, chunk.size & 0x7fff_ffff)?;
                dst.chunks.push(Chunk { pointer, ..chunk });
            }
            dst.push(blocks::PERMUTATIONS, &pe, false);
        }
        dst.push(blocks::PITCH_RANGES, &e, false);
    }
    put_u16(
        &mut out,
        snd::FIRST_PITCH_RANGE,
        index16("pitch range", first_out)?,
    );

    let custom = header[snd::CUSTOM_PLAYBACK];
    if custom != 0xff {
        let e = src.nested[blocks::CUSTOM_PLAYBACKS]
            .get(custom as usize)
            .ok_or_else(|| Error::Invalid(format!("snd!: custom playback {custom}")))?
            .clone();
        dst.nested[blocks::CUSTOM_PLAYBACKS].push(e);
        out[snd::CUSTOM_PLAYBACK] =
            index8("custom playback", dst.count(blocks::CUSTOM_PLAYBACKS) - 1)?;
    }

    let extra = u16_at(header, snd::EXTRA_INFO);
    if extra != 0xffff {
        let mut e = src.nested[blocks::EXTRA_INFOS]
            .get(extra as usize)
            .ok_or_else(|| Error::Invalid(format!("snd!: extra info {extra}")))?
            .clone();
        let p = u32_at(&e.bytes, EXTRA_RAW);
        if p != 0 && p != u32::MAX {
            let size = u32_at(&e.bytes, EXTRA_RAW + 4);
            match map.raw(ChunkPointer::Stored(p), size)? {
                ChunkPointer::Stored(v) => {
                    e.bytes[EXTRA_RAW..EXTRA_RAW + 4].copy_from_slice(&v.to_le_bytes())
                }
                ChunkPointer::Local(_) => {
                    return Err(Error::Unsupported(
                        "snd!: an extra info's raw can only stay where it is".into(),
                    ));
                }
            }
        }
        e.bytes[EXTRA_OWNER..EXTRA_OWNER + 4].copy_from_slice(&owner.to_le_bytes());
        dst.nested[blocks::EXTRA_INFOS].push(e);
        put_u16(
            &mut out,
            snd::EXTRA_INFO,
            index16("extra info", dst.count(blocks::EXTRA_INFOS) - 1)?,
        );
    }
    Ok(out)
}
