//! A converted multiplayer map with **nothing pinned** (`h1 convert
//! --fresh`).
//!
//! The pinned conversion ([`Prepared::build`]) keeps the base map's file
//! and metadata image and changes tags inside it. A fresh map starts from
//! an empty [`TagSet`] (no [`map::build::PinnedImage`]) that takes the
//! pinned set's index as it is (same slots, names, datums and string
//! table), so no reference needs rewriting, and gives every tag a payload
//! the builder lays out itself:
//!
//! | Tag | Payload |
//! |---|---|
//! | every ordinary tag | the schema's depth-first tree ([`tags::schema::transplant::Unpinner`]); local raw resources copied into the new file |
//! | size-0 aliases | [`TagPayload::Alias`] of their owner |
//! | `scnr` | built from scratch: the root's scalars and tag references, every root block field zeroed, then each block the converted scenario fills that the schema can copy ([`Relocator::block_field`]); the BSP descriptors' region words become [`FixupKind::Bsp`] |
//! | `ugh!` | [`h1sounds::gestalt::SoundGestalt`], chunks' local raw copied |
//! | converted tags (structure, lightmap, collections, objects) | as the conversion built them |
//!
//! So the map carries the base's tags (everything Coagulation has, whether
//! Blood Gulch uses it or not), the converted ones, and no byte of the
//! base's file beyond what those tags name.

use std::io::{Cursor, Read, Seek};

use h1sounds::gestalt::SoundGestalt;
use map::build::{BspField, FixupKind, TagPayload, TagSet};
use map::{FourCC, Map, TagId};
use tags::schema::transplant::Unpinner;
use tags::schema::{Field, Relocator, Schema};

use crate::convert::{Prepared, map_name};
use crate::{Error, Result};

/// `scnr` BSP descriptors: block field and element size.
const BSPS: u32 = 0x210;
const BSP: u32 = 0x44;

/// What [`Prepared::build_fresh`] did.
#[derive(Debug, Clone, Default)]
pub struct FreshReport {
    /// Tags turned from pinned bytes into built trees.
    pub unpinned: usize,
    /// Of them, size-0 aliases.
    pub aliases: usize,
    /// Local raw resources copied into the new file, and their bytes.
    pub raw: (usize, u64),
    /// Pointers into the shared caches, kept.
    pub external: usize,
    /// `scnr` root block fields copied, as `(offset, elements)`.
    pub scenario_blocks: Vec<(u32, u32)>,
    /// Of them, copied flat from outside every tag (blocks the scenario
    /// editor grew).
    pub scenario_flat: Vec<u32>,
    /// `scnr` root block fields the converted scenario fills but the
    /// schema cannot copy (left empty), with why.
    pub scenario_skipped: Vec<(u32, String)>,
    /// Block fields of built tags (converted on a template) that held the
    /// template's addresses, copied as trees ([`Unpinner::rehome`]):
    /// `group name: fields`.
    pub rehomed: Vec<String>,
    /// Of those, the ones the schema refused, left pointing at nothing:
    /// `group name +offset -> address`.
    pub stale: Vec<String>,
    /// Sound gestalt chunks whose raw data was copied.
    pub sound_chunks: usize,
    /// Bytes of the low-detail bitmap region copied.
    pub low_detail: u32,
    /// The base's tags nothing reaches, dropped ([`prune`]).
    pub pruned: Pruned,
    /// Pinned `sbsp`/`ltmp` tags left out ([`unpin_all`]).
    pub bsps_left: usize,
    /// Size-0 tags at an address no tag owns, kept with their group's root
    /// bytes from there (or no data when those are not in the metadata).
    pub no_data: usize,
    /// Tags the schema cannot move, left without data, and why
    /// ([`check_refused`]).
    pub refused: Vec<(TagId, String)>,
    /// The map from scratch ([`crate::scratch`]), or why not (the pruned
    /// set is written instead).
    pub scratch: Option<crate::scratch::ScratchReport>,
    pub scratch_failed: Option<String>,
    /// The base's tags the pinned map it is read from dropped
    /// ([`prune_pinned`]), before [`Self::pruned`].
    pub pinned_pruned: Pruned,
}

/// A fresh map.
#[derive(Debug, Clone)]
pub struct Fresh {
    /// `h1_<map>`.
    pub name: String,
    /// The map file.
    pub bytes: Vec<u8>,
    /// What was done.
    pub report: FreshReport,
    /// The precision swaps' collections, as this map's datums (for
    /// [`Prepared::manifest_with`]).
    pub precision: Vec<(TagId, TagId)>,
    /// The HUD pairs (owner, Halo 1 HUD, Halo 2 HUD), as this map's datums.
    pub hud: Vec<[TagId; 3]>,
}

impl Prepared {
    /// The converted map with nothing pinned (see the module docs).
    pub fn build_fresh<R: Read + Seek>(&self, base: &mut Map<R>, mopp: &[u8]) -> Result<Fresh> {
        let (pinned, bytes, section, pinned_pruned) = self.assemble(base, mopp)?;
        let mut src = Map::from_reader(Cursor::new(&bytes[..]))?;
        let mut fresh = unpinned_copy(&pinned);
        // The pinned map keeps the base's file prefix, so the base's own
        // BSP regions and low-detail region are still there, unnamed:
        // resources end before them.
        let h = base.header();
        let mut bounds = vec![h.low_detail_offset];
        let meta = base.metadata_space();
        for t in base.tags() {
            if let Some(l) = t.location.filter(|l| Some(l.space) != meta) {
                bounds.push(u32::try_from(l.space.file_offset).unwrap_or(u32::MAX));
            }
        }
        let mut report = unpin_all(&pinned, &mut src, &mut fresh, &[section], &bounds, false)?;
        report.pinned_pruned = pinned_pruned;
        if let Some(scenery) = &self.scenery {
            put_scenery(&mut fresh, scenery)?;
        }
        // The precision swaps' collections are named by no placement: kept
        // for `boot::precision` to swap in at game start.
        // So is each owner's Halo 2 HUD, for `boot::hud_swap` (docs/halo1/hud.md).
        let huds = self.hud_swaps();
        let mut keep: Vec<TagId> = self.applied.precision.iter().map(|p| p.1).collect();
        keep.extend(huds.iter().map(|h| h[2]));
        report.pruned = prune(&mut fresh, base.index_slots() as usize, &keep);
        check_refused(&mut fresh, &report)?;
        drop(src);
        // Then from scratch: the base's tags by path, only what is reached,
        // on a dense index. The pruned set is the fallback.
        let mut precision = self.applied.precision.clone();
        let mut hud = huds.clone();
        match from_scratch(base, &pinned, &bytes, &fresh, &keep) {
            Ok((set, r)) => {
                // The swaps' datums, in the new index (the manifest's).
                let key = |s: &TagSet, d: TagId| {
                    let slot = s.slots.get(d.index())?;
                    let t = slot.tag.as_ref().filter(|t| t.id == d)?;
                    Some((t.group, slot.name.clone()))
                };
                let by_key: std::collections::HashMap<_, TagId> = set
                    .slots
                    .iter()
                    .filter_map(|s| s.tag.as_ref().map(|t| ((t.group, s.name.clone()), t.id)))
                    .collect();
                let new = |d: TagId| key(&fresh, d).and_then(|k| by_key.get(&k).copied());
                precision = self
                    .applied
                    .precision
                    .iter()
                    .map(|&(a, b)| Some((new(a)?, new(b)?)))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| Error::Fresh("a precision swap's tag left out".into()))?;
                hud = huds
                    .iter()
                    .map(|h| Some([new(h[0])?, new(h[1])?, new(h[2])?]))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| Error::Fresh("a HUD pair's tag left out".into()))?;
                fresh = set;
                report.scratch = Some(r);
            }
            Err(e) => report.scratch_failed = Some(e.to_string()),
        }
        let (bytes, _) = fresh.build()?;
        Ok(Fresh {
            name: map_name(&self.h1_name),
            bytes,
            report,
            precision,
            hud,
        })
    }
}

