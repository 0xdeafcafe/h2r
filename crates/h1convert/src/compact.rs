//! The sound gestalt cut to the sounds a map keeps.
//!
//! A base map's `ugh!` holds the entries of every sound the base had; a map
//! built from scratch keeps only the `snd!` tags something reaches, so the
//! rest of the gestalt (and the raw its chunks name) is dead weight.
//! [`gestalt`] builds a new gestalt from nothing: each kept `snd!`, in index
//! order, takes its entries across ([`h1sounds::merge::merge_sound`]) and
//! gets its indices rewritten. Chunk and extra-info raw stay the resources
//! they were and import names the strings they were (their fixups carried
//! across as tokens), and the raw nothing names any more is emptied
//! ([`crate::fresh::drop_unused_raw`]).
//!
//! A promotion, which the merge refuses, is carried whole (its rules index
//! the sound's own pitch ranges: [`PROMOTION_EVIDENCE`]).
//!
//! The gestalt is left whole, with the reason, when a `snd!` is not a
//! built 20-byte tree, or when the `ugh!` holds raw anywhere but a chunk or
//! an extra info, or names in a form other than string IDs.

use std::collections::HashMap;

use h1sounds::convert::snd;
use h1sounds::gestalt::{ChunkPointer, SoundGestalt, blocks};
use h1sounds::merge::{MergeMap, merge_sound};
use map::FourCC;
use map::build::{Block, FixupKind, RawRef, TagPayload, TagSet};

use crate::{Error, Result};

/// What [`gestalt`] did.
#[derive(Debug, Clone, Default)]
pub struct Compacted {
    /// Sounds carried across.
    pub sounds: usize,
    /// Gestalt bytes (tree, without padding) before and after.
    pub bytes: (u64, u64),
    /// Chunks before and after.
    pub chunks: (usize, usize),
    /// Raw resources emptied, and their bytes.
    pub raw_dropped: (usize, u64),
}

/// Why a promotion moves whole. Its rules (`+0`, `0x10` each) start with
/// a pitch range index *within the sound* (V, 2026-11-18, every promotion
/// of 05a, Coagulation and 03a: each index is below the sound's own range
/// count, and `glass_breakable`'s rules are the same on 05a and
/// Coagulation though its first range is 13 on one and 20 on the other);
/// its runtime timers (`+8`) are one per rule, and the rest of the element
/// is runtime state (`ffffffff`, then zeros). Every promotion has one
/// sound.
pub const PROMOTION_EVIDENCE: () = ();

/// Extra info: element size and its raw pointer.
const EXTRA_LEN: u32 = 0x2c;
const EXTRA_RAW: u32 = 8;
/// Chunk: element size (its raw pointer at `+0`).
const CHUNK_LEN: u32 = 12;
/// Tokens standing for raw fixups while the gestalt is an
/// [`SoundGestalt`] (which keeps pointers as stored words).
const TOKEN: u32 = 0xf000_0000;

fn fail(s: impl Into<String>) -> Error {
    Error::Fresh(format!("gestalt: {}", s.into()))
}

fn child_mut(root: &mut Block, at: u32) -> Option<&mut Block> {
    root.fixups.iter_mut().find_map(|f| match &mut f.kind {
        FixupKind::Block { child, .. } if f.offset == at => Some(child),
        _ => None,
    })
}

/// The fixups [`SoundGestalt`] keeps as stored words, by token.
#[derive(Default)]
struct Tokens {
    raws: Vec<RawRef>,
    names: Vec<String>,
}

/// Import names: token base (string IDs are below it).
const NAME: u32 = 0xe000_0000;

