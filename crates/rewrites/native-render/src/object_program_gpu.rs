//! Objects' lit parts on the GPU, drawn with the game's own vertex
//! programs and pixel shaders ([`crate::object_program`]): per section
//! its vertex streams expanded once through each declaration a program
//! needs ([`crate::object_streams`]), each program uploaded once, and per
//! view changed vertex and pixel constants staged in the submission upload ring,
//! then ordered packets appended to the D3D render pass already open on the
//! game's targets (docs/graphics/native-objects.md, "Object programs").

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gfx::external::ExternalDraws;
use gfx::compile::PendingPipeline;
use gfx::pipeline::{PipelineKey, ShaderId, ShaderProgram};
use gfx::{BoundGroup, RecordedDraw, ScissorRect};

use shaders::psh::PixelUniforms;
use shaders::vsh::VertexUniforms;
use wgpu::util::DeviceExt;

use crate::gpu::{Image, texture};
use crate::light_gpu::{StaticLightTexture, uniforms_layout};
use crate::look::Stage;
use crate::object_lighting::ShaderValues;
use crate::object_program::{
    IMPLEMENTATIONS, ObjectInputs, ObjectPass, Row, object_layer3, object_pass, object_pixel_uniforms, object_vertex_uniforms,
};
use crate::object_program::{LIGHTMAP_STREAM, object_layer_pass, object_layer_passes, texaccum_dimensions};
use crate::object_streams::{Attribute, Element, SLOT, StreamData, StreamElement, StreamFormats, declaration, gather, registers};
use crate::object_state::{self, DrawState, EntryStates};
use crate::objects::{ItemDraw, ItemKey, ObjectFrame};
use frame_scene::{AuxDepth, Defaults, FrameScene, Instance, LayerItem, LayerOwner, LayerPass, Lighting, PartDraw, PartFlags, Subparts};
use crate::program::{ViewInputs, bias_signed, frame_pixel, frame_vertex, sampler_descriptor};

/// One section's vertices through one declaration: per vertex, per
/// register in `registers` order, its guest bytes in a slot
/// ([`crate::object_streams::gather`]) of the guest format in `formats`,
/// which the vertex unit converts as the D3D path's fetch does
/// ([`vertex_fetch`]); and the section's index list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LitBufferInput {
    pub registers: Vec<u8>,
    pub formats: Vec<Attribute>,
    pub vertices: Vec<[u8; SLOT]>,
    pub indices: Vec<u32>,
}

/// One compiled implementation and the images of its sampled stages.
pub struct LitProgramInput {
    pub pass: ObjectPass,
    pub images: [Option<Image>; 4],
}

/// One lit part of a model: its mode section and its index among the
/// section's parts (what a render record's key names, `objects::ItemKey`),
/// the section's decompression, the material's shader values, the index
/// range in its buffers, and per implementation index the program and
/// buffer it draws with.
#[derive(Clone, Debug, PartialEq)]
pub struct LitPart {
    pub section: u16,
    pub part: u8,
    pub decompression: [Row; 3],
    /// Its material's shader tag datum (diagnostics).
    pub shader_tag: u32,
    pub shader: ShaderValues,
    pub indices: Range<u32>,
    pub implementations: [Option<(usize, usize)>; IMPLEMENTATIONS],
    /// Its subparts' index ranges in the same buffers, when the part has
    /// subparts a record can choose ([`LitSubparts`]).
    pub subparts: Option<SubpartRanges>,
    /// A strip part's indices decoded as a list, in the same buffers:
    /// what the emitter draws for a record without bit 9
    /// ([`LitSection::listed`]). `None` for a list part.
    pub listed: Option<Range<u32>>,
}

/// A lit part's subparts in its buffers: the section's index of the
/// first, and each subpart's index range, in order. Each range holds the
/// subpart's triangles decoded alone, as the emitter draws a chosen
/// subpart (`frame_scene::Subparts`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubpartRanges {
    pub first: u16,
    pub ranges: Vec<Range<u32>>,
    /// A strip part's subparts as a record without bit 9 draws a choice
    /// of them ([`ListedRanges`]); `None` for a list part.
    pub listed: Option<ListedRanges>,
}

/// A strip part's subparts in its buffers as the emitter's list draw
/// reads them (`0x0004db68`): `indices` the section's indices from the
/// subparts' lowest to their highest, verbatim, and per subpart its first
/// index and count within them ([`ListedSubparts`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedRanges {
    pub indices: Range<u32>,
    pub spans: Vec<(u32, u32)>,
}

impl SubpartRanges {
    fn chooses(&self, chosen: Subparts) -> Result<(), String> {
        if chosen.first != self.first || usize::from(chosen.count) != self.ranges.len() {
            return Err(format!(
                "the record chooses subparts {}+{}, the part's are {}+{}",
                chosen.first, chosen.count, self.first, self.ranges.len()
            ));
        }
        Ok(())
    }

    /// The index ranges a record without bit 9 draws for `chosen` on a
    /// strip part, as the emitter's list loop `0x0004db68` does: the
    /// chosen subparts in order, a run of index-contiguous ones (each
    /// starting where the last ended) one `TRIANGLELIST` draw of its
    /// indices verbatim, so its `n / 3` whole triangles; a run with none
    /// draws nothing. An error when `chosen` is not over these subparts or
    /// the part has no list of them.
    pub fn drawn_as_list(&self, chosen: Subparts) -> Result<Vec<Range<u32>>, String> {
        self.chooses(chosen)?;
        let listed = self.listed.as_ref().ok_or("the part's subparts have no list natively")?;
        if listed.spans.len() != self.ranges.len() {
            return Err("the part's listed subparts are not its subparts".into());
        }
        let base = listed.indices.start;
        let mut out: Vec<Range<u32>> = Vec::new();
        let mut flush = |(start, end): (u32, u32)| {
            let whole = (end - start) / 3 * 3;
            if whole > 0 {
                out.push(base + start..base + start + whole);
            }
        };
        let mut run: Option<(u32, u32)> = None;
        for n in chosen.drawn() {
            let (start, count) = listed.spans[usize::from(n)];
            run = match run {
                Some((first, end)) if start == end => Some((first, end + count)),
                Some(done) => {
                    flush(done);
                    Some((start, start + count))
                }
                None => Some((start, start + count)),
            };
        }
        if let Some(done) = run {
            flush(done);
        }
        if out.iter().any(|r| r.end > listed.indices.end) {
            return Err("the part's listed subparts lie outside their indices".into());
        }
        Ok(out)
    }

    /// The index ranges `chosen` draws, ascending, adjacent ones joined
    /// (one packet each), empty ones left out; an error when `chosen` is
    /// not over these subparts.
    pub fn drawn(&self, chosen: Subparts) -> Result<Vec<Range<u32>>, String> {
        self.chooses(chosen)?;
        let mut out: Vec<Range<u32>> = Vec::new();
        for n in chosen.drawn() {
            let r = self.ranges[usize::from(n)].clone();
            if r.is_empty() {
                continue;
            }
            match out.last_mut() {
                Some(last) if last.end == r.start => last.end = r.end,
                _ => out.push(r),
            }
        }
        Ok(out)
    }
}

/// The world layers besides 3 whose passes an object item draws natively
/// from the scene (docs/graphics/framescene.md, slice 4): 1, 5, 7, 8 and
/// 20, and from the sorted list's camouflage handler 18 and 19
/// (docs/graphics/native-water-fog.md). In layer 5 an item whose model has
/// a PRT block draws its pass's PRT variant ([`LitLayerPass::prt`]) with
/// the stream and constants the geometry draw adds
/// (`frame_scene::PrtStream`).
pub const OBJECT_LAYERS: [u8; 7] = [1, PRT_LAYER, 7, 8, 20, CAMO_LAYER, CAMO_MODULATE_LAYER];

/// Active camouflage, the layer the sorted handler `0x00041cc0` draws its
/// records' camouflage passes in (`scene_extract::layout::world_layers::sorted_site`).
pub const CAMO_LAYER: u8 = 18;

/// The camouflage's second layer, which the same handler draws after 18
/// into render target 3 while its modulate state holds.
pub const CAMO_MODULATE_LAYER: u8 = 19;

/// The layers whose passes a shader the game puts on a record in place of
/// its part's material draws ([`LitModelInput::append`] with
/// [`override_sections`]): the sorted handlers'.
pub const OVERRIDE_LAYERS: [u8; 2] = [CAMO_LAYER, CAMO_MODULATE_LAYER];

/// The world layer whose geometry draw adds an object's PRT stream
/// (`scene_extract::layout::prt`).
pub const PRT_LAYER: u8 = 5;

/// Most bytes a PRT draw's stream is read from guest memory.
const MAX_PRT_BYTES: usize = 16 << 20;

/// The PRT stream type the layer-5 variants compile for (10 bytes a
/// vertex, `0x00024490`): a draw whose stream is of another type draws
/// only when its format feeds the same registers the same way
/// ([`LitObjects::fill_prt`]).
pub const PRT_STREAM: u8 = scene_extract::layout::prt::SHORT_STREAM;

/// What a layer-5 PRT variant was compiled with: the vertex program's
/// input semantics and the section's stream formats (to build the
/// declaration again for the stream type a draw finds), the registers the
/// PRT stream feeds (ascending), every register supplied and their
/// fetches, and the highest constant from `c[68]` the program reads
/// directly.
#[derive(Clone, Debug, PartialEq)]
pub struct PrtInput {
    pub semantics: Vec<u16>,
    pub streams: Vec<Vec<StreamElement>>,
    pub registers: Vec<u8>,
    pub provided: u16,
    pub fetch: [shaders::vsh::InputFetch; 16],
    pub last_constant: Option<u8>,
}

/// A PRT draw's stream as the hook gathered it ([`LitObjects::fill_prt`]):
/// the registers of [`PrtInput::registers`], per vertex each in a
/// [`SLOT`], in `formats`.
#[derive(Clone, Debug, PartialEq)]
pub struct PrtVertices {
    pub formats: Vec<Attribute>,
    pub vertices: Arc<Vec<[u8; SLOT]>>,
}

/// The highest constant from `c[68]` below the section's decompression
/// (`c[170]` on, which the draw sets itself) that `program` reads
/// directly. Reads relative to `a0.x` (the node rows) are not bounded
/// here.
fn prt_constant_read(program: &shaders::vsh::Program) -> Option<u8> {
    let below = crate::program::TEXCOORD_DECOMPRESSION - 2;
    program
        .instructions
        .iter()
        .flat_map(|i| i.sources())
        .filter_map(|src| match src.reg {
            shaders::vsh::SrcReg::Const { index, relative: false }
                if index >= frame_scene::PRT_FIRST_CONSTANT && usize::from(index) < below =>
            {
                Some(index)
            }
            _ => None,
        })
        .max()
}

/// One pass of a lit part's world layer: how many implementations the
/// pass has (with one, every item draws it; otherwise the item's,
/// `crate::layer_pass::passes_of`), per implementation the program and
/// buffer it draws with, and those that draw nothing, having no vertex
/// program (`0x00016610`; bit `n`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LitLayerPass {
    pub count: usize,
    pub implementations: [Option<(usize, usize)>; IMPLEMENTATIONS],
    pub empty: u8,
    /// Layer 5: per implementation the PRT variant's program and buffer
    /// (`compile_layer_sections` with the PRT stream), which an item
    /// carrying a `frame_scene::PrtStream` draws.
    pub prt: [Option<(usize, usize)>; IMPLEMENTATIONS],
}

/// A lit part's passes of one world layer other than 3
/// ([`OBJECT_LAYERS`]): what [`LitPart`] holds for layer 3, per pass in
/// the order the layer draws them.
#[derive(Clone, Debug, PartialEq)]
pub struct LitLayerPart {
    pub layer: u8,
    pub section: u16,
    pub part: u8,
    pub decompression: [Row; 3],
    /// Its material's shader tag datum.
    pub shader_tag: u32,
    pub shader: ShaderValues,
    pub indices: Range<u32>,
    pub subparts: Option<SubpartRanges>,
    /// [`LitPart::listed`].
    pub listed: Option<Range<u32>>,
    pub passes: Vec<LitLayerPass>,
}

/// A model's lit parts, ready to upload: programs and buffers indexed by
/// [`LitPart::implementations`] and [`LitLayerPass::implementations`].
#[derive(Default)]
pub struct LitModelInput {
    pub programs: Vec<LitProgramInput>,
    pub buffers: Vec<LitBufferInput>,
    pub parts: Vec<LitPart>,
    /// The parts' passes of the world layers [`OBJECT_LAYERS`].
    pub layers: Vec<LitLayerPart>,
}

impl LitModelInput {
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty() && self.layers.is_empty()
    }

    fn valid_draw_references(&self) -> bool {
        let valid = |mut slots: &mut dyn Iterator<Item = (usize, usize)>,
                     indices: &Range<u32>,
                     subparts: Option<&SubpartRanges>,
                     listed: Option<&Range<u32>>| {
            // `&mut dyn Iterator` is itself a sized iterator; `all` needs one.
            Iterator::all(&mut slots, |(program, buffer)| {
                let Some(buffer) = self.buffers.get(buffer) else { return false };
                let fits = |range: &Range<u32>| range.start <= range.end && range.end as usize <= buffer.indices.len();
                program < self.programs.len() && fits(indices)
                    && subparts.is_none_or(|subparts| {
                        subparts.ranges.iter().all(fits) && subparts.listed.as_ref().is_none_or(|l| fits(&l.indices))
                    })
                    && listed.is_none_or(fits)
            })
        };
        self.parts.iter().all(|part| {
            valid(
                &mut part.implementations.iter().flatten().copied(),
                &part.indices,
                part.subparts.as_ref(),
                part.listed.as_ref(),
            )
        }) && self.layers.iter().all(|part| {
            valid(
                &mut part.passes.iter().flat_map(|p| p.implementations.iter().flatten().copied()),
                &part.indices,
                part.subparts.as_ref(),
                part.listed.as_ref(),
            )
        })
    }

    /// Whether a program samples the texture accumulation target
    /// ([`crate::object_program::TEXACCUM_TEXTURE`]): a pass drawing the
    /// model then needs the level bound ([`LitObjects::draws`]).
    pub fn samples_texaccum(&self) -> bool {
        self.programs.iter().any(|p| p.pass.texaccum.contains(&true))
    }

    /// Moves `other`'s world-layer parts (with the programs and buffers
    /// they draw with) into this model's, their program and buffer indices
    /// moved past this model's. `other`'s layer-3 parts are left out: a
    /// shader put on a record in place of its part's material
    /// ([`override_sections`]) is drawn only in the layers it names.
    pub fn append(&mut self, other: LitModelInput) {
        let programs = self.programs.len();
        let buffers = self.buffers.len();
        let moved = |slot: &mut Option<(usize, usize)>| {
            if let Some((p, b)) = slot {
                *p += programs;
                *b += buffers;
            }
        };
        let LitModelInput { programs: their_programs, buffers: their_buffers, layers, .. } = other;
        self.programs.extend(their_programs);
        self.buffers.extend(their_buffers);
        for mut part in layers {
            for pass in &mut part.passes {
                pass.implementations.iter_mut().for_each(moved);
                pass.prt.iter_mut().for_each(moved);
            }
            self.layers.push(part);
        }
    }
}

/// The model-local material index [`override_sections`] gives every part.
pub const OVERRIDE_MATERIAL: u16 = u16::MAX;

/// `sections` with every drawn part's material [`OVERRIDE_MATERIAL`]: the
/// input to [`lit_model`] for a shader the game may put on any record of
/// the model in place of its part's material. The record producer
/// `0x0002cbf0` takes that shader as an argument (from the object render
/// at `0x001341d0`), and when it is not -1 stores it in the record's slot
/// of the table at `0x004c1228` (index byte at `0x004c0ed6 + record`, -1
/// for none; at most `0xc0` a frame, `[0x004c1528]`). The item build
/// `0x00044c90` hands it to `0x00041040`, which draws the item with it
/// instead of the part's own: always for the hologram's handler (its
/// last argument 1), and for the camouflage handler's records with `+0`
/// bit 15. Built with the materials map `{OVERRIDE_MATERIAL: shader}`,
/// its layer-3 compile refuses (the shader has no layer 3), and its
/// [`OVERRIDE_LAYERS`] passes compile over every part; the result goes
/// into the model's with [`LitModelInput::append`].
pub fn override_sections<'a>(sections: &[LitSection<'a>]) -> Vec<LitSection<'a>> {
    sections
        .iter()
        .map(|s| LitSection {
            index: s.index,
            geometry: s.geometry,
            influences: s.influences,
            decompression: s.decompression,
            streams: s.streams.clone(),
            vertex_count: s.vertex_count,
            parts: s.parts.iter().map(|(k, _, t)| (*k, OVERRIDE_MATERIAL, t.clone())).collect(),
            subparts: s.subparts.clone(),
            listed: s.listed.clone(),
        })
        .collect()
}

struct LitBuffer {
    layout_id: usize,
    /// The section's vertices the buffer holds (a PRT stream gathers as
    /// many, [`LitObjects::fill_prt`]).
    vertex_count: usize,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    registers: Vec<u8>,
    formats: Vec<Attribute>,
}

type InheritedState = std::sync::Mutex<Option<(bool, EntryStates, DrawState)>>;

struct LitProgram {
    pass: ObjectPass,
    compiler: crate::pipelines::Pipelines,
    textures_layout: wgpu::BindGroupLayout,
    /// The stages' views the program owns, and every stage's sampler.
    views: Vec<(usize, wgpu::TextureView)>,
    samplers: Vec<wgpu::Sampler>,
    /// Its stages' bind group, made once; `None` when a stage samples the
    /// texture accumulation target or the view's copy, whose group is
    /// cached by the live views it binds (the level's, the copy's).
    textures: Option<wgpu::BindGroup>,
    accum_groups: Vec<(EncodeViews, Option<wgpu::BindGroup>)>,
    /// Per attachment formats, buffer layout and state: the pipeline and
    /// the key its ordered packets describe themselves by.
    /// Keyed also by the PRT stream's formats (empty without one).
    pipelines: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat, usize, DrawState, Vec<Attribute>), (PipelineKey, Arc<PendingPipeline>)>,
    inherited: InheritedState,
}

/// The PRT stream's vertex layout `draw` binds with `program`: `Some(None)`
/// for a draw without one, `None` for one not gathered yet or gathered
/// for other registers.
fn prt_layout(program: &LitProgram, draw: &PlannedDraw) -> Option<Option<(Vec<u8>, Vec<Attribute>)>> {
    match &draw.prt {
        None => Some(None),
        Some((_, None)) => None,
        Some((_, Some(v))) => {
            let input = program.pass.prt.as_ref()?;
            (input.registers.len() == v.formats.len()).then(|| Some((input.registers.clone(), v.formats.clone())))
        }
    }
}

/// The shader id the objects' packets carry (diagnostics only: their
/// pipelines are built here, never looked up by it).
const OBJECT_PROGRAM: ShaderId = ShaderId(u32::MAX);

/// The bind group of `views` (and `extra`, the texture accumulation
/// level's stages) with their samplers.
fn texture_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    views: &[(usize, wgpu::TextureView)],
    extra: &[(usize, &wgpu::TextureView)],
    samplers: &[wgpu::Sampler],
) -> wgpu::BindGroup {
    let mut entries = Vec::new();
    for (n, view) in views.iter().map(|(n, v)| (*n, v)).chain(extra.iter().copied()) {
        entries.push(wgpu::BindGroupEntry { binding: 2 * n as u32, resource: wgpu::BindingResource::TextureView(view) });
        entries.push(wgpu::BindGroupEntry { binding: 2 * n as u32 + 1, resource: wgpu::BindingResource::Sampler(&samplers[n]) });
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("native object program textures"), layout, entries: &entries })
}

/// The texture accumulation level an encode binds: its view and its size
/// in guest texels (the coordinates the programs give are texels of it).
#[derive(Clone, Copy)]
pub struct Texaccum<'a> {
    pub view: &'a wgpu::TextureView,
    pub guest_size: (u32, u32),
}

/// The views a program's per-encode group binds: the texture
/// accumulation level's and the view copy's, each when it samples one.
type EncodeViews = (Option<wgpu::TextureView>, Option<wgpu::TextureView>);

struct LitModel {
    programs: Vec<LitProgram>,
    buffers: Vec<LitBuffer>,
    parts: Vec<LitPart>,
    part_index: HashMap<(u16, u8), usize>,
    layers: Vec<LitLayerPart>,
    /// (layer, section, part, shader datum) to its index in `layers`: a
    /// part draws its own material's passes, and in the sorted handlers'
    /// layers those of a shader the game puts on its record instead
    /// ([`OVERRIDE_LAYERS`]).
    layer_index: HashMap<(u8, u16, u8, u32), usize>,
}

/// Every model's lit parts on the GPU.
pub struct LitObjects {
    uniforms_layout: wgpu::BindGroupLayout,
    models: HashMap<u32, LitModel>,
    /// The vector normalization cube, as the lights load it (engine
    /// texture 1).
    normalization: Option<wgpu::TextureView>,
    /// What the set's programs share ([`Shared`]).
    shared: Shared,
    /// The scratch copies of the bound colour the view stages sample, one
    /// per target copy (reference, display; [`LitObjects::view_copy`]).
    view_copies: [Option<wgpu::TextureView>; 2],
}

