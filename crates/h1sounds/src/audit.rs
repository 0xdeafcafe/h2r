//! Every sound of a built map, read back the way the engine finds it and
//! compared with where it came from.
//!
//! The engine plays a `snd!` by following its indices into the map's `ugh!`
//! (pitch ranges, permutations, chunks) and streaming each permutation's
//! chunks in order. [`audit`] does the same for every `snd!` in a built map
//! and checks, for each:
//!
//! - the indices land inside their blocks, every pitch range has
//!   permutations and every permutation chunks;
//! - a permutation's size is the sum of its chunks, a whole number of
//!   blocks, and its chunks lie inside the file;
//! - a sound converted from Halo 1 (found by name in the Halo 1 map) has the
//!   header, playback and scale [`convert_sound`] makes, one permutation per
//!   Halo 1 chain, and exactly that chain's bytes: so it plays as long as it
//!   did in Halo 1;
//! - any other sound is the retail sound of the same name in one of the
//!   source maps (the base, a donor): the same header fields, playback,
//!   scale, pitch range parameters, permutations and bytes.
//!
//! A sound whose chunks point at the wrong bytes, or at too few, plays as a
//! short buzz that repeats; this finds it without running the game.

use std::collections::HashMap;
use std::io::Cursor;

use h1map::H1Map;
use map::{FourCC, Map};

use crate::convert::snd;
use crate::gestalt::{ChunkPointer, SoundGestalt, UGH, blocks};
use crate::h1::SND;
use crate::{Result, convert_sound};

/// What [`audit`] found.
#[derive(Debug, Default, Clone)]
pub struct Audit {
    /// `snd!` tags in the built map.
    pub sounds: usize,
    /// Sounds matched to a Halo 1 sound and checked against it.
    pub converted: usize,
    /// Sounds matched to a retail sound and checked against it.
    pub retail: usize,
    /// Permutations read back.
    pub permutations: usize,
    /// Seconds of converted Halo 1 sound read back.
    pub converted_seconds: f64,
    /// Sounds with no source of the same name.
    pub unmatched: Vec<String>,
    /// Everything that differs from its source or can't play as stored.
    pub problems: Vec<String>,
}

/// A permutation's samples as the built map stores them.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Data {
    /// Every chunk is in this map: the bytes, in order.
    Local(Vec<u8>),
    /// Some chunk is in another file (`shared`, `single_player_shared`):
    /// each chunk's stored pointer and size.
    Elsewhere(Vec<(u32, u32)>),
}

/// One permutation: bytes `+2..+8` of the element (name index left out),
/// its size, chunk count and samples.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Perm {
    raw: [u8; 6],
    size: u32,
    chunks: usize,
    data: Data,
}

/// One `snd!` read through the gestalt.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Read {
    header: [u8; snd::LEN],
    playback: Option<Vec<u8>>,
    scale: Option<Vec<u8>>,
    /// Per range: its parameters element and permutations.
    ranges: Vec<(Option<Vec<u8>>, Vec<Perm>)>,
}

