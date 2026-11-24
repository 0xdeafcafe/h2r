//! A vehicle's flight along a labelled path: where it is at a tick and
//! which way it faces. Pure integer math in 16.16, so every machine and a
//! film compute the same pose from the same points, start and length
//! (docs/game/rules-world-api.md, "Vehicles and drop ships").
//!
//! The vehicle moves at one speed along the polyline of the path's points
//! in number order, so it covers the path's length in exactly the stated
//! ticks and is at the last point on the last one. Its facing is the unit
//! tangent of the segment it is on; a point where two segments meet
//! belongs to the second, and the last point to the last segment.

/// A position or a direction in 16.16 world units.
pub type Vec3 = [i32; 3];

/// One in 16.16: the length of a unit tangent.
pub const ONE: i32 = 1 << 16;

/// The most points a path holds (`ty::MAX_LABELLED`).
pub const MAX_POINTS: usize = crate::ty::MAX_LABELLED;

/// Why a flight cannot be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("a path has at least 2 points")]
    TooFewPoints,
    #[error("a path has at most 256 points")]
    TooManyPoints,
    #[error("a flight takes at least 1 tick")]
    NoTicks,
}

/// Where a vehicle is and which way it faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pose {
    pub position: Vec3,
    /// The unit tangent, each axis in 16.16.
    pub facing: Vec3,
}

/// A flight: a path's points, the tick it started on and the ticks it
/// takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flight {
    points: Vec<Vec3>,
    /// Each segment's length, in 16.16.
    lengths: Vec<u64>,
    pub start: u32,
    pub ticks: u32,
}

/// The integer square root: the largest `r` with `r * r <= n`.
fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = n.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

fn length(a: Vec3, b: Vec3) -> u64 {
    let sq: u128 = (0..3)
        .map(|i| {
            let d = (b[i] as i128 - a[i] as i128).unsigned_abs();
            d * d
        })
        .sum();
    isqrt(sq) as u64
}

impl Flight {
    /// A flight along `points` in order, begun on tick `start`, taking
    /// `ticks`.
    pub fn new(points: Vec<Vec3>, start: u32, ticks: u32) -> Result<Flight, Refusal> {
        if points.len() < 2 {
            return Err(Refusal::TooFewPoints);
        }
        if points.len() > MAX_POINTS {
            return Err(Refusal::TooManyPoints);
        }
        if ticks == 0 {
            return Err(Refusal::NoTicks);
        }
        let lengths = points.windows(2).map(|w| length(w[0], w[1])).collect();
        Ok(Flight {
            points,
            lengths,
            start,
            ticks,
        })
    }

    /// The path's length, in 16.16 world units.
    pub fn total(&self) -> u64 {
        self.lengths.iter().sum()
    }

    /// The tick the vehicle is at the last point.
    pub fn end(&self) -> u32 {
        self.start.saturating_add(self.ticks)
    }

    /// Whether the flight is over by `tick`.
    pub fn done(&self, tick: u32) -> bool {
        tick >= self.end()
    }

    /// The pose at `tick`: the first point before the start, the last
    /// point from the end on. A path whose points all coincide faces
    /// along +x.
    pub fn at(&self, tick: u32) -> Pose {
        let elapsed = tick.saturating_sub(self.start).min(self.ticks) as u128;
        let total = self.total() as u128;
        let along = total * elapsed / self.ticks as u128;
        // The segment `along` falls in, and the distance before it. The
        // point where two segments meet belongs to the second, and the
        // end of the path to the last segment with length.
        let mut base = 0u128;
        let mut seg = None;
        for (i, &len) in self.lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            seg = Some((i, base));
            let end = base + len as u128;
            if along < end {
                break;
            }
            base = end;
        }
        let Some((i, base)) = seg else {
            return Pose {
                position: self.points[0],
                facing: [ONE, 0, 0],
            };
        };
        let len = self.lengths[i] as i128;
        let into = (along.saturating_sub(base) as i128).min(len);
        let (a, b) = (self.points[i], self.points[i + 1]);
        let mut position = [0; 3];
        let mut facing = [0; 3];
        for k in 0..3 {
            let d = b[k] as i128 - a[k] as i128;
            position[k] = (a[k] as i128 + d * into / len) as i32;
            facing[k] = (d * ONE as i128 / len) as i32;
        }
        Pose { position, facing }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(n: i32) -> i32 {
        n * ONE
    }

