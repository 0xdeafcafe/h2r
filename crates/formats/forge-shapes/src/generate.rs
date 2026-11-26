//! The generators of the structural shapes. Every length is `+ - * /` on
//! `f32` and integers, so a definition gives the same bits on every host.

use crate::circle;
use crate::def::{PieceDef, Shape};
use crate::error::{Error, Result};
use crate::natural;
use crate::solid::{Axis, Face, Hull, P3, Role, Solid};
use crate::units::{fraction, world};

impl PieceDef {
    /// The geometry of the definition, after [`PieceDef::validate`].
    pub fn generate(&self) -> Result<Solid> {
        self.validate()?;
        let [ux, uy, uz] = self.size;
        let (sx, sy, sz) = (world(ux), world(uy), world(uz));
        match self.shape {
            Shape::Block | Shape::Wall | Shape::FloorPlate => {
                Ok(Solid::convex(Hull::boxed([0.0; 3], [sx, sy, sz])))
            }
            Shape::Ramp => {
                let profile = [[0.0, 0.0], [sx, 0.0], [sx, sz]];
                Ok(Solid::convex(Hull::prism(&profile, Axis::Y, 0.0, sy)))
            }
            Shape::Wedge => {
                let profile = [[0.0, 0.0], [sx, 0.0], [sx, sy]];
                Ok(Solid::convex(Hull::prism(&profile, Axis::Z, 0.0, sz)))
            }
            Shape::Stairs => Ok(stairs(self.size, self.param)),
            Shape::Cylinder => Ok(cylinder(self.size, self.param)),
            Shape::Arch => arch(self.size, self.param),
            Shape::Boulder | Shape::Slab | Shape::Cliff | Shape::TerrainWedge | Shape::Spire => {
                natural::generate(self.shape, self.size, self.param)
            }
        }
    }
}

impl Shape {
    /// The geometry of a definition; see [`PieceDef::generate`].
    pub fn generate(def: &PieceDef) -> Result<Solid> {
        def.generate()
    }
}

fn cylinder(size: [u16; 3], sides: u16) -> Solid {
    let (w, d, h) = (world(size[0]), world(size[1]), world(size[2]));
    let s = circle::SCALE;
    let profile: Vec<[f32; 2]> = (0..sides)
        .map(|k| {
            let (c, n) = circle::point(sides, k);
            [
                circle::scaled(w, s + c) * 0.5,
                circle::scaled(d, s + n) * 0.5,
            ]
        })
        .collect();
    Solid::convex(Hull::prism(&profile, Axis::Z, 0.0, h))
}

/// A rectangle in the plane `y = at` as a ring (x then z corners).
fn rect_y(x: [f32; 2], z: [f32; 2], at: f32) -> Vec<P3> {
    vec![
        [x[0], at, z[0]],
        [x[1], at, z[0]],
        [x[1], at, z[1]],
        [x[0], at, z[1]],
    ]
}

/// A rectangle in the plane `x = at`.
fn rect_x(y: [f32; 2], z: [f32; 2], at: f32) -> Vec<P3> {
    vec![
        [at, y[0], z[0]],
        [at, y[1], z[0]],
        [at, y[1], z[1]],
        [at, y[0], z[1]],
    ]
}

/// A rectangle in the plane `z = at`.
fn rect_z(x: [f32; 2], y: [f32; 2], at: f32) -> Vec<P3> {
    vec![
        [x[0], y[0], at],
        [x[1], y[0], at],
        [x[1], y[1], at],
        [x[0], y[1], at],
    ]
}

const NEG_X: P3 = [-1.0, 0.0, 0.0];
const POS_X: P3 = [1.0, 0.0, 0.0];
const NEG_Y: P3 = [0.0, -1.0, 0.0];
const POS_Y: P3 = [0.0, 1.0, 0.0];
const NEG_Z: P3 = [0.0, 0.0, -1.0];
const POS_Z: P3 = [0.0, 0.0, 1.0];

/// `n` equal steps rising along x. One hull a step; the sides are a fan of
/// triangles from the foot of the back wall, which joins every corner of the
/// profile without a T-junction against the treads and risers.
fn stairs(size: [u16; 3], n: u16) -> Solid {
    let sy = world(size[1]);
    let xs: Vec<f32> = (0..=n).map(|i| fraction(size[0], i, n)).collect();
    let hs: Vec<f32> = (0..=n).map(|i| fraction(size[2], i, n)).collect();
    let (len, top) = (xs[usize::from(n)], hs[usize::from(n)]);
    let steps = usize::from(n);

    let hulls = (1..=steps)
        .map(|s| Hull::boxed([xs[s - 1], 0.0, 0.0], [xs[s], sy, hs[s]]))
        .collect();

    let mut shell = Vec::new();
    let mut profile = vec![[0.0f32, 0.0]];
    for s in 1..=steps {
        profile.push([xs[s - 1], hs[s]]);
        profile.push([xs[s], hs[s]]);
    }
    let apex = [len, 0.0];
    for (at, toward) in [(0.0, NEG_Y), (sy, POS_Y)] {
        for pair in profile.windows(2) {
            let ring = [apex, pair[0], pair[1]]
                .iter()
                .map(|p| [p[0], at, p[1]])
                .collect();
            shell.push(Face::oriented(ring, toward, None));
        }
    }
    shell.push(Face::oriented(
        rect_z([0.0, len], [0.0, sy], 0.0),
        NEG_Z,
        None,
    ));
    shell.push(Face::oriented(
        rect_x([0.0, sy], [0.0, top], len),
        POS_X,
        None,
    ));
    for s in 1..=steps {
        let tread = rect_z([xs[s - 1], xs[s]], [0.0, sy], hs[s]);
        shell.push(Face::oriented(tread, POS_Z, None));
        let riser = rect_x([0.0, sy], [hs[s - 1], hs[s]], xs[s - 1]);
        shell.push(Face::oriented(riser, NEG_X, Some(Role::Trim)));
    }
    Solid { shell, hulls }
}

