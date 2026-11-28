//! The visibility gather's setup, `0x00132b80`, the caller's glue and the
//! reset `0x001348e0`, bit for bit, as a shadow-arm [`Proposal`]
//! (docs/graphics/native-visibility.md, "The gather's setup").
//!
//! The gather `0x001320f0` calls the setup at `0x00132129` with ECX the
//! view flags, EBX the frustum count and nine stack arguments, and the
//! setup returns the mode in EAX with `ret 24h`. The caller then stores
//! the struct at `0x004c0b80` and calls the reset with EDX the struct
//! (`0x00132138`), which returns to `0x0013213d`.
//!
//! | Argument | Read as | Written to |
//! |---|---|---|
//! | 0 | the visibility struct `V` | |
//! | 1 | the view kind | `V+0x2a78` |
//! | 2 | the frusta, `0x1bc` bytes each | `[V]+4`, unless it is that; with argument 7, its `+0x74` vec4 at `V+0x2a88` |
//! | 3 | the start cluster (`i32`; -1 locates the centre, `0x0011bed0`) | its low word at `V+8` |
//! | 4 | the source view (0 for none) | `V+0x2ad0`; nonzero sets flag 1 and zeroes `V+0x2a98..+0x2acc` |
//! | 5 | the sphere centre (vec3) | `V+0x2a68` |
//! | 6 | the sphere radius | `V+0x2a74` |
//! | 7 | the clip plane byte | flag `0x200` |
//! | 8 | the clip plane offset | `V+0x2a84`, and added to `V+0x2a94` |
//!
//! ECX goes to `V+0x2a60`, BX to `V+0x2a64` and the count to `[[V]]`. The
//! mode (`V+0x2a7c`, and EAX) is 2 for kind 0 under flag `0x80`; 1 with no
//! frusta, or a radius under `[0x0045dbd4]` and no flag `0x100`; else 0.
//!
//! The reset clears the counts (`+4`, `u16`) of the four lists at
//! `[V+0xc]`..`[V+0x18]`, the 64-byte cluster bit row at `V+0x21c`, the
//! `[V+0xa5c]` words at `V+0x25c`, the two counts at `V+0xa5c` and
//! `V+0xa5e`, and fills `V+0x1c..+0x21c` with `0xff`.
//!
//! The port refuses an MXCSR other than 0x1f80 (flags aside), a NaN in
//! the clip plane sum (its payload is x86's, not the host's), more than
//! six frusta, frusta that overlap their destination without being it, a
//! reset count over 512, a BSP walk past 4096 nodes, and memory that does
//! not read.

use guest_layout::memory::{GuestMemory, MemFault};
use crate::shadow_arm::{Proposal, Shadowed};

/// The image's words and the guest's sites the port names.
pub mod guest {
    /// The setup itself.
    pub const SETUP: u32 = 0x0013_2b80;
    /// The setup's return in the gather: its one caller.
    pub const SETUP_RETURN: u32 = 0x0013_212e;
    /// The reset's return in the gather: where an answer resumes.
    pub const RESET_RETURN: u32 = 0x0013_213d;
    /// The active visibility struct, which the gather stores.
    pub const ACTIVE_VIS: u32 = 0x004c_0b80;
    /// The active structure BSP's index (`0xffff`: none).
    pub const BSP_INDEX: u32 = 0x0046_86c4;
    /// The collision BSP the location walks (`0x0014a280`).
    pub const COLLISION_BSP: u32 = 0x004e_033c;
    /// The active structure, whose leaf table (`+0x30`) names each leaf's
    /// cluster.
    pub const STRUCTURE: u32 = 0x004e_0348;
    /// The radius under which the sphere mode may be chosen.
    pub const MODE_RADIUS: u32 = 0x0045_dbd4;
    /// One frustum's size.
    pub const FRUSTUM: u32 = 0x1bc;
}

/// The setup's frustum count limit: the six faces of a cube light, which
/// is all `[V]+4..+0xa6c` holds.
pub const MAX_FRUSTA: u32 = 6;
/// The reset's word count limit: `V+0x25c..+0xa5c`.
pub const MAX_RESET_WORDS: u32 = 0x200;
/// A walk deeper than this is a broken tree, not a guest's.
const MAX_WALK: usize = 4096;