    fn line() -> Flight {
        // 10 units along +x, then 10 along +y: 20 in all.
        Flight::new(vec![[0, 0, 0], [w(10), 0, 0], [w(10), w(10), 0]], 5, 20).unwrap()
    }

    #[test]
    fn a_flight_needs_two_points_and_a_tick() {
        assert_eq!(Flight::new(vec![[0; 3]], 0, 10), Err(Refusal::TooFewPoints));
        assert_eq!(
            Flight::new(vec![[0; 3], [1, 0, 0]], 0, 0),
            Err(Refusal::NoTicks)
        );
        assert_eq!(
            Flight::new(vec![[0; 3]; MAX_POINTS + 1], 0, 1),
            Err(Refusal::TooManyPoints)
        );
    }

    #[test]
    fn it_is_at_the_first_point_until_it_starts_and_the_last_when_it_ends() {
        let f = line();
        assert_eq!(f.total(), w(20) as u64);
        assert_eq!(f.at(0).position, [0, 0, 0]);
        assert_eq!(f.at(5).position, [0, 0, 0]);
        assert_eq!(f.end(), 25);
        assert!(!f.done(24) && f.done(25));
        assert_eq!(f.at(25).position, [w(10), w(10), 0]);
        assert_eq!(f.at(900).position, [w(10), w(10), 0]);
    }

    #[test]
    fn it_covers_equal_lengths_in_equal_ticks() {
        let f = line();
        // One world unit a tick.
        for t in 0..=20u32 {
            let p = f.at(5 + t).position;
            let want = if t <= 10 {
                [w(t as i32), 0, 0]
            } else {
                [w(10), w(t as i32 - 10), 0]
            };
            assert_eq!(p, want, "tick {t}");
        }
    }

    #[test]
    fn it_faces_the_tangent_of_its_segment() {
        let f = line();
        assert_eq!(f.at(5).facing, [ONE, 0, 0]);
        assert_eq!(f.at(14).facing, [ONE, 0, 0]);
        // The corner belongs to the second segment.
        assert_eq!(f.at(15).facing, [0, ONE, 0]);
        assert_eq!(f.at(25).facing, [0, ONE, 0]);
    }

    #[test]
    fn a_diagonal_has_a_unit_tangent() {
        let f = Flight::new(vec![[0; 3], [w(3), 0, w(4)]], 0, 5).unwrap();
        let t = f.at(2).facing;
        // 3-4-5: (0.6, 0, 0.8) in 16.16, truncated.
        assert_eq!(t, [39321, 0, 52428]);
        assert_eq!(f.at(5).position, [w(3), 0, w(4)]);
    }

    #[test]
    fn coincident_points_are_skipped_and_a_still_path_faces_along_x() {
        let f = Flight::new(vec![[0; 3], [0; 3], [w(4), 0, 0]], 0, 4).unwrap();
        assert_eq!(f.at(2).position, [w(2), 0, 0]);
        assert_eq!(f.at(2).facing, [ONE, 0, 0]);
        let still = Flight::new(vec![[7, 8, 9]; 3], 0, 4).unwrap();
        assert_eq!(
            still.at(2),
            Pose {
                position: [7, 8, 9],
                facing: [ONE, 0, 0]
            }
        );
        // A trailing run of coincident points ends on the last real segment.
        let tail = Flight::new(vec![[0; 3], [w(4), 0, 0], [w(4), 0, 0]], 0, 4).unwrap();
        assert_eq!(tail.at(4).position, [w(4), 0, 0]);
        assert_eq!(tail.at(4).facing, [ONE, 0, 0]);
    }

    #[test]
    fn the_far_corners_of_the_world_do_not_overflow() {
        let f = Flight::new(
            vec![
                [i32::MIN, i32::MIN, i32::MIN],
                [i32::MAX, i32::MAX, i32::MAX],
            ],
            0,
            u32::MAX,
        )
        .unwrap();
        assert_eq!(f.end(), u32::MAX);
        assert_eq!(f.at(u32::MAX).position, [i32::MAX; 3]);
        assert_eq!(f.at(0).position, [i32::MIN; 3]);
    }

    #[test]
    fn isqrt_is_the_floor_root() {
        for n in [
            0u128,
            1,
            2,
            3,
            4,
            8,
            9,
            10,
            1 << 64,
            (1 << 64) + 1,
            u128::MAX >> 1,
        ] {
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1) * (r + 1) > n, "{n}");
        }
    }
}