/// The map from scratch ([`crate::scratch`]): `base`'s tags (the retail
/// map) and the conversion's (`pinned`, built as `bytes`) read by path,
/// with `fresh`'s scenario and BSP.
fn from_scratch<R: Read + Seek>(
    base: &mut Map<R>,
    pinned: &TagSet,
    bytes: &[u8],
    fresh: &TagSet,
    extra: &[TagId],
) -> Result<(TagSet, crate::scratch::ScratchReport)> {
    use crate::scratch::{LevelSource, build, changed, file_bytes, low_detail};
    use tags::repo::{MapSource, SharedFiles};
    let repo = |e: tags::repo::Error| Error::Fresh(format!("from scratch: {e}"));
    let schema = Schema::builtin();
    let from_level = changed(base, pinned);
    let low = low_detail(base)?;
    let file = file_bytes(base)?;
    let mut shared_base = SharedFiles::default();
    shared_base.unread_ok = true;
    let mut shared_level = SharedFiles::default();
    shared_level.unread_ok = true;
    let base_map = Map::from_reader(Cursor::new(&file[..]))?;
    let level_map = Map::from_reader(Cursor::new(bytes))?;
    let mut src = LevelSource {
        base: MapSource::new(schema, base_map, &mut shared_base).map_err(repo)?,
        level: MapSource::new(schema, level_map, &mut shared_level).map_err(repo)?,
        from_level,
    };
    let from_level = src.from_level.clone();
    build(&mut src, fresh, &MULTIPLAYER, extra, &from_level, low)
}

/// A set with `pinned`'s index, strings, raw resources and BSP sections,
/// and nothing pinned. Payloads are copied as they are; [`unpin_all`]
/// replaces the pinned ones.
pub fn unpinned_copy(pinned: &TagSet) -> TagSet {
    let mut set = TagSet::new(pinned.cache_type, &pinned.name, &pinned.scenario_path);
    set.groups = pinned.groups.clone();
    set.strings = pinned.strings.clone();
    set.slots = pinned.slots.clone();
    set.scenario = pinned.scenario;
    set.globals = pinned.globals;
    set.index_word_14 = pinned.index_word_14;
    set.raw = pinned.raw.clone();
    set.bsps = pinned.bsps.clone();
    set.salt_base = pinned.salt_base;
    // Header values the builder does not own (dependency identities and
    // the like) stay the base's; no byte of its file does.
    set.header_template = pinned.pinned.as_ref().map(|p| p.header.clone());
    set
}