/// A copy of the bound colour for the stages that sample the view's own
/// target ([`LitObjects::view_copy`]): record `from` into `to` (`extent`)
/// before the pass that samples `view`.
pub struct ViewCopy {
    pub from: wgpu::Texture,
    pub to: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub extent: wgpu::Extent3d,
}

/// A stage image's identity within one set: the stage it is bound at,
/// what it samples (a bitmap's image, or `None` for the objects' lightmap
/// stage) and the channels stored biased. A set's uploads come from one
/// map load, which decodes equal keys to equal pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct StageImage {
    stage: usize,
    bitmap: Option<(map::TagId, u32)>,
    signed: [bool; 4],
}

impl StageImage {
    fn of(program: &crate::program::MaterialProgram, stage: usize) -> Option<Self> {
        let bitmap = match program.stages[stage] {
            Stage::Bitmap { bitmap, image } => Some((bitmap, image)),
            Stage::Lightmap => None,
            _ => return None,
        };
        Some(Self { stage, bitmap, signed: program.signed[stage] })
    }
}

/// How many GPU resources a set made for its programs. Equal programs
/// share theirs, so uploading a model whose programs and stage images the
/// set already holds leaves these unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Created {
    pub shader_modules: u64,
    pub layouts: u64,
    pub samplers: u64,
    pub textures: u64,
}

/// The GPU resources of one set's programs, made once per distinct input.
/// A map's object materials repeat a few generated shaders, stage layouts
/// and samplers across models and implementations, and their passes bind
/// the same bitmaps; the loader uploads every object definition in the tag
/// index, so one of each per program came to thousands of modules,
/// layouts, samplers and textures (docs/graphics/native-objects.md,
/// "Shared program resources").
#[derive(Default)]
struct Shared {
    layouts: HashMap<Vec<wgpu::BindGroupLayoutEntry>, wgpu::BindGroupLayout>,
    /// Per WGSL and stage layout: the module, its pipeline layout and its
    /// table of pipelines.
    shaders: HashMap<(String, Vec<wgpu::BindGroupLayoutEntry>), crate::pipelines::Pipelines>,
    /// Few distinct, and the descriptor does not hash.
    samplers: Vec<(wgpu::SamplerDescriptor<'static>, wgpu::Sampler)>,
    images: HashMap<StageImage, wgpu::TextureView>,
    created: Created,
}

impl Shared {
    fn layout(&mut self, device: &wgpu::Device, entries: &[wgpu::BindGroupLayoutEntry]) -> wgpu::BindGroupLayout {
        if let Some(layout) = self.layouts.get(entries) {
            return layout.clone();
        }
        self.created.layouts += 1;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native object program textures"),
            entries,
        });
        self.layouts.insert(entries.to_vec(), layout.clone());
        layout
    }

    fn sampler(&mut self, device: &wgpu::Device, descriptor: wgpu::SamplerDescriptor<'static>) -> wgpu::Sampler {
        if let Some((_, sampler)) = self.samplers.iter().find(|(d, _)| *d == descriptor) {
            return sampler.clone();
        }
        self.created.samplers += 1;
        let sampler = device.create_sampler(&descriptor);
        self.samplers.push((descriptor, sampler.clone()));
        sampler
    }

    /// The shader for `wgsl` over the set's uniforms and `textures`, or
    /// `None` when the device refuses it (not kept: nothing else differs
    /// on a retry).
    fn shader(
        &mut self,
        device: &wgpu::Device,
        wgsl: &str,
        uniforms: &wgpu::BindGroupLayout,
        textures: (&[wgpu::BindGroupLayoutEntry], &wgpu::BindGroupLayout),
    ) -> Option<crate::pipelines::Pipelines> {
        let key = (wgsl.to_owned(), textures.0.to_vec());
        if let Some(shader) = self.shaders.get(&key) {
            return Some(shader.clone());
        }
        self.created.shader_modules += 1;
        let module = gfx::program::validated_resource(device, || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native object program"),
                source: wgpu::ShaderSource::Wgsl(wgsl.into()),
            })
        })?;
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native object program"),
            bind_group_layouts: &[Some(uniforms), Some(textures.1)],
            immediate_size: 0,
        });
        let shader = ShaderProgram {
            module, layout, vs_entry: "vs_main".into(), fs_entry: "fs_main".into(), count_entry: None,
        };
        let compiler = crate::pipelines::Pipelines::new(device, wgsl, shader, "native object program");
        self.shaders.insert(key, compiler.clone());
        Some(compiler)
    }

    /// Stage `n`'s texture of `program` from `image`, or the one made
    /// from an equal stage before. A missing image gets a black texel of
    /// its own, never kept, so it cannot stand in for a later real one.
    fn image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        program: &crate::program::MaterialProgram,
        n: usize,
        image: Option<Image>,
    ) -> wgpu::TextureView {
        let key = StageImage::of(program, n);
        if let Some(view) = key.and_then(|k| self.images.get(&k)) {
            return view.clone();
        }
        let keep = image.is_some();
        let mut image = image.unwrap_or(Image { width: 1, height: 1, rgba: vec![0, 0, 0, 255], mips: Vec::new(), blocks: None });
        bias_signed(&mut image, program.signed[n]);
        self.created.textures += 1;
        let view = texture(device, queue, &image, "native object program stage");
        if keep && let Some(key) = key {
            self.images.insert(key, view.clone());
        }
        view
    }
}

/// One accepted item's draw, resolved at the hook ([`LitObjects::plan`]):
/// the part and implementation its record picked, the program and buffer
/// they draw with, the part's indices, its vertex and pixel blocks with
/// every per-object constant in (the view's put in at the encode), the
/// state resolved field by field, and whether it samples the texture
/// accumulation level.
#[derive(Clone, Debug)]
pub struct PlannedDraw {
    /// Salted handle of the object whose record supplied this draw.
    pub instance: frame_scene::InstanceKey,
    pub part: usize,
    pub implementation: usize,
    pub program: usize,
    pub buffer: usize,
    /// The part's whole index range.
    pub indices: Range<u32>,
    /// What the draw issues: the whole part, or the subparts its record
    /// chose ([`SubpartRanges::drawn`]; none when it chose none).
    pub ranges: Vec<Range<u32>>,
    /// A part whose record toggles the auxiliary state
    /// (`frame_scene::PartFlags::AUX_DEPTH`): the view's rows and clip it
    /// draws with in place of the device's ([`with_aux_depth`]).
    pub aux_depth: Option<AuxDepth>,
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    pub state: DrawState,
    pub texaccum: bool,
    /// Whether it samples the view's own target: drawn only with a copy
    /// of the bound colour ([`LitObjects::draws_with_view`]).
    pub view: bool,
    pub default_final: bool,
    /// A layer-5 PRT variant's draw: where its stream is in guest memory
    /// (`frame_scene::PrtStream`), and, once the hook has gathered it
    /// ([`LitObjects::fill_prt`]), its vertices.
    pub prt: Option<(frame_scene::PrtStream, Option<PrtVertices>)>,
}

impl LitBufferInput {
    /// Vertex `v`'s register `k` (in `registers` order) as the vertex unit
    /// reads it (the CPU's decode, for tests).
    pub fn decoded(&self, v: usize, k: usize) -> [f32; 4] {
        self.formats[k].decode(&self.vertices[v * self.registers.len() + k])
    }
}

/// The vertex format and shader fetch a guest attribute is read with, as
/// the D3D path reads it with its default (GPU) fetch
/// (`gfx::repack::AttributeFetch::raw`, `d3d::nv2a::input_fetch`): the
/// guest bytes as they are, converted by the vertex unit. Converted on
/// the CPU instead, normalised shorts can round another way than the
/// GPU's conversion, and a depth-equal pass (the third pass of 03a's
/// rebar, over the depth its D3D pass wrote) fails where w differs in the
/// last bit, as the world's packed normals did.
pub fn vertex_fetch(a: Attribute) -> (wgpu::VertexFormat, shaders::vsh::InputFetch) {
    use shaders::vsh::InputFetch as I;
    use wgpu::VertexFormat as F;
    match a {
        Attribute::Float(1) => (F::Float32, I::Float),
        Attribute::Float(2) => (F::Float32x2, I::Float),
        Attribute::Float(3) => (F::Float32x3, I::Float),
        Attribute::Float(_) => (F::Float32x4, I::Float),
        Attribute::PByte(1) => (F::Unorm8, I::Float),
        Attribute::PByte(2) => (F::Unorm8x2, I::Float),
        Attribute::PByte(3) => (F::Unorm8x4, I::Xyz1),
        Attribute::PByte(_) => (F::Unorm8x4, I::Float),
        Attribute::Short(1) => (F::Sint16, I::Int),
        Attribute::Short(2) => (F::Sint16x2, I::Int),
        Attribute::Short(3) => (F::Sint16x4, I::IntXyz1),
        Attribute::Short(_) => (F::Sint16x4, I::Int),
        Attribute::NormShort(1) => (F::Snorm16, I::Float),
        Attribute::NormShort(2) => (F::Snorm16x2, I::Float),
        Attribute::NormShort(3) => (F::Snorm16x4, I::Xyz1),
        Attribute::NormShort(_) => (F::Snorm16x4, I::Float),
        Attribute::NormPacked3 => (F::Uint32, I::NormPacked3),
        Attribute::Colour => (F::Unorm8x4Bgra, I::Float),
    }
}

/// The shader fetch of each register `declaration` supplies
/// ([`vertex_fetch`]); the rest are floats.
pub fn input_fetch(declaration: &[Element]) -> [shaders::vsh::InputFetch; 16] {
    let mut out = [shaders::vsh::InputFetch::Float; 16];
    for e in declaration {
        if let Some(f) = out.get_mut(usize::from(e.register)) {
            *f = vertex_fetch(e.format).1;
        }
    }
    out
}

/// The vertex attributes of a buffer holding `registers` of `formats`,
/// each in a [`SLOT`].
pub fn attributes(registers: &[u8], formats: &[Attribute]) -> Vec<wgpu::VertexAttribute> {
    registers
        .iter()
        .zip(formats)
        .enumerate()
        .map(|(k, (&r, &f))| wgpu::VertexAttribute {
            format: vertex_fetch(f).0,
            offset: (SLOT * k) as u64,
            shader_location: u32::from(r),
        })
        .collect()
}

impl LitObjects {
    /// An empty draw set whose uniform bindings are compatible with models
    /// uploaded here. The loader keeps its private normalization cube.
    pub fn empty_with_layout(&self) -> Self {
        Self {
            uniforms_layout: self.uniforms_layout.clone(),
            models: HashMap::new(),
            normalization: None,
            shared: Shared::default(),
            view_copies: [None, None],
        }
    }

