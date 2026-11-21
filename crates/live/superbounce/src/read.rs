//! Collision polygons out of a decoded BSP and out of a `.map`.
use std::io::{Read, Seek};

use map::{FourCC, Map, TagId};
use tags::collision::Bsp;
use tags::geometry::TagView;
use tags::structure::{InstancedGeometryDefinition, InstancedGeometryInstance};
use tags::{Error, Result};

use crate::geom::{cross, dot, unit};
use crate::{Poly, Vec3};

/// Scenario `+0x210`: the structure BSP block (0x44 a row, the `sbsp`
/// reference's tag id at `+0x14`); `crates/live/bots/src/autonav.rs` and
/// the agent's `debug_static.rs` read the same.
const SCENARIO_BSPS: usize = 0x210;
const SCENARIO_BSP_ROW: usize = 0x44;
/// `sbsp+0x138`: instanced geometry definitions, each with its collision
/// BSP's eight blocks at `+0x70` (docs/engine/collision-queries.md, entry
/// E); `sbsp+0x140` is the placement block
/// ([`InstancedGeometryInstance::placements`]).
const SBSP_INSTANCE_DEFINITIONS: usize = 0x138;
const DEFINITION_COLLISION: usize = 0x70;
/// A map's structure and instances together stay well under this; past it
/// the read stops with an error rather than allocating without bound.
const MAX_POLYS: usize = 4 << 20;

pub(crate) fn polys(bsp: &Bsp, transform: Option<[[f32; 4]; 3]>) -> Vec<Poly> {
    let mut out = Vec::with_capacity(bsp.surfaces.len());
    // docs/formats/collision.md: a legitimate BSP walks each edge at most
    // twice; all rings share this budget so a malformed ring cannot make
    // the read quadratic.
    let mut budget = 4 * bsp.edges.len() + bsp.surfaces.len() + 64;
    for (i, surface) in bsp.surfaces.iter().enumerate() {
        // The surface's plane word: low 15 bits the plane, bit 15 flips it
        // (docs/formats/collision.md, surfaces). The flipped plane's
        // normal points out of the solid. Every surface is kept, whatever
        // its flags: two-sided, invisible (player clip), climbable and
        // breakable surfaces all collide with a biped, and the agent's
        // collision view draws them all too.
        let Some(plane) = bsp.planes.get(usize::from(surface.plane & 0x7fff)) else {
            continue;
        };
        let [nx, ny, nz, _] = plane.0;
        let mut normal = if surface.plane & 0x8000 != 0 {
            [-nx, -ny, -nz]
        } else {
            [nx, ny, nz]
        };
        let Some(ring) = ring(bsp, i, &mut budget) else {
            break;
        };
        let mut verts: Vec<Vec3> = ring
            .iter()
            .filter_map(|&v| bsp.vertices.get(usize::from(v)).map(|v| v.point))
            .collect();
        if let Some(m) = transform {
            verts.iter_mut().for_each(|p| *p = apply(&m, *p));
            normal = transform_normal(&m, normal);
        }
        let Some(normal) = unit(normal) else {
            continue;
        };
        if verts.len() >= 3 && verts.as_flattened().iter().all(|v| v.is_finite()) {
            out.push(Poly { verts, normal });
        }
    }
    out
}

/// One surface's vertex ring, by the engine's walk
/// (docs/formats/collision.md, "Ring rule"): an edge whose right surface is
/// this one gives its end vertex and continues at its reverse edge,
/// otherwise its start vertex and its forward edge, until the first edge
/// comes round. `None` when the shared budget runs out.
fn ring(bsp: &Bsp, surface: usize, budget: &mut usize) -> Option<Vec<u16>> {
    let mut out = Vec::new();
    let first = bsp.surfaces[surface].first_edge;
    let mut e = first;
    loop {
        *budget = budget.checked_sub(1)?;
        let Some(edge) = bsp.edges.get(usize::from(e)) else {
            return Some(Vec::new());
        };
        let right = usize::from(edge.right_surface) == surface;
        out.push(if right {
            edge.end_vertex
        } else {
            edge.start_vertex
        });
        e = if right {
            edge.reverse_edge
        } else {
            edge.forward_edge
        };
        if e == first {
            return Some(out);
        }
    }
}

