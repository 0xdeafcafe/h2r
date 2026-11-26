use crate::{Face, PieceDef, Shape, Solid};
use std::collections::HashMap;

/// One definition of each structural shape, with sizes chosen so that no
/// two axes are equal.
pub fn sample_defs() -> Vec<PieceDef> {
    vec![
        PieceDef::dev(Shape::Block, [96, 160, 224], 0),
        PieceDef::dev(Shape::Wall, [256, 16, 128], 0),
        PieceDef::dev(Shape::FloorPlate, [320, 192, 16], 0),
        PieceDef::dev(Shape::Ramp, [200, 80, 120], 0),
        PieceDef::dev(Shape::Wedge, [150, 90, 60], 0),
        PieceDef::dev(Shape::Stairs, [240, 100, 128], 8),
        PieceDef::dev(Shape::Stairs, [133, 41, 77], 7),
        PieceDef::dev(Shape::Cylinder, [100, 140, 180], 12),
        PieceDef::dev(Shape::Cylinder, [64, 64, 64], 16),
        PieceDef::dev(Shape::Arch, [256, 32, 200], 8),
        PieceDef::dev(Shape::Arch, [133, 20, 150], 5),
    ]
}

/// One definition of each natural shape, at its starting size.
pub fn natural_defs() -> Vec<PieceDef> {
    vec![
        PieceDef::dev(Shape::Boulder, [96, 96, 64], 1),
        PieceDef::dev(Shape::Slab, [256, 192, 32], 2),
        PieceDef::dev(Shape::Cliff, [256, 64, 256], 3),
        PieceDef::dev(Shape::TerrainWedge, [256, 256, 96], 4),
        PieceDef::dev(Shape::Spire, [64, 64, 256], 5),
    ]
}

pub fn solid(def: &PieceDef) -> Solid {
    def.generate().expect("a sample definition generates")
}

/// A fixed xorshift stream: the same points on every run.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    pub fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

pub type Key = [u32; 3];

pub fn key(p: [f32; 3]) -> Key {
    [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]
}

/// How many times each directed edge occurs in the shell.
pub fn edge_counts(shell: &[Face]) -> HashMap<(Key, Key), u32> {
    let mut edges = HashMap::new();
    for face in shell {
        for (i, a) in face.ring.iter().enumerate() {
            let b = face.ring[(i + 1) % face.ring.len()];
            *edges.entry((key(*a), key(b))).or_insert(0) += 1;
        }
    }
    edges
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn wide(p: [f32; 3]) -> [f64; 3] {
    [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
}

/// Whether `p` is inside the shell: a ray from it crosses the faces an odd
/// number of times (each face as a fan of triangles, Moller-Trumbore).
pub fn inside_shell(shell: &[Face], p: [f64; 3]) -> bool {
    let dir = [0.577_350_3, 0.371_113_7, 0.731_058_6];
    let mut hits = 0;
    for face in shell {
        let a = wide(face.ring[0]);
        for w in face.ring[1..].windows(2) {
            let (b, c) = (wide(w[0]), wide(w[1]));
            let (e1, e2) = (sub(b, a), sub(c, a));
            let h = cross(dir, e2);
            let det = dot(e1, h);
            if det.abs() < 1e-12 {
                continue;
            }
            let s = sub(p, a);
            let u = dot(s, h) / det;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q = cross(s, e1);
            let v = dot(dir, q) / det;
            if v < 0.0 || u + v > 1.0 {
                continue;
            }
            if dot(e2, q) / det > 0.0 {
                hits += 1;
            }
        }
    }
    hits % 2 == 1
}
