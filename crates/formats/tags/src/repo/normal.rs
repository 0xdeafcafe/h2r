//! Normal forms (docs/engine/asset-store.md, "Normal forms"): a tag's
//! content with what ties it to the one map it was read from taken out,
//! and the asset hash over it.
//!
//! A tag file holds what one map had. In it, three things say as much
//! about that map as about the tag, so the same asset comes out of two
//! maps as two files:
//!
//! - a sound (`snd!`) is a 20-byte header of indices into its map's sound
//!   gestalt (`ugh!`): the playback it uses, its pitch ranges and through
//!   them its permutations and their encoded chunks, and the extra info of
//!   a dialogue sound. Its form holds those rows where the indices were;
//! - a bitmap (`bitm`) names each image's low-detail picture by where it
//!   sits in its map's low-detail region (header `+0x158`). Its form holds
//!   the picture as a raw resource, by the hash of its bytes;
//! - a raw resource of the globals or the scenario, whose stored size the
//!   extractor trusts only as a lower bound (`extract::NOISY_SIZES`), runs
//!   to the next resource, padding included. Its form is cut to the size.
//!
//! What a form keeps: every other byte, a raw resource by the hash of its
//! bytes and its length, a reference by key. Where a resource sat in a
//! shared cache is not part of a form, and neither are the three bytes at
//! `snd!` `+0xb..+0xe`, which no read of the executable explains
//! (`defs/snd_.txt`). A resource with no stored size keeps the length it
//! was extracted with. The files in the store stay as extracted, so a
//! retail map reads its own bytes.
//!
//! A form is a [`LooseTag`], so it encodes (`format::encode`) and compares
//! ([`canonical`]) like any tag. Rows moved in from the gestalt are child
//! nodes of one element, at offsets past the element's own bytes. [`hash`]
//! is over a form's lines: two maps agree on an asset exactly when its
//! content is the same.
use super::extract::NOISY_SIZES;
use super::{
    Body, Error, LooseTag, Node, Origin, RawRes, Result, TagKey, Tree, Value, canonical, sha256,
};
use crate::schema::{Field, Schema, Struct};
use map::FourCC;

/// Version of the normal forms. Raised when what a form holds changes:
/// every asset hash moves with it.
pub const VERSION: u32 = 1;

const SOUND: FourCC = FourCC(*b"snd!");
const GESTALT: FourCC = FourCC(*b"ugh!");
const BITMAP: FourCC = FourCC(*b"bitm");

// `snd!` (defs/snd_.txt).
const SOUND_HEADER: u32 = 0x14;
/// Index into the gestalt's playbacks.
const SOUND_PLAYBACK: usize = 0x6;
const SOUND_FIRST_RANGE: usize = 0x8;
const SOUND_RANGE_COUNT: usize = 0xa;
const SOUND_EXTRA_INFO: usize = 0xe;
const NO_EXTRA_INFO: u16 = 0xffff;

// `ugh!` (defs/ugh_.txt): the blocks of the 0x58-byte root, and a row of each.
const GESTALT_HEADER: u32 = 0x58;
const GESTALT_PLAYBACKS: u32 = 0x0;
const GESTALT_RANGES: u32 = 0x20;
const GESTALT_PERMUTATIONS: u32 = 0x28;
const GESTALT_CHUNKS: u32 = 0x40;
const GESTALT_EXTRA_INFOS: u32 = 0x50;
const PLAYBACK_SIZE: usize = 0x38;
const RANGE_SIZE: usize = 0xc;
const RANGE_FIRST_PERMUTATION: usize = 0x8;
const PERMUTATION_SIZE: usize = 0x10;
const PERMUTATION_FIRST_CHUNK: usize = 0xc;
const CHUNK_SIZE: usize = 0xc;
const EXTRA_INFO_SIZE: usize = 0x2c;
/// An extra info's datum of the gestalt that owns its resource: the
/// gestalt the rows come from, so a form does not name it.
const EXTRA_INFO_OWNER: u32 = 0x20;

// `bitm` (defs/bitm.txt).
const BITMAP_HEADER: u32 = 0x4c;
const BITMAP_IMAGES: u32 = 0x44;
const IMAGE_SIZE: usize = 0x74;
/// An image's low-detail picture: its offset into the region, then its size.
const IMAGE_REDUCED: usize = 0x60;