/// Makes `root` (a built `ugh!`) what [`SoundGestalt::from_block`] reads:
/// the runtime permutation flags a block of bytes rather than data, and
/// the raw fixups of the chunks and extra infos and the import names'
/// string IDs taken out, a token written in their place.
fn tokenize(root: &mut Block) -> Result<Tokens> {
    let mut t = Tokens::default();
    let flags = (blocks::RUNTIME_PERMUTATION_FLAGS * 8) as u32;
    for f in &mut root.fixups {
        if f.offset == flags
            && let FixupKind::Data(child) = &f.kind
        {
            let child = child.clone();
            f.kind = FixupKind::Block {
                count: child.bytes.len() as u32,
                child,
            };
        }
    }
    if let Some(b) = child_mut(root, (blocks::IMPORT_NAMES * 8) as u32) {
        let n = b.bytes.len() / 4;
        let mut seen = vec![false; n];
        let mut by_name: HashMap<String, u32> = HashMap::new();
        for f in std::mem::take(&mut b.fixups) {
            let FixupKind::StringId(name) = f.kind else {
                return Err(fail("an import name that is not a string ID"));
            };
            let next = NAME | t.names.len() as u32;
            let token = *by_name.entry(name.clone()).or_insert_with(|| {
                t.names.push(name);
                next
            });
            b.bytes[f.offset as usize..][..4].copy_from_slice(&token.to_le_bytes());
            seen[f.offset as usize / 4] = true;
        }
        if seen.iter().any(|s| !s) {
            return Err(fail("an import name stored as a word"));
        }
    }
    let raws = &mut t.raws;
    for (i, stride, at) in [
        (blocks::CHUNKS, CHUNK_LEN, 0),
        (blocks::EXTRA_INFOS, EXTRA_LEN, EXTRA_RAW),
    ] {
        let Some(b) = child_mut(root, (i * 8) as u32) else {
            continue;
        };
        for k in 0..b.bytes.len() as u32 / stride {
            let w = u32::from_le_bytes(
                b.bytes[(k * stride + at) as usize..][..4]
                    .try_into()
                    .expect("4"),
            );
            if w & TOKEN == TOKEN {
                return Err(fail(format!("a stored pointer {w:#x} looks like a token")));
            }
        }
        let mut kept = Vec::new();
        for f in std::mem::take(&mut b.fixups) {
            match f.kind {
                FixupKind::Raw(r) if f.offset % stride == at => {
                    let token = TOKEN | raws.len() as u32;
                    b.bytes[f.offset as usize..][..4].copy_from_slice(&token.to_le_bytes());
                    raws.push(r);
                }
                FixupKind::Raw(_) => return Err(fail("raw at an unknown place")),
                _ => kept.push(f),
            }
        }
        b.fixups = kept;
    }
    // Raw anywhere else would be lost by the decode.
    fn any_raw(b: &Block) -> bool {
        b.fixups.iter().any(|f| match &f.kind {
            FixupKind::Raw(_) => true,
            FixupKind::Block { child, .. } | FixupKind::Data(child) | FixupKind::Pointer(child) => {
                any_raw(child)
            }
            _ => false,
        })
    }
    if any_raw(root) {
        return Err(fail("raw outside the chunks and extra infos"));
    }
    if raws.len() as u32 >= !TOKEN || t.names.len() as u32 >= TOKEN - NAME {
        return Err(fail("too many raw resources or names"));
    }
    Ok(t)
}

