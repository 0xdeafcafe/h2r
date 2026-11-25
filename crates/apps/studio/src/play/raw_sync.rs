//! Geometry and pixels live: the Modeling tool's section patches and the
//! Textures tool's painted pixels sent to a paired game as `edit.batch`
//! raw entries (docs/editor/realtime-edits.md, "The editor").
//!
//! The model is [`super::tag_sync`]'s, over raw blocks:
//!
//! * [`RawBaseline`] is what the game is taken to hold beyond the loaded
//!   file: every raw byte (by owner and offset) and tag byte (by address)
//!   it acknowledged, each beside the file's byte under it, and each
//!   grown section's appended block and the raw pointer the game gave it.
//! * A **want** is what the documents say the game should hold now:
//!   `GeometrySession::changes` (each patch with its owner and the file's
//!   bytes, `geometry_edit::RawWrite::was`) and the painted layers of the
//!   Textures tool encoded to their images' stored format.
//! * A **plan** is the difference. A byte the baseline holds and the
//!   documents no longer change wants its file byte back, so undo sends
//!   the old bytes. Every entry carries `was`, the bytes the baseline says
//!   the game holds; the game refuses the batch whole when it holds
//!   others (`MISMATCH`), and the edit stays pending until the documents
//!   change or the user sends again.
//!
//! A grown section goes as `edit.raw.append` (the new block), then one
//! batch with the pointer word the append returned, the record writes and
//! bounds, and `evict` naming the section, in that order. Moved vertices
//! go as one batch: the stream bytes as `raw` and the bounds as `writes`.
//! Pixels go as `raw` entries per LOD chain; during a stroke only mip 0's
//! bytes go, and the rebuilt mips at the stroke's end.
//!
//! Nothing is sent in View mode: the plan only fills Pending.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use agent_proto::hot::encode_base64;
use bitmap_edit::Layer;
use geometry_edit::{Target, TargetKind};
use map::TagId;

use super::tag_sync::{self, Effect};
use super::*;

/// Raw bytes in one `edit.batch`: well under the game's 1 MiB request
/// line (`MAX_LINE`) once base64 doubles them with `was`. A larger plan
/// goes as several batches, the tag writes and evictions with the last.
pub(crate) const BATCH_RAW_BYTES: usize = 256 << 10;
/// A run of `was` bytes longer than this is sent as its FNV-1a 64 hash.
const WAS_INLINE: usize = 64;
/// Ids of raw requests: apart from the tag batches' (`Purpose::Edit`).
const ID_BASE: u64 = 1 << 48;

/// What a grown section's Pending row says (docs/editor/realtime-edits.md,
/// "The editor": `now`, with a note).
pub(crate) const GROWN_NOTE: &str = "a grown section: the level rebuilds for a moment";

/// Which section block an owner indexes (`edit.batch` `kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SectionKind {
    Cluster,
    InstanceDef,
    Mode,
    /// A collision BSP: patched and grown in the tag blocks, never raw.
    Coll,
}

/// A raw entry's owner: the record whose raw pointer locates the bytes
/// (docs/runtime/agent-api.md, "Raw edits").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Owner {
    Section { tag: u32, kind: SectionKind, index: u32 },
    Pixels { tag: u32, image: u32, lod: u32 },
}

impl Owner {
    pub fn section(t: Target) -> Self {
        let kind = match t.kind {
            TargetKind::Cluster => SectionKind::Cluster,
            TargetKind::InstanceDef => SectionKind::InstanceDef,
            TargetKind::ModeSection => SectionKind::Mode,
            TargetKind::Coll => SectionKind::Coll,
        };
        Owner::Section {
            tag: t.tag.0,
            kind,
            index: t.index,
        }
    }

    /// The Modeling target of a section owner.
    pub fn target(self) -> Option<Target> {
        let Owner::Section { tag, kind, index } = self else {
            return None;
        };
        let kind = match kind {
            SectionKind::Cluster => TargetKind::Cluster,
            SectionKind::InstanceDef => TargetKind::InstanceDef,
            SectionKind::Mode => TargetKind::ModeSection,
            SectionKind::Coll => TargetKind::Coll,
        };
        Some(Target::new(kind, TagId(tag), index))
    }

    pub fn tag(self) -> TagId {
        match self {
            Owner::Section { tag, .. } | Owner::Pixels { tag, .. } => TagId(tag),
        }
    }

    /// The wire form: `{section: {tag, kind, index}}` or
    /// `{pixels: {tag, image, lod}}`.
    pub fn json(self) -> Value {
        match self {
            Owner::Section { tag, kind, index } => {
                let kind = match kind {
                    SectionKind::Cluster => "cluster",
                    SectionKind::InstanceDef => "instance_def",
                    SectionKind::Mode => "mode",
                    SectionKind::Coll => "coll",
                };
                json!({"section": {"tag": format!("{tag:#010x}"), "kind": kind, "index": index}})
            }
            Owner::Pixels { tag, image, lod } => {
                json!({"pixels": {"tag": format!("{tag:#010x}"), "image": image, "lod": lod}})
            }
        }
    }

