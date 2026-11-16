//! Point and segment queries on a collision BSP's 3D tree, written once over
//! both games' trees so a converted BSP can be checked against its source:
//! each side reads its own encoding through [`SolidTree`].

/// Where a 3D child leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kid {
    /// Another node.
    Node(usize),
    /// A leaf: open space.
    Leaf(usize),
    /// Solid.
    Solid,
}

/// A 3D BSP tree as each game stores it.
pub trait SolidTree {
    /// Node count; the root is node 0.
    fn node_count(&self) -> usize;
    /// A node's plane `[nx, ny, nz, d]` (`n.p - d`) and its back (negative
    /// side) and front children.
    fn node(&self, i: usize) -> Option<([f32; 4], Kid, Kid)>;
}

impl SolidTree for h1map::bsp::CollisionBsp {
    fn node_count(&self) -> usize {
        self.nodes.len()
    }
    fn node(&self, i: usize) -> Option<([f32; 4], Kid, Kid)> {
        use h1map::bsp::Child;
        let n = self.nodes.get(i)?;
        let kid = |c| match c {
            Child::Node(n) => Kid::Node(n as usize),
            Child::Leaf(l) => Kid::Leaf(l as usize),
            Child::Solid => Kid::Solid,
        };
        Some((
            *self.planes.get(n.plane as usize)?,
            kid(n.back),
            kid(n.front),
        ))
    }
}

impl SolidTree for tags::collision::Bsp {
    fn node_count(&self) -> usize {
        self.nodes.len()
    }
    fn node(&self, i: usize) -> Option<([f32; 4], Kid, Kid)> {
        use tags::collision::Child;
        let n = self.nodes.get(i)?;
        let kid = |c| match c {
            Child::Node(n) => Kid::Node(n as usize),
            Child::Leaf(l) => Kid::Leaf(l as usize),
            Child::None => Kid::Solid,
        };
        Some((
            self.planes.get(n.plane as usize)?.0,
            kid(n.back),
            kid(n.front),
        ))
    }
}

fn dist(p: [f32; 4], x: [f64; 3]) -> f64 {
    p[0] as f64 * x[0] + p[1] as f64 * x[1] + p[2] as f64 * x[2] - p[3] as f64
}

fn root<T: SolidTree + ?Sized>(t: &T) -> Kid {
    if t.node_count() == 0 {
        Kid::Solid
    } else {
        Kid::Node(0)
    }
}

/// The leaf holding `p` (`Some`), or `None` in solid space or a broken tree.
pub fn leaf_at<T: SolidTree + ?Sized>(t: &T, p: [f32; 3]) -> Option<usize> {
    let x = p.map(f64::from);
    let mut k = root(t);
    for _ in 0..=t.node_count() {
        match k {
            Kid::Solid => return None,
            Kid::Leaf(l) => return Some(l),
            Kid::Node(n) => {
                let (plane, back, front) = t.node(n)?;
                k = if dist(plane, x) >= 0.0 { front } else { back };
            }
        }
    }
    None
}

/// Where the segment `a`→`b` first enters solid space, as a fraction of its
/// length, or `None` when it stays in open space. A segment that starts in
/// solid hits at 0.
pub fn first_hit<T: SolidTree + ?Sized>(t: &T, a: [f32; 3], b: [f32; 3]) -> Option<f64> {
    let (a, b) = (a.map(f64::from), b.map(f64::from));
    hit(t, root(t), a, b, 0.0, 1.0, t.node_count() + 1)
}

fn at(a: [f64; 3], b: [f64; 3], s: f64) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * s)
}

fn hit<T: SolidTree + ?Sized>(
    t: &T,
    k: Kid,
    a: [f64; 3],
    b: [f64; 3],
    t0: f64,
    t1: f64,
    depth: usize,
) -> Option<f64> {
    if depth == 0 {
        return None;
    }
    let n = match k {
        Kid::Solid => return Some(t0),
        Kid::Leaf(_) => return None,
        Kid::Node(n) => n,
    };
    let (plane, back, front) = t.node(n)?;
    let (d0, d1) = (dist(plane, at(a, b, t0)), dist(plane, at(a, b, t1)));
    if d0 >= 0.0 && d1 >= 0.0 {
        return hit(t, front, a, b, t0, t1, depth - 1);
    }
    if d0 < 0.0 && d1 < 0.0 {
        return hit(t, back, a, b, t0, t1, depth - 1);
    }
    let mid = t0 + (t1 - t0) * (d0 / (d0 - d1));
    let (near, far) = if d0 >= 0.0 {
        (front, back)
    } else {
        (back, front)
    };
    hit(t, near, a, b, t0, mid, depth - 1).or_else(|| hit(t, far, a, b, mid, t1, depth - 1))
}