/// Offsets in the visibility struct.
pub mod vis {
    pub const LOCATION: u32 = 0x4;
    pub const START: u32 = 0x8;
    pub const BSP: u32 = 0xa;
    pub const LISTS: [u32; 4] = [0xc, 0x10, 0x14, 0x18];
    pub const VISITED: u32 = 0x1c;
    pub const CLUSTER_BITS: u32 = 0x21c;
    pub const WORDS: u32 = 0x25c;
    pub const WORD_COUNT: u32 = 0xa5c;
    pub const WORD_COUNT_2: u32 = 0xa5e;
    pub const FLAGS: u32 = 0x2a60;
    pub const FRUSTUM_COUNT: u32 = 0x2a64;
    pub const CENTRE: u32 = 0x2a68;
    pub const RADIUS: u32 = 0x2a74;
    pub const KIND: u32 = 0x2a78;
    pub const MODE: u32 = 0x2a7c;
    pub const CLIP_OFFSET: u32 = 0x2a84;
    pub const CLIP_PLANE: u32 = 0x2a88;
    pub const CLIP_D: u32 = 0x2a94;
    pub const OVERLAPS: u32 = 0x2a98;
    pub const OVERLAPS_LEN: u32 = 13 * 4;
    pub const SOURCE: u32 = 0x2ad0;
}

/// Why the port declines; the guest runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// MXCSR is not the state the port computes under.
    FloatState,
    /// A read faults, or a write would not land.
    Unread,
    /// More frusta or reset words than the struct holds, or frusta that
    /// overlap their destination.
    Layout,
    /// The clip plane sum would be a NaN.
    Nan,
    /// The location's BSP walk does not reach a leaf.
    Walk,
}

impl Refusal {
    pub fn name(self) -> &'static str {
        match self {
            Self::FloatState => "float_state",
            Self::Unread => "unread",
            Self::Layout => "layout",
            Self::Nan => "nan",
            Self::Walk => "walk",
        }
    }
}

impl From<MemFault> for Refusal {
    fn from(_: MemFault) -> Self {
        Self::Unread
    }
}

/// The setup's inputs: its two registers and nine stack words.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Call {
    pub flags: u32,
    pub count: u32,
    pub vis: u32,
    pub kind: u32,
    pub frusta: u32,
    pub start: u32,
    pub source: u32,
    pub centre: u32,
    pub radius: u32,
    pub clip: u8,
    pub clip_offset: u32,
}

impl Call {
    /// The call at entry: `esp` points at the return address.
    pub fn read(m: &dyn GuestMemory, esp: u32, ecx: u32, ebx: u32) -> Result<Self, Refusal> {
        let arg = |k: u32| m.u32(esp.wrapping_add(4 + 4 * k));
        Ok(Self {
            flags: ecx,
            count: ebx,
            vis: arg(0)?,
            kind: arg(1)?,
            frusta: arg(2)?,
            start: arg(3)?,
            source: arg(4)?,
            centre: arg(5)?,
            radius: arg(6)?,
            clip: arg(7)? as u8,
            clip_offset: arg(8)?,
        })
    }
}

/// What the setup, the glue and the reset leave, and the registers at
/// the reset's return.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Built {
    pub proposal: Proposal,
    /// EAX from the setup; EDI at the reset's return.
    pub mode: u32,
    /// The setup's XMM1 when it summed the clip plane.
    pub xmm1: Option<u32>,
}