/// What a map gives the tags that index it, read once for a map.
pub struct Tables<'a> {
    /// The map's sound gestalt (`ugh!`). `None` leaves sounds as extracted.
    pub gestalt: Option<&'a LooseTag>,
    /// The map's low-detail bitmap region. Empty leaves bitmaps as
    /// extracted.
    pub low_detail: &'a [u8],
    /// Raw bytes of the store by hash, to cut a resource to its size.
    /// `None` leaves resources as extracted.
    pub raw: Option<&'a mut dyn FnMut(&[u8; 32]) -> Result<Vec<u8>>>,
    /// The layouts, for a resource's stored size. `None` cuts nothing.
    pub schema: Option<&'a Schema>,
}

impl<'a> Tables<'a> {
    /// No tables: the form is then the tag with its resources as content.
    pub fn none() -> Tables<'a> {
        Tables {
            gestalt: None,
            low_detail: &[],
            raw: None,
            schema: None,
        }
    }
}

/// The normal form of `tag` as read for the map whose tables are `tables`.
/// A sound whose index names no row of its gestalt is an error naming the
/// tag; a sound with no gestalt, and a bitmap with no low-detail region,
/// are left as extracted.
pub fn form(tag: &LooseTag, tables: &mut Tables<'_>) -> Result<LooseTag> {
    let Body::Tree(tree) = &tag.body else {
        return Ok(tag.clone());
    };
    let key = tag.key.clone();
    let mut t = tree.clone();
    for r in &mut t.raws {
        r.origin = Origin::Local;
    }
    match key.group {
        SOUND => sound(&mut t, &key, tables)?,
        BITMAP => bitmap(&mut t, tables.low_detail),
        _ => {}
    }
    if NOISY_SIZES.iter().any(|g| FourCC(**g) == key.group)
        && let Some(root) = tables.schema.and_then(|s| s.groups.get(&key.group))
    {
        cut(&mut t, 0, root, true, tables)?;
    }
    Ok(LooseTag {
        key,
        body: Body::Tree(t),
    })
}

/// The asset hash of a normal form: the SHA-256 of [`VERSION`], the group
/// and each of its lines ([`canonical`]) with its length.
pub fn hash(form: &LooseTag) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(b"h2 asset normal form\0");
    h.update(&VERSION.to_le_bytes());
    h.update(&form.key.group.0);
    for line in canonical(form) {
        h.update((line.len() as u32).to_le_bytes());
        h.update(line.as_bytes());
    }
    h.finalize().into()
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Element `i` of an array of `size`-byte elements.
fn elem(node: &Node, size: usize, i: usize) -> Option<&[u8]> {
    (node.size as usize == size)
        .then(|| node.bytes.get(i * size..(i + 1) * size))
        .flatten()
}

/// The array a block field at `offset` of the root of `tree` names.
fn block<'a>(root: &'a Node, tree: &'a Tree, offset: u32) -> Option<(u32, &'a Node)> {
    let value = root
        .fields
        .iter()
        .find(|(o, _)| *o == offset)
        .map(|(_, v)| v)?;
    let (Value::Block(child) | Value::Data(child)) = value else {
        return None;
    };
    Some((*child, tree.nodes.get(*child as usize)?))
}

/// `block` when its elements are `size` bytes.
fn rows<'a>(block: Option<(u32, &'a Node)>, size: usize) -> Option<&'a Node> {
    match block {
        Some((_, n)) if n.size as usize == size => Some(n),
        _ => None,
    }
}

/// A form under construction: its nodes and raw resources, and the tree
/// rows are moved in from.
struct Builder<'a> {
    from: &'a Tree,
    nodes: Vec<Node>,
    raws: Vec<RawRes>,
}

impl<'a> Builder<'a> {
    fn new(from: &'a Tree) -> Self {
        Builder {
            from,
            nodes: Vec::new(),
            raws: Vec::new(),
        }
    }

    /// A node of one element of `size` zero bytes; its index.
    fn open(&mut self, size: u32) -> u32 {
        self.nodes.push(Node {
            bytes: vec![0; size as usize],
            count: 1,
            size,
            align: 4,
            fields: Vec::new(),
        });
        (self.nodes.len() - 1) as u32
    }

    /// A block field of node `at` at byte `offset`: one element, `child`.
    fn link(&mut self, at: u32, offset: usize, child: u32) {
        let node = &mut self.nodes[at as usize];
        node.bytes[offset..offset + 4].copy_from_slice(&1u32.to_le_bytes());
        node.fields.push((offset as u32, Value::Block(child)));
    }

