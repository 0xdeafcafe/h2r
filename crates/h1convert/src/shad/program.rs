//! Compiled shader programs for Halo 1 features with no retail instance.

use map::build::Block;
use map::{FourCC, TagId};
use shaders::psh::PixelShaderDef;
use tags::defs::{self, Defs, Record, StructDef, Val};
use tags::shader::{IndexRange, TemplatePostprocess};

use super::{Layout, Root, build_record, set};
use crate::{Error, Result};

pub mod force_field;
pub mod model_defaults;
pub mod model_effects;
pub mod scope_filters;

const SPAS: FourCC = FourCC(*b"spas");

fn def(group: &[u8; 4]) -> &'static StructDef {
    Defs::builtin()
        .group(FourCC(*group))
        .expect("shader definition")
}

fn children(
    record: &mut Record,
    desc: &StructDef,
    field: &str,
    values: impl IntoIterator<Item = Vec<(&'static str, Val)>>,
) -> Result<()> {
    let child = desc
        .child(field)
        .ok_or(Error::Invalid("shader block definition"))?;
    let mut rows = Vec::new();
    for fields in values {
        let mut row = defs::default_record(child);
        for (name, value) in fields {
            set(&mut row, name, value)?;
        }
        rows.push(row);
    }
    set(record, field, Val::Block(rows))
}

fn int(n: impl Into<i64>) -> Val {
    Val::Int(n.into())
}

fn range(start: u16, count: u16) -> Val {
    int(start | (count << 9))
}

fn packed(r: IndexRange) -> Val {
    int(r.encode())
}

fn encode(def: &StructDef, record: &Record) -> Result<Block> {
    defs::to_built(def, record, &mut |_| None)
        .map_err(|e| Error::Textures(format!("{}: {e}", def.name)))
}

pub(super) fn stem(graph: &TemplatePostprocess, pass_id: impl Fn(TagId) -> TagId) -> Result<Block> {
    let d = def(b"stem");
    let pp = d.child("postprocess").expect("stem postprocess");
    let mut body = defs::default_record(pp);
    children(
        &mut body,
        pp,
        "lods",
        graph.lods.iter().map(|lod| {
            vec![
                ("layers", packed(lod.layers)),
                ("layer_mask", int(lod.layer_mask)),
            ]
        }),
    )?;
    children(
        &mut body,
        pp,
        "layers",
        graph
            .layers
            .iter()
            .map(|&layer| vec![("passes", packed(layer))]),
    )?;
    children(
        &mut body,
        pp,
        "passes",
        graph.passes.iter().map(|p| {
            vec![
                ("pass", Val::tag(SPAS, pass_id(p.pass.id))),
                ("implementations", packed(p.implementations)),
            ]
        }),
    )?;
    children(
        &mut body,
        pp,
        "implementations",
        graph.implementations.iter().map(|i| {
            vec![
                ("bitmaps", packed(i.bitmaps)),
                ("pixel_constants", packed(i.pixel_constants)),
                ("vertex_constants", packed(i.vertex_constants)),
            ]
        }),
    )?;
    children(
        &mut body,
        pp,
        "bindings",
        graph.bindings.iter().map(|b| {
            vec![
                ("target", int(b.word)),
                ("lane_mask", int(b.b2)),
                ("source", int(b.source)),
            ]
        }),
    )?;
    let mut root = defs::default_record(d);
    set(&mut root, "postprocess", Val::Block(vec![body]))?;
    encode(d, &root)
}

/// One implementation with a fixed combiner, one sampler per texture
/// and the state changes applied on entry and restored on exit.
pub(super) fn pass(
    vertex_shader: TagId,
    pixel: &PixelShaderDef,
    textures: &[(u8, u8)],
    address: u8,
    mip_filter: u8,
    states: &[(u8, u32)],
    restore: &[(u8, u32)],
) -> Result<Block> {
    pass_with_inputs(
        vertex_shader,
        pixel,
        textures,
        address,
        mip_filter,
        states,
        restore,
        &[],
        &[],
        0,
    )
}

#[allow(clippy::too_many_arguments)]
fn pass_with_inputs(
    vertex_shader: TagId,
    pixel: &PixelShaderDef,
    textures: &[(u8, u8)],
    address: u8,
    mip_filter: u8,
    states: &[(u8, u32)],
    restore: &[(u8, u32)],
    pixel_bindings: &[[u8; 4]],
    vertex_bindings: &[[u8; 4]],
    scalars: u64,
) -> Result<Block> {
    // Mode 4 reads a vector engine input (the object change colours, for example).
    let vectors = pixel_bindings
        .iter()
        .filter(|b| b[2] & 7 == 4 && b[3] < 64)
        .fold(0u64, |mask, b| mask | (1u64 << b[3]));
    let d = def(b"spas");
    let pp = d.child("postprocess").expect("spas postprocess");
    let mut body = defs::default_record(pp);
    let bytes = pixel.to_bytes();
    let byte_list = |part: &[u8]| Val::List(part.iter().map(|&x| int(x)).collect());
    children(
        &mut body,
        pp,
        "implementations",
        [vec![
            ("textures", range(0, textures.len() as u16)),
            ("render_states", range(0, states.len() as u16)),
            ("stage_states", range(0, textures.len() as u16)),
            ("pixel_shader", byte_list(&bytes[..0xd6])),
            (
                "pixel_shader_mid",
                Val::Word(u32::from_le_bytes(bytes[0xd6..0xda].try_into().unwrap())),
            ),
            ("pixel_shader_tail", byte_list(&bytes[0xda..])),
            (
                "vector_inputs",
                Val::List(vec![int(vectors as u32), int((vectors >> 32) as u32)]),
            ),
            (
                "scalar_inputs",
                Val::List(vec![int(scalars as u32), int((scalars >> 32) as u32)]),
            ),
            (
                "external_pixel_constants",
                range(0, pixel_bindings.len() as u16),
            ),
            (
                "external_vertex_constants",
                range(pixel_bindings.len() as u16, vertex_bindings.len() as u16),
            ),
            ("vertex_shader", Val::tag(FourCC(*b"vrtx"), vertex_shader)),
            (
                "restore_states",
                range(states.len() as u16, restore.len() as u16),
            ),
        ]],
    )?;
    children(
        &mut body,
        pp,
        "texture_bindings",
        textures.iter().enumerate().map(|(stage, &(slot, source))| {
            vec![
                ("slot", int(slot)),
                ("source", int(source)),
                ("stage", int(stage as u8)),
            ]
        }),
    )?;
    children(
        &mut body,
        pp,
        "render_states",
        states
            .iter()
            .chain(restore)
            .map(|&(state, value)| vec![("state", int(state)), ("value", int(value))]),
    )?;
    children(
        &mut body,
        pp,
        "stage_states",
        textures.iter().map(|_| {
            vec![
                ("min_filter", int(0x20)),
                ("mag_mip_filter", int(2 | (mip_filter << 4))),
                ("address_u", int(address)),
                ("address_v", int(address)),
                ("address_w", int(address)),
            ]
        }),
    )?;
    children(
        &mut body,
        pp,
        "engine_bindings",
        pixel_bindings
            .iter()
            .chain(vertex_bindings)
            .map(|bytes| vec![("bytes", byte_list(bytes))]),
    )?;
    let mut root = defs::default_record(d);
    set(&mut root, "postprocess", Val::Block(vec![body]))?;
    encode(d, &root)
}

/// Vertex program for Halo 2's layer-15 screen effect quads.
pub const SCOPE_VERTEX: &str = "rasterizer\\vertex_shaders_xbox\\screen_xform_4";

/// Composites black over the scene with the mask's luminance as opacity.
/// Engine texture 16 selects the bitmap and image set by the HUD draw.
pub fn scope_pass(vertex_shader: TagId) -> Result<Block> {
    let pixel = PixelShaderDef {
        combiner_count: 1,
        texture_modes: 1,
        final_inputs_efg: 0x0000_0880, // G = t0.b
        ..PixelShaderDef::default()
    };
    // RGB writes, blending, SrcAlpha, InvSrcAlpha, Add, no depth or cull.
    let states = [
        (67, 0x0001_0101),
        (59, 1),
        (60, 0),
        (62, 0x302),
        (63, 0x303),
        (74, 0x8006),
        (143, 0),
        (147, 0),
    ];
    pass(
        vertex_shader,
        &pixel,
        &[(0xff, 16)],
        3,
        0,
        &states,
        &[(143, 2), (147, 0x900), (67, 0x0101_0101)],
    )
}

fn scope_template(passes: &[TagId]) -> Result<Block> {
    use tags::shader::{
        ImplementationBindings, ParameterBinding, TagRef, TemplateLod, TemplatePass,
    };

    let mut layers = vec![IndexRange::default(); 25];
    layers[15] = IndexRange {
        start: 0,
        count: u8::try_from(passes.len())
            .map_err(|_| Error::Invalid("scope template: more passes than a layer range holds"))?,
    };
    let graph = TemplatePostprocess {
        lods: vec![TemplateLod {
            layers: IndexRange {
                start: 0,
                count: 25,
            },
            layer_mask: 1 << 15,
            unknown_6: 0,
        }],
        layers,
        passes: passes
            .iter()
            .map(|&id| TemplatePass {
                pass: TagRef { group: SPAS, id },
                implementations: IndexRange { start: 0, count: 1 },
            })
            .collect(),
        implementations: vec![ImplementationBindings {
            vertex_constants: IndexRange { start: 0, count: 8 },
            ..ImplementationBindings::default()
        }],
        // Identity rows for texture stages 0 through 3, c[18]..c[25].
        bindings: (0..8)
            .map(|i| 0x01d0 + i + 0x20 * (i % 2))
            .enumerate()
            .map(|(i, word)| ParameterBinding {
                word,
                b2: 0,
                source: i as u8,
            })
            .collect(),
    };
    stem(&graph, |id| id)
}

/// A scope effect has no static bitmap parameter. The active HUD supplies
/// the current viewport's image through engine texture 16.
pub fn scope_instance(stem_id: TagId) -> Result<Block> {
    let layout = Layout {
        stem: "",
        word_14: 5,
        ints: [0, 0],
        bitmaps: &[],
        colors: &[],
        vectors: &[],
        root: Root::None,
        animation: None,
        tint: None,
    };
    let vectors = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]].repeat(4);
    let rec = build_record(&layout, stem_id, &[], &[], &vectors, None)?;
    encode(super::shad_def(), &rec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaders::psh::{PixelOptions, TextureDim, interp};

    struct Mask(f32);

    impl interp::Sampler for Mask {
        fn sample(&self, stage: usize, _: TextureDim, _: [f32; 3]) -> [f32; 4] {
            assert_eq!(stage, 0);
            [self.0, self.0, self.0, 1.0]
        }
    }

    #[test]
    fn scope_shader_uses_luminance_as_viewport_opacity() {
        let built = scope_pass(TagId(0xe200_0001)).unwrap();
        let record = defs::from_built(def(b"spas"), &built).unwrap();
        let path = |key| format!("postprocess[0].implementations[0].{key}");
        let bytes = |key| match record.at(&path(key)).unwrap() {
            Val::List(list) => list
                .iter()
                .map(|v| match v {
                    Val::Int(n) => *n as u8,
                    other => panic!("{other:?}"),
                })
                .collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        let mut ps = bytes("pixel_shader");
        let Some(Val::Word(mid)) = record.at(&path("pixel_shader_mid")) else {
            panic!("shader word")
        };
        ps.extend(mid.to_le_bytes());
        ps.extend(bytes("pixel_shader_tail"));
        let def = PixelShaderDef::from_bytes(&ps).unwrap();
        for luminance in [0.0, 0.5, 1.0] {
            let color = interp::run(
                &def,
                &PixelOptions::default(),
                &interp::Inputs::default(),
                &interp::Uniforms::default(),
                &Mask(luminance),
            )
            .unwrap();
            assert_eq!(color, [0.0, 0.0, 0.0, luminance]);
        }
        assert_eq!(
            record.at("postprocess[0].texture_bindings[0].source"),
            Some(&int(16))
        );
    }
}
