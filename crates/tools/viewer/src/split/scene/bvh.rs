//! Two-sided, finite-segment intersection against posed render triangles.
use super::mask::Coverage;
use scenario_edit::geometry::{Vec3, cross, dot, sub};

#[derive(Clone)]
pub(super) struct Triangle {
    pub positions: [Vec3; 3],
    pub uv: [[f32; 2]; 3],
    pub material: usize,
}

impl Triangle {
    fn blocks(&self, origin: Vec3, dir: Vec3, limit: f32, materials: &[Coverage]) -> bool {
        let [a, b, c] = self.positions;
        let e = sub(b, a);
        let f = sub(c, a);
        let p = cross(dir, f);
        let determinant = dot(e, p);
        if determinant.abs() < 1e-12 {
            return false;
        }
        let t = sub(origin, a);
        let u = dot(t, p) / determinant;
        if !(0.0..=1.0).contains(&u) {
            return false;
        }
        let q = cross(t, e);
        let v = dot(dir, q) / determinant;
        let distance = dot(f, q) / determinant;
        if v < 0.0 || u + v > 1.0 || distance <= 0.0 || distance >= limit {
            return false;
        }
        let uv = std::array::from_fn(|k| {
            self.uv[0][k] * (1.0 - u - v) + self.uv[1][k] * u + self.uv[2][k] * v
        });
        materials[self.material].covers(uv)
    }
}

struct Node {
    lo: Vec3,
    hi: Vec3,
    content: Content,
}
enum Content {
    Children([usize; 2]),
    Leaf(std::ops::Range<usize>),
}

impl Node {
    fn intersects(&self, origin: Vec3, dir: Vec3, mut far: f32) -> bool {
        let mut near: f32 = 0.0;
        for k in 0..3 {
            if dir[k] == 0.0 {
                if origin[k] < self.lo[k] || origin[k] > self.hi[k] {
                    return false;
                }
            } else {
                let t0 = (self.lo[k] - origin[k]) / dir[k];
                let t1 = (self.hi[k] - origin[k]) / dir[k];
                near = near.max(t0.min(t1));
                far = far.min(t0.max(t1));
                if near > far {
                    return false;
                }
            }
        }
        true
    }
}

pub(super) struct Bvh {
    triangles: Vec<Triangle>,
    nodes: Vec<Node>,
}

impl Bvh {
    pub fn new(mut triangles: Vec<Triangle>) -> Self {
        let mut nodes = Vec::new();
        if !triangles.is_empty() {
            Self::partition(&mut triangles, 0, &mut nodes);
        }
        Self { triangles, nodes }
    }

    fn partition(tris: &mut [Triangle], start: usize, nodes: &mut Vec<Node>) -> usize {
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for p in tris.iter().flat_map(|t| t.positions) {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let id = nodes.len();
        nodes.push(Node {
            lo,
            hi,
            content: Content::Leaf(start..(start + tris.len())),
        });
        if tris.len() > 8 {
            let axis = (0..3)
                .max_by(|&i, &j| (hi[i] - lo[i]).total_cmp(&(hi[j] - lo[j])))
                .unwrap();
            let middle = tris.len() / 2;
            let centroid = |t: &Triangle| t.positions.iter().map(|p| p[axis]).sum::<f32>();
            tris.select_nth_unstable_by(middle, |a, b| centroid(a).total_cmp(&centroid(b)));
            let (a, b) = tris.split_at_mut(middle);
            let children = [
                Self::partition(a, start, nodes),
                Self::partition(b, start + middle, nodes),
            ];
            nodes[id].content = Content::Children(children);
        }
        id
    }

    pub fn blocked(&self, origin: Vec3, dir: Vec3, limit: f32, materials: &[Coverage]) -> bool {
        !self.nodes.is_empty() && self.hit(0, origin, dir, limit, materials)
    }

    fn hit(&self, id: usize, p: Vec3, d: Vec3, limit: f32, materials: &[Coverage]) -> bool {
        let n = &self.nodes[id];
        n.intersects(p, d, limit)
            && match &n.content {
                Content::Children(ids) => ids.iter().any(|&i| self.hit(i, p, d, limit, materials)),
                Content::Leaf(range) => self.triangles[range.clone()]
                    .iter()
                    .any(|t| t.blocks(p, d, limit, materials)),
            }
    }
}