/// Replace every pinned payload of `fresh` (a copy of `pinned`, whose
/// built map is `src`) with a built one. `sections[k]` is the BSP section
/// the scenario's descriptor `k` names, when the scenario is pinned and so
/// rebuilt here; a scenario the conversion already built (a campaign
/// level's) is kept as it is. A pinned `sbsp` or `ltmp` (a base BSP no
/// descriptor names any more: its data is in the base's BSP space, which
/// the schema does not move) is left out, its slot emptied. A slot already
/// empty in `fresh` (pruned first) is not unpinned.
/// `bounds` are more file offsets no copied resource runs past (regions of
/// `src`'s file that no tag names). With `built_too` (a campaign level),
/// the tags the conversion built (BSPs, scenario and gestalt aside) are
/// read back from `src` and rebuilt the same way (its converted objects
/// keep words naming arrays they share with their templates, which only
/// `src`'s layout makes good), and the base's unowned metadata is read
/// too (see the body).
pub fn unpin_all<R: Read + Seek>(
    pinned: &TagSet,
    src: &mut Map<R>,
    fresh: &mut TagSet,
    sections: &[usize],
    bounds: &[u32],
    built_too: bool,
) -> Result<FreshReport> {
    let schema = Schema::builtin();
    let mut u = Unpinner::new(schema, src).map_err(tags_error)?;
    u.add_starts(bounds.iter().copied());
    // A campaign intermediate (`built_too`): arrays of base tags live in
    // metadata no tag owns (05a's scenario range and the gaps between
    // tags), the base tags point only into the base's own metadata, and a
    // few retail words are data ([`DATA_WORDS`]).
    if built_too {
        if let Some(p) = &pinned.pinned {
            u.add_regions(src, p.unowned.iter().cloned());
            u.limit_space(p.metadata_base, p.metadata_base + p.metadata_size);
        }
        u.data_words(&DATA_WORDS);
        let gaps = gaps(src)?;
        u.add_regions(src, gaps);
    }
    let mut report = FreshReport::default();
    // The low-detail bitmap region, which bitmaps index into, as a raw
    // resource of its own.
    let h = src.header().clone();
    if h.low_detail_size > 0 {
        let bytes = src.read_file_range(h.low_detail_offset.into(), h.low_detail_size.into())?;
        let align = 1 << h.low_detail_offset.trailing_zeros().min(12);
        fresh.low_detail = Some(fresh.add_raw(bytes, align));
        report.low_detail = h.low_detail_size;
    }
    let ugh = FourCC(*b"ugh!");
    let mut ordinary = Vec::new();
    let mut gestalt = None;
    for slot in &pinned.slots {
        let Some(t) = &slot.tag else { continue };
        // A gestalt that gained sounds (`h1sounds::install_sounds`) is
        // built, but its base chunks still point into the base's file.
        if t.group == ugh
            && let TagPayload::Built(b) = &t.payload
        {
            let g = SoundGestalt::from_block(b).map_err(|e| Error::Fresh(format!("ugh!: {e}")))?;
            gestalt = Some((t.id, g));
            continue;
        }
        if built_too
            && matches!(t.payload, TagPayload::Built(_))
            && t.id != pinned.scenario
            && ![*b"sbsp", *b"ltmp", *b"ugh!"].contains(&t.group.0)
        {
            ordinary.push(t.id);
            continue;
        }
        let TagPayload::Pinned { address, bytes } = &t.payload else {
            continue;
        };
        if t.id == pinned.scenario {
            continue;
        }
        // Emptied by the caller already (pruned before unpinning).
        if fresh
            .slots
            .get(t.id.index())
            .is_none_or(|s| s.tag.is_none())
        {
            continue;
        }
        if t.group == FourCC(*b"sbsp") || t.group == FourCC(*b"ltmp") {
            if let Some(s) = fresh.slots.get_mut(t.id.index()) {
                s.tag = None;
            }
            report.bsps_left += 1;
            continue;
        }
        if t.group == ugh {
            let g = SoundGestalt::decode(bytes, *address)
                .map_err(|e| Error::Fresh(format!("ugh!: {e}")))?;
            gestalt = Some((t.id, g));
            continue;
        }
        if bytes.is_empty() {
            report.aliases += 1;
        }
        ordinary.push(t.id);
    }
    // A tag the schema cannot move is left without data: the unreachable
    // ones are pruned next, and [`check_refused`] fails the map when one
    // is still reached. A size-0 tag at an address no tag owns (`NoData`)
    // had no data to begin with.
    loop {
        match u
            .unpin(src, &ordinary, fresh)
            .map_err(|e| Error::Fresh(format!("unpinning: {e}")))?
        {
            Ok(n) => {
                report.unpinned = n;
                break;
            }
            Err(refused) => {
                for r in refused {
                    // A size-0 tag at an address no tag owns reads its
                    // group's root there (05a's `colo ui\\dialog`): those
                    // bytes, as they were.
                    let root = src
                        .tag(r.id)
                        .ok()
                        .and_then(|t| {
                            Some((t.location?.address, schema.groups.get(&t.group)?.size))
                        })
                        .and_then(|(a, n)| u.relocator().memory().slice(a, n).map(<[u8]>::to_vec));
                    if let Some(t) = fresh.tag_mut(r.id) {
                        t.payload = match root {
                            Some(b) if r.detail.refusal == tags::schema::Refusal::NoData => {
                                TagPayload::Built(map::build::Block::new(b))
                            }
                            _ => TagPayload::Empty,
                        };
                    }
                    ordinary.retain(|&id| id != r.id);
                    if r.detail.refusal == tags::schema::Refusal::NoData {
                        report.no_data += 1;
                    } else {
                        report.refused.push((
                            r.id,
                            format!(
                                "{} {} ({:?} at {:x?} +{:#x}: {:#x})",
                                r.group,
                                r.name,
                                r.detail.refusal,
                                r.detail.path,
                                r.detail.offset,
                                r.detail.value
                            ),
                        ));
                    }
                }
            }
        }
    }
    // Tags the conversion built on a template (the Halo 1 sandbox's
    // objects) keep the template's blocks by address: the pinned map's,
    // which a fresh map does not have. (`built_too` re-reads them from
    // `src` whole instead.)
    for slot in pinned.slots.iter().filter(|_| !built_too) {
        let Some(t) = &slot.tag else { continue };
        if t.group == ugh || t.id == pinned.scenario {
            continue;
        }
        let (TagPayload::Built(_), Some(root)) = (&t.payload, schema.groups.get(&t.group)) else {
            continue;
        };
        let Some(TagPayload::Built(block)) = fresh.tag_mut(t.id).map(|t| &mut t.payload) else {
            continue;
        };
        let mut block = std::mem::replace(block, map::build::Block::zeroed(0));
        let got = u.rehome(root, &mut block, 1, src, fresh);
        fresh.tag_mut(t.id).expect("in the set").payload = TagPayload::Built(block);
        let (n, refused) = got.map_err(tags_error)?;
        let name = slot.name.as_deref().unwrap_or("");
        if n > 0 {
            report.rehomed.push(format!("{} {name}: {n}", t.group));
        }
        report.stale.extend(
            refused
                .into_iter()
                .map(|(o, a)| format!("{} {name} +{o:#x} -> {a:#x}", t.group)),
        );
    }
    if let Some((id, g)) = gestalt {
        report.sound_chunks = sound_gestalt(&mut u, src, fresh, id, g)?;
        report.unpinned += 1;
    }
    if matches!(
        pinned.tag(pinned.scenario).map(|t| &t.payload),
        Some(TagPayload::Pinned { .. })
    ) {
        scenario(&mut u, src, fresh, pinned.scenario, sections, &mut report)?;
        report.unpinned += 1;
    }
    report.raw = u.raw;
    report.external = u.external;
    Ok(report)
}

/// Words outside every schema field that look like metadata addresses but
/// are data: 05a's `prt3 effects\generic\solids\debris_shirt` holds
/// `0x804f0400` as a 4-byte element of `+0x8c`/`+0x2c` (an array of 4-byte
/// values, no room for a block or reference; `plasma_in_beams` holds
/// `0x80810400` there), and the `spas`
/// `shaders\shader_passes\texture_mapping\texaccum_disabled` holds
/// `0x80800000` at `+0xb0` of a `+0x1c` element (a flag pattern; the
/// address is no array start in 05a).
pub const DATA_WORDS: [u32; 3] = [0x804f_0400, 0x8081_0400, 0x8080_0000];

/// The metadata between `src`'s tags, as (address, bytes).
fn gaps<R: Read + Seek>(src: &mut Map<R>) -> Result<Vec<(u32, Vec<u8>)>> {
    let Some(space) = src.metadata_space() else {
        return Ok(Vec::new());
    };
    let mut spans: Vec<(u32, u32)> = src
        .tags()
        .iter()
        .filter_map(|t| t.location)
        .filter(|l| l.space == space && l.size > 0)
        .map(|l| (l.address, l.address + l.size))
        .collect();
    spans.sort_unstable();
    let mut out = Vec::new();
    let mut at = space.virtual_base;
    let end = space.virtual_base + space.size;
    for (lo, hi) in spans.into_iter().chain([(end, end)]) {
        if lo > at {
            let off = src.file_offset(at, lo - at)?;
            out.push((at, src.read_file_range(off, u64::from(lo - at))?));
        }
        at = at.max(hi);
    }
    Ok(out)
}

