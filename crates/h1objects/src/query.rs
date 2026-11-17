//! Point and ray queries on a collision BSP's 3D tree, in either game's
//! form, so a converted `coll` can be checked against Halo 1's answer.
//!
//! The walk is the engine's (docs/formats/collision.md, nodes): at a node
//! with plane `(n, d)` a point goes to the front child when `n.p - d >= 0`,
//! else to the back. A leaf is open space; a missing child (Halo 1's
//! `-1`, Halo 2's `0xffffff`) is solid, inside the object.

/// A 3D-tree child, either game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Child {
    Node(u32),
    Leaf(u32),
    Solid,
}

/// A BSP's 3D tree: node `i` as (plane, back, front).
pub trait Tree {
    fn node(&self, i: u32) -> Option<([f32; 4], Child, Child)>;
    fn node_count(&self) -> usize;
    fn root(&self) -> Child {
        if self.node_count() == 0 {
            Child::Solid
        } else {
            Child::Node(0)
        }
    }
}

impl Tree for tags::collision::Bsp {
    fn node(&self, i: u32) -> Option<([f32; 4], Child, Child)> {
        use tags::collision::Child as C;
        let n = self.nodes.get(i as usize)?;
        let plane = self.planes.get(n.plane as usize)?.0;
        let c = |c: C| match c {
            C::Node(n) => Child::Node(n),
            C::Leaf(l) => Child::Leaf(l),
            C::None => Child::Solid,
        };
        Some((plane, c(n.back), c(n.front)))
    }
    fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

impl Tree for h1map::bsp::CollisionBsp {
    fn node(&self, i: u32) -> Option<([f32; 4], Child, Child)> {
        use h1map::bsp::Child as C;
        let n = self.nodes.get(i as usize)?;
        let plane = *self.planes.get(n.plane as usize)?;
        let c = |c: C| match c {
            C::Node(n) => Child::Node(n),
            C::Leaf(l) => Child::Leaf(l),
            C::Solid => Child::Solid,
        };
        Some((plane, c(n.back), c(n.front)))
    }
    fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

fn side(plane: [f32; 4], p: [f32; 3]) -> f32 {
    plane[0] * p[0] + plane[1] * p[1] + plane[2] * p[2] - plane[3]
}

/// What holds `p`: `Leaf(i)` (open space) or `Solid`. `None` on a broken
/// tree (an index out of range, or a walk longer than the node count).
pub fn locate(t: &impl Tree, p: [f32; 3]) -> Option<Child> {
    let mut c = t.root();
    for _ in 0..=t.node_count() {
        match c {
            Child::Node(n) => {
                let (plane, back, front) = t.node(n)?;
                c = if side(plane, p) >= 0.0 { front } else { back };
            }
            done => return Some(done),
        }
    }
    None
}

/// The first point of the segment `a -> b` in solid space, as a fraction
/// of the segment (0 when `a` is solid), or `None` when the whole segment
/// is open. `Err` on a broken tree.
pub fn ray(t: &impl Tree, a: [f32; 3], b: [f32; 3]) -> Result<Option<f32>, &'static str> {
    let mut budget = 64 * (t.node_count() + 1);
    walk(t, t.root(), a, b, 0.0, 1.0, &mut budget)
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn walk(
    t: &impl Tree,
    c: Child,
    a: [f32; 3],
    b: [f32; 3],
    t0: f32,
    t1: f32,
    budget: &mut usize,
) -> Result<Option<f32>, &'static str> {
    *budget = budget.checked_sub(1).ok_or("ray walk over budget")?;
    let n = match c {
        Child::Solid => return Ok(Some(t0)),
        Child::Leaf(_) => return Ok(None),
        Child::Node(n) => n,
    };
    let (plane, back, front) = t.node(n).ok_or("node out of range")?;
    let (p0, p1) = (lerp(a, b, t0), lerp(a, b, t1));
    let (d0, d1) = (side(plane, p0), side(plane, p1));
    if d0 >= 0.0 && d1 >= 0.0 {
        return walk(t, front, a, b, t0, t1, budget);
    }
    if d0 < 0.0 && d1 < 0.0 {
        return walk(t, back, a, b, t0, t1, budget);
    }
    let tm = (t0 + (t1 - t0) * (d0 / (d0 - d1))).clamp(t0, t1);
    let (near, far) = if d0 >= 0.0 {
        (front, back)
    } else {
        (back, front)
    };
    if let Some(hit) = walk(t, near, a, b, t0, tm, budget)? {
        return Ok(Some(hit));
    }
    walk(t, far, a, b, tm, t1, budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use h1map::bsp::{Child as C, CollisionBsp, Leaf, Node3d};

    /// A unit cube, solid inside: six planes, each node's front open.
    pub(crate) fn cube() -> CollisionBsp {
        let planes = vec![
            [1.0, 0.0, 0.0, 1.0],
            [-1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, -1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
            [0.0, 0.0, -1.0, 1.0],
        ];
        let nodes = (0..6)
            .map(|i| Node3d {
                plane: i,
                front: C::Leaf(0),
                back: if i == 5 { C::Solid } else { C::Node(i + 1) },
            })
            .collect();
        CollisionBsp {
            nodes,
            planes,
            leaves: vec![Leaf {
                flags: 0,
                ref_count: 0,
                first_ref: u32::MAX,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn points_and_rays_on_a_cube() {
        let c = cube();
        assert_eq!(locate(&c, [0.0, 0.0, 0.0]), Some(Child::Solid));
        assert_eq!(locate(&c, [2.0, 0.0, 0.0]), Some(Child::Leaf(0)));
        let hit = ray(&c, [-3.0, 0.2, 0.1], [3.0, 0.2, 0.1]).unwrap().unwrap();
        assert!((hit - 2.0 / 6.0).abs() < 1e-6, "{hit}");
        assert_eq!(ray(&c, [-3.0, 2.0, 0.0], [3.0, 2.0, 0.0]).unwrap(), None);
        assert_eq!(
            ray(&c, [0.0, 0.0, 0.0], [3.0, 0.0, 0.0]).unwrap(),
            Some(0.0)
        );
        // The converted form answers the same.
        let h2 = h1convert::collision::convert(&c).unwrap();
        assert_eq!(locate(&h2, [0.0, 0.0, 0.0]), Some(Child::Solid));
        let hit2 = ray(&h2, [-3.0, 0.2, 0.1], [3.0, 0.2, 0.1])
            .unwrap()
            .unwrap();
        assert_eq!(hit, hit2);
    }
}
