//! The icons on the markers' plates (docs/game/gametype-marker-forms.md,
//! "The kinds"): drawings of lines and filled triangles in a unit square,
//! u to the right and v up, 0 to 1, so they need no texture. Every stroke
//! is written here; none is copied from a game.
//!
//! [`Icon::mesh`] puts an icon in its plate's space: the plate lies in the
//! marker's front plane (y and z), facing +x, its centre at the origin,
//! and u runs along +y, which is the right of an eye on +x looking back.
//! [`plate`] is the plate with its icon and, for a hill, a territory or a
//! site, its number.
use std::f32::consts::TAU;

use crate::form::{ICON_HALF, ICON_LIFT, PLATE};
use crate::mesh::{Line, Mesh, Tri, Vec3};
use crate::shapes::{self, Plane};
use crate::{LINE_ALPHA, digits};

/// The alpha of an icon's filled shapes: solid enough to read at range.
pub const ICON_FILL_ALPHA: f32 = 0.7;
/// The lines of the plate's rim and the triangles of its disc.
pub const PLATE_SEGMENTS: usize = 16;

/// An icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    /// A figure inside a turning arrow: a respawn point.
    Respawn,
    /// A figure standing on a bar: a starting location.
    Start,
    /// A pennant on a pole: a flag stand, a territory.
    Pennant,
    /// A pennant over an arrow down: a flag return.
    FlagReturn,
    /// A ball with a fuse: the bomb's spawn.
    Bomb,
    /// A target: a bomb goal.
    Target,
    /// A ball: the ball's spawn.
    Ball,
    /// A crown: a hill.
    Crown,
    /// An arrow into a ring: a teleporter's sending end.
    Sender,
    /// An arrow out of a ring: a teleporter's receiving end.
    Receiver,
    /// A long gun.
    Weapon,
    /// A wedge with a cab on two wheels.
    Vehicle,
    /// A diamond.
    Equipment,
    /// An arrow down onto a bar: where a wave arrives.
    Drop,
    /// A house: a base.
    Base,
    /// A shield: a firing position.
    Shield,
    /// A pennant in a ring: a siege site.
    Site,
    /// A target with a diamond: the siege goal.
    Goal,
    /// A banner between two poles: a siege line.
    Banner,
}

/// An icon's drawing in its unit square.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Strokes {
    pub lines: Vec<[[f32; 2]; 2]>,
    pub tris: Vec<[[f32; 2]; 3]>,
}

type P = [f32; 2];

impl Strokes {
    fn line(&mut self, a: P, b: P) -> &mut Self {
        self.lines.push([a, b]);
        self
    }

    fn path(&mut self, points: &[P], closed: bool) -> &mut Self {
        for w in points.windows(2) {
            self.line(w[0], w[1]);
        }
        if closed && points.len() > 2 {
            self.line(points[points.len() - 1], points[0]);
        }
        self
    }

    /// The outline of a convex polygon and its fill.
    fn shape(&mut self, points: &[P]) -> &mut Self {
        self.path(points, true);
        for i in 1..points.len().saturating_sub(1) {
            self.tris.push([points[0], points[i], points[i + 1]]);
        }
        self
    }

    /// The points of an ellipse round `c` from angle `from` to `to`
    /// (turns), `n` segments.
    fn arc(c: P, r: [f32; 2], from: f32, to: f32, n: usize) -> Vec<P> {
        (0..=n)
            .map(|i| {
                let a = TAU * (from + (to - from) * i as f32 / n as f32);
                [c[0] + r[0] * a.cos(), c[1] + r[1] * a.sin()]
            })
            .collect()
    }

    fn ring(&mut self, c: P, r: [f32; 2], n: usize) -> &mut Self {
        let mut points = Self::arc(c, r, 0.0, 1.0, n);
        points.pop();
        self.path(&points, true)
    }

    fn disc(&mut self, c: P, r: f32, n: usize) -> &mut Self {
        let mut points = Self::arc(c, [r, r], 0.0, 1.0, n);
        points.pop();
        self.shape(&points)
    }

