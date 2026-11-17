//! Pathfinding data as a `map::build` block tree, and the matching decode
//! of raw element bytes (for round-trip tests).
use map::build::Block;
use tags::pathfinding::PathfindingData;

fn table(bytes: Vec<u8>) -> Block {
    Block::new(bytes)
}

/// The 0x74-byte pathfinding element with every child block as a fixup.
/// The caller puts it in the BSP tree as `sbsp.block(0xc4, 1, element)`.
///
/// Object refs and user-placed hints are written as raw elements; their
/// own child blocks are not described, so a non-empty object-ref list or a
/// user-hint element with non-zero child blocks is refused (their pointers
/// would be left stale). Generated data has neither.
pub fn element(pf: &PathfindingData) -> Result<Block, &'static str> {
    if !pf.object_refs.is_empty() {
        return Err("object refs have undescribed child blocks");
    }
    if pf
        .user_hints
        .iter()
        .any(|e| e.len() != PathfindingData::USER_HINTS_SIZE || e.iter().any(|&b| b != 0))
    {
        return Err("user-placed hints have undescribed child blocks");
    }
    let mut e = Block::zeroed(PathfindingData::SIZE);
    let mut sectors = Vec::with_capacity(pf.sectors.len() * 8);
    for s in &pf.sectors {
        sectors.extend(s.flags.to_le_bytes());
        sectors.extend(s.hint.to_le_bytes());
        sectors.extend(s.first_link.to_le_bytes());
    }
    let mut links = Vec::with_capacity(pf.links.len() * 16);
    for l in &pf.links {
        for v in [
            l.vertex1,
            l.vertex2,
            l.flags,
            l.hint,
            l.forward_link,
            l.reverse_link,
            l.left_sector,
            l.right_sector,
        ] {
            links.extend(v.to_le_bytes());
        }
    }
    let refs: Vec<u8> = pf.refs.iter().flat_map(|r| r.0.to_le_bytes()).collect();
    let mut nodes = Vec::with_capacity(pf.bsp2d_nodes.len() * 0x14);
    for n in &pf.bsp2d_nodes {
        for f in n.plane {
            nodes.extend(f.to_le_bytes());
        }
        nodes.extend(n.positive.0.to_le_bytes());
        nodes.extend(n.negative.0.to_le_bytes());
    }
    let vertices: Vec<u8> = pf
        .vertices
        .iter()
        .flatten()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let hints: Vec<u8> = pf.hints.iter().flatten().copied().collect();
    let instance_refs: Vec<u8> = pf
        .instance_refs
        .iter()
        .flat_map(|r| r.to_le_bytes())
        .collect();
    let user: Vec<u8> = pf.user_hints.iter().flatten().copied().collect();
    let mut put = |offset: u32, count: usize, bytes: Vec<u8>| {
        if count > 0 {
            e.block(offset, count as u32, table(bytes));
        }
    };
    put(0x00, pf.sectors.len(), sectors);
    put(0x08, pf.links.len(), links);
    put(0x10, pf.refs.len(), refs);
    put(0x18, pf.bsp2d_nodes.len(), nodes);
    put(0x28, pf.vertices.len(), vertices);
    put(0x38, pf.hints.len(), hints);
    put(0x40, pf.instance_refs.len(), instance_refs);
    put(0x6c, pf.user_hints.len(), user);
    e.put_u32(0x48, pf.checksum);
    Ok(e)
}
