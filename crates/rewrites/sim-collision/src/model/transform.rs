//! The object node's 13-word similarity transform. Keep the add order of
//! the inverse (141590), local ray (1eff40) and world plane (168b40).
use crate::{Bail, GuestRead};
use sim_core::det::{self, Fp};
use std::cmp::Ordering;

#[derive(Clone, Copy)]
pub(super) struct Matrix {
    scale: u32,
    axes: [[u32; 3]; 3],
    position: [u32; 3],
}

impl Matrix {
    pub(super) fn read(g: &impl GuestRead, at: u32) -> Result<Self, Bail> {
        Ok(Self {
            scale: g.u32(at)?,
            axes: [
                g.vec3(at.wrapping_add(4))?,
                g.vec3(at.wrapping_add(16))?,
                g.vec3(at.wrapping_add(28))?,
            ],
            position: g.vec3(at.wrapping_add(40))?,
        })
    }

    pub(super) fn inverse(self, fp: &mut Fp, one: u32) -> Self {
        fp.check(self.scale);
        fp.check(one);
        if det::compare(self.scale, 0) == Ordering::Equal {
            return Self {
                scale: 0,
                axes: [[0; 3]; 3],
                position: [0; 3],
            };
        }
        let scaled = det::compare(self.scale, one) != Ordering::Equal;
        let scale = if scaled { fp.div(one, self.scale) } else { one };
        let neg = self.position.map(|p| {
            let v = fp.sub(0, p);
            if scaled { fp.mul(scale, v) } else { v }
        });
        let axes: [[u32; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| self.axes[j][i]));
        let position = std::array::from_fn(|i| {
            let x = fp.mul(axes[0][i], neg[0]);
            let z = fp.mul(axes[2][i], neg[2]);
            let y = fp.mul(axes[1][i], neg[1]);
            let xz = if i == 2 { fp.add(x, z) } else { fp.add(z, x) };
            fp.add(xz, y)
        });
        Self {
            scale,
            axes,
            position,
        }
    }

    pub(super) fn local(&self, fp: &mut Fp, mut v: [u32; 3], one: u32, point: bool) -> [u32; 3] {
        if det::compare(self.scale, one) != Ordering::Equal {
            v = v.map(|n| fp.mul(n, self.scale));
        }
        std::array::from_fn(|i| {
            let x = fp.mul(self.axes[0][i], v[0]);
            let y = fp.mul(self.axes[1][i], v[1]);
            let z = fp.mul(self.axes[2][i], v[2]);
            let n = if i == 0 {
                let yx = fp.add(y, x);
                fp.add(yx, z)
            } else {
                let zy = fp.add(z, y);
                fp.add(zy, x)
            };
            if point {
                fp.add(n, self.position[i])
            } else {
                n
            }
        })
    }

    pub(super) fn world_plane(self, fp: &mut Fp, normal: [u32; 3], d: u32, flip: bool) -> [u32; 4] {
        let mut p = [0; 4];
        for (i, n) in p[..3].iter_mut().enumerate() {
            let z = fp.mul(self.axes[2][i], normal[2]);
            let y = fp.mul(self.axes[1][i], normal[1]);
            let x = fp.mul(self.axes[0][i], normal[0]);
            let zy = fp.add(z, y);
            *n = fp.add(zy, x);
        }
        let z = fp.mul(self.position[2], p[2]);
        let scaled = fp.mul(d, self.scale);
        let y = fp.mul(self.position[1], p[1]);
        let x = fp.mul(p[0], self.position[0]);
        let zd = fp.add(z, scaled);
        let zdy = fp.add(zd, y);
        p[3] = fp.add(zdy, x);
        if flip { p.map(|v| fp.sub(0, v)) } else { p }
    }
}