    fn is_section(self) -> bool {
        matches!(self, Owner::Section { .. })
    }
}

/// FNV-1a 64, the `{hash}` form of `was` (`hot_tags::raw::fnv1a64`).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A grown section the game holds: the block appended for it, and the
/// raw pointer the game gave that block (`None` in a play copy, which
/// holds the block at an offset the editor does not know).
#[derive(Clone, Debug, PartialEq)]
struct Held {
    bytes: Arc<Vec<u8>>,
    pointer: Option<u32>,
    /// The block info's pointer word and the file's word there: an undo
    /// writes it back whole.
    pointer_at: u32,
    file_word: [u8; 4],
}

/// What the game holds beyond its loaded file: `(held, file)` per byte.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawBaseline {
    raw: BTreeMap<(Owner, u32), (u8, u8)>,
    tags: BTreeMap<u32, (u8, u8)>,
    grown: BTreeMap<Owner, Held>,
}

impl RawBaseline {
    /// What a play copy saved now holds: the geometry the save wrote.
    /// Pixel edits are not saved (the Textures tool writes no tag yet).
    pub fn of_copy(app: &mut App) -> Self {
        let mut want = Want::default();
        geometry_want(app, &mut want, false);
        let mut base = RawBaseline::default();
        if want.frozen.geometry {
            return base;
        }
        for (k, (now, file)) in want.raw {
            base.raw.insert(k, (now, file));
        }
        for (at, (now, file)) in want.tags {
            base.tags.insert(at, (now, file));
        }
        for (owner, g) in want.grown {
            base.grown.insert(
                owner,
                Held {
                    bytes: g.bytes,
                    pointer: None,
                    pointer_at: g.pointer_at,
                    file_word: g.file_word,
                },
            );
        }
        base
    }

    /// Whether the game is taken to hold anything beyond its file.
    #[allow(dead_code)] // R5 (raw sync) is in progress; its caller lands next.
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty() && self.tags.is_empty() && self.grown.is_empty()
    }

    fn apply(&mut self, ack: Ack) {
        for (owner, at, now, file) in ack.raw {
            for (i, (n, f)) in now.into_iter().zip(file).enumerate() {
                self.raw.insert((owner, at + i as u32), (n, f));
            }
        }
        for (at, now, file) in ack.tags {
            for (i, (n, f)) in now.into_iter().zip(file).enumerate() {
                self.tags.insert(at + i as u32, (n, f));
            }
        }
        for (owner, held) in ack.grown {
            match held {
                Some(h) => self.grown.insert(owner, h),
                None => self.grown.remove(&owner),
            };
        }
        // Bytes back at their file value hold nothing beyond the file.
        self.raw.retain(|_, (n, f)| n != f);
        self.tags.retain(|_, (n, f)| n != f);
    }
}

/// A grown section's wanted block.
#[derive(Clone, Debug)]
struct Grown {
    bytes: Arc<Vec<u8>>,
    align: u32,
    /// The block info's raw pointer word, and the file's word there.
    pointer_at: u32,
    file_word: [u8; 4],
}

/// Where nothing is sent now (a Reload row says why).
#[derive(Debug, Default)]
struct Frozen {
    /// Every section and tag byte of the geometry.
    geometry: bool,
    owners: BTreeSet<Owner>,
    ranges: Vec<(Owner, Range<u32>)>,
}

impl Frozen {
    fn raw(&self, owner: Owner, offset: u32) -> bool {
        (self.geometry && owner.is_section())
            || self.owners.contains(&owner)
            || self
                .ranges
                .iter()
                .any(|(o, r)| *o == owner && r.contains(&offset))
    }
}

/// What the documents say the game should hold: `(now, file)` per byte.
#[derive(Debug, Default)]
struct Want {
    raw: BTreeMap<(Owner, u32), (u8, u8)>,
    tags: BTreeMap<u32, (u8, u8)>,
    grown: BTreeMap<Owner, Grown>,
    /// The first addresses of grown sections' pointer words: a changed
    /// pointer is written whole, so the game never reads half of it.
    words: BTreeSet<u32>,
    frozen: Frozen,
    /// Reload rows for what cannot go live.
    rows: Vec<Value>,
}

/// One contiguous run of a plan.
#[derive(Clone, Debug, PartialEq)]
struct Run {
    at: u32,
    now: Vec<u8>,
    was: Vec<u8>,
    file: Vec<u8>,
}

