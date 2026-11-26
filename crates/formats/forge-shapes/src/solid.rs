//! The geometry a definition generates: a closed shell of convex planar
//! faces and the convex hulls whose union fills it.

/// A point or a vector of the piece's space: x, y, z in world units, the
/// origin at the lowest corner of the bounds.
pub type P3 = [f32; 3];

/// The family a face belongs to; a material is chosen per role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    Floor,
    Wall,
    Ceiling,
    Trim,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Floor, Role::Wall, Role::Ceiling, Role::Trim];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The role of a face by its normal: within 45 degrees of up is
    /// `Floor`, of down `Ceiling`, any other `Wall`. A normal at exactly 45
    /// degrees counts as the nearer of up or down.
    pub fn of_normal(n: P3) -> Role {
        let len2 = n[0] * n[0] + n[1] * n[1] + n[2] * n[2];
        let z2 = 2.0 * n[2] * n[2];
        if n[2] > 0.0 && z2 >= len2 {
            Role::Floor
        } else if n[2] < 0.0 && z2 >= len2 {
            Role::Ceiling
        } else {
            Role::Wall
        }
    }
}

/// A convex planar polygon wound counter-clockwise seen from outside.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub ring: Vec<P3>,
    /// Texture coordinates, one a vertex: the position along the face's two
    /// dominant axes in units over [`crate::units::TILE`].
    pub uv: Vec<[f32; 2]>,
    pub role: Role,
}

/// A convex polyhedron: points and faces as indices into them, each face
/// wound outward.
#[derive(Clone, Debug, PartialEq)]
pub struct Hull {
    pub points: Vec<P3>,
    pub faces: Vec<Vec<u32>>,
}

/// The shell and the hulls of one definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Solid {
    pub shell: Vec<Face>,
    pub hulls: Vec<Hull>,
}

