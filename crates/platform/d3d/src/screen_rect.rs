//! Where a small unindexed draw's vertices land in the guest window: each
//! vertex run through the draw's vertex program, or read as pre-transformed
//! (XYZRHW) coordinates, with its texture coordinates. One reading serves
//! two callers: the device decides from it whether a draw in a fitted UI
//! scope is a full-frame pass (docs/graphics/high-resolution.md), and
//! `render.frame_layout` reports it as `window_xy` (docs/runtime/agent-api.md).
//! Positions written by the program from constant rows, from inline
//! Begin/End vertices or from the `SetVertexData` defaults come out the same
//! way as positions read from a vertex buffer.

use gfx::vertex::VertexLayout;
use shaders::vsh::{OutDest, OutReg, Program, ProgramKind};

use crate::backend::{ResolvedDraw, StreamBytes};
use crate::shader::{ShaderInputs, VertexProcessing};

/// `[[min x, min y], [max x, max y]]`.
pub type Bounds = [[f32; 2]; 2];

/// One vertex as the rasteriser sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenVertex {
    /// Guest window position (`oPos.xy`; XYZRHW positions with their
    /// half-pixel bias).
    pub xy: [f32; 2],
    /// `oT0..oT3`, unprojected.
    pub tex: [[f32; 4]; 4],
}

/// The inputs a draw's window positions are computed from.
#[derive(Clone, Copy)]
pub struct DrawVertices<'a> {
    /// The vertex program or FVF, its constants and the input defaults.
    pub shader: &'a ShaderInputs,
    /// Where each register's attribute sits in its stream.
    pub layout: &'a VertexLayout,
    /// One per stream of `layout`.
    pub streams: &'a [StreamBytes],
    /// Vertices in `streams`.
    pub vertex_count: u32,
    /// The draw selects its vertices through indices.
    pub indexed: bool,
    /// `D3DTSS_TEXCOORDINDEX` per stage (pre-transformed draws only).
    pub tex_coord_index: [u32; 4],
}

impl<'a> DrawVertices<'a> {
    pub fn of(draw: &'a ResolvedDraw) -> Self {
        Self {
            shader: &draw.shader,
            layout: &draw.layout,
            streams: &draw.streams,
            vertex_count: draw.vertex_count,
            indexed: draw.indices.is_some(),
            tex_coord_index: std::array::from_fn(|stage| draw.state.stages[stage].tex_coord_index),
        }
    }

    /// Whether the positions come from a vertex program.
    pub fn programmable(&self) -> bool {
        matches!(self.shader.vertex, VertexProcessing::Program { .. })
    }

    /// Every vertex in the window, or `None`: indexed, empty or more than
    /// `max_vertices`, fixed-function geometry that is not pre-transformed
    /// (its positions are model space), an unreadable program or input.
    pub fn screen(&self, max_vertices: u32) -> Option<Vec<ScreenVertex>> {
        if self.indexed || self.vertex_count == 0 || self.vertex_count > max_vertices {
            return None;
        }
        let program = match &self.shader.vertex {
            VertexProcessing::Program { microcode, .. } => {
                Some(Program::from_words(ProgramKind::Normal, microcode).ok()?)
            }
            VertexProcessing::FixedFunction { .. } if self.layout.pretransformed => None,
            VertexProcessing::FixedFunction { .. } => return None,
        };
        let Some(program) = program else {
            return (0..self.vertex_count as usize)
                .map(|vertex| self.pretransformed(&self.inputs(vertex)?))
                .collect();
        };
        // A screen pass reads its constants. A program that also writes
        // them sees each vertex start from the draw's block.
        let writes_constants = program
            .instructions
            .iter()
            .any(|i| i.out.is_some_and(|o| matches!(o.dest, OutDest::Const(_))));
        let mut constants = *self.shader.constants;
        (0..self.vertex_count as usize)
            .map(|vertex| {
                let inputs = self.inputs(vertex)?;
                if writes_constants && vertex > 0 {
                    constants = *self.shader.constants;
                }
                let out = shaders::vsh::interp::run(&program, &inputs, &mut constants);
                let [x, y, _, _] = out.get(OutReg::Pos);
                Some(ScreenVertex {
                    xy: [x, y],
                    tex: std::array::from_fn(|n| out.get(OutReg::T(n as u8))),
                })
            })
            .collect()
    }

    /// The bounds of [`screen`](Self::screen)'s positions.
    pub fn window_rect(&self, max_vertices: u32) -> Option<Bounds> {
        bounds(self.screen(max_vertices)?.iter().map(|v| v.xy))
    }

    /// XYZRHW uses the same half-pixel bias and texture input selection
    /// as the translated pre-transformed program.
    fn pretransformed(&self, inputs: &[[f32; 4]; 16]) -> Option<ScreenVertex> {
        let [x, y, _, _] = inputs[0];
        let mut tex = [[0.0, 0.0, 0.0, 1.0]; 4];
        for (uv, &index) in tex.iter_mut().zip(&self.tex_coord_index) {
            if index >= 4 {
                return None;
            }
            *uv = inputs[9 + index as usize];
        }
        Some(ScreenVertex { xy: [x + 0.5, y + 0.5], tex })
    }

    /// The 16 input registers of `vertex`: the stream attributes over the
    /// `SetVertexData` defaults.
    fn inputs(&self, vertex: usize) -> Option<[[f32; 4]; 16]> {
        let mut inputs = self.shader.vertex_defaults;
        for (stream, source) in self.layout.streams.iter().zip(self.streams) {
            let stride = if source.stride == 0 {
                stream.stride
            } else {
                source.stride
            } as usize;
            let data: &[u8] = source.data.as_ref();
            for attr in &stream.attributes {
                let value = inputs.get_mut(attr.register.0 as usize)?;
                let start = vertex * stride + attr.offset as usize;
                let bytes = data.get(start..start + attr.data_type.byte_size() as usize)?;
                *value = crate::device::attribute_value(attr.data_type, bytes);
            }
        }
        Some(inputs)
    }
}

/// The bounds of `points`; `None` when empty or any is not finite.
pub fn bounds(mut points: impl Iterator<Item = [f32; 2]>) -> Option<Bounds> {
    let first = points.next()?;
    let mut range = [first, first];
    for point in std::iter::once(first).chain(points) {
        if !point.iter().all(|v| v.is_finite()) {
            return None;
        }
        for (axis, value) in point.into_iter().enumerate() {
            range[0][axis] = range[0][axis].min(value);
            range[1][axis] = range[1][axis].max(value);
        }
    }
    Some(range)
}
