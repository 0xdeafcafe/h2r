//! Point to leaf, `0x0014a280` (docs/engine/collision-queries.md, entry A).
use super::{BAD_CHILD, BAD_PLANE, Bsp, LEAF, MAX_DEPTH, NONE, block, non_negative};
use crate::Bail;
use sim_core::det::Fp;

/// The terminal visit of a point walk. The guest leaves the node cursor
/// in ECX, the signed distance in XMM1, and the x product in XMM2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointResult {
    pub leaf: u32,
    pub node_cursor: u32,
    pub distance: u32,
    pub x_product: u32,
}

impl Bsp {
    /// The leaf holding point `p` (`x, y, z` bits), walking from `node`
    /// (masked to 23 bits, as the guest does even for a leaf word). At
    /// each node `dist = ((nz*pz + ny*py) + nx*px) - d`; the front child
    /// when `dist >= 0`. Returns the leaf index, or [`NONE`] for the solid
    /// sentinel.
    pub fn point_to_leaf(&self, node: u32, p: [u32; 3]) -> Result<u32, Bail> {
        Ok(self.point_result(node, p)?.leaf)
    }

    /// Walk once and also return the intermediates visible at the guest ABI.
    pub fn point_result(&self, node: u32, p: [u32; 3]) -> Result<PointResult, Bail> {
        self.point_result_with(&mut Fp::new(), node, p)
    }

    /// [`Bsp::point_result`] in the caller's `fp`: under
    /// [`Fp::tracked`] it gathers the flags the guest's walk raises.
    pub fn point_result_with(
        &self,
        fp: &mut Fp,
        node: u32,
        p: [u32; 3],
    ) -> Result<PointResult, Bail> {
        let mut index = node & 0x7f_ffff;
        for _ in 0..MAX_DEPTH {
            let n = self.nodes.get(index as usize).ok_or(Bail::Data)?;
            if n.bad & BAD_PLANE != 0 {
                return Err(Bail::Data);
            }
            let [nx, ny, nz, d] = n.plane;
            let z = fp.mul(nz, p[2]);
            let y = fp.mul(ny, p[1]);
            let zy = fp.add(z, y);
            let x = fp.mul(nx, p[0]);
            let dot = fp.add(zy, x);
            let dist = fp.sub(dot, d);
            let side = usize::from(non_negative(dist));
            if !fp.exact() {
                return Err(Bail::Float);
            }
            if n.bad & BAD_CHILD[side] != 0 {
                return Err(Bail::Data);
            }
            let child = n.child[side];
            if child & LEAF != 0 {
                return Ok(PointResult {
                    leaf: if child == NONE {
                        NONE
                    } else {
                        child & 0x7f_ffff
                    },
                    node_cursor: self
                        .block_addr(block::NODES)
                        .wrapping_add(index * 8 + side as u32 * 2),
                    distance: dist,
                    x_product: x,
                });
            }
            index = child;
        }
        Err(Bail::Depth)
    }
}