    /// A raw resource of the source tree as content; its index among the
    /// form's.
    fn raw(&mut self, raw: u32) -> u32 {
        let r = &self.from.raws[raw as usize];
        let i = self.raws.len() as u32;
        self.raws.push(RawRes {
            origin: Origin::Local,
            len: r.len,
            hash: r.hash,
        });
        i
    }

    /// Element `i` of the array `node` (`size`-byte elements) as a node of
    /// its own, its child arrays and raw resources with it.
    fn element(&mut self, node: &Node, size: usize, i: usize) -> Option<u32> {
        let bytes = elem(node, size, i)?;
        let me = self.open(size as u32);
        self.nodes[me as usize].bytes.copy_from_slice(bytes);
        let lo = i * size;
        let fields: Vec<(u32, Value)> = node
            .fields
            .iter()
            .filter(|(o, _)| *o as usize >= lo && (*o as usize) < lo + size)
            .map(|(o, v)| (*o - lo as u32, v.clone()))
            .collect();
        for (o, v) in fields {
            let v = match v {
                Value::Block(c) => Value::Block(self.graft(c)),
                Value::Data(c) => Value::Data(self.graft(c)),
                Value::Raw(r) => Value::Raw(self.raw(r)),
                v => v,
            };
            self.nodes[me as usize].fields.push((o, v));
        }
        Some(me)
    }

    /// The array `node` of the source tree, whole, as a node of the form.
    fn graft(&mut self, node: u32) -> u32 {
        let from = self.from;
        let src = &from.nodes[node as usize];
        let me = self.nodes.len() as u32;
        self.nodes.push(Node {
            bytes: src.bytes.clone(),
            count: src.count,
            size: src.size,
            align: src.align,
            fields: Vec::new(),
        });
        let fields = src
            .fields
            .iter()
            .map(|(o, v)| match v {
                Value::Block(c) => (*o, Value::Block(self.graft(*c))),
                Value::Data(c) => (*o, Value::Data(self.graft(*c))),
                Value::Raw(r) => (*o, Value::Raw(self.raw(*r))),
                v => (*o, v.clone()),
            })
            .collect();
        self.nodes[me as usize].fields = fields;
        me
    }
}

