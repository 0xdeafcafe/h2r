//! The material's silhouette at a ray hit. Use its layer-10 depth implementation
//! (world 2, model 1). Evaluate cutouts with the CPU pixel interpreter at LOD 0.
use anyhow::{Context, Result, bail, ensure};
use assets::{Level, ResourceFiles};
use gfx::state::{CompareFunc, RenderState};
use gfx::xdk::{HALO2_BUILD, RenderStateTable};
use map::{Map, TagId};
use shaders::psh::{AlphaFunc, PixelOptions, PixelShaderDef, TexMode, TextureDim, interp};
use std::{
    collections::HashMap,
    io::{Read, Seek},
    sync::Arc,
};
use tags::material::{PassSelection, ShaderLibrary, TextureSource};

pub(super) enum Coverage {
    Solid,
    Mask(Box<Mask>),
    Empty,
}

impl Coverage {
    pub fn covers(&self, uv: [f32; 2]) -> bool {
        match self {
            Self::Empty => false,
            Self::Solid => true,
            Self::Mask(mask) => mask.covers(uv),
        }
    }
}

struct Texture {
    image: Arc<Level>,
    address: [u8; 2],
    border: [f32; 4],
    linear: bool,
    transform: [f32; 4],
}

impl Texture {
    fn sample(&self, uv: [f32; 2]) -> [f32; 4] {
        let size = [self.image.width as i32, self.image.height as i32];
        let p: [f32; 2] = std::array::from_fn(|k| {
            // MIRRORONCE reflects negative coordinates, then clamps to the edge.
            let coordinate = if self.address[k] == 5 {
                uv[k].abs()
            } else {
                uv[k]
            };
            coordinate * size[k] as f32 - if self.linear { 0.5 } else { 0.0 }
        });
        let texel = |di: i32, dj: i32| {
            let mut ij = [
                (p[0].floor() as i32).saturating_add(di),
                (p[1].floor() as i32).saturating_add(dj),
            ];
            for k in 0..2 {
                let n = size[k];
                ij[k] = match self.address[k] {
                    1 => ij[k].rem_euclid(n),
                    2 => {
                        let t = ij[k].rem_euclid(2 * n);
                        if t < n { t } else { 2 * n - 1 - t }
                    }
                    4 if !(0..n).contains(&ij[k]) => return self.border,
                    _ => ij[k].clamp(0, n - 1),
                };
            }
            let off = (ij[1] as usize * size[0] as usize + ij[0] as usize) * 4;
            std::array::from_fn(|k| f32::from(self.image.rgba[off + k]) / 255.0)
        };
        let a = texel(0, 0);
        if !self.linear {
            return a;
        }
        let (b, c, d) = (texel(1, 0), texel(0, 1), texel(1, 1));
        let (u, v) = (p[0] - p[0].floor(), p[1] - p[1].floor());
        std::array::from_fn(|k| {
            (a[k] * (1.0 - u) + b[k] * u) * (1.0 - v) + (c[k] * (1.0 - u) + d[k] * u) * v
        })
    }
}

pub(super) struct Mask {
    stages: [Option<Texture>; 4],
    def: PixelShaderDef,
    options: PixelOptions,
    uniforms: interp::Uniforms,
}

impl Mask {
    fn covers(&self, uv: [f32; 2]) -> bool {
        let inputs = interp::Inputs {
            t: std::array::from_fn(|i| {
                let [sx, sy, ox, oy] = self.stages[i]
                    .as_ref()
                    .map_or([1., 1., 0., 0.], |s| s.transform);
                [uv[0] * sx + ox, uv[1] * sy + oy, 0.0, 1.0]
            }),
            d0: [1.; 4],
            front: true,
            ..Default::default()
        };
        interp::run(&self.def, &self.options, &inputs, &self.uniforms, self).is_some()
    }
}

impl interp::Sampler for Mask {
    fn sample(&self, stage: usize, _dim: TextureDim, coord: [f32; 3]) -> [f32; 4] {
        self.stages[stage]
            .as_ref()
            .expect("validated 2D sampler")
            .sample([coord[0], coord[1]])
    }
}

pub(super) struct Materials {
    pub coverage: Vec<Coverage>,
    pub warnings: Vec<String>,
    seen: HashMap<(TagId, bool, bool), usize>,
    images: HashMap<(TagId, u32), Arc<Level>>,
    library: ShaderLibrary,
}

