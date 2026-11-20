//! Main-camera structure record publication. The shared streaming routine
//! 0x12de70 stamps a resident resource. A cold resource can initiate IO;
//! defer the entire call to the guest before publishing anything in that case.

use crate::light_constants::constant::{HALF, ONE, QUARTER, TWO};
use crate::memory::GuestMemory;
use crate::points::PIXEL_SCALE;
use crate::render_loop::occlusion::guest::{MAIN_VIS, VIS_KIND};
use x87::{F80, Mode};

pub const CLUSTERS_RETURN: u32 = 0x0003_f3be;
pub const PLACEMENTS_RETURN: u32 = 0x0003_f3c4;
const CURRENT_VIS: u32 = 0x004c_0b80;
const ACTIVE_BSP: u32 = 0x004e_0348;
pub const LIGHTMAP: u32 = 0x004e_0344;
const RESOURCE_CACHE: u32 = 0x004e_649c;
const RECORDS: u32 = 0x004b_a138;
const COUNT: u32 = 0x004c_0b78;
const MAIN_COUNT: u32 = 0x004c_0b7c;
const PIXEL_SIZES: u32 = 0x004c_0b84;
const ATTACHMENTS: u32 = 0x004c_0ed6;
const SUBPART_POOL: u32 = 0x25c;
const SUBPART_WORDS_USED: u32 = 0xa5c;
const PROJECTION_DEPTH: u32 = 0x004b_9e20;
const MIN_DEPTH: u32 = 0x0045_dc68;
const SMALL_RADIUS: u32 = 0x0045_dd08;
const SMALL_SCALE: u32 = 0x0045_e18c;
const FADE_START: u32 = 0x0045_e0d8;
const FADE_SLOPE: u32 = 0x0043_fde4;
const FADE_RANGE: u32 = 0x0045_dc38;
const SCENE_MODE: u32 = 0x004b_9ef0;
const VIEW_FOG_TAG: u32 = 0x004b_9f8c;
const VIEW_FOG_AMOUNT: u32 = 0x004b_9ffc;
const PLANAR_FOG_ACTIVE: u32 = 0x004b_a004;
const ZERO_FLOAT: u32 = 0x0045_dbd8;
const CLUSTER_LIST: u32 = 0xc;
const PLACEMENT_LIST: u32 = 0x18;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Walk { Clusters, Placements }

// All reads and address calculations finish before writes. Each edit keeps
// its own preimage (including repeated cache stamps), so a failed write
// can restore every earlier edit in reverse order.
struct Edit { at: u32, len: usize, old: [u8; 32], new: [u8; 32] }
struct Plan { edits: Vec<Edit>, sections: u32 }

impl Plan {
    fn put(&mut self, m: &dyn GuestMemory, at: u32, new: &[u8]) -> Option<()> {
        let len = new.len();
        let mut edit = Edit { at, len, old: [0; 32], new: [0; 32] };
        m.read(at, &mut edit.old[..len]).ok()?;
        edit.new[..len].copy_from_slice(new);
        self.edits.push(edit);
        Some(())
    }

    fn commit(self, m: &mut dyn GuestMemory) -> Option<u32> {
        for (i, e) in self.edits.iter().enumerate() {
            if m.write(e.at, &e.new[..e.len]).is_err() {
                for e in self.edits[..i].iter().rev() {
                    m.write(e.at, &e.old[..e.len]).expect("restore structure walk before fallback");
                }
                return None;
            }
        }
        Some(self.sections)
    }

    /// The resident arm of the resource touch. The nonresident arm mutates
    /// the request queue and can run callbacks inside the walk.
    fn touch(&mut self, m: &dyn GuestMemory, resource: u32) -> Option<()> {
        if m.u8(resource.checked_add(0x1e)?).ok()? == 0 { return None; }
        let index = u32::from(m.u16(resource.checked_add(0x20)?).ok()?);
        let cache = ptr(m, RESOURCE_CACHE)?;
        let pool = ptr(m, cache.checked_add(0x64)?)?;
        let entries = ptr(m, pool.checked_add(0x44)?)?;
        let frame = m.u32(cache.checked_add(0x38)?).ok()?;
        self.put(m, entries.checked_add(index * 0x18 + 0x14)?, &frame.to_le_bytes())
    }