/// What a batch changes in the baseline once the game acknowledges it.
#[derive(Clone, Debug, Default)]
struct Ack {
    raw: Vec<(Owner, u32, Vec<u8>, Vec<u8>)>,
    tags: Vec<(u32, Vec<u8>, Vec<u8>)>,
    grown: Vec<(Owner, Option<Held>)>,
}

/// The difference between the baseline and the documents.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// A grown section whose block must be appended first.
    append: Option<(Owner, Arc<Vec<u8>>, u32)>,
    writes: Vec<Run>,
    raw: Vec<(Owner, Run)>,
    evict: Vec<Owner>,
    grown: Vec<(Owner, Option<Held>)>,
    /// Owners whose new block waits for its append.
    appending: BTreeSet<Owner>,
}

impl Plan {
    pub(crate) fn is_empty(&self) -> bool {
        self.append.is_none()
            && self.writes.is_empty()
            && self.raw.is_empty()
            && self.evict.is_empty()
            && self.appending.is_empty()
    }
}

/// A raw request in flight.
#[derive(Debug)]
enum Sent {
    Append {
        id: u64,
        owner: Owner,
        bytes: Arc<Vec<u8>>,
    },
    Batch {
        id: u64,
        ack: Ack,
    },
}

/// Why nothing goes until the documents change or the user sends again.
#[derive(Clone, Debug)]
struct Hold {
    rev: u64,
    why: String,
    /// The game holds other bytes (`MISMATCH`), rather than refused.
    conflict: bool,
}

/// The decoded source of one bitmap's images and LOD chains.
#[derive(Default)]
struct PixelCache {
    generation: Option<u64>,
    images: HashMap<u32, Result<Arc<Vec<tags::Image>>, String>>,
    chains: HashMap<(u32, u32, u32), Result<Arc<Chain>, String>>,
}

/// One LOD chain as the file holds it.
struct Chain {
    layout: gfx::texture::TextureLayout,
    file: Vec<u8>,
}

/// The raw half of a pairing: baseline, the request in flight and why
/// sending waits.
#[derive(Default)]
pub(crate) struct RawSync {
    pub(super) base: RawBaseline,
    sent: Option<Sent>,
    next: u64,
    /// Blocks this map's appends made, by owner, with the pointer the game
    /// gave each: reused while the same block is wanted.
    appended: BTreeMap<Owner, (Arc<Vec<u8>>, u32)>,
    hold: Option<Hold>,
    pixels: PixelCache,
}

impl RawSync {
    /// A raw request is in flight.
    pub(super) fn busy(&self) -> bool {
        self.sent.is_some()
    }

    /// Whether answer `id` is for a raw request.
    pub(super) fn owns(&self, id: u64) -> bool {
        match &self.sent {
            Some(Sent::Append { id: sent, .. } | Sent::Batch { id: sent, .. }) => *sent == id,
            None => false,
        }
    }

    /// Start again from `base`: a new map, epoch or pairing.
    pub(super) fn reset(&mut self, base: RawBaseline) {
        *self = RawSync {
            base,
            next: self.next,
            ..RawSync::default()
        };
    }

    /// The connection dropped: what was in flight is unknown.
    pub(super) fn transport_lost(&mut self) {
        self.sent = None;
    }

    /// `edit.revert` put the game's raw bytes back and dropped its appends.
    pub(super) fn reverted(&mut self) {
        self.reset(RawBaseline::default());
    }

    /// The user sent again: retry what a refusal or mismatch held.
    pub(super) fn retry(&mut self) {
        self.hold = None;
    }

    fn id(&mut self) -> u64 {
        self.next += 1;
        ID_BASE + self.next
    }
}

/// A revision of everything a raw or collision want reads: the geometry
/// session, the open bitmaps, whether a stroke is active and the level
/// collision edits.
pub(crate) fn doc_rev(app: &App) -> u64 {
    let mut h = DefaultHasher::new();
    app.modeling.session.rev().hash(&mut h);
    app.textures.generation.hash(&mut h);
    app.textures.stroke.map(|t| t.0).hash(&mut h);
    let mut revs: Vec<(u32, u64)> = app
        .textures
        .open
        .iter()
        .map(|(tag, o)| (tag.0, o.bitmap.rev()))
        .collect();
    revs.sort_unstable();
    revs.hash(&mut h);
    super::collision_sync::doc_rev(app).hash(&mut h);
    h.finish()
}

fn tag_name(app: &App, tag: TagId) -> String {
    app.tags
        .tag(tag)
        .map_or_else(|| format!("{:#010x}", tag.0), |t| t.name.clone())
}

fn reload_row(doc: &str, tag: TagId, path: &str, why: &str) -> Value {
    tag_sync::row(
        doc,
        json!(format!("{:#010x}", tag.0)),
        path,
        Effect::Reload,
        why,
    )
}

