//! The decals of the open view, from the decal datums
//! (docs/graphics/native-decals-research.md, "The dynamic decal path"):
//! what the guest's decal callback `0x00044040` draws, as
//! [`frame_scene::DecalItem`]s in its order.
//!
//! The guest calls the callback once per visible cluster of the list at
//! `0x00547f94`, for each of the seven modes in turn (`0x0002c370`). The
//! callback takes the head of bucket `mode * 512 + cluster` in the table at
//! `0x004ea94c`, follows each record's `+0x34` link, and draws a record
//! once: the record's owner (`+0x38`) carries a frame marker at `+8` that
//! it compares with, and then sets to, the frame word `0x004ba038`. A
//! record owner met again in the view is skipped here the same way. The
//! one draw is `quads` quads (`+0x2a`) of 16-byte vertices at the stream
//! base `0x00509444` plus the allocation's offset.
//!
//! Extraction only reads: the markers are not written, because the guest
//! callback does not run once its body is cut. A record that does not read
//! is the view's decal remainder, which the native stage counts
//! `scene_dropped_decal_unread`.

use std::collections::HashSet;

use frame_scene::decal::{DecalItem, DecalVertex, VERTEX_SIZE};
use frame_scene::{BuildError, SceneBuilder};

use crate::GuestRead;

/// The bucket table: `u32` heads, `mode * 512 + cluster`.
pub const GROUPS: u32 = 0x004e_a94c;
/// The decal datum array header; its data (`+0x44`) holds `0x40`-byte records.
pub const DECALS: u32 = 0x004e_a950;
/// The visible-cluster list the callback walks.
pub const CLUSTERS: u32 = 0x0054_7f94;
/// The dynamic geometry allocator: its allocation array (`+0x64`) and the
/// byte shift of an allocation's offset (`+0x34`).
pub const GEOMETRY: u32 = 0x0050_9448;
/// The vertex stream the decals' geometry lives in; its base is `+4`.
pub const STREAM: u32 = 0x0050_9444;

/// Decal modes the guest has.
pub const MODES: u8 = 7;
/// Clusters per mode in the bucket table.
pub const BUCKET_CLUSTERS: u32 = 512;
/// Bytes of a record.
pub const RECORD_SIZE: u32 = 0x40;
/// The longest chain or list followed: the datum array's own bound.
const LIMIT: u32 = 1 << 16;
/// The most quads one record may draw.
const MAX_QUADS: i32 = 1 << 12;
const NONE: u32 = u32::MAX;

/// What [`extract`] put in the view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decals {
    pub items: u32,
    pub quads: u32,
    /// Records that did not read.
    pub remainder: u32,
}

fn word16(mem: &dyn GuestRead, addr: u32) -> Option<u32> {
    mem.read_u32(addr).map(|w| w & 0xffff)
}

/// A record read: the item and where its vertices are.
struct Record {
    owner: u32,
    item: DecalItem,
    quads: u32,
    vertices: u32,
    next: u32,
}

fn read_record(mem: &dyn GuestRead, data: u32, at: u32, mode: u8, cluster: u16, index: u32) -> Result<Record, ()> {
    let word = |off: u32| mem.read_u32(at.checked_add(off)?);
    let packed = word(0x28).ok_or(())?;
    let quads = (packed >> 16) as i16 as i32;
    if !(0..=MAX_QUADS).contains(&quads) {
        return Err(());
    }
    let owner = (word(0x38).ok_or(())? & 0xffff) * RECORD_SIZE + data;
    // The allocation's offset, as `0x00044161` forms the start vertex.
    let geometry = mem.read_u32(GEOMETRY).filter(|&g| g != 0).ok_or(())?;
    let allocations = mem.read_u32(geometry.checked_add(0x64).ok_or(())?).ok_or(())?;
    let allocations = mem.read_u32(allocations.checked_add(0x44).ok_or(())?).ok_or(())?;
    let alloc = word(4).ok_or(())? & 0xffff;
    let offset = mem.read_u32(allocations.checked_add(alloc * 24 + 8).ok_or(())?).ok_or(())?;
    let shift = mem.read_u32(geometry.checked_add(0x34).ok_or(())?).ok_or(())? & 0xff;
    let offset = offset.checked_shl(shift).ok_or(())?;
    let base = mem.read_u32(STREAM).ok_or(())?;
    let base = mem.read_u32(base.checked_add(4).ok_or(())?).ok_or(())? | 0x8000_0000;
    Ok(Record {
        owner,
        quads: quads as u32,
        vertices: base.checked_add(offset).ok_or(())?,
        next: word(0x34).ok_or(())?,
        item: DecalItem {
            mode,
            cluster,
            record: index as u16,
            bitmap: word(0x2c).ok_or(())?,
            image: (packed >> 8) as u8 as i8,
            colour: word(0x24).ok_or(())?,
            vertices: 0..0,
        },
    })
}