impl Materials {
    pub fn new<R: Read + Seek>(map: &mut Map<R>) -> Result<Self> {
        Ok(Self {
            coverage: Vec::new(),
            warnings: Vec::new(),
            seen: HashMap::new(),
            images: HashMap::new(),
            library: ShaderLibrary::read(map)?,
        })
    }

    pub fn index<R: Read + Seek>(
        &mut self,
        map: &mut Map<R>,
        files: &mut ResourceFiles,
        shader: TagId,
        object: bool,
        transparent: bool,
    ) -> usize {
        let key = (shader, object, transparent);
        if let Some(&idx) = self.seen.get(&key) {
            return idx;
        }
        let coverage = match self.resolve(map, files, shader, object, transparent) {
            Ok(c) => c,
            Err(e) => {
                let name = map
                    .tag(shader)
                    .ok()
                    .and_then(|t| t.name.as_deref())
                    .unwrap_or("unnamed");
                self.warnings.push(format!("Caster material {name} ({:#010x}): {e:#}. Its triangles were excluded from visibility.",shader.0));
                Coverage::Empty
            }
        };
        let idx = self.coverage.len();
        self.coverage.push(coverage);
        self.seen.insert(key, idx);
        idx
    }

    fn resolve<R: Read + Seek>(
        &mut self,
        map: &mut Map<R>,
        files: &mut ResourceFiles,
        shader: TagId,
        object: bool,
        transparent: bool,
    ) -> Result<Coverage> {
        // Drawn unassigned BSP parts still have a geometric silhouette.
        if shader == TagId::NULL {
            return Ok(if transparent {
                Coverage::Empty
            } else {
                Coverage::Solid
            });
        }
        let instance = self.library.shader(shader)?;
        let template = self.library.template(instance.template.id)?.compiled()?;
        template.validate()?;
        let first_lod = |layer| {
            (0..template.lods.len()).find(|&lod| {
                template
                    .layer_passes(lod, layer)
                    .is_some_and(|passes| !passes.is_empty())
            })
        };
        let (layer, lod, implementation) = if let Some(lod) = first_lod(10) {
            (10, lod, if object { 1 } else { 2 })
        } else if transparent {
            return Ok(Coverage::Empty);
        } else if let Some(lod) = first_lod(1) {
            // Missing optional shadow pass on a solid part: the base-depth
            // pass carries the material's cutout.
            (1, lod, 0)
        } else {
            return Ok(Coverage::Solid);
        };
        let selected = self.library.selected_implementation(
            shader,
            PassSelection {
                lod,
                layer,
                pass: 0,
                implementation,
            },
            None,
            None,
        )?;
        ensure!(
            selected.layer_passes == 1,
            "multiple depth passes in layer {layer}"
        );
        self.mask_for_pass(map, files, &selected.implementation)
    }

