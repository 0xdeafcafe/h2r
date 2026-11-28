//! Shape-local edges are reusable. The component tells us which shapes
//! actually exist and the motion supplies their complete world transform.
use guest_layout::{DatumHandle, GuestMemory};
use guest_layout::tags::{GroupTag, tag_get_checked};
use live_telemetry::{havok, havok_agents, layout};
use map::TagId;
use sim_state::NodeMatrix;
use std::collections::{HashMap, HashSet};
use std::f32::consts::{FRAC_PI_2, TAU};
use tags::collision::{PhysicsModel, ShapeType, visit_hull_faces};

use super::{Edge, Point, append, dot, point};

fn cross([x, y, z]: Point, [a, b, c]: Point) -> Point {
    [y * c - z * b, z * a - x * c, x * b - y * a]
}

fn unit(v: Point) -> Point {
    let n = dot(v, v).sqrt();
    if n > 1e-8 {
        v.map(|k| k / n)
    } else {
        [0., 0., 1.]
    }
}

fn cap_edges(a: Point, b: Point, radius: f32) -> Vec<Edge> {
    const STEPS: usize = 16;
    if !radius.is_finite() || radius <= 0.0 {
        return vec![];
    }
    let pole = unit(std::array::from_fn(|i| b[i] - a[i]));
    let side = unit(cross(
        pole,
        if pole[0].abs() < 0.9 {
            [1., 0., 0.]
        } else {
            [0., 1., 0.]
        },
    ));
    let up = cross(pole, side);
    let surface = |base: Point, longitude: f32, latitude: f32| -> Point {
        let (s, c) = longitude.sin_cos();
        let (h, r) = latitude.sin_cos();
        std::array::from_fn(|i| base[i] + radius * (r * (side[i] * c + up[i] * s) + h * pole[i]))
    };
    let mut edges = Vec::new();
    for (base, sign) in [(a, -1.), (b, 1.)] {
        if sign < 0.0 || a != b {
            for i in 0..STEPS {
                let phi = |j| TAU * j as f32 / STEPS as f32;
                edges.push([surface(base, phi(i), 0.), surface(base, phi(i + 1), 0.)]);
            }
        }
        for i in 0..4 {
            let angle = FRAC_PI_2 * i as f32;
            for j in 0..4 {
                let lat = |k| sign * FRAC_PI_2 * k as f32 / 4.;
                edges.push([
                    surface(base, angle, lat(j)),
                    surface(base, angle, lat(j + 1)),
                ]);
            }
            if a != b && sign > 0.0 {
                edges.push([surface(a, angle, 0.), surface(b, angle, 0.)]);
            }
        }
    }
    edges
}