    /// Publish a completed upload into the draw set without creating GPU
    /// resources under the draw set's lock.
    pub fn move_model_into(&mut self, index: u32, target: &mut Self) -> bool {
        if self.uniforms_layout != target.uniforms_layout {
            return false;
        }
        let Some(model) = self.models.remove(&index) else { return false };
        target.models.insert(index, model);
        true
    }

    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            uniforms_layout: uniforms_layout(device),
            models: HashMap::new(),
            normalization: None,
            shared: Shared::default(),
            view_copies: [None, None],
        }
    }

    /// The GPU resources this set made for its programs so far.
    pub fn created(&self) -> Created {
        self.shared.created
    }

    /// Whether `color` (`size` texels) can be copied for the view stages:
    /// a single-sampled 2D texture of that size that allows copies from it.
    pub fn can_copy_view(color: &wgpu::TextureView, size: (u32, u32)) -> bool {
        let texture = color.texture();
        texture.usage().contains(wgpu::TextureUsages::COPY_SRC)
            && texture.dimension() == wgpu::TextureDimension::D2
            && texture.sample_count() == 1
            && texture.depth_or_array_layers() == 1
            && (texture.width(), texture.height()) == size
    }

    /// The copy of the bound colour `color` the view stages of this target
    /// copy sample: its scratch texture, kept per target copy and made
    /// again when the colour's extent or format changes, and the copy to
    /// record before the pass draws into the target. An attachment cannot
    /// be sampled as it is drawn into, so the stages read the target as
    /// the draws before the pass left it.
    pub fn view_copy(&mut self, device: &wgpu::Device, color: &wgpu::TextureView, copy: gfx::external::TargetCopy) -> ViewCopy {
        let texture = color.texture();
        let extent = texture.size();
        let format = texture.format();
        let scratch = match copy {
            gfx::external::TargetCopy::Reference => &mut self.view_copies[0],
            gfx::external::TargetCopy::Display => &mut self.view_copies[1],
        };
        if scratch.as_ref().is_some_and(|view| {
            let t = view.texture();
            t.size() != extent || t.format() != format
        }) {
            *scratch = None;
        }
        let view = scratch.get_or_insert_with(|| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("native object view copy"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        });
        ViewCopy { from: texture.clone(), to: view.texture().clone(), view: view.clone(), extent }
    }

    /// Uploads the normalization cube the programs' normalization stages
    /// sample, once.
    pub fn set_normalization(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, image: &StaticLightTexture) {
        if self.normalization.is_none() {
            self.normalization = Some(image.view(device, queue));
        }
    }

    /// Publish a model only when its geometry and every material program
    /// upload successfully. Failure keeps the previous entry, if present.
    /// The result belongs to this upload, even when the key was already used.
    pub fn add_model(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, index: u32, input: LitModelInput) -> bool {
        if !input.valid_draw_references() {
            return false;
        }
        let mut layouts = HashMap::new();
        let buffers = input
            .buffers
            .iter()
            .map(|b| LitBuffer {
                layout_id: {
                    let next = layouts.len();
                    *layouts.entry((b.registers.clone(), b.formats.clone())).or_insert(next)
                },
                vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native object program vertices"),
                    contents: bytemuck::cast_slice(&b.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native object program indices"),
                    contents: bytemuck::cast_slice(&b.indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                vertex_count: if b.registers.is_empty() { 0 } else { b.vertices.len() / b.registers.len() },
                registers: b.registers.clone(),
                formats: b.formats.clone(),
            })
            .collect();
        let Some(programs) = input.programs.into_iter().map(|p| self.program(device, queue, p)).collect::<Option<Vec<_>>>() else {
            return false;
        };
        let part_index = index_parts(&input.parts);
        let mut layer_index = HashMap::with_capacity(input.layers.len());
        for (i, p) in input.layers.iter().enumerate() {
            layer_index.entry((p.layer, p.section, p.part, p.shader_tag)).or_insert(i);
        }
        self.models.insert(index, LitModel { programs, buffers, parts: input.parts, part_index, layers: input.layers, layer_index });
        true
    }

    fn program(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, input: LitProgramInput) -> Option<LitProgram> {
        let LitProgramInput { pass, mut images } = input;
        let p = &pass.program;
        let mut layout_entries = Vec::new();
        let mut views = Vec::new();
        let samplers: Vec<wgpu::Sampler> = (0..4).map(|n| self.shared.sampler(device, sampler_descriptor(p.samplers[n].as_ref()))).collect();
        for (n, d) in p.dims.iter().enumerate() {
            if d.is_none() {
                continue;
            }
            layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: if pass.normalization[n] {
                        wgpu::TextureViewDimension::Cube
                    } else {
                        wgpu::TextureViewDimension::D2
                    },
                    multisampled: false,
                },
                count: None,
            });
            layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32 + 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
            if pass.normalization[n] {
                // Not biased, as the lights sample it.
                views.push((n, self.normalization.clone()?));
                continue;
            }
            if pass.texaccum[n] || pass.view[n] {
                // The pass's level or the view's copy, bound per encode.
                continue;
            }
            // Checked at load: every sampled stage has its image.
            views.push((n, self.shared.image(device, queue, p, n, images[n].take())));
        }
        let textures_layout = self.shared.layout(device, &layout_entries);
        let compiler = self.shared.shader(device, &p.wgsl, &self.uniforms_layout, (&layout_entries, &textures_layout))?;
        let textures = if pass.texaccum.contains(&true) || pass.view.contains(&true) {
            None
        } else {
            Some(gfx::program::validated_resource(device, || {
                texture_group(device, &textures_layout, &views, &[], &samplers)
            })?)
        };
        Some(LitProgram {
            pass, compiler, textures_layout, views, samplers, textures,
            accum_groups: Vec::new(), pipelines: HashMap::new(), inherited: Default::default(),
        })
    }

    /// Each model's shader tag datums, over its layer-3 parts and its
    /// other layers' parts, ascending and each once, by model: what a live
    /// shader edit reloads (docs/editor/realtime-edits.md, "The native
    /// refresh").
    pub fn model_shaders(&self) -> Vec<(u32, Vec<u32>)> {
        let mut out: Vec<(u32, Vec<u32>)> = self
            .models
            .iter()
            .map(|(&index, m)| {
                let mut shaders: Vec<u32> =
                    m.parts.iter().map(|p| p.shader_tag).chain(m.layers.iter().map(|p| p.shader_tag)).collect();
                shaders.sort_unstable();
                shaders.dedup();
                (index, shaders)
            })
            .collect();
        out.sort_unstable_by_key(|(index, _)| *index);
        out
    }

    /// Drop model `index`: its parts keep their D3D draws. A live edit
    /// whose model no longer uploads takes it out rather than draw the
    /// old geometry (docs/editor/realtime-edits.md, "The native refresh").
    pub fn remove_model(&mut self, index: u32) -> bool {
        self.models.remove(&index).is_some()
    }

    /// Each stage texture made from image `image` of `bitmap`, once, with
    /// its stage and the channels stored biased there.
    fn image_textures(&self, bitmap: map::TagId, image: u32) -> Vec<(wgpu::Texture, usize, [bool; 4])> {
        let want = crate::look::Stage::Bitmap { bitmap, image };
        // `wgpu::Texture` hashes and compares by its id, not its interior state.
        #[allow(clippy::mutable_key_type)]
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        let mut models: Vec<_> = self.models.iter().collect();
        models.sort_by_key(|(k, _)| **k);
        for (_, m) in models {
            for program in &m.programs {
                let p = &program.pass.program;
                for (n, view) in &program.views {
                    if program.pass.normalization[*n] || p.stages.get(*n) != Some(&want) {
                        continue;
                    }
                    let t = view.texture().clone();
                    if seen.insert(t.clone()) {
                        out.push((t, *n, p.signed[*n]));
                    }
                }
            }
        }
        out
    }

    /// The stages whose programs sample image `image` of `bitmap`,
    /// ascending, each once: what [`LitObjects::rewrite_image`] needs
    /// decoded.
    pub fn image_stages(&self, bitmap: map::TagId, image: u32) -> Vec<usize> {
        let mut stages: Vec<usize> = self.image_textures(bitmap, image).into_iter().map(|(_, n, _)| n).collect();
        stages.sort_unstable();
        stages.dedup();
        stages
    }

    /// Rewrite every stage texture made from image `image` of `bitmap` in
    /// place with `decoded` (by stage), biased for its signed channels as
    /// at upload. Every texture is checked before the first write; a size,
    /// level count or format change refuses them all
    /// (docs/editor/realtime-edits.md, "The native refresh"). Returns how
    /// many textures were written.
    pub fn rewrite_image(
        &self,
        queue: &wgpu::Queue,
        bitmap: map::TagId,
        image: u32,
        decoded: &HashMap<usize, Image>,
    ) -> Result<usize, crate::gpu::ImageRefusal> {
        let mut writes = Vec::new();
        for (t, n, signed) in self.image_textures(bitmap, image) {
            let mut pixels = decoded.get(&n).ok_or(crate::gpu::ImageRefusal::NotDecoded)?.clone();
            bias_signed(&mut pixels, signed);
            crate::gpu::check_rewrite(&t, &pixels)?;
            writes.push((t, pixels));
        }
        for (t, pixels) in &writes {
            crate::gpu::rewrite_texture(queue, t, pixels)?;
        }
        Ok(writes.len())
    }

    /// Whether model `index` has lit parts.
    pub fn has(&self, index: u32) -> bool {
        self.models.contains_key(&index)
    }

    /// Whether the uploaded model owns the exact geometry and material
    /// the accepted item requires. Dynamic targets and pipelines are
    /// validated separately when recording this draw.
    pub fn has_draw_assets(&self, model: u32, draw: &PlannedDraw) -> bool {
        self.models.get(&model).is_some_and(|m| {
            m.buffers.get(draw.buffer).is_some() && m.programs.get(draw.program).is_some()
        })
    }

    /// The draw of the item `key` (the part and implementation its record
    /// picked) of `frame` in model `index`, its record's node rows `rows`,
    /// over the render state the device held at the layer-3 call
    /// (`entry`): every per-object constant put in and every state field
    /// resolved ([`crate::object_state::resolve`]), only the view's own
    /// left for the encode ([`Self::draws`]: the view's constants and
    /// clip, and the texture accumulation level's size). An error says why
    /// the item keeps its D3D draw.
    pub fn plan(&self, index: u32, frame: &ObjectFrame, key: &ItemKey, rows: &[Row], entry: &EntryStates) -> Result<PlannedDraw, String> {
        self.plan_with_scene_state(index, frame, key, rows, entry, false)
    }

    /// [`Self::plan`] for a first-person item: the topology and subparts
    /// its record draws ([`ItemDraw::flags`], [`ItemDraw::subparts`]), as
    /// the scene's parts draw theirs ([`plan_item_part`]).
    pub fn plan_item(&self, index: u32, frame: &ObjectFrame, item: &ItemDraw, entry: &EntryStates) -> Result<PlannedDraw, String> {
        self.plan_inputs(index, item_inputs(frame, item)?, &item.key, &item.rows, entry, false)
    }

    pub fn plan_scene(&self, index: u32, frame: &ObjectFrame, key: &ItemKey, rows: &[Row], entry: &EntryStates) -> Result<PlannedDraw, String> {
        self.plan_with_scene_state(index, frame, key, rows, entry, true)
    }

    /// [`Self::plan_scene`] for a part of a view of the scene: the
    /// instance's lighting and defaults, the part's key and its rows, all
    /// as extraction copied them.
    pub fn plan_part_of(
        &self, index: u32, scene: &FrameScene, instance: &Instance, part: &PartDraw, entry: &EntryStates,
    ) -> Result<PlannedDraw, String> {
        let inputs = PartInputs {
            instance: instance.key,
            lighting: instance.lighting.as_ref(),
            defaults: instance.defaults,
            subparts: part.subparts,
            strip: Some(part.flags.contains(PartFlags::STRIP)),
        };
        self.plan_inputs(index, inputs, &part.key, scene.rows_of(part), entry, true)
    }

    fn plan_with_scene_state(
        &self, index: u32, frame: &ObjectFrame, key: &ItemKey, rows: &[Row], entry: &EntryStates,
        scene: bool,
    ) -> Result<PlannedDraw, String> {
        self.plan_inputs(index, PartInputs::of(frame), key, rows, entry, scene)
    }

    fn plan_inputs(
        &self, index: u32, inputs: PartInputs<'_>, key: &ItemKey, rows: &[Row], entry: &EntryStates, scene: bool,
    ) -> Result<PlannedDraw, String> {
        let m = self.models.get(&index).ok_or("its model is not uploaded")?;
        let part = m.part_index.get(&(key.section, key.part)).copied();
        plan_indexed_part(&m.parts, part, &|program| m.programs.get(program).map(|p| (&p.pass, Some(&p.inherited))), inputs, key, rows, entry, scene)
    }

    /// The draws of an object item of world layer `layer` (one of
    /// [`OBJECT_LAYERS`]; `pass` its layer draw in `scene`, `item` the
    /// item) in model `index`: per pass its material draws in the layer,
    /// in order, the implementation the pass draws for the item (its only
    /// one, or the item's), every per-object constant put in from the
    /// item's lighting, the draw's defaults and the item's rows, and its
    /// state resolved over the device at the layer call (`entry`,
    /// [`object_state::resolve_layer`]); a marked item's in stencil mode 2
    /// unless the layer draws with the stencil off. A pass drawing nothing
    /// for the item is left out. An error says why the item keeps its D3D
    /// draw: extraction's cause ([`LayerItem::refusal`]), the auxiliary
    /// state toggle (its values are read at the view's layer 3, not here),
    /// a part or implementation that is not native, or an input or state
    /// that does not resolve.
    pub fn plan_layer_item(
        &self, index: u32, layer: u8, scene: &FrameScene, pass: &LayerPass, item: &LayerItem, entry: &EntryStates,
    ) -> Result<Vec<PlannedDraw>, String> {
        let LayerOwner::Object(instance) = item.owner else { return Err("not an object's item".into()) };
        if let Some(why) = item.refusal {
            return Err(why.name().into());
        }
        let m = self.models.get(&index).ok_or("its model is not uploaded")?;
        let rows = scene.rows.get(item.rows.start as usize..item.rows.end as usize).ok_or("the item's rows are not in the scene")?;
        let at = m.layer_index.get(&(layer, item.key.section, item.key.part, item.key.shader)).copied();
        if at.is_none() && m.layers.iter().any(|p| p.layer == layer && p.section == item.key.section && p.part == item.key.part) {
            return Err(format!(
                "the item names shader {:#010x}, which section {} part {} has no native passes of in layer {layer}",
                item.key.shader, item.key.section, item.key.part
            ));
        }
        plan_layer_part(&m.layers, at, &|program| m.programs.get(program).map(|p| &p.pass), instance, layer, rows, pass, item, entry)
    }

    /// Gathers a layer-5 PRT draw's stream (planned with the item's
    /// `frame_scene::PrtStream`, [`plan_layer_part`]) from guest memory at
    /// the hook: the section's vertices from the stream's address, its
    /// stride apart (`read` the guest's bytes), through the declaration
    /// the stream setup builds for the stream's type (`formats`, read
    /// from guest memory). That declaration must supply the registers the
    /// PRT variant was compiled for, fetched the same way: a stream type
    /// whose format differs refuses. A draw without a stream is left as it
    /// is.
    pub fn fill_prt(
        &self, index: u32, draw: &mut PlannedDraw, formats: &StreamFormats, read: &dyn Fn(u32, usize) -> Option<Vec<u8>>,
    ) -> Result<(), String> {
        let Some((stream, gathered)) = &mut draw.prt else { return Ok(()) };
        let model = self.models.get(&index).ok_or("its model is not uploaded")?;
        let input = model.programs.get(draw.program).and_then(|p| p.pass.prt.as_ref()).ok_or("its program has no PRT stream")?;
        let count = model.buffers.get(draw.buffer).ok_or("its buffer is missing")?.vertex_count;
        let mut all = input.streams.clone();
        all.push(formats.elements(stream.kind)?);
        let full = declaration(&input.semantics, &all);
        if registers(&full) != input.provided || input_fetch(&full) != input.fetch {
            return Err(format!("PRT stream type {:#x} feeds other registers than type {PRT_STREAM:#x}'s", stream.kind));
        }
        let prt: Vec<Element> = full
            .into_iter()
            .filter(|e| e.stream == input.streams.len())
            .map(|e| Element { stream: 0, ..e })
            .collect();
        if prt.iter().map(|e| e.register).ne(input.registers.iter().copied()) {
            return Err(format!("PRT stream type {:#x} feeds other registers than type {PRT_STREAM:#x}'s", stream.kind));
        }
        let stride = usize::from(stream.stride);
        let len = count.checked_mul(stride).filter(|&n| n <= MAX_PRT_BYTES).ok_or("its PRT stream is too large")?;
        let bytes = read(stream.data, len).filter(|b| b.len() == len).ok_or("its PRT stream does not read")?;
        let vertices = gather(&prt, &[Some(StreamData { bytes: &bytes, stride })], count)?;
        *gathered = Some(PrtVertices { formats: prt.iter().map(|e| e.format).collect(), vertices: Arc::new(vertices) });
        Ok(())
    }

    pub fn planned_pass(&self, index: u32, draw: &PlannedDraw) -> Option<&ObjectPass> {
        Some(&self.models.get(&index)?.programs.get(draw.program)?.pass)
    }

    /// The blocks `d` (of model `index`) is issued with in `view`, and the
    /// texture accumulation level's size when it samples one: what
    /// [`Self::draws`] uploads. `None` when the model is gone or `d`
    /// samples the level and there is none.
    fn issued(
        &self, index: u32, d: &PlannedDraw, view: &ViewInputs, projection_factor: f32, texaccum: Option<Texaccum<'_>>,
        view_copy: Option<Texaccum<'_>>,
    ) -> Option<(VertexUniforms, PixelUniforms)> {
        let pass = &self.models.get(&index)?.programs.get(d.program)?.pass;
        if (d.texaccum && texaccum.is_none()) || (d.view && view_copy.is_none()) {
            return None;
        }
        let mut vertex = frame_vertex(&d.vertex, view);
        vertex.w_clamp = d.state.w_clamp;
        let mut pixel = frame_pixel(&d.pixel, view);
        if let Some(aux) = &d.aux_depth {
            with_aux_depth(&mut vertex, &mut pixel, aux);
            // This per-part matrix replaced the view upload above.
            d3d::device::widen_projection_rows(&mut vertex.constants[..4], projection_factor);
        }
        if let Some(t) = texaccum.filter(|_| d.texaccum) {
            texaccum_dimensions(&mut vertex, &pass.implementation, pass.texaccum, t.guest_size);
            texaccum_scale(&mut pixel, pass.texaccum, t.guest_size);
        }
        // The view's target is a linear render target as the level is:
        // its size where the resolver left 0 by 0, its coordinates texels.
        if let Some(t) = view_copy.filter(|_| d.view) {
            texaccum_dimensions(&mut vertex, &pass.implementation, pass.view, t.guest_size);
            texaccum_scale(&mut pixel, pass.view, t.guest_size);
        }
        Some((vertex, pixel))
    }

    /// The sections model `index`'s lit parts come from, each once
    /// (diagnostics).
    pub fn sections(&self, index: u32) -> Vec<u16> {
        let mut s: Vec<u16> = self.models.get(&index).map(|m| m.parts.iter().map(|p| p.section).collect()).unwrap_or_default();
        s.sort_unstable();
        s.dedup();
        s
    }

    /// Request each distinct variant before the guest surrenders a run. A
    /// change to the inherited state, target or vertex layout uses a new key.
    pub fn prewarm_draws(
        &mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat,
        draws: &[(u32, PlannedDraw)],
    ) -> bool {
        let mut ready = !draws.is_empty();
        for (model, draw) in draws {
            ready &= self.request_draw(device, color, depth, *model, draw)
                .is_some_and(|(pending, _)| pending.previous_frame().is_some_and(Option::is_some));
        }
        ready
    }

    fn request_draw(
        &mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat,
        model: u32, draw: &PlannedDraw,
    ) -> Option<(&Arc<PendingPipeline>, bool)> {
        let model = self.models.get_mut(&model)?;
        let buffer = model.buffers.get(draw.buffer)?;
        let program = model.programs.get_mut(draw.program)?;
        let prt = prt_layout(program, draw)?;
        let key = (color, depth, buffer.layout_id, draw.state, prt.as_ref().map(|(_, f)| f.clone()).unwrap_or_default());
        let (entry, resident) = match program.pipelines.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => (entry.into_mut(), true),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut vertex_buffers = vec![gfx::vertex::OwnedVertexBufferLayout {
                    stride: (SLOT * buffer.registers.len()) as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: attributes(&buffer.registers, &buffer.formats),
                }];
                if let Some((registers, formats)) = prt.filter(|(r, _)| !r.is_empty()) {
                    vertex_buffers.push(gfx::vertex::OwnedVertexBufferLayout {
                        stride: (SLOT * registers.len()) as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: attributes(&registers, &formats),
                    });
                }
                let descriptor = PipelineKey {
                    shader: OBJECT_PROGRAM,
                    vertex_buffers,
                    primitive: draw.state.primitive,
                    depth_stencil: Some(draw_depth_stencil(&draw.state, depth)?),
                    color_target: wgpu::ColorTargetState {
                        format: color, blend: draw.state.blend, write_mask: draw.state.color_writes,
                    },
                    samples: 1,
                };
                let pending = program.compiler.request(device, &descriptor);
                (entry.insert((descriptor, pending)), false)
            }
        };
        Some((&entry.1, resident))
    }

    /// Ordered packets for `draws` (each with its model index, planned at
    /// the hook, [`Self::plan`]) on attachments of `color_format` and
    /// `depth_format`, `target_size` texels, the viewport the whole target
    /// as the programs expect. The backend appends them to the render pass
    /// already open on those attachments, as the D3D draws they replace
    /// would have been: no render pass, command buffer or bundle of their
    /// own. Pipelines and texture groups persist across frames; the only
    /// per-frame work is the constants' changed rows in the submission's
    /// resident uniform run. A draw sampling the texture accumulation
    /// target takes `texaccum`, its stages' coordinates in that level's
    /// guest texels. Refuses the whole run (the caller keeps every
    /// original) when any draw cannot be issued exactly.
    #[allow(clippy::too_many_arguments)]
    pub fn draws(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &gfx::ring::SubmissionUploads,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        view: &ViewInputs,
        projection_factor: f32,
        draws: &[(u32, PlannedDraw)],
        texaccum: Option<Texaccum<'_>>,
    ) -> Result<ExternalDraws, &'static str> {
        self.draws_with_view(
            device, queue, uploads, color_format, depth_format, target_size, view, projection_factor, draws, texaccum, None,
        )
    }

    /// [`Self::draws`] with `view_copy`, the copy of the bound colour a
    /// draw sampling the view's own target binds ([`Self::view_copy`]),
    /// its size the target's in guest pixels (the programs address it in
    /// texels, as the level). Such a draw is refused without one.
    #[allow(clippy::too_many_arguments)]
    pub fn draws_with_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &gfx::ring::SubmissionUploads,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        view: &ViewInputs,
        projection_factor: f32,
        draws: &[(u32, PlannedDraw)],
        texaccum: Option<Texaccum<'_>>,
        view_copy: Option<Texaccum<'_>>,
    ) -> Result<ExternalDraws, &'static str> {
        if draws.is_empty() {
            return Err("no object draws");
        }
        let mut issued: Vec<(&(u32, PlannedDraw), (VertexUniforms, PixelUniforms))> = Vec::with_capacity(draws.len());
        for d in draws {
            let blocks = self.issued(d.0, &d.1, view, projection_factor, texaccum, view_copy).ok_or("object draw inputs unavailable")?;
            issued.push((d, blocks));
        }
        // Whether each draw found its pipeline and texture group resident.
        let mut resident = Vec::with_capacity(issued.len());
        for ((model, draw), _) in &issued {
            let (pending, hit) = self.request_draw(device, color_format, depth_format, *model, draw)
                .ok_or("object pipeline unavailable")?;
            if pending.try_get().and_then(Option::as_ref).is_none() {
                return Err("object pipeline pending or invalid");
            }
            resident.push(hit);
        }
        let uniforms = uploads.resident_uniform_run(
            device, queue, &self.uniforms_layout, [VertexUniforms::SIZE, PixelUniforms::SIZE],
            issued.len(), |i, v, p| {
                let (_, (vertex, pixel)) = &issued[i];
                vertex.write_bytes(v);
                pixel.write_bytes(p);
            },
        ).ok_or("object constants unavailable")?;
        let scissor = ScissorRect::full(target_size.0, target_size.1);
        let mut packets = Vec::with_capacity(issued.len());
        let mut discards = false;
        for (i, (planned, _)) in issued.iter().enumerate() {
            let (model, d) = (planned.0, &planned.1);
            let m = self.models.get_mut(&model).ok_or("object model unavailable")?;
            let b = &m.buffers[d.buffer];
            if d.ranges.iter().any(|r| r.is_empty() || u64::from(r.end) > b.indices.size() / 4) || b.vertices.size() == 0 {
                return Err("object index range unavailable");
            }
            let p = &mut m.programs[d.program];
            let prt_formats = d.prt.as_ref().and_then(|(_, v)| v.as_ref()).map(|v| v.formats.clone()).unwrap_or_default();
            let (described, pipeline) = p.pipelines.get(&(color_format, depth_format, b.layout_id, d.state, prt_formats))
                .cloned().ok_or("object pipeline unavailable")?;
            let textures = match &p.textures {
                Some(textures) => textures.clone(),
                None => {
                    let accum = if p.pass.texaccum.contains(&true) {
                        Some(texaccum.as_ref().ok_or("object texture accumulation unavailable")?.view)
                    } else {
                        None
                    };
                    let copy = if p.pass.view.contains(&true) {
                        Some(view_copy.as_ref().ok_or("object view copy unavailable")?.view)
                    } else {
                        None
                    };
                    let key: EncodeViews = (accum.cloned(), copy.cloned());
                    let slot = if let Some(at) = p.accum_groups.iter().position(|(views, _)| *views == key) {
                        at
                    } else {
                        resident[i] = false;
                        let mut extra: Vec<(usize, &wgpu::TextureView)> = Vec::new();
                        for n in 0..4 {
                            if let (true, Some(v)) = (p.pass.texaccum[n], accum) {
                                extra.push((n, v));
                            } else if let (true, Some(v)) = (p.pass.view[n], copy) {
                                extra.push((n, v));
                            }
                        }
                        let group = gfx::program::validated_resource(device, || {
                            texture_group(device, &p.textures_layout, &p.views, &extra, &p.samplers)
                        });
                        // Keep both target copies hot. A resized target evicts the oldest view.
                        if p.accum_groups.len() == 2 { p.accum_groups.remove(0); }
                        p.accum_groups.push((key, group));
                        p.accum_groups.len() - 1
                    };
                    p.accum_groups[slot].1.clone().ok_or("object texture group invalid")?
                }
            };
            gfx::churn::native_cache(resident[i]);
            discards |= d.state.alpha_test.is_some();
            let groups = vec![
                BoundGroup { index: 0, group: uniforms.group.clone(), offsets: uniforms.offsets(i).to_vec() },
                BoundGroup::new(1, textures),
            ];
            // The stencil reference is zero (`object_state::resolve_scene`)
            // but for a marked part's mode 2 (`object_state::mark`); the
            // packet sets it.
            let state = gfx::state::RenderState { stencil: d.state.stencil, ..Default::default() };
            // A PRT draw's stream, gathered at the hook, in this
            // submission's uploads.
            let mut vertex_buffers = vec![(b.vertices.clone(), 0, b.vertices.size())];
            match &d.prt {
                Some((_, None)) => return Err("object PRT stream not gathered"),
                Some((_, Some(v))) if !v.vertices.is_empty() => {
                    let bytes: &[u8] = bytemuck::cast_slice(v.vertices.as_slice());
                    let (buffer, at) = uploads.stage(device, queue, bytes);
                    vertex_buffers.push((buffer, at, bytes.len() as u64));
                }
                None | Some((_, Some(_))) => {}
            }
            // One packet per range: the whole part, or each run of the
            // subparts its record chose (none when it chose none).
            for r in &d.ranges {
                let count = r.end - r.start;
                packets.push(RecordedDraw::prepared_indexed(
                    &described,
                    pipeline.clone(),
                    vertex_buffers.clone(),
                    (b.indices.clone(), u64::from(r.start) * 4, u64::from(count) * 4),
                    count,
                    groups.clone(),
                    &state,
                    target_size,
                    scissor,
                ).with_blend_constant(d.state.blend_color()));
            }
        }
        // `ranges` counts the planned draws (`d3d::ExternalPlan::ranges`),
        // however many packets their subparts take.
        Ok(ExternalDraws { ranges: issued.len(), packets, discards, writes: Vec::new() })
    }
}

/// `vertex` and `pixel` as the auxiliary state toggle leaves the device
/// for a part's draw (`0x00012d50` on): the view-projection rows `c[0..3]`
/// and the program clip range it sets, the clamp the device's.
pub fn with_aux_depth(vertex: &mut VertexUniforms, pixel: &mut PixelUniforms, aux: &AuxDepth) {
    vertex.constants[..4].copy_from_slice(&aux.rows);
    vertex.w_clip = Some(aux.clip);
    pixel.depth_range = aux.clip;
}

/// The depth and stencil a lit part's pipeline draws with on a
/// `depth_format` attachment; `None` when it tests the stencil and the
/// attachment has none.
fn draw_depth_stencil(state: &DrawState, depth_format: wgpu::TextureFormat) -> Option<wgpu::DepthStencilState> {
    if state.stencil.enable && !depth_format.has_stencil_aspect() {
        return None;
    }
    let stencil = gfx::translate::depth_stencil_state(
        &gfx::state::RenderState { stencil: state.stencil, ..Default::default() },
        Some(depth_format),
        wgpu::PrimitiveTopology::TriangleList,
    )?.stencil;
    Some(wgpu::DepthStencilState {
        format: depth_format,
        depth_write_enabled: Some(state.depth_write),
        depth_compare: Some(state.depth_compare),
        stencil,
        bias: wgpu::DepthBiasState::default(),
    })
}

/// [`LitObjects::plan`] over a model's `parts` and its programs' passes
/// (`pass_of`): exactly the part the item's key names (section and part),
/// with exactly the implementation its record picked, its item's shader
/// the part's; then its per-object blocks and its state.
pub fn plan_part<'a>(
    parts: &[LitPart],
    pass_of: &dyn Fn(usize) -> Option<&'a ObjectPass>,
    frame: &ObjectFrame,
    key: &ItemKey,
    rows: &[Row],
    entry: &EntryStates,
) -> Result<PlannedDraw, String> {
    let part = parts.iter().position(|p| p.section == key.section && p.part == key.part);
    plan_indexed_part(parts, part, &|i| pass_of(i).map(|p| (p, None)), PartInputs::of(frame), key, rows, entry, false)
}

/// [`LitObjects::plan_item`] over a model's `parts` and its programs'
/// passes (`pass_of`).
pub fn plan_item_part<'a>(
    parts: &[LitPart],
    pass_of: &dyn Fn(usize) -> Option<&'a ObjectPass>,
    frame: &ObjectFrame,
    item: &ItemDraw,
    entry: &EntryStates,
) -> Result<PlannedDraw, String> {
    let key = &item.key;
    let part = parts.iter().position(|p| p.section == key.section && p.part == key.part);
    let inputs = item_inputs(frame, item)?;
    plan_indexed_part(parts, part, &|i| pass_of(i).map(|p| (p, None)), inputs, key, &item.rows, entry, false)
}

