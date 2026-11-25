//! In-place edits of the shape fields documented in docs/formats/collision.md.

use super::{
    Error, HULL_TOLERANCE, MAX_HULL_ELEMENTS, PHMO_ROOT_SIZE, Payload, PhysicsModel, Polyhedron,
    Result, ShapeType, cross, dot, f32_at, u32_at,
};

type Point = [f32; 3];

#[derive(Clone, Debug, PartialEq)]
pub enum ShapeEdit {
    Radius(f32),
    HalfExtents(Point),
    Bottom(Point),
    Top(Point),
    Translation(Point),
    Rotation([Point; 3]),
    Vertex { index: usize, position: Point },
}

/// Plan disjoint four-byte writes, relative to the tag root. Validate the
/// entire edit before returning any write.
pub fn plan(
    bytes: &[u8],
    base: u32,
    kind: ShapeType,
    index: usize,
    edit: &ShapeEdit,
) -> Result<Vec<(usize, [u8; 4])>> {
    let model = PhysicsModel::decode(bytes, base)?;
    let (field, stride) = match kind {
        ShapeType::Sphere => (0x48, 0x80),
        ShapeType::Pill => (0x58, 0x50),
        ShapeType::Box => (0x60, 0x90),
        ShapeType::Polyhedron => (0x70, 0x100),
        _ => return Err(Error::Invalid("shape kind cannot be edited in place")),
    };
    let p = Payload::new(bytes, base);
    let at = block_offset(&p, field, stride, index)?;
    let mut writes = Vec::new();
    let mut put = |offset, values: &[f32]| -> Result<()> {
        for (i, &v) in values.iter().enumerate() {
            if !v.is_finite() {
                return Err(Error::Invalid("shape edit produces non-finite coordinates"));
            }
            let offset = offset + 4 * i;
            let encoded = v.to_le_bytes();
            if bytes[offset..offset + 4] != encoded {
                writes.push((offset, encoded));
            }
        }
        Ok(())
    };
    match (kind, edit) {
        (ShapeType::Sphere | ShapeType::Pill | ShapeType::Box, ShapeEdit::Radius(r))
            if *r >= 0.0 =>
        {
            let offset = if kind == ShapeType::Box { 0x3c } else { 0x2c };
            put(at + offset, &[*r])?;
            if kind == ShapeType::Pill {
                let old = f32_at(bytes, at + offset);
                // Keep the endpoint copies in sync where the source stores them.
                for w in [0x3c, 0x4c] {
                    if f32_at(bytes, at + w).to_bits() == old.to_bits() {
                        put(at + w, &[*r])?;
                    }
                }
            }
        }
        (ShapeType::Box, ShapeEdit::HalfExtents(v)) if v.iter().all(|&x| x > 0.0) => {
            put(at + 0x30, v)?;
        }
        (ShapeType::Pill, ShapeEdit::Bottom(v)) => put(at + 0x30, v)?,
        (ShapeType::Pill, ShapeEdit::Top(v)) => put(at + 0x40, v)?,
        (ShapeType::Sphere | ShapeType::Box, ShapeEdit::Translation(v)) => {
            let offset = if kind == ShapeType::Sphere {
                0x70
            } else {
                0x80
            };
            put(at + offset, v)?;
        }
        (ShapeType::Sphere | ShapeType::Box, ShapeEdit::Rotation(rows)) => {
            if rows
                .iter()
                .flatten()
                .any(|x| !x.is_finite() || x.abs() > 1.0001)
                || (dot(cross(rows[0], rows[1]), rows[2]) - 1.0).abs() > 1e-4
                || (0..3).any(|i| {
                    (i..3).any(|j| {
                        (dot(rows[i], rows[j]) - if i == j { 1.0 } else { 0.0 }).abs() > 1e-4
                    })
                })
            {
                return Err(Error::Invalid(
                    "rotation must be a right-handed orthonormal matrix",
                ));
            }
            let offset = if kind == ShapeType::Sphere {
                0x40
            } else {
                0x50
            };
            for (i, row) in rows.iter().enumerate() {
                put(at + offset + i * 16, row)?;
            }
        }
        (
            ShapeType::Polyhedron,
            ShapeEdit::Vertex {
                index: vertex,
                position,
            },
        ) => {
            let hull = &model.polyhedra[index];
            let old = hull
                .vertices
                .get(*vertex)
                .ok_or(Error::Invalid("no such hull vertex"))?;
            if old == position {
                return Ok(writes);
            }
            let (center, half, planes) = move_vertex(hull, *vertex, *position)?;
            let preceding = &model.polyhedra[..index];
            let group = *vertex / 4;
            let lane = *vertex % 4;
            let points = if hull.group_capacity <= 3 {
                at + 0x60 + group * 0x30
            } else {
                let cursor: usize = preceding
                    .iter()
                    .filter(|h| h.group_capacity > 3)
                    .map(|h| h.group_count as usize)
                    .sum();
                block_offset(&p, 0x78, 0x30, cursor + group)?
            };
            for (axis, value) in position.iter().enumerate() {
                put(points + axis * 16 + lane * 4, &[*value])?;
            }
            put(at + 0x30, &half)?;
            put(at + 0x40, &center)?;
            let cursor: usize = preceding.iter().map(|h| h.plane_count as usize).sum();
            for (i, plane) in planes.iter().enumerate() {
                put(block_offset(&p, 0x80, 16, cursor + i)?, plane)?;
            }
        }
        _ => {
            return Err(Error::Invalid(
                "field or value is invalid for this physics shape",
            ));
        }
    }

    // A MOPP is compiled against its children's bounds. The shape codec
    // cannot regenerate physics-model MOPP code.
    if !writes.is_empty() && used_by_mopp(&model, kind, index) {
        return Err(Error::Invalid(
            "this shape is in a MOPP and requires a rebuild",
        ));
    }
    writes.sort_unstable_by_key(|(offset, _)| *offset);
    if writes.windows(2).any(|pair| pair[0].0 + 4 > pair[1].0) {
        return Err(Error::Invalid("physics shape blocks overlap"));
    }
    Ok(writes)
}