/// The form of a sound: its header with the indices out and the rows they
/// name in, as block fields of the header past its 20 bytes, in this
/// order: the playback, each pitch range (holding its permutations, each
/// holding its chunks), the extra info.
fn sound(t: &mut Tree, key: &TagKey, tables: &Tables<'_>) -> Result<()> {
    let root = &t.nodes[0];
    if root.count != 1 || root.size != SOUND_HEADER || root.bytes.len() != SOUND_HEADER as usize {
        return Ok(());
    }
    let header = root.bytes.clone();
    let playback_at = u16_at(&header, SOUND_PLAYBACK) as usize;
    let first = u16_at(&header, SOUND_FIRST_RANGE) as usize;
    let count = header[SOUND_RANGE_COUNT] as usize;
    let extra = u16_at(&header, SOUND_EXTRA_INFO);
    let Some(gestalt) = tables.gestalt.and_then(LooseTag::tree) else {
        return Ok(());
    };
    if gestalt.nodes.is_empty()
        || gestalt.nodes[0].count != 1
        || gestalt.nodes[0].size != GESTALT_HEADER
    {
        return Ok(());
    }
    let owner = tables.gestalt.map_or("-", |g| g.key.path.as_str());
    let bad = |what: String| {
        Error::Invalid(format!(
            "{key}: {what} (its map's sound gestalt is {owner}.{GESTALT})"
        ))
    };
    let root = &gestalt.nodes[0];
    // An index past the playbacks is not one: the header keeps it.
    let playback = rows(block(root, gestalt, GESTALT_PLAYBACKS), PLAYBACK_SIZE)
        .filter(|n| playback_at < n.count as usize);
    let ranges = rows(block(root, gestalt, GESTALT_RANGES), RANGE_SIZE);
    let permutations = rows(block(root, gestalt, GESTALT_PERMUTATIONS), PERMUTATION_SIZE);
    let chunks = rows(block(root, gestalt, GESTALT_CHUNKS), CHUNK_SIZE);
    let extra_infos = rows(block(root, gestalt, GESTALT_EXTRA_INFOS), EXTRA_INFO_SIZE);
    let has_extra = extra != NO_EXTRA_INFO;

    let mut b = Builder::new(gestalt);
    let size = SOUND_HEADER
        + 8 * count as u32
        + 8 * u32::from(playback.is_some())
        + 8 * u32::from(has_extra);
    let me = b.open(size);
    b.nodes[me as usize].bytes[..SOUND_HEADER as usize].copy_from_slice(&header);
    {
        let h = &mut b.nodes[me as usize].bytes;
        h[SOUND_FIRST_RANGE..SOUND_FIRST_RANGE + 2].fill(0);
        h[SOUND_EXTRA_INFO..SOUND_EXTRA_INFO + 2].copy_from_slice(&NO_EXTRA_INFO.to_le_bytes());
        if playback.is_some() {
            h[SOUND_PLAYBACK..SOUND_PLAYBACK + 2].fill(0);
        }
    }
    let mut at = SOUND_HEADER as usize;
    if let Some(node) = playback {
        let row = b
            .element(node, PLAYBACK_SIZE, playback_at)
            .expect("index and stride checked");
        b.link(me, at, row);
        at += 8;
    }
    for r in first..first.saturating_add(count) {
        let array = ranges.ok_or_else(|| bad(format!("names pitch range {r}")))?;
        let row = elem(array, RANGE_SIZE, r)
            .ok_or_else(|| bad(format!("names pitch range {r} of {}", array.count)))?;
        let from = u16_at(row, RANGE_FIRST_PERMUTATION) as usize;
        let n = u16_at(row, RANGE_FIRST_PERMUTATION + 2) as usize;
        let range = b.open((RANGE_SIZE + 8 * n) as u32);
        b.nodes[range as usize].bytes[..RANGE_SIZE].copy_from_slice(row);
        b.nodes[range as usize].bytes[RANGE_FIRST_PERMUTATION..RANGE_FIRST_PERMUTATION + 2].fill(0);
        for (j, p) in (from..from.saturating_add(n)).enumerate() {
            let array = permutations
                .ok_or_else(|| bad(format!("pitch range {r} names permutation {p}")))?;
            let row = elem(array, PERMUTATION_SIZE, p).ok_or_else(|| {
                bad(format!(
                    "pitch range {r} names permutation {p} of {}",
                    array.count
                ))
            })?;
            let from = u16_at(row, PERMUTATION_FIRST_CHUNK) as usize;
            let n = u16_at(row, PERMUTATION_FIRST_CHUNK + 2) as usize;
            let permutation = b.open((PERMUTATION_SIZE + 8 * n) as u32);
            b.nodes[permutation as usize].bytes[..PERMUTATION_SIZE].copy_from_slice(row);
            b.nodes[permutation as usize].bytes
                [PERMUTATION_FIRST_CHUNK..PERMUTATION_FIRST_CHUNK + 2]
                .fill(0);
            for (k, c) in (from..from.saturating_add(n)).enumerate() {
                let array =
                    chunks.ok_or_else(|| bad(format!("permutation {p} names chunk {c}")))?;
                let row = elem(array, CHUNK_SIZE, c).ok_or_else(|| {
                    bad(format!(
                        "permutation {p} names chunk {c} of {}",
                        array.count
                    ))
                })?;
                let chunk = b.open(CHUNK_SIZE as u32);
                b.nodes[chunk as usize].bytes.copy_from_slice(row);
                if let Some((_, Value::Raw(raw))) = array
                    .fields
                    .iter()
                    .find(|(o, _)| *o as usize == c * CHUNK_SIZE)
                {
                    let raw = b.raw(*raw);
                    b.nodes[chunk as usize].fields.push((0, Value::Raw(raw)));
                }
                b.link(permutation, PERMUTATION_SIZE + 8 * k, chunk);
            }
            b.link(range, RANGE_SIZE + 8 * j, permutation);
        }
        b.link(me, at, range);
        at += 8;
    }
    if has_extra {
        let array = extra_infos.ok_or_else(|| bad(format!("names extra info {extra}")))?;
        let node = b
            .element(array, EXTRA_INFO_SIZE, extra as usize)
            .ok_or_else(|| bad(format!("names extra info {extra} of {}", array.count)))?;
        b.nodes[node as usize]
            .fields
            .retain(|(o, _)| *o != EXTRA_INFO_OWNER);
        b.link(me, at, node);
        at += 8;
    }
    debug_assert_eq!(at, size as usize);
    t.nodes = b.nodes;
    t.raws.extend(b.raws);
    Ok(())
}

