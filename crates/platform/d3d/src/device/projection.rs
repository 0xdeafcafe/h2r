//! Horizontal correction for draws whose captured projection or screen-space
//! material inputs still use the retail lens.
use super::*;

pub(super) struct DisplayProjection {
    factor: f32,
    camera_factor: f32,
    #[cfg(feature = "gpu")]
    last: Option<(crate::shader::Constants, usize, crate::shader::Constants)>,
}

impl DisplayProjection {
    /// The camera's own lens factor, as bits (a run's state key).
    pub(super) fn camera_factor_bits(&self) -> u32 {
        self.camera_factor.to_bits()
    }
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
pub use gfx::guest_program::{is_perspective, widen_projection_rows};

#[cfg(feature = "gpu")]
impl DisplayProjection {
    fn matrix_row(shader: &ShaderInputs) -> Option<usize> {
        let VertexProcessing::Program { microcode, .. } = &shader.vertex else {
            return None;
        };
        use crate::clip_space::epilogue::Position;
        use crate::shader::MicrocodeClip;
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
            let (x, y) = (canvas_fit::horizontal_term(self.camera_factor), canvas_fit::vertical_term(self.camera_factor));
            stage.bump_env_mat[0] *= x;
            stage.bump_env_mat[2] *= x;
            stage.bump_env_mat[1] *= y;
            stage.bump_env_mat[3] *= y;
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
    use crate::shader::MicrocodeClip;

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

    /// Halo 2's water and sky programs take the display lens at 16:9 and
    /// 21:9, as the world's do. The water's clip goes through a temporary
    /// (`dph r10.xyz`, `dph r10.w` also to `oPos.w`, then the ILU's `mov
    /// oPos.xyz, r10`); the sky's is clamped to the far plane (its W row
    /// also written to Z).
    #[test]
    fn water_and_sky_programs_take_the_wide_lens() {
        use shaders::vsh::{
            IluOp, MacOp, OutReg,
            encode::{Inst, Operand},
        };
        const W: u8 = 1;
        let row = |i: u8| Some(Operand::c(i));
        let mut water: Vec<Inst> = (0..3)
            .map(|i| Inst::mac(MacOp::Dph, Some(Operand::v(0)), row(i), None).to_temp(10, 8 >> i))
            .collect();
        water.push(
            Inst::mac(MacOp::Dph, Some(Operand::v(0)), row(3), None)
                .to_temp(10, W)
                .to_out(false, OutReg::Pos, W),
        );
        water.push(
            Inst::mac(MacOp::Dp3, Some(Operand::r(11)), Some(Operand::r(11)), None)
                .to_temp(11, W)
                .with_ilu(IluOp::Mov, Operand::r(10))
                .to_out(true, OutReg::Pos, 0xE)
                .last(),
        );
        let sky: Vec<Inst> = [(0u8, 8u8), (1, 4), (3, 2 | W)]
            .into_iter()
            .enumerate()
            .map(|(n, (i, mask))| {
                let op = Inst::mac(MacOp::Dp4, Some(Operand::v(0)), row(i), None)
                    .to_out(false, OutReg::Pos, mask);
                if n == 2 { op.last() } else { op }
            })
            .collect();
        let mut c = [[0.0; 4]; CONSTANTS];
        c[..4].copy_from_slice(&[
            [1.2, 0.0, 0.0, 0.0],
            [0.0, 1.6, 0.0, 0.0],
            [0.0, 0.0, 1.01, -0.2],
            [0.0, 0.0, 1.0, 0.0],
        ]);
        let source = Arc::new(c);
        for (name, program, clip) in [("water", water, "rows(0)"), ("sky", sky, "far(0)")] {
            let words: Vec<u32> = program.iter().flat_map(|i| i.words()).collect();
            for output in [(2560, 1440), (3440, 1440)] {
                let factor = canvas_fit::Lens::for_window(output).factor();
                assert!(factor < 1.0, "{output:?}");
                let mut draw = ShaderInputs {
                    vertex: VertexProcessing::Program { start: 0, microcode: words.clone().into() },
                    provided_inputs: 1,
                    vertex_defaults: [[0.; 4]; 16],
                    constants: source.clone(),
                    pixel_shader: None,
                    combiner_state: Arc::new([0; 57]),
                    final_combiner_latch: false,
                    affine_projection: false,
                };
                if let VertexProcessing::Program { microcode, .. } = &draw.vertex {
                    assert_eq!(microcode.clip_text(), clip, "{name}");
                }
                let mut projection = DisplayProjection { factor, ..Default::default() };
                let retail = projection.for_draw(&mut draw, true);
                assert!(retail.is_some_and(|r| Arc::ptr_eq(&r, &source)), "{name} at {output:?} kept the retail lens");
                let x = canvas_fit::horizontal_term(factor);
                assert_eq!(draw.constants[0], source[0].map(|v| v * x), "{name} at {output:?}");
                assert_eq!(draw.constants[1..], source[1..], "{name} at {output:?}");
            }
        }
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
        // A portrait window grows the vertical lens and leaves the row of
        // the horizontal one alone.
        for output in [(960, 1280), (720, 1280)] {
            let lens = canvas_fit::Lens::for_window(output);
            let mut tall = view;
            assert!(widen_projection_rows(&mut tall, lens.factor()));
            assert_eq!(tall[0], view[0], "{output:?}");
            assert_eq!(tall[1], view[1].map(|v| v * lens.y), "{output:?}");
            assert_eq!(tall[2..], view[2..]);
        }
        let mut ortho = view;
        ortho[3] = [0., 0., 0., 1.];
        assert!(!is_perspective(&ortho));
    }
}