/// A first-person item's inputs: its frame's, and the topology and
/// subparts its record draws. A record marking the stencil or toggling
/// the auxiliary state refuses: the first-person draw keeps the device's
/// stencil and reads no auxiliary depth.
fn item_inputs<'a>(frame: &'a ObjectFrame, item: &ItemDraw) -> Result<PartInputs<'a>, String> {
    if item.flags.contains(PartFlags::STENCIL_MARK) {
        return Err("the record marks the stencil, which the first-person draw does not".into());
    }
    if item.flags.contains(PartFlags::AUX_DEPTH) {
        return Err("the auxiliary state toggle, which the first-person draw does not read".into());
    }
    Ok(PartInputs { subparts: item.subparts, strip: Some(item.flags.contains(PartFlags::STRIP)), ..PartInputs::of(frame) })
}

/// [`LitObjects::plan_layer_item`] over a model's world-layer parts
/// (`layers`, `at` the index of the item's in layer `layer`) and its
/// programs' passes (`pass_of`), the item's rows `rows`: per pass of the
/// part, the implementation it draws for the item, its blocks and its
/// state. In layer 1 with the stencil off the draws take stencil mode 0,
/// as the prologue sets it (`0x0004d640`); layers 5, 7, 8 and 20 draw
/// under the device's stencil. An item with a PRT stream (layer 5) draws
/// each pass's PRT variant ([`LitLayerPass::prt`]) with the stream's
/// constants from `c[68]` on, written after the item's own (the draw sets
/// them after its rows, `0x0004d269`), and refuses a program reading a
/// constant past them; its draws still need their stream gathered
/// ([`LitObjects::fill_prt`]).
#[allow(clippy::too_many_arguments)]
pub fn plan_layer_part<'a>(
    layers: &[LitLayerPart], at: Option<usize>,
    pass_of: &dyn Fn(usize) -> Option<&'a ObjectPass>,
    instance: frame_scene::InstanceKey, layer: u8, rows: &[Row], pass: &LayerPass, item: &LayerItem, entry: &EntryStates,
) -> Result<Vec<PlannedDraw>, String> {
    if let Some(why) = item.refusal {
        return Err(why.name().into());
    }
    if item.prt.is_some() && layer != PRT_LAYER {
        return Err(format!("a PRT stream in layer {layer}"));
    }
    if item.flags.contains(PartFlags::AUX_DEPTH) {
        return Err("the auxiliary state toggle outside layer 3".into());
    }
    let key = &item.key;
    let at = at.ok_or_else(|| format!("section {} part {} draws nothing native in layer {layer}", key.section, key.part))?;
    let p = layers.get(at).ok_or("its layer part is missing")?;
    if p.layer != layer || p.section != key.section || p.part != key.part {
        return Err(format!("section {} part {} draws nothing native in layer {layer}", key.section, key.part));
    }
    if p.shader_tag != key.shader {
        return Err(format!("the item names shader {:#010x}, the part's is {:#010x}", key.shader, p.shader_tag));
    }
    let strip = Some(item.flags.contains(PartFlags::STRIP));
    let ranges = part_ranges(&p.indices, p.subparts.as_ref(), p.listed.as_ref(), strip, item.subparts)?;
    let defaults = pass.inputs.defaults.ok_or("the inputs' defaults do not read")?;
    let inputs = ObjectInputs { lighting: item.lighting.as_ref(), defaults: &defaults, shader: p.shader };
    let unstenciled;
    let entry = if layer == 1 && pass.stencil_off {
        let mut e = entry.clone();
        e.0.retain(|(r, _)| *r != gfx::state::RenderStateType::StencilEnable);
        e.0.push((gfx::state::RenderStateType::StencilEnable, 0));
        unstenciled = e;
        &unstenciled
    } else {
        entry
    };
    let mut out = Vec::with_capacity(p.passes.len());
    for (n, lp) in p.passes.iter().enumerate() {
        let implementation = if lp.count == 1 { 0 } else { key.implementation };
        if implementation < IMPLEMENTATIONS && lp.empty >> implementation & 1 != 0 {
            continue;
        }
        let slots = if item.prt.is_some() { &lp.prt } else { &lp.implementations };
        let (program, buffer) = slots
            .get(implementation)
            .copied()
            .flatten()
            .ok_or_else(|| match item.prt {
                Some(_) => format!("layer {layer} pass {n} has no native implementation {implementation} with the PRT stream"),
                None => format!("layer {layer} pass {n} has no native implementation {implementation}"),
            })?;
        let compiled = pass_of(program).ok_or("its program is missing")?;
        let mut vertex = object_vertex_uniforms(&compiled.program.vertex, &compiled.implementation, &inputs, rows, p.decompression)?;
        if let Some(prt) = &item.prt {
            let input = compiled.prt.as_ref().ok_or("its PRT variant has no PRT stream")?;
            let first = usize::from(frame_scene::PRT_FIRST_CONSTANT);
            if let Some(c) = input.last_constant.filter(|&c| usize::from(c) >= first + prt.constants.len()) {
                return Err(format!("its program reads c[{c}], past the {} constants the PRT draw sets", prt.constants.len()));
            }
            let end = (first + prt.constants.len()).min(vertex.constants.len());
            vertex.constants[first..end].copy_from_slice(&prt.constants[..end - first]);
        }
        let pixel = object_pixel_uniforms(&compiled.program.pixel, &compiled.implementation, &inputs)?;
        let alpha_test = compiled.program.state.as_ref().ok_or("its pass has no state")?.alpha_test;
        let mut state = object_state::resolve_layer(&compiled.implementation.render_states, entry, alpha_test)?;
        if item.flags.contains(PartFlags::STENCIL_MARK) && !pass.stencil_off {
            object_state::mark(&mut state);
        }
        out.push(PlannedDraw {
            instance,
            part: at,
            implementation,
            program,
            buffer,
            indices: p.indices.clone(),
            ranges: ranges.clone(),
            aux_depth: None,
            vertex,
            pixel,
            state,
            texaccum: compiled.texaccum.contains(&true),
            view: compiled.view.contains(&true),
            default_final: compiled.default_final,
            prt: item.prt.clone().map(|s| (s, None)),
        });
    }
    if out.is_empty() {
        return Err(format!("its passes of layer {layer} draw nothing"));
    }
    Ok(out)
}

/// What a part's per-object constants are made from: its object's
/// lighting copy and the inputs' defaults.
#[derive(Clone, Copy)]
struct PartInputs<'a> {
    instance: frame_scene::InstanceKey,
    lighting: Option<&'a Lighting>,
    defaults: Option<Defaults>,
    /// The subparts the item's record chose; `None`, the whole part.
    subparts: Option<Subparts>,
    /// Whether the emitter draws the part as a strip
    /// (`frame_scene::PartFlags::STRIP`); `None`, in the part's own
    /// topology.
    strip: Option<bool>,
}

impl<'a> PartInputs<'a> {
    fn of(frame: &'a ObjectFrame) -> Self {
        Self {
            instance: frame_scene::InstanceKey(frame.handle),
            lighting: frame.lighting.as_ref(),
            defaults: frame.defaults,
            subparts: None,
            strip: None,
        }
    }
}

/// The index ranges a part draws (`whole` its own topology's, `subparts`
/// its chosen subparts', `listed` a strip part's as a list) for a record
/// that draws it as a strip or not (`strip`, `None` in the part's own
/// topology) and chooses `chosen` of its subparts. The emitter `0x0004daa0`
/// draws a part as a strip only with bit 9 and the part's triangle-list
/// flag clear (`0x0004d5e2`), so a strip part whose record lacks bit 9
/// draws its indices as a list, and its chosen subparts as the list loop
/// `0x0004db68` joins them ([`SubpartRanges::drawn_as_list`]). A list
/// part draws as a list whatever the record says.
fn part_ranges(
    whole: &Range<u32>, subparts: Option<&SubpartRanges>, listed: Option<&Range<u32>>, strip: Option<bool>,
    chosen: Option<Subparts>,
) -> Result<Vec<Range<u32>>, String> {
    let native = || subparts.ok_or("the record chooses subparts and the part has none natively");
    match (listed, strip, chosen) {
        (Some(_), Some(false), Some(chosen)) => native()?.drawn_as_list(chosen),
        (Some(listed), Some(false), None) => Ok(vec![listed.clone()]),
        (_, _, None) => Ok(vec![whole.clone()]),
        (_, _, Some(chosen)) => native()?.drawn(chosen),
    }
}

fn index_parts(parts: &[LitPart]) -> HashMap<(u16, u8), usize> {
    let mut index = HashMap::with_capacity(parts.len());
    for (i, p) in parts.iter().enumerate() {
        index.entry((p.section, p.part)).or_insert(i);
    }
    index
}

fn plan_indexed_part<'a>(
    parts: &[LitPart], part: Option<usize>,
    pass_of: &dyn Fn(usize) -> Option<(&'a ObjectPass, Option<&'a InheritedState>)>,
    frame: PartInputs<'_>, key: &ItemKey, rows: &[Row], entry: &EntryStates, scene: bool,
) -> Result<PlannedDraw, String> {
    let part = part.ok_or_else(|| format!("section {} part {} is not a native part", key.section, key.part))?;
    let p = &parts[part];
    if p.shader_tag as u16 != key.shader as u16 || (scene && p.shader_tag != key.shader) {
        return Err(format!("the item names shader {:#010x}, the part's is {:#010x}", key.shader, p.shader_tag));
    }
    let (program, buffer) = p
        .implementations
        .get(key.implementation)
        .copied()
        .flatten()
        .ok_or_else(|| format!("the part has no implementation {}", key.implementation))?;
    let (pass, inherited) = pass_of(program).ok_or("its program is missing")?;
    let ranges = part_ranges(&p.indices, p.subparts.as_ref(), p.listed.as_ref(), frame.strip, frame.subparts)?;
    let defaults = frame.defaults.ok_or("the inputs' defaults do not read")?;
    let inputs = ObjectInputs { lighting: frame.lighting, defaults: &defaults, shader: p.shader };
    let vertex = object_vertex_uniforms(&pass.program.vertex, &pass.implementation, &inputs, rows, p.decompression)?;
    let pixel = object_pixel_uniforms(&pass.program.pixel, &pass.implementation, &inputs)?;
    let compiled = pass.program.state.as_ref().ok_or("its pass has no state")?.alpha_test;
    let resolve = if scene { object_state::resolve_scene } else { object_state::resolve };
    let mut cached = inherited.map(|s| s.lock().unwrap_or_else(|e| e.into_inner()));
    let state = if let Some((_, _, state)) = cached.as_deref().and_then(Option::as_ref)
        .filter(|(was_scene, previous, _)| *was_scene == scene && previous == entry) {
        *state
    } else {
        let state = resolve(&pass.implementation.render_states, entry, compiled)?;
        if let Some(slot) = cached.as_deref_mut() { *slot = Some((scene, entry.clone(), state)); }
        state
    };
    Ok(PlannedDraw {
        instance: frame.instance,
        part,
        implementation: key.implementation,
        program,
        buffer,
        indices: p.indices.clone(),
        ranges,
        aux_depth: None,
        vertex,
        pixel,
        state,
        texaccum: pass.texaccum.contains(&true),
        view: pass.view.contains(&true),
        default_final: pass.default_final,
        prt: None,
    })
}

/// The engine's textures the lit build may bind: whether the vector
/// normalization cube loads (engine texture 1), what an object's draw
/// samples for the lightmap ([`crate::object_program::OBJECT_LIGHTMAP`])
/// when it is uniform ([`uniform`]), whether the host binds the
/// texture accumulation target (engine texture 3) per pass, and whether
/// it binds a copy of the view's own target (engine texture 5,
/// [`crate::object_program::VIEW_TEXTURE`]) to a world layer's pass
/// ([`LitObjects::view_copy`]; layer 3's never sample it).
#[derive(Clone, Debug, Default)]
pub struct EngineStages {
    pub normalization: bool,
    pub lightmap: Option<Image>,
    pub texaccum: bool,
    pub view: bool,
}

/// Sets the coordinate scale of the stages in `stages` (those sampling
/// the texture accumulation target) to one over its guest size: the
/// game's programs address that linear target in texels, as the D3D path
/// samples a linear texture.
pub fn texaccum_scale(pixel: &mut PixelUniforms, stages: [bool; 4], guest_size: (u32, u32)) {
    for (n, on) in stages.into_iter().enumerate() {
        if on {
            pixel.tex_scale[n] = [1.0 / guest_size.0.max(1) as f32, 1.0 / guest_size.1.max(1) as f32, 1.0, 1.0];
        }
    }
}

/// Whether every texel of `image`, at every level, is the same: then it
/// samples the same wherever its coordinates fall, with any filter.
pub fn uniform(image: &Image) -> bool {
    let first = image.rgba.get(..4);
    first.is_some() && image.rgba.chunks_exact(4).chain(image.mips.iter().flat_map(|m| m.chunks_exact(4))).all(|t| Some(t) == first)
}

/// `streams` with a zeroed stream of type [`LIGHTMAP_STREAM`] after them
/// (where the draw adds the lightmap coordinates), its bytes in `zeros`.
fn with_lightmap_stream<'a>(
    elements: &mut Vec<Vec<crate::object_streams::StreamElement>>,
    data: &mut Vec<Option<StreamData<'a>>>,
    formats: &StreamFormats,
    zeros: &'a [u8],
    stride: usize,
) -> Result<(), String> {
    elements.push(formats.elements(LIGHTMAP_STREAM)?);
    data.push(Some(StreamData { bytes: zeros, stride }));
    Ok(())
}

/// The bytes one vertex of stream type `kind` takes.
fn stream_stride(formats: &StreamFormats, kind: u8) -> Result<usize, String> {
    Ok(formats
        .elements(kind)?
        .iter()
        .map(|e| match *e {
            crate::object_streams::StreamElement::Skip(n) => usize::from(n),
            crate::object_streams::StreamElement::Attribute { format, .. } => format.size(),
        })
        .sum())
}

/// One mode section as the lit build reads it: its index (the render
/// record key's section), its classification inputs (`+0x18` geometry
/// classification, `+0x14` influences), its decompression rows, its
/// streams by descriptor slot (stream type, bytes), its vertex count, and
/// per drawn part its material and triangles.
#[derive(Clone)]
pub struct LitSection<'a> {
    pub index: u16,
    pub geometry: u16,
    pub influences: u8,
    pub decompression: [Row; 3],
    pub streams: Vec<Option<(u8, StreamData<'a>)>>,
    pub vertex_count: usize,
    /// Its drawn opaque parts: index among the section's parts (a record
    /// key's bits 0..3), material and triangles.
    pub parts: Vec<(u8, u16, Vec<[u16; 3]>)>,
    /// By part index, the subparts of the parts a record can draw a
    /// choice of; a part not here draws whole only.
    pub subparts: HashMap<u8, LitSubparts>,
    /// By part index, each drawn strip part's indices decoded as a list
    /// (`tags::geometry::list_triangles`): what the emitter draws for a
    /// record without bit 9 ([`LitPart::listed`]).
    pub listed: HashMap<u8, Vec<[u16; 3]>>,
}

/// A part's subparts as the emitter draws a chosen one
/// (`0x0004daa0`): the section's index of its first subpart, and each
/// subpart's triangles decoded alone (a strip part's subparts each their
/// own strip; a list part's whole triangles, so that consecutive ones
/// joined are the list the guest draws).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LitSubparts {
    pub first: u16,
    pub triangles: Vec<Vec<[u16; 3]>>,
    /// A strip part's subparts as a record without bit 9 draws a choice
    /// of them; `None` for a list part.
    pub listed: Option<ListedSubparts>,
}

/// A strip part's subparts as the emitter's list loop reads them for a
/// record without bit 9 (`0x0004db68`): it joins a run of
/// index-contiguous chosen subparts into one `TRIANGLELIST` draw of the
/// section's indices, so a triangle can span two subparts and a run's
/// last `n % 3` indices draw nothing. `indices` holds the section's
/// indices from the subparts' lowest to their highest, verbatim; `spans`
/// each subpart's first index (from the lowest) and count.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedSubparts {
    pub indices: Vec<u16>,
    pub spans: Vec<(u32, u32)>,
}

/// The subparts of `part` (a section's part, over the section's `indices`
/// and `subparts`) as [`LitSubparts`]: each decoded alone in the part's
/// topology, and a strip part's also as the list draw reads them
/// ([`ListedSubparts`]). `None` when it has none or more than
/// [`Subparts::MAX`], when one does not lie in the indices, or when a
/// list part's subpart is not whole triangles (the guest joins
/// index-contiguous chosen subparts into one list, which would then cut
/// its triangles differently).
pub fn lit_subparts(
    indices: &[u16], subparts: &[tags::geometry::Subpart], part: &tags::geometry::Part,
) -> Option<LitSubparts> {
    let count = usize::from(part.subpart_count);
    if count == 0 || count > usize::from(Subparts::MAX) {
        return None;
    }
    let first = usize::from(part.first_subpart);
    let list = part.is_triangle_list();
    let mut triangles = Vec::with_capacity(count);
    let mut spans = Vec::with_capacity(count);
    for sub in subparts.get(first..first + count)? {
        let start = usize::from(sub.first_index);
        let run = indices.get(start..start + usize::from(sub.index_count))?;
        if list && run.len() % 3 != 0 {
            return None;
        }
        triangles.push(if list { tags::geometry::list_triangles(run) } else { tags::geometry::strip_triangles(run) });
        spans.push((start, run.len()));
    }
    let listed = (!list).then(|| {
        let low = spans.iter().map(|&(start, _)| start).min().unwrap_or(0);
        let high = spans.iter().map(|&(start, n)| start + n).max().unwrap_or(low);
        ListedSubparts {
            indices: indices[low..high].to_vec(),
            spans: spans.iter().map(|&(start, n)| ((start - low) as u32, n as u32)).collect(),
        }
    });
    Some(LitSubparts { first: part.first_subpart, triangles, listed })
}

/// A strip `part`'s indices (over the section's `indices`) decoded as a
/// list, as the emitter draws them for a record without bit 9
/// ([`LitSection::listed`]): D3D's list of `n` indices is its `n / 3`
/// whole triangles. `None` for a list part, or one whose range does not
/// lie in the indices.
pub fn lit_listed(indices: &[u16], part: &tags::geometry::Part) -> Option<Vec<[u16; 3]>> {
    if part.is_triangle_list() {
        return None;
    }
    Some(tags::geometry::list_triangles(indices.get(part.index_range())?))
}

/// A material the lit build takes: resolved, and its shader values.
pub struct LitMaterial {
    pub material: tags::material::Material,
    pub shader: ShaderValues,
}

/// What [`lit_model`] built: the input to upload, the materials taken,
/// those refused with the reason, and the implementations (bit `n`) every
/// taken material has (an object whose records draw another stays on D3D).
#[derive(Default)]
pub struct LitBuild {
    pub input: LitModelInput,
    pub taken: Vec<u16>,
    pub refused: Vec<(u16, String)>,
    pub implementations: u8,
    /// The world layers' implementations ([`OBJECT_LAYERS`]) that did not
    /// compile: material, layer and why. Their items keep their D3D draws.
    pub layer_refused: Vec<(u16, u8, String)>,
}

/// What a `vrtx` gives a section: its program for the section's
/// classification and that classification's input semantics.
pub type VertexSource<'s> = dyn FnMut(map::TagId, u16, u8) -> Result<(Vec<u8>, Vec<u16>), String> + 's;

/// A stage's image as the D3D path samples it (stage, bitmap, image).
pub type StageSource<'s> = dyn FnMut(usize, map::TagId, u32) -> Option<Image> + 's;

