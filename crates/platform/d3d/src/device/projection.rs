//! Horizontal correction for draws whose captured projection or screen-space
//! material inputs still use the retail lens.
use super::*;

pub(super) struct DisplayProjection {
    factor: f32,
    camera_factor: f32,
    #[cfg(feature = "gpu")]
    last: Option<(crate::shader::Constants, usize, crate::shader::Constants)>,
}

impl Default for DisplayProjection {
    fn default() -> Self {
        Self {
            factor: 1.0,
            camera_factor: 1.0,
            #[cfg(feature = "gpu")]
            last: None,
        }
    }
}

impl<B: Backend> Device<B> {
    pub fn set_projection_factor(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0
            && (factor != self.projection.factor || self.projection.camera_factor != 1.0)
        {
            self.projection = DisplayProjection {
                factor,
                ..Default::default()
            };
        }
    }

    /// The camera already projects through this lens. Screen-space offsets
    /// stored outside the projection matrix still need its horizontal factor.
    pub fn set_camera_projection_factor(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            self.projection = DisplayProjection {
                camera_factor: factor,
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
    crate::clip_space::camera::Persp::split(&m).is_some()
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
    fn matrix_row(shader: &ShaderInputs) -> Option<usize> {
        let VertexProcessing::Program { microcode, .. } = &shader.vertex else {
            return None;
        };
        use crate::clip_space::epilogue::Position;
        let row = match microcode.clip_position() {
            Position::Rows(row) | Position::FarPlaneRows(row) => row,
            Position::Fixed | Position::Unknown => return None,
        };
        let row = usize::from(row);
        (row + 4 <= shader.constants.len()).then_some(row)
    }

    pub(super) fn fit_camera_bumps(
        &self,
        shader: &ShaderInputs,
        scene_target: bool,
        extras: &mut [crate::state::StageExtras],
    ) {
        if self.camera_factor == 1.0 || !scene_target || !extras.iter().any(|stage| stage.samples_frame) {
            return;
        }
        let Some(row) = Self::matrix_row(shader) else { return };
        if !is_perspective(&shader.constants[row..row + 4]) {
            return;
        }
        for stage in extras.iter_mut().filter(|stage| stage.samples_frame) {
            stage.bump_env_mat[0] *= self.camera_factor;
            stage.bump_env_mat[2] *= self.camera_factor;
        }
    }

    pub(super) fn for_draw(
        &mut self,
        shader: &mut ShaderInputs,
        scene_target: bool,
    ) -> Option<crate::shader::Constants> {
        if !scene_target || self.factor == 1.0 {
            return None;
        }
        let row = Self::matrix_row(shader)?;
        // The shader projects X, Y and W with this matrix.
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
        if let VertexProcessing::Program { microcode, .. } = &world.vertex {
            assert_eq!(microcode.clip_text(), "rows(8)");
        }
        let retail = projection.for_draw(&mut world, true).unwrap();
        assert!(Arc::ptr_eq(&source, &retail));
        assert_eq!(world.constants[start], source[start].map(|x| x * 0.5));
        assert_eq!(world.constants[start + 1..], source[start + 1..]);
        let mut offscreen = base.clone();
        assert!(projection.for_draw(&mut offscreen, false).is_none());
        assert!(Arc::ptr_eq(&offscreen.constants, &source));
        let mut projected = DisplayProjection { camera_factor: 0.75, ..Default::default() };
        let mut scene = world.clone();
        let mut extras = StateWords::defaults(false).stage_extras();
        for stage in &mut extras { stage.bump_env_mat = [8., 7., -4., 5.]; }
        extras[0].samples_frame = true;
        projected.fit_camera_bumps(&scene, false, &mut extras);
        assert_eq!(extras[0].bump_env_mat, extras[1].bump_env_mat);
        projected.fit_camera_bumps(&scene, true, &mut extras);
        assert_eq!(extras[0].bump_env_mat, [6., 7., -3., 5.]);
        assert_eq!(extras[1].bump_env_mat, [8., 7., -4., 5.]);
        assert!(projected.for_draw(&mut scene, true).is_none());
        let mut quad = base;
        let pass = Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None)
            .to_out(false, OutReg::Pos, 15)
            .last();
        quad.vertex = VertexProcessing::Program {
            start: 0,
            microcode: pass.words().to_vec().into(),
        };
        assert!(projection.for_draw(&mut quad, true).is_none());
        if let VertexProcessing::Program { microcode, .. } = &quad.vertex {
            assert_eq!(microcode.clip_text(), "fixed");
        }
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