/// The geometry the session's save would write, as wanted bytes. With
/// `live`, a target whose tag the game has not loaded freezes the geometry.
fn geometry_want(app: &mut App, want: &mut Want, live: bool) {
    app.sync_modeling();
    let g = &app.modeling.session;
    if !g.has_changes() {
        return;
    }
    let edited: Vec<Target> = g
        .open_targets()
        .into_iter()
        .filter(|t| g.class(*t).is_some_and(|c| c != geometry_edit::WriteClass::Unchanged))
        .collect();
    // A recompiled collision BSP grows blocks in the file; the game's copy
    // is not patched half way, so the edit waits for a reload.
    if let Some(t) = edited.iter().find(|t| t.kind == TargetKind::Coll) {
        want.frozen.geometry = true;
        let why = "a recompiled collision BSP takes effect on a reload";
        want.rows.push(reload_row("geometry", t.tag, &t.id(), why));
        return;
    }
    let p = match g.changes() {
        Ok(p) => p,
        Err(e) => {
            want.frozen.geometry = true;
            for t in &edited {
                want.rows.push(reload_row("geometry", t.tag, &t.id(), &e.to_string()));
            }
            return;
        }
    };
    if live {
        let absent: Vec<&Target> = edited
            .iter()
            .filter(|t| !app.tags.resident(t.tag))
            .collect();
        if !absent.is_empty() {
            want.frozen.geometry = true;
            for t in absent {
                let why = format!("{} is not loaded in the game", tag_name(app, t.tag));
                want.rows.push(reload_row("geometry", t.tag, &t.id(), &why));
            }
            return;
        }
    }
    for w in p.raw {
        let owner = Owner::section(w.owner);
        for (i, (n, f)) in w.bytes.iter().zip(&w.was).enumerate() {
            want.raw.insert((owner, w.offset + i as u32), (*n, *f));
        }
    }
    let tag_bytes = |w: &geometry_edit::RegionWrite, want: &mut Want| {
        for (i, (n, f)) in w.bytes.iter().zip(&w.was).enumerate() {
            want.tags.insert(w.address + i as u32, (*n, *f));
        }
    };
    for w in &p.tags {
        tag_bytes(w, want);
    }
    for a in p.appended {
        for w in &a.writes {
            tag_bytes(w, want);
        }
        want.grown.insert(
            Owner::section(a.target),
            Grown {
                bytes: Arc::new(a.bytes),
                align: a.align,
                pointer_at: a.pointer_address,
                file_word: a.old.encode().to_le_bytes(),
            },
        );
    }
}

impl PixelCache {
    fn sync(&mut self, generation: u64) {
        if self.generation != Some(generation) {
            *self = PixelCache {
                generation: Some(generation),
                ..PixelCache::default()
            };
        }
    }

    fn images(&mut self, app: &App, tag: TagId) -> Result<Arc<Vec<tags::Image>>, String> {
        self.images
            .entry(tag.0)
            .or_insert_with(|| {
                let source = app.session.source().cloned().ok_or("no map is open")?;
                let mut map = source.open().map_err(|e| e.to_string())?;
                let decoded = tags::DecodedTag::read(&mut map, tag).map_err(|e| e.to_string())?;
                match decoded.data {
                    tags::TagData::Bitmap(b) => Ok(Arc::new(b.images)),
                    _ => Err("not a bitmap".to_owned()),
                }
            })
            .clone()
    }

    /// LOD `lod` of an image as the file holds it: in the map itself or a
    /// shared cache, wherever its pointer names (the overlay serves both).
    fn chain(
        &mut self,
        app: &App,
        tag: TagId,
        image: u32,
        img: &tags::Image,
        lod: u32,
    ) -> Result<Arc<Chain>, String> {
        self.chains
            .entry((tag.0, image, lod))
            .or_insert_with(|| {
                let range = img.lods[lod as usize].ok_or("the LOD is absent")?;
                let desc = assets::texture_desc(img, lod).map_err(|e| e.to_string())?;
                let layout = desc.layout().map_err(|e| e.to_string())?;
                let mut files = app.shown_raw_files()?;
                let len = range.size.max(layout.total_size as u32);
                let file = files
                    .read(range.pointer.file, range.pointer.offset, len)
                    .map_err(|e| format!("its pixels do not read: {e}"))?;
                Ok(Arc::new(Chain { layout, file }))
            })
            .clone()
    }
}