    fn mask_for_pass<R: Read + Seek>(
        &mut self,
        map: &mut Map<R>,
        files: &mut ResourceFiles,
        imp: &tags::material::ResolvedImplementation,
    ) -> Result<Coverage> {
        let mut state = RenderState::default();
        let table = RenderStateTable::for_build(HALO2_BUILD);
        for s in &imp.render_states {
            state
                .set_guest(&table, u32::from(s.state), s.value)
                .map_err(|e| anyhow::anyhow!("render state {e:?}"))?;
        }
        let alpha_test = (state.alpha_test_enable && state.alpha_func != CompareFunc::Always)
            .then_some(match state.alpha_func {
                CompareFunc::Always => AlphaFunc::Always,
                CompareFunc::Never => AlphaFunc::Never,
                CompareFunc::Greater => AlphaFunc::Greater,
                CompareFunc::GreaterEqual => AlphaFunc::GreaterEqual,
                CompareFunc::Equal => AlphaFunc::Equal,
                CompareFunc::NotEqual => AlphaFunc::NotEqual,
                CompareFunc::Less => AlphaFunc::Less,
                CompareFunc::LessEqual => AlphaFunc::LessEqual,
            });
        let alpha_kill =
            std::array::from_fn(|i| imp.stage_states.get(i).is_some_and(|s| s.alpha_kill != 0));
        if alpha_test.is_none() && !alpha_kill.contains(&true) {
            return Ok(Coverage::Solid);
        }
        if alpha_test == Some(AlphaFunc::Never) {
            return Ok(Coverage::Empty);
        }
        let options = PixelOptions {
            alpha_test,
            alpha_kill,
            ..Default::default()
        };
        let def = PixelShaderDef::from_bytes(&imp.pixel_shader)?
            .with_default_final_combiner(false, false);
        // ShaderLibrary resolves material and engine constants into this
        // definition, as native_render::program::translate_pass reads them.
        let (c0, c1) = def.embedded_constants();
        let uniforms = interp::Uniforms {
            c0,
            c1,
            alpha_ref: [f32::from(state.alpha_ref); 4],
            ..Default::default()
        };
        let mut stages = std::array::from_fn(|_| None);
        for (i, mode) in def.decode().texture_modes.iter().enumerate() {
            match mode {
                TexMode::None | TexMode::Passthru => continue,
                TexMode::Project2d => (),
                _ => bail!("alpha-test stage {i} has unsupported mode {mode:?}"),
            }
            let binding = imp.textures[i].context("unbound alpha-test texture")?;
            let TextureSource::Bitmap { bitmap, image, .. } = binding.source else {
                bail!("alpha-test stage {i} requires {:?}", binding.source);
            };
            let key = (bitmap, image);
            let level = if let Some(cached) = self.images.get(&key) {
                cached.clone()
            } else {
                let tags::TagData::Bitmap(b) = tags::DecodedTag::read(map, bitmap)?.data else {
                    bail!("alpha-test texture isn't a bitmap");
                };
                let frame = b
                    .images
                    .get(image as usize)
                    .context("alpha-test frame outside bitmap")?;
                ensure!(
                    frame.kind == tags::TextureKind::D2,
                    "alpha-test frame is not a 2D image"
                );
                ensure!(
                    !matches!(
                        frame.format,
                        tags::PixelFormat::P8 | tags::PixelFormat::P8Bump
                    ),
                    "alpha-test texture requires a runtime colour palette"
                );
                let level = assets::decode_image(frame, files)?
                    .levels
                    .into_iter()
                    .find(|l| l.face == 0 && l.level == 0 && l.depth == 1)
                    .context("no 2D bitmap level")?;
                let level = Arc::new(level);
                self.images.insert(key, level.clone());
                level
            };
            let mut transform = [1., 1., 0., 0.];
            if let Some(r) = imp
                .vertex_constants
                .iter()
                .find(|r| r.index as usize == 18 + i)
            {
                for (k, x) in transform.iter_mut().enumerate() {
                    if r.written & (1 << k) != 0 {
                        *x = r.value[k];
                    }
                }
            }
            let s = imp
                .stage_states
                .get(i)
                .context("missing alpha-test sampler state")?;
            ensure!(s.color_sign == 0, "signed alpha-test sampler");
            ensure!(
                [s.address_u, s.address_v]
                    .iter()
                    .all(|a| (1..=5).contains(a)),
                "unknown alpha-test addressing"
            );
            stages[i] = Some(Texture {
                image: level,
                transform,
                linear: s.mag_filter != 1,
                address: [s.address_u, s.address_v],
                border: shaders::psh::argb(s.border_color),
            });
        }
        Ok(Coverage::Mask(Box::new(Mask {
            stages,
            def,
            options,
            uniforms,
        })))
    }
}

#[cfg(test)]
pub(super) fn invented_cutout() -> Coverage {
    // A two-texel 2D texture, alpha kill on stage zero.
    let def = PixelShaderDef {
        texture_modes: 1,
        ..Default::default()
    };
    let options = PixelOptions {
        alpha_kill: [true, false, false, false],
        ..Default::default()
    };
    let image = Arc::new(Level {
        face: 0,
        level: 0,
        width: 2,
        height: 1,
        depth: 1,
        rgba: vec![255, 255, 255, 0, 255, 255, 255, 255],
    });
    let mut stages = std::array::from_fn(|_| None);
    stages[0] = Some(Texture {
        image,
        address: [1, 1],
        border: [0.; 4],
        linear: false,
        transform: [1., 1., 0., 0.],
    });
    Coverage::Mask(Box::new(Mask {
        stages,
        def,
        options,
        uniforms: Default::default(),
    }))
}
