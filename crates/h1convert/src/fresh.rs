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
        let (pinned, bytes, section, pinned_pruned) = self.assemble(base, mopp, true)?;
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
    let mut level = MapSource::new(schema, level_map, &mut shared_level).map_err(repo)?;
    level.add_regions(grown(pinned).1);
    let mut src = LevelSource {
        base: MapSource::new(schema, base_map, &mut shared_base).map_err(repo)?,
        level,
        from_level,
    };
    let from_level = src.from_level.clone();
    build(&mut src, fresh, &MULTIPLAYER, extra, &from_level, low)
}

/// The pinned tags of `pinned` an edit grew past their own bytes, and the
/// arrays it wrote for them (metadata no tag owns), from
/// [`map::build::PinnedImage::grown`]: a reader of the map `pinned` builds
/// reads those arrays, which no tag's bytes hold.
pub fn grown(pinned: &TagSet) -> (Vec<TagId>, Vec<(u32, Vec<u8>)>) {
    let Some(p) = &pinned.pinned else {
        return (Vec::new(), Vec::new());
    };
    let tags = p.grown.iter().map(|&(id, _)| id).collect();
    let arrays = p
        .unowned
        .iter()
        .filter(|(at, _)| p.grown.iter().any(|&(_, a)| a == *at))
        .cloned()
        .collect();
    (tags, arrays)
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
    // Pinned tags an edit grew (the globals' damage table,
    // `h1objects::player_damage`): their new arrays read, their replaced
    // ones left out.
    let (grown_tags, grown_arrays) = grown(pinned);
    if !grown_tags.is_empty() {
        u.add_grown(src, grown_tags, grown_arrays);
    }
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
    const EXTRA_RAW: usize = 8;
    let extra = |e: &h1sounds::gestalt::Nested| word(&e.bytes, EXTRA_RAW as u32);
    u.add_starts(g.chunks.iter().filter_map(|c| match c.pointer {
        ChunkPointer::Stored(p) => Some(p),
        ChunkPointer::Local(_) => None,
    }));
    u.add_starts(g.nested[blocks::EXTRA_INFOS].iter().map(extra));
    let mut copied = 0;
    for (k, e) in g.nested[blocks::EXTRA_INFOS].iter_mut().enumerate() {
        let p = extra(e);
        if p != 0 && p >> 30 == 0 {
            let len = word(&e.bytes, 0xc);
            let raw = u.copy_raw(src, fresh, p, len)
                .map_err(|e| Error::Fresh(format!("lip-sync data at {p:#x}: {e}")))?;
            e.bytes[EXTRA_RAW..EXTRA_RAW + 4].fill(0);
            g.extra_raws.insert(k, raw);
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
    let block = g.to_block();
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
    /// [`prune_pinned`]: each kept slot's datum and the edge it was first
    /// reached by, to follow what keeps a tag back to its root.
    pub why: std::collections::HashMap<TagId, Reach>,
    /// [`prune_pinned`]: the kept pinned tags read word by word, every
    /// word a datum and an address, because the schema doesn't know their
    /// group or their root doesn't read: stale words in them keep tags.
    pub word_read: Vec<TagId>,
    /// [`prune_pinned`]: the kept pinned tags walked by the schema with
    /// some array skipped ([`walk`]: outside the metadata, or implausibly
    /// long); their own words name tags too, as datums only.
    pub partly_walked: Vec<TagId>,
    /// [`prune_pinned`]: the tags nothing reaches kept only because a
    /// kept tag's block sits in their bytes ([`Reach::Holds`]); and how
    /// many words of the kept pinned tags named a dropped tag and were
    /// nulled (a holder's own references, and stale words no array of a
    /// reached tag covers: a removed placement's, an emptied block's old
    /// elements).
    pub holders: Vec<TagId>,
    pub nulled_words: usize,
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

/// A pinned set's metadata by address: each pinned tag's bytes and each
/// stretch no tag owns, sorted, for reading words at an address.
struct Space<'a> {
    /// (start, end, bytes, owner slot or `None` for unowned).
    parts: Vec<(u32, u32, &'a [u8], Option<usize>)>,
}

impl<'a> Space<'a> {
    fn new(set: &'a TagSet) -> Self {
        let mut parts = Vec::new();
        for (i, s) in set.slots.iter().enumerate() {
            // A size-0 tag (an alias, its root in another tag's bytes)
            // owns no byte: as a part it would hide the tag it sits in
            // from [`Space::part`].
            if let Some(TagPayload::Pinned { address, bytes }) = s.tag.as_ref().map(|t| &t.payload)
                && !bytes.is_empty()
            {
                parts.push((*address, *address + bytes.len() as u32, &bytes[..], Some(i)));
            }
        }
        if let Some(p) = &set.pinned {
            for (a, b) in &p.unowned {
                parts.push((*a, *a + b.len() as u32, &b[..], None));
            }
        }
        parts.sort_by_key(|p| p.0);
        Self { parts }
    }

    /// The part holding `addr`, by index.
    fn part(&self, addr: u32) -> Option<usize> {
        let i = self.parts.partition_point(|p| p.0 <= addr).checked_sub(1)?;
        (addr < self.parts[i].1).then_some(i)
    }

    fn word(&self, addr: u32) -> Option<u32> {
        let p = &self.parts[self.part(addr)?];
        let o = (addr - p.0) as usize;
        Some(u32::from_le_bytes(p.2.get(o..o + 4)?.try_into().ok()?))
    }
}

/// What one pinned element array names, by the schema (`s`, `count`
/// elements at `at`), depth first through its blocks: every word of every
/// array it holds (as the fresh build's [`named`] reads a built tree: a
/// datum may sit in a field the schema doesn't describe), and every
/// array below it as its address and length in bytes (the count times
/// the element size of the block field pointing at it). Bytes no array covers (an emptied block's old
/// elements, a removed placement's) are not read. An array below the
/// root that is not in the metadata or is implausibly long is skipped
/// (counted in `skipped`), its pointer not an address: only the fields the
/// schema says are blocks give addresses, never a data word. `None` when
/// the root array itself doesn't read: the caller then reads the tag word
/// by word.
#[allow(clippy::too_many_arguments)]
fn walk(
    space: &Space,
    s: &tags::schema::Struct,
    at: u32,
    count: u32,
    depth: usize,
    words: &mut Vec<u32>,
    arrays: &mut Vec<(u32, u32)>,
    skipped: &mut usize,
) -> Option<()> {
    if depth > 32 || count > 0x10_0000 {
        return None;
    }
    let len = count.checked_mul(s.size)?;
    let mut own = Vec::with_capacity(len as usize / 4);
    for o in (0..len / 4).map(|k| k * 4) {
        own.push(space.word(at.checked_add(o)?)?);
    }
    words.extend(own);
    for e in 0..count {
        let base = at + e * s.size;
        for (off, f) in &s.fields {
            if let Field::Block(child) = f {
                let n = space.word(base + off)?;
                let p = space.word(base + off + 4)?;
                if n > 0 && p != 0 {
                    // An array that doesn't read (outside the metadata,
                    // implausibly long: a field the schema takes for a
                    // block that isn't one) is skipped, and its pointer is
                    // not an address the tag holds.
                    let (mut w, mut a) = (Vec::new(), Vec::new());
                    match walk(space, child, p, n, depth + 1, &mut w, &mut a, skipped) {
                        Some(()) => {
                            arrays.push((p, n * child.size));
                            words.extend(w);
                            arrays.extend(a);
                        }
                        None => *skipped += 1,
                    }
                }
            }
        }
    }
    Some(())
}

/// What a pinned set reaches from `keep`, the scenario, the globals and
/// `roots`, as the fresh build would see it. A pinned tag with a schema
/// layout names what its arrays hold ([`walk`], from its root through
/// every block, so stale words outside them, an emptied block's old
/// elements or a removed placement's, keep nothing), and points at its
/// blocks' arrays only; one without (or whose root doesn't read) names
/// every word in it, as a datum and as an address. A built tag names its
/// fixups and words ([`named`]), as datums only: its own blocks are laid
/// out when it is built, and a word in it that falls in another tag's
/// bytes is data (V: the sound gestalt rebuilt with Halo 1's sounds holds
/// raw offsets such as `0x804f3600`, inside Coagulation's wraith model,
/// which kept every retail vehicle).
///
/// Retail shares block data between tags: a walked tag's block can sit
/// in another tag's bytes (V: the `c_turret_ap` model's block at
/// `0x804f4164`, inside the wraith model; the compiler shares identical
/// function data, a `jpt!`'s `[0x6c, 0x44]`). Such a block keeps the
/// datums in it (its count times its element size, and each block below
/// it the same way), which the walk has read, and the tag it falls in is
/// kept as a holder of those bytes ([`Reached::holder`], [`Reach::Holds`])
/// but not reached: its own references keep nothing. The holder stays a
/// whole tag because the schema moves a block outside its tag only when
/// it sits inside another tag (`tags::schema::walk`): kept as metadata no
/// tag owns, the fresh build refused it (converter version 97, round 9:
/// 1288 references out of bounds). A holder's own blocks that sit in a
/// third tag make that one a holder too. One whose root doesn't read by
/// the schema is reached whole, as a word-read tag's address keeps the
/// tag it falls in (it has no length), and so is every datum in metadata
/// no tag owns that a word-read tag points into.
fn pinned_reach(set: &TagSet, keep: &Keep, roots: &[TagId]) -> Reached {
    let n = set.slots.len();
    let space = Space::new(set);
    let schema = Schema::builtin();
    let mut seen = vec![false; n];
    let mut todo = Vec::new();
    let mut by_pointer = 0;
    let mut why: Vec<Option<Reach>> = vec![None; n];
    let mut mark = |i: usize, how: Reach, seen: &mut Vec<bool>, todo: &mut Vec<usize>| {
        if i < n && !seen[i] && set.slots[i].tag.is_some() {
            seen[i] = true;
            why[i] = Some(how);
            todo.push(i);
            true
        } else {
            false
        }
    };
    for (i, s) in set.slots.iter().enumerate() {
        let Some(t) = &s.tag else { continue };
        let root = if t.id == set.scenario {
            Some("scenario")
        } else if t.id == set.globals {
            Some("globals")
        } else if keep.groups.iter().any(|g| t.group == FourCC(**g)) {
            Some("kept group")
        } else if roots.contains(&t.id) {
            Some("caller's root")
        } else if s
            .name
            .as_deref()
            .is_some_and(|n| keep.prefixes.iter().any(|p| n.starts_with(p)))
        {
            Some("kept name prefix")
        } else {
            None
        };
        if let Some(r) = root {
            mark(i, Reach::Root(r), &mut seen, &mut todo);
        }
    }
    let datum = |w: u32| {
        let k = (w & 0xffff) as usize;
        (k < n && set.slots[k].tag.as_ref().is_some_and(|t| t.id.0 == w)).then_some(k)
    };
    let mut live_gaps = std::collections::HashSet::new();
    let (mut datums, mut arrays, mut blocks) = (Vec::new(), Vec::new(), Vec::new());
    let mut gap_datums: Vec<(u32, u32)> = Vec::new();
    let mut word_read = Vec::new();
    let mut partly = Vec::new();
    let mut holder = vec![false; n];
    let mut held_by: Vec<Option<(TagId, u32)>> = vec![None; n];
    let mut held: Vec<usize> = Vec::new();
    loop {
        let Some(i) = todo.pop() else {
            // A holder: only its blocks count, for the bytes they use.
            let Some(c) = held.pop() else { break };
            if seen[c] {
                continue;
            }
            let Some(t) = &set.slots[c].tag else { continue };
            let walked = match &t.payload {
                TagPayload::Pinned { address, .. } => schema.groups.get(&t.group).and_then(|s| {
                    let (mut d, mut a, mut skipped) = (Vec::new(), Vec::new(), 0);
                    walk(&space, s, *address, 1, 0, &mut d, &mut a, &mut skipped).map(|()| a)
                }),
                _ => None,
            };
            match walked {
                Some(blocks) => hold(
                    &space,
                    c,
                    t.id,
                    &blocks,
                    &seen,
                    &mut holder,
                    &mut held_by,
                    &mut held,
                ),
                None => {
                    if let Some((by, word)) = held_by[c]
                        && mark(c, Reach::Address { by, word }, &mut seen, &mut todo)
                    {
                        by_pointer += 1;
                    }
                }
            }
            continue;
        };
        let Some(t) = &set.slots[i].tag else { continue };
        let by = t.id;
        datums.clear();
        arrays.clear();
        blocks.clear();
        gap_datums.clear();
        let mut words_too = false;
        match &t.payload {
            TagPayload::Pinned { address, bytes } => {
                let walked = schema.groups.get(&t.group).and_then(|s| {
                    let (mut d, mut a, mut skipped) = (Vec::new(), Vec::new(), 0);
                    walk(&space, s, *address, 1, 0, &mut d, &mut a, &mut skipped)
                        .map(|()| (d, a, skipped))
                });
                match walked {
                    Some((d, a, skipped)) => {
                        datums = d;
                        blocks = a;
                        // Its root too: a size-0 alias's lies in another
                        // tag's bytes, which then holds it (in its own
                        // bytes, [`hold`] passes over it).
                        if let Some(s) = schema.groups.get(&t.group) {
                            blocks.push((*address, s.size));
                        }
                        // Arrays it couldn't walk: its own words still
                        // name tags (a datum may sit in one), but none is
                        // taken for an address.
                        if skipped > 0 {
                            datums.extend(
                                bytes
                                    .as_chunks::<4>()
                                    .0
                                    .iter()
                                    .map(|w| u32::from_le_bytes(*w)),
                            );
                            partly.push(by);
                        }
                    }
                    None => {
                        datums.extend(
                            bytes
                                .as_chunks::<4>()
                                .0
                                .iter()
                                .map(|w| u32::from_le_bytes(*w)),
                        );
                        words_too = true;
                        word_read.push(by);
                    }
                }
            }
            // A built tag names what its fixups and words name; it holds
            // no address into the pinned metadata (its blocks are laid out
            // when it is built), so its data words (a rebuilt sound
            // gestalt's raw offsets) are not taken for addresses.
            other => named(other, &mut datums),
        }
        // A walked block in another tag's bytes: that tag holds it.
        hold(
            &space,
            i,
            by,
            &blocks,
            &seen,
            &mut holder,
            &mut held_by,
            &mut held,
        );
        if words_too {
            arrays.extend(datums.iter().copied());
        }
        // A word-read tag's addresses: the tag or gap holding each.
        let mut k = 0;
        while k < arrays.len() {
            let a = arrays[k];
            k += 1;
            let Some(p) = space.part(a) else { continue };
            match space.parts[p].3 {
                Some(owner) if owner != i => {
                    if mark(owner, Reach::Address { by, word: a }, &mut seen, &mut todo) {
                        by_pointer += 1;
                    }
                }
                None if live_gaps.insert(p) => {
                    // A gap a word-read tag points into: its words name
                    // tags and point on.
                    let words: Vec<u32> = space.parts[p]
                        .2
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|w| u32::from_le_bytes(*w))
                        .collect();
                    let gap = space.parts[p].0;
                    gap_datums.extend(words.iter().map(|w| (*w, gap)));
                    arrays.extend(words);
                }
                _ => {}
            }
        }
        for &w in &datums {
            if let Some(k) = datum(w) {
                mark(k, Reach::Datum { by }, &mut seen, &mut todo);
            }
        }
        for &(w, gap) in &gap_datums {
            if let Some(k) = datum(w) {
                mark(k, Reach::Gap { by, gap }, &mut seen, &mut todo);
            }
        }
    }
    for (k, h) in held_by.iter().enumerate() {
        if let Some((by, block)) = h
            && !seen[k]
        {
            why[k] = Some(Reach::Holds {
                by: *by,
                block: *block,
            });
        }
    }
    Reached {
        seen,
        by_pointer,
        why,
        word_read,
        partly,
        holder,
    }
}

/// Makes each tag but `i` whose bytes hold part of one of `blocks` (`i`'s,
/// as address and length) a holder ([`pinned_reach`]), unless reached.
#[allow(clippy::too_many_arguments)]
fn hold(
    space: &Space,
    i: usize,
    by: TagId,
    blocks: &[(u32, u32)],
    seen: &[bool],
    holder: &mut [bool],
    held_by: &mut [Option<(TagId, u32)>],
    held: &mut Vec<usize>,
) {
    for &(a, len) in blocks {
        let end = a.saturating_add(len);
        let Some(mut p) = space.part(a) else { continue };
        while p < space.parts.len() && space.parts[p].0 < end {
            if let Some(o) = space.parts[p].3
                && o != i
                && !seen[o]
                && !holder[o]
            {
                holder[o] = true;
                held_by[o] = Some((by, a));
                held.push(o);
            }
            p += 1;
        }
    }
}

/// What [`pinned_reach`] found.
struct Reached {
    /// The slots reached.
    seen: Vec<bool>,
    /// How many were reached only by a word-read tag's address.
    by_pointer: usize,
    /// The edge each reached slot was first reached by (`None` for one
    /// not reached).
    why: Vec<Option<Reach>>,
    /// The pinned tags read word by word (no schema, or their root
    /// doesn't read) and those walked with arrays skipped ([`walk`]).
    word_read: Vec<TagId>,
    partly: Vec<TagId>,
    /// The slots kept only as holders of bytes a reached tag's block uses
    /// (see [`pinned_reach`]).
    holder: Vec<bool>,
}

/// How [`pinned_reach`] reached a slot first: the edge that kept it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// A root: the scenario, the globals, a [`Keep`] group or name
    /// prefix, or one of the caller's tags.
    Root(&'static str),
    /// A word of `by` is its datum.
    Datum { by: TagId },
    /// A word of `by` (`word`), a tag read word by word, is an address
    /// in its bytes.
    Address { by: TagId, word: u32 },
    /// A word in metadata no tag owns (starting at `gap`) that `by`
    /// points into is its datum.
    Gap { by: TagId, gap: u32 },
    /// Not reached: a block of `by` at `block` sits in its bytes, so it is
    /// kept for them, and its words naming a dropped tag are nulled
    /// ([`prune_pinned`]).
    Holds { by: TagId, block: u32 },
}

/// A scenario palette element's size (every object palette, D;
/// [`BASE_PALETTES`]).
const PALETTE_ENTRY: u32 = 0x28;

/// In a pinned scenario, each object type's palette entries
/// (`tags::object::OBJECT_TYPES`: placement block, palette block,
/// placement stride) that no placement names (its `+0` short, the palette
/// index) and that name a base tag (`is_base`): nulled, tag reference
/// `-1, -1`. A type whose blocks don't read is left alone. Returns the
/// entries nulled.
fn null_unplaced(set: &mut TagSet, is_base: impl Fn(u32) -> bool) -> usize {
    let scenario = set.scenario;
    let Some(address) = (match &set.tag(scenario).map(|t| &t.payload) {
        Some(TagPayload::Pinned { address, .. }) => Some(*address),
        _ => None,
    }) else {
        return 0;
    };
    let mut nulled = 0;
    for ty in tags::object::OBJECT_TYPES {
        let Some((placements, palette, stride)) = ty.placement else {
            continue;
        };
        let block = |set: &mut TagSet, at: u16| {
            let field = address + u32::from(at);
            Some((read_word(set, field)?, read_word(set, field + 4)?))
        };
        let (Some((n, p)), Some((entries, q))) = (block(set, placements), block(set, palette))
        else {
            continue;
        };
        if entries == 0 || entries > 0x1000 || n > 0x10000 {
            continue;
        }
        let mut used = std::collections::HashSet::new();
        let mut ok = true;
        for k in 0..n {
            match read_word(set, p + k * u32::from(stride)) {
                Some(w) => {
                    used.insert(w as u16 as i16);
                }
                None => ok = false,
            }
        }
        if !ok {
            continue;
        }
        for k in 0..entries {
            if used.contains(&(k as i16)) {
                continue;
            }
            let e = q + k * PALETTE_ENTRY;
            if read_word(set, e + 4).is_some_and(&is_base) {
                write_word(set, e, u32::MAX);
                write_word(set, e + 4, u32::MAX);
                nulled += 1;
            }
        }
    }
    nulled
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
/// tags' bytes and into metadata no tag owns, and stale words no array of
/// it covers: reachability walks it by the schema ([`pinned_reach`]).
/// A tag nothing reaches whose bytes hold a kept tag's block (retail block
/// data shared between tags) is kept whole for those bytes, and each word
/// of it naming a dropped tag is nulled, so it keeps nothing
/// ([`Pruned::holders`]). So is every word of a kept pinned tag naming a
/// dropped one: a stale word no array covers keeps nothing, and the map
/// keeps no word naming a tag it lacks ([`Pruned::nulled_words`]). Only pinned payloads are dropped (the others
/// hold no base metadata).
/// `first` is the first slot the conversion added.
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
    // Each object palette's base entries no placement names: the base's
    // placements the conversion removed named them, and nothing else
    // places from a palette ([`null_unplaced`]).
    out.palette_entries += null_unplaced(set, is_base);
    let roots = extra.to_vec();
    if let Some((field, count, elements)) = pinned_block(set, SCENARIO_RESOURCES) {
        let list: Option<Vec<u32>> = (0..count)
            .map(|k| read_word(set, elements + 4 * k))
            .collect();
        if let Some(list) = list {
            // Against what the rest reaches: the list itself left out.
            for k in 0..count {
                write_word(set, elements + 4 * k, u32::MAX);
            }
            let seen = pinned_reach(set, keep, &roots).seen;
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
    let Reached {
        seen,
        by_pointer,
        why,
        word_read,
        partly,
        holder,
    } = pinned_reach(set, keep, &roots);
    out.kept_by_pointer = by_pointer;
    out.word_read = word_read;
    out.partly_walked = partly;
    out.why = set
        .slots
        .iter()
        .zip(why)
        .filter_map(|(s, w)| Some((s.tag.as_ref()?.id, w?)))
        .collect();
    let gone: std::collections::HashSet<u32> = set
        .slots
        .iter()
        .enumerate()
        .take(first)
        .filter(|(i, _)| !seen[*i] && !holder[*i])
        .filter_map(|(_, s)| {
            let t = s.tag.as_ref()?;
            matches!(t.payload, TagPayload::Pinned { .. }).then_some(t.id.0)
        })
        .collect();
    for (i, s) in set.slots.iter_mut().enumerate().take(first) {
        if seen[i] {
            continue;
        }
        let Some(t) = &mut s.tag else { continue };
        let TagPayload::Pinned { bytes, .. } = &mut t.payload else {
            continue;
        };
        if holder[i] {
            out.holders.push(t.id);
            continue;
        }
        out.freed += bytes.len() as u64;
        *out.groups.entry(t.group.to_string()).or_default() += 1;
        s.tag = None;
    }
    // What stays names no dropped tag: a word of a kept pinned tag naming
    // one is one no array the reach walked covers (or a holder's own), so
    // nothing reads it; nulled as the engine finds a reference to nothing.
    for s in &mut set.slots {
        if let Some(TagPayload::Pinned { bytes, .. }) = s.tag.as_mut().map(|t| &mut t.payload) {
            for w in bytes.as_chunks_mut::<4>().0 {
                if gone.contains(&u32::from_le_bytes(*w)) {
                    *w = u32::MAX.to_le_bytes();
                    out.nulled_words += 1;
                }
            }
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
    const N: usize = 10;

    fn id(i: usize) -> TagId {
        TagId(((0xe174 + i as u32) << 16) | i as u32)
    }

    fn put(bytes: &mut [u8], at: usize, w: u32) {
        bytes[at..at + 4].copy_from_slice(&w.to_le_bytes());
    }

    fn bytes(set: &mut TagSet, i: usize) -> &mut Vec<u8> {
        match &mut set.slots[i].tag.as_mut().expect("tag").payload {
            TagPayload::Pinned { bytes, .. } => bytes,
            _ => panic!("slot {i} is not pinned"),
        }
    }

    /// A pinned set: a scenario and globals (zeros), a tag of a group the
    /// schema doesn't know (`zzzz`, read word by word) and an `MGS2`
    /// (walked by the schema: a block at `+0x8` of `0x98` elements, a tag
    /// reference at `+4`; `0x10` bytes after them), both roots; metadata no tag owns at `+0xa00`
    /// and `+0xc00`.
    fn set() -> TagSet {
        let mut set = TagSet::new(CacheType::Multiplayer, "m", "s");
        let tags: [(&[u8; 4], &str, u32, usize); N] = [
            (b"scnr", "s", 0, 0x400),
            (b"matg", "globals", 0x400, 0x40),
            (b"zzzz", "read word by word", 0x500, 0x40),
            (b"bitm", "pointed into", 0x900, 0x40),
            (b"shad", "named from a gap pointed into", 0x600, 0x40),
            (b"bitm", "named from a gap nothing points into", 0x700, 0x40),
            (b"MGS2", "walked", 0xe00, 0xb8),
            (b"bitm", "named by a field", 0x800, 0x40),
            (b"mode", "named by a word no field holds", 0xb00, 0x40),
            (b"mode", "named by nothing", 0xd00, 0x40),
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
        put(&mut live, 4, id(4).0);
        let mut dead = vec![0u8; 0x10];
        put(&mut dead, 4, id(5).0);
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
            grown: Vec::new(),
        });
        // `zzzz` points into tag 3's bytes and into the first gap.
        let b = bytes(&mut set, 2);
        put(b, 0x10, BASE + 0x904);
        put(b, 0x14, BASE + 0xa00);
        // `MGS2`: one element at `+0x10` whose reference names tag 7, and
        // after it a stale datum no array covers.
        let b = bytes(&mut set, 6);
        put(b, 0xa8, id(8).0);
        put(b, 0x8, 1);
        put(b, 0xc, BASE + 0xe10);
        put(b, 0x10 + 0x4, u32::from_be_bytes(*b"bitm"));
        put(b, 0x10 + 0x8, id(7).0);
        set
    }

    const NOTHING: Keep<'static> = Keep {
        palettes: &[],
        groups: &[],
        prefixes: &[],
    };

    /// A pinned map holds what no datum names: an address into another
    /// tag's bytes, and datums in metadata no tag owns that a tag points
    /// into (blocks the scenario editor grew). A tag the schema knows names
    /// only what its arrays hold, so a stale word no array covers keeps
    /// nothing (an emptied block's old elements, a removed placement's). A gap nothing
    /// points into keeps nothing, and what nothing reaches is dropped.
    #[test]
    fn a_pinned_set_keeps_what_its_fields_and_addresses_reach() {
        let roots = [id(2), id(6)];
        let mut set = set();
        let p = prune_pinned(&mut set, N, &NOTHING, &roots);
        let kept: Vec<bool> = set.slots.iter().map(|s| s.tag.is_some()).collect();
        assert_eq!(
            kept,
            [
                true, true, true, true, true, false, true, true, false, false
            ]
        );
        assert_eq!(p.tags(), 3);
        assert_eq!(p.freed, 0xc0);
        assert_eq!(p.kept_by_pointer, 1);
        // Each kept tag's first edge, to follow back to a root.
        assert_eq!(p.why.get(&id(0)), Some(&Reach::Root("scenario")));
        assert_eq!(p.why.get(&id(2)), Some(&Reach::Root("caller's root")));
        assert_eq!(
            p.why.get(&id(3)),
            Some(&Reach::Address {
                by: id(2),
                word: BASE + 0x904
            })
        );
        assert_eq!(
            p.why.get(&id(4)),
            Some(&Reach::Gap {
                by: id(2),
                gap: BASE + 0xa00
            })
        );
        assert_eq!(p.why.get(&id(7)), Some(&Reach::Datum { by: id(6) }));
        assert_eq!(p.why.get(&id(5)), None);
        // A conversion's tag (from `first` on) is never dropped.
        let mut set = super::pinned_prune_tests::set();
        prune_pinned(&mut set, N - 1, &NOTHING, &roots);
        assert!(set.slots[N - 1].tag.is_some());
        assert!(set.slots[8].tag.is_none());
    }

    /// Only a field the schema says is a block gives an address: a built
    /// tag's words (a rebuilt sound gestalt's raw offsets) and a walked
    /// tag's data words that fall inside another tag's bytes keep nothing.
    /// A block that doesn't read (here outside the metadata) is skipped,
    /// and the tag's own words then name tags as datums only.
    #[test]
    fn data_words_inside_another_tag_keep_nothing() {
        let mut set = set();
        // A built gestalt whose data words land in tag 3's bytes and in
        // the gap holding tag 4's datum.
        let mut gestalt = map::build::Block::zeroed(0x10);
        put(&mut gestalt.bytes, 0x0, BASE + 0x904);
        put(&mut gestalt.bytes, 0x4, BASE + 0xa00);
        set.slots.push(Slot {
            name: Some("gestalt".into()),
            tag: Some(TagEntry {
                id: id(N),
                group: FourCC(*b"ugh!"),
                payload: TagPayload::Built(gestalt),
            }),
        });
        // The walked `MGS2`: a data word in its element pointing into tag
        // 3, and its element's block (`+0x10`) pointing outside the
        // metadata.
        let b = bytes(&mut set, 6);
        put(b, 0x10 + 0x30, BASE + 0x904);
        put(b, 0x10 + 0x10, 5);
        put(b, 0x10 + 0x14, BASE + 0x2000);
        let p = prune_pinned(&mut set, N + 1, &NOTHING, &[id(6), id(N)]);
        let kept: Vec<bool> = set.slots.iter().map(|s| s.tag.is_some()).collect();
        assert_eq!(
            kept,
            [
                true, true, false, false, false, false, true, true, true, false, true
            ]
        );
        assert_eq!(p.kept_by_pointer, 0);
        // The skipped block: `MGS2`'s own words name tags (the stale
        // datum of tag 8), none of them an address.
        assert_eq!(p.partly_walked, [id(6)]);
        assert_eq!(p.why.get(&id(8)), Some(&Reach::Datum { by: id(6) }));
    }

    /// Retail shares block data between tags: a walked tag's block can sit
    /// in another tag's bytes. The block keeps the datums in it (its count
    /// times its element size, and each block below it the same way), and
    /// the tag it falls in stays whole as their holder, since the schema
    /// moves a block outside its tag only into another tag; but the holder
    /// is not reached: a word of it outside the block naming a tag keeps
    /// nothing and is nulled. Before, the holder's bytes went to metadata
    /// no tag owns and the fresh build refused every block in them.
    #[test]
    fn a_block_in_another_tag_keeps_that_tag_as_its_holder_only() {
        let mut set = set();
        // Tag 8 as a group the schema walks from a root of `0x10` (its
        // block empty).
        set.slots[8].tag.as_mut().unwrap().group = FourCC(*b"MGS2");
        // `MGS2`'s element: its `+0x38` block, one `0x1c` element at
        // `0xb10` in tag 8's bytes, whose data (`+0x0`) is 4 bytes at
        // `0xb30`, in tag 8 too.
        let b = bytes(&mut set, 6);
        put(b, 0x10 + 0x38, 1);
        put(b, 0x10 + 0x3c, BASE + 0xb10);
        // In tag 8: the element names tag 9; past both arrays a word names
        // tag 5.
        let b = bytes(&mut set, 8);
        put(b, 0x10, 4);
        put(b, 0x14, BASE + 0xb30);
        put(b, 0x10 + 0x10, id(9).0);
        put(b, 0x30, 0x1234_5678);
        put(b, 0x3c, id(5).0);
        let before = bytes(&mut set, 8).clone();
        let p = prune_pinned(&mut set, N, &NOTHING, &[id(6)]);
        assert!(
            set.slots[8].tag.is_some(),
            "the tag holding the block stays"
        );
        assert_eq!(
            p.why.get(&id(8)),
            Some(&Reach::Holds {
                by: id(6),
                block: BASE + 0xb10
            })
        );
        assert_eq!(p.holders, [id(8)]);
        assert_eq!(p.why.get(&id(9)), Some(&Reach::Datum { by: id(6) }));
        assert!(set.slots[9].tag.is_some(), "a datum in the block keeps");
        assert!(
            set.slots[5].tag.is_none(),
            "a datum past the block keeps nothing"
        );
        assert_eq!(p.kept_by_pointer, 0);
        // The holder's bytes as they were, but its word naming the dropped
        // tag 5, nulled.
        let after = bytes(&mut set, 8).clone();
        assert_eq!(after[..0x3c], before[..0x3c]);
        assert_eq!(after[0x3c..0x40], u32::MAX.to_le_bytes());
        assert_eq!(p.nulled_words, 1);
        assert_eq!(
            set.pinned.as_ref().unwrap().unowned.len(),
            2,
            "no bytes moved"
        );
        // Without the block, tags 8 and 9 go: here both stay.
        let mut without = super::pinned_prune_tests::set();
        let all = prune_pinned(&mut without, N, &NOTHING, &[id(6)]);
        assert_eq!(all.freed - p.freed, 0x40 + 0x40);
    }

    /// A size-0 tag (an alias) reads its root in another tag's bytes: a
    /// reached alias keeps that tag as its holder, or the pinned map's
    /// alias pointed at freed bytes and nothing could read it (the fresh
    /// test's `canonical` visit). It owns no byte itself, so an address
    /// at its start still finds the tag it sits in.
    #[test]
    fn an_alias_keeps_the_tag_its_root_sits_in() {
        let mut set = set();
        set.slots[8].tag.as_mut().unwrap().group = FourCC(*b"MGS2");
        // Tag 8's root names tag 9 (a word the alias's root reads too);
        // past it a word names tag 5.
        let b = bytes(&mut set, 8);
        put(b, 0x4, id(9).0);
        put(b, 0x3c, id(5).0);
        set.slots.push(Slot {
            name: Some("alias".into()),
            tag: Some(TagEntry {
                id: id(N),
                group: FourCC(*b"MGS2"),
                payload: TagPayload::Pinned {
                    address: BASE + 0xb00,
                    bytes: Vec::new(),
                },
            }),
        });
        let p = prune_pinned(&mut set, N + 1, &NOTHING, &[id(N)]);
        assert!(set.slots[8].tag.is_some(), "the alias's root stays");
        assert_eq!(
            p.why.get(&id(8)),
            Some(&Reach::Holds {
                by: id(N),
                block: BASE + 0xb00
            })
        );
        assert_eq!(p.why.get(&id(9)), Some(&Reach::Datum { by: id(N) }));
        assert!(set.slots[5].tag.is_none());
        assert_eq!(bytes(&mut set, 8)[0x3c..0x40], u32::MAX.to_le_bytes());
    }

    /// A stale word of a kept tag (one no array of it covers: a removed
    /// placement's, an emptied block's old elements) keeps nothing, and is
    /// nulled when the tag it names is dropped, so the map holds no word
    /// naming a tag it lacks (V: the pinned Blood Gulch scenario still
    /// named the base's dropped powerups, vehicles, ambience, reverbs and
    /// decorators).
    #[test]
    fn a_stale_word_naming_a_dropped_tag_is_nulled() {
        let mut set = set();
        // The scenario's stale word too: past every block (none here).
        put(bytes(&mut set, 0), 0x3f0, id(9).0);
        let p = prune_pinned(&mut set, N, &NOTHING, &[id(6)]);
        assert!(set.slots[8].tag.is_none() && set.slots[9].tag.is_none());
        // `MGS2`'s stale datum of tag 8, after its element.
        assert_eq!(bytes(&mut set, 6)[0xa8..0xac], u32::MAX.to_le_bytes());
        assert_eq!(bytes(&mut set, 0)[0x3f0..0x3f4], u32::MAX.to_le_bytes());
        assert_eq!(p.nulled_words, 2);
        // A word naming a kept tag stays.
        assert_eq!(bytes(&mut set, 6)[0x18..0x1c], id(7).0.to_le_bytes());
    }

    /// A pinned scenario's object palette keeps the entries a placement
    /// names; a base entry none names is nulled and its tag dropped (the
    /// base's vehicles the conversion's placements replaced).
    #[test]
    fn an_object_palette_keeps_what_is_placed() {
        let mut set = TagSet::new(CacheType::Multiplayer, "m", "s");
        let tags: [(&[u8; 4], &str, u32, usize); 4] = [
            (b"scnr", "s", 0, 0x400),
            (b"matg", "globals", 0x400, 0x40),
            (b"vehi", "not placed", 0x500, 0x40),
            (b"vehi", "placed", 0x600, 0x40),
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
        // Vehicles (`OBJECT_TYPES[1]`): one placement at `+0x200` of
        // palette entry 1; the palette's two entries at `+0x260`.
        let (placements, palette, _) = tags::object::OBJECT_TYPES[1].placement.unwrap();
        let b = bytes(&mut set, 0);
        put(b, placements as usize, 1);
        put(b, placements as usize + 4, BASE + 0x200);
        put(b, 0x200, 1);
        put(b, palette as usize, 2);
        put(b, palette as usize + 4, BASE + 0x260);
        for (k, slot) in [2, 3].into_iter().enumerate() {
            let e = 0x260 + k * PALETTE_ENTRY as usize;
            put(b, e, u32::from_be_bytes(*b"vehi"));
            put(b, e + 4, id(slot).0);
        }
        let p = prune_pinned(&mut set, 4, &NOTHING, &[]);
        assert_eq!(p.palette_entries, 1);
        assert!(set.slots[2].tag.is_none(), "the unplaced vehicle goes");
        assert!(set.slots[3].tag.is_some(), "the placed vehicle stays");
        let b = bytes(&mut set, 0);
        assert_eq!(b[0x260..0x268], [0xff; 8]);
        assert_eq!(b[0x28c..0x290], id(3).0.to_le_bytes());
    }
}
