//! Line geometry from the loaded scenario and active structure BSP.
//! Field layouts are in tags' sbsp definition and the H2 scenario writer in
//! h1campaign/src/scenario.rs (trigger_volumes and the kill-trigger list).
use std::collections::HashSet;
use std::io::{Read, Seek};

use agent_proto::debug::View;
use map::{FourCC, Map, TagId};
use tags::collision::{BSP_STRIDES, Bsp};
use tags::geometry::{RawSource, Section, TagView};
use tags::structure::{
    GeometryHeader, InstancedGeometryDefinition, InstancedGeometryInstance, StructureRender,
};
use tags::wire::TagBlock;
use tags::{Error, Result};

type Point = [f32; 3];
type Edge = [Point; 2];

// Separate from the per-image draw budget. One cache is built per active BSP.
const MAX_LINES: usize = 131_072;
// Referenced by signed 16-bit scenario/BSP indices. Bound the eager
// allocations in the existing tag readers before handing them a descriptor.
const MAX_ROWS: u32 = i16::MAX as u32 + 1;
const MAX_DECODE_BYTES: u64 = 128 << 20;
// Includes transforms yielding non-finite coordinates, which do not fill
// an output slot and would otherwise evade the line limit.
const MAX_PLACED_LINES: usize = 4 * MAX_LINES;

const INVISIBLE: u8 = 1 << 1; // docs/halo1/map-format.md, collision surface flags

/// Static debug edges for one map's active BSP.
#[derive(Default)]
pub struct StaticViews {
    pub(super) geometry_edges: Vec<Edge>,
    pub(super) triggers: Vec<Edge>,
    pub(super) kill_volumes: Vec<Edge>,
    pub(super) barriers: Vec<Edge>,
    pub(super) collision: Vec<Edge>,
    errors: Vec<(View, String)>,
}

impl StaticViews {
    /// Read the scenario and the BSP at its scenario index. Each view records
    /// its own decode failure after the scenario has been opened.
    pub fn build<R: Read + Seek>(
        map: &mut Map<R>,
        raw: &mut dyn RawSource,
        bsp: usize,
    ) -> Result<Self> {
        let scenario = map.scenario().ok_or(Error::Invalid("no scenario"))?.id;
        let (bytes, base) = payload(map, scenario)?;
        let scenario = TagView::new(&bytes, base);
        let mut out = Self::default();
        out.volumes(&scenario);
        let structure = (|| {
            let bsps = rows(&scenario, 0x210, 0x44)?;
            let row = bsps
                .get(bsp)
                .ok_or(Error::Invalid("no such scenario BSP"))?;
            let id = TagId(u32_at(row, 0x14));
            if map.tag(id)?.group != FourCC(*b"sbsp") {
                return Err(Error::Invalid("scenario BSP has the wrong class"));
            }
            let (bytes, base) = payload(map, id)?;
            Ok((id, bytes, base))
        })();
        match structure {
            Ok((id, bytes, base)) => out.structure(id, &bytes, base, raw),
            Err(error) => out.failed(
                &[
                    View::GeometryEdges,
                    View::CollisionGeometry,
                    View::InvisibleBarriers,
                ],
                &error,
            ),
        }
        Ok(out)
    }

    pub(super) fn refusal(&self, view: View) -> Option<&str> {
        self.errors
            .iter()
            .find(|(v, _)| *v == view)
            .map(|(_, message)| message.as_str())
    }

    fn failed(&mut self, views: &[View], error: &Error) {
        for &view in views {
            self.errors.push((view, error.to_string()));
        }
    }

    fn volumes(&mut self, scenario: &TagView<'_>) {
        let volumes = match rows(scenario, 0x108, 0x44) {
            Ok(rows) => rows,
            Err(error) => {
                self.failed(&[View::TriggerVolumes, View::KillVolumes], &error);
                return;
            }
        };
        for row in &volumes {
            if self.triggers.len() + 12 > MAX_LINES {
                break;
            }
            self.triggers.extend(volume(row));
        }
        match rows(scenario, 0x230, 2) {
            Ok(indices) => {
                let mut seen = HashSet::new();
                for row in indices {
                    if self.kill_volumes.len() + 12 > MAX_LINES {
                        break;
                    }
                    let i = u16::from_le_bytes([row[0], row[1]]);
                    if let Some(volume_row) = volumes.get(usize::from(i))
                        && seen.insert(i)
                    {
                        self.kill_volumes.extend(volume(volume_row));
                    }
                }
            }
            Err(error) => self.failed(&[View::KillVolumes], &error),
        }
    }