/// Builds the lit parts of `sections` for `materials` (model-local
/// material -> what it draws with): per material, each implementation of
/// its layer-3 pass (the record picks one, `0x0004cc70`) compiled for
/// every section it is in, through the declaration the game would build
/// there (with `engine`'s textures for the stages that sample the
/// normalization cube or the lightmap; one sampling the lightmap reads
/// the coordinates the draw adds as zeros, `engine.lightmap` being
/// uniform); an implementation that fails anywhere is left out. A
/// material
/// none of whose implementations compiles is refused, with the reasons;
/// one in no drawn section is neither taken nor refused. Each part
/// independently matches the implementation in its sorted dispatch.
pub fn lit_model(
    sections: &[LitSection<'_>],
    materials: &HashMap<u16, LitMaterial>,
    formats: &StreamFormats,
    engine: &EngineStages,
    vertex: &mut VertexSource<'_>,
    image: &mut StageSource<'_>,
) -> LitBuild {
    let mut out = LitBuild { implementations: u8::MAX, ..LitBuild::default() };
    // (section, declaration) -> buffer.
    let mut buffers: HashMap<(u16, Vec<Element>), usize> = HashMap::new();
    // Per section, its lit parts' triangles in one list: (material, part,
    // range, subpart ranges, the range as a list), one per part (a record
    // draws one part), each part's subparts and list after it.
    type Listed = (u16, u8, Range<u32>, Option<SubpartRanges>, Option<Range<u32>>);
    let mut lists: HashMap<u16, (Vec<u32>, Vec<Listed>)> = HashMap::new();
    for s in sections {
        let mut list = Vec::new();
        let mut ranges = Vec::new();
        for (part, material, triangles) in &s.parts {
            if !materials.contains_key(material) {
                continue;
            }
            let start = list.len() as u32;
            list.extend(triangles.iter().flatten().map(|&i| u32::from(i)));
            let whole = start..list.len() as u32;
            let subparts = s.subparts.get(part).map(|sub| {
                let ranges = sub
                    .triangles
                    .iter()
                    .map(|t| {
                        let start = list.len() as u32;
                        list.extend(t.iter().flatten().map(|&i| u32::from(i)));
                        start..list.len() as u32
                    })
                    .collect();
                // A strip part's subparts as the list draw reads them,
                // verbatim after their own triangles.
                let listed = sub.listed.as_ref().map(|l| {
                    let start = list.len() as u32;
                    list.extend(l.indices.iter().map(|&i| u32::from(i)));
                    ListedRanges { indices: start..list.len() as u32, spans: l.spans.clone() }
                });
                SubpartRanges { first: sub.first, ranges, listed }
            });
            let listed = s.listed.get(part).map(|t| {
                let start = list.len() as u32;
                list.extend(t.iter().flatten().map(|&i| u32::from(i)));
                start..list.len() as u32
            });
            ranges.push((*material, *part, whole, subparts, listed));
        }
        lists.insert(s.index, (list, ranges));
    }
    let mut ids: Vec<u16> = materials.keys().copied().collect();
    ids.sort_unstable();
    for m in ids {
        let lit = &materials[&m];
        let pass = match object_layer3(&lit.material) {
            Ok(p) => p,
            Err(e) => {
                out.refused.push((m, e));
                continue;
            }
        };
        // The material's parts: (section, part, index range, subparts,
        // the range as a list).
        let parts: Vec<MaterialPart<'_, '_>> = material_parts(sections, &lists, m);
        if parts.is_empty() {
            continue;
        }
        // Compiled once per section: its parts share the programs.
        let mut secs: Vec<&LitSection<'_>> = Vec::new();
        for (s, _, _, _, _) in &parts {
            if !secs.iter().any(|x| x.index == s.index) {
                secs.push(s);
            }
        }
        let mut per: Vec<[Option<(LitProgramInput, usize)>; IMPLEMENTATIONS]> = (0..secs.len()).map(|_| Default::default()).collect();
        let mut mask = 0u8;
        let mut reasons = Vec::new();
        for imp in pass.implementations.iter().filter(|i| i.index < IMPLEMENTATIONS) {
            let n = imp.index;
            // Buffers made for an implementation that fails later stay
            // unused.
            let samples_lightmap = engine.lightmap.is_some()
                && imp.textures.iter().flatten().any(|t| t.source == tags::material::TextureSource::Lightmap);
            let mut compile = || -> Result<Vec<(LitProgramInput, usize)>, String> {
                let vs = imp.vertex_shader.ok_or_else(|| format!("implementation {n} has no vertex program"))?;
                let mut got = Vec::new();
                for s in &secs {
                    let mut streams = s
                        .streams
                        .iter()
                        .map(|t| t.map_or(Ok(Vec::new()), |(kind, _)| formats.elements(kind)))
                        .collect::<Result<Vec<_>, _>>()?;
                    let stride = if samples_lightmap { stream_stride(formats, LIGHTMAP_STREAM)? } else { 0 };
                    let zeros = vec![0u8; stride * s.vertex_count];
                    let mut data: Vec<Option<StreamData<'_>>> = s.streams.iter().map(|t| t.map(|(_, d)| d)).collect();
                    if samples_lightmap {
                        with_lightmap_stream(&mut streams, &mut data, formats, &zeros, stride)?;
                    }
                    let (microcode, semantics) = vertex(vs, s.geometry, s.influences).map_err(|e| format!("section {}: {e}", s.index))?;
                    let decl = declaration(&semantics, &streams);
                    let compiled = object_pass(pass.pass, imp, &microcode, registers(&decl), input_fetch(&decl), engine.lightmap.is_some())
                        .map_err(|e| format!("section {} implementation {n}: {e}", s.index))?;
                    let key = (s.index, decl);
                    let buffer = match buffers.get(&key) {
                        Some(&b) => b,
                        None => {
                            let vertices = gather(&key.1, &data, s.vertex_count).map_err(|e| format!("section {}: {e}", s.index))?;
                            out.input.buffers.push(LitBufferInput {
                                registers: key.1.iter().map(|e| e.register).collect(),
                                formats: key.1.iter().map(|e| e.format).collect(),
                                vertices,
                                indices: lists[&s.index].0.clone(),
                            });
                            buffers.insert(key, out.input.buffers.len() - 1);
                            out.input.buffers.len() - 1
                        }
                    };
                    let mut images: [Option<Image>; 4] = Default::default();
                    for (k, d) in compiled.program.dims.iter().enumerate() {
                        if d.is_none() {
                            continue;
                        }
                        if compiled.normalization[k] {
                            if !engine.normalization {
                                return Err(format!("implementation {n} stage {k} samples the normalization cube, which does not load"));
                            }
                            continue;
                        }
                        if compiled.texaccum[k] {
                            if !engine.texaccum {
                                return Err(format!("implementation {n} stage {k} samples the texture accumulation target, which the host does not bind"));
                            }
                            continue;
                        }
                        if compiled.view[k] {
                            return Err(format!("implementation {n} stage {k} samples the view's own target, which layer 3's draw does not copy"));
                        }
                        if compiled.program.stages[k] == Stage::Lightmap {
                            images[k] = engine.lightmap.clone();
                            continue;
                        }
                        let Stage::Bitmap { bitmap, image: i } = compiled.program.stages[k] else {
                            return Err(format!("implementation {n} stage {k} samples no bitmap"));
                        };
                        images[k] = Some(image(k, bitmap, i).ok_or_else(|| format!("implementation {n} stage {k}'s bitmap does not decode"))?);
                    }
                    got.push((LitProgramInput { pass: compiled, images }, buffer));
                }
                Ok(got)
            };
            match compile() {
                Ok(got) => {
                    mask |= 1 << n;
                    for (slot, x) in per.iter_mut().zip(got) {
                        slot[n] = Some(x);
                    }
                }
                Err(e) => reasons.push(e),
            }
        }
        if mask == 0 {
            let why = if reasons.is_empty() { "no implementation".to_string() } else { reasons.join("; ") };
            out.refused.push((m, why));
            continue;
        }
        let mut by_section: Vec<[Option<(usize, usize)>; IMPLEMENTATIONS]> = Vec::with_capacity(secs.len());
        for slots in per {
            let mut implementations = [None; IMPLEMENTATIONS];
            for (n, slot) in slots.into_iter().enumerate() {
                let Some((program, buffer)) = slot else { continue };
                out.input.programs.push(program);
                implementations[n] = Some((out.input.programs.len() - 1, buffer));
            }
            by_section.push(implementations);
        }
        for (s, part, indices, subparts, listed) in parts {
            let k = secs.iter().position(|x| x.index == s.index).expect("listed above");
            let implementations = by_section[k];
            out.input.parts.push(LitPart {
                section: s.index,
                part,
                decompression: s.decompression,
                shader_tag: lit.material.shader.0,
                shader: lit.shader,
                indices,
                implementations,
                subparts,
                listed,
            });
        }
        out.taken.push(m);
        out.implementations &= mask;
    }
    if out.taken.is_empty() {
        out.implementations = 0;
    }
    // The world layers besides 3: each pass's implementations compiled
    // per section as layer 3's are, over the same buffers, without the
    // lightmap's stream (the draw adds it only in layer 3).
    let mut ids: Vec<u16> = materials.keys().copied().collect();
    ids.sort_unstable();
    for m in ids {
        let lit = &materials[&m];
        let parts = material_parts(sections, &lists, m);
        if parts.is_empty() {
            continue;
        }
        let mut secs: Vec<&LitSection<'_>> = Vec::new();
        for (s, _, _, _, _) in &parts {
            if !secs.iter().any(|x| x.index == s.index) {
                secs.push(s);
            }
        }
        for layer in OBJECT_LAYERS {
            let Ok(passes) = object_layer_passes(&lit.material, layer) else { continue };
            // A copy of the view is made only for the sorted handlers'
            // layers (`native_layers_glue`'s encoded pass).
            let copies_view = engine.view && OVERRIDE_LAYERS.contains(&layer);
            let mut per: Vec<Vec<LitLayerPass>> = secs.iter().map(|_| vec![LitLayerPass::default(); passes.len()]).collect();
            let mut drawn = false;
            for (k, pass) in passes.iter().enumerate() {
                for slot in per.iter_mut() {
                    slot[k].count = pass.implementations.len();
                }
                for (n, imp) in pass.implementations.iter().enumerate().take(IMPLEMENTATIONS) {
                    if imp.vertex_shader.is_none() {
                        for slot in per.iter_mut() {
                            slot[k].empty |= 1 << n;
                        }
                        continue;
                    }
                    let index_list = |s: u16| lists[&s].0.clone();
                    let compiled = compile_layer_sections(
                        None, pass.pass, imp, &secs, formats, engine, copies_view, &mut *vertex, &mut *image, &mut buffers,
                        &mut out.input.buffers, &index_list,
                    );
                    match compiled {
                        Ok(got) => {
                            for (slot, (program, buffer)) in per.iter_mut().zip(got) {
                                out.input.programs.push(program);
                                slot[k].implementations[n] = Some((out.input.programs.len() - 1, buffer));
                            }
                            drawn = true;
                        }
                        Err(e) => out.layer_refused.push((m, layer, format!("pass {k} implementation {n}: {e}"))),
                    }
                    // Layer 5's variant for an object whose model has a PRT
                    // block: the same pass over the section's streams and
                    // the PRT stream ([`PrtInput`]).
                    if layer == PRT_LAYER {
                        let compiled = formats.elements(PRT_STREAM).and_then(|prt| {
                            compile_layer_sections(
                                Some(prt.as_slice()), pass.pass, imp, &secs, formats, engine, false, &mut *vertex, &mut *image,
                                &mut buffers, &mut out.input.buffers, &index_list,
                            )
                        });
                        match compiled {
                            Ok(got) => {
                                for (slot, (program, buffer)) in per.iter_mut().zip(got) {
                                    out.input.programs.push(program);
                                    slot[k].prt[n] = Some((out.input.programs.len() - 1, buffer));
                                }
                                drawn = true;
                            }
                            Err(e) => out.layer_refused.push((m, layer, format!("pass {k} implementation {n} with the PRT stream: {e}"))),
                        }
                    }
                }
            }
            if !drawn {
                continue;
            }
            for (s, part, indices, subparts, listed) in &parts {
                let k = secs.iter().position(|x| x.index == s.index).expect("listed above");
                out.input.layers.push(LitLayerPart {
                    layer,
                    section: s.index,
                    part: *part,
                    decompression: s.decompression,
                    shader_tag: lit.material.shader.0,
                    shader: lit.shader,
                    indices: indices.clone(),
                    subparts: subparts.clone(),
                    listed: listed.clone(),
                    passes: per[k].clone(),
                });
            }
        }
    }
    out
}

/// A material's part in [`lit_model`]'s lists: its section, its index
/// among the section's parts, its index range, its subparts' and its range
/// as a list.
type MaterialPart<'s, 'a> = (&'s LitSection<'a>, u8, Range<u32>, Option<SubpartRanges>, Option<Range<u32>>);

/// Material `m`'s parts in `lists` (by section: the section's index list
/// and its parts' ranges), in `sections`' order.
fn material_parts<'s, 'a>(
    sections: &'s [LitSection<'a>],
    lists: &HashMap<u16, (Vec<u32>, Vec<(u16, u8, Range<u32>, Option<SubpartRanges>, Option<Range<u32>>)>)>,
    m: u16,
) -> Vec<MaterialPart<'s, 'a>> {
    sections
        .iter()
        .flat_map(|s| {
            lists[&s.index].1.iter().filter(move |r| r.0 == m).map(move |r| (s, r.1, r.2.clone(), r.3.clone(), r.4.clone()))
        })
        .collect()
}