/// Empties every raw resource of `set` no kept tag names (those of the
/// tags [`prune`] dropped, and the converted tags' first copies that
/// `built_too` replaced), so they take no room in the file. Returns
/// (resources, bytes) emptied.
pub fn drop_unused_raw(set: &mut TagSet) -> (usize, u64) {
    fn walk(b: &map::build::Block, used: &mut Vec<bool>) {
        for f in &b.fixups {
            match &f.kind {
                FixupKind::Raw(map::build::RawRef::Local(r)) => {
                    if let Some(u) = used.get_mut(r.0) {
                        *u = true;
                    }
                }
                FixupKind::Block { child, .. }
                | FixupKind::Data(child)
                | FixupKind::Pointer(child) => walk(child, used),
                _ => {}
            }
        }
    }
    let mut used = vec![false; set.raw.len()];
    for t in set.slots.iter().filter_map(|s| s.tag.as_ref()) {
        if let TagPayload::Built(b) = &t.payload {
            walk(b, &mut used);
        }
    }
    if let Some(r) = set.low_detail {
        used[r.0] = true;
    }
    let mut out = (0, 0);
    for (blob, used) in set.raw.iter_mut().zip(used) {
        if !used && !blob.bytes.is_empty() {
            out.0 += 1;
            out.1 += blob.bytes.len() as u64;
            blob.bytes = Vec::new();
        }
    }
    out
}

/// Drops the tags [`unpin_all`] could not move from `set`, and fails when
/// another tag still names one (a tag kept only as a root, such as a
/// shader pass under `shaders\\`, is just dropped).
pub fn check_refused(set: &mut TagSet, report: &FreshReport) -> Result<()> {
    let mut gone = Vec::new();
    for (id, why) in &report.refused {
        if let Some(s) = set.slots.get_mut(id.index())
            && s.tag.as_ref().is_some_and(|t| t.id == *id)
        {
            s.tag = None;
            gone.push((id.0, why.as_str()));
        }
    }
    if gone.is_empty() {
        return Ok(());
    }
    let mut named_by = Vec::new();
    let mut words = Vec::new();
    for (i, s) in set.slots.iter().enumerate() {
        let Some(t) = &s.tag else { continue };
        words.clear();
        named(&t.payload, &mut words);
        for (d, why) in &gone {
            if words.contains(d) {
                named_by.push(format!(
                    "{why}, named by {} {} (slot {i})",
                    t.group,
                    s.name.as_deref().unwrap_or("?")
                ));
            }
        }
    }
    if named_by.is_empty() {
        return Ok(());
    }
    Err(Error::Fresh(format!(
        "{} references to tags the schema cannot move: {}",
        named_by.len(),
        named_by[..named_by.len().min(12)].join("; ")
    )))
}

fn word(b: &[u8], at: u32) -> u32 {
    u32::from_le_bytes(b[at as usize..at as usize + 4].try_into().expect("4"))
}

fn tags_error(e: tags::Error) -> Error {
    Error::Fresh(e.to_string())
}

/// The `ugh!` as a built tree, the chunks stored in `src`'s file copied
/// (chunks already local stay). Returns the chunks copied.
fn sound_gestalt<R: Read + Seek>(
    u: &mut Unpinner,
    src: &mut Map<R>,
    fresh: &mut TagSet,
    id: TagId,
    mut g: SoundGestalt,
) -> Result<usize> {
    use h1sounds::gestalt::{ChunkPointer, blocks};
    // Extra infos (lip-sync) hold a raw pointer at +8 with no stored
    // length: copy up to the next known raw start, chunks included.
    const EXTRA_RAW: usize = 8;
    let extra = |e: &h1sounds::gestalt::Nested| {
        u32::from_le_bytes(e.bytes[EXTRA_RAW..EXTRA_RAW + 4].try_into().expect("4"))
    };
    u.add_starts(g.chunks.iter().filter_map(|c| match c.pointer {
        ChunkPointer::Stored(p) => Some(p),
        ChunkPointer::Local(_) => None,
    }));
    u.add_starts(g.nested[blocks::EXTRA_INFOS].iter().map(extra));
    let mut copied = 0;
    // Local lip-sync data: copied as a raw resource of its own, its
    // pointer a raw fixup on the element (added once the tree is built).
    let mut lip_sync = Vec::new();
    for (k, e) in g.nested[blocks::EXTRA_INFOS].iter_mut().enumerate() {
        let p = extra(e);
        if p != u32::MAX && p != 0 && p >> 30 == 0 {
            let len = u.extent(p);
            let raw = u
                .copy_raw(src, fresh, p, len)
                .map_err(|e| Error::Fresh(format!("lip-sync data at {p:#x}: {e}")))?;
            e.bytes[EXTRA_RAW..EXTRA_RAW + 4].fill(0);
            lip_sync.push((k, raw));
            copied += 1;
        }
    }
    for c in &mut g.chunks {
        if let ChunkPointer::Stored(p) = c.pointer
            && p != u32::MAX
            && p >> 30 == 0
        {
            let len = c.size & 0x7fff_ffff;
            c.pointer = ChunkPointer::Local(u.copy_raw(src, fresh, p, len).map_err(|e| {
                Error::Fresh(format!("sound chunk at {p:#x}, {len:#x} bytes: {e}"))
            })?);
            copied += 1;
        }
    }
    let mut block = g.to_block();
    if !lip_sync.is_empty() {
        let at = (blocks::EXTRA_INFOS * 8) as u32;
        let infos = child_mut(&mut block, at)
            .ok_or(Error::Invalid("the gestalt's extra infos did not build"))?
            .1;
        let stride = h1sounds::gestalt::STRIDES[blocks::EXTRA_INFOS];
        for (k, raw) in lip_sync {
            infos.raw(
                (k * stride + EXTRA_RAW) as u32,
                map::build::RawRef::Local(raw),
            );
        }
    }
    fresh
        .tag_mut(id)
        .ok_or(Error::Invalid("no ugh! slot"))?
        .payload = TagPayload::Built(block);
    Ok(copied)
}