/// The form of a bitmap: each image's low-detail picture as a raw resource
/// named where the image held the picture's offset into its map's region.
/// A picture the region does not hold stays as extracted.
fn bitmap(t: &mut Tree, low_detail: &[u8]) {
    if low_detail.is_empty() {
        return;
    }
    let root = &t.nodes[0];
    if root.count != 1 || root.size != BITMAP_HEADER {
        return;
    }
    let Some((images, node)) = block(root, t, BITMAP_IMAGES) else {
        return;
    };
    if node.size as usize != IMAGE_SIZE {
        return;
    }
    let mut moved = Vec::new();
    for i in 0..node.count as usize {
        let e = &node.bytes[i * IMAGE_SIZE..(i + 1) * IMAGE_SIZE];
        let offset = u32_at(e, IMAGE_REDUCED) as usize;
        let size = u32_at(e, IMAGE_REDUCED + 4) as usize;
        if size > 0
            && offset
                .checked_add(size)
                .is_some_and(|end| end <= low_detail.len())
        {
            moved.push((i * IMAGE_SIZE + IMAGE_REDUCED, offset, size));
        }
    }
    for (at, offset, size) in moved {
        let raw = t.raws.len() as u32;
        t.raws.push(RawRes {
            origin: Origin::Local,
            len: size as u32,
            hash: sha256(&low_detail[offset..offset + size]),
        });
        let node = &mut t.nodes[images as usize];
        node.bytes[at..at + 4].fill(0);
        node.fields.push((at as u32, Value::Raw(raw)));
    }
    t.nodes[images as usize].fields.sort_by_key(|f| f.0);
}

/// Cut each raw resource of node `at` (elements laid out as `s`) to the
/// size its element stores ([`Field::RawSize`]), where that is shorter
/// than the length extracted, and walk the node's blocks the same way.
/// A field of the root's locale descriptors keeps its descriptor's length
/// (`localization::matg_locale_raw_len`).
fn cut(t: &mut Tree, at: u32, s: &Struct, root: bool, tables: &mut Tables<'_>) -> Result<()> {
    let node = &t.nodes[at as usize];
    let Some(size) = (node.size > 1)
        .then_some(node.size as usize)
        .filter(|n| *n == s.size as usize)
    else {
        return Ok(());
    };
    let mut cuts = Vec::new();
    for (o, v) in &node.fields {
        let Value::Raw(raw) = v else {
            continue;
        };
        let element = *o as usize / size;
        let within = *o as usize % size;
        // The size field names the raw it sizes ([`Field::RawSize`]).
        let Some(size_at) = s.fields.iter().find_map(|(k, f)| {
            matches!(f, Field::RawSize(of) if *of as usize == within).then_some(*k)
        }) else {
            continue;
        };
        if root && crate::localization::matg_locale_raw_len(&node.bytes, *o).is_some() {
            continue;
        }
        let stored = u32_at(&node.bytes, element * size + size_at as usize);
        let raw_index = raw;
        let raw = &t.raws[*raw as usize];
        if stored == 0 || stored >= raw.len {
            continue;
        }
        let Some(read) = tables.raw.as_deref_mut() else {
            continue;
        };
        let bytes = read(&raw.hash)?;
        if bytes.len() != raw.len as usize {
            return Err(Error::Invalid(format!(
                "raw resource {} is {} bytes in the store, {} in the tag",
                raw.hex(),
                bytes.len(),
                raw.len
            )));
        }
        cuts.push((
            *raw_index,
            RawRes {
                origin: Origin::Local,
                len: stored,
                hash: sha256(&bytes[..stored as usize]),
            },
        ));
    }
    for (raw, shorter) in cuts {
        t.raws[raw as usize] = shorter;
    }
    let children: Vec<(u32, u32)> = t.nodes[at as usize]
        .fields
        .iter()
        .filter_map(|(o, v)| match v {
            Value::Block(c) => Some((*o, *c)),
            _ => None,
        })
        .collect();
    for (o, c) in children {
        if let Some(Field::Block(child)) = s.fields.get(&o) {
            cut(t, c, child, false, tables)?;
        }
    }
    Ok(())
}