    /// A shaft from `from` to `to` and two wings back from the tip, each
    /// `wing` long, at a right angle to each other.
    fn arrow(&mut self, from: P, to: P, wing: f32) -> &mut Self {
        self.line(from, to);
        self.head(from, to, wing)
    }

    fn head(&mut self, from: P, to: P, wing: f32) -> &mut Self {
        let d = [to[0] - from[0], to[1] - from[1]];
        let len = d[0].hypot(d[1]).max(f32::EPSILON);
        let (dx, dy) = (d[0] / len * wing, d[1] / len * wing);
        let k = std::f32::consts::FRAC_1_SQRT_2;
        for s in [1.0, -1.0] {
            let back = [to[0] - k * (dx - s * dy), to[1] - k * (dy + s * dx)];
            self.line(to, back);
        }
        self
    }

    /// The standing figure of `kind::FIGURE`, its feet at `feet`, `scale`
    /// icon units a world unit.
    fn figure(&mut self, feet: P, scale: f32) -> &mut Self {
        let at = |p: &[f32; 2]| [feet[0] + p[0] * scale, feet[1] + p[1] * scale];
        let mut points = vec![at(&crate::kind::CROTCH)];
        points.extend(crate::kind::FIGURE.iter().map(at));
        points.extend(crate::kind::FIGURE.iter().rev().map(|p| at(&[-p[0], p[1]])));
        self.path(&points, true)
    }

    fn pennant(&mut self, foot: P, top: f32, flag: [f32; 2]) -> &mut Self {
        let tip = [foot[0], top];
        self.line(foot, tip);
        self.shape(&[
            tip,
            [foot[0] + flag[0], top - flag[1] / 2.0],
            [foot[0], top - flag[1]],
        ])
    }

    /// Every point the icon has.
    pub fn points(&self) -> impl Iterator<Item = P> + '_ {
        self.lines
            .iter()
            .flatten()
            .chain(self.tris.iter().flatten())
            .copied()
    }
}

impl Icon {
    pub const ALL: [Icon; 19] = [
        Icon::Respawn,
        Icon::Start,
        Icon::Pennant,
        Icon::FlagReturn,
        Icon::Bomb,
        Icon::Target,
        Icon::Ball,
        Icon::Crown,
        Icon::Sender,
        Icon::Receiver,
        Icon::Weapon,
        Icon::Vehicle,
        Icon::Equipment,
        Icon::Drop,
        Icon::Base,
        Icon::Shield,
        Icon::Site,
        Icon::Goal,
        Icon::Banner,
    ];