/// The scenario, built from scratch (module docs).
fn scenario<R: Read + Seek>(
    u: &mut Unpinner,
    src: &mut Map<R>,
    fresh: &mut TagSet,
    id: TagId,
    sections: &[usize],
    report: &mut FreshReport,
) -> Result<()> {
    let schema = Schema::builtin();
    let root_layout = &schema.groups[&FourCC(*b"scnr")];
    let bytes = src.read_tag_bytes(id)?;
    let size = root_layout.size as usize;
    let mut root_bytes = bytes
        .get(..size)
        .ok_or(Error::Invalid("the scenario is smaller than its root"))?
        .to_vec();
    let blocks: Vec<(u32, &tags::schema::Struct)> = root_layout
        .fields
        .iter()
        .filter_map(|(&o, f)| match f {
            Field::Block(c) => Some((o, &**c)),
            _ => None,
        })
        .collect();
    for &(o, _) in &blocks {
        root_bytes[o as usize..o as usize + 8].fill(0);
    }
    let mut root = map::build::Block::new(root_bytes);
    let rel: &Relocator = u.relocator();
    let mut children = Vec::new();
    for &(o, layout) in &blocks {
        match rel.block_field(id, o) {
            Ok(None) => {}
            Ok(Some((count, mut child))) => {
                if o == BSPS {
                    if count as usize != sections.len() {
                        return Err(Error::Fresh(format!(
                            "the scenario has {count} BSP descriptors for {} BSP sections",
                            sections.len()
                        )));
                    }
                    for (k, &section) in sections.iter().enumerate() {
                        for (at, field) in [
                            (0, BspField::FileOffset),
                            (4, BspField::Size),
                            (8, BspField::Address),
                        ] {
                            child.fixup(k as u32 * BSP + at, FixupKind::Bsp { section, field });
                        }
                    }
                    debug_assert_eq!(layout.size, BSP);
                }
                report.scenario_blocks.push((o, count));
                children.push((o, count, child));
            }
            Err(d) => {
                // A block the scenario editor grew is appended to the
                // metadata outside every tag (netgame flags, say). With no
                // child block and no raw field, its bytes are the whole
                // of it: copy them as they are.
                let flat = layout
                    .fields
                    .values()
                    .all(|f| !matches!(f, Field::Block(_) | Field::Raw));
                let count = word(&bytes, o);
                let at = word(&bytes, o + 4);
                let read = flat
                    .then(|| {
                        let n = count.checked_mul(layout.size)?;
                        let off = src.file_offset(at, n).ok()?;
                        src.read_file_range(off, n.into()).ok()
                    })
                    .flatten();
                match read {
                    Some(b) => {
                        report.scenario_blocks.push((o, count));
                        report.scenario_flat.push(o);
                        children.push((o, count, map::build::Block::new(b).aligned(layout.align)));
                    }
                    None => report.scenario_skipped.push((
                        o,
                        format!("{:?} at {:x?} +{:#x}", d.refusal, d.path, d.offset),
                    )),
                }
            }
        }
    }
    if !report.scenario_blocks.iter().any(|b| b.0 == BSPS) {
        return Err(Error::Invalid("the scenario's BSP descriptor did not copy"));
    }
    for (o, count, child) in children {
        root.block(o, count, child);
    }
    u.localise(root_layout, &mut root, 1, src, fresh)
        .map_err(tags_error)?;
    fresh
        .tag_mut(id)
        .ok_or(Error::Invalid("no scenario slot"))?
        .payload = TagPayload::Built(root);
    Ok(())
}

/// Writes Halo 1's scenery into the fresh set's scenario
/// ([`crate::convert::halo1::Scenery`]): its palette and placements replace
/// the scenario's, which the conversion emptied of the base's placements.
/// The scenario is a built tree here, so the blocks take any size.
pub fn put_scenery(set: &mut TagSet, scenery: &crate::convert::halo1::Scenery) -> Result<()> {
    use crate::convert::halo1::{SCENERY_PALETTE, SCENERY_PLACEMENTS};
    let scenario = set.scenario;
    let Some(TagPayload::Built(root)) = set.tag_mut(scenario).map(|t| &mut t.payload) else {
        return Err(Error::Invalid("the fresh scenario is not a built tree"));
    };
    root.fixups
        .retain(|f| f.offset != SCENERY_PALETTE && f.offset != SCENERY_PLACEMENTS);
    for at in [SCENERY_PALETTE, SCENERY_PLACEMENTS] {
        root.bytes[at as usize..at as usize + 8].fill(0);
    }
    if !scenery.palette.is_empty() {
        root.block(
            SCENERY_PALETTE,
            scenery.palette.len() as u32,
            scenery.palette_block(),
        );
    }
    if !scenery.placements.is_empty() {
        root.block(
            SCENERY_PLACEMENTS,
            scenery.placements.len() as u32,
            scenery.placements_block(),
        );
    }
    Ok(())
}

/// What [`prune`] (or [`prune_pinned`]) dropped.
#[derive(Debug, Clone, Default)]
pub struct Pruned {
    /// Scenario palette entries naming a base tag, nulled.
    pub palette_entries: usize,
    /// Scenario `+0x3d8` entries naming a base tag nothing else reaches.
    pub resource_entries: usize,
    /// Tags dropped, by group.
    pub groups: std::collections::BTreeMap<String, usize>,
    /// [`prune_pinned`]: metadata bytes the dropped tags held, free for
    /// built tags; and the unreached tags kept because a kept tag's word
    /// points into their bytes.
    pub freed: u64,
    pub kept_by_pointer: usize,
}

impl Pruned {
    /// Tags dropped.
    pub fn tags(&self) -> usize {
        self.groups.values().sum()
    }
}

/// Scenario palettes whose base entries are nulled: `(offset, element)`,
/// the tag reference at `+0`. Scenery `+0x58`, sound scenery `+0xe0`,
/// light fixtures `+0xf0`, crates `+0x330` (`tags::object::OBJECT_TYPES`;
/// every palette is `0x28` an element, `tags::schema::infer::ENGINE_STRIDES`).
pub const BASE_PALETTES: [(u32, usize); 4] =
    [(0x58, 0x28), (0xe0, 0x28), (0xf0, 0x28), (0x330, 0x28)];

/// The scenario's tag list (`+0x3d8`, a datum each).
pub const SCENARIO_RESOURCES: u32 = 0x3d8;

/// Groups kept whole, reached or not: what the engine may find without a
/// reference from the scenario or the globals. None is confirmed in the
/// XBE yet; these are the groups retail maps hold one of (or that hold
/// shared tables: the sound gestalt, strings, vertex and shader programs).
pub const KEEP_GROUPS: [&[u8; 4]; 11] = [
    b"matg", b"scnr", b"ugh!", b"unic", b"mulg", b"stem", b"spas", b"vrtx", b"pmov", b"sncl",
    b"snmx",
];