/// A round-headed opening through y in a rounded body: two piers and a ring
/// of `n` voussoirs. The outer arc has radius half the width, so the body's
/// top is round; the opening is about half as wide.
///
/// With an odd `n` no vertex is at the apex and the top sits a little below
/// the bounds.
fn arch(size: [u16; 3], n: u16) -> Result<Solid> {
    let [ux, uy, uz] = size;
    // The opening's width has the parity of the size so a pier is whole units.
    let mut open = ux / 2;
    if (ux - open) % 2 != 0 {
        open += 1;
    }
    let min_z = (ux + 3) / 2;
    if uz < min_z {
        return Err(Error::ArchTooLow { size_x: ux, min_z });
    }
    let (sx, sy, sz) = (world(ux), world(uy), world(uz));
    let outer_r = sx * 0.5;
    let inner_r = world(open) * 0.5;
    let (cx, spring) = (sx * 0.5, sz - outer_r);

    let sides = 2 * n;
    let at = |k: u16, r: f32| -> [f32; 2] {
        let (c, s) = circle::point(sides, k);
        [cx + circle::scaled(r, c), spring + circle::scaled(r, s)]
    };
    let outer: Vec<[f32; 2]> = (0..=n).map(|k| at(k, outer_r)).collect();
    let inner: Vec<[f32; 2]> = (0..=n).map(|k| at(k, inner_r)).collect();
    let nn = usize::from(n);
    // Left pier: from the outer end of the arc's far side to its inner end.
    let left = [outer[nn][0], inner[nn][0]];
    let right = [inner[0][0], outer[0][0]];

    let mut hulls = vec![
        Hull::boxed([left[0], 0.0, 0.0], [left[1], sy, spring]),
        Hull::boxed([right[0], 0.0, 0.0], [right[1], sy, spring]),
    ];
    for j in 0..nn {
        let profile = [inner[j], outer[j], outer[j + 1], inner[j + 1]];
        hulls.push(Hull::prism(&profile, Axis::Y, 0.0, sy));
    }

    let mut shell = Vec::new();
    for (y, toward) in [(0.0, NEG_Y), (sy, POS_Y)] {
        for x in [left, right] {
            shell.push(Face::oriented(rect_y(x, [0.0, spring], y), toward, None));
        }
        for j in 0..nn {
            let ring = [inner[j], outer[j], outer[j + 1], inner[j + 1]]
                .iter()
                .map(|p| [p[0], y, p[1]])
                .collect();
            shell.push(Face::oriented(ring, toward, None));
        }
    }
    for x in [left, right] {
        shell.push(Face::oriented(rect_z(x, [0.0, sy], 0.0), NEG_Z, None));
    }
    shell.push(Face::oriented(
        rect_x([0.0, sy], [0.0, spring], left[0]),
        NEG_X,
        None,
    ));
    shell.push(Face::oriented(
        rect_x([0.0, sy], [0.0, spring], right[1]),
        POS_X,
        None,
    ));
    shell.push(Face::oriented(
        rect_x([0.0, sy], [0.0, spring], left[1]),
        POS_X,
        None,
    ));
    shell.push(Face::oriented(
        rect_x([0.0, sy], [0.0, spring], right[0]),
        NEG_X,
        None,
    ));

    let centre = [cx, spring];
    for j in 0..nn {
        let strip = |arc: &[[f32; 2]]| -> Vec<P3> {
            vec![
                [arc[j][0], 0.0, arc[j][1]],
                [arc[j + 1][0], 0.0, arc[j + 1][1]],
                [arc[j + 1][0], sy, arc[j + 1][1]],
                [arc[j][0], sy, arc[j][1]],
            ]
        };
        let mid = |arc: &[[f32; 2]]| {
            [
                (arc[j][0] + arc[j + 1][0]) * 0.5 - centre[0],
                0.0,
                (arc[j][1] + arc[j + 1][1]) * 0.5 - centre[1],
            ]
        };
        shell.push(Face::oriented(strip(&outer), mid(&outer), None));
        let into = mid(&inner).map(|v| -v);
        shell.push(Face::oriented(strip(&inner), into, Some(Role::Trim)));
    }
    Ok(Solid { shell, hulls })
}
