//! The light's geometry flags, `0x00031660`: per the definition's bits,
//! the cluster, placement and object rows of the light's visibility get
//! their caster and receiver bits, the owner's objects theirs, and a
//! light forced onto its clusters gets `+0xf8` set to one.

use super::guest::{LIGHT_VIS, List, NONE, TAGS, at, lights, ptr, tag};
use super::{Refusal, Result, Writes};
use guest_layout::memory::GuestMemory;

const ONE: u32 = 0x0045_dbc0;

/// The arguments of `0x00031660`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arguments {
    /// EAX: the light datum.
    pub light: u32,
    /// ECX: the `ligh` datum.
    pub definition: u32,
    /// ESI: the visibility struct the gather filled.
    pub vis: u32,
    /// Stack 0: the owner (`L+0x4c`, or -1).
    pub owner: u32,
    /// Stack 1: the owner's root.
    pub root: u32,
    /// Stack 2: its low byte is set for a kind-0 (point) volume.
    pub point: u32,
    /// Stack 3: the shadow byte's address.
    pub shadow: u32,
}

/// `0x00031660`. Only the light's own visibility struct is taken.
pub fn flags(m: &dyn GuestMemory, a: Arguments) -> Result<Writes> {
    if a.vis != LIGHT_VIS {
        return Err(Refusal::Unsupported);
    }
    let tags = ptr(m, TAGS)?;
    let light = lights(m)?.slot(m, a.light)?;
    let definition = tag(m, tags, a.definition)?;
    if m.u32(at(light, 4)?)? != a.definition {
        return Err(Refusal::Layout);
    }
    let flags = m.u32(definition)?;
    let mut out = Writes::new();

    let clusters = List::read(m, ptr(m, at(a.vis, 0xc)?)?)?;
    let force_clusters = flags & 4 != 0 && clusters.count > 1;
    if force_clusters {
        if a.shadow == 0 && a.point as u8 == 0 {
            return Err(Refusal::Unsupported);
        }
        out.word(at(light, 0xf8)?, m.u32(ONE)?);
    }
    if flags & 0x10000 != 0 {
        write_flags(m, &mut out, &clusters, |_, old| Ok((old & !0x2000) | 0x1000))?;
    }
    if flags & 0x10000 != 0 {
        let placements = List::read(m, ptr(m, at(a.vis, 0x18)?)?)?;
        write_flags(m, &mut out, &placements, |_, old| Ok((old & !0x2000) | 0x1000))?;
    }
    if flags & 0x23800 != 0 {
        let objects = List::read(m, ptr(m, at(a.vis, 0x14)?)?)?;
        let (owner, root) = (a.owner, a.root);
        let handles = if flags & 0x3800 != 0 && objects.count != 0 && owner != NONE && root != NONE {
            objects.column(m, 0xc)?
        } else {
            0
        };
        write_flags(m, &mut out, &objects, |i, old| {
            let base = if flags & 0x20000 != 0 { (old & !0x2000) | 0x1000 } else { old };
            if flags & 0x3800 == 0 {
                return Ok(base);
            }
            let matches = owner == NONE || root == NONE || {
                let handle = m.u32(at(handles, i as u32 * 4)?)?;
                handle == owner || handle == root
            };
            Ok(if matches { owner_flags(base, flags) } else { base })
        })?;
    }
    Ok(out)
}

/// An owner object's caster and receiver bits under definition bits 11
/// to 13.
pub fn owner_flags(old: u16, flags: u32) -> u16 {
    let first = flags & 0x2000 == 0 || flags & 0x800 == 0;
    let second = flags & 0x2000 == 0 || flags & 0x1000 == 0;
    (old & !0x3000) | (u16::from(first) << 12) | (u16::from(second) << 13)
}

fn write_flags(
    m: &dyn GuestMemory,
    out: &mut Writes,
    list: &List,
    mut apply: impl FnMut(usize, u16) -> Result<u16>,
) -> Result<()> {
    if list.count == 0 {
        return Ok(());
    }
    let addr = list.column(m, 8)?;
    let mut flags = m.bytes(addr, list.count * 2)?;
    for (i, bits) in flags.chunks_exact_mut(2).enumerate() {
        let value = apply(i, u16::from_le_bytes([bits[0], bits[1]]))?;
        bits.copy_from_slice(&value.to_le_bytes());
    }
    out.array(addr, flags);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_geometry_preserves_unrelated_visibility_bits() {
        assert_eq!(owner_flags(0xc602, 0x2800), 0xe602);
        assert_eq!(owner_flags(0xe602, 0x3800), 0xc602);
        assert_eq!(owner_flags(0xc602, 0x800), 0xf602);
    }
}