/// Tags kept whole by name prefix: the engine's own defaults, which
/// templates name in ways the tree walk doesn't see (D: a `stem` names an
/// `spas` the fresh tree holds no word of).
pub const KEEP_PREFIXES: [&str; 2] = ["shaders\\", "rasterizer\\"];

/// What [`prune_with`] keeps besides what the scenario and the globals
/// reach, and which scenario palettes it empties of base entries.
#[derive(Debug, Clone, Copy)]
pub struct Keep<'a> {
    /// Scenario palettes whose base entries are nulled first.
    pub palettes: &'a [(u32, usize)],
    /// Groups kept whole, and what they reach.
    pub groups: &'a [&'a [u8; 4]],
    /// Tags kept by name prefix, and what they reach.
    pub prefixes: &'a [&'a str],
}

/// A multiplayer map's: [`BASE_PALETTES`], [`KEEP_GROUPS`], [`KEEP_PREFIXES`].
pub const MULTIPLAYER: Keep<'static> = Keep {
    palettes: &BASE_PALETTES,
    groups: &KEEP_GROUPS,
    prefixes: &KEEP_PREFIXES,
};

fn child_mut(b: &mut map::build::Block, at: u32) -> Option<(u32, &mut map::build::Block)> {
    b.fixups.iter_mut().find_map(|f| match &mut f.kind {
        FixupKind::Block { count, child } if f.offset == at && *count > 0 => Some((*count, child)),
        _ => None,
    })
}

/// Every datum a payload names: 4-aligned words of every block (a word
/// that happens to equal a datum only keeps a tag), tag-reference and
/// datum fixups, and an alias's owner.
fn named(payload: &TagPayload, out: &mut Vec<u32>) {
    fn walk(b: &map::build::Block, out: &mut Vec<u32>) {
        out.extend(
            b.bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|w| u32::from_le_bytes(*w)),
        );
        for f in &b.fixups {
            match &f.kind {
                FixupKind::Block { child, .. }
                | FixupKind::Data(child)
                | FixupKind::Pointer(child) => walk(child, out),
                FixupKind::TagRef(t) | FixupKind::TagId(t) => out.push(t.0),
                FixupKind::TagOffset { tag, .. } => out.push(tag.0),
                _ => {}
            }
        }
    }
    match payload {
        TagPayload::Built(b) => walk(b, out),
        TagPayload::Pinned { bytes, .. } => out.extend(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|w| u32::from_le_bytes(*w)),
        ),
        TagPayload::Alias { tag, .. } => out.push(tag.0),
        TagPayload::Empty => {}
    }
}

/// The slots reachable from the scenario, the globals, `keep`'s roots and
/// the tags `extra`.
pub fn reachable(set: &TagSet, keep: &Keep, extra: &[TagId]) -> Vec<bool> {
    let n = set.slots.len();
    let mut seen = vec![false; n];
    let mut todo = Vec::new();
    for (i, s) in set.slots.iter().enumerate() {
        let Some(t) = &s.tag else { continue };
        if t.id == set.scenario
            || t.id == set.globals
            || keep.groups.iter().any(|g| t.group == FourCC(**g))
            || extra.contains(&t.id)
            || s.name
                .as_deref()
                .is_some_and(|n| keep.prefixes.iter().any(|p| n.starts_with(p)))
        {
            seen[i] = true;
            todo.push(i);
        }
    }
    let mut words = Vec::new();
    while let Some(i) = todo.pop() {
        let Some(t) = &set.slots[i].tag else { continue };
        words.clear();
        named(&t.payload, &mut words);
        for &w in &words {
            let k = (w & 0xffff) as usize;
            if k < n && !seen[k] && set.slots[k].tag.as_ref().is_some_and(|t| t.id.0 == w) {
                seen[k] = true;
                todo.push(k);
            }
        }
    }
    seen
}

/// Drops the base's tags nothing reaches from a fresh set (`first` is the
/// first slot the conversion added; those are never dropped):
///
/// 1. each base tag in the scenario's [`BASE_PALETTES`] is nulled (no
///    placement names it: the conversion removed the base's placements);
/// 2. the scenario's tag list ([`SCENARIO_RESOURCES`]) keeps a base tag
///    only when something else reaches it;
/// 3. every base slot then unreachable ([`reachable`]) is emptied, its
///    datum named by nothing. The tags in `extra`, and what they reach,
///    stay.
pub fn prune(set: &mut TagSet, first: usize, extra: &[TagId]) -> Pruned {
    prune_with(set, first, &MULTIPLAYER, extra)
}

/// [`prune`] with other roots and palettes (a campaign level's:
/// `h1campaign::KEEP`).
pub fn prune_with(set: &mut TagSet, first: usize, keep: &Keep, extra: &[TagId]) -> Pruned {
    let mut out = Pruned::default();
    let scenario = set.scenario;
    let is_base = |w: u32| ((w & 0xffff) as usize) < first && w != u32::MAX;
    if let Some(TagPayload::Built(root)) = set.tag_mut(scenario).map(|t| &mut t.payload) {
        for &(at, size) in keep.palettes {
            if let Some((count, palette)) = child_mut(root, at) {
                for k in 0..count as usize {
                    let e = k * size;
                    let Some(d) = palette.bytes.get(e + 4..e + 8) else {
                        break;
                    };
                    let d = u32::from_le_bytes(d.try_into().expect("4"));
                    if is_base(d) {
                        palette.bytes[e..e + 8].fill(0xff);
                        out.palette_entries += 1;
                    }
                }
            }
        }
    }
    // The tag list, against what the rest reaches.
    let list = match set.tag_mut(scenario).map(|t| &mut t.payload) {
        Some(TagPayload::Built(root)) => {
            child_mut(root, SCENARIO_RESOURCES).map(|(_, b)| std::mem::take(&mut b.bytes))
        }
        _ => None,
    };
    if let Some(list) = list {
        let seen = reachable(set, keep, extra);
        let kept: Vec<u8> = list
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|w| {
                let d = u32::from_le_bytes(**w);
                let ok = !is_base(d) || seen.get((d & 0xffff) as usize).copied().unwrap_or(false);
                if !ok {
                    out.resource_entries += 1;
                }
                ok
            })
            .flatten()
            .copied()
            .collect();
        if let Some(TagPayload::Built(root)) = set.tag_mut(scenario).map(|t| &mut t.payload) {
            for f in &mut root.fixups {
                if f.offset == SCENARIO_RESOURCES
                    && let FixupKind::Block { count, child } = &mut f.kind
                {
                    *count = (kept.len() / 4) as u32;
                    child.bytes = kept.clone();
                }
            }
        }
    }
    let seen = reachable(set, keep, extra);
    for (i, s) in set.slots.iter_mut().enumerate().take(first) {
        if seen[i] {
            continue;
        }
        if let Some(t) = s.tag.take() {
            *out.groups.entry(t.group.to_string()).or_default() += 1;
        }
    }
    out
}