fn read_vertices(mem: &dyn GuestRead, at: u32, quads: u32) -> Option<Vec<DecalVertex>> {
    (0..quads * 4)
        .map(|v| {
            let w = |k: u32| mem.read_u32(at.checked_add(v * VERTEX_SIZE + 4 * k)?);
            Some(DecalVertex { position: [w(0)?, w(1)?, w(2)?], packed: w(3)? })
        })
        .collect()
}

/// The decals of the current cluster list into the open view, in the
/// guest's order: modes 0 to 6, each over the list's clusters, each
/// bucket's chain from its head. `None` when the tables do not read at
/// all (the view has no decal state yet).
pub fn extract(mem: &dyn GuestRead, builder: &mut SceneBuilder) -> Result<Option<Decals>, BuildError> {
    let (Some(groups), Some(header), Some(list)) = (
        mem.read_u32(GROUPS).filter(|&g| g != 0),
        mem.read_u32(DECALS).filter(|&d| d != 0),
        mem.read_u32(CLUSTERS).filter(|&l| l != 0),
    ) else {
        return Ok(None);
    };
    let (Some(data), Some(count), Some(values)) = (
        mem.read_u32(header + 0x44),
        word16(mem, list + 4),
        mem.read_u32(list + 0xc),
    ) else {
        return Ok(None);
    };
    let mut seen = HashSet::new();
    let mut out = Decals::default();
    for mode in 0..MODES {
        for n in 0..count {
            let Some(cluster) = mem.read_u32(values + 4 * n).map(|v| v as u16) else {
                out.remainder += 1;
                continue;
            };
            if u32::from(cluster) >= BUCKET_CLUSTERS {
                continue;
            }
            let bucket = groups + 4 * (u32::from(mode) * BUCKET_CLUSTERS + u32::from(cluster));
            let Some(mut head) = mem.read_u32(bucket) else {
                out.remainder += 1;
                continue;
            };
            let mut followed = 0;
            while head != NONE && followed < LIMIT {
                followed += 1;
                let index = head & 0xffff;
                let at = data + index * RECORD_SIZE;
                let Ok(record) = read_record(mem, data, at, mode, cluster, index) else {
                    out.remainder += 1;
                    break;
                };
                head = record.next;
                if !seen.insert(record.owner) || record.quads == 0 {
                    continue;
                }
                let Some(vertices) = read_vertices(mem, record.vertices, record.quads) else {
                    out.remainder += 1;
                    continue;
                };
                builder.push_decal(record.item, &vertices)?;
                out.items += 1;
                out.quads += record.quads;
            }
        }
    }
    builder.add_decal_remainder(out.remainder)?;
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;
    use frame_scene::{SceneId, Stamp, ViewKey, ViewKind};

    const HEAD_TABLE: u32 = 0x0100_0000;
    const DATA: u32 = 0x0101_0000;
    const LIST: u32 = 0x0102_0000;
    const VALUES: u32 = 0x0102_0100;
    const ALLOCS: u32 = 0x0103_0000;
    const GEO: u32 = 0x0104_0000;
    const GEO_ARRAY: u32 = 0x0104_0100;
    const STREAM_AT: u32 = 0x0104_0200;
    const VERTICES: u32 = 0x8100_0000;

    /// Tables for two visible clusters (3 and 7); the stream is at
    /// `VERTICES - 0x80000000 + ...`: the test memory serves the
    /// contiguous alias directly.
    fn tables(m: &mut Mem) {
        m.w(GROUPS, HEAD_TABLE);
        m.w(DECALS, 0x0105_0000);
        m.w(0x0105_0000 + 0x44, DATA);
        m.w(CLUSTERS, LIST);
        m.w(LIST + 4, 2);
        m.w(LIST + 0xc, VALUES);
        m.w(VALUES, 3);
        m.w(VALUES + 4, 7);
        m.w(GEOMETRY, GEO);
        m.w(GEO + 0x64, GEO_ARRAY);
        m.w(GEO + 0x34, 4);
        m.w(GEO_ARRAY + 0x44, ALLOCS);
        m.w(STREAM, STREAM_AT);
        m.w(STREAM_AT + 4, VERTICES & 0x7fff_ffff);
        for i in 0..(MODES as u32 * BUCKET_CLUSTERS) {
            m.w(HEAD_TABLE + 4 * i, NONE);
        }
    }

    /// Record `index` in bucket (`mode`, `cluster`) chain: owner `owner`,
    /// `quads` quads, allocation `alloc` at byte offset `offset` (before
    /// the shift), link `next`.
    fn record(m: &mut Mem, index: u32, owner: u32, quads: u32, alloc: u32, offset: u32, next: u32) {
        let at = DATA + index * RECORD_SIZE;
        m.w(at + 4, alloc);
        m.w(at + 0x24, 0xff00_0000 | index);
        m.w(at + 0x28, quads << 16 | 0xfe << 8);
        m.w(at + 0x2c, 0xe600_0000 | index);
        m.w(at + 0x34, next);
        m.w(at + 0x38, owner);
        m.w(ALLOCS + alloc * 24 + 8, offset);
        for v in 0..quads * 4 {
            let p = VERTICES + (offset << 4) + v * VERTEX_SIZE;
            m.f(p, index as f32);
            m.f(p + 4, v as f32);
            m.f(p + 8, 0.5);
            m.w(p + 12, v);
        }
    }

    fn open() -> SceneBuilder {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        let camera = frame_scene::RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.0,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 100.0,
        };
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera).unwrap();
        b
    }

    #[test]
    fn decal_items_match_the_datum_list() {
        let mut m = Mem::default();
        tables(&mut m);
        // Mode 2, cluster 3: records 0 then 1 (chain). Mode 0, cluster 7: record 2.
        // Mode 2, cluster 7: record 3, whose owner is record 0's (a duplicate).
        record(&mut m, 0, 0, 2, 0, 0, 1);
        record(&mut m, 1, 1, 1, 1, 8, NONE);
        record(&mut m, 2, 2, 1, 2, 16, NONE);
        record(&mut m, 3, 0, 1, 3, 24, NONE);
        m.w(HEAD_TABLE + 4 * (2 * BUCKET_CLUSTERS + 3), 0);
        m.w(HEAD_TABLE + 4 * (7), 2);
        m.w(HEAD_TABLE + 4 * (2 * BUCKET_CLUSTERS + 7), 3);
        let mut b = open();
        let out = extract(&m, &mut b).unwrap().unwrap();
        assert_eq!(out, Decals { items: 3, quads: 4, remainder: 0 });
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let items = s.decals_of(&s.views[0]);
        // Mode order, then the list's clusters, then each chain.
        assert_eq!(items.iter().map(|d| (d.mode, d.cluster, d.record)).collect::<Vec<_>>(), [(0, 7, 2), (2, 3, 0), (2, 3, 1)]);
        assert_eq!((items[1].bitmap, items[1].image, items[1].colour), (0xe600_0000, -2, 0xff00_0000));
        let v = s.decal_vertices_of(&items[1]);
        assert_eq!(v.len(), 8);
        assert_eq!((v[5].position(), v[5].packed), ([0.0, 5.0, 0.5], 5));
        assert_eq!(s.decal_vertices_of(&items[2])[0].position()[0], 1.0);
    }

    #[test]
    fn a_record_that_does_not_read_is_the_remainder() {
        let mut m = Mem::default();
        tables(&mut m);
        record(&mut m, 0, 0, 1, 0, 0, NONE);
        record(&mut m, 1, 1, 1, 1, 8, NONE);
        // Record 1's vertices are missing.
        m.0.remove(&(VERTICES + 8 * 16));
        m.w(HEAD_TABLE + 4 * 3, 0);
        m.w(HEAD_TABLE + 4 * (BUCKET_CLUSTERS + 3), 1);
        let mut b = open();
        let out = extract(&m, &mut b).unwrap().unwrap();
        assert_eq!((out.items, out.remainder), (1, 1));
        let s = b.seal(Stamp::default()).unwrap();
        assert_eq!(s.views[0].decal_remainder, 1);
        // No tables, no decal state.
        let mut b = open();
        assert_eq!(extract(&Mem::default(), &mut b).unwrap(), None);
    }
}
