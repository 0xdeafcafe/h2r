//! The Shader tab's read of a `shad` (docs/editor/rich-editors.md,
//! "Shaders", slice S1): its bitmap, colour and vector parameters as
//! slots, each with the `tag.set` path that writes it and a `Used by` line
//! from the template's bindings, its animated properties and what they
//! drive, and its preview on a sphere, a square or an object's model.
//!
//! Everything reads the shown map with the document's edits laid over it
//! ([`App::open_shown_map_edited`]), so the tab, `shader.read` and
//! `shader.preview` see an edit the moment it is made; the decodes are the
//! game's own (`tags::material::ShaderLibrary`, `tags::shader`). A slot is
//! written by `tag.set` at its path (`shader.set` is that call), so undo,
//! save and the Pending tab see an ordinary field edit.
//!
//! Slots bind to the template by index: the template's parameter names
//! (`stem` `+0x40`, `+0x4c`) are not read (slice S3), so a slot is named
//! by its kind and index and the tab says [`NAMES_NOT_READ`] once.

use std::collections::BTreeMap;

use map::{FourCC, TagId};
use scenario_edit::native_view::code::{self, PassCode};
use scenario_edit::native_view::preview::{self, MaterialDraws, MaterialPreview, Shape};
use serde_json::{Value, json};
use tags::material::ShaderLibrary;
use tags::shader::{IndexRange, PassPostprocess, ShaderPostprocess, TemplatePostprocess};

use super::App;

/// The shader instance group.
pub const SHAD: FourCC = FourCC(*b"shad");

/// The level of detail the tab shows and the preview draws: the first,
/// the one the game draws a near object with.
pub const LOD: usize = 0;

/// The tab's sentence while the template's parameter names are not read.
pub const NAMES_NOT_READ: &str = "This template's parameter names are not read yet.";

/// Why a preview shape the tab does not offer here is refused.
pub const NO_OBJECT: &str = "the object shape needs an object tag: name it with `object`";

/// The kind of a parameter slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SlotKind {
    Bitmap,
    Colour,
    Vector,
}

impl SlotKind {
    pub const ALL: [Self; 3] = [Self::Bitmap, Self::Colour, Self::Vector];

    /// The id `shader.set`'s `slot.kind` takes.
    pub fn id(self) -> &'static str {
        match self {
            Self::Bitmap => "bitmap",
            Self::Colour => "colour",
            Self::Vector => "vector",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "bitmap" => Some(Self::Bitmap),
            "colour" | "color" => Some(Self::Colour),
            "vector" => Some(Self::Vector),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bitmap => "Bitmap",
            Self::Colour => "Colour",
            Self::Vector => "Vector",
        }
    }

    /// The `tag.set` path of slot `index`: the compiled postprocess the
    /// engine binds from (`defs/shad.txt`), never the parameter block it
    /// does not read.
    pub fn path(self, index: usize) -> String {
        match self {
            Self::Bitmap => format!("postprocess[0].bitmaps[{index}].bitmap"),
            Self::Colour => format!("postprocess[0].colors[{index}].color"),
            Self::Vector => format!("postprocess[0].vectors[{index}].vector"),
        }
    }
}

/// One place a slot is read: a pass of a layer at [`LOD`], one of its
/// implementations, and what of it reads the slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsedBy {
    pub layer: usize,
    pub pass: usize,
    pub implementation: usize,
    /// In words: `stage 1`, `final combiner c0`, `vertex constant c18`.
    pub reads: String,
}

impl UsedBy {
    pub fn text(&self) -> String {
        format!(
            "{} of pass {} in layer {} (implementation {})",
            self.reads, self.pass, self.layer, self.implementation
        )
    }

    pub fn json(&self) -> Value {
        json!({
            "layer": self.layer,
            "pass": self.pass,
            "implementation": self.implementation,
            "reads": self.reads,
        })
    }
}

/// A slot's value as the postprocess holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SlotValue {
    Bitmap { bitmap: TagId, image: i32 },
    Colour(u32),
    Vector([f32; 4]),
}

/// One parameter slot.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub kind: SlotKind,
    pub index: usize,
    pub value: SlotValue,
    pub used_by: Vec<UsedBy>,
}

impl Slot {
    pub fn path(&self) -> String {
        self.kind.path(self.index)
    }

    /// The row's name until the template's names are read:
    /// `Bitmap 2 · stage 1 of pass 3 in layer 5`.
    pub fn label(&self) -> String {
        let name = format!("{} {}", self.kind.label(), self.index);
        match self.used_by.first() {
            Some(u) => format!("{name} · {}", u.text()),
            None => format!("{name} · not read at this level of detail"),
        }
    }
}

