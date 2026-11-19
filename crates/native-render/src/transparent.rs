//! Stage 3, the transparents: which sorted transparent entries are
//! geometry the native renderer can draw, and the runs it draws them in
//! (docs/graphics/native-renderer.md, "Stage 3: transparents").
//!
//! Research points (Code, `h2 disasm`):
//!
//! - The transparent list: `[0x004b6288]` entries of `0x70` bytes, the
//!   live ones from `[0x004b6284]` to `[0x004b6280]` (at most `0x400`),
//!   allocated by `0x0001e2d0`. `SCENE_TRANSPARENTS` (`0x0001e370`) fills
//!   the `u16` index array `[0x004b628c]`, sorts it with `0x0013df80`
//!   (comparator `0x0001e700`), then calls each entry's `+0x20` with the
//!   entry's `+0x24` as its one stdcall argument, skipping a null `+0x20`.
//! - A geometry entry (`0x00041040`'s transparent branch, from
//!   `0x00041381`): `+0` its sort class (`0x00040e10`), `+4` the view
//!   depth, `+0xc` a byte (a plane at `+0x10` follows when set), `+0x20`
//!   the handler `0x00041020`, `+0x24` the shader datum, `+0x28` the
//!   argument the opaque items carry at `+0xc` (render record index
//!   `<< 8`, the low byte a part index), `+0x64` the position.
//! - The handler `0x00041020` (stdcall, the entry's `+0x24`, `ret 4`) runs
//!   `0x00040f60` with the opaque path's build `0x0004cbf0` and draw
//!   `0x0004d0b0`, once per pass of the template's layer 15 (the word at
//!   `+0x1e` of the template's layer table: `count << 9 | first`).
//! - The render record (`0x004ba138 + 0x20 · index`): `+4` the render
//!   model tag datum (`-1` for the BSP); `+0xc` bits 9..17 the cluster,
//!   bits 18..28 the instanced-geometry placement (`0x7ff` none) for BSP
//!   records; an object's record names its object through its lighting
//!   pointer `+0x10` ([`crate::objects::record_owner`]).
//!
//! The guest runs the handlers in sorted order. The native stage uses this
//! record to identify the current part at its resolved indexed draw.

use crate::camera::GuestRead;
use crate::objects::{RecordOwner, record_owner};
use crate::skip::{RENDER_RECORDS, RENDER_RECORD_SIZE};

/// The geometry entries' handler.
pub const GEOMETRY_HANDLER: u32 = 0x0004_1020;
/// The handler's argument is the entry's `+0x24`; the entry starts here
/// before it.
pub const DATA_OFFSET: u32 = 0x24;
/// Bits 18..28 of a BSP record's `+0xc` when it is a cluster.
const NO_PLACEMENT: u32 = 0x7ff;

/// What a transparent geometry entry draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A BSP cluster's section, by cluster index.
    Cluster(u16),
    /// An instanced-geometry placement, by placement index.
    Placement(u16),
    /// An object, by handle.
    Object(u32),
}

/// One transparent geometry entry, as the handler meets it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    /// The shader datum (`+0x24`).
    pub shader: u32,
    /// Render record index encoded alongside the part.
    pub record: u16,
    /// The part index (the argument's low byte).
    pub part: u8,
    pub source: Source,
    /// The view depth the sort used (`+4`).
    pub depth: f32,
    /// The entry's position (`+0x64`).
    pub position: [f32; 3],
}

/// Reads the geometry entry whose handler argument is `data` (the entry's
/// `+0x24`). `None` when it cannot be read, its record is not one the
/// geometry path fills, or it is an object's whose owner does not resolve.
pub fn read_entry(mem: &dyn GuestRead, data: u32) -> Option<Entry> {
    let entry = data.checked_sub(DATA_OFFSET)?;
    if mem.read_u32(data.checked_sub(4)?)? != GEOMETRY_HANDLER {
        return None;
    }
    let shader = mem.read_u32(data)?;
    let arg = mem.read_u32(data.checked_add(4)?)?;
    let record = (arg >> 8) as u16 as i16;
    if record < 0 {
        return None;
    }
    let source = match record_owner(mem, record as u16)? {
        RecordOwner::Object(handle) => Source::Object(handle),
        RecordOwner::Unresolved => return None,
        RecordOwner::World => {
            let packed = mem.read_u32(RENDER_RECORDS + RENDER_RECORD_SIZE * record as u32 + 0xc)?;
            match (packed >> 18) & 0x7ff {
                NO_PLACEMENT => Source::Cluster(((packed >> 9) & 0x1ff) as u16),
                p => Source::Placement(p as u16),
            }
        }
    };
    let position = [
        mem.read_f32(entry.checked_add(0x64)?)?,
        mem.read_f32(entry.checked_add(0x68)?)?,
        mem.read_f32(entry.checked_add(0x6c)?)?,
    ];
    Some(Entry {
        shader,
        record: record as u16,
        part: arg as u8,
        source,
        depth: mem.read_f32(entry + 4)?,
        position,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::Mem;

    const ENTRY: u32 = 0x0069_0000;

    /// Entry of render record `record`'s part `part`: the BSP's with
    /// `packed` at `+0xc` for `lighting` `None`, else an object's lit by
    /// `lighting`.
    fn entry(m: &mut Mem, record: u32, part: u32, lighting: Option<u32>, packed: u32) {
        m.w(ENTRY + 4, (-12.5f32).to_bits());
        m.w(ENTRY + 0x20, GEOMETRY_HANDLER);
        m.w(ENTRY + 0x24, 0xe000_0042);
        m.w(ENTRY + 0x28, record << 8 | part);
        for (i, v) in [1.0f32, 2.0, 3.0].into_iter().enumerate() {
            m.w(ENTRY + 0x64 + 4 * i as u32, v.to_bits());
        }
        crate::objects::tests::record(m, record, lighting);
        m.w(RENDER_RECORDS + 0x20 * record + 0xc, packed);
    }

    #[test]
    fn a_cluster_entry_names_its_cluster_part_and_shader() {
        let mut m = Mem::default();
        entry(&mut m, 9, 2, None, NO_PLACEMENT << 18 | 37 << 9 | 5);
        let e = read_entry(&m, ENTRY + DATA_OFFSET).unwrap();
        assert_eq!(
            e,
            Entry {
                shader: 0xe000_0042,
                record: 9,
                part: 2,
                source: Source::Cluster(37),
                depth: -12.5,
                position: [1.0, 2.0, 3.0],
            }
        );
    }

    #[test]
    fn placements_and_objects_are_told_apart() {
        let mut m = Mem::default();
        entry(&mut m, 3, 0, None, 140 << 18 | 0x1ff << 9);
        assert_eq!(read_entry(&m, ENTRY + DATA_OFFSET).unwrap().source, Source::Placement(140));
        // An object's: named by its lighting pointer's cache record owner,
        // not by the record's model datum.
        let h = crate::objects::tests::object(&mut m, 4, 0xe000_0100, 1);
        entry(&mut m, 3, 0, Some(crate::objects::tests::lighting(4 + 7)), 0);
        assert_eq!(read_entry(&m, ENTRY + DATA_OFFSET).unwrap().source, Source::Object(h));
        // An object's whose owner does not resolve: not read (D3D draws it).
        entry(&mut m, 3, 0, Some(0), 0);
        assert_eq!(read_entry(&m, ENTRY + DATA_OFFSET), None);
        // A negative record (no render record): not geometry this reads.
        m.w(ENTRY + 0x28, 0xffff_ff00);
        assert_eq!(read_entry(&m, ENTRY + DATA_OFFSET), None);
    }
}
