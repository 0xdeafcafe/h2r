//! Lens flare fans. Ownership ends at one BEGIN/END pair. The bridge
//! identifies visibility and colour geometry, while the guest keeps the
//! query tickets and per-flare loop.
//!
//! [`Site`] is pure data. `FlareGpu` (feature `gpu`) wraps
//! `UiGpu` behind a per-site contract: it checks the
//! primitive, the vertex registers, the program inputs and the texture
//! dimensions before the shared shader and pipeline path draws the fan.
//! docs/graphics/native-particles.md.

/// One flare fan. Each is one `TriangleFan` of four vertices, no indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Site {
    /// Read-only visibility fan inside an active z-pass report.
    Visibility,
    /// Reflection billboard: position, uv and a carried colour.
    Reflection,
    /// Position only; the colour is a carried default.
    Mask,
    /// Position and uv.
    MaskSample,
    /// Downsample: position and uv.
    FilterSource,
    /// Filter: position and uv.
    Filter,
    /// Additive composite: position, uv and a carried register 9.
    Composite,
}

impl Site {
    /// Every site, in the order a frame issues them.
    pub const ALL: [Site; 7] = [
        Self::Visibility,
        Self::Reflection,
        Self::Mask,
        Self::MaskSample,
        Self::FilterSource,
        Self::Filter,
        Self::Composite,
    ];

    /// The site's part of a counter's name (`scene_kept_flare_<site>_*`).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Visibility => "visibility",
            Self::Reflection => "reflection",
            Self::Mask => "mask",
            Self::MaskSample => "mask_sample",
            Self::FilterSource => "filter_source",
            Self::Filter => "filter",
            Self::Composite => "composite",
        }
    }

    /// The vertex registers written for the fan (bit `r` is register `r`).
    /// Registers outside it come from the carried defaults.
    pub const fn registers(self) -> u16 {
        match self {
            Self::Visibility | Self::Mask => 1,
            Self::MaskSample | Self::FilterSource | Self::Filter => 1 | 1 << 3,
            Self::Reflection | Self::Composite => 1 | 1 << 3 | 1 << 9,
        }
    }

    /// Whether `written` is exactly the site's register set.
    pub const fn accepts_registers(self, written: u16) -> bool {
        written == self.registers()
    }
}

#[cfg(feature = "gpu")]
pub use gpu::{FlareGpu, Refusal, check};

#[cfg(feature = "gpu")]
mod gpu {
    use super::Site;
    use crate::ui_gpu::{self, UiGpu};
    use d3d::backend::{ExternalAttachments, ExternalDraws, ResolvedDraw, StreamBytes};
    use d3d::shader::VertexProcessing;
    use gfx::texture::TextureKind;
    use gfx::vertex::{PrimitiveType, VertexDataType, VertexLayout};