pub(super) fn body_edges(model: &PhysicsModel, budget: &mut usize) -> Vec<Vec<Edge>> {
    let mut bodies = Vec::new();
    for body in &model.rigid_bodies {
        let mut lines = Vec::new();
        let mut pending = vec![body.shape];
        let mut seen = HashSet::new();
        // Sharing and cycles are both possible in a malformed compound.
        while *budget > lines.len()
            && let Some(shape) = pending.pop()
        {
            if !seen.insert((shape.raw_type, shape.index)) {
                continue;
            }
            if seen.len() > 1024 {
                break;
            }
            let i = shape.index as usize;
            let remaining = *budget - lines.len();
            match shape.kind {
                ShapeType::List => {
                    if let Some(l) = model.lists.get(i) {
                        pending.extend(
                            l.children
                                .iter()
                                .take(1024usize.saturating_sub(pending.len()))
                                .rev()
                                .map(|c| c.shape),
                        );
                    }
                }
                ShapeType::Mopp => {
                    if let Some(m) = model.mopps.get(i) {
                        pending.push(m.child);
                    }
                }
                ShapeType::Box => {
                    if let Some(b) = model.boxes.get(i) {
                        let corner = |v: usize| -> Point {
                            let p: Point = std::array::from_fn(|i| {
                                if v & (1 << i) == 0 {
                                    -b.half_extents[i]
                                } else {
                                    b.half_extents[i]
                                }
                            });
                            // Stored transform columns (docs/engine/havok-runtime.md).
                            std::array::from_fn(|k| {
                                b.transform.translation[k]
                                    + (0..3)
                                        .map(|j| b.transform.rotation[j][k] * p[j])
                                        .sum::<f32>()
                            })
                        };
                        for v in 0..8 {
                            for axis in [1, 2, 4] {
                                if v & axis == 0 && lines.len() < *budget {
                                    lines.push([corner(v), corner(v | axis)]);
                                }
                            }
                        }
                    }
                }
                ShapeType::Sphere => {
                    if let Some(s) = model.spheres.get(i) {
                        lines.extend(
                            cap_edges(s.transform.translation, s.transform.translation, s.radius)
                                .into_iter()
                                .take(remaining),
                        );
                    }
                }
                ShapeType::Pill => {
                    if let Some(s) = model.pills.get(i) {
                        lines.extend(
                            cap_edges(s.bottom, s.top, s.radius)
                                .into_iter()
                                .take(remaining),
                        );
                    }
                }
                ShapeType::Polyhedron => {
                    if let Some(h) = model.polyhedra.get(i) {
                        let mut edges = HashSet::new();
                        let _ = visit_hull_faces(&h.vertices, &h.planes, |face| {
                            for (&a, &b) in face.iter().zip(face.iter().cycle().skip(1)) {
                                if lines.len() == *budget {
                                    return Err(tags::Error::Invalid("edge limit"));
                                }
                                if edges.insert((a.min(b), a.max(b))) {
                                    lines.push([h.vertices[a], h.vertices[b]]);
                                }
                            }
                            Ok(())
                        });
                    }
                }
                _ => {}
            }
        }
        *budget -= lines.len();
        bodies.push(lines);
    }
    bodies
}

// The phmo loader replaces every rigid body's shape reference with its
// resolved shape pointer. Match the entity to that pointer, so unused
// ragdoll bodies and alternate shapes stay out of the overlay.
fn body_indices(
    guest: &dyn GuestMemory,
    tag: TagId,
    expected: usize,
) -> Option<HashMap<u32, usize>> {
    let phmo = tag_get_checked(
        guest,
        DatumHandle::from_raw(tag.0)?,
        GroupTag::from_chars(*b"phmo"),
    )
    .ok()??;
    let count = guest.u32(phmo.checked_add(0x38)?).ok()? as usize;
    if phmo == 0 || count != expected || count > 1024 {
        return None;
    }
    let table = guest.u32(phmo.checked_add(0x3c)?).ok()?;
    if table == 0 {
        return None;
    }
    let mut result = HashMap::new();
    for i in 0..count {
        let shape = guest.u32(table.checked_add(i as u32 * 0x90 + 0x38)?).ok()?;
        if shape != 0 {
            result.insert(shape, i);
        }
    }
    Some(result)
}

// The motion's four xyz columns, docs/engine/havok-runtime.md, "Motion".
// Transform shapes use the same four vec4 layout.
fn frame(guest: &dyn GuestMemory, addr: u32) -> Option<NodeMatrix> {
    let mut m = NodeMatrix::default();
    m.0[0] = 1.;
    for i in 0..4 {
        let xyz = guest.vec3(addr.checked_add(i as u32 * 16)?).ok()?;
        m.0[1 + 3 * i..4 + 3 * i].copy_from_slice(&xyz);
    }
    Some(m)
}