/// Puts the fixups back where [`tokenize`] left tokens.
fn restore(root: &mut Block, t: &Tokens) -> Result<()> {
    let raws = &t.raws;
    if let Some(b) = child_mut(root, (blocks::IMPORT_NAMES * 8) as u32) {
        for k in 0..b.bytes.len() / 4 {
            let w = u32::from_le_bytes(b.bytes[k * 4..][..4].try_into().expect("4"));
            let name = t
                .names
                .get(w.wrapping_sub(NAME) as usize)
                .filter(|_| w & TOKEN == NAME)
                .ok_or_else(|| fail("an import name lost its token"))?;
            b.bytes[k * 4..][..4].fill(0);
            b.string_id((k * 4) as u32, name);
        }
    }
    let flags = (blocks::RUNTIME_PERMUTATION_FLAGS * 8) as u32;
    for f in &mut root.fixups {
        if f.offset == flags
            && let FixupKind::Block { child, .. } = &f.kind
        {
            f.kind = FixupKind::Data(child.clone());
        }
    }
    for (i, stride, at) in [
        (blocks::CHUNKS, CHUNK_LEN, 0),
        (blocks::EXTRA_INFOS, EXTRA_LEN, EXTRA_RAW),
    ] {
        let Some(b) = child_mut(root, (i * 8) as u32) else {
            continue;
        };
        for k in 0..b.bytes.len() as u32 / stride {
            let o = k * stride + at;
            let w = u32::from_le_bytes(b.bytes[o as usize..][..4].try_into().expect("4"));
            if w & TOKEN == TOKEN {
                let r = raws
                    .get((w & !TOKEN) as usize)
                    .ok_or_else(|| fail("a token past the table"))?;
                b.bytes[o as usize..][..4].fill(0);
                b.raw(o, *r);
            }
        }
    }
    Ok(())
}

/// Every pointer and name as it is: the gestalt stays in the same set.
struct Same;

impl MergeMap for Same {
    fn name(&mut self, id: u32) -> h1sounds::Result<u32> {
        Ok(id)
    }
    fn raw(&mut self, pointer: ChunkPointer, _size: u32) -> h1sounds::Result<ChunkPointer> {
        Ok(pointer)
    }
}