/// The painted layers of the open bitmaps, encoded, as wanted bytes.
fn pixel_want(app: &App, cache: &mut PixelCache, want: &mut Want) {
    cache.sync(app.textures.generation);
    let stroking = app.textures.stroke;
    let mut tags: Vec<&TagId> = app.textures.open.keys().collect();
    tags.sort_unstable_by_key(|t| t.0);
    for &tag in tags {
        let open = &app.textures.open[&tag];
        let mut by_image: BTreeMap<u32, Vec<&Layer>> = BTreeMap::new();
        for l in open.bitmap.layers().iter().filter(|l| l.is_edited()) {
            by_image.entry(l.id.image).or_default().push(l);
        }
        for (image, layers) in by_image {
            let path = format!("{} · image {image}", open.name);
            let pixels = |lod| Owner::Pixels {
                tag: tag.0,
                image,
                lod,
            };
            let refuse = |want: &mut Want, why: String| {
                want.frozen.owners.extend((0..3).map(pixels));
                want.rows.push(reload_row("bitmaps", tag, &path, &why));
            };
            if !app.tags.resident(tag) {
                refuse(want, format!("{} is not loaded in the game", open.name));
                continue;
            }
            let img = match cache.images(app, tag) {
                Ok(images) => match images.get(image as usize) {
                    Some(img) => img.clone(),
                    None => {
                        refuse(want, "the image is not in the bitmap".into());
                        continue;
                    }
                },
                Err(e) => {
                    refuse(want, e);
                    continue;
                }
            };
            if let Err(why) =
                image_want(app, cache, want, tag, image, &img, &layers, stroking == Some(tag))
            {
                refuse(want, why);
            }
        }
    }
}

/// One image's painted tiles in every LOD chain and mip it stores, as
/// `assets::encode::encode_tiles` writes them (docs/editor/modeling.md,
/// "Texture editing"): the bytes a save patches in place. During a stroke
/// the levels below mip 0 wait for its end.
#[allow(clippy::too_many_arguments)]
fn image_want(
    app: &App,
    cache: &mut PixelCache,
    want: &mut Want,
    tag: TagId,
    image: u32,
    img: &tags::Image,
    layers: &[&Layer],
    stroking: bool,
) -> Result<(), String> {
    use assets::encode::{Dirty, PixelRect, Planes};
    let open = &app.textures.open[&tag];
    let desc0 = assets::texture_desc(img, 0).map_err(|e| e.to_string())?;
    let depth = if desc0.kind == gfx::texture::TextureKind::Volume {
        desc0.depth
    } else {
        1
    };
    let mut data = Vec::new();
    for face in 0..desc0.faces() {
        for slice in 0..depth {
            let layer = open
                .bitmap
                .layer(bitmap_edit::LayerId::new(image, face, slice))
                .ok_or("a layer of the image did not decode")?;
            data.push(layer.canvas.to_rgba());
        }
    }
    let refs: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
    let planes = Planes {
        width: desc0.width,
        height: desc0.height,
        depth,
        planes: &refs,
    };
    let dirty: Vec<Dirty> = layers
        .iter()
        .map(|l| {
            let r = l.canvas.changed_rect(&l.source);
            Dirty {
                face: l.id.face,
                slice: l.id.slice,
                rect: PixelRect::new(r.x, r.y, r.w, r.h),
            }
        })
        .filter(|d| !d.rect.is_empty())
        .collect();
    let spans = assets::encode::encode_tiles(img, &planes, &dirty).map_err(|e| e.to_string())?;
    // During a stroke, every level below mip 0 keeps what the game holds.
    let mut held_back: Vec<(Owner, Range<u32>)> = Vec::new();
    for lod in 0..assets::bitmap::LOD_COUNT {
        if !stroking || img.lods[lod as usize].is_none() {
            continue;
        }
        let chain = cache.chain(app, tag, image, img, lod)?;
        let owner = Owner::Pixels {
            tag: tag.0,
            image,
            lod,
        };
        for level in &chain.layout.levels {
            if lod + level.level == 0 {
                continue;
            }
            for f in 0..chain.layout.faces {
                let at = (f as usize * chain.layout.face_stride + level.offset) as u32;
                held_back.push((owner, at..at + level.size as u32));
            }
        }
    }
    let mut wanted = Vec::new();
    for span in spans {
        let chain = cache.chain(app, tag, image, img, span.lod)?;
        let owner = Owner::Pixels {
            tag: tag.0,
            image,
            lod: span.lod,
        };
        for (i, &n) in span.bytes.iter().enumerate() {
            let at = span.offset + i;
            let f = *chain
                .file
                .get(at)
                .ok_or("the encoded pixels run past the LOD")?;
            let at = at as u32;
            if n != f && !held_back.iter().any(|(o, r)| *o == owner && r.contains(&at)) {
                wanted.push(((owner, at), (n, f)));
            }
        }
    }
    want.frozen.ranges.extend(held_back);
    want.raw.extend(wanted);
    Ok(())
}