fn live_capsule(guest: &dyn GuestMemory, entity: u32, shape: u32) -> Option<Vec<Edge>> {
    use havok::ShapeKind;
    // The biped controller's type 0x17 delegates support to its current
    // pill (docs/engine/havok-runtime.md, "Convex child state").
    let getter = guest
        .u32(
            guest
                .u32(shape)
                .ok()?
                .checked_add(layout::SHAPE_TYPE_SLOT)?,
        )
        .ok()?;
    if getter == 0x0009_f550 {
        let child = guest.u32(shape.checked_add(0x30)?).ok()?;
        if child == 0 {
            return None;
        }
        let kind = guest
            .u32(
                guest
                    .u32(child)
                    .ok()?
                    .checked_add(layout::SHAPE_TYPE_SLOT)?,
            )
            .ok()?;
        if kind != layout::SHAPE_TYPE_CAPSULE_FN {
            return None;
        }
        return Some(cap_edges(
            guest.vec3(child.checked_add(layout::CAPSULE_A)?).ok()?,
            guest.vec3(child.checked_add(layout::CAPSULE_B)?).ok()?,
            guest.f32(child.checked_add(layout::SHAPE_RADIUS)?).ok()?,
        ));
    }
    let as_f32 = |p: [f64; 3]| p.map(|n| n as f32);
    let shape = havok::entity_shape(guest, entity).ok()??;
    let leaf = |kind: &ShapeKind| match kind {
        ShapeKind::Capsule { a, b, radius } => {
            Some(cap_edges(as_f32(*a), as_f32(*b), *radius as f32))
        }
        ShapeKind::Sphere { radius } => Some(cap_edges([0.; 3], [0.; 3], *radius as f32)),
        _ => None,
    };
    if let ShapeKind::Transform {
        child_shape: Some(child),
        ..
    } = &shape.kind
    {
        let m = frame(guest, shape.addr.checked_add(0x10)?)?;
        Some(
            leaf(&child.kind)?
                .into_iter()
                .map(|e| e.map(|p| point(&m, p)))
                .collect(),
        )
    } else {
        leaf(&shape.kind)
    }
}

pub(super) fn live_edges(
    guest: &dyn GuestMemory,
    datum: u32,
    physics: Option<&(TagId, Vec<Vec<Edge>>)>,
    indices: &mut HashMap<TagId, HashMap<u32, usize>>,
    out: &mut Vec<Edge>,
    cap: usize,
) {
    let Ok(entities) = havok_agents::object_entities(guest, datum) else {
        return;
    };
    for entity in entities {
        if out.len() >= cap {
            break;
        }
        let Some((shape, matrix)) = (|| {
            let shape = guest.u32(entity.checked_add(layout::ENTITY_SHAPE)?).ok()?;
            let motion = guest.u32(entity.checked_add(layout::RIGID_MOTION)?).ok()?;
            if shape == 0 || motion == 0 {
                return None;
            }
            Some((shape, frame(guest, motion.checked_add(0x80)?)?))
        })() else {
            continue;
        };
        if let Some((id, bodies)) = physics {
            let pointers = indices
                .entry(*id)
                .or_insert_with(|| body_indices(guest, *id, bodies.len()).unwrap_or_default());
            if let Some(edges) = pointers
                .get(&shape)
                .and_then(|&i| bodies.get(i))
                .filter(|e| !e.is_empty())
            {
                append(out, cap, &matrix, edges);
                continue;
            }
        }
        if let Some(edges) = live_capsule(guest, entity, shape) {
            append(out, cap, &matrix, &edges);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn the_biped_wrapper_uses_the_current_child_and_radius() {
        const WRAPPER: u32 = 0x2000;
        const CHILD: u32 = 0x3000;
        let mut guest = FakeGuest::new();
        guest
            .put_u32(WRAPPER, 0x1000)
            .put_u32(0x1000 + layout::SHAPE_TYPE_SLOT, 0x0009_f550)
            .put_u32(WRAPPER + 0x30, CHILD)
            .put_u32(CHILD, 0x1100)
            .put_u32(
                0x1100 + layout::SHAPE_TYPE_SLOT,
                layout::SHAPE_TYPE_CAPSULE_FN,
            )
            .put_f32(CHILD + layout::CAPSULE_B + 8, 2.)
            .put_f32(CHILD + layout::SHAPE_RADIUS, 0.5);
        let top = |g: &FakeGuest| {
            live_capsule(g, 0, WRAPPER)
                .unwrap()
                .iter()
                .flatten()
                .map(|p| p[2])
                .fold(f32::NEG_INFINITY, f32::max)
        };
        assert_eq!(top(&guest), 2.5);
        guest.put_f32(CHILD + layout::CAPSULE_B + 8, 1.);
        assert_eq!(top(&guest), 1.5);
        guest.put_u32(WRAPPER + 0x30, 0);
        assert!(live_capsule(&guest, 0, WRAPPER).is_none());
    }
}
