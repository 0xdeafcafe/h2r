//! Point to leaf, `0x0014a280` (docs/engine/collision-queries.md, entry A).
use super::{BAD_CHILD, BAD_PLANE, Bsp, LEAF, MAX_DEPTH, NONE, non_negative};
use crate::Bail;
use sim_core::det::Fp;

impl Bsp {
    /// The leaf holding point `p` (`x, y, z` bits), walking from `node`
    /// (masked to 23 bits, as the guest does even for a leaf word). At
    /// each node `dist = ((nz*pz + ny*py) + nx*px) - d`; the front child
    /// when `dist >= 0`. Returns the leaf index, or [`NONE`] for the solid
    /// sentinel.
    pub fn point_to_leaf(&self, node: u32, p: [u32; 3]) -> Result<u32, Bail> {
        let mut fp = Fp::new();
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
                return Ok(if child == NONE {
                    NONE
                } else {
                    child & 0x7f_ffff
                });
            }
            index = child;
        }
        Err(Bail::Depth)
    }
}
