//! The primitives the markers are made of. Each adds to a mesh, in the
//! colour given: lines at [`LINE_ALPHA`], faces at [`FILL_ALPHA`]. The
//! doc of each gives the lines and triangles it adds, which
//! `tests::every_kind_has_its_lines_faces_and_bounds` counts on.
use std::f32::consts::TAU;

use crate::area::{Area, Shape};
use crate::form::{WALL_HIGH, WALL_LOW};
use crate::mesh::{Line, Mesh, Tri, Vec3};
use crate::{FILL_ALPHA, LINE_ALPHA};

/// The plane a flat shape lies in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plane {
    /// x and y: on the floor.
    Floor,
    /// x and z: seen from the marker's side.
    Side,
    /// y and z: seen from the marker's front.
    Front,
}

impl Plane {
    /// The point of the plane's `(u, v)` through `centre`.
    pub fn point(self, centre: Vec3, u: f32, v: f32) -> Vec3 {
        let [x, y, z] = centre;
        match self {
            Plane::Floor => [x + u, y + v, z],
            Plane::Side => [x + u, y, z + v],
            Plane::Front => [x, y + u, z + v],
        }
    }
}

fn line_colour(c: [f32; 3]) -> [f32; 4] {
    [c[0], c[1], c[2], LINE_ALPHA]
}

fn fill_colour(c: [f32; 3]) -> [f32; 4] {
    [c[0], c[1], c[2], FILL_ALPHA]
}

/// `n` points round a circle of `radius`, counter-clockwise from +x.
pub fn circle(radius: f32, n: usize) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let (s, c) = (TAU * i as f32 / n as f32).sin_cos();
            [radius * c, radius * s]
        })
        .collect()
}

/// One line.
pub fn line(mesh: &mut Mesh, a: Vec3, b: Vec3, colour: [f32; 3]) {
    mesh.lines.push(Line {
        a,
        b,
        colour: line_colour(colour),
    });
}

/// The lines joining `points` in order, back to the first when `closed`:
/// one line a point when closed, one fewer when open.
pub fn path(mesh: &mut Mesh, points: &[Vec3], closed: bool, colour: [f32; 3]) {
    for pair in points.windows(2) {
        line(mesh, pair[0], pair[1], colour);
    }
    if closed && points.len() > 2 {
        line(mesh, points[points.len() - 1], points[0], colour);
    }
}

/// The faces of the flat polygon `points`, as a fan from its first
/// point: two fewer triangles than points.
pub fn fan(mesh: &mut Mesh, points: &[Vec3], colour: [f32; 3]) {
    for i in 1..points.len().saturating_sub(1) {
        mesh.tris.push(Tri::flat(
            [points[0], points[i], points[i + 1]],
            fill_colour(colour),
        ));
    }
}

/// A ring of `n` lines in `plane`.
pub fn ring(mesh: &mut Mesh, centre: Vec3, radius: f32, n: usize, plane: Plane, colour: [f32; 3]) {
    let points: Vec<Vec3> = circle(radius, n)
        .into_iter()
        .map(|[u, v]| plane.point(centre, u, v))
        .collect();
    path(mesh, &points, true, colour);
}

/// A filled disc on the floor: `n` triangles round its centre.
pub fn disc(mesh: &mut Mesh, centre: Vec3, radius: f32, n: usize, colour: [f32; 3]) {
    disc_in(mesh, centre, radius, n, Plane::Floor, colour);
}

/// A filled disc in `plane`: `n` triangles round its centre.
pub fn disc_in(
    mesh: &mut Mesh,
    centre: Vec3,
    radius: f32,
    n: usize,
    plane: Plane,
    colour: [f32; 3],
) {
    let rim = circle(radius, n);
    for i in 0..n {
        let (a, b) = (rim[i], rim[(i + 1) % n]);
        mesh.tris.push(Tri::flat(
            [
                centre,
                plane.point(centre, a[0], a[1]),
                plane.point(centre, b[0], b[1]),
            ],
            fill_colour(colour),
        ));
    }
}

