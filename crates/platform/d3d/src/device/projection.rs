//! The host draw's horizontal lens. The GPU register file and guest memory
//! retain their original values, including the matrix used for visibility.
use super::*;

pub(super) struct DisplayProjection {
    factor: f32,
    #[cfg(feature = "gpu")]
    last: Option<(crate::shader::Constants, usize, crate::shader::Constants)>,
}

impl Default for DisplayProjection {
    fn default() -> Self {
        Self {
            factor: 1.0,
            #[cfg(feature = "gpu")]
            last: None,
        }
    }
}

impl<B: Backend> Device<B> {
    pub fn set_projection_factor(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 && factor != self.projection.factor {
            self.projection = DisplayProjection {
                factor,
                ..Default::default()
            };
        }
    }

    /// Horizontal multiplier selected for the current view, applied after
    /// the guest has submitted its visible objects.
    pub fn projection_factor(&self) -> f32 {
        self.projection.factor
    }

    pub fn projection_factor_for_target(&self, size: (u32, u32)) -> f32 {
        if self.ui_fit.is_none() && size == (640, 480) {
            self.projection.factor
        } else {
            1.0
        }
    }
}

#[cfg(feature = "gpu")]
fn is_perspective(rows: &[[f32; 4]]) -> bool {
    let m = std::array::from_fn(|i| rows[i].map(f64::from));
    crate::interp::camera::Persp::split(&m).is_some()
}

/// Widen the horizontal row of a captured perspective matrix, leaving
/// orthographic matrices and the remaining three rows intact.
#[cfg(feature = "gpu")]
pub fn widen_projection_rows(rows: &mut [[f32; 4]], factor: f32) -> bool {
    if factor == 1.0
        || !factor.is_finite()
        || factor <= 0.0
        || rows.len() != 4
        || !is_perspective(rows)
    {
        return false;
    }
    rows[0] = rows[0].map(|x| x * factor);
    true
}

#[cfg(feature = "gpu")]
impl DisplayProjection {
    pub(super) fn for_draw(
        &mut self,
        shader: &mut ShaderInputs,
        scene_target: bool,
    ) -> Option<crate::shader::Constants> {
        if !scene_target || self.factor == 1.0 {
            return None;
        }
        let VertexProcessing::Program { microcode, .. } = &shader.vertex else {
            return None;
        };
        let crate::interp::epilogue::Position::Rows(row) = microcode.clip_position() else {
            return None;
        };
        let row = usize::from(row);
        // The shader computes all four clip coordinates with this matrix.
        // Post quads and orthographic menus keep their own mappings.
        if let Some((source, r, result)) = &self.last
            && *r == row
            && Arc::ptr_eq(source, &shader.constants)
        {
            shader.constants = result.clone();
            return Some(source.clone());
        }
        let mut rows: [[f32; 4]; 4] = shader.constants[row..row + 4].try_into().ok()?;
        if !widen_projection_rows(&mut rows, self.factor) {
            return None;
        }
        let original = shader.constants.clone();
        Arc::make_mut(&mut shader.constants)[row] = rows[0];
        self.last = Some((original.clone(), row, shader.constants.clone()));
        Some(original)
    }
}

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::*;

    #[test]
    fn immutable_capture_widens_only_the_perspective_program_on_a_scene_surface() {
        use shaders::vsh::{
            MacOp, OutReg,
            encode::{Inst, Operand},
        };
        let mut c = [[0.0; 4]; CONSTANTS];
        let start = 8usize;
        c[start..start + 4].copy_from_slice(&[
            [1.2, 0.0, 0.0, 0.0],
            [0.0, 1.6, 0.0, 0.0],
            [0.0, 0.0, 1.01, -0.2],
            [0.0, 0.0, 1.0, 0.0],
        ]);
        let program: Vec<_> = (0..4u8)
            .flat_map(|i| {
                let op = Inst::mac(
                    MacOp::Dp4,
                    Some(Operand::v(0)),
                    Some(Operand::c(start as u8 + i)),
                    None,
                )
                .to_out(false, OutReg::Pos, 8 >> i);
                if i == 3 {
                    op.last().words()
                } else {
                    op.words()
                }
            })
            .collect();
        let source = Arc::new(c);
        let base = ShaderInputs {
            vertex: VertexProcessing::Program {
                start: 0,
                microcode: program.into(),
            },
            provided_inputs: 1,
            vertex_defaults: [[0.; 4]; 16],
            constants: source.clone(),
            pixel_shader: None,
            combiner_state: Arc::new([0; 57]),
            final_combiner_latch: false,
            affine_projection: false,
        };
        let mut projection = DisplayProjection {
            factor: 0.5,
            ..Default::default()
        };
        let mut world = base.clone();
        let retail = projection.for_draw(&mut world, true).unwrap();
        assert!(Arc::ptr_eq(&source, &retail));
        assert_eq!(world.constants[start], source[start].map(|x| x * 0.5));
        assert_eq!(world.constants[start + 1..], source[start + 1..]);
        let mut offscreen = base.clone();
        assert!(projection.for_draw(&mut offscreen, false).is_none());
        assert!(Arc::ptr_eq(&offscreen.constants, &source));
        let mut quad = base;
        let pass = Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None)
            .to_out(false, OutReg::Pos, 15)
            .last();
        quad.vertex = VertexProcessing::Program {
            start: 0,
            microcode: pass.words().to_vec().into(),
        };
        assert!(projection.for_draw(&mut quad, true).is_none());
        assert!(Arc::ptr_eq(&quad.constants, &source));
    }

    #[test]
    fn only_the_perspective_lens_accepts_horizontal_projection() {
        let view = [
            [1.2, 0.0, 0.0, -5.0],
            [0.0, 1.6, 0.0, 3.0],
            [0.0, 0.0, 1.01, -0.2],
            [0.0, 0.0, 1.0, 0.0],
        ];
        let mut wide = view;
        assert!(widen_projection_rows(&mut wide, 0.5));
        assert_eq!(wide[0], view[0].map(|v| v * 0.5));
        assert_eq!(wide[1..], view[1..]);
        let mut ortho = view;
        ortho[3] = [0., 0., 0., 1.];
        assert!(!is_perspective(&ortho));
    }
}
