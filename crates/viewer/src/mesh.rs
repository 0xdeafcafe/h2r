//! Flattens a decoded render model into one indexed triangle list.
use anyhow::{Result, anyhow, bail};
use std::collections::{HashMap, HashSet};
use tags::model::RenderModelGeometry;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

pub struct Mesh {
    pub vertices: Vec<GpuVertex>,
    pub indices: Vec<u32>,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub sections: usize,
    pub parts: usize,
}

/// Output budget, well under wgpu's default 256 MiB buffer limit (36-byte
/// vertices, 4-byte indices). The largest retail model needs far less.
pub const MAX_VERTICES: usize = 4 << 20;
pub const MAX_INDICES: usize = 12 << 20;

/// One section's drawable input: shared vertex attributes, and the
/// triangles of each drawn part with its material index.
pub struct SectionInput<'a> {
    pub positions: &'a [[f32; 3]],
    pub normals: &'a [[f32; 3]],
    pub parts: Vec<(u16, Vec<[u16; 3]>)>,
}

/// A stable, distinct colour per material index (`0xffff` = none).
fn material_color(i: u16) -> [f32; 3] {
    if i == u16::MAX {
        return [0.7, 0.7, 0.7];
    }
    let h = (f32::from(i) * 0.618_034).fract();
    let k = |n: f32| {
        let x = (n + h * 6.0) % 6.0;
        1.0 - (x.min(4.0 - x).clamp(0.0, 1.0))
    };
    let (r, g, b) = (k(5.0), k(3.0), k(1.0));
    [0.35 + 0.6 * r, 0.35 + 0.6 * g, 0.35 + 0.6 * b]
}

/// The drawable inputs of `sections` (duplicates once; a missing section
/// is an error). Part kinds the engine skips (0 and 5) are left out.
pub fn section_inputs<'a>(
    model: &'a RenderModelGeometry,
    sections: &[usize],
) -> Result<Vec<SectionInput<'a>>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for &si in sections {
        if !seen.insert(si) {
            continue;
        }
        let s = model
            .sections
            .get(si)
            .ok_or_else(|| anyhow!("section {si} does not exist"))?;
        let g = &s.geometry;
        out.push(SectionInput {
            positions: &s.vertices.positions,
            normals: &s.vertices.normals,
            parts: g
                .parts
                .iter()
                .filter(|p| p.is_drawn())
                .map(|p| (p.material, g.part_triangles(p)))
                .collect(),
        });
    }
    Ok(out)
}

impl Mesh {
    /// Build from explicit sections (default: each region's best LOD).
    pub fn build(model: &RenderModelGeometry, sections: &[usize]) -> Result<Self> {
        Self::from_inputs(&section_inputs(model, sections)?, MAX_VERTICES, MAX_INDICES)
    }

    /// Build with explicit limits; over-budget output is an error.
    pub fn from_inputs(
        inputs: &[SectionInput<'_>],
        max_vertices: usize,
        max_indices: usize,
    ) -> Result<Self> {
        let mut vertices: Vec<GpuVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut parts = 0;
        for s in inputs {
            // Vertices are duplicated per part so each carries its material colour.
            for (material, tris) in &s.parts {
                parts += 1;
                let color = material_color(*material);
                if indices.len() + tris.len() * 3 > max_indices {
                    bail!("mesh exceeds {max_indices} indices");
                }
                let mut remap = HashMap::new();
                for t in tris {
                    for &i in t {
                        let slot = match remap.get(&i) {
                            Some(&s) => s,
                            None => {
                                if vertices.len() >= max_vertices {
                                    bail!("mesh exceeds {max_vertices} vertices");
                                }
                                let at = usize::from(i);
                                vertices.push(GpuVertex {
                                    position: *s
                                        .positions
                                        .get(at)
                                        .ok_or_else(|| anyhow!("index {at} has no vertex"))?,
                                    normal: s.normals.get(at).copied().unwrap_or([0.0, 0.0, 1.0]),
                                    color,
                                });
                                let s = u32::try_from(vertices.len() - 1)?;
                                remap.insert(i, s);
                                s
                            }
                        };
                        indices.push(slot);
                    }
                }
            }
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in &vertices {
            for k in 0..3 {
                min[k] = min[k].min(v.position[k]);
                max[k] = max[k].max(v.position[k]);
            }
        }
        if vertices.is_empty() {
            (min, max) = ([-1.0; 3], [1.0; 3]);
        }
        Ok(Self {
            vertices,
            indices,
            min,
            max,
            sections: inputs.len(),
            parts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUAD: [[f32; 3]; 4] = [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]];

    /// One section: a quad (2 triangles) in one part.
    fn input() -> SectionInput<'static> {
        SectionInput {
            positions: &QUAD,
            normals: &[],
            parts: vec![(0, vec![[0, 1, 2], [1, 3, 2]])],
        }
    }

    #[test]
    fn builds_one_section() {
        let m = Mesh::from_inputs(&[input()], 100, 100).unwrap();
        assert_eq!((m.vertices.len(), m.indices.len(), m.sections), (4, 6, 1));
        assert_eq!(m.max, [1.0, 1.0, 0.0]);
    }

    #[test]
    fn budgets_and_bad_indices_are_errors() {
        assert!(Mesh::from_inputs(&[input()], 3, 100).is_err());
        assert!(Mesh::from_inputs(&[input()], 100, 5).is_err());
        assert!(Mesh::from_inputs(&[input()], 4, 6).is_ok());
        let mut bad = input();
        bad.parts[0].1.push([0, 1, 9]);
        assert!(Mesh::from_inputs(&[bad], 100, 100).is_err());
    }

    #[test]
    fn missing_sections_are_errors() {
        use tags::records::ModeGeometryHeader;
        use tags::wire::BinaryRecord;
        let model = RenderModelGeometry {
            id: map::TagId(0),
            name: None,
            header: ModeGeometryHeader::decode(&[0; ModeGeometryHeader::SIZE]).unwrap(),
            compression: None,
            regions: vec![],
            nodes: vec![],
            materials: vec![],
            sections: vec![],
            validation: Default::default(),
        };
        assert!(section_inputs(&model, &[0]).is_err());
        assert!(section_inputs(&model, &[]).unwrap().is_empty());
    }
}