/// A pad a marker stands on: a ring of `n` lines and its disc of `n`
/// triangles, lifted off the floor.
pub fn pad(mesh: &mut Mesh, radius: f32, n: usize, colour: [f32; 3]) {
    let centre = [0.0, 0.0, crate::LIFT];
    ring(mesh, centre, radius, n, Plane::Floor, colour);
    disc(mesh, centre, radius, n, colour);
}

/// The upright prism over the floor outline `outline` between the
/// heights `low` and `high`: for an outline of k points, 3k lines (the
/// bottom, the top and a post at each point) and 2k triangles (the
/// walls). It has no floor and no lid.
pub fn prism(mesh: &mut Mesh, outline: &[[f32; 2]], low: f32, high: f32, colour: [f32; 3]) {
    path(mesh, &level(outline, low), true, colour);
    sides(mesh, outline, low, high, colour);
}

/// A prism with no bottom edge: for an outline of k points, 2k lines (the
/// top and a post at each point) and 2k triangles. A teleporter's column
/// stands on the portal's ring, which is drawn once.
pub fn sides(mesh: &mut Mesh, outline: &[[f32; 2]], low: f32, high: f32, colour: [f32; 3]) {
    let (bottom, top) = (level(outline, low), level(outline, high));
    path(mesh, &top, true, colour);
    let k = outline.len();
    for i in 0..k {
        line(mesh, bottom[i], top[i], colour);
        let j = (i + 1) % k;
        for points in [[bottom[i], bottom[j], top[j]], [bottom[i], top[j], top[i]]] {
            mesh.tris.push(Tri::flat(points, fill_colour(colour)));
        }
    }
}

/// The points of `outline` at the height `z`.
pub fn level(outline: &[[f32; 2]], z: f32) -> Vec<Vec3> {
    outline.iter().map(|p| [p[0], p[1], z]).collect()
}

/// The points of `outline` scaled by `scale` round the origin.
pub fn shrunk(outline: &[[f32; 2]], scale: f32) -> Vec<[f32; 2]> {
    outline
        .iter()
        .map(|p| [p[0] * scale, p[1] * scale])
        .collect()
}

/// How often a wall stands a post on the outline of `area`: every
/// fourth point of a circle, every point (a corner) of anything else.
pub fn post_step(area: &Area) -> usize {
    match area.shape {
        Shape::Radius(_) => 4,
        _ => 1,
    }
}

/// The boundary of `area` (docs/game/gametype-marker-forms.md,
/// "Boundaries"), the same in the edit and the play form: [`wall_of`]
/// its outline from its lowest to its highest.
pub fn wall(mesh: &mut Mesh, area: &Area, colour: [f32; 3]) {
    if let Shape::Sphere(radius) = area.shape {
        sphere_wall(mesh, &area.outline(), radius, colour);
        return;
    }
    wall_of(
        mesh,
        &area.outline(),
        -area.below,
        area.above,
        post_step(area),
        colour,
    );
}

/// The boundary of a sphere of `radius` whose equator is `equator`: the
/// equator and two rings at 0.7 of the radius above and below it, each at
/// [`wall_of`]'s line alpha, and a meridian through the three rings at every
/// fourth point. A sphere has no floor to stand a wall on, so it has no skin.
fn sphere_wall(mesh: &mut Mesh, equator: &[[f32; 2]], radius: f32, colour: [f32; 3]) {
    const RING: f32 = 0.7;
    let across = (1.0 - RING * RING).sqrt();
    let upper = level(&shrunk(equator, across), RING * radius);
    let middle = level(equator, 0.0);
    let lower = level(&shrunk(equator, across), -RING * radius);
    for ring in [&upper, &middle, &lower] {
        path(mesh, ring, true, colour);
    }
    for i in (0..equator.len()).step_by(4) {
        line(mesh, lower[i], middle[i], colour);
        line(mesh, middle[i], upper[i], colour);
    }
}