    fn structure(&mut self, id: TagId, bytes: &[u8], base: u32, raw: &mut dyn RawSource) {
        let view = TagView::new(bytes, base);
        if let Err(error) = self
            .structure_collision(&view)
            .and_then(|()| self.instance_collision(&view))
        {
            self.collision.clear();
            self.barriers.clear();
            self.failed(&[View::CollisionGeometry, View::InvisibleBarriers], &error);
        }
        let render = [0x9c, 0x138, 0x140]
            .into_iter()
            .try_for_each(|at| descriptor(&view, at).map(|_| ()))
            .and_then(|()| StructureRender::decode(id, &TagView::new(bytes, base)))
            .and_then(|bsp| self.drawn_parts(&bsp, raw));
        if let Err(error) = render {
            self.geometry_edges.clear();
            self.failed(&[View::GeometryEdges], &error);
        }
    }

    fn structure_collision(&mut self, view: &TagView<'_>) -> Result<()> {
        // sbsp+0x14 is a block of one eight-block collision BSP. Checking
        // the count first avoids repeatedly decoding aliased child arrays.
        let root = descriptor(view, 0x14)?;
        match root.count {
            0 => return Ok(()),
            1 => {}
            _ => return Err(Error::Invalid("expected one structure collision BSP")),
        }
        let at = payload_offset(view, root.address)?;
        let mut bytes_left = MAX_DECODE_BYTES;
        let mesh = CollisionMesh::read(view, at, &mut bytes_left)?;
        self.collision = mesh.all;
        self.barriers = mesh.invisible;
        Ok(())
    }

    fn instance_collision(&mut self, view: &TagView<'_>) -> Result<()> {
        let definitions = descriptor(view, 0x138)?;
        descriptor(view, 0x140)?;
        let count = definitions.count as usize;
        let placements = InstancedGeometryInstance::placements(view, count)?;
        if placements.is_empty() {
            return Ok(());
        }
        view.range(
            definitions.address,
            u64::from(definitions.count) * InstancedGeometryDefinition::SIZE as u64,
        )?;
        let base = payload_offset(view, definitions.address)?;
        let mut grouped = vec![Vec::new(); count];
        for instance in &placements {
            grouped[instance.definition as usize].push(instance);
        }
        let mut bytes_left = MAX_DECODE_BYTES;
        let mut work = MAX_PLACED_LINES;
        for (i, placements) in grouped.iter().enumerate().filter(|(_, p)| !p.is_empty()) {
            if self.collision.len() == MAX_LINES && self.barriers.len() == MAX_LINES {
                break;
            }
            // docs/engine/collision-queries.md, entry E: the per-definition
            // collision test receives the eight-block descriptor at +0x70.
            let at = base + i * InstancedGeometryDefinition::SIZE + 0x70;
            let mesh = CollisionMesh::read(view, at, &mut bytes_left)?;
            for instance in placements {
                let transform = |p| instance.transform(p);
                place(
                    &mut self.collision,
                    &mesh.all,
                    MAX_LINES,
                    &mut work,
                    &transform,
                )?;
                place(
                    &mut self.barriers,
                    &mesh.invisible,
                    MAX_LINES,
                    &mut work,
                    &transform,
                )?;
            }
        }
        Ok(())
    }