fn used_by_mopp(model: &PhysicsModel, kind: ShapeType, index: usize) -> bool {
    let mut pending: Vec<_> = model.mopps.iter().map(|mopp| mopp.child).collect();
    let mut seen = std::collections::HashSet::new();
    while let Some(shape) = pending.pop() {
        let i = usize::from(shape.index);
        if shape.kind == kind && i == index {
            return true;
        }
        if !seen.insert((shape.raw_type, i)) {
            continue;
        }
        match shape.kind {
            ShapeType::List => {
                if let Some(list) = model.lists.get(i) {
                    pending.extend(list.children.iter().map(|child| child.shape));
                }
            }
            ShapeType::Type6 => {
                if let Some(wrapper) = model.type6.get(i) {
                    pending.push(wrapper.child);
                }
            }
            _ => {}
        }
    }
    false
}

fn block_offset(p: &Payload<'_>, field: usize, stride: usize, index: usize) -> Result<usize> {
    let (count, _) = p.block(field, stride)?;
    if index >= count {
        return Err(Error::Invalid("no such physics shape or block element"));
    }
    let start = (u32_at(p.bytes, field + 4) - p.base) as usize;
    if start < PHMO_ROOT_SIZE {
        return Err(Error::Invalid("physics shape block overlaps the tag root"));
    }
    let end = start + count * stride;
    // Read-only decoders tolerate aliases. Refuse to edit a block whose
    // bytes are also owned by another top-level block.
    for (other, size) in [
        (0x28, 0x68),
        (0x30, 0x18),
        (0x38, 0x90),
        (0x40, 12),
        (0x48, 0x80),
        (0x50, 0xb0),
        (0x58, 0x50),
        (0x60, 0x90),
        (0x68, 0x60),
        (0x70, 0x100),
        (0x78, 0x30),
        (0x80, 16),
        (0x90, 0x38),
        (0x98, 8),
        (0xa0, 0x14),
        (0xa8, 1),
        (0xb0, 0x78),
        (0xb8, 0x94),
        (0xc0, 12),
        (0xc8, 12),
        (0xe8, 0x84),
        (0xf0, 0x78),
        (0xf8, 0x7c),
        (0x100, 0x84),
        (0x108, 0x20),
    ] {
        let n = u32_at(p.bytes, other) as u64;
        if other == field || n == 0 {
            continue;
        }
        let Some(lo) = u32_at(p.bytes, other + 4)
            .checked_sub(p.base)
            .map(u64::from)
        else {
            continue;
        };
        if lo < end as u64 && (start as u64) < lo + n * size {
            return Err(Error::Invalid("physics shape blocks overlap"));
        }
    }
    Ok(start + index * stride)
}