    /// The name the agent API uses.
    pub fn name(self) -> &'static str {
        match self {
            Icon::Respawn => "respawn",
            Icon::Start => "start",
            Icon::Pennant => "pennant",
            Icon::FlagReturn => "flag_return",
            Icon::Bomb => "bomb",
            Icon::Target => "target",
            Icon::Ball => "ball",
            Icon::Crown => "crown",
            Icon::Sender => "sender",
            Icon::Receiver => "receiver",
            Icon::Weapon => "weapon",
            Icon::Vehicle => "vehicle",
            Icon::Equipment => "equipment",
            Icon::Drop => "drop",
            Icon::Base => "base",
            Icon::Shield => "shield",
            Icon::Site => "site",
            Icon::Goal => "goal",
            Icon::Banner => "banner",
        }
    }

    /// The icon's drawing in its unit square.
    pub fn strokes(self) -> Strokes {
        let mut s = Strokes::default();
        match self {
            Icon::Respawn => {
                s.figure([0.5, 0.18], 0.85);
                // Most of a turn round the figure, its head at the top
                // right.
                let arc = Strokes::arc([0.5, 0.5], [0.4, 0.4], 0.32, 1.07, 18);
                s.path(&arc, false);
                let n = arc.len();
                s.head(arc[n - 2], arc[n - 1], 0.1);
            }
            Icon::Start => {
                s.figure([0.5, 0.2], 0.9);
                s.shape(&[[0.18, 0.06], [0.82, 0.06], [0.82, 0.16], [0.18, 0.16]]);
            }
            Icon::Pennant => {
                s.pennant([0.32, 0.08], 0.92, [0.5, 0.34]);
            }
            Icon::FlagReturn => {
                s.pennant([0.3, 0.5], 0.95, [0.42, 0.26]);
                s.arrow([0.62, 0.62], [0.62, 0.08], 0.14);
            }
            Icon::Bomb => {
                s.disc([0.44, 0.4], 0.3, 16);
                s.path(&[[0.63, 0.63], [0.72, 0.78], [0.84, 0.82]], false);
                s.line([0.84, 0.82], [0.92, 0.9]);
                s.line([0.84, 0.82], [0.94, 0.78]);
            }
            Icon::Target => {
                s.ring([0.5, 0.5], [0.45, 0.45], 20);
                s.ring([0.5, 0.5], [0.26, 0.26], 16);
                s.disc([0.5, 0.5], 0.08, 8);
            }
            Icon::Ball => {
                s.disc([0.5, 0.5], 0.4, 20);
                s.line([0.1, 0.5], [0.9, 0.5]);
                s.path(
                    &Strokes::arc([0.5, 0.5], [0.16, 0.4], -0.25, 0.25, 8),
                    false,
                );
            }
            Icon::Crown => {
                s.path(
                    &[
                        [0.1, 0.2],
                        [0.9, 0.2],
                        [0.9, 0.7],
                        [0.7, 0.42],
                        [0.5, 0.82],
                        [0.3, 0.42],
                        [0.1, 0.7],
                    ],
                    true,
                );
                s.tris.extend([
                    [[0.1, 0.2], [0.9, 0.2], [0.9, 0.42]],
                    [[0.1, 0.2], [0.9, 0.42], [0.1, 0.42]],
                    [[0.1, 0.42], [0.3, 0.42], [0.1, 0.7]],
                    [[0.3, 0.42], [0.7, 0.42], [0.5, 0.82]],
                    [[0.7, 0.42], [0.9, 0.42], [0.9, 0.7]],
                ]);
            }
            Icon::Sender => {
                s.ring([0.5, 0.22], [0.4, 0.14], 16);
                s.arrow([0.5, 0.94], [0.5, 0.26], 0.16);
            }
            Icon::Receiver => {
                s.ring([0.5, 0.22], [0.4, 0.14], 16);
                s.arrow([0.5, 0.26], [0.5, 0.94], 0.16);
            }
            Icon::Weapon => {
                let gun = crate::kind::WEAPON.map(|[x, z]| [0.5 + x * 1.6, 0.5 + (z - 0.34) * 3.0]);
                s.path(&gun, true);
            }
            Icon::Vehicle => {
                let body = crate::kind::VEHICLE
                    .map(|[x, z]| [0.5 + (x - 0.025) * 0.54, 0.5 + (z - 0.5) * 0.9]);
                s.path(&body, true);
                s.ring([0.27, 0.28], [0.08, 0.08], 8);
                s.ring([0.73, 0.28], [0.08, 0.08], 8);
            }
            Icon::Equipment => {
                s.shape(&[[0.5, 0.08], [0.85, 0.5], [0.5, 0.92], [0.15, 0.5]]);
                s.line([0.15, 0.5], [0.85, 0.5]);
            }
            Icon::Drop => {
                s.arrow([0.5, 0.94], [0.5, 0.3], 0.16);
                s.shape(&[[0.15, 0.08], [0.85, 0.08], [0.85, 0.18], [0.15, 0.18]]);
            }
            Icon::Base => {
                s.shape(&[
                    [0.2, 0.1],
                    [0.8, 0.1],
                    [0.8, 0.55],
                    [0.5, 0.88],
                    [0.2, 0.55],
                ]);
                s.path(
                    &[[0.42, 0.1], [0.42, 0.36], [0.58, 0.36], [0.58, 0.1]],
                    false,
                );
            }
            Icon::Shield => {
                s.shape(&[
                    [0.2, 0.88],
                    [0.8, 0.88],
                    [0.8, 0.48],
                    [0.5, 0.08],
                    [0.2, 0.48],
                ]);
                s.line([0.5, 0.88], [0.5, 0.08]);
            }
            Icon::Site => {
                s.ring([0.5, 0.5], [0.45, 0.45], 20);
                s.pennant([0.4, 0.24], 0.76, [0.32, 0.22]);
            }
            Icon::Goal => {
                s.ring([0.5, 0.5], [0.45, 0.45], 20);
                s.shape(&[[0.5, 0.26], [0.74, 0.5], [0.5, 0.74], [0.26, 0.5]]);
            }
            Icon::Banner => {
                s.line([0.15, 0.08], [0.15, 0.92]);
                s.line([0.85, 0.08], [0.85, 0.92]);
                s.shape(&[[0.15, 0.5], [0.85, 0.5], [0.85, 0.86], [0.15, 0.86]]);
            }
        }
        s
    }

    /// The icon in its plate's space, in `colour`: lines at full alpha,
    /// fills at [`ICON_FILL_ALPHA`], the square [`ICON_HALF`] each side of
    /// the centre and [`ICON_LIFT`] in front of the plate.
    pub fn mesh(self, colour: [f32; 3]) -> Mesh {
        strokes_mesh(&self.strokes(), [0.5, 0.5], 1.0, colour)
    }
}