/// One animated property and what it drives.
#[derive(Clone, Debug, PartialEq)]
pub struct Animated {
    pub index: usize,
    /// The object function that drives it (a string ID), or none: game
    /// time, with `period` seconds a cycle.
    pub input: Option<u32>,
    pub period: Option<f32>,
    pub function_bytes: usize,
    /// In words: `pixel constant 8 of pass 0 in layer 5`.
    pub drives: Vec<String>,
}

/// The tab's read of one shader.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderRead {
    pub tag: TagId,
    pub template: TagId,
    pub lod: usize,
    /// The template's levels of detail.
    pub lods: usize,
    pub bitmaps: Vec<Slot>,
    pub colours: Vec<Slot>,
    pub vectors: Vec<Slot>,
    pub animations: Vec<Animated>,
    /// Whether the slots have the template's names (never yet: S3).
    pub names_known: bool,
    /// The passes of each layer at [`LOD`], by layer: how many a preview
    /// draws or leaves out.
    pub passes: BTreeMap<usize, usize>,
    /// Why the tag may not be written, when it may not.
    pub read_only: Option<String>,
    /// What did not read, each in a sentence: a pass that did not decode.
    pub problems: Vec<String>,
}

impl ShaderRead {
    pub fn slots(&self, kind: SlotKind) -> &[Slot] {
        match kind {
            SlotKind::Bitmap => &self.bitmaps,
            SlotKind::Colour => &self.colours,
            SlotKind::Vector => &self.vectors,
        }
    }
}

/// What a texture binding's slot is read by in a pass implementation.
fn stage_reads(spas: Option<&PassPostprocess>, imp: usize, slot: u8) -> String {
    let Some(p) = spas else {
        return format!("slot {slot}");
    };
    let stages: Vec<String> = p
        .implementations
        .get(imp)
        .and_then(|i| p.textures.get(i.textures.indices()))
        .unwrap_or(&[])
        .iter()
        .filter(|t| t.slot == slot)
        .map(|t| format!("stage {}", t.stage))
        .collect();
    if stages.is_empty() {
        format!("slot {slot}, which no stage samples")
    } else {
        stages.join(" and ")
    }
}

/// What a colour binding word writes (`tags::material::apply_color`):
/// the final combiner's constant, or a combiner stage's.
fn colour_reads(word: u16) -> String {
    let c = if word & 0x400 != 0 { "c1" } else { "c0" };
    if word & 0x3c0 == 0x200 {
        format!("final combiner {c}")
    } else {
        format!("combiner stage {} {c}", (word >> 6) & 0xf)
    }
}

/// What a vector binding word writes (`ShaderLibrary`'s bind of
/// `0x00017000`): a vertex default `vN` (bank 0) or the vertex constant
/// `c[18 + N]` (bank 1).
fn vector_reads(word: u16) -> String {
    let reg = word & 0xf;
    if (word >> 4) & 1 == 0 {
        format!("vertex default v{reg}")
    } else {
        format!("vertex constant c{}", 18 + reg)
    }
}

fn get<T>(list: &[T], r: IndexRange) -> &[T] {
    list.get(r.indices()).unwrap_or(&[])
}

/// The `Used by` lines of every slot, and the passes per layer, at
/// [`LOD`]: each pass of each layer, each implementation's binding row,
/// each binding by the slot it names (`ParameterBinding::source`).
fn bindings(
    tp: &TemplatePostprocess,
    passes: &BTreeMap<TagId, PassPostprocess>,
) -> (BTreeMap<(SlotKind, usize), Vec<UsedBy>>, BTreeMap<usize, usize>) {
    let mut used: BTreeMap<(SlotKind, usize), Vec<UsedBy>> = BTreeMap::new();
    let mut counts = BTreeMap::new();
    let Some(l) = tp.lods.get(LOD) else {
        return (used, counts);
    };
    for layer in 0..usize::from(l.layers.count).min(32) {
        let Some(list) = tp.layer_passes(LOD, layer) else {
            continue;
        };
        if list.is_empty() {
            continue;
        }
        counts.insert(layer, list.len());
        for (pi, p) in list.iter().enumerate() {
            let spas = passes.get(&p.pass.id);
            for k in 0..usize::from(p.implementations.count) {
                let Some(ib) = tp
                    .implementations
                    .get(usize::from(p.implementations.start) + k)
                else {
                    continue;
                };
                let at = |reads: String| UsedBy {
                    layer,
                    pass: pi,
                    implementation: k,
                    reads,
                };
                for b in get(&tp.bindings, ib.bitmaps) {
                    let slot = (b.word & 0xff) as u8;
                    used.entry((SlotKind::Bitmap, usize::from(b.source)))
                        .or_default()
                        .push(at(stage_reads(spas, k, slot)));
                }
                for b in get(&tp.bindings, ib.pixel_constants) {
                    used.entry((SlotKind::Colour, usize::from(b.source)))
                        .or_default()
                        .push(at(colour_reads(b.word)));
                }
                for b in get(&tp.bindings, ib.vertex_constants) {
                    used.entry((SlotKind::Vector, usize::from(b.source)))
                        .or_default()
                        .push(at(vector_reads(b.word)));
                }
            }
        }
    }
    (used, counts)
}