    /// Why a fan stays with the ordinary draw path.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Refusal {
        /// Not one `TriangleFan` of four vertices with no index data.
        Geometry,
        /// The program, registers or stream do not match the site.
        Layout,
        /// A bound texture is not 2D, or a glyph filter is set.
        Texture,
        /// The query state does not match the read-only visibility draw.
        QueryState,
        /// The shared UI path refused.
        Ui(ui_gpu::Refusal),
    }

    impl Refusal {
        /// The refusal's part of a kept fan's counter name
        /// (`scene_kept_flare_<site>_<cause>`).
        pub const fn cause(self) -> &'static str {
            match self {
                Self::Geometry => "geometry",
                Self::Layout => "layout",
                Self::Texture => "texture",
                Self::QueryState => "query_state",
                Self::Ui(_) => "ui",
            }
        }
    }

    const STRIDE: u32 = 256;

    /// Check a resolved draw against the site's contract. No device.
    pub fn check(site: Site, draw: &ResolvedDraw) -> Result<(), Refusal> {
        let visibility = site == Site::Visibility;
        if draw.hw.zpass_count_enable != visibility
            || (visibility
                && (draw.target.depth_format.is_none()
                    || draw.state.render.color_write.xbox_value() != 0
                    || draw.writes_depth_stencil()))
        {
            return Err(Refusal::QueryState);
        }
        check_shape(
            site,
            draw.primitive,
            draw.vertex_count,
            draw.indices.is_some(),
            &draw.layout,
            &draw.streams,
            matches!(draw.shader.vertex, VertexProcessing::Program { .. }),
            draw.shader.provided_inputs,
        )?;
        if draw.glyph_cache != 0
            || draw
                .texture_info
                .iter()
                .flatten()
                .any(|info| !matches!(info.kind, TextureKind::D2))
        {
            return Err(Refusal::Texture);
        }
        Ok(())
    }

    /// The geometry and register contract. The guest's vertex program reads
    /// the immediate stream: stream 0 of stride 256 holding one `Float4` per
    /// written register at `16 * register`. The written set must equal
    /// [`Site::registers`] and the shader's provided inputs. Pre-transformed
    /// and fixed-function layouts are not supported.
    #[allow(clippy::too_many_arguments)]
    fn check_shape(
        site: Site,
        primitive: PrimitiveType,
        vertex_count: u32,
        indexed: bool,
        layout: &VertexLayout,
        streams: &[StreamBytes],
        program: bool,
        provided: u16,
    ) -> Result<(), Refusal> {
        if primitive != PrimitiveType::TriangleFan || vertex_count != 4 || indexed {
            return Err(Refusal::Geometry);
        }
        if !program || layout.pretransformed {
            return Err(Refusal::Layout);
        }
        let ([stream], [bytes]) = (layout.streams.as_slice(), streams) else {
            return Err(Refusal::Layout);
        };
        if stream.stream != 0
            || stream.stride != STRIDE
            || bytes.stride != STRIDE
            || bytes.data.len() != 4 * STRIDE as usize
        {
            return Err(Refusal::Layout);
        }
        let mut written = 0u16;
        for a in &stream.attributes {
            let r = u32::from(a.register.0);
            if r >= 16
                || written & (1 << r) != 0
                || a.data_type != VertexDataType::Float4
                || a.offset != 16 * r
            {
                return Err(Refusal::Layout);
            }
            written |= 1 << r;
        }
        if !site.accepts_registers(written) || provided != written {
            return Err(Refusal::Layout);
        }
        Ok(())
    }

    /// The flare fans' GPU path: the shared UI shader, pipeline and upload
    /// path behind a per-site contract.
    ///
    /// `NativeDrawProgram::resolve_ui` refuses (`Ui(Shader)` or `Ui(Texture)`)
    /// while hot reload is active, for a fixed-function draw the ordinary
    /// backend renders with its built-in diffuse program, for a sampled
    /// stage with no resolved view, and for shadow textures. `UiGpu` also
    /// refuses a missing clip, a target or depth format that differs from the
    /// attachments, and a texture that aliases the colour or depth target.
    #[derive(Default)]
    pub struct FlareGpu(UiGpu);

    impl FlareGpu {
        pub fn prepare(
            &mut self,
            site: Site,
            target: &ExternalAttachments,
            draw: &ResolvedDraw,
            frame: u32,
        ) -> Result<ExternalDraws, Refusal> {
            check(site, draw)?;
            self.0.prepare(target, draw, frame).map_err(Refusal::Ui)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use gfx::vertex::{StreamLayout, VertexAttribute, VertexRegister};

        const SITES: [Site; 7] = Site::ALL;

        #[test]
        fn every_site_and_refusal_has_its_own_counter_name() {
            let sites: std::collections::BTreeSet<_> = SITES.iter().map(|s| s.name()).collect();
            assert_eq!(sites.len(), SITES.len());
            let causes: std::collections::BTreeSet<_> = [
                Refusal::Geometry,
                Refusal::Layout,
                Refusal::Texture,
                Refusal::QueryState,
            ]
            .iter()
            .map(|r| r.cause())
            .collect();
            assert_eq!(causes.len(), 4);
        }

        fn layout(registers: &[u8]) -> VertexLayout {
            VertexLayout {
                streams: vec![StreamLayout {
                    stream: 0,
                    attributes: registers
                        .iter()
                        .map(|&r| VertexAttribute {
                            register: VertexRegister(r),
                            data_type: VertexDataType::Float4,
                            offset: 16 * u32::from(r),
                        })
                        .collect(),
                    stride: STRIDE,
                }],
                pretransformed: false,
            }
        }

        fn bytes(len: usize) -> Vec<StreamBytes> {
            vec![StreamBytes {
                data: vec![0u8; len].into(),
                stride: STRIDE,
            }]
        }

        fn mask_of(registers: &[u8]) -> u16 {
            registers.iter().fold(0, |m, r| m | 1 << r)
        }

        fn shape(site: Site, registers: &[u8], provided: u16) -> Result<(), Refusal> {
            check_shape(
                site,
                PrimitiveType::TriangleFan,
                4,
                false,
                &layout(registers),
                &bytes(1024),
                true,
                provided,
            )
        }

        #[test]
        fn each_site_accepts_exactly_its_register_set() {
            for site in SITES {
                let regs: Vec<u8> = (0..16).filter(|r| site.registers() & 1 << r != 0).collect();
                assert_eq!(shape(site, &regs, mask_of(&regs)), Ok(()), "{site:?}");
            }
        }

        #[test]
        fn a_wrong_written_mask_is_refused() {
            // Mask writes only position: a uv, or no position, is wrong.
            assert_eq!(shape(Site::Mask, &[0, 3], 0b1001), Err(Refusal::Layout));
            assert_eq!(shape(Site::MaskSample, &[0], 0b1), Err(Refusal::Layout));
            assert_eq!(shape(Site::MaskSample, &[3], 0b1000), Err(Refusal::Layout));
            // A register the site never writes, or a missing carried one.
            assert_eq!(
                shape(Site::MaskSample, &[0, 3, 9], 0b10_0000_1001),
                Err(Refusal::Layout)
            );
            assert_eq!(
                shape(Site::Reflection, &[0, 3], 0b1001),
                Err(Refusal::Layout)
            );
            assert_eq!(shape(Site::Composite, &[0, 3, 9], 0b10_0000_1001), Ok(()));
            // Reflection has the register set of Composite but not of Filter.
            assert_eq!(
                shape(Site::Filter, &[0, 3, 9], 0b10_0000_1001),
                Err(Refusal::Layout)
            );
        }

        #[test]
        fn provided_inputs_must_equal_the_written_set() {
            assert_eq!(shape(Site::MaskSample, &[0, 3], 0b1001), Ok(()));
            assert_eq!(
                shape(Site::MaskSample, &[0, 3], 0b1011),
                Err(Refusal::Layout)
            );
            assert_eq!(
                shape(Site::MaskSample, &[0, 3], 0b0001),
                Err(Refusal::Layout)
            );
        }

        #[test]
        fn geometry_and_stream_shape_are_exact() {
            let l = layout(&[0]);
            let go =
                |p, n, i, s: &[StreamBytes], prog| check_shape(Site::Mask, p, n, i, &l, s, prog, 1);
            let fan = PrimitiveType::TriangleFan;
            assert_eq!(
                go(PrimitiveType::TriangleStrip, 4, false, &bytes(1024), true),
                Err(Refusal::Geometry)
            );
            assert_eq!(
                go(fan, 5, false, &bytes(1280), true),
                Err(Refusal::Geometry)
            );
            assert_eq!(go(fan, 4, true, &bytes(1024), true), Err(Refusal::Geometry));
            assert_eq!(go(fan, 4, false, &bytes(768), true), Err(Refusal::Layout));
            assert_eq!(go(fan, 4, false, &[], true), Err(Refusal::Layout));
            assert_eq!(go(fan, 4, false, &bytes(1024), false), Err(Refusal::Layout));
            assert_eq!(go(fan, 4, false, &bytes(1024), true), Ok(()));
        }

        #[test]
        fn stream_index_offsets_types_and_pretransform_are_checked() {
            let go = |l: &VertexLayout| {
                check_shape(
                    Site::MaskSample,
                    PrimitiveType::TriangleFan,
                    4,
                    false,
                    l,
                    &bytes(1024),
                    true,
                    0b1001,
                )
            };
            let mut l = layout(&[0, 3]);
            assert_eq!(go(&l), Ok(()));
            l.streams[0].stream = 1;
            assert_eq!(go(&l), Err(Refusal::Layout));
            let mut l = layout(&[0, 3]);
            l.streams[0].attributes[1].offset = 32;
            assert_eq!(go(&l), Err(Refusal::Layout));
            let mut l = layout(&[0, 3]);
            l.streams[0].attributes[1].data_type = VertexDataType::Float2;
            assert_eq!(go(&l), Err(Refusal::Layout));
            let mut l = layout(&[0, 3]);
            l.streams[0].attributes[1].register = VertexRegister(0);
            l.streams[0].attributes[1].offset = 0;
            assert_eq!(go(&l), Err(Refusal::Layout));
            let mut l = layout(&[0, 3]);
            l.pretransformed = true;
            assert_eq!(go(&l), Err(Refusal::Layout));
        }
    }
}