    fn drawn_parts(&mut self, bsp: &StructureRender, raw: &mut dyn RawSource) -> Result<()> {
        let material_count = Some(bsp.material_count as usize);
        let mut bytes_left = MAX_DECODE_BYTES;
        let mut work = MAX_PLACED_LINES;
        let mut read = |header: &GeometryHeader| -> Result<Option<Vec<Edge>>> {
            // Distinct headers may reference the same (potentially large) raw resource.
            if header.block.block.is_some() {
                charge_bytes(&mut bytes_left, u64::from(header.block.block_size))?;
            }
            header
                .load(raw, material_count)?
                .as_ref()
                .map(render_edges)
                .transpose()
        };
        for cluster in &bsp.clusters {
            if self.geometry_edges.len() == MAX_LINES {
                return Ok(());
            }
            if let Some(mesh) = read(&cluster.geometry)? {
                place(
                    &mut self.geometry_edges,
                    &mesh,
                    MAX_LINES,
                    &mut work,
                    &|p| p,
                )?;
            }
        }
        // Load a definition once for all placements that reference it, and
        // drop its resources before loading the next definition.
        let mut grouped = vec![Vec::new(); bsp.instance_definitions.len()];
        for instance in &bsp.instances {
            grouped[instance.definition as usize].push(instance);
        }
        for (definition, placements) in bsp.instance_definitions.iter().zip(grouped) {
            if !placements.is_empty()
                && let Some(mesh) = read(&definition.geometry)?
            {
                for instance in placements {
                    if self.geometry_edges.len() == MAX_LINES {
                        return Ok(());
                    }
                    place(
                        &mut self.geometry_edges,
                        &mesh,
                        MAX_LINES,
                        &mut work,
                        &|p| instance.transform(p),
                    )?;
                }
            }
        }
        Ok(())
    }
}

fn payload<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<(Vec<u8>, u32)> {
    let location = map.tag(id)?.location.ok_or(map::Error::NoTagData)?;
    if u64::from(location.size) > MAX_DECODE_BYTES {
        return Err(Error::Invalid(
            "debug geometry tag exceeds the decode budget",
        ));
    }
    Ok((map.read_tag_bytes(id)?, location.address))
}

fn payload_offset(view: &TagView<'_>, address: u32) -> Result<usize> {
    Ok(address
        .checked_sub(view.address())
        .ok_or(Error::Invalid("geometry address precedes its payload"))? as usize)
}

fn descriptor(view: &TagView<'_>, at: usize) -> Result<TagBlock> {
    let field = view.at(at, 8)?;
    let desc = TagBlock {
        count: u32_at(field, 0),
        address: u32_at(field, 4),
    };
    if desc.count > MAX_ROWS {
        return Err(Error::Invalid("debug geometry block exceeds its row limit"));
    }
    Ok(desc)
}

fn rows<'a>(view: &TagView<'a>, at: usize, stride: usize) -> Result<Vec<&'a [u8]>> {
    let block = descriptor(view, at)?;
    view.elements(block.count, block.address, stride)
}

fn charge_bytes(remaining: &mut u64, bytes: u64) -> Result<()> {
    *remaining = remaining
        .checked_sub(bytes)
        .ok_or(Error::Invalid("debug geometry exceeds the decode budget"))?;
    Ok(())
}

fn place(
    out: &mut Vec<Edge>,
    local: &[Edge],
    cap: usize,
    work: &mut usize,
    transform: &impl Fn(Point) -> Point,
) -> Result<()> {
    let count = local.len().min(cap.saturating_sub(out.len()));
    *work = work.checked_sub(count).ok_or(Error::Invalid(
        "debug geometry exceeds the placement budget",
    ))?;
    for p in &local[..count] {
        let world = p.map(transform);
        if world.as_flattened().iter().all(|n| n.is_finite()) {
            out.push(world);
        }
    }
    Ok(())
}

struct CollisionMesh {
    all: Vec<Edge>,
    invisible: Vec<Edge>,
}