/// Contiguous runs of `(key, (now, was, file))`, by `group` and address.
fn runs<K: Copy + Ord, G: Copy + PartialEq>(
    diffs: impl IntoIterator<Item = (K, (u8, u8, u8))>,
    split: impl Fn(K) -> (G, u32),
) -> Vec<(G, Run)> {
    let mut out: Vec<(G, Run)> = Vec::new();
    for (k, (now, was, file)) in diffs {
        let (g, at) = split(k);
        if let Some((lg, run)) = out.last_mut()
            && *lg == g
            && run.at.checked_add(run.now.len() as u32) == Some(at)
        {
            run.now.push(now);
            run.was.push(was);
            run.file.push(file);
            continue;
        }
        out.push((
            g,
            Run {
                at,
                now: vec![now],
                was: vec![was],
                file: vec![file],
            },
        ));
    }
    out
}

/// The grown sections' part of a plan. A changed block needs its append
/// first; one the game holds from a play copy (no pointer known) cannot
/// change live.
fn grown_plan(
    base: &RawBaseline,
    appended: &BTreeMap<Owner, (Arc<Vec<u8>>, u32)>,
    want: &mut Want,
    plan: &mut Plan,
) {
    if want.frozen.geometry {
        return;
    }
    let owners: BTreeSet<Owner> = want.grown.keys().chain(base.grown.keys()).copied().collect();
    let mut copy_held = Vec::new();
    for owner in owners {
        let held = base.grown.get(&owner);
        let Some(g) = want.grown.get(&owner).cloned() else {
            // Undone: the pointer word wants its file value back (the
            // baseline holds the appended block's), and the section is
            // evicted so the guest pages the old block in.
            if held.is_some_and(|h| h.pointer.is_none()) {
                copy_held.push(owner);
            } else {
                if let Some(h) = held {
                    // The word wants the file's value; the baseline keeps
                    // only the bytes that differ from it.
                    want.words.insert(h.pointer_at);
                    for (i, f) in h.file_word.into_iter().enumerate() {
                        want.tags.entry(h.pointer_at + i as u32).or_insert((f, f));
                    }
                }
                plan.evict.push(owner);
                plan.grown.push((owner, None));
            }
            continue;
        };
        let pointer = |p: u32, want: &mut Want| {
            want.words.insert(g.pointer_at);
            for (i, (n, f)) in p.to_le_bytes().into_iter().zip(g.file_word).enumerate() {
                want.tags.insert(g.pointer_at + i as u32, (n, f));
            }
        };
        match held {
            Some(h) if *h.bytes == *g.bytes => {
                if let Some(p) = h.pointer {
                    pointer(p, want);
                }
            }
            Some(h) if h.pointer.is_none() => copy_held.push(owner),
            _ => match appended.get(&owner).filter(|(b, _)| **b == *g.bytes) {
                Some(&(_, p)) => {
                    pointer(p, want);
                    plan.evict.push(owner);
                    plan.grown.push((
                        owner,
                        Some(Held {
                            bytes: g.bytes.clone(),
                            pointer: Some(p),
                            pointer_at: g.pointer_at,
                            file_word: g.file_word,
                        }),
                    ));
                }
                None => {
                    plan.appending.insert(owner);
                    if plan.append.is_none() {
                        plan.append = Some((owner, g.bytes.clone(), g.align));
                    }
                }
            },
        }
    }
    if !copy_held.is_empty() {
        want.frozen.geometry = true;
        plan.evict.clear();
        plan.grown.clear();
        plan.appending.clear();
        plan.append = None;
        for owner in copy_held {
            let path = owner.target().map_or_else(String::new, |t| t.id());
            want.rows.push(reload_row(
                "geometry",
                owner.tag(),
                &path,
                "the game's play copy holds this grown section: reload to send further edits",
            ));
        }
    }
}