/// One implementation of a world layer's pass (`pass` its tag)
/// compiled for every section in `secs` through the declaration the game
/// builds there ([`object_layer_pass`]), each section's buffer for that
/// declaration made once over `buffers` (its indices the section's list,
/// `index_list`), with its stages' images; the first failure, as the
/// reason the implementation is left out. A stage sampling the view's own
/// target compiles only with `copies_view` (bound per encode,
/// [`LitObjects::view_copy`]).
///
/// With `prt` (layer 5, the PRT stream's format), the declaration has the
/// PRT stream after the section's, as the stream setup `0x0001c8c0` adds
/// it: the buffer holds the section's registers only, and the program
/// carries the PRT stream's ([`PrtInput`]), which the draw gathers from
/// guest memory ([`LitObjects::fill_prt`]), and the highest PRT constant
/// it reads ([`prt_constant_read`]). Without it the program reads its
/// constants as an item without a PRT stream leaves them, as in the
/// other layers.
#[allow(clippy::too_many_arguments)]
fn compile_layer_sections(
    prt: Option<&[StreamElement]>,
    pass: map::TagId,
    imp: &tags::material::ResolvedImplementation,
    secs: &[&LitSection<'_>],
    formats: &StreamFormats,
    engine: &EngineStages,
    copies_view: bool,
    vertex: &mut VertexSource<'_>,
    image: &mut StageSource<'_>,
    buffers: &mut HashMap<(u16, Vec<Element>), usize>,
    out: &mut Vec<LitBufferInput>,
    index_list: &dyn Fn(u16) -> Vec<u32>,
) -> Result<Vec<(LitProgramInput, usize)>, String> {
    let vs = imp.vertex_shader.ok_or("no vertex program")?;
    let mut got = Vec::with_capacity(secs.len());
    for s in secs {
        let streams = s
            .streams
            .iter()
            .map(|t| t.map_or(Ok(Vec::new()), |(kind, _)| formats.elements(kind)))
            .collect::<Result<Vec<_>, _>>()?;
        let data: Vec<Option<StreamData<'_>>> = s.streams.iter().map(|t| t.map(|(_, d)| d)).collect();
        let (microcode, semantics) = vertex(vs, s.geometry, s.influences).map_err(|e| format!("section {}: {e}", s.index))?;
        let last_constant = match prt {
            Some(_) => {
                let program = shaders::vsh::Program::parse(&microcode).map_err(|e| format!("section {}: vertex program: {e:?}", s.index))?;
                prt_constant_read(&program)
            }
            None => None,
        };
        let mut all = streams.clone();
        if let Some(p) = prt {
            all.push(p.to_vec());
        }
        let full = declaration(&semantics, &all);
        let mut compiled = object_layer_pass(pass, imp, &microcode, registers(&full), input_fetch(&full))
            .map_err(|e| format!("section {}: {e}", s.index))?;
        if prt.is_some() {
            compiled.prt = Some(PrtInput {
                semantics: semantics.clone(),
                streams: streams.clone(),
                registers: full.iter().filter(|e| e.stream == streams.len()).map(|e| e.register).collect(),
                provided: registers(&full),
                fetch: input_fetch(&full),
                last_constant,
            });
        }
        let decl: Vec<Element> = full.into_iter().filter(|e| e.stream < streams.len()).collect();
        let key = (s.index, decl);
        let buffer = match buffers.get(&key) {
            Some(&b) => b,
            None => {
                let vertices = gather(&key.1, &data, s.vertex_count).map_err(|e| format!("section {}: {e}", s.index))?;
                out.push(LitBufferInput {
                    registers: key.1.iter().map(|e| e.register).collect(),
                    formats: key.1.iter().map(|e| e.format).collect(),
                    vertices,
                    indices: index_list(s.index),
                });
                buffers.insert(key, out.len() - 1);
                out.len() - 1
            }
        };
        let mut images: [Option<Image>; 4] = Default::default();
        for (k, d) in compiled.program.dims.iter().enumerate() {
            if d.is_none() {
                continue;
            }
            if compiled.normalization[k] {
                if !engine.normalization {
                    return Err(format!("stage {k} samples the normalization cube, which does not load"));
                }
                continue;
            }
            if compiled.texaccum[k] {
                if !engine.texaccum {
                    return Err(format!("stage {k} samples the texture accumulation target, which the host does not bind"));
                }
                continue;
            }
            if compiled.view[k] {
                if !copies_view {
                    return Err(format!("stage {k} samples the view's own target, which the host copies only for the sorted handlers' layers"));
                }
                continue;
            }
            let Stage::Bitmap { bitmap, image: i } = compiled.program.stages[k] else {
                return Err(format!("stage {k} samples no bitmap"));
            };
            images[k] = Some(image(k, bitmap, i).ok_or_else(|| format!("stage {k}'s bitmap does not decode"))?);
        }
        got.push((LitProgramInput { pass: compiled, images }, buffer));
    }
    Ok(got)
}

#[cfg(test)]
// The tests name one-range draws as literal lists of ranges.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use crate::object_lighting::Lighting;
    use gfx::state::RenderStateType;

    /// Synthetic stream formats: type 1 a 3-float position (semantic 0),
    /// a 2-float coordinate (semantic 3) and a 3-float normal (semantic 2);
    /// the lightmap stream's type a 2-float coordinate (semantic 7).
    fn formats() -> StreamFormats {
        let size = crate::object_streams::STREAM_FORMAT_SIZE;
        let mut t = vec![0u8; (usize::from(LIGHTMAP_STREAM) + 1) * size];
        t[size..size + 8].copy_from_slice(&[1, 0, 2, 3, 1, 2, 2, 0xff]);
        let l = usize::from(LIGHTMAP_STREAM) * size;
        t[l..l + 4].copy_from_slice(&[LIGHTMAP_STREAM, 7, 1, 0xff]);
        StreamFormats::from_bytes(&t)
    }

    #[test]
    fn a_uniform_image_is_one_texel_everywhere() {
        let grey = Image { width: 2, height: 1, rgba: vec![9, 9, 9, 255, 9, 9, 9, 255], mips: vec![vec![9, 9, 9, 255]], blocks: None };
        assert!(uniform(&grey));
        let mut mip = grey.clone();
        mip.mips[0][0] = 10;
        assert!(!uniform(&mip), "a level below differs");
        let mut texel = grey.clone();
        texel.rgba[4] = 0;
        assert!(!uniform(&texel));
        assert!(!uniform(&Image { width: 0, height: 0, rgba: Vec::new(), mips: Vec::new(), blocks: None }));
    }

    #[test]
    fn an_implementation_sampling_the_lightmap_gets_the_objects_lightmap_and_zeroed_coordinates() {
        use crate::object_program::tests::{lit, rigid_program};
        let mut bytes = Vec::new();
        for k in 0..3 {
            for v in [k as f32, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0] {
                bytes.extend(v.to_le_bytes());
            }
        }
        let section = LitSection {
            index: 0,
            geometry: 0,
            influences: 0,
            decompression: [[1.0; 4]; 3],
            streams: vec![Some((1, StreamData { bytes: &bytes, stride: 32 }))],
            vertex_count: 3,
            parts: vec![(0, 0, vec![[0, 1, 2]])],
            subparts: HashMap::new(),
            listed: HashMap::new(),
        };
        let mut imp = lit();
        imp.render_states = vec![state(RenderStateType::AlphaBlendEnable, 0), state(RenderStateType::ZWriteEnable, 1)];
        imp.textures[3] = Some(tags::material::StageTexture {
            source: tags::material::TextureSource::Lightmap,
            size: None,
            stage_size: None,
        });
        // The combiners sample t3 (PROJECT2D, 5 bits a stage): a stage the
        // shader does not sample has no texture to bind.
        imp.pixel_shader = shaders::PixelShaderDef { texture_modes: 1 << 15, ..Default::default() }.to_bytes();
        let mut materials = HashMap::new();
        materials.insert(0, LitMaterial { material: material(vec![imp]), shader: ShaderValues::default() });
        // v1 reads semantic 7: the lightmap coordinates the draw adds.
        let mut vertex = |_: map::TagId, _: u16, _: u8| Ok::<_, String>((rigid_program(), vec![0u16, 7, 2]));
        let mut image = |_: usize, _: map::TagId, _: u32| -> Option<Image> { None };
        let grey = Image { width: 1, height: 1, rgba: vec![128, 128, 128, 255], mips: Vec::new(), blocks: None };
        let without = lit_model(
            std::slice::from_ref(&section),
            &materials,
            &formats(),
            &EngineStages::default(),
            &mut vertex,
            &mut image,
        );
        assert!(without.taken.is_empty());
        assert!(without.refused[0].1.contains("samples the lightmap"), "{:?}", without.refused);
        let engine = EngineStages { normalization: false, lightmap: Some(grey.clone()), texaccum: false, view: false };
        let with = lit_model(&[section], &materials, &formats(), &engine, &mut vertex, &mut image);
        assert_eq!(with.taken, vec![0], "{:?}", with.refused);
        assert_eq!(with.input.programs[0].images[3], Some(grey), "stage 3 binds the object's lightmap");
        let b = &with.input.buffers[0];
        assert_eq!(b.registers, vec![0, 1, 2]);
        assert_eq!(b.decoded(0, 1), [0.0, 0.0, 0.0, 1.0], "v1, the lightmap coordinates, zero");
        assert_eq!(b.decoded(1, 0), [1.0, 0.0, 0.0, 1.0], "vertex 1's position from the section");
    }

    #[test]
    fn an_implementation_sampling_the_texture_accumulation_target_needs_the_host_to_bind_it() {
        use crate::object_program::TEXACCUM_TEXTURE;
        use crate::object_program::tests::{lit, rigid_program};
        let mut bytes = Vec::new();
        for k in 0..3 {
            for v in [k as f32, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0] {
                bytes.extend(v.to_le_bytes());
            }
        }
        let section = LitSection {
            index: 0,
            geometry: 0,
            influences: 0,
            decompression: [[1.0; 4]; 3],
            streams: vec![Some((1, StreamData { bytes: &bytes, stride: 32 }))],
            vertex_count: 3,
            parts: vec![(0, 0, vec![[0, 1, 2]])],
            subparts: HashMap::new(),
            listed: HashMap::new(),
        };
        let mut imp = lit();
        imp.render_states = vec![state(RenderStateType::AlphaBlendEnable, 0), state(RenderStateType::ZWriteEnable, 1)];
        imp.textures[2] = Some(tags::material::StageTexture {
            source: tags::material::TextureSource::Engine(TEXACCUM_TEXTURE),
            size: None,
            stage_size: None,
        });
        imp.pixel_shader = shaders::PixelShaderDef { texture_modes: 1 << 10, ..Default::default() }.to_bytes();
        let mut materials = HashMap::new();
        materials.insert(0, LitMaterial { material: material(vec![imp]), shader: ShaderValues::default() });
        let mut vertex = |_: map::TagId, _: u16, _: u8| Ok::<_, String>((rigid_program(), vec![0u16, 3, 2]));
        let mut image = |_: usize, _: map::TagId, _: u32| -> Option<Image> { None };
        let without = lit_model(
            std::slice::from_ref(&section),
            &materials,
            &formats(),
            &EngineStages::default(),
            &mut vertex,
            &mut image,
        );
        assert!(without.taken.is_empty());
        assert!(without.refused[0].1.contains("texture accumulation"), "{:?}", without.refused);
        let engine = EngineStages { texaccum: true, ..EngineStages::default() };
        let with = lit_model(&[section], &materials, &formats(), &engine, &mut vertex, &mut image);
        assert_eq!(with.taken, vec![0], "{:?}", with.refused);
        assert!(with.input.samples_texaccum());
        let pass = &with.input.programs[0].pass;
        assert_eq!(pass.texaccum, [false, false, true, false]);
        assert_eq!(with.input.programs[0].images[2], None, "no image: the pass's level is bound per encode");
        // Its coordinates are texels of the level.
        let mut pixel = pass.program.pixel;
        texaccum_scale(&mut pixel, pass.texaccum, (640, 480));
        assert_eq!(pixel.tex_scale[2], [1.0 / 640.0, 1.0 / 480.0, 1.0, 1.0]);
        assert_eq!(pixel.tex_scale[0], pass.program.pixel.tex_scale[0]);
    }

    /// Render state `t` set to `value`, at its slot in the game's table.
    fn state(t: RenderStateType, value: u32) -> tags::shader::RenderStateValue {
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let slot = (0..=255u8)
            .find(|&s| RenderStateType::from_guest_index(&table, u32::from(s)) == Some(t))
            .expect("a slot");
        tags::shader::RenderStateValue { state: slot, value }
    }

    fn material(implementations: Vec<tags::material::ResolvedImplementation>) -> tags::material::Material {
        tags::material::Material {
            shader: map::TagId(0xe000_0010),
            template: map::TagId(0xe000_0011),
            inputs: Default::default(),
            layers: vec![tags::material::ResolvedLayer {
                lod: 0,
                layer: crate::look::LIGHTMAP_LAYER,
                passes: vec![tags::material::ResolvedPass {
                    index: 0,
                    pass: map::TagId(0xe000_0001),
                    implementations,
                }],
            }],
        }
    }

    #[test]
    fn a_lit_material_compiles_every_implementation_it_may_draw_over_one_shared_buffer() {
        use crate::object_program::tests::{lit, rigid_program};
        // Three vertices of 32 bytes: position (k, 0, 0), coordinate, normal.
        let mut bytes = Vec::new();
        for k in 0..3 {
            for v in [k as f32, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0] {
                bytes.extend(v.to_le_bytes());
            }
        }
        let section = LitSection {
            index: 4,
            geometry: 0,
            influences: 0,
            decompression: [[1.0; 4]; 3],
            streams: vec![Some((1, StreamData { bytes: &bytes, stride: 32 }))],
            vertex_count: 3,
            parts: vec![(0, 5, vec![[2, 1, 0]]), (1, 0, vec![[0, 1, 2]]), (2, 3, vec![[0, 2, 1]]), (3, 1, vec![[1, 2, 0]])],
            // Part 2's subparts 7..10, the second empty, and as the list
            // draw reads them.
            subparts: HashMap::from([(
                2,
                LitSubparts {
                    first: 7,
                    triangles: vec![vec![[0, 2, 1]], Vec::new(), vec![[2, 1, 0]]],
                    listed: Some(ListedSubparts { indices: vec![0, 2, 1, 2, 1, 0], spans: vec![(0, 3), (3, 0), (3, 3)] }),
                },
            )]),
            listed: HashMap::new(),
        };
        // An opaque pass: no blending, depth written (both must be set).
        let opaque = |index: usize| {
            let mut imp = lit();
            imp.index = index;
            imp.render_states = vec![state(RenderStateType::AlphaBlendEnable, 0), state(RenderStateType::ZWriteEnable, 1)];
            imp
        };
        // One reading the time (scalar 2): never compiled.
        let timed = |index: usize| {
            let mut imp = opaque(index);
            imp.input_reads.scalars |= 1 << 2;
            imp
        };
        let lit_material = |imps| LitMaterial { material: material(imps), shader: ShaderValues::default() };
        let mut materials = HashMap::new();
        materials.insert(0, lit_material(vec![opaque(0), opaque(1)]));
        materials.insert(1, lit_material(vec![timed(0)]));
        // In no section: neither taken nor refused.
        materials.insert(2, lit_material(vec![opaque(0), opaque(1)]));
        // Implementation 1 does not compile: 0 alone.
        materials.insert(3, lit_material(vec![opaque(0), timed(1)]));
        let mut asked = Vec::new();
        let mut vertex = |vs: map::TagId, geometry: u16, influences: u8| {
            asked.push((vs, geometry, influences));
            Ok::<_, String>((rigid_program(), vec![0u16, 3, 2]))
        };
        let mut image = |_: usize, _: map::TagId, _: u32| -> Option<Image> { None };
        let LitBuild { input, taken, refused, implementations, .. } = lit_model(&[section], &materials, &formats(), &EngineStages::default(), &mut vertex, &mut image);
        assert_eq!(taken, vec![0, 3], "refused: {refused:?}");
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0].0, 1);
        assert!(refused[0].1.contains("reads scalar input 2"), "{}", refused[0].1);
        assert_eq!(implementations, 0b01, "material 3 has implementation 0 alone");
        assert_eq!(input.programs.len(), 3);
        assert_eq!(input.buffers.len(), 1, "both implementations read v0..v2 the same way");
        let b = &input.buffers[0];
        assert_eq!(b.registers, vec![0, 1, 2]);
        assert_eq!(b.decoded(1, 0), [1.0, 0.0, 0.0, 1.0], "vertex 1's position");
        assert_eq!(b.decoded(1, 1), [0.5, 0.5, 0.0, 1.0]);
        assert_eq!(
            b.indices,
            vec![0, 1, 2, 0, 2, 1, 0, 2, 1, 2, 1, 0, 0, 2, 1, 2, 1, 0, 1, 2, 0],
            "material 5 is not lit here; part 2's subparts and their list follow it"
        );
        assert_eq!(input.parts.len(), 2);
        let p = &input.parts[0];
        assert_eq!((p.section, p.part, p.indices.clone()), (4, 1, 0..3));
        assert_eq!(p.implementations, [Some((0, 0)), Some((1, 0)), None, None, None]);
        assert_eq!(p.subparts, None);
        let p = &input.parts[1];
        assert_eq!((p.part, p.indices.clone()), (2, 3..6));
        assert_eq!(p.implementations, [Some((2, 0)), None, None, None, None]);
        let listed = ListedRanges { indices: 12..18, spans: vec![(0, 3), (3, 0), (3, 3)] };
        assert_eq!(p.subparts, Some(SubpartRanges { first: 7, ranges: vec![6..9, 9..9, 9..12], listed: Some(listed) }));
        assert!(input.programs.iter().all(|p| p.pass.program.inputs == 0b111));
        assert_eq!(asked[0], (map::TagId(0xe000_0002), 0, 0));
    }

    #[test]
    fn a_buffers_registers_are_slots_in_order_read_as_the_d3d_path_fetches_them() {
        let formats = [Attribute::NormShort(3), Attribute::Float(2), Attribute::NormPacked3];
        let a = attributes(&[0, 3, 9], &formats);
        let got: Vec<(u64, u32, wgpu::VertexFormat)> = a.iter().map(|x| (x.offset, x.shader_location, x.format)).collect();
        assert_eq!(
            got,
            vec![(0, 0, wgpu::VertexFormat::Snorm16x4), (16, 3, wgpu::VertexFormat::Float32x2), (32, 9, wgpu::VertexFormat::Uint32)]
        );
        let decl = [
            Element { register: 0, stream: 0, offset: 0, format: Attribute::NormShort(3) },
            Element { register: 3, stream: 0, offset: 8, format: Attribute::Float(2) },
            Element { register: 9, stream: 0, offset: 16, format: Attribute::NormPacked3 },
        ];
        let fetch = input_fetch(&decl);
        use shaders::vsh::InputFetch as I;
        assert_eq!((fetch[0], fetch[3], fetch[9], fetch[1]), (I::Xyz1, I::Float, I::NormPacked3, I::Float));
        // Every guest format, as the D3D path's raw fetch reads it.
        for code in 0..=17u8 {
            let f = Attribute::from_code(code).unwrap();
            let (format, _) = vertex_fetch(f);
            assert!(format.size() as usize <= SLOT, "{f:?}");
            assert!(format.size() as usize >= f.size(), "{f:?} reads its bytes");
        }
    }

    fn part(section: u16, part: u8, implementations: [Option<(usize, usize)>; IMPLEMENTATIONS]) -> LitPart {
        LitPart {
            section,
            part,
            decompression: [[0.0; 4]; 3],
            shader_tag: 0xe5da_0465,
            shader: ShaderValues::default(),
            indices: u32::from(part) * 3..u32::from(part) * 3 + 3,
            implementations,
            subparts: None,
            listed: None,
        }
    }

    /// An opaque implementation `index` of the lit fixture, setting depth
    /// Equal with no writes and leaving the rest (cull, winding, colour
    /// writes) to the device.
    fn equal_pass(index: usize) -> ObjectPass {
        use crate::object_program::tests::{lit, rigid_program};
        let mut imp = lit();
        imp.index = index;
        imp.render_states = vec![
            state(RenderStateType::AlphaBlendEnable, 0),
            state(RenderStateType::ZWriteEnable, 0),
            state(RenderStateType::ZFunc, gfx::state::CompareFunc::Equal.xbox_value()),
        ];
        object_pass(map::TagId(0xe000_0001), &imp, &rigid_program(), crate::program::INPUT_MASK, [shaders::vsh::InputFetch::Float; 16], true)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// The device at the layer-3 call: W-buffered, LessEqual written,
    /// `CULL_CW`, nothing else on.
    fn entry() -> EntryStates {
        use gfx::state::{CompareFunc, CullMode, DepthBufferMode, FillMode, FrontFace};
        EntryStates::read(|t| {
            Some(match t {
                RenderStateType::ZEnable => DepthBufferMode::W.xbox_value(),
                RenderStateType::ZFunc => CompareFunc::LessEqual.xbox_value(),
                RenderStateType::ZWriteEnable => 1,
                RenderStateType::ColorWriteEnable => 0x0101_0101,
                RenderStateType::CullMode => CullMode::Cw.xbox_value(),
                RenderStateType::FrontFace => FrontFace::Cw.xbox_value(),
                RenderStateType::FillMode | RenderStateType::BackFillMode => FillMode::Solid.xbox_value(),
                _ => 0,
            })
        })
    }

    fn frame() -> ObjectFrame {
        ObjectFrame {
            handle: 1,
            definition: 2,
            nodes: Vec::new(),
            sphere: ([0.0; 3], 1.0),
            lighting: Some(Lighting { words: [0u32; 30], root: Some(0) }),
            defaults: Some(Default::default()),
        }
    }

    #[test]
    fn model_index_matches_the_first_part_in_each_section_and_is_rebuilt_per_model() {
        let p = |section, number| part(section, number, [None; IMPLEMENTATIONS]);
        let index = index_parts(&[p(0, 2), p(1, 2), p(0, 2)]);
        assert_eq!(index.get(&(0, 2)), Some(&0));
        assert_eq!(index.get(&(1, 2)), Some(&1));
        let replacement = index_parts(&[p(1, 2)]);
        assert_eq!(replacement.get(&(1, 2)), Some(&0));
        assert!(!replacement.contains_key(&(0, 2)));
    }

    fn key(section: u16, part: u8, implementation: usize) -> ItemKey {
        ItemKey { record: 9, shader: 0xe5da_0465, section, part, implementation }
    }

    #[test]
    fn an_item_draws_exactly_the_part_and_implementation_its_record_picked() {
        let passes = [equal_pass(0), equal_pass(1), equal_pass(2)];
        let pass_of = |i: usize| passes.get(i);
        // Section 4: part 0 with implementations 0 and 1, part 1 with 2;
        // section 7: part 0 with 0.
        let parts = [
            part(4, 0, [Some((0, 0)), Some((1, 0)), None, None, None]),
            part(4, 1, [None, None, Some((2, 1)), None, None]),
            part(7, 0, [Some((0, 2)), None, None, None, None]),
        ];
        let rows = [[1.0f32; 4]; 3];
        let f = frame();
        let d = plan_part(&parts, &pass_of, &f, &key(4, 1, 2), &rows, &entry()).unwrap();
        assert_eq!((d.part, d.implementation, d.program, d.buffer, d.indices.clone()), (1, 2, 2, 1, 3..6), "part 1 only, not its section's part 0");
        assert_eq!(d.vertex.constants[50], [1.0; 4], "its record's rows");
        let d = plan_part(&parts, &pass_of, &f, &key(4, 0, 1), &rows, &entry()).unwrap();
        assert_eq!((d.part, d.program), (0, 1));
        let refused = |k: ItemKey| plan_part(&parts, &pass_of, &f, &k, &rows, &entry()).unwrap_err();
        assert!(refused(key(4, 0, 2)).contains("no implementation 2"));
        assert!(refused(key(4, 2, 0)).contains("not a native part"));
        assert!(refused(key(5, 0, 0)).contains("not a native part"));
        assert!(refused(ItemKey { shader: 0xe000_0010, ..key(4, 0, 0) }).contains("shader"));
    }

    /// A first-person item draws the topology and subparts its record
    /// picks, as a scene part does; a marked or aux-depth record refuses.
    #[test]
    fn a_first_person_item_draws_the_topology_and_subparts_its_record_picks() {
        let passes = [equal_pass(0)];
        let pass_of = |i: usize| passes.get(i);
        let mut strip = part(4, 0, [Some((0, 0)), None, None, None, None]);
        strip.indices = 0..9;
        strip.listed = Some(9..12);
        strip.subparts = Some(SubpartRanges { first: 0, ranges: vec![12..15, 15..18], listed: None });
        let parts = [strip];
        let f = frame();
        let item = |flags, subparts| ItemDraw { index: 0, object: 0, key: key(4, 0, 0), rows: vec![[1.0f32; 4]; 3], flags, subparts };
        let plan = |item: &ItemDraw| plan_item_part(&parts, &pass_of, &f, item, &entry());
        assert_eq!(plan(&item(PartFlags::STRIP, None)).unwrap().ranges, [0..9], "a strip");
        assert_eq!(plan(&item(PartFlags::NONE, None)).unwrap().ranges, [9..12], "without bit 9, a list");
        let chosen = Subparts { first: 0, count: 2, visible: 0b10 };
        let flagged = PartFlags::STRIP | PartFlags::SUBPARTS;
        assert_eq!(plan(&item(flagged, Some(chosen))).unwrap().ranges, [15..18], "its chosen subpart");
        assert!(plan(&item(PartFlags::STRIP | PartFlags::STENCIL_MARK, None)).unwrap_err().contains("stencil"));
        assert!(plan(&item(PartFlags::STRIP | PartFlags::AUX_DEPTH, None)).unwrap_err().contains("auxiliary"));
    }

    #[test]
    fn a_parts_subparts_decode_alone_in_its_topology() {
        use tags::geometry::{PART_FLAG_TRIANGLE_LIST, Part, Subpart};
        let part = |flags, first_subpart, subpart_count| Part {
            kind: 1, flags, material: 0, first_index: 0, index_count: 9, first_subpart, subpart_count, raw: Vec::new(),
        };
        let sub = |first_index, index_count| Subpart { first_index, index_count, unknown_4: 0xffff, part: 0 };
        let indices = [0u16, 1, 2, 3, 4, 5, 6, 7, 8];
        let subparts = [sub(0, 0), sub(0, 5), sub(5, 4)];
        // A strip part: each subpart its own strip, winding restarting.
        let strip = lit_subparts(&indices, &subparts, &part(0, 1, 2)).unwrap();
        assert_eq!(strip.first, 1);
        assert_eq!(strip.triangles, [vec![[0, 1, 2], [1, 3, 2], [2, 3, 4]], vec![[5, 6, 7], [6, 8, 7]]]);
        // And as the list draw reads them: the indices they span, verbatim.
        let listed = strip.listed.unwrap();
        assert_eq!((listed.indices, listed.spans), (indices.to_vec(), vec![(0, 5), (5, 4)]));
        let later = lit_subparts(&indices, &[sub(6, 2), sub(2, 3)], &part(0, 0, 2)).unwrap().listed.unwrap();
        assert_eq!((later.indices, later.spans), (vec![2, 3, 4, 5, 6, 7], vec![(4, 2), (0, 3)]), "from the lowest");
        // A list part's subparts must be whole triangles.
        let whole = [sub(0, 3), sub(3, 6)];
        let list = lit_subparts(&indices, &whole, &part(PART_FLAG_TRIANGLE_LIST, 0, 2)).unwrap();
        assert_eq!(list.triangles, [vec![[0, 1, 2]], vec![[3, 4, 5], [6, 7, 8]]]);
        assert_eq!(list.listed, None, "a list part draws its own");
        assert!(lit_subparts(&indices, &subparts, &part(PART_FLAG_TRIANGLE_LIST, 1, 2)).is_none());
        // None, too many, or outside the indices.
        assert!(lit_subparts(&indices, &subparts, &part(0, 0, 0)).is_none());
        assert!(lit_subparts(&indices, &subparts, &part(0, 0, 65)).is_none());
        assert!(lit_subparts(&indices, &subparts, &part(0, 2, 2)).is_none());
        assert!(lit_subparts(&indices, &[sub(7, 4)], &part(0, 0, 1)).is_none());
    }

    /// A strip part whose record lacks bit 9 draws its indices as a list
    /// (`0x0004d5e2`): its whole triangles, a partial one dropped.
    #[test]
    fn a_strip_part_decodes_as_the_list_a_record_without_bit_9_draws() {
        use tags::geometry::{PART_FLAG_TRIANGLE_LIST, Part};
        let part = |flags, first_index, index_count| Part {
            kind: 1, flags, material: 0, first_index, index_count, first_subpart: 0, subpart_count: 0, raw: Vec::new(),
        };
        let indices = [9u16, 0, 1, 2, 3, 4, 5, 3, 3];
        assert_eq!(lit_listed(&indices, &part(0, 1, 8)), Some(vec![[0, 1, 2], [3, 4, 5]]));
        assert_eq!(lit_listed(&indices, &part(0, 1, 7)), Some(vec![[0, 1, 2], [3, 4, 5]]), "a partial triangle draws nothing");
        assert_eq!(lit_listed(&indices, &part(0, 3, 6)), Some(vec![[2, 3, 4]]), "a degenerate triangle dropped");
        assert_eq!(lit_listed(&indices, &part(PART_FLAG_TRIANGLE_LIST, 0, 9)), None, "a list part");
        assert_eq!(lit_listed(&indices, &part(0, 4, 9)), None, "outside the indices");
    }

    /// The ranges a part draws by the record's topology: a strip part
    /// drawn as a list takes its listed range, and its chosen subparts'
    /// listed runs; a list part and a strip drawn as a strip their own.
    #[test]
    fn a_part_draws_the_ranges_of_the_topology_its_record_picks() {
        let listed = ListedRanges { indices: 30..39, spans: vec![(0, 5), (5, 4)] };
        let subparts = SubpartRanges { first: 2, ranges: vec![10..13, 13..19], listed: Some(listed) };
        let chosen = Subparts { first: 2, count: 2, visible: 0b10 };
        let strip = |strip, chosen| part_ranges(&(0..9), Some(&subparts), Some(&(20..26)), strip, chosen);
        assert_eq!(strip(Some(true), None), Ok(vec![0..9]));
        assert_eq!(strip(Some(true), Some(chosen)), Ok(vec![13..19]));
        assert_eq!(strip(None, None), Ok(vec![0..9]), "in its own topology");
        assert_eq!(strip(Some(false), None), Ok(vec![20..26]), "as a list");
        assert_eq!(strip(Some(false), Some(chosen)), Ok(vec![35..38]), "its chosen subpart's whole triangles");
        let both = Subparts { visible: 0b11, ..chosen };
        assert_eq!(strip(Some(false), Some(both)), Ok(vec![30..39]), "two index-contiguous subparts, one list");
        let unlisted = SubpartRanges { listed: None, ..subparts.clone() };
        let refused = part_ranges(&(0..9), Some(&unlisted), Some(&(20..26)), Some(false), Some(chosen));
        assert!(refused.unwrap_err().contains("no list"));
        let list = |strip| part_ranges(&(0..9), None, None, strip, None);
        assert_eq!((list(Some(false)), list(None)), (Ok(vec![0..9]), Ok(vec![0..9])));
    }

    /// The emitter's list loop `0x0004db68` (`h2 disasm`), over a part's
    /// own subparts: per visible subpart in order, a run grows while the
    /// next starts one past the run's last index (`ebx + 1`), and each run
    /// is one `TRIANGLELIST` draw of its `ebx - esi + 1` indices, its
    /// `n / 3` triangles.
    fn guest_list_loop(indices: &[u16], subparts: &[tags::geometry::Subpart], visible: u64) -> Vec<[u16; 3]> {
        let mut out = Vec::new();
        let mut draw = |esi: i64, ebx: i64| {
            let run = &indices[esi as usize..(ebx + 1) as usize];
            out.extend(run.chunks_exact(3).map(|t| [t[0], t[1], t[2]]));
        };
        let (mut esi, mut ebx) = (-1i64, -1i64);
        for (n, sub) in subparts.iter().enumerate() {
            if visible >> n & 1 == 0 {
                continue;
            }
            let (first, count) = (i64::from(sub.first_index), i64::from(sub.index_count));
            if esi == -1 {
                (esi, ebx) = (first, first + count - 1);
            } else if first == ebx + 1 {
                ebx = first + count - 1;
            } else {
                draw(esi, ebx);
                (esi, ebx) = (first, first + count - 1);
            }
        }
        if esi != -1 {
            draw(esi, ebx);
        }
        out
    }

    /// A strip part's chosen subparts drawn by a record without bit 9:
    /// every choice gives the triangles the guest's list loop draws, a
    /// triangle spanning two joined subparts and a run's partial one left
    /// out, through the part's listed indices in the buffer.
    #[test]
    fn a_strip_parts_chosen_subparts_draw_the_triangles_the_list_loop_draws() {
        use tags::geometry::{Part, Subpart};
        let sub = |first_index, index_count| Subpart { first_index, index_count, unknown_4: 0xffff, part: 0 };
        // 0 and 1 contiguous (5 and 4 indices: a triangle spans them), 2
        // after a gap, 3 empty and contiguous with 2, 4 contiguous with 3.
        let subparts = [sub(1, 5), sub(6, 4), sub(11, 4), sub(15, 0), sub(15, 3)];
        let indices: Vec<u16> = (0..18).map(|i| 100 + i).collect();
        let part = Part {
            kind: 1, flags: 0, material: 0, first_index: 1, index_count: 17, first_subpart: 0, subpart_count: 5, raw: Vec::new(),
        };
        let lit = lit_subparts(&indices, &subparts, &part).unwrap();
        let listed = lit.listed.clone().unwrap();
        // The buffer as the lit build lays it out: other indices first.
        let base = 7u32;
        let mut buffer: Vec<u16> = vec![0; base as usize];
        buffer.extend(&listed.indices);
        let ranges = SubpartRanges {
            first: 0,
            ranges: vec![0..0; 5],
            listed: Some(ListedRanges { indices: base..buffer.len() as u32, spans: listed.spans }),
        };
        for visible in 0..32u64 {
            let chosen = Subparts { first: 0, count: 5, visible };
            let drawn = ranges.drawn_as_list(chosen).unwrap();
            let native: Vec<[u16; 3]> = drawn
                .iter()
                .flat_map(|r| buffer[r.start as usize..r.end as usize].chunks_exact(3).map(|t| [t[0], t[1], t[2]]))
                .collect();
            assert_eq!(native, guest_list_loop(&indices, &subparts, visible), "visible {visible:#07b}");
            assert!(drawn.iter().all(|r| r.len() % 3 == 0 && !r.is_empty()));
        }
        assert!(ranges.drawn_as_list(Subparts { first: 1, count: 5, visible: 1 }).is_err(), "another part's subparts");
    }

    #[test]
    fn an_auxiliary_depth_draw_takes_the_toggles_rows_and_clip_and_nothing_else() {
        let base = VertexUniforms::default();
        let view = ViewInputs {
            constants: std::array::from_fn(|i| [i as f32; 4]),
            guest_size: (640, 480),
            size: (640, 480),
            z_scale: 1000.0,
            w_clip: Some(([2.0, 900.0], true)),
        };
        let mut vertex = frame_vertex(&base, &view);
        let mut pixel = frame_pixel(&PixelUniforms::default(), &view);
        let (before_v, before_p) = (vertex.clone(), pixel);
        let aux = AuxDepth {
            scale: 0.5,
            rows: [[9.0; 4], [8.0; 4], [7.0; 4], [6.0; 4]],
            clip: [1.0, 900.0],
            rest_rows: [[0.0; 4], [1.0; 4], [2.0; 4], [3.0; 4]],
            rest_clip: [2.0, 900.0],
        };
        with_aux_depth(&mut vertex, &mut pixel, &aux);
        assert_eq!(vertex.constants[..4], aux.rows);
        assert_eq!(vertex.constants[4..], before_v.constants[4..], "the view's other constants");
        assert_eq!((vertex.w_clip, vertex.w_clamp), (Some([1.0, 900.0]), true));
        assert_eq!(pixel.depth_range, [1.0, 900.0]);
        assert_eq!(pixel.depth, before_p.depth, "the W-buffer's scale is the view's");
    }

    /// An additive implementation `index` of the lit fixture (One, One,
    /// no depth writes), as a world layer's pass compiles it.
    fn additive(index: usize) -> tags::material::ResolvedImplementation {
        use crate::object_program::tests::lit;
        let mut imp = lit();
        imp.index = index;
        imp.render_states = vec![
            state(RenderStateType::AlphaBlendEnable, 1),
            state(RenderStateType::SrcBlend, gfx::state::BlendFactor::One.xbox_value()),
            state(RenderStateType::DestBlend, gfx::state::BlendFactor::One.xbox_value()),
            state(RenderStateType::ZWriteEnable, 0),
        ];
        imp
    }

    #[test]
    fn a_materials_world_layer_passes_compile_over_its_layer_3_buffers() {
        use crate::object_program::tests::{lit, rigid_program};
        use tags::material::{ResolvedLayer, ResolvedPass, StageTexture, TextureSource};
        let mut bytes = Vec::new();
        for k in 0..3 {
            for v in [k as f32, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0] {
                bytes.extend(v.to_le_bytes());
            }
        }
        let section = LitSection {
            index: 4,
            geometry: 0,
            influences: 0,
            decompression: [[1.0; 4]; 3],
            streams: vec![Some((1, StreamData { bytes: &bytes, stride: 32 }))],
            vertex_count: 3,
            parts: vec![(0, 0, vec![[0, 1, 2]])],
            subparts: HashMap::new(),
            listed: HashMap::new(),
        };
        let mut opaque = lit();
        opaque.render_states = vec![state(RenderStateType::AlphaBlendEnable, 0), state(RenderStateType::ZWriteEnable, 1)];
        // Implementation 1 of layer 7's first pass has no vertex program:
        // it draws nothing.
        let mut nothing = additive(1);
        nothing.vertex_shader = None;
        // Layer 8's samples the lightmap, which the draw binds only in
        // layer 3.
        let mut lightmapped = additive(0);
        lightmapped.textures[3] = Some(StageTexture { source: TextureSource::Lightmap, size: None, stage_size: None });
        lightmapped.pixel_shader = shaders::PixelShaderDef { texture_modes: 1 << 15, ..Default::default() }.to_bytes();
        let pass = |index: usize, implementations| ResolvedPass { index, pass: map::TagId(0xe000_0001), implementations };
        let material = tags::material::Material {
            shader: map::TagId(0xe000_0010),
            template: map::TagId(0xe000_0011),
            inputs: Default::default(),
            layers: vec![
                ResolvedLayer { lod: 0, layer: crate::look::LIGHTMAP_LAYER, passes: vec![pass(0, vec![opaque])] },
                ResolvedLayer { lod: 0, layer: 7, passes: vec![pass(0, vec![additive(0), nothing]), pass(1, vec![additive(0)])] },
                ResolvedLayer { lod: 0, layer: 8, passes: vec![pass(0, vec![lightmapped])] },
            ],
        };
        let mut materials = HashMap::new();
        materials.insert(0, LitMaterial { material, shader: ShaderValues::default() });
        let mut vertex = |_: map::TagId, _: u16, _: u8| Ok::<_, String>((rigid_program(), vec![0u16, 3, 2]));
        let mut image = |_: usize, _: map::TagId, _: u32| -> Option<Image> { None };
        let build = lit_model(&[section], &materials, &formats(), &EngineStages::default(), &mut vertex, &mut image);
        assert_eq!(build.taken, vec![0], "{:?}", build.refused);
        assert_eq!(build.input.layers.len(), 1, "layer 7's part; layer 8's does not compile");
        let p = &build.input.layers[0];
        assert_eq!((p.layer, p.section, p.part, p.indices.clone(), p.shader_tag), (7, 4, 0, 0..3, 0xe000_0010));
        assert_eq!(p.passes.len(), 2, "each pass of the layer, in order");
        assert_eq!((p.passes[0].count, p.passes[0].empty), (2, 0b10));
        assert_eq!(p.passes[0].implementations[..2], [Some((1, 0)), None], "after layer 3's program, over its buffer");
        assert_eq!((p.passes[1].count, p.passes[1].implementations[0]), (1, Some((2, 0))));
        assert_eq!(build.input.buffers.len(), 1, "the same declaration: one buffer");
        assert!(build.input.programs[1].pass.program.state.as_ref().unwrap().blend.is_some(), "its blend kept");
        assert!(build.input.programs[0].pass.program.state.as_ref().unwrap().blend.is_none());
        assert_eq!(build.layer_refused.len(), 1, "{:?}", build.layer_refused);
        let (m, layer, why) = &build.layer_refused[0];
        assert_eq!((*m, *layer), (0, 8));
        assert!(why.contains("only in layer 3"), "{why}");
        assert!(build.input.valid_draw_references());
    }

    #[test]
    fn an_override_shaders_camouflage_passes_compile_over_every_part_after_the_models_own() {
        use crate::object_program::VIEW_TEXTURE;
        use crate::object_program::tests::{lit, rigid_program};
        use tags::material::{ResolvedLayer, ResolvedPass, StageTexture, TextureSource};
        let mut bytes = Vec::new();
        for k in 0..3 {
            for v in [k as f32, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0] {
                bytes.extend(v.to_le_bytes());
            }
        }
        let section = LitSection {
            index: 4,
            geometry: 0,
            influences: 0,
            decompression: [[1.0; 4]; 3],
            streams: vec![Some((1, StreamData { bytes: &bytes, stride: 32 }))],
            vertex_count: 3,
            parts: vec![(0, 0, vec![[0, 1, 2]]), (1, 0, vec![[2, 1, 0]])],
            subparts: HashMap::new(),
            listed: HashMap::new(),
        };
        // The parts' own material: layer 3 only.
        let mut opaque = lit();
        opaque.render_states = vec![state(RenderStateType::AlphaBlendEnable, 0), state(RenderStateType::ZWriteEnable, 1)];
        let mut materials = HashMap::new();
        materials.insert(0, LitMaterial { material: material(vec![opaque]), shader: ShaderValues::default() });
        let mut vertex = |_: map::TagId, _: u16, _: u8| Ok::<_, String>((rigid_program(), vec![0u16, 3, 2]));
        let mut image = |_: usize, _: map::TagId, _: u32| -> Option<Image> { None };
        let engine = EngineStages { view: true, ..EngineStages::default() };
        let mut model = lit_model(std::slice::from_ref(&section), &materials, &formats(), &engine, &mut vertex, &mut image);
        assert_eq!(model.taken, vec![0], "{:?}", model.refused);
        // A camouflage shader: one additive pass sampling the view at
        // stage 1, in layers 18 and 19 and again in layer 7.
        let mut camo = additive(0);
        camo.textures[1] = Some(StageTexture { source: TextureSource::Engine(VIEW_TEXTURE), size: None, stage_size: None });
        camo.pixel_shader = shaders::PixelShaderDef { texture_modes: 1 << 5, ..Default::default() }.to_bytes();
        let pass = |implementations| ResolvedPass { index: 0, pass: map::TagId(0xe000_0001), implementations };
        let shader = map::TagId(0xe000_0020);
        let camouflage = tags::material::Material {
            shader,
            template: map::TagId(0xe000_0021),
            inputs: Default::default(),
            layers: vec![
                ResolvedLayer { lod: 0, layer: usize::from(CAMO_LAYER), passes: vec![pass(vec![camo.clone()])] },
                ResolvedLayer { lod: 0, layer: usize::from(CAMO_MODULATE_LAYER), passes: vec![pass(vec![camo.clone()])] },
                ResolvedLayer { lod: 0, layer: 7, passes: vec![pass(vec![camo])] },
            ],
        };
        let overrides = HashMap::from([(OVERRIDE_MATERIAL, LitMaterial { material: camouflage, shader: ShaderValues::default() })]);
        let sections = override_sections(std::slice::from_ref(&section));
        assert!(sections[0].parts.iter().all(|(_, m, _)| *m == OVERRIDE_MATERIAL));
        let built = lit_model(&sections, &overrides, &formats(), &engine, &mut vertex, &mut image);
        assert!(built.taken.is_empty(), "it has no layer 3");
        assert_eq!(built.input.layers.len(), 4, "layers 18 and 19 over both parts");
        assert!(built.input.layers.iter().all(|p| p.shader_tag == shader.0));
        for layer in OVERRIDE_LAYERS {
            let parts: Vec<u8> = built.input.layers.iter().filter(|p| p.layer == layer).map(|p| p.part).collect();
            assert_eq!(parts, [0, 1], "layer {layer}");
        }
        let refused: Vec<(u8, &str)> = built.layer_refused.iter().map(|(_, l, why)| (*l, why.as_str())).collect();
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0].0, 7, "the view is copied only for the sorted handlers' layers");
        assert!(refused[0].1.contains("view's own target"), "{}", refused[0].1);
        let camo_program = built.input.layers[0].passes[0].implementations[0].unwrap().0;
        assert_eq!(built.input.programs[camo_program].pass.view, [false, true, false, false]);
        assert_eq!(built.input.programs[camo_program].images[1], None, "the copy is bound per encode");
        let (programs, buffers) = (model.input.programs.len(), model.input.buffers.len());
        let theirs = built.input.layers[0].passes[0].implementations[0].unwrap();
        model.input.append(built.input);
        assert_eq!(model.input.layers.len(), 4);
        assert_eq!(model.input.parts.len(), 1, "the model's layer-3 part alone");
        assert_eq!(
            model.input.layers[0].passes[0].implementations[0],
            Some((theirs.0 + programs, theirs.1 + buffers)),
            "moved past the model's programs and buffers"
        );
        assert!(model.input.valid_draw_references());
        // Without the host's copy the camouflage pass does not compile.
        let without = lit_model(&sections, &overrides, &formats(), &EngineStages::default(), &mut vertex, &mut image);
        assert!(without.input.layers.is_empty());
        assert!(without.layer_refused.iter().any(|(_, l, _)| *l == CAMO_LAYER));
        assert!(without.layer_refused.iter().any(|(_, l, _)| *l == CAMO_MODULATE_LAYER));
    }

    /// [`formats`] with a PRT stream record: semantic 2 (the normal) as
    /// three normalised shorts, then four bytes skipped (10 a vertex).
    fn prt_formats() -> StreamFormats {
        let size = crate::object_streams::STREAM_FORMAT_SIZE;
        let base = formats();
        let mut t = vec![0u8; (usize::from(PRT_STREAM) + 1) * size];
        t[size..size + 8].copy_from_slice(&[1, 0, 2, 3, 1, 2, 2, 0xff]);
        let l = usize::from(LIGHTMAP_STREAM) * size;
        t[l..l + 4].copy_from_slice(&[LIGHTMAP_STREAM, 7, 1, 0xff]);
        let p = usize::from(PRT_STREAM) * size;
        t[p..p + 6].copy_from_slice(&[PRT_STREAM, 2, 14, 0xfe, 4, 0xff]);
        let f = StreamFormats::from_bytes(&t);
        assert_eq!(f.elements(1), base.elements(1));
        f
    }

    #[test]
    fn layer_5_compiles_a_prt_variant_whose_stream_feeds_its_registers() {
        use crate::object_program::tests::rigid_program;
        use tags::material::{ResolvedLayer, ResolvedPass};
        let mut bytes = Vec::new();
        for k in 0..3 {
            for v in [k as f32, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0] {
                bytes.extend(v.to_le_bytes());
            }
        }
        let section = LitSection {
            index: 4,
            geometry: 0,
            influences: 0,
            decompression: [[1.0; 4]; 3],
            streams: vec![Some((1, StreamData { bytes: &bytes, stride: 32 }))],
            vertex_count: 3,
            parts: vec![(0, 0, vec![[0, 1, 2]])],
            subparts: HashMap::new(),
            listed: HashMap::new(),
        };
        let mut opaque = crate::object_program::tests::lit();
        opaque.render_states = vec![state(RenderStateType::AlphaBlendEnable, 0), state(RenderStateType::ZWriteEnable, 1)];
        let pass = |implementations| ResolvedPass { index: 0, pass: map::TagId(0xe000_0001), implementations };
        let material = tags::material::Material {
            shader: map::TagId(0xe000_0010),
            template: map::TagId(0xe000_0011),
            inputs: Default::default(),
            layers: vec![
                ResolvedLayer { lod: 0, layer: crate::look::LIGHTMAP_LAYER, passes: vec![pass(vec![opaque])] },
                ResolvedLayer { lod: 0, layer: usize::from(PRT_LAYER), passes: vec![pass(vec![additive(0)])] },
            ],
        };
        let mut materials = HashMap::new();
        materials.insert(0, LitMaterial { material, shader: ShaderValues::default() });
        let mut vertex = |_: map::TagId, _: u16, _: u8| Ok::<_, String>((rigid_program(), vec![0u16, 3, 2]));
        let mut image = |_: usize, _: map::TagId, _: u32| -> Option<Image> { None };
        let build = lit_model(std::slice::from_ref(&section), &materials, &prt_formats(), &EngineStages::default(), &mut vertex, &mut image);
        assert!(build.layer_refused.is_empty(), "{:?}", build.layer_refused);
        let p = &build.input.layers[0];
        assert_eq!(p.layer, PRT_LAYER);
        assert_eq!(p.passes[0].implementations[0], Some((1, 0)), "the plain variant over layer 3's buffer");
        assert_eq!(p.passes[0].prt[0], Some((2, 1)), "the PRT variant over the section's registers only");
        assert_eq!(build.input.buffers[1].registers, [0, 1], "the PRT stream feeds v2");
        let prt = build.input.programs[2].pass.prt.as_ref().expect("the PRT variant's stream");
        assert_eq!(prt.registers, [2]);
        assert_eq!(prt.provided & 0b111, 0b111);
        assert_eq!(prt.last_constant, None);
        assert!(build.input.programs[1].pass.prt.is_none());
        assert!(build.input.valid_draw_references());
        // Without a PRT record the plain variant still compiles; the PRT
        // one is named.
        let build = lit_model(&[section], &materials, &formats(), &EngineStages::default(), &mut vertex, &mut image);
        assert_eq!(build.input.layers[0].passes[0].prt[0], None);
        assert!(build.layer_refused.iter().any(|(_, l, why)| *l == PRT_LAYER && why.contains("PRT")), "{:?}", build.layer_refused);
    }

    #[test]
    fn a_prt_item_draws_its_passes_prt_variant_with_its_constants() {
        use crate::object_program::tests::rigid_program;
        use frame_scene::{InstanceKey, LayerInputs, PrtStream};
        let plain = object_layer_pass(map::TagId(0xe000_0001), &additive(0), &rigid_program(), crate::program::INPUT_MASK, [shaders::vsh::InputFetch::Float; 16])
            .unwrap_or_else(|e| panic!("{e}"));
        let mut with_prt = plain.clone();
        with_prt.prt = Some(PrtInput {
            semantics: vec![0, 3, 2],
            streams: vec![],
            registers: vec![2],
            provided: crate::program::INPUT_MASK,
            fetch: [shaders::vsh::InputFetch::Float; 16],
            last_constant: Some(70),
        });
        let passes = [plain, with_prt];
        let pass_of = |i: usize| passes.get(i);
        let layers = [LitLayerPart {
            layer: PRT_LAYER,
            section: 4,
            part: 0,
            decompression: [[0.0; 4]; 3],
            shader_tag: 0xe5da_0465,
            shader: ShaderValues::default(),
            indices: 0..3,
            subparts: None,
            listed: None,
            passes: vec![LitLayerPass {
                count: 1,
                implementations: [Some((0, 0)), None, None, None, None],
                empty: 0,
                prt: [Some((1, 1)), None, None, None, None],
            }],
        }];
        let pass = LayerPass {
            layer: PRT_LAYER,
            stencil_off: false,
            texaccum: false,
            items: 0..1,
            remainder: 0,
            inputs: LayerInputs { defaults: Some(Defaults::default()), ..LayerInputs::default() },
        };
        let constants: Vec<[f32; 4]> = (0..3).map(|n| [n as f32 + 1.0; 4]).collect();
        let stream = PrtStream { kind: PRT_STREAM, stride: 10, data: 0x8000_1000, constants };
        let item = LayerItem {
            slot: 3,
            key: key(4, 0, 0),
            owner: LayerOwner::Object(InstanceKey(1)),
            model: 0xe100_0001,
            rows: 0..3,
            lighting: Some(Lighting { words: [0u32; 30], root: Some(0) }),
            flags: PartFlags::NONE,
            subparts: None,
            refusal: None,
            prt: Some(stream.clone()),
        };
        let rows = [[1.0f32; 4]; 3];
        // The device's blend op reads (the additive pass inherits it).
        let mut e = entry();
        e.0.retain(|(t, _)| *t != RenderStateType::BlendOp);
        e.0.push((RenderStateType::BlendOp, 0x8006));
        let plan = |item: &LayerItem| plan_layer_part(&layers, Some(0), &pass_of, InstanceKey(1), PRT_LAYER, &rows, &pass, item, &e);
        let d = plan(&item).unwrap();
        assert_eq!((d[0].program, d[0].buffer), (1, 1), "the PRT variant");
        assert_eq!(d[0].vertex.constants[68..71], [[1.0; 4], [2.0; 4], [3.0; 4]]);
        assert_eq!(d[0].prt, Some((stream.clone(), None)), "its stream still to gather");
        let plain = LayerItem { prt: None, ..item.clone() };
        assert_eq!(plan(&plain).unwrap()[0].program, 0, "without a stream, the plain variant");
        // The program reads c[70]: two constants do not reach it.
        let short = LayerItem { prt: Some(PrtStream { constants: vec![[0.0; 4]; 2], ..stream }), ..item };
        assert!(plan(&short).unwrap_err().contains("c[70]"));
    }

    #[test]
    fn a_world_layers_object_item_draws_each_pass_with_the_implementation_it_picks() {
        use crate::object_program::tests::rigid_program;
        use frame_scene::{InstanceKey, LayerInputs, LayerObjectRefusal};
        let compile = |index| {
            object_layer_pass(map::TagId(0xe000_0001), &additive(index), &rigid_program(), crate::program::INPUT_MASK, [shaders::vsh::InputFetch::Float; 16])
                .unwrap_or_else(|e| panic!("{e}"))
        };
        let passes = [compile(0), compile(1)];
        let pass_of = |i: usize| passes.get(i);
        let part = |layer: u8| LitLayerPart {
            layer,
            section: 4,
            part: 0,
            decompression: [[0.0; 4]; 3],
            shader_tag: 0xe5da_0465,
            shader: ShaderValues::default(),
            indices: 0..3,
            subparts: None,
            listed: None,
            passes: vec![
                // One implementation: every item draws it.
                LitLayerPass { count: 1, implementations: [Some((0, 0)), None, None, None, None], empty: 0, prt: [None; IMPLEMENTATIONS] },
                // The item's: 0 or 1; 2 draws nothing.
                LitLayerPass { count: 3, implementations: [Some((0, 0)), Some((1, 0)), None, None, None], empty: 0b100, prt: [None; IMPLEMENTATIONS] },
            ],
        };
        let layers = [part(7), part(8)];
        let layer_pass = |layer: u8, stencil_off: bool| LayerPass {
            layer,
            stencil_off,
            texaccum: false,
            items: 0..1,
            remainder: 0,
            inputs: LayerInputs { defaults: Some(Defaults::default()), ..LayerInputs::default() },
        };
        let item = |implementation: usize| LayerItem {
            slot: 3,
            key: key(4, 0, implementation),
            owner: LayerOwner::Object(InstanceKey(1)),
            model: 0xe100_0001,
            rows: 0..3,
            lighting: Some(Lighting { words: [0u32; 30], root: Some(0) }),
            flags: PartFlags::NONE,
            subparts: None,
            refusal: None,
            prt: None,
        };
        let rows = [[1.0f32; 4]; 3];
        // The device's blend op reads (the passes inherit it).
        let mut e = entry();
        e.0.retain(|(t, _)| *t != RenderStateType::BlendOp);
        e.0.push((RenderStateType::BlendOp, 0x8006));
        let plan = |at: Option<usize>, layer: u8, pass: &LayerPass, item: &LayerItem, e: &EntryStates| {
            plan_layer_part(&layers, at, &pass_of, InstanceKey(1), layer, &rows, pass, item, e)
        };
        let seven = layer_pass(7, false);
        let d = plan(Some(0), 7, &seven, &item(1), &e).unwrap();
        assert_eq!(d.iter().map(|d| (d.implementation, d.program)).collect::<Vec<_>>(), [(0, 0), (1, 1)], "the only one, then the item's");
        assert!(d.iter().all(|d| d.state.blend.is_some() && !d.state.depth_write && d.ranges == std::slice::from_ref(&(0..3))));
        assert_eq!(d[0].vertex.constants[50], [1.0; 4], "its record's rows");
        assert_eq!(plan(Some(0), 7, &seven, &item(2), &e).unwrap().len(), 1, "implementation 2 of the second pass draws nothing");
        assert!(plan(Some(0), 7, &seven, &item(3), &e).unwrap_err().contains("no native implementation 3"));
        assert!(plan(None, 7, &seven, &item(0), &e).unwrap_err().contains("draws nothing native in layer 7"));
        assert!(plan(Some(1), 7, &seven, &item(0), &e).unwrap_err().contains("draws nothing native"), "layer 8's part");
        let other = LayerItem { key: ItemKey { shader: 0xe000_0010, ..key(4, 0, 0) }, ..item(0) };
        assert!(plan(Some(0), 7, &seven, &other, &e).unwrap_err().contains("shader"));
        let refused = LayerItem { refusal: Some(LayerObjectRefusal::Fade), ..item(0) };
        assert_eq!(plan(Some(0), 7, &seven, &refused, &e).unwrap_err(), "object_fade", "extraction's cause");
        let prt = frame_scene::PrtStream { kind: 0x39, stride: 10, data: 0x8000_0000, constants: vec![[0.0; 4]; 2] };
        let streamed = LayerItem { prt: Some(prt), ..item(0) };
        assert!(plan(Some(0), 7, &seven, &streamed, &e).unwrap_err().contains("PRT stream"));
        let aux = LayerItem { flags: PartFlags::AUX_DEPTH, ..item(0) };
        assert!(plan(Some(0), 7, &seven, &aux, &e).unwrap_err().contains("auxiliary"));
        let no_defaults = LayerPass { inputs: LayerInputs::default(), ..layer_pass(7, false) };
        assert!(plan(Some(0), 7, &no_defaults, &item(0), &e).unwrap_err().contains("defaults"));
        // A marked item draws in mode 2 unless the stencil is off.
        let marked = LayerItem { flags: PartFlags::STENCIL_MARK, ..item(0) };
        let d = plan(Some(0), 7, &seven, &marked, &e).unwrap();
        assert!(d.iter().all(|d| d.state.stencil.reference == object_state::STENCIL_MARK_REFERENCE));
        let d = plan(Some(0), 7, &layer_pass(7, true), &marked, &e).unwrap();
        assert!(d.iter().all(|d| !d.state.stencil.enable));
        // With the stencil on at the call and its fields unread, layer 7
        // inherits it and is refused; layer 1 with the stencil off draws
        // in mode 0, as the prologue sets it.
        let mut stenciled = e.clone();
        stenciled.0.retain(|(t, _)| *t != RenderStateType::StencilEnable);
        stenciled.0.push((RenderStateType::StencilEnable, 1));
        assert!(plan(Some(0), 7, &seven, &item(0), &stenciled).is_err());
        let layer_1 = [part(1)];
        let d = plan_layer_part(&layer_1, Some(0), &pass_of, InstanceKey(1), 1, &rows, &layer_pass(1, true), &item(0), &stenciled).unwrap();
        assert!(d.iter().all(|d| !d.state.stencil.enable));
    }

    #[test]
    fn chosen_subparts_draw_their_ranges_joined_where_adjacent() {
        let sub = SubpartRanges { first: 4, ranges: vec![10..13, 13..19, 19..19, 19..22, 22..28], listed: None };
        let chosen = |visible| Subparts { first: 4, count: 5, visible };
        assert_eq!(
            sub.drawn(chosen(0b1_1111)).unwrap().as_slice(),
            std::slice::from_ref(&(10..28)),
            "all, as one run"
        );
        assert_eq!(sub.drawn(chosen(0b1_1001)).unwrap(), [10..13, 19..28], "the empty one joins its neighbours");
        assert_eq!(
            sub.drawn(chosen(0b0_0010)).unwrap().as_slice(),
            std::slice::from_ref(&(13..19))
        );
        assert!(sub.drawn(chosen(0)).unwrap().is_empty(), "none chosen: nothing drawn");
        assert!(sub.drawn(Subparts { first: 5, ..chosen(1) }).is_err(), "another part's subparts");
        assert!(sub.drawn(Subparts { count: 4, ..chosen(1) }).is_err());
    }

    #[test]
    fn an_item_choosing_subparts_draws_them_and_otherwise_the_whole_part() {
        let passes = [equal_pass(0)];
        let pass_of = |i: usize| passes.get(i).map(|p| (p, None::<&InheritedState>));
        let mut with = part(4, 1, [Some((0, 0)), None, None, None, None]);
        with.subparts = Some(SubpartRanges { first: 2, ranges: vec![6..9, 9..12], listed: None });
        let without = part(4, 0, [Some((0, 0)), None, None, None, None]);
        let parts = [without, with];
        let f = frame();
        let rows = [[1.0f32; 4]; 3];
        let inputs = |subparts| PartInputs { subparts, ..PartInputs::of(&f) };
        let plan = |part: usize, subparts: Option<Subparts>| plan_indexed_part(&parts, Some(part), &pass_of, inputs(subparts), &key(4, part as u8, 0), &rows, &entry(), true);
        let whole = plan(1, None).unwrap();
        assert_eq!(whole.instance, frame_scene::InstanceKey(f.handle));
        assert_eq!(whole.indices, 3..6);
        assert_eq!(whole.ranges.as_slice(), std::slice::from_ref(&whole.indices));
        let chosen = plan(1, Some(Subparts { first: 2, count: 2, visible: 0b10 })).unwrap();
        assert_eq!(chosen.indices, 3..6);
        assert_eq!(chosen.ranges.as_slice(), std::slice::from_ref(&(9..12)));
        let refused = plan(0, Some(Subparts { first: 0, count: 1, visible: 1 })).unwrap_err();
        assert!(refused.contains("subparts"), "{refused}");
    }

    #[test]
    fn model_upload_checks_all_geometry_and_material_references() {
        let mut input = LitModelInput {
            parts: vec![part(0, 0, [Some((0, 0)), None, None, None, None])],
            programs: vec![LitProgramInput { pass: equal_pass(0), images: Default::default() }],
            buffers: vec![LitBufferInput { indices: vec![0, 1, 2], ..Default::default() }],
            layers: Vec::new(),
        };
        assert!(input.valid_draw_references());
        input.parts[0].implementations[0] = Some((0, 1));
        assert!(!input.valid_draw_references());
        input.parts[0].implementations[0] = Some((1, 0));
        assert!(!input.valid_draw_references());
        input.parts[0].implementations[0] = Some((0, 0));
        input.parts[0].subparts = Some(SubpartRanges { first: 0, ranges: std::iter::once(1..4).collect(), listed: None });
        assert!(!input.valid_draw_references());
        input.parts[0].subparts = None;
        input.parts[0].indices = 2..4;
        assert!(!input.valid_draw_references());
    }

    /// The map loader uploads every object definition in the tag index:
    /// programs equal across models and implementations must share their
    /// module, layout, samplers and stage textures, or a map's load makes
    /// thousands of each (lab 0076, Ascension: 9k textures, 12k samplers,
    /// 3.6k modules and layouts).
    #[test]
    fn equal_programs_share_their_gpu_resources_across_models() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let mut objects = LitObjects::new(&ctx.device);
        let bitmap = |n: u32| Stage::Bitmap { bitmap: map::TagId(0xe000_0100 + n), image: 0 };
        // Stage 0 samples `stage`: the layout binds it, which the module
        // need not read.
        let program = |stage: Stage, image: Option<Image>| {
            let mut pass = equal_pass(0);
            pass.program.dims[0] = Some(shaders::psh::TextureDim::D2);
            pass.program.stages[0] = stage;
            LitProgramInput { pass, images: [image, None, None, None] }
        };
        let solid = || Some(Image::solid([200, 100, 50, 255]));
        let model = |programs: Vec<LitProgramInput>| {
            let pass = equal_pass(0);
            let registers: Vec<_> = (0u8..16).filter(|i| pass.program.inputs & (1u16 << i) != 0).collect();
            LitModelInput {
                parts: vec![part(0, 0, [Some((0, 0)), None, None, None, None])],
                buffers: vec![LitBufferInput {
                    vertices: vec![[0; SLOT]; 3 * registers.len()],
                    formats: vec![Attribute::Float(4); registers.len()],
                    registers,
                    indices: vec![0, 1, 2],
                }],
                programs,
                layers: Vec::new(),
            }
        };
        let (device, queue) = (&ctx.device, &ctx.queue);
        assert!(objects.add_model(device, queue, 1, model(vec![program(bitmap(0), solid()), program(bitmap(0), solid())])));
        let first = objects.created();
        assert_eq!((first.shader_modules, first.layouts, first.textures), (1, 1, 1), "one model's two equal programs share: {first:?}");
        for index in 2..40 {
            assert!(objects.add_model(device, queue, index, model(vec![program(bitmap(0), solid())])));
        }
        assert_eq!(objects.created(), first, "equal programs of other models make nothing");
        let texture = |objects: &LitObjects, index: u32| objects.models[&index].programs[0].views[0].1.texture().clone();
        assert_eq!(texture(&objects, 1), texture(&objects, 39));
        assert_eq!(objects.image_stages(map::TagId(0xe000_0100), 0), vec![0]);

        // Another bitmap is another texture, and only that.
        assert!(objects.add_model(device, queue, 40, model(vec![program(bitmap(1), solid())])));
        assert_eq!(objects.created(), Created { textures: first.textures + 1, ..first });
        assert_ne!(texture(&objects, 1), texture(&objects, 40));

        // A missing image's black texel is never kept for its stage: the
        // real image after it gets a texture of its own, which is.
        assert!(objects.add_model(device, queue, 41, model(vec![program(bitmap(2), None)])));
        assert!(objects.add_model(device, queue, 42, model(vec![program(bitmap(2), solid())])));
        assert!(objects.add_model(device, queue, 43, model(vec![program(bitmap(2), solid())])));
        assert_eq!(objects.created().textures, first.textures + 3);
        assert_ne!(texture(&objects, 41), texture(&objects, 42));
        assert_eq!(texture(&objects, 42), texture(&objects, 43));

        // A staging set made for a refresh shares nothing with the draw
        // set: an edited model's images upload again.
        let mut staging = objects.empty_with_layout();
        assert!(staging.add_model(device, queue, 1, model(vec![program(bitmap(0), solid())])));
        assert_eq!(staging.created().textures, 1);
        assert!(staging.move_model_into(1, &mut objects));
        assert_ne!(texture(&objects, 1), texture(&objects, 2));
    }

    #[test]
    fn object_draws_reuse_their_pipeline_and_bindings_across_swaps_and_poses() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let mut objects = LitObjects::new(&ctx.device);
        let pass = equal_pass(0);
        let registers: Vec<_> = (0u8..16).filter(|i| pass.program.inputs & (1u16 << i) != 0).collect();
        let buffer = LitBufferInput {
            vertices: vec![[0; SLOT]; 3 * registers.len()],
            formats: vec![Attribute::Float(4); registers.len()],
            registers, indices: vec![0, 1, 2],
        };
        objects.add_model(&ctx.device, &ctx.queue, 7, LitModelInput {
            programs: vec![LitProgramInput { pass, images: Default::default() }],
            buffers: vec![buffer],
            parts: vec![part(0, 0, [Some((0, 0)), None, None, None, None])],
            layers: Vec::new(),
        });
        let invalid = LitModelInput {
            parts: vec![part(0, 0, [Some((0, 0)), None, None, None, None])],
            ..Default::default()
        };
        assert!(!objects.add_model(&ctx.device, &ctx.queue, 7, invalid), "failed replacement reports its own result");
        let mut requested = objects.plan(7, &frame(), &key(0, 0, 0), &[[1.0; 4]; 3], &entry()).unwrap();
        assert!(objects.has_draw_assets(7, &requested));
        assert!(!objects.has_draw_assets(8, &requested));
        requested.buffer = 1;
        assert!(!objects.has_draw_assets(7, &requested));
        requested.buffer = 0;
        requested.program = 1;
        assert!(!objects.has_draw_assets(7, &requested));
        let pool = gfx::ring::UploadPool::default();
        let view = ViewInputs { constants: Default::default(), guest_size: (16, 16), size: (16, 16),
            z_scale: 1.0, w_clip: None };
        let mut record = |translate| {
            let arena = pool.begin();
            let rows = [[translate; 4]; 3];
            let draw = objects.plan(7, &frame(), &key(0, 0, 0), &rows, &entry()).unwrap();
            let color = wgpu::TextureFormat::Rgba8Unorm;
            let depth = wgpu::TextureFormat::Depth24PlusStencil8;
            let pending = objects.request_draw(&ctx.device, color, depth, 7, &draw).unwrap().0;
            assert!(pending.get().is_some());
            timeline::frame();
            assert!(objects.prewarm_draws(&ctx.device, color, depth, &[(7, draw.clone())]));
            let packets = objects.draws(&ctx.device, &ctx.queue, &arena, wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Depth24PlusStencil8, view.size, &view, 1.0, &[(7, draw)], None).unwrap();
            arena.flush(&ctx.queue);
            ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            assert_eq!((packets.ranges, packets.packets.len()), (1, 1));
            assert!(packets.packets[0].pipeline_valid());
            assert!(!packets.packets[0].starts_pass(), "appended to the open pass");
            objects.models[&7].programs[0].pipelines.len()
        };
        assert_eq!(record(0.5), 1);
        let before = gfx::churn::snapshot();
        assert_eq!(record(0.5), 1, "the same pipeline on the next swap");
        assert_eq!((gfx::churn::snapshot() - before).queue_writes, 0);
        assert_eq!(record(1.5), 1, "and for a new pose");
        let total = gfx::churn::snapshot() - before;
        assert_eq!((total.buffers_created, total.queue_writes), (0, 1));
    }

    #[test]
    fn an_items_draw_resolves_every_per_object_input_and_state_field_or_is_refused() {
        let passes = [equal_pass(0)];
        let pass_of = |i: usize| passes.get(i);
        let parts = [part(4, 0, [Some((0, 0)), None, None, None, None])];
        let rows = [[1.0f32; 4]; 3];
        let d = plan_part(&parts, &pass_of, &frame(), &key(4, 0, 0), &rows, &entry()).unwrap();
        assert_eq!((d.state.depth_compare, d.state.depth_write), (wgpu::CompareFunction::Equal, false), "its own");
        assert!(d.state.primitive.cull_mode.is_some(), "the device's cull, not none");
        // Lighting the implementation reads, absent: refused, not drawn
        // with zeros.
        let dark = ObjectFrame { lighting: None, ..frame() };
        assert!(plan_part(&parts, &pass_of, &dark, &key(4, 0, 0), &rows, &entry()).unwrap_err().contains("lighting"));
        let no_defaults = ObjectFrame { defaults: None, ..frame() };
        assert!(plan_part(&parts, &pass_of, &no_defaults, &key(4, 0, 0), &rows, &entry()).unwrap_err().contains("defaults"));
        // A state field the device's array does not give.
        let mut unread = entry();
        unread.0.retain(|(t, _)| *t != RenderStateType::FrontFace);
        assert!(plan_part(&parts, &pass_of, &frame(), &key(4, 0, 0), &rows, &unread).unwrap_err().contains("FrontFace"));
        // The stencil on at the layer-3 call.
        let mut stencil = entry();
        stencil.0.retain(|(t, _)| *t != RenderStateType::StencilEnable);
        stencil.0.push((RenderStateType::StencilEnable, 1));
        assert!(plan_part(&parts, &pass_of, &frame(), &key(4, 0, 0), &rows, &stencil).unwrap_err().contains("stencil"));
    }
}