fn u16_at(b: &[u8], at: usize) -> usize {
    usize::from(u16::from_le_bytes([b[at], b[at + 1]]))
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

/// Bytes per block, or `None` when not Xbox ADPCM or PCM in one or two
/// channels.
fn block_len(header: &[u8]) -> Option<usize> {
    let channels = match header[snd::ENCODING] {
        0 => 1,
        1 => 2,
        _ => return None,
    };
    match header[snd::COMPRESSION] {
        1 => Some(36 * channels),
        0 | 3 => Some(2 * channels),
        _ => None,
    }
}

/// Every `snd!` of `bytes` (a whole map file) by name, read through its
/// gestalt; what can't be read goes to `problems`, prefixed with `label`.
fn read_map(
    bytes: &[u8],
    label: &str,
    problems: &mut Vec<String>,
) -> Result<HashMap<String, Read>> {
    let mut m = Map::from_reader(Cursor::new(bytes))?;
    let mut out = HashMap::new();
    let Some(t) = m.tags().iter().find(|t| t.group == UGH).cloned() else {
        return Ok(out);
    };
    let Some(loc) = t.location else {
        return Ok(out);
    };
    let g = SoundGestalt::decode(&m.read_tag_bytes(t.id)?, loc.address)?;
    let element = |i: usize, n: usize| (n < g.count(i)).then(|| g.element(i, n).to_vec());
    let ids: Vec<_> = m
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"snd!") && t.location.is_some())
        .map(|t| (t.id, t.name.clone().unwrap_or_default()))
        .collect();
    for (id, name) in ids {
        let b = m.read_tag_bytes(id)?;
        let mut bad = |what: String| problems.push(format!("{label}: {name}: {what}"));
        if b.len() < snd::LEN {
            bad(format!("snd! is {} bytes", b.len()));
            continue;
        }
        let header: [u8; snd::LEN] = b[..snd::LEN].try_into().expect("20 bytes");
        let block = block_len(&header);
        let first = u16_at(&header, snd::FIRST_PITCH_RANGE);
        let n = usize::from(header[snd::PITCH_RANGE_COUNT]);
        if n == 0 {
            bad("no pitch ranges".into());
        }
        if first + n > g.count(blocks::PITCH_RANGES) {
            bad(format!("pitch ranges {first}+{n} past the gestalt"));
            continue;
        }
        let mut ranges = Vec::with_capacity(n);
        for r in first..first + n {
            let e = g.element(blocks::PITCH_RANGES, r);
            let params = u16_at(e, 2);
            let (fp, np) = (u16_at(e, 8), u16_at(e, 10));
            if np == 0 {
                bad(format!("pitch range {} has no permutations", r - first));
            }
            if fp + np > g.count(blocks::PERMUTATIONS) {
                bad(format!("permutations {fp}+{np} past the gestalt"));
                ranges.push((None, Vec::new()));
                continue;
            }
            let mut perms = Vec::with_capacity(np);
            for p in fp..fp + np {
                let e = g.element(blocks::PERMUTATIONS, p);
                let (size, fc, nc) = (u32_at(e, 8), u16_at(e, 12), u16_at(e, 14));
                let at = format!("range {} permutation {}", r - first, p - fp);
                if nc == 0 {
                    bad(format!("{at} has no chunks"));
                }
                if let Some(block) = block
                    && !(size as usize).is_multiple_of(block)
                {
                    bad(format!(
                        "{at}: {size} bytes is not whole {block}-byte blocks"
                    ));
                }
                let Some(chunks) = g.chunks.get(fc..fc + nc) else {
                    bad(format!("{at}: chunks {fc}+{nc} past the gestalt"));
                    continue;
                };
                let sum: u64 = chunks.iter().map(|c| u64::from(c.size & 0x7fff_ffff)).sum();
                if sum != u64::from(size) {
                    bad(format!("{at}: size {size}, its chunks {sum}"));
                }
                let mut local = Some(Vec::with_capacity(size as usize));
                let mut stored = Vec::with_capacity(nc);
                for c in chunks {
                    let len = c.size & 0x7fff_ffff;
                    let ChunkPointer::Stored(ptr) = c.pointer else {
                        unreachable!("decoded chunks are stored pointers")
                    };
                    stored.push((ptr, len));
                    if ptr >> 30 != 0 {
                        local = None;
                        continue;
                    }
                    let at_byte = ptr as usize;
                    match bytes.get(at_byte..at_byte + len as usize) {
                        Some(s) => {
                            if let Some(l) = local.as_mut() {
                                l.extend_from_slice(s);
                            }
                        }
                        None => {
                            bad(format!("{at}: chunk at {ptr:#x}+{len:#x} past the file"));
                            local = None;
                        }
                    }
                }
                perms.push(Perm {
                    raw: e[2..8].try_into().expect("6 bytes"),
                    size,
                    chunks: nc,
                    data: local.map_or(Data::Elsewhere(stored), Data::Local),
                });
            }
            ranges.push((element(blocks::PITCH_RANGE_PARAMETERS, params), perms));
        }
        let playback = element(blocks::PLAYBACKS, u16_at(&header, snd::PLAYBACK));
        if playback.is_none() {
            bad("playback index past the gestalt".into());
        }
        let scale = element(blocks::SCALES, usize::from(header[snd::SCALE]));
        out.insert(
            name,
            Read {
                header,
                playback,
                scale,
                ranges,
            },
        );
    }
    Ok(out)
}

/// The fields two maps' copies of one sound share: flags, class, rate,
/// encoding, compression, range count, max play time. Gestalt indices
/// differ between maps.
fn shared_fields(h: &[u8; snd::LEN]) -> Vec<u8> {
    let mut v = h[..snd::PLAYBACK].to_vec();
    v.push(h[snd::PITCH_RANGE_COUNT]);
    v.extend_from_slice(&h[snd::MAX_PLAY_TIME..]);
    v
}