/// The plan from the baseline to `want`.
fn plan(
    base: &RawBaseline,
    appended: &BTreeMap<Owner, (Arc<Vec<u8>>, u32)>,
    want: &mut Want,
) -> Plan {
    let mut plan = Plan::default();
    grown_plan(base, appended, want, &mut plan);
    let frozen = &want.frozen;
    let keys: BTreeSet<(Owner, u32)> = want
        .raw
        .keys()
        .chain(base.raw.keys())
        .copied()
        .filter(|(o, at)| !frozen.raw(*o, *at))
        .collect();
    let raw_diffs = keys.into_iter().filter_map(|k| {
        let held = base.raw.get(&k);
        let file = want.raw.get(&k).map(|w| w.1).or(held.map(|h| h.1))?;
        let now = want.raw.get(&k).map_or(file, |w| w.0);
        let was = held.map_or(file, |h| h.0);
        (now != was).then_some((k, (now, was, file)))
    });
    plan.raw = runs(raw_diffs, |(o, at)| (o, at));
    if !frozen.geometry {
        let addresses: BTreeSet<u32> = want.tags.keys().chain(base.tags.keys()).copied().collect();
        let byte = |at: u32| {
            let held = base.tags.get(&at);
            let file = want.tags.get(&at).map(|w| w.1).or(held.map(|h| h.1))?;
            let now = want.tags.get(&at).map_or(file, |w| w.0);
            let was = held.map_or(file, |h| h.0);
            Some((now, was, file))
        };
        let changed = |at: u32| byte(at).is_some_and(|(now, was, _)| now != was);
        // A pointer word with any byte changed goes whole.
        let whole: BTreeSet<u32> = want
            .words
            .iter()
            .filter(|&&w| (w..w + 4).any(changed))
            .flat_map(|&w| w..w + 4)
            .collect();
        let tag_diffs = addresses.into_iter().filter_map(|at| {
            let b = byte(at)?;
            (b.0 != b.1 || whole.contains(&at)).then_some((at, b))
        });
        plan.writes = runs(tag_diffs, |at| ((), at))
            .into_iter()
            .map(|((), r)| r)
            .collect();
    }
    plan
}

/// The Pending rows of a plan: one per section, one per image, and one
/// for geometry tag bytes that no section's row covers.
fn plan_rows(app: &App, plan: &Plan, hold: Option<&Hold>) -> Vec<Value> {
    let mut sections: BTreeSet<Owner> = BTreeSet::new();
    let mut images: BTreeSet<(u32, u32)> = BTreeSet::new();
    for (o, _) in &plan.raw {
        match *o {
            Owner::Section { .. } => {
                sections.insert(*o);
            }
            Owner::Pixels { tag, image, .. } => {
                images.insert((tag, image));
            }
        }
    }
    sections.extend(plan.evict.iter().copied());
    sections.extend(plan.appending.iter().copied());
    let mut rows = Vec::new();
    for o in &sections {
        let Some(t) = o.target() else { continue };
        let grown = plan.evict.contains(o) || plan.appending.contains(o);
        let why = if grown {
            GROWN_NOTE
        } else {
            "moved vertices show on the next frame"
        };
        let mut row = tag_sync::row(
            "geometry",
            json!(format!("{:#010x}", t.tag.0)),
            &t.id(),
            Effect::Now,
            why,
        );
        row["label"] = json!(format!("{} · {} {}", tag_name(app, t.tag), t.kind.label(), t.index));
        row["target"] = json!({"target": t.id()});
        row["grown"] = json!(grown);
        rows.push(row);
    }
    if sections.is_empty() && !plan.writes.is_empty() {
        rows.push(tag_sync::row(
            "geometry",
            Value::Null,
            "geometry bounds",
            Effect::Now,
            "bounds show on the next frame",
        ));
    }
    for (tag, image) in images {
        let name = tag_name(app, TagId(tag));
        let mut row = tag_sync::row(
            "bitmaps",
            json!(format!("{tag:#010x}")),
            &format!("{name} · image {image}"),
            Effect::Now,
            "painted pixels show on the next frame",
        );
        row["target"] = json!({"bitmap": format!("{tag:#010x}"), "image": image});
        rows.push(row);
    }
    if let Some(h) = hold {
        for row in &mut rows {
            row["why"] = json!(h.why);
            if h.conflict {
                row["conflict"] = json!(true);
                row["resolvable"] = json!(false);
            } else {
                tag_sync::set_effect(row, Effect::Reload);
            }
        }
    }
    rows
}

/// A `was` as sent: the bytes, or their hash when long.
fn was_json(was: &[u8]) -> Value {
    if was.len() > WAS_INLINE {
        json!({"hash": format!("{:016x}", fnv1a64(was))})
    } else {
        json!(encode_base64(was))
    }
}

/// The next batch of `plan` and what it acknowledges: raw runs up to
/// [`BATCH_RAW_BYTES`], and with the last of them the tag writes, the
/// evictions and the grown sections' new state.
fn next_batch(plan: Plan) -> (Value, Ack) {
    let mut ack = Ack::default();
    let mut raw = Vec::new();
    let mut budget = BATCH_RAW_BYTES;
    let mut rest = false;
    for (owner, run) in plan.raw {
        if budget == 0 {
            rest = true;
            break;
        }
        let n = run.now.len().min(budget);
        if n < run.now.len() {
            rest = true;
        }
        budget -= n;
        raw.push(json!({
            "owner": owner.json(),
            "offset": run.at,
            "bytes": encode_base64(&run.now[..n]),
            "was": was_json(&run.was[..n]),
        }));
        ack.raw.push((owner, run.at, run.now[..n].to_vec(), run.file[..n].to_vec()));
    }
    let mut writes = Vec::new();
    let mut evict = Vec::new();
    if !rest {
        for run in plan.writes {
            writes.push(json!({
                "at": run.at,
                "value": sync::hex(&run.now),
                "was": sync::hex(&run.was),
            }));
            ack.tags.push((run.at, run.now, run.file));
        }
        evict = plan.evict.iter().map(|o| o.json()).collect();
        ack.grown = plan.grown;
    }
    let mut params = json!({"writes": writes, "raw": raw});
    if !evict.is_empty() {
        params["evict"] = json!(evict);
    }
    (params, ack)
}