/// Where a metadata address of a pinned set lies: a pinned tag's bytes
/// (slot, offset) or the metadata no tag owns (region, offset).
#[derive(Clone, Copy)]
enum At {
    Slot(usize, usize),
    Unowned(usize, usize),
}

fn locate(set: &TagSet, addr: u32, len: usize) -> Option<At> {
    let inside = |base: u32, n: usize| {
        addr.checked_sub(base)
            .map(|o| o as usize)
            .filter(|o| o + len <= n)
    };
    for (i, s) in set.slots.iter().enumerate() {
        if let Some(TagPayload::Pinned { address, bytes }) = s.tag.as_ref().map(|t| &t.payload)
            && let Some(o) = inside(*address, bytes.len())
        {
            return Some(At::Slot(i, o));
        }
    }
    let p = set.pinned.as_ref()?;
    p.unowned
        .iter()
        .enumerate()
        .find_map(|(k, (a, b))| Some(At::Unowned(k, inside(*a, b.len())?)))
}

fn bytes_at(set: &mut TagSet, at: At) -> Option<&mut [u8]> {
    match at {
        At::Slot(i, o) => match &mut set.slots[i].tag.as_mut()?.payload {
            TagPayload::Pinned { bytes, .. } => bytes.get_mut(o..),
            _ => None,
        },
        At::Unowned(k, o) => set.pinned.as_mut()?.unowned.get_mut(k)?.1.get_mut(o..),
    }
}

fn read_word(set: &mut TagSet, addr: u32) -> Option<u32> {
    let at = locate(set, addr, 4)?;
    let b = bytes_at(set, at)?;
    Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?))
}

fn write_word(set: &mut TagSet, addr: u32, w: u32) -> Option<()> {
    let at = locate(set, addr, 4)?;
    bytes_at(set, at)?
        .get_mut(..4)?
        .copy_from_slice(&w.to_le_bytes());
    Some(())
}

/// A pinned scenario's block field `at`: its address, count and elements'
/// address (`+0` count, `+4` address, as every Xbox block field).
fn pinned_block(set: &mut TagSet, at: u32) -> Option<(u32, u32, u32)> {
    let scenario = set.scenario;
    let address = match &set.tag(scenario)?.payload {
        TagPayload::Pinned { address, .. } => *address,
        _ => return None,
    };
    let field = address + at;
    let count = read_word(set, field)?;
    let elements = read_word(set, field + 4)?;
    (count > 0 && count < 0x10000).then_some((field, count, elements))
}

/// What a pinned set reaches ([`reachable`] from `keep`, the scenario,
/// the globals and `roots`), with what a pinned map holds beyond datums:
/// metadata no tag owns (blocks the scenario editor grew) is live when a
/// reached tag's word, or a live region's, points into it, and its datums
/// are reached; a base tag (slot under `first`) that a reached tag's or a
/// live region's word points into is reached, with what it reaches.
/// `roots` gains those tags; returns the slots reached and how many tags
/// were kept for a pointer.
fn pinned_reach(
    set: &TagSet,
    keep: &Keep,
    first: usize,
    roots: &mut Vec<TagId>,
) -> (Vec<bool>, usize) {
    let n = set.slots.len();
    let regions: Vec<(u32, u32, Vec<u32>)> = set.pinned.as_ref().map_or(Vec::new(), |p| {
        let mut v: Vec<(u32, u32, Vec<u32>)> = p
            .unowned
            .iter()
            .map(|(a, b)| {
                let words = b
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|w| u32::from_le_bytes(*w))
                    .collect();
                (*a, *a + b.len() as u32, words)
            })
            .collect();
        v.sort_by_key(|r| r.0);
        v
    });
    let find = |ranges: &[(u32, u32, usize)], w: u32| {
        let k = ranges.partition_point(|r| r.0 <= w);
        (k > 0 && w < ranges[k - 1].1).then(|| ranges[k - 1].2)
    };
    let region_ranges: Vec<(u32, u32, usize)> = regions
        .iter()
        .enumerate()
        .map(|(k, r)| (r.0, r.1, k))
        .collect();
    let mut by_pointer = 0;
    loop {
        let seen = reachable(set, keep, roots);
        let mut tags: Vec<(u32, u32, usize)> = set
            .slots
            .iter()
            .enumerate()
            .take(first)
            .filter(|(i, _)| !seen[*i])
            .filter_map(|(i, s)| match &s.tag.as_ref()?.payload {
                TagPayload::Pinned { address, bytes } => {
                    Some((*address, *address + bytes.len() as u32, i))
                }
                _ => None,
            })
            .collect();
        tags.sort_unstable();
        let mut words = Vec::new();
        for (i, s) in set.slots.iter().enumerate() {
            if seen[i]
                && let Some(t) = &s.tag
            {
                named(&t.payload, &mut words);
            }
        }
        // Live regions, and their words in turn.
        let mut live = vec![false; regions.len()];
        let mut k = 0;
        while k < words.len() {
            if let Some(r) = find(&region_ranges, words[k])
                && !live[r]
            {
                live[r] = true;
                words.extend_from_slice(&regions[r].2);
            }
            k += 1;
        }
        let mut new = std::collections::BTreeSet::new();
        for &w in &words {
            if let Some(i) = find(&tags, w) {
                new.insert(i);
            }
        }
        let pointed = new.len();
        for (r, region) in regions.iter().enumerate() {
            if !live[r] {
                continue;
            }
            for &w in &region.2 {
                let i = (w & 0xffff) as usize;
                if i < n && !seen[i] && set.slots[i].tag.as_ref().is_some_and(|t| t.id.0 == w) {
                    new.insert(i);
                }
            }
        }
        if new.is_empty() {
            return (seen, by_pointer);
        }
        by_pointer += pointed;
        roots.extend(
            new.into_iter()
                .filter_map(|i| Some(set.slots[i].tag.as_ref()?.id)),
        );
    }
}