/// `0x00132b80`, the glue and `0x001348e0` for `call`.
pub fn build(m: &dyn GuestMemory, call: &Call, mxcsr: u32) -> Result<Built, Refusal> {
    if mxcsr & !0x3f != 0x1f80 {
        return Err(Refusal::FloatState);
    }
    if call.count > MAX_FRUSTA {
        return Err(Refusal::Layout);
    }
    let v = call.vis;
    let at = |o: u32| v.checked_add(o).ok_or(Refusal::Layout);
    let mut s = Shadowed::new(m);

    // The view's fields.
    s.put_u16("start", at(vis::START)?, call.start as u16)?;
    let bsp_index = s.u16(guest::BSP_INDEX)?;
    s.put_u16("bsp", at(vis::BSP)?, bsp_index)?;
    s.put_u32("location", at(vis::LOCATION)?, u32::MAX)?;
    s.put_u32("kind", at(vis::KIND)?, call.kind)?;
    s.put_u32("flags", at(vis::FLAGS)?, call.flags)?;
    s.put_u16("frustum_count", at(vis::FRUSTUM_COUNT)?, call.count as u16)?;
    let centre = s.bytes(call.centre, 12)?;
    s.put("centre", at(vis::CENTRE)?, &centre)?;
    s.put_u32("radius", at(vis::RADIUS)?, call.radius)?;
    s.put_u32("clip_offset", at(vis::CLIP_OFFSET)?, call.clip_offset)?;

    // The clip plane.
    let mut xmm1 = None;
    if call.clip != 0 {
        s.put_u32("flags", at(vis::FLAGS)?, call.flags | 0x200)?;
        let plane = s.bytes(call.frusta.checked_add(0x74).ok_or(Refusal::Layout)?, 16)?;
        s.put("clip_plane", at(vis::CLIP_PLANE)?, &plane)?;
        let d = s.f32(at(vis::CLIP_D)?)?;
        let offset = f32::from_bits(call.clip_offset);
        let sum = d + offset;
        if d.is_nan() || offset.is_nan() || sum.is_nan() {
            return Err(Refusal::Nan);
        }
        s.put_u32("clip_plane", at(vis::CLIP_D)?, sum.to_bits())?;
        xmm1 = Some(sum.to_bits());
    } else {
        s.put_u32("flags", at(vis::FLAGS)?, call.flags & !0x200)?;
    }

    // The frusta, then their count at the region buffer's head.
    let buffer = s.u32(v)?;
    let dest = buffer.checked_add(4).ok_or(Refusal::Layout)?;
    if call.frusta != dest {
        let len = call.count * guest::FRUSTUM;
        if len != 0 {
            let (a, b) = (u64::from(call.frusta), u64::from(dest));
            if a < b + u64::from(len) && b < a + u64::from(len) {
                return Err(Refusal::Layout);
            }
            let bytes = s.bytes(call.frusta, len as usize)?;
            s.put("frusta", dest, &bytes)?;
        }
    }
    let buffer = s.u32(v)?;
    s.put_u16("frustum_count", buffer, call.count as u16)?;

    // The location, `0x0011bed0`, from the centre.
    if call.start == u32::MAX {
        let [x, y, z] = [0, 4, 8].map(|o| s.f32(call.centre.wrapping_add(o)));
        let point = [x?, y?, z?];
        let (leaf, cluster) = locate(&s, point)?;
        s.put_u32("location", at(vis::LOCATION)?, leaf)?;
        s.put_u16("start", at(vis::START)?, cluster)?;
        s.put_u16("bsp", at(vis::BSP)?, bsp_index)?;
    }

    // The source view.
    s.put_u32("source", at(vis::SOURCE)?, call.source)?;
    if call.source != 0 {
        let flags = s.u32(at(vis::FLAGS)?)?;
        s.put_u32("flags", at(vis::FLAGS)?, flags | 1)?;
        s.put("overlaps", at(vis::OVERLAPS)?, &[0; vis::OVERLAPS_LEN as usize])?;
    }

    // The mode.
    let flags = s.u32(at(vis::FLAGS)?)?;
    let mode_radius = s.f32(guest::MODE_RADIUS)?;
    let mode = mode(call.kind, flags, call.count, f32::from_bits(call.radius), mode_radius);
    s.put_u32("mode", at(vis::MODE)?, mode)?;

    // The glue: the active struct.
    s.put_u32("active", guest::ACTIVE_VIS, v)?;

    // The reset, `0x001348e0`.
    for list in vis::LISTS {
        let header = s.u32(at(list)?)?;
        s.put_u16("list_counts", header.checked_add(4).ok_or(Refusal::Layout)?, 0)?;
    }
    s.put("cluster_bits", at(vis::CLUSTER_BITS)?, &[0; 0x40])?;
    let words = u32::from(s.u16(at(vis::WORD_COUNT)?)?);
    if words > MAX_RESET_WORDS {
        return Err(Refusal::Layout);
    }
    if words != 0 {
        s.put("words", at(vis::WORDS)?, &vec![0; 4 * words as usize])?;
    }
    s.put_u16("word_counts", at(vis::WORD_COUNT)?, 0)?;
    s.put_u16("word_counts", at(vis::WORD_COUNT_2)?, 0)?;
    s.put("visited", at(vis::VISITED)?, &[0xff; 0x200])?;

    Ok(Built { proposal: s.proposal, mode, xmm1 })
}

