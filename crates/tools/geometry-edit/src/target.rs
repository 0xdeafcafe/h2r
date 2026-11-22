//! What a mesh comes from, and the listing of a map's targets.

use std::fmt;
use std::io::{Read, Seek};

use map::{FourCC, Map, ResourceFile, TagId};
use tags::geometry::RawSource;
use tags::model::RenderModelGeometry;
use tags::structure::StructureRender;

/// The kinds of target this crate opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TargetKind {
    /// A structure BSP cluster's section (`sbsp+0x9c`).
    Cluster,
    /// An instanced geometry definition's section (`sbsp+0x138`).
    InstanceDef,
    /// One section of a render model (`mode+0x24`).
    ModeSection,
}

impl TargetKind {
    pub const ALL: [TargetKind; 3] = [Self::Cluster, Self::InstanceDef, Self::ModeSection];

    /// The name the agent API and target ids use.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cluster => "cluster",
            Self::InstanceDef => "instance_def",
            Self::ModeSection => "mode_section",
        }
    }

    /// The window's label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cluster => "Cluster",
            Self::InstanceDef => "Instanced geometry",
            Self::ModeSection => "Model section",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// A target: its kind, the tag (`sbsp` or `mode`) and the element index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub kind: TargetKind,
    pub tag: TagId,
    pub index: u32,
}

impl Target {
    pub fn new(kind: TargetKind, tag: TagId, index: u32) -> Self {
        Self { kind, tag, index }
    }

    /// `kind:tag:index`, the tag as eight hex digits:
    /// `cluster:e1000004:0`.
    pub fn id(&self) -> String {
        format!("{}:{:08x}:{}", self.kind.name(), self.tag.0, self.index)
    }

    /// The inverse of [`Self::id`].
    pub fn parse(id: &str) -> Option<Self> {
        let mut it = id.split(':');
        let kind = TargetKind::from_name(it.next()?)?;
        let tag = it.next()?;
        let tag = u32::from_str_radix(tag.strip_prefix("0x").unwrap_or(tag), 16).ok()?;
        let index = it.next()?.parse().ok()?;
        if it.next().is_some() {
            return None;
        }
        Some(Self::new(kind, TagId(tag), index))
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.id())
    }
}

/// One listed target with its counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetInfo {
    pub target: Target,
    /// The owning tag's name (or its id when the map has no name for it).
    pub tag_name: String,
    pub vertices: usize,
    pub triangles: usize,
    pub parts: usize,
    /// Whether its raw block is in this map: a block in `shared.map` or
    /// another cache opens for viewing but is never written.
    pub local: bool,
}

fn tag_name<R: Read + Seek>(map: &Map<R>, id: TagId) -> String {
    map.tag(id)
        .ok()
        .and_then(|t| t.name.clone())
        .unwrap_or_else(|| format!("{:08x}", id.0))
}

/// Every cluster and instanced geometry definition with a raw block, of
/// every `sbsp` in the map, from the block infos alone (no raw data is
/// read). A BSP that does not decode is skipped.
pub fn list<R: Read + Seek>(map: &mut Map<R>) -> Vec<TargetInfo> {
    let bsps: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"sbsp") && t.location.is_some())
        .map(|t| t.id)
        .collect();
    let mut out = Vec::new();
    for bsp in bsps {
        let Ok(render) = StructureRender::read(map, bsp) else {
            continue;
        };
        let name = tag_name(map, bsp);
        let headers = render
            .clusters
            .iter()
            .map(|c| (TargetKind::Cluster, &c.geometry))
            .enumerate()
            .chain(
                render
                    .instance_definitions
                    .iter()
                    .map(|d| (TargetKind::InstanceDef, &d.geometry))
                    .enumerate(),
            );
        for (i, (kind, g)) in headers {
            let Some(pointer) = g.block.block else {
                continue;
            };
            out.push(TargetInfo {
                target: Target::new(kind, bsp, i as u32),
                tag_name: name.clone(),
                vertices: usize::from(g.info.total_vertex_count),
                triangles: usize::from(g.info.total_triangle_count),
                parts: usize::from(g.info.total_part_count),
                local: pointer.file == ResourceFile::Local,
            });
        }
    }
    out
}

/// A render model's sections (reads its raw blocks through `raw`).
pub fn list_model<R: Read + Seek>(
    map: &mut Map<R>,
    raw: &mut dyn RawSource,
    model: TagId,
) -> tags::Result<Vec<TargetInfo>> {
    let m = RenderModelGeometry::read(map, model, raw)?;
    let name = tag_name(map, model);
    Ok(m.sections
        .iter()
        .enumerate()
        .map(|(i, s)| TargetInfo {
            target: Target::new(TargetKind::ModeSection, model, i as u32),
            tag_name: name.clone(),
            vertices: s.geometry.vertex_count(),
            triangles: s
                .geometry
                .parts
                .iter()
                .map(|p| s.geometry.part_triangles(p).len())
                .sum(),
            parts: s.geometry.parts.len(),
            local: s.raw.file == ResourceFile::Local,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_ids_round_trip() {
        for kind in TargetKind::ALL {
            let t = Target::new(kind, TagId(0xe100_0004), 7);
            assert_eq!(Target::parse(&t.id()), Some(t));
        }
        assert_eq!(Target::parse("cluster:e1000004"), None);
        assert_eq!(Target::parse("portal:e1000004:0"), None);
        assert_eq!(Target::parse("cluster:e1000004:0:1"), None);
        assert_eq!(
            Target::parse("cluster:0xe1000004:2"),
            Some(Target::new(TargetKind::Cluster, TagId(0xe100_0004), 2))
        );
    }
}