/// [`prune_with`] for a pinned set: the map `Prepared::assemble` builds
/// keeps the base's metadata in place, every retail tag of it, reached or
/// not, and puts what the conversion built after it, so a level that adds
/// a lot runs past the engine's tag memory. The same roots ([`Keep`],
/// the scenario and globals, `extra`) and the same scenario edits, made
/// in the pinned scenario's bytes: base entries of `keep.palettes`
/// nulled, the tag list ([`SCENARIO_RESOURCES`]) keeping a base tag only
/// when something else reaches it. Then every pinned base tag nothing
/// reaches is dropped, its bytes free for the built tags
/// (`TagSet::reuse_free_space`). A pinned tag keeps addresses into other
/// tags' bytes and into metadata no tag owns, which reachability (datums)
/// doesn't see ([`pinned_reach`]). Only pinned payloads are dropped (the others hold
/// no base metadata). `first` is the first slot the conversion added.
pub fn prune_pinned(set: &mut TagSet, first: usize, keep: &Keep, extra: &[TagId]) -> Pruned {
    let mut out = Pruned::default();
    let is_base = |w: u32| ((w & 0xffff) as usize) < first && w != u32::MAX;
    for &(at, size) in keep.palettes {
        let Some((_, count, elements)) = pinned_block(set, at) else {
            continue;
        };
        for k in 0..count {
            let e = elements + k * size as u32;
            if read_word(set, e + 4).is_some_and(is_base) {
                write_word(set, e, u32::MAX);
                write_word(set, e + 4, u32::MAX);
                out.palette_entries += 1;
            }
        }
    }
    let mut roots = extra.to_vec();
    if let Some((field, count, elements)) = pinned_block(set, SCENARIO_RESOURCES) {
        let list: Option<Vec<u32>> = (0..count)
            .map(|k| read_word(set, elements + 4 * k))
            .collect();
        if let Some(list) = list {
            // Against what the rest reaches: the list itself left out.
            for k in 0..count {
                write_word(set, elements + 4 * k, u32::MAX);
            }
            let (seen, _) = pinned_reach(set, keep, first, &mut roots.clone());
            let kept: Vec<u32> = list
                .iter()
                .copied()
                .filter(|&d| {
                    !is_base(d) || seen.get((d & 0xffff) as usize).copied().unwrap_or(false)
                })
                .collect();
            out.resource_entries = list.len() - kept.len();
            for (k, d) in kept.iter().enumerate() {
                write_word(set, elements + 4 * k as u32, *d);
            }
            write_word(set, field, kept.len() as u32);
        }
    }
    let (seen, by_pointer) = pinned_reach(set, keep, first, &mut roots);
    out.kept_by_pointer = by_pointer;
    for (i, s) in set.slots.iter_mut().enumerate().take(first) {
        if seen[i] {
            continue;
        }
        let dropped = match &s.tag {
            Some(t) => match &t.payload {
                TagPayload::Pinned { bytes, .. } => Some((bytes.len() as u64, t.group)),
                _ => None,
            },
            None => None,
        };
        if let Some((len, group)) = dropped {
            out.freed += len;
            *out.groups.entry(group.to_string()).or_default() += 1;
            s.tag = None;
        }
    }
    out
}

#[cfg(test)]
mod pinned_prune_tests {
    use super::*;
    use map::CacheType;
    use map::build::{PinnedImage, Slot, TagEntry};

    const BASE: u32 = 0x8000_0000;

    fn id(i: usize) -> TagId {
        TagId(((0xe174 + i as u32) << 16) | i as u32)
    }

    fn put(bytes: &mut [u8], at: usize, w: u32) {
        bytes[at..at + 4].copy_from_slice(&w.to_le_bytes());
    }

    /// A pinned set: a scenario, globals and four base tags, each at its
    /// address; metadata no tag owns at `+0xa00` and `+0xc00`.
    fn set() -> TagSet {
        let mut set = TagSet::new(CacheType::Multiplayer, "m", "s");
        let tags: [(&[u8; 4], &str, u32, usize); 6] = [
            (b"scnr", "s", 0, 0x400),
            (b"matg", "globals", 0x400, 0x40),
            (b"bitm", "pointed", 0x800, 0x40),
            (b"shad", "named from a live gap", 0x900, 0x40),
            (b"bitm", "named from a dead gap", 0xb00, 0x40),
            (b"mode", "nothing", 0xd00, 0x40),
        ];
        for (i, (g, name, at, len)) in tags.iter().enumerate() {
            set.slots.push(Slot {
                name: Some((*name).into()),
                tag: Some(TagEntry {
                    id: id(i),
                    group: FourCC(**g),
                    payload: TagPayload::Pinned {
                        address: BASE + at,
                        bytes: vec![0; *len],
                    },
                }),
            });
        }
        set.scenario = id(0);
        set.globals = id(1);
        let mut live = vec![0u8; 0x10];
        put(&mut live, 4, id(3).0);
        let mut dead = vec![0u8; 0x10];
        put(&mut dead, 4, id(4).0);
        set.pinned = Some(PinnedImage {
            header: Vec::new(),
            file: Vec::new(),
            tables: [(0, 0); 5],
            index: Vec::new(),
            index_entries_end: 0,
            index_size: 0,
            bsp_size: 0,
            metadata_base: BASE,
            metadata_size: 0x1000,
            unowned: vec![(BASE + 0xa00, live), (BASE + 0xc00, dead)],
        });
        // The scenario points into tag 2's bytes and into the first gap.
        if let Some(TagPayload::Pinned { bytes, .. }) =
            set.slots[0].tag.as_mut().map(|t| &mut t.payload)
        {
            put(bytes, 0x10, BASE + 0x804);
            put(bytes, 0x14, BASE + 0xa00);
        }
        set
    }

    const NOTHING: Keep<'static> = Keep {
        palettes: &[],
        groups: &[],
        prefixes: &[],
    };

    /// A pinned map holds what no datum names: a kept tag's address into
    /// another tag's bytes, and datums in metadata no tag owns that a kept
    /// tag points into (blocks the scenario editor grew). Those stay; a
    /// gap nothing points into keeps nothing, and the rest is dropped.
    #[test]
    fn a_pinned_set_keeps_what_its_addresses_reach() {
        let mut set = set();
        let p = prune_pinned(&mut set, 6, &NOTHING, &[]);
        let kept: Vec<bool> = set.slots.iter().map(|s| s.tag.is_some()).collect();
        assert_eq!(kept, [true, true, true, true, false, false]);
        assert_eq!(p.tags(), 2);
        assert_eq!(p.freed, 0x80);
        assert_eq!(p.kept_by_pointer, 1);
        // A conversion's tag (from `first` on) is never dropped.
        let mut set = super::pinned_prune_tests::set();
        prune_pinned(&mut set, 5, &NOTHING, &[]);
        assert!(set.slots[5].tag.is_some());
        assert!(set.slots[4].tag.is_none());
    }
}
