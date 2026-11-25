//! The bounce region of one site in input space
//! (docs/runtime/bounce-search.md, "The region and the ease"): each
//! point's class over the Havok variants, the region's extent, and the
//! window: the largest cube of human-sized steps around a bounce that
//! holds no failure.

use serde::{Deserialize, Serialize};

use super::{N, Vector};
use crate::drops::Outcome;

/// What one point did over the variants it ran on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Bounced on every variant: provable.
    All,
    /// Bounced on one at least, and not on every one (it did not bounce
    /// on another, or another has no outcome).
    Some,
    /// Bounced on none, and touched down primed on one at least.
    None,
    /// No outcome on any variant (the prime was lost or the setup failed
    /// each time).
    #[default]
    Unknown,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::All => "all",
            Class::Some => "some",
            Class::None => "none",
            Class::Unknown => "unknown",
        }
    }
}

/// A point's class from its outcome on each variant that has a base
/// (`None`: not run there). A lost prime or a failed setup is no outcome.
pub fn class(outcomes: impl IntoIterator<Item = Option<Outcome>>) -> Class {
    let (mut yes, mut no, mut missing) = (0u32, 0u32, 0u32);
    for o in outcomes {
        match o {
            Some(Outcome::Bounced) => yes += 1,
            Some(Outcome::NoBounce) => no += 1,
            _ => missing += 1,
        }
    }
    match (yes, no, missing) {
        (0, 0, _) => Class::Unknown,
        (0, _, _) => Class::None,
        (_, 0, 0) => Class::All,
        _ => Class::Some,
    }
}

/// The window: a cube of `radius` human steps around `centre`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// A point that bounced on every variant.
    pub centre: Vector,
    /// The ease: how many human steps (`scale`) every axis can be off at
    /// once, either way, with no sampled failure inside.
    pub radius: f32,
    /// Each axis's own range, `[low, high]`, with every other axis kept
    /// within `radius` steps of the centre: the first failure either way,
    /// or the end of what was sampled.
    pub bounds: [[f32; 2]; N],
    /// Points inside the cube; every one bounced on every variant.
    pub support: u32,
}

/// The ranges the points with an outcome cover, and the axes that vary
/// and have a scale.
fn domain(points: &[(Vector, Class)], scale: &[f32; N]) -> ([f32; N], [f32; N], [bool; N]) {
    let mut lo = [f32::INFINITY; N];
    let mut hi = [f32::NEG_INFINITY; N];
    for (p, c) in points {
        if *c == Class::Unknown {
            continue;
        }
        for a in 0..N {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    let mut active = [false; N];
    for a in 0..N {
        active[a] = scale[a] > 0.0 && hi[a] - lo[a] > 1e-6;
    }
    (lo, hi, active)
}

/// The largest difference on any active axis, in steps.
fn cheb(a: &Vector, b: &Vector, scale: &[f32; N], active: &[bool; N]) -> f32 {
    let mut d = 0.0f32;
    for i in 0..N {
        if active[i] {
            d = d.max((a[i] - b[i]).abs() / scale[i]);
        }
    }
    d
}

/// How many of the best candidates are tried for support.
const CANDIDATES: usize = 64;

/// The window over `points`, each with its class: the centre is the
/// [`Class::All`] point furthest (the largest difference on any axis, in
/// `scale` steps) from every failure ([`Class::Some`] or [`Class::None`])
/// and from the ends of what was sampled, whose cube holds at least
/// `min_support` points. `None` when no cube with a radius over 0 has the
/// support.
pub fn window(points: &[(Vector, Class)], scale: &[f32; N], min_support: u32) -> Option<Window> {
    let (lo, hi, active) = domain(points, scale);
    if !active.iter().any(|&a| a) {
        return None;
    }
    let fails: Vec<&Vector> = points
        .iter()
        .filter(|(_, c)| matches!(c, Class::Some | Class::None))
        .map(|(p, _)| p)
        .collect();
    let good: Vec<&Vector> = points
        .iter()
        .filter(|(_, c)| *c == Class::All)
        .map(|(p, _)| p)
        .collect();
    let mut radii: Vec<(usize, f32)> = good
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut r = f32::INFINITY;
            for a in 0..N {
                if active[a] {
                    r = r.min((c[a] - lo[a]).min(hi[a] - c[a]) / scale[a]);
                }
            }
            for f in &fails {
                r = r.min(cheb(c, f, scale, &active));
            }
            (i, r)
        })
        .collect();
    radii.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    for &(i, radius) in radii.iter().take(CANDIDATES) {
        if radius.is_nan() || radius <= 0.0 {
            break;
        }
        let c = good[i];
        let support = good
            .iter()
            .filter(|p| cheb(c, p, scale, &active) <= radius)
            .count() as u32;
        if support < min_support {
            continue;
        }
        let mut bounds = [[0.0f32; 2]; N];
        for a in 0..N {
            if !active[a] {
                bounds[a] = [c[a], c[a]];
                continue;
            }
            let (mut down, mut up) = (c[a] - lo[a], hi[a] - c[a]);
            for f in &fails {
                let beside = (0..N)
                    .all(|k| k == a || !active[k] || (f[k] - c[k]).abs() / scale[k] < radius);
                if !beside {
                    continue;
                }
                let d = f[a] - c[a];
                if d > 0.0 {
                    up = up.min(d);
                } else {
                    down = down.min(-d);
                }
            }
            bounds[a] = [c[a] - down, c[a] + up];
        }
        return Some(Window {
            centre: *c,
            radius,
            bounds,
            support,
        });
    }
    None
}

/// The range each axis covers over the [`Class::All`] points, `[low,
/// high]`; `None` with none.
pub fn extent(points: &[(Vector, Class)]) -> Option<[[f32; 2]; N]> {
    let mut out = [[f32::INFINITY, f32::NEG_INFINITY]; N];
    let mut any = false;
    for (p, c) in points {
        if *c != Class::All {
            continue;
        }
        any = true;
        for a in 0..N {
            out[a][0] = out[a][0].min(p[a]);
            out[a][1] = out[a][1].max(p[a]);
        }
    }
    any.then_some(out)
}

/// Along `axis` with every other axis inside the window's `radius` steps
/// of its centre (the failure that bounds the window is at the radius,
/// and is not inside): the share of the points with an outcome that are
/// [`Class::All`], and how many there were. For the depth axis, sampled
/// over one tick of fall, this is the share of tick phases that bounce at
/// the window's aim.
pub fn share_along(
    points: &[(Vector, Class)],
    w: &Window,
    scale: &[f32; N],
    axis: usize,
) -> Option<(f32, u32)> {
    let (_, _, active) = domain(points, scale);
    let (mut all, mut n) = (0u32, 0u32);
    for (p, c) in points {
        if *c == Class::Unknown {
            continue;
        }
        let beside = (0..N).all(|k| {
            k == axis
                || !active[k]
                || (p[k] - w.centre[k]).abs() / scale[k] < w.radius * 0.999
        });
        if beside {
            n += 1;
            if *c == Class::All {
                all += 1;
            }
        }
    }
    (n > 0).then(|| (all as f32 / n as f32, n))
}