/// `strokes` in the plate's space: the icon point `centre` at the plate's
/// centre, `scale` times the full size.
fn strokes_mesh(strokes: &Strokes, centre: P, scale: f32, colour: [f32; 3]) -> Mesh {
    let at = |[u, v]: P| -> Vec3 {
        [
            ICON_LIFT,
            (u - centre[0]) * 2.0 * ICON_HALF * scale,
            (v - centre[1]) * 2.0 * ICON_HALF * scale,
        ]
    };
    let [r, g, b] = colour;
    Mesh {
        lines: strokes
            .lines
            .iter()
            .map(|[a, z]| Line {
                a: at(*a),
                b: at(*z),
                colour: [r, g, b, LINE_ALPHA],
            })
            .collect(),
        tris: strokes
            .tris
            .iter()
            .map(|t| Tri::flat(t.map(at), [r, g, b, ICON_FILL_ALPHA]))
            .collect(),
    }
}

/// A plate in its own space (see the module): a disc of [`PLATE`] at
/// [`crate::FILL_ALPHA`] with its rim, holding `icon` in `colour`. With a
/// `number` the icon shrinks to the top left and the number's stroke
/// digits stand at the bottom right.
pub fn plate(icon: Icon, number: Option<u32>, colour: [f32; 3]) -> Mesh {
    let mut m = Mesh::default();
    shapes::ring(
        &mut m,
        [0.0; 3],
        PLATE,
        PLATE_SEGMENTS,
        Plane::Front,
        colour,
    );
    shapes::disc_in(
        &mut m,
        [0.0; 3],
        PLATE,
        PLATE_SEGMENTS,
        Plane::Front,
        colour,
    );
    match number {
        None => m.extend(icon.mesh(colour)),
        Some(n) => {
            // The icon in the top left 0.62 of the square.
            let small = 0.62;
            let corner = [0.5 / small, 1.0 - 0.5 / small];
            m.extend(strokes_mesh(&icon.strokes(), corner, small, colour));
            // The number in the bottom right, 0.4 of the square tall.
            let height = 0.4;
            let wide = digits::width(n) * height;
            let scale = if wide > 0.5 { 0.5 / wide } else { 1.0 };
            let origin = [0.96 - wide * scale, 0.04];
            let lines = digits::number(n, origin, height * scale);
            m.extend(strokes_mesh(
                &Strokes {
                    lines,
                    tris: Vec::new(),
                },
                [0.5, 0.5],
                1.0,
                colour,
            ));
        }
    }
    m
}
