//! Keep a subset of generated pathfinding data's sectors: the links that
//! border a kept sector, the vertices those links use (one per position),
//! and every collision surface's ref, `NONE` where its sector went. See
//! docs/formats/pathfinding.md, "Trimming".
use std::collections::HashMap;
use tags::pathfinding::{Link, PathfindingData, Ref, RefTarget, Sector};

/// "None" in a 16-bit sector or link index, and a dropped sector in the
/// map [`keep`] returns.
pub const NONE: u16 = u16::MAX;

/// The data with only the sectors whose flag in `keep` is set, in their
/// old order, and for each old sector its new index ([`NONE`] when
/// dropped). A link survives when a kept sector borders it; a side whose
/// sector went becomes `NONE` and its next-link field points at the link
/// itself, as the generator writes an unused side. Refs keep one entry
/// per collision surface. Vertices are shared by position.
///
/// Generated data only: 2D BSP nodes, object refs and compiled hints
/// name sectors in ways this does not rewrite, so they are refused.
pub fn keep_sectors(
    pf: &PathfindingData,
    keep: &[bool],
) -> Result<(PathfindingData, Vec<u16>), &'static str> {
    if keep.len() != pf.sectors.len() {
        return Err("one keep flag per sector");
    }
    if !pf.bsp2d_nodes.is_empty() || !pf.object_refs.is_empty() || !pf.hints.is_empty() {
        return Err("only generated data (no 2D BSP nodes, object refs or hints) is trimmed");
    }
    let mut sector_of = vec![NONE; pf.sectors.len()];
    let mut kept = 0u16;
    for (slot, _) in sector_of.iter_mut().zip(keep).filter(|(_, k)| **k) {
        *slot = kept;
        kept += 1;
    }
    let side = |s: u16| sector_of.get(usize::from(s)).copied().unwrap_or(NONE);
    let mut link_of = vec![NONE; pf.links.len()];
    let mut count = 0u16;
    for (slot, l) in link_of.iter_mut().zip(&pf.links) {
        if side(l.left_sector) != NONE || side(l.right_sector) != NONE {
            *slot = count;
            count += 1;
        }
    }
    let mut vertex_of: HashMap<[u32; 3], u16> = HashMap::new();
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut vertex = |v: u16| -> Result<u16, &'static str> {
        let p = *pf
            .vertices
            .get(usize::from(v))
            .ok_or("link vertex out of range")?;
        let key = p.map(f32::to_bits);
        if let Some(&i) = vertex_of.get(&key) {
            return Ok(i);
        }
        let i = u16::try_from(vertices.len())
            .ok()
            .filter(|&i| i != NONE)
            .ok_or("too many vertices")?;
        vertices.push(p);
        vertex_of.insert(key, i);
        Ok(i)
    };
    let mut links = Vec::with_capacity(usize::from(count));
    for (l, &new) in pf.links.iter().zip(&link_of) {
        if new == NONE {
            continue;
        }
        let (left, right) = (side(l.left_sector), side(l.right_sector));
        // The next link around a kept side is on that sector's ring, so it
        // is kept too; around a dropped side, the link itself.
        let next = |sector: u16, at: u16| -> Result<u16, &'static str> {
            if sector == NONE {
                return Ok(new);
            }
            link_of
                .get(usize::from(at))
                .copied()
                .filter(|&k| k != NONE)
                .ok_or("a kept sector's ring leaves its links")
        };
        links.push(Link {
            vertex1: vertex(l.vertex1)?,
            vertex2: vertex(l.vertex2)?,
            flags: l.flags,
            hint: l.hint,
            forward_link: next(left, l.forward_link)?,
            reverse_link: next(right, l.reverse_link)?,
            left_sector: left,
            right_sector: right,
        });
    }
    let sectors = pf
        .sectors
        .iter()
        .zip(keep)
        .filter(|(_, k)| **k)
        .map(|(s, _)| {
            let first = usize::try_from(s.first_link)
                .ok()
                .and_then(|l| link_of.get(l).copied())
                .filter(|&l| l != NONE)
                .ok_or("a kept sector's first link")?;
            Ok(Sector {
                first_link: u32::from(first),
                ..*s
            })
        })
        .collect::<Result<Vec<_>, &'static str>>()?;
    let refs = pf
        .refs
        .iter()
        .map(|&r| match r.target() {
            RefTarget::Sector(s) => match sector_of.get(s as usize) {
                Some(&n) if n != NONE => Ref::sector(u32::from(n)),
                _ => Ref::NONE,
            },
            _ => r,
        })
        .collect();
    Ok((
        PathfindingData {
            sectors,
            links,
            refs,
            bsp2d_nodes: Vec::new(),
            vertices,
            object_refs: Vec::new(),
            hints: Vec::new(),
            instance_refs: pf.instance_refs.clone(),
            checksum: pf.checksum,
            user_hints: pf.user_hints.clone(),
        },
        sector_of,
    ))
}