/// The animated properties and what each drives at [`LOD`]
/// (`ShaderPostprocess::animation_leaf`, each kind's bindings through
/// their property ranges).
fn animated(sp: &ShaderPostprocess, counts: &BTreeMap<usize, usize>) -> Vec<Animated> {
    let mut drives: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (&layer, &n) in counts {
        for pi in 0..n {
            for k in 0..32 {
                let Some(a) = sp.animation_leaf(LOD, layer, pi, k) else {
                    break;
                };
                for (kind, range) in [
                    ("bitmap", a.bitmaps),
                    ("render state", a.render_states),
                    ("stage state", a.stage_states),
                    ("pixel constant", a.pixel_constants),
                    ("vertex constant", a.vertex_constants),
                ] {
                    for b in get(&sp.animations, range) {
                        let Some(&pr) = sp.property_ranges.get(usize::from(b.properties)) else {
                            continue;
                        };
                        for r in get(&sp.property_refs, pr) {
                            let Ok(p) = usize::try_from(r.property) else {
                                continue;
                            };
                            drives.entry(p).or_default().push(format!(
                                "{kind} {} of pass {pi} in layer {layer}",
                                b.target
                            ));
                        }
                    }
                }
            }
        }
    }
    sp.properties
        .iter()
        .enumerate()
        .map(|(index, p)| Animated {
            index,
            input: (p.input != 0).then_some(p.input),
            period: (p.input == 0).then(|| f32::from_bits(p.input_b)),
            function_bytes: p.function.len(),
            drives: drives.remove(&index).unwrap_or_default(),
        })
        .collect()
}

/// The read of `shader` from a library over the map it is in.
pub fn read_from(lib: &mut ShaderLibrary, shader: TagId) -> Result<ShaderRead, String> {
    let si = lib.shader(shader).map_err(|e| format!("the shader: {e}"))?;
    let sp = si
        .compiled()
        .map_err(|e| format!("the shader: {e}"))?
        .clone();
    let template = si.template.id;
    let tp = lib
        .template(template)
        .and_then(|t| t.compiled().cloned())
        .map_err(|e| format!("its template: {e}"))?;
    let mut problems = Vec::new();
    let mut passes = BTreeMap::new();
    for l in 0..tp.lods.get(LOD).map_or(0, |l| usize::from(l.layers.count).min(32)) {
        for p in tp.layer_passes(LOD, l).unwrap_or(&[]) {
            if passes.contains_key(&p.pass.id) {
                continue;
            }
            match lib.pass(p.pass.id).and_then(|s| s.compiled().cloned()) {
                Ok(pp) => {
                    passes.insert(p.pass.id, pp);
                }
                Err(e) => problems.push(format!("pass {:#010x} of layer {l}: {e}", p.pass.id.0)),
            }
        }
    }
    let (mut used, counts) = bindings(&tp, &passes);
    let mut slots = |kind: SlotKind, values: Vec<SlotValue>| -> Vec<Slot> {
        values
            .into_iter()
            .enumerate()
            .map(|(index, value)| Slot {
                kind,
                index,
                value,
                used_by: used.remove(&(kind, index)).unwrap_or_default(),
            })
            .collect()
    };
    let bitmaps = slots(
        SlotKind::Bitmap,
        sp.bitmaps
            .iter()
            .map(|b| SlotValue::Bitmap {
                bitmap: b.bitmap,
                image: b.image,
            })
            .collect(),
    );
    let colours = slots(
        SlotKind::Colour,
        sp.colors.iter().map(|&c| SlotValue::Colour(c)).collect(),
    );
    let vectors = slots(
        SlotKind::Vector,
        sp.vectors.iter().map(|&v| SlotValue::Vector(v)).collect(),
    );
    // A binding naming a slot the shader lacks: the material refuses to
    // resolve, and the tab says which.
    for ((kind, index), _) in used {
        problems.push(format!(
            "the template binds {} {index}, which the shader does not have",
            kind.id()
        ));
    }
    Ok(ShaderRead {
        tag: shader,
        template,
        lod: LOD,
        lods: tp.lods.len(),
        bitmaps,
        colours,
        vectors,
        animations: animated(&sp, &counts),
        names_known: false,
        passes: counts,
        read_only: None,
        problems,
    })
}