/// `m` is row-major 3x4: `out[i] = m[i][0..3] . p + m[i][3]`.
fn apply(m: &[[f32; 4]; 3], p: Vec3) -> Vec3 {
    std::array::from_fn(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3])
}

/// A normal moves by the inverse transpose of the 3x3 part: its cofactor
/// matrix over the determinant. Only the sign of the determinant matters
/// before normalising (a mirror keeps an outward normal outward).
fn transform_normal(m: &[[f32; 4]; 3], n: Vec3) -> Vec3 {
    let col = |j: usize| [m[0][j], m[1][j], m[2][j]];
    let (c0, c1, c2) = (col(0), col(1), col(2));
    // Rows of the cofactor matrix are the cross products of the columns.
    let rows = [cross(c1, c2), cross(c2, c0), cross(c0, c1)];
    let det = dot(c0, rows[0]);
    let s = if det < 0.0 { -1.0 } else { 1.0 };
    // (M^-T) n = (1/det) * C n, where C's row i is rows[i] read as a column:
    // C = [rows[0] rows[1] rows[2]] (as columns), so (C n)_k = sum_i rows[i][k] n_i.
    std::array::from_fn(|k| s * (rows[0][k] * n[0] + rows[1][k] * n[1] + rows[2][k] * n[2]))
}

/// The row-major 3x4 transform of an instance placement:
/// `position + scale * (x * forward + y * left + z * up)`
/// ([`InstancedGeometryInstance::transform`]).
pub(crate) fn instance_transform(i: &InstancedGeometryInstance) -> [[f32; 4]; 3] {
    std::array::from_fn(|r| {
        [
            i.scale * i.basis[0][r],
            i.scale * i.basis[1][r],
            i.scale * i.basis[2][r],
            i.position[r],
        ]
    })
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub(crate) fn map_polys<R: Read + Seek>(map: &mut Map<R>, bsp: usize) -> Result<Vec<Poly>> {
    let scenario = map.scenario().ok_or(Error::Invalid("no scenario"))?.clone();
    let location = scenario
        .location
        .ok_or(Error::Invalid("the scenario has no payload"))?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    let rows = view.block(view.bytes(), SCENARIO_BSPS, SCENARIO_BSP_ROW)?;
    let row = rows
        .get(bsp)
        .ok_or(Error::Invalid("no such scenario BSP"))?;
    let id = TagId(u32_at(row, 0x14));
    let tag = map.tag(id)?.clone();
    if tag.group != FourCC(*b"sbsp") {
        return Err(Error::Invalid("scenario BSP has the wrong class"));
    }
    let location = tag
        .location
        .ok_or(Error::Invalid("the structure BSP has no payload"))?;
    let bytes = map.read_tag_bytes(id)?;
    let view = TagView::new(&bytes, location.address);
    let offset = |slice: &[u8]| slice.as_ptr() as usize - bytes.as_ptr() as usize;

    let mut out = Vec::new();
    // sbsp+0x14: one structure collision BSP (tags::pathfinding).
    let structure = view.block(
        view.bytes(),
        tags::pathfinding::SBSP_COLLISION_FIELD,
        Bsp::STRUCTURE_SIZE,
    )?;
    if let Some(first) = structure.first() {
        let collision = Bsp::decode_structure(&bytes, location.address, offset(first))?;
        out.extend(polys(&collision, None));
    }

    let definitions = view.block(
        view.bytes(),
        SBSP_INSTANCE_DEFINITIONS,
        InstancedGeometryDefinition::SIZE,
    )?;
    let placements = InstancedGeometryInstance::placements(&view, definitions.len())?;
    let mut grouped = vec![Vec::new(); definitions.len()];
    for p in &placements {
        grouped[p.definition as usize].push(p);
    }
    for (definition, placed) in definitions.iter().zip(&grouped) {
        if placed.is_empty() {
            continue;
        }
        let at = offset(definition) + DEFINITION_COLLISION;
        let collision = Bsp::decode_structure(&bytes, location.address, at)?;
        for p in placed {
            out.extend(polys(&collision, Some(instance_transform(p))));
            if out.len() > MAX_POLYS {
                return Err(Error::Invalid("collision exceeds the polygon budget"));
            }
        }
    }
    Ok(out)
}