/// A wall over the floor outline `outline` between `low` and `high`:
///
/// - the outline at `low` at full alpha, the edge on the floor: k lines;
/// - the outline at `high` at a third of it: k lines;
/// - a post from `low` to `high` at every `step`th point: k / `step`
///   lines, rounded up;
/// - the translucent wall, two triangles an edge, each corner's alpha
///   [`WALL_LOW`] at `low` and [`WALL_HIGH`] at `high`, so the alpha
///   falls evenly up the wall: 2k triangles.
///
/// It has no floor and no lid.
pub fn wall_of(
    mesh: &mut Mesh,
    outline: &[[f32; 2]],
    low: f32,
    high: f32,
    step: usize,
    colour: [f32; 3],
) {
    let (bottom, top) = (level(outline, low), level(outline, high));
    let [r, g, b] = colour;
    path(mesh, &bottom, true, colour);
    let faint = [r, g, b, LINE_ALPHA / 3.0];
    let k = outline.len();
    for i in 0..k {
        mesh.lines.push(Line {
            a: top[i],
            b: top[(i + 1) % k],
            colour: faint,
        });
    }
    for i in (0..k).step_by(step.max(1)) {
        line(mesh, bottom[i], top[i], colour);
    }
    let (lo, hi) = ([r, g, b, WALL_LOW], [r, g, b, WALL_HIGH]);
    for i in 0..k {
        let j = (i + 1) % k;
        mesh.tris.push(Tri {
            points: [bottom[i], bottom[j], top[j]],
            colours: [lo, lo, hi],
        });
        mesh.tris.push(Tri {
            points: [bottom[i], top[j], top[i]],
            colours: [lo, hi, hi],
        });
    }
}

/// An arrow of three lines: the shaft from `from` to `to` and two wings
/// back from the tip, each a quarter of the shaft long, spread along
/// `side` (a unit vector across the shaft).
pub fn arrow(mesh: &mut Mesh, from: Vec3, to: Vec3, side: Vec3, colour: [f32; 3]) {
    line(mesh, from, to, colour);
    let wing: Vec3 = std::array::from_fn(|i| (to[i] - from[i]) * 0.25);
    let length = wing.iter().map(|c| c * c).sum::<f32>().sqrt();
    for sign in [1.0, -1.0] {
        let back: Vec3 = std::array::from_fn(|i| to[i] - wing[i] + sign * length * side[i]);
        line(mesh, to, back, colour);
    }
}

/// An octahedron of `radius` round `centre`: 12 lines and 8 triangles.
pub fn diamond(mesh: &mut Mesh, centre: Vec3, radius: f32, colour: [f32; 3]) {
    let [x, y, z] = centre;
    let (top, bottom) = ([x, y, z + radius], [x, y, z - radius]);
    let belt = [
        [x + radius, y, z],
        [x, y + radius, z],
        [x - radius, y, z],
        [x, y - radius, z],
    ];
    path(mesh, &belt, true, colour);
    for i in 0..4 {
        let (a, b) = (belt[i], belt[(i + 1) % 4]);
        line(mesh, a, top, colour);
        line(mesh, a, bottom, colour);
        for points in [[a, b, top], [b, a, bottom]] {
            mesh.tris.push(Tri::flat(points, fill_colour(colour)));
        }
    }
}

/// A ball as three rings of `n` lines, one in each plane: 3n lines.
pub fn ball(mesh: &mut Mesh, centre: Vec3, radius: f32, n: usize, colour: [f32; 3]) {
    for plane in [Plane::Floor, Plane::Side, Plane::Front] {
        ring(mesh, centre, radius, n, plane, colour);
    }
}

/// A box of half sizes `half` round the origin: 12 lines and 12
/// triangles.
pub fn cuboid(mesh: &mut Mesh, half: Vec3, colour: [f32; 3]) {
    let [x, y, z] = half;
    let outline = [[-x, -y], [x, -y], [x, y], [-x, y]];
    // The four walls: 12 lines and 8 triangles.
    prism(mesh, &outline, -z, z, colour);
    // The floor and the lid: 2 triangles each.
    for h in [-z, z] {
        let face: Vec<Vec3> = outline.iter().map(|p| [p[0], p[1], h]).collect();
        fan(mesh, &face, colour);
    }
}