/// The area-weighted normal of a ring (Newell's method).
pub fn newell(ring: &[P3]) -> P3 {
    let mut n = [0.0f32; 3];
    for (i, a) in ring.iter().enumerate() {
        let b = &ring[(i + 1) % ring.len()];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    n
}

fn dot(a: P3, b: P3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn centroid(points: &[P3]) -> P3 {
    let mut c = [0.0f32; 3];
    for p in points {
        for (c, p) in c.iter_mut().zip(p) {
            *c += p;
        }
    }
    let n = points.len() as f32;
    [c[0] / n, c[1] / n, c[2] / n]
}

fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

impl Face {
    /// A face from a ring wound either way: it is reversed when its normal
    /// points away from `toward`, a direction known to be outward. With no
    /// `role` the role comes from the normal.
    pub fn oriented(mut ring: Vec<P3>, toward: P3, role: Option<Role>) -> Face {
        if dot(newell(&ring), toward) < 0.0 {
            ring.reverse();
        }
        let n = newell(&ring);
        let uv = project(&ring, n);
        Face {
            ring,
            uv,
            role: role.unwrap_or_else(|| Role::of_normal(n)),
        }
    }

    /// The unit-free normal, outward.
    pub fn normal(&self) -> P3 {
        newell(&self.ring)
    }
}

/// u and v of a ring: its positions along the two axes the normal is
/// least along, in units over the tile.
fn project(ring: &[P3], n: P3) -> Vec<[f32; 2]> {
    let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
    let (u, v) = if ax >= ay && ax >= az {
        (1, 2)
    } else if ay >= az {
        (0, 2)
    } else {
        (0, 1)
    };
    ring.iter()
        .map(|p| {
            [
                p[u] * 100.0 / crate::units::TILE,
                p[v] * 100.0 / crate::units::TILE,
            ]
        })
        .collect()
}

/// The profile's axis of extrusion.
#[derive(Clone, Copy)]
pub(crate) enum Axis {
    /// The profile is in x and z, extruded along y.
    Y,
    /// The profile is in x and y, extruded along z.
    Z,
}

impl Hull {
    /// A convex profile extruded between `lo` and `hi` along `axis`.
    pub(crate) fn prism(profile: &[[f32; 2]], axis: Axis, lo: f32, hi: f32) -> Hull {
        let place = |p: [f32; 2], at: f32| match axis {
            Axis::Y => [p[0], at, p[1]],
            Axis::Z => [p[0], p[1], at],
        };
        let m = profile.len() as u32;
        let mut points: Vec<P3> = profile.iter().map(|&p| place(p, lo)).collect();
        points.extend(profile.iter().map(|&p| place(p, hi)));
        let mut faces: Vec<Vec<u32>> = vec![(0..m).collect(), (m..2 * m).collect()];
        for i in 0..m {
            let j = (i + 1) % m;
            faces.push(vec![i, j, m + j, m + i]);
        }
        let mut hull = Hull { points, faces };
        hull.wind_outward();
        hull
    }

    /// The box from `lo` to `hi`.
    pub(crate) fn boxed(lo: P3, hi: P3) -> Hull {
        let profile = [
            [lo[0], lo[2]],
            [hi[0], lo[2]],
            [hi[0], hi[2]],
            [lo[0], hi[2]],
        ];
        Hull::prism(&profile, Axis::Y, lo[1], hi[1])
    }

    fn wind_outward(&mut self) {
        let centre = centroid(&self.points);
        for face in &mut self.faces {
            let ring: Vec<P3> = face.iter().map(|&i| self.points[i as usize]).collect();
            if dot(newell(&ring), sub(centroid(&ring), centre)) < 0.0 {
                face.reverse();
            }
        }
    }

    /// The ring of face `i`.
    pub fn ring(&self, i: usize) -> Vec<P3> {
        self.faces[i]
            .iter()
            .map(|&p| self.points[p as usize])
            .collect()
    }

    /// Whether `p` is on or behind every face's plane.
    pub fn contains(&self, p: [f64; 3]) -> bool {
        (0..self.faces.len()).all(|i| {
            let ring = self.ring(i);
            let n = newell(&ring);
            let o = ring[0];
            let d = [
                p[0] - f64::from(o[0]),
                p[1] - f64::from(o[1]),
                p[2] - f64::from(o[2]),
            ];
            f64::from(n[0]) * d[0] + f64::from(n[1]) * d[1] + f64::from(n[2]) * d[2] <= 0.0
        })
    }

    pub fn volume(&self) -> f64 {
        (0..self.faces.len())
            .map(|i| ring_volume(&self.ring(i)))
            .sum()
    }
}

/// The signed volume under one outward ring by the divergence theorem:
/// positive when the ring is wound outward about a body that holds the
/// origin's side, summed over a closed surface it is the volume.
fn ring_volume(ring: &[P3]) -> f64 {
    let f = |p: P3| [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
    let a = f(ring[0]);
    let mut sum = 0.0;
    for w in ring[1..].windows(2) {
        let (b, c) = (f(w[0]), f(w[1]));
        let cross = [
            b[1] * c[2] - b[2] * c[1],
            b[2] * c[0] - b[0] * c[2],
            b[0] * c[1] - b[1] * c[0],
        ];
        sum += (a[0] * cross[0] + a[1] * cross[1] + a[2] * cross[2]) / 6.0;
    }
    sum
}

impl Solid {
    /// A solid that is one hull: its faces are the shell.
    pub(crate) fn convex(hull: Hull) -> Solid {
        let centre = centroid(&hull.points);
        let shell = (0..hull.faces.len())
            .map(|i| {
                let ring = hull.ring(i);
                let toward = sub(centroid(&ring), centre);
                Face::oriented(ring, toward, None)
            })
            .collect();
        Solid {
            shell,
            hulls: vec![hull],
        }
    }

    /// The volume inside the shell, in cubic world units.
    pub fn volume(&self) -> f64 {
        self.shell.iter().map(|f| ring_volume(&f.ring)).sum()
    }

    /// The least and greatest corner of the shell.
    pub fn bounds(&self) -> (P3, P3) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in self.shell.iter().flat_map(|f| &f.ring) {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }

    /// Every shell vertex's bits, face by face, the input of the golden
    /// hashes: ring length, then x y z of each vertex and u v, then the role.
    pub fn bits(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for f in &self.shell {
            out.extend((f.ring.len() as u32).to_le_bytes());
            for (p, uv) in f.ring.iter().zip(&f.uv) {
                for v in p.iter().chain(uv) {
                    out.extend(v.to_bits().to_le_bytes());
                }
            }
            out.push(f.role.index() as u8);
        }
        out
    }
}