type HullUpdate = (Point, Point, Vec<[f32; 4]>);

fn move_vertex(hull: &Polyhedron, index: usize, position: Point) -> Result<HullUpdate> {
    let invalid = || Error::Invalid("vertex edit must preserve a convex hull and its faces");
    if hull.vertices.len() < 4
        || hull.vertices.len() > MAX_HULL_ELEMENTS
        || hull.planes.len() < 4
        || hull.planes.len() > MAX_HULL_ELEMENTS
        || hull
            .vertices
            .iter()
            .flatten()
            .chain(&position)
            .any(|x| !x.is_finite())
    {
        return Err(invalid());
    }
    let mut vertices = hull.vertices.clone();
    vertices[index] = position;
    if vertices
        .iter()
        .enumerate()
        .any(|(i, v)| i != index && *v == position)
    {
        return Err(invalid());
    }
    let mut faces_at_vertex = 0;
    let mut planes = Vec::with_capacity(hull.planes.len());
    for &plane in &hull.planes {
        let normal = [plane[0], plane[1], plane[2]];
        let on: Vec<_> = hull
            .vertices
            .iter()
            .enumerate()
            .filter_map(|(i, &v)| {
                ((dot(normal, v) + plane[3]).abs() <= HULL_TOLERANCE).then_some(i)
            })
            .collect();
        if on.is_empty() {
            return Err(invalid());
        }
        let mut updated = plane;
        if on.contains(&index) && on.len() >= 3 {
            faces_at_vertex += 1;
        }
        if on.contains(&index) && (dot(normal, position) + plane[3]).abs() > HULL_TOLERANCE {
            if on.len() >= 3 {
                let anchor = vertices[on[0]];
                let sub = |a: Point, b: Point| std::array::from_fn(|i| a[i] - b[i]);
                let axis = on
                    .iter()
                    .map(|&i| sub(vertices[i], anchor))
                    .max_by(|&a, &b| dot(a, a).total_cmp(&dot(b, b)))
                    .ok_or_else(invalid)?;
                let perpendicular = on
                    .iter()
                    .map(|&i| cross(axis, sub(vertices[i], anchor)))
                    .max_by(|&a, &b| dot(a, a).total_cmp(&dot(b, b)))
                    .ok_or_else(invalid)?;
                let length = dot(perpendicular, perpendicular).sqrt();
                if !length.is_finite() || length == 0.0 || dot(normal, perpendicular) == 0.0 {
                    return Err(invalid());
                }
                let sign = dot(normal, perpendicular).signum();
                let n = perpendicular.map(|v| v / length * sign);
                updated = [n[0], n[1], n[2], -dot(n, anchor)];
            } else {
                // A support plane touching an edge or point has no unique
                // rotation. Keep its normal and move it to the new support.
                updated[3] = -vertices
                    .iter()
                    .map(|&v| dot(normal, v))
                    .fold(f32::NEG_INFINITY, f32::max);
            }
        }
        let n = [updated[0], updated[1], updated[2]];
        for (i, &v) in vertices.iter().enumerate() {
            let distance = dot(n, v) + updated[3];
            if !distance.is_finite()
                || distance > HULL_TOLERANCE
                || (on.len() >= 3
                    && (distance.abs() <= HULL_TOLERANCE) != on.binary_search(&i).is_ok())
            {
                return Err(invalid());
            }
        }
        planes.push(updated);
    }
    if faces_at_vertex < 3 {
        return Err(invalid());
    }
    let lo: Point =
        std::array::from_fn(|i| vertices.iter().map(|v| v[i]).fold(f32::INFINITY, f32::min));
    let hi: Point = std::array::from_fn(|i| {
        vertices
            .iter()
            .map(|v| v[i])
            .fold(f32::NEG_INFINITY, f32::max)
    });
    Ok((
        std::array::from_fn(|i| hi[i] * 0.5 + lo[i] * 0.5),
        std::array::from_fn(|i| hi[i] * 0.5 - lo[i] * 0.5),
        planes,
    ))
}