/// A shader preview's passes: how many draw and why the others do not,
/// from the layers the preview drew ([`MaterialDraws`]) and the passes
/// each layer has ([`ShaderRead::passes`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewPasses {
    pub drawn: usize,
    /// `(layer, passes, why)`, in layer order.
    pub refused: Vec<(u8, usize, String)>,
    /// Why the base look does not draw, when it does not.
    pub base: Option<String>,
    /// Why the material did not resolve, when it did not.
    pub unresolved: Vec<String>,
}

impl PreviewPasses {
    pub fn of(read: &ShaderRead, draws: &MaterialDraws) -> Self {
        // Layer 3 with no pass of its own is the base look: one pass.
        let n = |layer: u8| read.passes.get(&usize::from(layer)).copied().unwrap_or(1);
        Self {
            drawn: draws.drawn.iter().map(|&l| n(l)).sum(),
            refused: draws
                .refused
                .iter()
                .map(|(l, why)| (*l, n(*l), why.clone()))
                .collect(),
            base: draws.base.clone(),
            unresolved: draws.unresolved.clone(),
        }
    }

    /// Every refusal in a sentence, as the tab's header lists them.
    pub fn sentences(&self) -> Vec<String> {
        let mut out: Vec<String> = self.base.iter().cloned().collect();
        out.extend(self.unresolved.iter().map(|w| format!("the material: {w}")));
        for (layer, n, why) in &self.refused {
            let passes = if *n == 1 { "pass" } else { "passes" };
            out.push(format!("layer {layer} ({n} {passes}) is left out: {why}"));
        }
        out
    }

    pub fn json(&self) -> Value {
        json!({
            "passes_drawn": self.drawn,
            "refusals": self.refused.iter().map(|(layer, passes, why)| json!({
                "layer": layer,
                "passes": passes,
                "why": why,
            })).collect::<Vec<_>>(),
            "base": self.base,
            "unresolved": self.unresolved,
        })
    }
}

/// The background a preview is drawn over (sRGB-encoded), and what
/// `covered` counts against.
pub const BACKGROUND: [f64; 3] = [0.08, 0.08, 0.09];

/// A drawn preview: its pixels and what of the material they hold.
pub struct Picture {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub passes: PreviewPasses,
}

impl App {
    /// The tab's read of `shader` from the shown map with its edits.
    pub fn shader_read(&self, shader: TagId) -> Result<ShaderRead, String> {
        let doc = self.shown_tags();
        let t = doc.tag(shader).ok_or("no such tag")?;
        if t.group != SHAD {
            return Err(format!(
                "{} is not a shader instance (shad)",
                doc.tag_label(shader.0)
            ));
        }
        let mut map = self.open_shown_map_edited()?;
        let mut lib = ShaderLibrary::read(&mut map).map_err(|e| e.to_string())?;
        let mut read = read_from(&mut lib, shader)?;
        read.read_only = doc.refusal(shader).map(str::to_owned);
        Ok(read)
    }

    /// The programs of pass number `pass` (across the layers, in layer
    /// order) and implementation `implementation` of `shader` as text,
    /// from the shown map with its edits: the Code section and
    /// `shader.code`.
    pub fn shader_code(
        &self,
        shader: TagId,
        pass: usize,
        implementation: usize,
    ) -> Result<PassCode, String> {
        let doc = self.shown_tags();
        let t = doc.tag(shader).ok_or("no such tag")?;
        if t.group != SHAD {
            return Err(format!(
                "{} is not a shader instance (shad)",
                doc.tag_label(shader.0)
            ));
        }
        let mut map = self.open_shown_map_edited()?;
        code::pass_code(&mut map, shader, pass, implementation)
    }

    /// `shader` drawn alone on `shape` from the shown map with its edits,
    /// ready to draw ([`preview::render_material`]).
    pub fn shader_preview(&self, shader: TagId, shape: Shape) -> Result<MaterialPreview, String> {
        let mut map = self.open_shown_map_edited()?;
        let mut files = self.shown_raw_files()?;
        preview::material(&mut map, &mut files, shader, shape)
    }

    /// `shader` drawn on `shape` into a `size` picture on `device`: the
    /// pixels, and the passes drawn and left out (read after the draw).
    pub fn shader_picture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        read: &ShaderRead,
        shape: Shape,
        size: [u32; 2],
    ) -> Result<Picture, String> {
        let p = self.shader_preview(read.tag, shape)?;
        let texture = preview::render_material(device, queue, &p, size, BACKGROUND);
        let rgba = crate::headless::read_texture(device, queue, &texture)
            .map_err(|e| format!("{e:#}"))?;
        Ok(Picture {
            rgba,
            width: texture.width(),
            height: texture.height(),
            passes: PreviewPasses::of(read, &preview::material_draws(&p)),
        })
    }
}

#[cfg(test)]
#[path = "shader_tests.rs"]
mod tests;