    /// BSP arm of the record writer's lighting selection (0x2d000).
    fn lighting(&mut self, m: &dyn GuestMemory, bsp: u32, placement: Option<(u32, u32)>) -> Option<u32> {
        let lm = m.u32(LIGHTMAP).ok()?;
        if lm == 0 || m.i32(lm.checked_add(0x80)?).ok()? <= 0 { return Some(3); }
        let group = ptr(m, lm.checked_add(0x84)?)?;
        if m.u32(group.checked_add(0x1c)?).ok()? == u32::MAX ||
            m.u32(group.checked_add(4)?).ok()? != m.u32(bsp.checked_add(8)?).ok()? {
            return Some(3);
        }
        let Some((id, at)) = placement else { return Some(0) };
        let infos = ptr(m, group.checked_add(0x54)?)?;
        let section = u32::from(m.u16(infos.checked_add(id * 12 + 2)?).ok()?);
        let sections = ptr(m, group.checked_add(0x44)?)?;
        self.touch(m, sections.checked_add(section * 0x38 + 0xc)?)?;
        Some(u32::from(m.u16(at.checked_add(0x56)?).ok()? != 0))
    }
}

fn ptr(m: &dyn GuestMemory, at: u32) -> Option<u32> {
    let p = m.u32(at).ok()?;
    (p != 0).then_some(p)
}

// The x87 sphere-radius block rounds each operation at the guest's precision.
fn cluster_sphere(m: &dyn GuestMemory, section: u32, mode: Mode) -> Option<([f32; 3], f32)> {
    let half = m.f32(HALF).ok()?;
    let mut centre = [0.; 3];
    let mut squares = [F80::ZERO; 3];
    for i in 0..3 {
        let at = section.checked_add(0x54 + i as u32 * 8)?;
        let lo = m.f32(at).ok()?;
        let hi = m.f32(at.checked_add(4)?).ok()?;
        centre[i] = (lo + hi) * half;
        let v = F80::from_f32_bits((hi - centre[i]).to_bits());
        squares[i] = x87::mul(v, v, mode).0;
    }
    let sum = x87::add(x87::add(squares[0], squares[1], mode).0, squares[2], mode).0;
    let radius = f32::from_bits(x87::sqrt(sum, mode).0.to_f32_bits(mode).0);
    Some((centre, radius))
}