impl CollisionMesh {
    fn read(view: &TagView<'_>, at: usize, bytes_left: &mut u64) -> Result<Self> {
        let fields = view.at(at, Bsp::STRUCTURE_SIZE)?;
        for (slot, stride) in BSP_STRIDES.iter().enumerate() {
            charge_bytes(
                bytes_left,
                u64::from(u32_at(fields, slot * 8)) * *stride as u64,
            )?;
        }
        let bsp = Bsp::decode_structure(view.bytes(), view.address(), at)?;
        let mut all = Vec::new();
        let mut invisible = Vec::new();
        // Classify a shared definition once. When a destination cache is
        // full, its later placements never traverse the local edges again.
        for edge in &bsp.edges {
            let [left, right] = [edge.left_surface, edge.right_surface].map(|s| {
                (s != u16::MAX)
                    .then(|| bsp.surfaces.get(usize::from(s)))
                    .flatten()
            });
            if left.is_none() && right.is_none() {
                continue;
            }
            let (Some(a), Some(b)) = (
                bsp.vertices.get(usize::from(edge.start_vertex)),
                bsp.vertices.get(usize::from(edge.end_vertex)),
            ) else {
                continue;
            };
            let edge = [a.point, b.point];
            if !edge.as_flattened().iter().all(|n| n.is_finite()) || edge[0] == edge[1] {
                continue;
            }
            if all.len() < MAX_LINES {
                all.push(edge);
            }
            if invisible.len() < MAX_LINES
                && [left, right]
                    .into_iter()
                    .flatten()
                    .any(|s| s.flags & INVISIBLE != 0)
            {
                invisible.push(edge);
            }
        }
        Ok(Self { all, invisible })
    }
}

fn u32_at(row: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(row[at..at + 4].try_into().expect("field in checked record"))
}

fn volume(row: &[u8]) -> Vec<Edge> {
    // The stored frame is in world space only for an unparented volume.
    // Positioned marker/parent frames need the live object's transform.
    if i16::from_le_bytes([row[4], row[5]]) >= 0 {
        return Vec::new();
    }
    let vec3 = |at| std::array::from_fn(|i| f32::from_bits(u32_at(row, at + 4 * i)));
    box_edges(vec3(0x24), vec3(0xc), vec3(0x18), vec3(0x30)).unwrap_or_default()
}

// Scenario volumes start at the origin corner. Extents are full lengths
// along forward, left and up, including for a rotated frame.
fn box_edges(origin: Point, forward: Point, up: Point, size: Point) -> Option<Vec<Edge>> {
    fn unit(v: Point) -> Option<Point> {
        let len = v[0].hypot(v[1]).hypot(v[2]);
        (len > 1e-6 && len.is_finite()).then(|| v.map(|x| x / len))
    }
    fn cross([x, y, z]: Point, [a, b, c]: Point) -> Point {
        [y * c - z * b, z * a - x * c, x * b - y * a]
    }
    if !size.into_iter().all(|v| v > 0.0 && v.is_finite()) {
        return None;
    }
    let forward = unit(forward)?;
    let left = unit(cross(unit(up)?, forward))?;
    let axes = [forward, left, cross(forward, left)];
    let mut corners = [origin; 8];
    for i in 1usize..8 {
        let axis = i.trailing_zeros() as usize;
        let from = corners[i & (i - 1)];
        corners[i] = std::array::from_fn(|k| from[k] + size[axis] * axes[axis][k]);
    }
    if !corners.as_flattened().iter().all(|v| v.is_finite()) {
        return None;
    }
    let mut edges = Vec::with_capacity(12);
    for (i, &from) in corners.iter().enumerate() {
        for bit in [1, 2, 4] {
            if i & bit == 0 {
                edges.push([from, corners[i | bit]]);
            }
        }
    }
    Some(edges)
}

fn render_edges(section: &Section) -> Result<Vec<Edge>> {
    let positions = section.vertices(None)?.positions;
    if positions.len() != section.vertex_count() {
        return Err(Error::Invalid("BSP render section has no position stream"));
    }
    let triangles = section
        .parts
        .iter()
        .filter(|p| p.is_drawn())
        .flat_map(|p| section.part_triangles(p));
    Ok(mesh_edges(&positions, triangles))
}

fn mesh_edges(positions: &[Point], triangles: impl IntoIterator<Item = [u16; 3]>) -> Vec<Edge> {
    let mut edges = Vec::new();
    let mut seen = HashSet::new();
    for [a, b, c] in triangles {
        for [i, j] in [[a, b], [b, c], [c, a]] {
            if seen.insert((i.min(j), i.max(j))) {
                edges.push([positions[usize::from(i)], positions[usize::from(j)]]);
                if edges.len() == MAX_LINES {
                    return edges;
                }
            }
        }
    }
    edges
}

#[cfg(test)]
mod tests;
