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
//! - The render record (`0x004ba138 + 0x20 · index`): `+4` the object
//!   handle (`-1` for the BSP); `+0xc` bits 9..17 the cluster, bits 18..28
//!   the instanced-geometry placement (`0x7ff` none) for BSP records.
//!
//! The skip is the handler's: a `before` hook answers `ret 4` for an
//! entry the native renderer draws, so the entry builds and draws nothing
//! and no guest memory changes. The sort order is the game's: the hook
//! meets the entries in it. Consecutive native entries share one
//! [`Run`], one external pass the device issues at its next entry, which
//! is the next D3D entry's draw or whatever follows the transparents: so
//! every native run lands between the D3D draws it sorts between.

use std::sync::Mutex;

use crate::camera::GuestRead;
use crate::objects::ObjectFrame;
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
    /// The part index (the argument's low byte).
    pub part: u8,
    pub source: Source,
    /// The view depth the sort used (`+4`).
    pub depth: f32,
    /// The entry's position (`+0x64`).
    pub position: [f32; 3],
}

/// Reads the geometry entry whose handler argument is `data` (the entry's
/// `+0x24`). `None` when it cannot be read or its record is not one the
/// geometry path fills.
pub fn read_entry(mem: &dyn GuestRead, data: u32) -> Option<Entry> {
    let entry = data.checked_sub(DATA_OFFSET)?;
    let shader = mem.read_u32(data)?;
    let arg = mem.read_u32(data + 4)?;
    let record = (arg >> 8) as u16 as i16;
    if record < 0 {
        return None;
    }
    let at = RENDER_RECORDS + RENDER_RECORD_SIZE * record as u32;
    let handle = mem.read_u32(at + 4)?;
    let source = if handle != u32::MAX {
        Source::Object(handle)
    } else {
        let packed = mem.read_u32(at + 0xc)?;
        match (packed >> 18) & 0x7ff {
            NO_PLACEMENT => Source::Cluster(((packed >> 9) & 0x1ff) as u16),
            p => Source::Placement(p as u16),
        }
    };
    let position = [
        mem.read_f32(entry + 0x64)?,
        mem.read_f32(entry + 0x68)?,
        mem.read_f32(entry + 0x6c)?,
    ];
    Some(Entry {
        shader,
        part: arg as u8,
        source,
        depth: mem.read_f32(entry + 4)?,
        position,
    })
}

/// One entry of a run: the entry, and for an object its node matrices and
/// sphere as the hook copied them out of the render's snapshot of the
/// frame (`crate::objects::read_object`), so the pass draws the frame's
/// pose whatever the next tick does to the body.
#[derive(Clone, Debug, PartialEq)]
pub struct RunEntry {
    pub entry: Entry,
    pub object: Option<ObjectFrame>,
}

/// Consecutive native entries in the game's sort order: one external
/// pass. The feature adds entries while the pass waits in the device; the
/// pass takes them when the device issues it ([`Run::seal`]), after which
/// the next native entry needs a new run (a D3D draw came between).
#[derive(Debug, Default)]
pub struct Run {
    inner: Mutex<(Vec<RunEntry>, bool)>,
}

impl Run {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `entry` (with its object's copy, for an object); `false` once
    /// the run is sealed.
    pub fn push(&self, entry: Entry, object: Option<ObjectFrame>) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.1 {
            return false;
        }
        g.0.push(RunEntry { entry, object });
        true
    }

    /// Takes the entries and closes the run (the device issued its pass).
    pub fn seal(&self) -> Vec<RunEntry> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.1 = true;
        std::mem::take(&mut g.0)
    }

    pub fn is_sealed(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).1
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::Mem;

    const ENTRY: u32 = 0x0070_0000;

    fn entry(m: &mut Mem, record: u32, part: u32, handle: u32, packed: u32) {
        m.w(ENTRY + 4, (-12.5f32).to_bits());
        m.w(ENTRY + 0x20, GEOMETRY_HANDLER);
        m.w(ENTRY + 0x24, 0xe000_0042);
        m.w(ENTRY + 0x28, record << 8 | part);
        for (i, v) in [1.0f32, 2.0, 3.0].into_iter().enumerate() {
            m.w(ENTRY + 0x64 + 4 * i as u32, v.to_bits());
        }
        m.w(RENDER_RECORDS + 0x20 * record + 4, handle);
        m.w(RENDER_RECORDS + 0x20 * record + 0xc, packed);
    }

    #[test]
    fn a_cluster_entry_names_its_cluster_part_and_shader() {
        let mut m = Mem::default();
        entry(&mut m, 9, 2, u32::MAX, NO_PLACEMENT << 18 | 37 << 9 | 5);
        let e = read_entry(&m, ENTRY + DATA_OFFSET).unwrap();
        assert_eq!(
            e,
            Entry {
                shader: 0xe000_0042,
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
        entry(&mut m, 3, 0, u32::MAX, 140 << 18 | 0x1ff << 9);
        assert_eq!(read_entry(&m, ENTRY + DATA_OFFSET).unwrap().source, Source::Placement(140));
        entry(&mut m, 3, 0, 0xe1a3_0004, 0);
        assert_eq!(read_entry(&m, ENTRY + DATA_OFFSET).unwrap().source, Source::Object(0xe1a3_0004));
        // A negative record (no render record): not geometry this reads.
        m.w(ENTRY + 0x28, 0xffff_ff00);
        assert_eq!(read_entry(&m, ENTRY + DATA_OFFSET), None);
    }

    #[test]
    fn a_sealed_run_takes_no_more_entries() {
        let mut m = Mem::default();
        entry(&mut m, 1, 0, u32::MAX, NO_PLACEMENT << 18);
        let e = read_entry(&m, ENTRY + DATA_OFFSET).unwrap();
        let run = Run::new();
        assert!(run.push(e, None) && run.push(e, None));
        assert_eq!(run.seal().len(), 2);
        assert!(run.is_sealed() && run.is_empty());
        assert!(!run.push(e, None));
    }
}