fn plan(m: &dyn GuestMemory, vis: u32, walk: Walk, mode: Mode, only: Option<u32>) -> Option<Plan> {
    if vis != MAIN_VIS || m.u32(CURRENT_VIS).ok()? != vis || m.u32(vis + VIS_KIND).ok()? != 0
        { return None; }
    let fog_view = m.u32(SCENE_MODE).ok()? == 3;
    let bsp = ptr(m, ACTIVE_BSP)?;
    let list = ptr(m, vis + if walk == Walk::Clusters { CLUSTER_LIST } else { PLACEMENT_LIST })?;
    let count = m.u16(list.checked_add(4)?).ok()? as i16;
    let mut plan = Plan { edits: Vec::new(), sections: 0 };
    if count <= 0 { return only.is_none().then_some(plan); }
    let count = count as u32;
    if count > m.u32(list).ok()? { return None; }
    let ids = ptr(m, list.checked_add(0xc)?)?;
    let flags = ptr(m, list.checked_add(8)?)?;
    let mut cursor = m.u32(COUNT).ok()?;
    let mut main = m.u32(MAIN_COUNT).ok()?;
    let mut used = m.u16(vis + SUBPART_WORDS_USED).ok()?;
    let depth = [
        m.f32(PROJECTION_DEPTH).ok()?, m.f32(PROJECTION_DEPTH + 12).ok()?,
        m.f32(PROJECTION_DEPTH + 24).ok()?, m.f32(PROJECTION_DEPTH + 36).ok()?,
    ];
    let scale = m.f32(PIXEL_SCALE).ok()?;
    let near = m.f32(MIN_DEPTH).ok()?;
    let double = m.f32(TWO).ok()?;
    let quantize = m.f32(QUARTER).ok()?;
    let vis_flag = u32::from(m.u8(vis + 0x2a60).ok()? & 0x80 != 0);
    let range = match only {
        None => 0..count,
        Some(n) if n < count => n..n+1,
        Some(_) => return None,
    };
    for n in range {
        let flag_at = flags.checked_add(2 * n)?;
        let f = u32::from(m.u16(flag_at).ok()?);
        let mut bits = (((f >> 2) & 0x2c00 | f & 0x4000) >> 9) | (f & 0x200) << 5 | vis_flag;
        if fog_view {
            let fog = m.u32(VIEW_FOG_TAG).ok()?;
            if f & 0x400 == 0 && fog != u32::MAX && m.f32(VIEW_FOG_AMOUNT).ok()? == m.f32(ZERO_FLOAT).ok()? {
                let tags = ptr(m, crate::tags::TAG_ENTRIES_GLOBAL)?;
                let tag = ptr(m, tags.checked_add((fog & 0xffff)*16 + 8)?)?;
                if m.u8(tag).ok()? & 2 == 0 { bits |= 1 << 8; }
            }
            if m.u8(PLANAR_FOG_ACTIVE).ok()? == 0 || f & 0x400 != 0 { bits |= 1 << 7; }
        }
        let id_at = ids.checked_add(4 * n)?;
        let (id, section, extra, placement, size, alpha) = match walk {
            Walk::Clusters => {
                let id = m.u32(id_at).ok()?;
                if id >= m.u32(bsp.checked_add(0x9c)?).ok()? || id >= 512 { return None; }
                let section = ptr(m, bsp.checked_add(0xa0)?)?.checked_add(id * 0xb0)?;
                let (p, r) = cluster_sphere(m, section, mode)?;
                let z = ((depth[2]*p[2] + depth[0]*p[0]) + depth[1]*p[1]) + depth[3];
                let size = (scale / z.abs().max(near)) * r * double;
                bits |= 1 << 6;
                let extra = m.u8(vis + 0x1c + id).ok()? as i8 as i16 as u16;
                (id, section, extra, None, size, u8::MAX)
            }
            Walk::Placements => {
                let id = u32::from(m.u16(id_at).ok()?);
                if id >= m.u32(bsp.checked_add(0x140)?).ok()? || id >= 0x7ff { return None; }
                let at = ptr(m, bsp.checked_add(0x144)?)?.checked_add(id * 0x58)?;
                let definition = u32::from(m.u16(at.checked_add(0x34)?).ok()?);
                if definition >= m.u32(bsp.checked_add(0x138)?).ok()? { return None; }
                let section = ptr(m, bsp.checked_add(0x13c)?)?.checked_add(definition.checked_mul(0xc8)?)?;
                let p = m.vec3(at.checked_add(0x3c)?).ok()?;
                let r = m.f32(at.checked_add(0x48)?).ok()?;
                let z = ((depth[1]*p[1] + depth[2]*p[2]) + depth[0]*p[0]) + depth[3];
                let mut size = (r / z.abs().max(near)) * scale * double;
                if r < m.f32(SMALL_RADIUS).ok()? { size *= m.f32(SMALL_SCALE).ok()?; }
                let fade = (size - m.f32(FADE_START).ok()?) * m.f32(FADE_SLOPE).ok()?;
                let fade = fade.max(0.).min(m.f32(ONE).ok()?);
                let alpha = x87::mul(F80::from_f32_bits(fade.to_bits()),
                    F80::from_f32_bits(m.f32(FADE_RANGE).ok()?.to_bits()), mode).0;
                let alpha = u8::try_from(alpha.to_i32(mode, true).0).ok()?;
                if alpha == 0 {
                    plan.put(m, flag_at, &[(f as u8) | 1])?;
                    continue;
                }
                bits |= (1 << 12) | 8 | if f & (1 << 11) == 0 { 1 << 6 } else { 1 };
                let extras = ptr(m, list.checked_add(0x14)?)?;
                let extra = m.u16(extras.checked_add(2*n)?).ok()?;
                (id, section, extra, Some((id, at)), size, alpha)
            }
        };
        if !size.is_finite() { return None; }
        if cursor >= 0x352 { continue; } // 0x2cbf0 discards the increment
        let lighting = plan.lighting(m, bsp, placement)?;
        plan.touch(m, section.checked_add(0x28)?)?;
        ptr(m, section.checked_add(0x50)?)?;
        let row = RECORDS + cursor * 0x20;
        let mut data = [0u8; 0x20];
        m.read(row, &mut data).ok()?;
        let packed = if placement.is_some() { id << 18 } else { (0x7ff << 18) | id << 9 };
        let geometry = u32::from(m.u16(section.checked_add(0x14)?).ok()?);
        let packed = geometry << 29 | packed | (0x1f << 4);
        if bits & (1 << 6) != 0 {
            let subparts = m.u16(section.checked_add(0x24)?).ok()?;
            let mut mask = 0;
            if subparts != 0 {
                let words = subparts.div_ceil(32);
                mask = vis + SUBPART_POOL;
                if u32::from(used) + u32::from(words) < 512 {
                    mask += u32::from(used) * 4;
                    used += words;
                }
            }
            data[20..24].copy_from_slice(&mask.to_le_bytes());
            if mask == 0 { bits = (bits & !(1 << 6)) | 1; }
        }
        data[0..4].copy_from_slice(&(bits | lighting << 29).to_le_bytes());
        data[4..12].fill(0xff);
        data[12..16].copy_from_slice(&packed.to_le_bytes());
        data[16..20].fill(0);
        data[24..26].copy_from_slice(&extra.to_le_bytes());
        data[26] = alpha;
        data[27] &= 0xf0;
        data[28..32].copy_from_slice(&placement.map_or(0, |(_, at)| at).to_le_bytes());
        plan.put(m, row, &data)?;
        plan.put(m, ATTACHMENTS + cursor, &[u8::MAX])?;
        let size = size * quantize;
        if !size.is_finite() || size >= i32::MAX as f32 { return None; }
        plan.put(m, PIXEL_SIZES + cursor, &[size.floor().clamp(0., 255.) as u8])?;
        cursor += 1;
        main = main.checked_add(1)?;
        plan.sections += 1;
    }
    plan.put(m, COUNT, &cursor.to_le_bytes())?;
    plan.put(m, MAIN_COUNT, &main.to_le_bytes())?;
    plan.put(m, vis + SUBPART_WORDS_USED, &used.to_le_bytes())?;
    Some(plan)
}