impl Play {
    /// Pending rows for geometry and pixels, pushed onto `pending`, and
    /// the plan that would bring the game to the documents. `None` when
    /// the editor's map is not the baseline's.
    pub(super) fn raw_pending(&mut self, app: &mut App, pending: &mut Vec<Value>) -> Option<Plan> {
        if self.edits.world != Some(app.session.world_rev()) {
            return None;
        }
        app.sync_textures();
        let rev = doc_rev(app);
        let raw = &mut self.edits.raw;
        if raw.hold.as_ref().is_some_and(|h| h.rev != rev) {
            raw.hold = None;
        }
        let mut want = Want::default();
        geometry_want(app, &mut want, true);
        pixel_want(app, &mut raw.pixels, &mut want);
        let plan = plan(&raw.base, &raw.appended, &mut want);
        pending.extend(plan_rows(app, &plan, raw.hold.as_ref()));
        pending.append(&mut want.rows);
        Some(plan)
    }

    /// Send the next request of `plan`: an append, or a batch.
    pub(super) fn send_raw(&mut self, plan: Plan) {
        let raw = &mut self.edits.raw;
        if plan.is_empty() || raw.busy() || raw.hold.is_some() {
            return;
        }
        let id = raw.id();
        if let Some((owner, bytes, align)) = plan.append {
            self.call(
                agent_proto::hot::method::EDIT_RAW_APPEND,
                json!({
                    "epoch": self.edits.epoch,
                    "confirm_peers": self.edits.confirmed,
                    "bytes": encode_base64(&bytes),
                    "align": align,
                }),
                Purpose::Edit(id),
            );
            self.edits.raw.sent = Some(Sent::Append { id, owner, bytes });
            return;
        }
        if !plan.appending.is_empty() {
            return;
        }
        let (mut params, ack) = next_batch(plan);
        params["epoch"] = json!(self.edits.epoch);
        params["confirm_peers"] = json!(self.edits.confirmed);
        params["label"] = json!("editor geometry and pixels");
        self.call(agent_proto::hot::method::EDIT_BATCH, params, Purpose::Edit(id));
        self.edits.raw.sent = Some(Sent::Batch { id, ack });
    }

    /// The game's answer to a raw request.
    pub(super) fn raw_answer(&mut self, app: &mut App, result: Result<Value, agent_proto::RpcError>) {
        let Some(sent) = self.edits.raw.sent.take() else {
            return;
        };
        let rev = doc_rev(app);
        let hold = |why: String, conflict: bool| Hold { rev, why, conflict };
        match (sent, result) {
            (Sent::Append { owner, bytes, .. }, Ok(v)) => {
                match v["pointer"].as_u64().and_then(|p| u32::try_from(p).ok()) {
                    Some(p) => {
                        self.edits.raw.appended.insert(owner, (bytes, p));
                    }
                    None => {
                        let why = "edit.raw.append: the game returned no raw pointer".to_owned();
                        self.edits.raw.hold = Some(hold(why.clone(), false));
                        self.feedback.problem(app, why);
                    }
                }
            }
            (Sent::Batch { ack, .. }, Ok(_)) => {
                if self.edits.world == Some(app.session.world_rev()) {
                    self.edits.raw.base.apply(ack);
                }
            }
            (sent, Err(e)) => {
                let method = match sent {
                    Sent::Append { .. } => "edit.raw.append",
                    Sent::Batch { .. } => "edit.batch",
                };
                let conflict = e.code == agent_proto::code::MISMATCH;
                let why = if conflict {
                    "the game holds other bytes there; change the edit or send again".to_owned()
                } else {
                    e.data
                        .as_ref()
                        .and_then(|d| d["why"].as_str())
                        .map(str::to_owned)
                        .unwrap_or(e.message)
                };
                self.edits.raw.hold = Some(hold(why.clone(), conflict));
                self.feedback.problem(app, format!("{method}: {why}"));
            }
        }
        self.edits.last_diff = None;
        self.next_push = None;
        if std::mem::take(&mut self.edits.restore_after_batch)
            && let Err(why) = self.apply_edits(app, Apply::Restore)
        {
            app.error(why);
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