/// The setup's choice of mode. The comparison is `comiss`: a NaN radius
/// is not under the threshold.
pub fn mode(kind: u32, flags: u32, count: u32, radius: f32, mode_radius: f32) -> u32 {
    if kind == 0 && flags & 0x80 != 0 {
        2
    } else if count == 0 || (mode_radius > radius && flags & 0x100 == 0) {
        1
    } else {
        0
    }
}

/// `0x0011bed0` over `0x0014a280`: the leaf (or -1) and the cluster
/// (or `0xffff`) holding `p`. No structure (`[0x004686c4]` is `0xffff`)
/// gives -1 and `0xffff`.
///
/// The walk from node 0 reads node `i`'s plane (`i16` at `+0`, 16-byte
/// planes), takes the front child (the 24 bits at `+5`) when
/// `((nz·z + ny·y) + nx·x) − d` is not below zero and the back child
/// (`+2`) otherwise, a NaN included, and stops at a child with bit 23
/// set: -1 is solid, else its low 23 bits are the leaf. A leaf's cluster
/// is the `i16` at `[[0x004e0348] + 0x30] + 8·leaf`.
pub fn locate(m: &dyn GuestMemory, p: [f32; 3]) -> Result<(u32, u16), Refusal> {
    if m.u16(guest::BSP_INDEX)? == u16::MAX {
        return Ok((u32::MAX, u16::MAX));
    }
    let bsp = m.u32(guest::COLLISION_BSP)?;
    let nodes = m.u32(bsp.wrapping_add(4))?;
    let planes = m.u32(bsp.wrapping_add(0xc))?;
    let mut child = 0u32;
    for _ in 0..MAX_WALK {
        let node = nodes.wrapping_add(8 * (child & 0x7f_ffff));
        let plane = m.u16(node)? as i16 as i32;
        let plane = planes.wrapping_add((plane << 4) as u32);
        let [nx, ny, nz, d] = [0, 4, 8, 12].map(|o| m.f32(plane.wrapping_add(o)));
        let (nx, ny, nz, d) = (nx?, ny?, nz?, d?);
        let distance = ((nz * p[2] + ny * p[1]) + nx * p[0]) - d;
        let side = if distance >= 0.0 { 4 } else { 1 };
        child = ((m.u32(node.wrapping_add(side))? as i32) >> 8) as u32;
        if child & 0x80_0000 == 0 {
            continue;
        }
        if child == u32::MAX {
            return Ok((u32::MAX, u16::MAX));
        }
        let leaf = child & 0x7f_ffff;
        let structure = m.u32(guest::STRUCTURE)?;
        let leaves = m.u32(structure.wrapping_add(0x30))?;
        let cluster = m.u16(leaves.wrapping_add(8 * leaf))?;
        return Ok((leaf, cluster));
    }
    Err(Refusal::Walk)
}

/// The arm over the setup: its state and the shadow of the sampled call
/// in flight live in the bridge; this is the arm and its switch.
pub mod arm {
    use std::sync::Mutex;

    use crate::shadow_arm::{ByteArm, Plan};

    struct State {
        arm: ByteArm,
        on: bool,
    }

    static STATE: Mutex<State> = Mutex::new(State { arm: ByteArm::new(), on: true });

    fn with<R>(f: impl FnOnce(&mut ByteArm, bool) -> R) -> R {
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let on = s.on;
        f(&mut s.arm, on)
    }

