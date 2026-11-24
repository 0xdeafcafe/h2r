//! A marker's geometry: coloured lines and translucent triangles.

pub type Vec3 = [f32; 3];

/// One line segment. The colour is straight RGBA, 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub a: Vec3,
    pub b: Vec3,
    pub colour: [f32; 4],
}

/// One filled triangle, drawn from both sides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tri {
    pub points: [Vec3; 3],
    pub colour: [f32; 4],
}

/// Lines and triangles in one space.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub lines: Vec<Line>,
    pub tris: Vec<Tri>,
}

/// Where a marker is: its origin and its three axes in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub at: Vec3,
    /// The world direction of the marker's x.
    pub forward: Vec3,
    /// The world direction of the marker's y.
    pub left: Vec3,
    /// The world direction of the marker's z.
    pub up: Vec3,
}

impl Place {
    /// Upright at `at`, facing `yaw` radians round z from the world's x.
    pub fn facing(at: Vec3, yaw: f32) -> Self {
        let (s, c) = yaw.sin_cos();
        Self {
            at,
            forward: [c, s, 0.0],
            left: [-s, c, 0.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    /// The world point of the marker's point `p`.
    pub fn point(&self, p: Vec3) -> Vec3 {
        std::array::from_fn(|i| {
            self.at[i] + p[0] * self.forward[i] + p[1] * self.left[i] + p[2] * self.up[i]
        })
    }
}

impl Mesh {
    /// The vertices a drawer uploads: two a line, three a triangle.
    pub fn vertex_count(&self) -> usize {
        self.lines.len() * 2 + self.tris.len() * 3
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.tris.is_empty()
    }

    /// Every point of the mesh.
    pub fn points(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.lines
            .iter()
            .flat_map(|l| [l.a, l.b])
            .chain(self.tris.iter().flat_map(|t| t.points))
    }

    /// The least and greatest corner of the box round every point;
    /// `None` for an empty mesh.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut points = self.points();
        let first = points.next()?;
        Some(points.fold((first, first), |(min, max), p| {
            (
                std::array::from_fn(|i| min[i].min(p[i])),
                std::array::from_fn(|i| max[i].max(p[i])),
            )
        }))
    }

    /// The same mesh at `place`.
    pub fn placed(&self, place: &Place) -> Mesh {
        Mesh {
            lines: self
                .lines
                .iter()
                .map(|l| Line {
                    a: place.point(l.a),
                    b: place.point(l.b),
                    colour: l.colour,
                })
                .collect(),
            tris: self
                .tris
                .iter()
                .map(|t| Tri {
                    points: t.points.map(|p| place.point(p)),
                    colour: t.colour,
                })
                .collect(),
        }
    }

    /// Adds `other`'s lines and triangles.
    pub fn extend(&mut self, other: Mesh) {
        self.lines.extend(other.lines);
        self.tris.extend(other.tris);
    }

    /// Scales every colour's alpha by `alpha`: a marker that fades with
    /// distance, or one drawn dim.
    pub fn faded(mut self, alpha: f32) -> Mesh {
        for l in &mut self.lines {
            l.colour[3] *= alpha;
        }
        for t in &mut self.tris {
            t.colour[3] *= alpha;
        }
        self
    }
}