/// Cuts `set`'s gestalt to the `snd!` tags it holds (see the module docs).
/// `set` is unchanged on error.
pub fn gestalt(set: &mut TagSet) -> Result<Compacted> {
    let slot = SoundGestalt::slot(set).ok_or_else(|| fail("no ugh!"))?;
    let owner = set.slots[slot].tag.as_ref().expect("gestalt");
    let TagPayload::Built(old) = &owner.payload else {
        return Err(fail("the ugh! is not built"));
    };
    let mut root = old.clone();
    let before = root.tree_len();
    let tokens = tokenize(&mut root)?;
    let src = SoundGestalt::from_block(&root).map_err(|e| fail(e.to_string()))?;

    let mut dst = SoundGestalt::default();
    let mut headers = HashMap::new();
    for (i, s) in set.slots.iter().enumerate() {
        let Some(t) = s.tag.as_ref().filter(|t| t.group == FourCC(*b"snd!")) else {
            continue;
        };
        let TagPayload::Built(b) = &t.payload else {
            return Err(fail(format!("{:?} is not built", s.name)));
        };
        use tags::sound::h1_promotion as h1;
        let promoted = h1::present(&b.bytes)
            && (b.fixups.is_empty()
                || matches!(b.fixups.as_slice(), [f] if f.offset == h1::TARGET
                    && matches!(f.kind, FixupKind::TagId(_))));
        if !promoted && (b.bytes.len() < snd::LEN || !b.fixups.is_empty()) {
            return Err(fail(format!("{:?} is not a plain snd!", s.name)));
        }
        // The merge refuses promotions; carried here whole (see
        // `PROMOTION_EVIDENCE`).
        let mut header = b.bytes.clone();
        let promotion = header[snd::PROMOTION];
        header[snd::PROMOTION] = 0xff;
        let mut out = merge_sound(&src, &header, &mut dst, owner.id.0, &mut Same)
            .map_err(|e| fail(format!("{:?}: {e}", s.name)))?;
        if promotion != 0xff {
            let e = src.nested[blocks::PROMOTIONS]
                .get(promotion as usize)
                .ok_or_else(|| fail(format!("{:?}: promotion {promotion}", s.name)))?
                .clone();
            dst.nested[blocks::PROMOTIONS].push(e);
            out[snd::PROMOTION] = u8::try_from(dst.count(blocks::PROMOTIONS) - 1)
                .ok()
                .filter(|&v| v != 0xff)
                .ok_or_else(|| fail("more than 254 promotions"))?;
        }
        headers.insert(i, out);
    }
    let mut new_root = dst.to_block();
    restore(&mut new_root, &tokens)?;
    let report = Compacted {
        sounds: headers.len(),
        bytes: (before, new_root.tree_len()),
        chunks: (src.chunks.len(), dst.chunks.len()),
        raw_dropped: (0, 0),
    };

    // Commit.
    for (i, h) in headers {
        if let Some(TagPayload::Built(b)) = set.slots[i].tag.as_mut().map(|t| &mut t.payload) {
            b.bytes = h;
        }
    }
    set.slots[slot].tag.as_mut().expect("tag").payload = TagPayload::Built(new_root);
    let raw_dropped = crate::fresh::drop_unused_raw(set);
    Ok(Compacted {
        raw_dropped,
        ..report
    })
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// The gestalt entries the `snd!` `header` names in `g`, without an index
/// in them: what two maps holding the same sound under different indices
/// (a gestalt and its [`gestalt`] cut, say) agree on. `name` shows an
/// import name (a string ID); `chunk` a chunk, or an extra info's raw as a
/// chunk of its pointer and size.
pub fn entries(
    g: &SoundGestalt,
    header: &[u8],
    name: &dyn Fn(u32) -> String,
    chunk: &dyn Fn(&h1sounds::gestalt::Chunk) -> String,
) -> Vec<String> {
    let at = |i: usize, n: usize| {
        if n < g.count(i) {
            g.element(i, n).to_vec()
        } else {
            Vec::new()
        }
    };
    let import = |n: u16| {
        let e = at(blocks::IMPORT_NAMES, n as usize);
        e.get(..4)
            .map(|b| name(u32::from_le_bytes(b.try_into().expect("4"))))
            .unwrap_or_else(|| "?".into())
    };
    let mut out = Vec::new();
    let mut h = header[..snd::LEN].to_vec();
    for (o, n) in [
        (snd::PLAYBACK, 2),
        (snd::FIRST_PITCH_RANGE, 2),
        (snd::PITCH_RANGE_COUNT, 0),
        (snd::SCALE, 1),
        (snd::PROMOTION, 1),
        (snd::CUSTOM_PLAYBACK, 1),
        (snd::EXTRA_INFO, 2),
    ] {
        h[o..o + n].fill(0);
    }
    out.push(format!("header {h:02x?}"));
    out.push(format!(
        "playback {:02x?}",
        at(blocks::PLAYBACKS, u16_at(header, snd::PLAYBACK) as usize)
    ));
    out.push(format!(
        "scale {:02x?}",
        at(blocks::SCALES, header[snd::SCALE] as usize)
    ));
    let first = u16_at(header, snd::FIRST_PITCH_RANGE) as usize;
    for r in first..first + header[snd::PITCH_RANGE_COUNT] as usize {
        let range = at(blocks::PITCH_RANGES, r);
        if range.len() < 12 {
            out.push(format!("pitch range {r} missing"));
            continue;
        }
        let params = at(blocks::PITCH_RANGE_PARAMETERS, u16_at(&range, 2) as usize);
        let (perm0, perms) = (u16_at(&range, 8) as usize, u16_at(&range, 10) as usize);
        let bits = if u16_at(&range, 4) == 0xffff {
            let from = u16_at(&range, 6) as usize;
            let f = &g.flat[blocks::RUNTIME_PERMUTATION_FLAGS];
            (0..perms)
                .map(|b| {
                    f.get((from + b) / 8)
                        .map_or('?', |v| char::from(b'0' + (v >> ((from + b) % 8) & 1)))
                })
                .collect::<String>()
        } else {
            format!("{:02x?}", &range[4..8])
        };
        out.push(format!(
            "range {} params {params:02x?} flags {bits}",
            import(u16_at(&range, 0))
        ));
        for p in perm0..perm0 + perms {
            let perm = at(blocks::PERMUTATIONS, p);
            if perm.len() < 16 {
                out.push(format!("permutation {p} missing"));
                continue;
            }
            let (c0, cn) = (u16_at(&perm, 12) as usize, u16_at(&perm, 14) as usize);
            let chunks: Vec<String> = (c0..c0 + cn)
                .map(|c| g.chunks.get(c).map_or_else(|| "?".into(), chunk))
                .collect();
            out.push(format!(
                "permutation {} {:02x?} chunks {chunks:?}",
                import(u16_at(&perm, 0)),
                &perm[2..12]
            ));
        }
    }
    let nested = |i: usize, n: usize| g.nested[i].get(n).map(|e| format!("{e:02x?}"));
    if header[snd::CUSTOM_PLAYBACK] != 0xff {
        out.push(format!(
            "custom playback {:?}",
            nested(
                blocks::CUSTOM_PLAYBACKS,
                header[snd::CUSTOM_PLAYBACK] as usize
            )
        ));
    }
    if header[snd::PROMOTION] != 0xff {
        out.push(format!(
            "promotion {:?}",
            nested(blocks::PROMOTIONS, header[snd::PROMOTION] as usize)
        ));
    }
    let extra = u16_at(header, snd::EXTRA_INFO);
    if extra != 0xffff {
        match g.nested[blocks::EXTRA_INFOS].get(extra as usize) {
            Some(e) => {
                let mut e = e.clone();
                let raw = h1sounds::gestalt::Chunk {
                    pointer: ChunkPointer::Stored(u32::from_le_bytes(
                        e.bytes[8..12].try_into().expect("4"),
                    )),
                    size: u32::from_le_bytes(e.bytes[12..16].try_into().expect("4")),
                    runtime: 0,
                };
                e.bytes[8..12].fill(0);
                e.bytes[0x20..0x24].fill(0);
                out.push(format!("extra info {e:02x?} raw {}", chunk(&raw)));
            }
            None => out.push("extra info missing".into()),
        }
    }
    out
}

/// [`entries`] of every `snd!` of `m` (whose whole file is `file`), by tag
/// name: import names as strings, local chunk and extra-info raw as a hash
/// of their bytes, external pointers as they are.
pub fn map_entries<R: std::io::Read + std::io::Seek>(
    m: &mut map::Map<R>,
    file: &[u8],
) -> Result<HashMap<String, Vec<String>>> {
    use std::hash::{Hash, Hasher};
    let ugh = m
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"ugh!"))
        .cloned()
        .ok_or_else(|| fail("no ugh!"))?;
    let address = ugh
        .location
        .ok_or_else(|| fail("ugh! has no data"))?
        .address;
    let g = SoundGestalt::decode(&m.read_tag_bytes(ugh.id)?, address)
        .map_err(|e| fail(e.to_string()))?;
    let strings = m.strings().to_vec();
    let name = |id: u32| format!("{:?}", strings.get((id & 0xff_ffff) as usize));
    let chunk = |c: &h1sounds::gestalt::Chunk| {
        let p = match c.pointer {
            ChunkPointer::Stored(p) => p,
            ChunkPointer::Local(r) => return format!("local {}", r.0),
        };
        let n = (c.size & 0x7fff_ffff) as usize;
        match p {
            0 | u32::MAX => "null".to_owned(),
            p if p >> 30 == 0 => {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                file.get(p as usize..p as usize + n).hash(&mut h);
                format!("{} {:016x}", c.size, h.finish())
            }
            p => format!("{} {p:#x}", c.size),
        }
    };
    let mut out = HashMap::new();
    for t in m.tags().to_vec() {
        if t.group != FourCC(*b"snd!") {
            continue;
        }
        let Ok(header) = m.read_tag_bytes(t.id) else {
            continue;
        };
        if header.len() >= snd::LEN {
            out.insert(
                t.name.clone().unwrap_or_default(),
                entries(&g, &header, &name, &chunk),
            );
        }
    }
    Ok(out)
}