/// Audits every sound of `built` (a whole map file). Sounds named as a
/// Halo 1 sound of `h1` are checked against its conversion; the rest
/// against the same-named sound in the first of `retail` (`(label, map
/// bytes)`) that has one.
pub fn audit(built: &[u8], h1: Option<&H1Map>, retail: &[(&str, &[u8])]) -> Result<Audit> {
    let mut a = Audit::default();
    let out = read_map(built, "built", &mut a.problems)?;
    a.sounds = out.len();
    let mut sources = Vec::with_capacity(retail.len());
    for (label, bytes) in retail {
        let mut ignored = Vec::new();
        sources.push(read_map(bytes, label, &mut ignored)?);
    }
    let mut names: Vec<&String> = out.keys().collect();
    names.sort();
    for name in names {
        let got = &out[name];
        a.permutations += got.ranges.iter().map(|r| r.1.len()).sum::<usize>();
        let mut bad = |what: String| a.problems.push(format!("{name}: {what}"));
        let h1_tag = h1.and_then(|m| m.find(SND, name).map(|t| (m, t)));
        if let Some((m, tag)) = h1_tag {
            a.converted += 1;
            let s = crate::Sound::read(m, tag)?;
            let c = convert_sound(&s, m.bytes())?;
            if shared_fields(&got.header) != shared_fields(&c.header) {
                bad(format!(
                    "header {:02x?}, converted {:02x?}",
                    got.header, c.header
                ));
            }
            if got.playback.as_deref() != Some(&c.playback[..]) {
                bad("playback differs from the conversion".into());
            }
            if got.scale.as_deref() != Some(&c.scale[..]) {
                bad("scale differs from the conversion".into());
            }
            if got.ranges.len() != c.ranges.len() {
                bad(format!(
                    "{} pitch ranges, Halo 1 {}",
                    got.ranges.len(),
                    c.ranges.len()
                ));
                continue;
            }
            let rate = f64::from(s.hz()?);
            let block = block_len(&c.header).unwrap_or(1);
            let per_block = if c.header[snd::COMPRESSION] == 1 {
                64.0
            } else {
                1.0
            };
            for (r, ((params, perms), want)) in got.ranges.iter().zip(&c.ranges).enumerate() {
                if params.as_deref() != Some(&want.parameters[..]) {
                    bad(format!("range {r}: parameters differ"));
                }
                if perms.len() != want.permutations.len() {
                    bad(format!(
                        "range {r}: {} permutations, Halo 1 {}",
                        perms.len(),
                        want.permutations.len()
                    ));
                    continue;
                }
                for (p, (perm, w)) in perms.iter().zip(&want.permutations).enumerate() {
                    let expect: Vec<u8> = w.chunks.concat();
                    let secs = |n: usize| n as f64 / block as f64 * per_block / rate;
                    match &perm.data {
                        Data::Local(b) if *b == expect => a.converted_seconds += secs(b.len()),
                        Data::Local(b) => bad(format!(
                            "range {r} permutation {p}: {} bytes ({:.2} s) differ from Halo 1's {} ({:.2} s)",
                            b.len(),
                            secs(b.len()),
                            expect.len(),
                            secs(expect.len())
                        )),
                        Data::Elsewhere(_) => {
                            bad(format!("range {r} permutation {p}: chunks outside the map"))
                        }
                    }
                    let mut raw = [0u8; 6];
                    raw[..2].copy_from_slice(&w.skip_fraction.to_le_bytes());
                    raw[2] = w.gain as u8;
                    raw[3] = u8::try_from(p).unwrap_or(u8::MAX);
                    raw[4..].copy_from_slice(&w.time_ms.to_le_bytes());
                    if perm.raw != raw {
                        bad(format!(
                            "range {r} permutation {p}: element {:02x?}, converted {raw:02x?}",
                            perm.raw
                        ));
                    }
                    if perm.chunks != w.chunks.len() {
                        bad(format!(
                            "range {r} permutation {p}: {} chunks, converted {}",
                            perm.chunks,
                            w.chunks.len()
                        ));
                    }
                }
            }
            continue;
        }
        let Some((label, want)) = retail
            .iter()
            .zip(&sources)
            .find_map(|((label, _), s)| s.get(name).map(|w| (label, w)))
        else {
            a.unmatched.push(name.clone());
            continue;
        };
        a.retail += 1;
        if shared_fields(&got.header) != shared_fields(&want.header) {
            bad(format!(
                "header {:02x?}, {label}'s {:02x?}",
                got.header, want.header
            ));
        }
        if got.playback != want.playback {
            bad(format!("playback differs from {label}'s"));
        }
        if got.scale != want.scale {
            bad(format!("scale differs from {label}'s"));
        }
        if got.ranges.len() != want.ranges.len() {
            bad(format!(
                "{} pitch ranges, {label}'s {}",
                got.ranges.len(),
                want.ranges.len()
            ));
            continue;
        }
        for (r, (g, w)) in got.ranges.iter().zip(&want.ranges).enumerate() {
            if g.0 != w.0 {
                bad(format!("range {r}: parameters differ from {label}'s"));
            }
            if g.1.len() != w.1.len() {
                bad(format!(
                    "range {r}: {} permutations, {label}'s {}",
                    g.1.len(),
                    w.1.len()
                ));
                continue;
            }
            for (p, (gp, wp)) in g.1.iter().zip(&w.1).enumerate() {
                if gp != wp {
                    let what = match (&gp.data, &wp.data) {
                        (Data::Local(x), Data::Local(y)) if x != y => {
                            format!("{} bytes differ from {label}'s {}", x.len(), y.len())
                        }
                        _ if gp.raw != wp.raw => {
                            format!("element {:02x?}, {label}'s {:02x?}", gp.raw, wp.raw)
                        }
                        _ => format!("chunks {:x?}, {label}'s {:x?}", gp.data, wp.data),
                    };
                    bad(format!("range {r} permutation {p}: {what}"));
                }
            }
        }
    }
    Ok(a)
}
