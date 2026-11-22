//! A structure section's visibility MOPP code (section `+0x28`, a data
//! resource) and its reorder table (`+0x30`, `u16`s), for a grown write
//! (docs/editor/modeling-targets.md, "Structure BSP render geometry").
//!
//! The layout is the one `h1convert::structure::visibility_mopp` writes
//! and docs/halo1/multiplayer.md ("The converted structure BSP") explains:
//! one leaf per subpart (one subpart per part, keyed by the part's index)
//! over the part's box, and the table the keys in the order the engine's
//! walk visits the leaves (`table[..n]`) and each key's position
//! (`table[n + key]`). A wrong table makes the engine's volume pass write
//! past a view's mask, so [`check`] is run on every section written.

/// Field of the code in the structure section struct.
pub(crate) const CODE_FIELD: u16 = 0x28;
/// Field of the reorder table (a block of `u16`).
pub(crate) const TABLE_FIELD: u16 = 0x30;

/// The code block and the table's bytes for parts with these boxes
/// (`[x min, x max, y min, y max, z min, z max]`).
pub(crate) fn build(boxes: &[[f32; 6]]) -> (Vec<u8>, Vec<u8>) {
    let surfaces: Vec<mopp::Surface> = boxes
        .iter()
        .enumerate()
        .map(|(k, b)| mopp::Surface {
            key: k as u32,
            vertices: vec![[b[0], b[2], b[4]], [b[1], b[3], b[5]]],
        })
        .collect();
    let code = mopp::build(&surfaces);
    let n = boxes.len();
    let order = leaf_order(&code);
    let mut table = vec![0u16; 2 * n];
    for (p, &k) in order.iter().enumerate().take(n) {
        table[p] = k as u16;
        if let Some(t) = table.get_mut(n + k as usize) {
            *t = p as u16;
        }
    }
    let table = table.into_iter().flat_map(u16::to_le_bytes).collect();
    (code.to_block(), table)
}

/// Keys of `code`'s leaves in the order the engine walks them (a query
/// every leaf passes visits a split's first child first).
fn leaf_order(code: &mopp::Code) -> Vec<u32> {
    let all = mopp::Aabb {
        min: [-1.0e9; 3],
        max: [1.0e9; 3],
    };
    mopp::query_aabb(code, &all).unwrap_or_default()
}

/// Whether a code block and table are laid out as the engine reads them
/// for `n` subparts: every key below `n`, one leaf per key, `table[..n]`
/// the keys in leaf order and `table[n + key]` each key's position.
pub(crate) fn check(n: usize, code: &[u8], table: &[u8]) -> Result<(), String> {
    let table: Vec<u16> = table
        .chunks_exact(2)
        .map(|w| u16::from_le_bytes([w[0], w[1]]))
        .collect();
    let code = mopp::Code::parse(code).map_err(|e| format!("visibility code: {e:?}"))?;
    mopp::check::validate(&code.bytecode).map_err(|e| format!("visibility code: {e:?}"))?;
    let leaves = leaf_order(&code);
    if table.len() < 2 * n {
        return Err(format!("visibility table of {} for {n} subparts", table.len()));
    }
    if leaves.len() != n || leaves.iter().any(|&k| k as usize >= n) {
        return Err(format!("{} visibility leaves for {n} subparts", leaves.len()));
    }
    for (p, &k) in table[..n].iter().enumerate() {
        if u32::from(k) != leaves[p] || usize::from(table[n + usize::from(k)]) != p {
            return Err(format!("visibility table disagrees at {p}"));
        }
    }
    Ok(())
}