/// Both callers immediately overwrite the volatile registers. Callee-saved
/// registers, ESP and the enclosing object gather belong to the caller.
pub fn publish(m: &mut dyn GuestMemory, vis: u32, walk: Walk, mxcsr: u32, cw: u16) -> Option<u32> {
    publish_slice(m, vis, walk, mxcsr, cw, None)
}

/// On a mixed-residency walk the guest can service the cold record and
/// invoke its streaming callbacks. Re-read all inputs for each later row.
pub fn publish_slice(m: &mut dyn GuestMemory, vis: u32, walk: Walk, mxcsr: u32, cw: u16,
    index: Option<u32>) -> Option<u32> {
    if mxcsr & 0xffc0 != 0x1f80 || cw & 0xc3f != 0x3f { return None; }
    plan(m, vis, walk, Mode::from_cw(cw), index)?.commit(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FlatMemory;

    const BSP: u32 = 0x510000;
    const SECTIONS: u32 = BSP + 0x800;
    const LIST: u32 = BSP + 0x400;
    const IDS: u32 = LIST + 0x40;
    const FLAGS: u32 = LIST + 0x80;
    const CACHE: u32 = BSP + 0x1000;
    const STAMP: u32 = CACHE + 0x200 + 0x14;

    fn fixture(walk: Walk) -> FlatMemory {
        let mut m = FlatMemory::new(0x400000, 0x180000);
        for (at, v) in [
            (CURRENT_VIS, MAIN_VIS), (ACTIVE_BSP, BSP),
            (MAIN_VIS + if walk == Walk::Clusters { CLUSTER_LIST } else { PLACEMENT_LIST }, LIST),
            (LIST, 3), (LIST+4, 1), (LIST+8, FLAGS), (LIST+12, IDS), (LIST+20, FLAGS+32),
            (BSP+0x9c, 2), (BSP+0xa0, SECTIONS), (BSP+0x138, 1), (BSP+0x13c, SECTIONS),
            (BSP+0x140, 1), (BSP+0x144, BSP+0x1800), (SECTIONS+0x50, BSP+0x2000),
            (RESOURCE_CACHE, CACHE), (CACHE+0x38, 7), (CACHE+0x64, CACHE+0x100),
            (CACHE+0x144, CACHE+0x200),
        ] { m.set_u32(at, v).unwrap(); }
        m.set_u8(SECTIONS+0x46, 1).unwrap();
        m.set_u16(SECTIONS+0x24, 33).unwrap();
        for (at, v) in [(HALF, 0.5), (QUARTER, 0.25), (ONE, 1.), (TWO, 2.),
            (PIXEL_SCALE, 12.), (MIN_DEPTH, 1.), (PROJECTION_DEPTH+36, 3.),
            (SECTIONS+0x58, 2.), (FADE_SLOPE, 1.), (FADE_RANGE, 255.)] {
            m.set_f32(at, v).unwrap();
        }
        m
    }

    #[test]
    fn cluster_publishes_camera_lod_stencil_and_visibility_allocation() {
        let mut m = fixture(Walk::Clusters);
        m.set_u16(FLAGS, 1<<9).unwrap();
        m.set_u8(MAIN_VIS+0x1c, 0xfe).unwrap();
        assert_eq!(publish(&mut m, MAIN_VIS, Walk::Clusters, 0x1f80, 0x027f), Some(1));
        assert_eq!(m.u32(RECORDS).unwrap(), (3<<29) | (1<<14) | 64);
        assert_eq!(m.u32(RECORDS+4).unwrap(), u32::MAX);
        assert_eq!(m.u32(RECORDS+20).unwrap(), MAIN_VIS+SUBPART_POOL);
        assert_eq!(m.u16(RECORDS+24).unwrap(), 0xfffe);
        assert_eq!(m.u8(PIXEL_SIZES).unwrap(), 2);
        assert_eq!(m.u16(MAIN_VIS+SUBPART_WORDS_USED).unwrap(), 2);
        assert_eq!((m.u32(COUNT).unwrap(), m.u32(MAIN_COUNT).unwrap(), m.u32(STAMP).unwrap()), (1,1,7));
    }

    #[test]
    fn fog_flags_follow_the_view_and_visible_cluster_flags() {
        let mut m = fixture(Walk::Clusters);
        m.set_u32(SCENE_MODE, 3).unwrap();
        m.set_u32(VIEW_FOG_TAG, 1).unwrap();
        m.set_u32(crate::tags::TAG_ENTRIES_GLOBAL, BSP+0x3000).unwrap();
        m.set_u32(BSP+0x3018, BSP+0x3100).unwrap();
        assert_eq!(publish(&mut m, MAIN_VIS, Walk::Clusters, 0x1f80, 0x027f), Some(1));
        assert_eq!(m.u32(RECORDS).unwrap() & 0x180, 0x180);
        m.set_u8(PLANAR_FOG_ACTIVE, 1).unwrap();
        m.set_u8(BSP+0x3100, 2).unwrap();
        assert_eq!(publish(&mut m, MAIN_VIS, Walk::Clusters, 0x1f80, 0x027f), Some(1));
        assert_eq!(m.u32(RECORDS+32).unwrap() & 0x180, 0);
    }

    #[test]
    fn late_cold_section_preserves_all_preceding_rows_and_cache_stamps() {
        let mut m = fixture(Walk::Clusters);
        m.set_u16(LIST+4, 2).unwrap();
        m.set_u32(IDS+4, 1).unwrap();
        let before = m.bytes.clone();
        assert_eq!(publish(&mut m, MAIN_VIS, Walk::Clusters, 0x1f80, 0x027f), None);
        assert_eq!(m.bytes, before);
        assert_eq!(publish_slice(&mut m, MAIN_VIS, Walk::Clusters, 0x1f80, 0x027f, Some(0)), Some(1));
        let hot = m.bytes.clone();
        assert_eq!(publish_slice(&mut m, MAIN_VIS, Walk::Clusters, 0x1f80, 0x027f, Some(1)), None);
        assert_eq!(m.bytes, hot);
        assert_eq!(publish(&mut m, MAIN_VIS+4, Walk::Clusters, 0x1f80, 0x027f), None);
    }

    #[test]
    fn placement_fade_and_packed_size_follow_the_view_pixel_scale() {
        for (render_scale, lod_distance, expected) in [
            (1., 1., None),                // At the fade start: culled.
            (4., 1., Some((48, 143))),     // Partly faded.
            (4., 2., Some((96, 255))),     // Full opacity.
            (6., 4., Some((255, 255))),    // Packed size saturates.
        ] {
            let mut m = fixture(Walk::Placements);
            // Radius equals depth. The small-placement factor halves the diameter,
            // so the projected size in pixels equals the view's scale here.
            m.set_f32(BSP + 0x1800 + 0x48, 3.).unwrap();
            m.set_f32(SMALL_RADIUS, 4.).unwrap();
            m.set_f32(SMALL_SCALE, 0.5).unwrap();
            m.set_f32(FADE_START, 48.).unwrap();
            m.set_f32(FADE_SLOPE, 1. / 256.).unwrap();
            m.set_f32(PIXEL_SCALE, 48. * render_scale * lod_distance).unwrap();

            let count = publish(&mut m, MAIN_VIS, Walk::Placements, 0x1f80, 0x027f);
            if let Some((size, alpha)) = expected {
                assert_eq!(count, Some(1));
                assert_eq!(m.u8(PIXEL_SIZES).unwrap(), size);
                assert_eq!(m.u8(RECORDS + 26).unwrap(), alpha);
            } else {
                assert_eq!(count, Some(0));
                assert_eq!(m.u16(FLAGS).unwrap(), 1);
            }
        }
    }

    #[test]
    fn placement_fade_or_matrix_publication_uses_the_current_view() {
        let mut m = fixture(Walk::Placements);
        let at = BSP+0x1800;
        assert_eq!(publish(&mut m, MAIN_VIS, Walk::Placements, 0x1f80, 0x027f), Some(0));
        assert_eq!(m.u16(FLAGS).unwrap(), 1);
        m.set_f32(at+0x48, 3.).unwrap();
        m.set_u16(FLAGS, 1<<11).unwrap();
        m.set_u16(FLAGS+32, 29).unwrap();
        assert_eq!(publish(&mut m, MAIN_VIS, Walk::Placements, 0x1f80, 0x027f), Some(1));
        assert_eq!(m.u32(RECORDS+28).unwrap(), at);
        assert_eq!(m.u8(RECORDS+26).unwrap(), 255);
        assert_eq!(m.u16(RECORDS+24).unwrap(), 29);
        assert_eq!(m.u32(RECORDS).unwrap() & ((1<<12)|64|1), (1<<12)|1);
        assert_eq!(m.u8(PIXEL_SIZES).unwrap(), 6);
    }
}