    /// `impl.render.native.visibility`: off, the arm compares and never
    /// answers.
    pub fn set_on(on: bool) {
        STATE.lock().unwrap_or_else(|e| e.into_inner()).on = on;
    }

    /// What to do with a call in `frame`; counts an unsampled guest call.
    pub fn plan(frame: u32) -> Plan {
        with(|a, on| {
            let plan = a.shadow.plan(frame, on);
            if plan == Plan::Guest {
                a.shadow.unsampled(Plan::Guest);
            }
            plan
        })
    }

    /// Rust answered an unsampled call.
    pub fn answered() {
        with(|a, _| {
            a.shadow.unsampled(Plan::Answer);
        });
    }

    /// The guest ran a sampled call: `differing` names the fields whose
    /// bytes (or the mode register) differ.
    pub fn compared(differing: Vec<&'static str>, frame: u32, vis: u32) {
        with(|a, on| {
            a.record(differing, on, || format!("frame {frame} vis {vis:#010x}"));
        });
    }

    pub fn refuse(why: &'static str) {
        with(|a, _| a.refuse(why));
    }

    pub fn new_map() {
        with(|a, _| a.new_map());
    }

    pub fn proven() -> bool {
        with(|a, _| a.shadow.proven())
    }

    /// `render.scene`'s counters (`scene_visibility_setup_*`).
    pub fn counters() -> Vec<(&'static str, u64)> {
        with(|a, _| a.counters("scene_visibility_setup"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const V: u32 = 0x0010_0000;
    const BUFFER: u32 = 0x0020_0000;
    const FRUSTA: u32 = 0x0030_0000;
    const CENTRE: u32 = 0x0031_0000;
    const LIST: u32 = 0x0032_0000;
    const BSP: u32 = 0x0040_0000;
    const NODES: u32 = 0x0041_0000;
    const PLANES: u32 = 0x0042_0000;
    const STRUCT: u32 = 0x0043_0000;
    const LEAVES: u32 = 0x0044_0000;

    /// A struct, its region buffer, two frusta, a centre at x = 2, and a
    /// one-node BSP split at x = 1: front leaf 5 (cluster 9), back solid.
    fn fixture() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.map(V, 0x2b00).map(BUFFER, 0xa70).map(FRUSTA, 2 * guest::FRUSTUM).map(CENTRE, 12);
        m.map(LIST, 0x40).map(0x0046_0000, 0x10000).map(0x004c_0000, 0x10000).map(0x004e_0000, 0x1000);
        m.map(BSP, 0x10).map(NODES, 8).map(PLANES, 16).map(STRUCT, 0x40).map(LEAVES, 0x40);
        m.put_u32(V, BUFFER);
        for (k, list) in vis::LISTS.into_iter().enumerate() {
            m.put_u32(V + list, LIST + 0x10 * k as u32).put_u16(LIST + 0x10 * k as u32 + 4, 7);
        }
        m.put_u16(V + vis::WORD_COUNT, 3).put_u32(V + vis::WORDS, 0xdead).put_u16(V + vis::WORD_COUNT_2, 2);
        for i in 0..2 * guest::FRUSTUM {
            m.put_u8(FRUSTA + i, i as u8);
        }
        m.put_f32(FRUSTA + 0x80, 1.5);
        m.put_f32(CENTRE, 2.0).put_f32(CENTRE + 4, 0.0).put_f32(CENTRE + 8, 0.0);
        m.put_f32(guest::MODE_RADIUS, 4.0).put_u16(guest::BSP_INDEX, 0);
        m.put_u32(guest::COLLISION_BSP, BSP).put_u32(BSP + 4, NODES).put_u32(BSP + 0xc, PLANES);
        // Node 0: plane 0; back child (bytes 2..5) solid, front (5..8) leaf 5.
        m.put(NODES, &[0, 0, 0xff, 0xff, 0xff, 5, 0, 0x80]);
        m.put_f32(PLANES, 1.0).put_f32(PLANES + 12, 1.0);
        m.put_u32(guest::STRUCTURE, STRUCT).put_u32(STRUCT + 0x30, LEAVES).put_u16(LEAVES + 8 * 5, 9);
        m
    }

    fn call() -> Call {
        Call {
            flags: 0x10,
            count: 2,
            vis: V,
            kind: 0,
            frusta: FRUSTA,
            start: u32::MAX,
            source: 0,
            centre: CENTRE,
            radius: 8f32.to_bits(),
            clip: 0,
            clip_offset: 0.25f32.to_bits(),
        }
    }

    fn after(m: &FakeGuest, built: &Built) -> FakeGuest {
        let mut out = m.clone();
        built.proposal.apply(&mut out).unwrap();
        out
    }

    #[test]
    fn the_setup_writes_the_view_copies_the_frusta_locates_and_resets() {
        let m = fixture();
        let built = build(&m, &call(), 0x1f80).unwrap();
        assert_eq!(built.mode, 0, "two frusta and a radius over the threshold");
        let g = after(&m, &built);
        assert_eq!(g.u32(V + vis::LOCATION).unwrap(), 5);
        assert_eq!(g.u16(V + vis::START).unwrap(), 9);
        assert_eq!(g.u16(V + vis::BSP).unwrap(), 0);
        assert_eq!(g.u32(V + vis::FLAGS).unwrap(), 0x10);
        assert_eq!(g.u16(V + vis::FRUSTUM_COUNT).unwrap(), 2);
        assert_eq!(g.f32(V + vis::CENTRE).unwrap(), 2.0);
        assert_eq!(g.u32(V + vis::MODE).unwrap(), 0);
        assert_eq!(g.u16(BUFFER).unwrap(), 2);
        assert_eq!(g.bytes(BUFFER + 4, 2 * guest::FRUSTUM as usize).unwrap(), m.bytes(FRUSTA, 2 * guest::FRUSTUM as usize).unwrap());
        assert_eq!(g.u32(guest::ACTIVE_VIS).unwrap(), V);
        for (k, _) in vis::LISTS.iter().enumerate() {
            assert_eq!(g.u16(LIST + 0x10 * k as u32 + 4).unwrap(), 0);
        }
        assert_eq!(g.u32(V + vis::WORDS).unwrap(), 0);
        assert_eq!(g.u16(V + vis::WORD_COUNT).unwrap(), 0);
        assert_eq!(g.u16(V + vis::WORD_COUNT_2).unwrap(), 0);
        assert_eq!(g.u32(V + vis::VISITED).unwrap(), u32::MAX);
        assert_eq!(g.u32(V + vis::VISITED + 0x1fc).unwrap(), u32::MAX);
        assert_eq!(g.u32(V + vis::CLUSTER_BITS).unwrap(), 0);
        assert!(built.proposal.differing(&g).is_empty());
    }

    #[test]
    fn a_given_start_is_kept_and_not_located() {
        let m = fixture();
        let built = build(&m, &Call { start: 3, ..call() }, 0x1f80).unwrap();
        let g = after(&m, &built);
        assert_eq!(g.u16(V + vis::START).unwrap(), 3);
        assert_eq!(g.u32(V + vis::LOCATION).unwrap(), u32::MAX);
    }

    #[test]
    fn the_back_side_is_solid_and_no_structure_locates_nothing() {
        let mut m = fixture();
        assert_eq!(locate(&m, [0.0, 0.0, 0.0]).unwrap(), (u32::MAX, u16::MAX));
        assert_eq!(locate(&m, [f32::NAN, 0.0, 0.0]).unwrap(), (u32::MAX, u16::MAX), "a NaN takes the back child");
        assert_eq!(locate(&m, [1.0, 0.0, 0.0]).unwrap(), (5, 9), "zero distance is the front");
        m.put_u16(guest::BSP_INDEX, 0xffff);
        assert_eq!(locate(&m, [2.0, 0.0, 0.0]).unwrap(), (u32::MAX, u16::MAX));
    }

    #[test]
    fn the_clip_plane_sets_its_flag_and_adds_the_offset() {
        let m = fixture();
        let built = build(&m, &Call { clip: 1, ..call() }, 0x1f80).unwrap();
        let g = after(&m, &built);
        assert_eq!(g.u32(V + vis::FLAGS).unwrap(), 0x210);
        assert_eq!(g.bytes(V + vis::CLIP_PLANE, 12).unwrap(), m.bytes(FRUSTA + 0x74, 12).unwrap());
        assert_eq!(g.f32(V + vis::CLIP_D).unwrap(), 1.75);
        assert_eq!(built.xmm1, Some(1.75f32.to_bits()));
        let flags = build(&m, &Call { flags: 0x210, ..call() }, 0x1f80).unwrap();
        assert_eq!(after(&m, &flags).u32(V + vis::FLAGS).unwrap(), 0x10, "no clip clears 0x200");
    }

    #[test]
    fn a_source_view_sets_flag_one_and_clears_the_overlaps() {
        let mut m = fixture();
        m.put_u32(V + vis::OVERLAPS + 0x30, 0x1234);
        let built = build(&m, &Call { source: 0x0054_7f88, ..call() }, 0x1f80).unwrap();
        let g = after(&m, &built);
        assert_eq!(g.u32(V + vis::FLAGS).unwrap(), 0x11);
        assert_eq!(g.u32(V + vis::SOURCE).unwrap(), 0x0054_7f88);
        assert_eq!(g.u32(V + vis::OVERLAPS + 0x30).unwrap(), 0);
    }

    #[test]
    fn the_mode_follows_kind_flags_count_and_radius() {
        assert_eq!(mode(0, 0x80, 2, 8.0, 4.0), 2);
        assert_eq!(mode(1, 0x80, 2, 8.0, 4.0), 0, "only kind 0 takes the visible set");
        assert_eq!(mode(1, 0, 0, 8.0, 4.0), 1, "no frusta");
        assert_eq!(mode(1, 0, 2, 1.0, 4.0), 1, "a small sphere");
        assert_eq!(mode(1, 0x100, 2, 1.0, 4.0), 0, "flag 0x100 keeps the frusta");
        assert_eq!(mode(1, 0, 2, f32::NAN, 4.0), 0, "a NaN radius is not under it");
    }

    #[test]
    fn frusta_already_in_place_are_not_copied() {
        let m = fixture();
        let built = build(&m, &Call { frusta: BUFFER + 4, ..call() }, 0x1f80).unwrap();
        assert!(built.proposal.fields.iter().all(|f| f.name != "frusta"));
    }

    #[test]
    fn the_port_names_what_it_refuses() {
        let m = fixture();
        assert_eq!(build(&m, &call(), 0x9f80).unwrap_err(), Refusal::FloatState);
        assert_eq!(build(&m, &Call { count: 7, ..call() }, 0x1f80).unwrap_err(), Refusal::Layout);
        assert_eq!(build(&m, &Call { frusta: BUFFER + 8, count: 1, ..call() }, 0x1f80).unwrap_err(), Refusal::Layout);
        assert_eq!(
            build(&m, &Call { clip: 1, clip_offset: f32::NAN.to_bits(), ..call() }, 0x1f80).unwrap_err(),
            Refusal::Nan
        );
        assert_eq!(build(&m, &Call { centre: 0x0900_0000, ..call() }, 0x1f80).unwrap_err(), Refusal::Unread);
        let mut big = fixture();
        big.put_u16(V + vis::WORD_COUNT, 0x201);
        assert_eq!(build(&big, &call(), 0x1f80).unwrap_err(), Refusal::Layout);
    }

    #[test]
    fn the_call_reads_its_nine_arguments_after_the_return_address() {
        let mut m = FakeGuest::new();
        m.map(0x1000, 0x40);
        for k in 0..9 {
            m.put_u32(0x1004 + 4 * k, 0x100 + k);
        }
        let c = Call::read(&m, 0x1000, 0xf1, 2).unwrap();
        assert_eq!((c.flags, c.count, c.vis, c.kind, c.frusta, c.start), (0xf1, 2, 0x100, 0x101, 0x102, 0x103));
        assert_eq!((c.source, c.centre, c.radius, c.clip, c.clip_offset), (0x104, 0x105, 0x106, 0x07, 0x108));
    }
}
