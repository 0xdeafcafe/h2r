//! `h2 shader`: print a resolved material, or validate every `shad`, `stem`
//! and `spas` in one map or the whole disc. Writes nothing to disk.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use map::{FourCC, Map, TagId, TagQuery};
use tags::material::{Material, Origin, ShaderLibrary, TextureSource};
use tags::shader::{self, SHAD, SPAS, STEM, VRTX, render_state_name};

const USAGE: &str = "usage: h2 shader <map> <shad tag name> [--disc disc.iso]
       h2 shader --all <map|all> [--disc disc.iso]

Prints the material a shad resolves to: per (lod, layer) the ordered passes,
and per pass implementation the texture stages, sampler and render states,
vertex constants/defaults and the final pixel shader definition. --all
decodes and validates every shad, stem and spas (and translates every
resolved pixel shader to WGSL through crates/formats/shaders and naga), printing
counts; it exits non-zero on any failure. Nothing is written to disk.";

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let (mut positional, mut disc, mut all) = (Vec::new(), None, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--all" => all = true,
            "--disc" => {
                disc = Some(PathBuf::from(
                    args.next().context("--disc needs an ISO path")?,
                ))
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            s if s.starts_with('-') => bail!("unknown option {s}\n{USAGE}"),
            _ => positional.push(a),
        }
    }
    match (all, positional.as_slice()) {
        (true, [map]) => {
            let failures = validate(map, disc)?;
            if failures > 0 {
                bail!("{failures} failures");
            }
            Ok(())
        }
        (false, [map, tag]) => print_one(map, tag, disc),
        _ => bail!("{USAGE}"),
    }
}

fn print_one(map_name: &str, tag: &str, disc: Option<PathBuf>) -> Result<()> {
    let mut map = super::maps::load_map(map_name, disc)?;
    let id = map
        .tag(TagQuery::NameInGroup(tag, SHAD))
        .with_context(|| format!("shad {tag}"))?
        .id;
    let mut lib = ShaderLibrary::read(&mut map)?;
    let s = lib.shader(id)?;
    let m = lib.material(id, None)?;
    let name = |id: TagId| -> String {
        if id == TagId::NULL {
            return "none".into();
        }
        map.tag(id)
            .ok()
            .and_then(|t| t.name.clone())
            .map(|n| {
                format!(
                    "{n} [{}]",
                    map.tag(id).map(|t| t.group.to_string()).unwrap_or_default()
                )
            })
            .unwrap_or_else(|| format!("{id}"))
    };
    let pp = s.compiled()?;
    println!("shad {tag} ({id})");
    println!("  template   {}", name(m.template));
    println!(
        "  inputs     int4={} int5={} scalar21={} vector24-source={}",
        s.int_inputs[0], s.int_inputs[1], s.scalar_input, s.vector_input
    );
    println!("  parameters");
    for (i, b) in pp.bitmaps.iter().enumerate() {
        println!(
            "    bitmap[{i}] {} image {} lod offset {}",
            name(b.bitmap),
            b.image,
            b.lod_offset
        );
    }
    for (i, c) in pp.colors.iter().enumerate() {
        println!("    color[{i}]  {c:#010x}");
    }
    for (i, v) in pp.vectors.iter().enumerate() {
        println!("    vector[{i}] {v:?}");
    }
    if !pp.properties.is_empty() {
        println!(
            "    {} animated properties, {} animation bindings",
            pp.properties.len(),
            pp.animations.len()
        );
    }
    for l in &m.layers {
        println!(
            "  lod {} layer {}: {} passes",
            l.lod,
            l.layer,
            l.passes.len()
        );
        for p in &l.passes {
            println!("    pass {} {}", p.index, name(p.pass));
            for i in &p.implementations {
                println!(
                    "      implementation {} vertex shader {}",
                    i.index,
                    i.vertex_shader.map(name).unwrap_or("none".into())
                );
                for (st, t) in i.textures.iter().enumerate() {
                    let Some(t) = t else { continue };
                    let src = match t.source {
                        TextureSource::Bitmap { bitmap, image, .. } => {
                            format!("{} image {image}", name(bitmap))
                        }
                        other => format!("{other:?}"),
                    };
                    let size = t
                        .size
                        .map(|(w, h)| format!(" (slot size refreshed to {w}x{h})"))
                        .unwrap_or_default();
                    println!("        stage {st}: {src}{size}");
                }
                for (st, s) in i.stage_states.iter().enumerate() {
                    println!(
                        "        sampler {st}: address {}/{}/{} filter mag {} min {} mip {} lodbias {} maxmip {} aniso {} alphakill {} colorsign {:#x}",
                        s.address_u,
                        s.address_v,
                        s.address_w,
                        s.mag_filter,
                        s.min_filter,
                        s.mip_filter,
                        f32::from_bits(s.mip_lod_bias),
                        s.max_mip_level,
                        s.max_anisotropy,
                        s.alpha_kill,
                        s.color_sign
                    );
                }
                let rs = |v: &[shader::RenderStateValue]| -> String {
                    v.iter()
                        .map(|r| {
                            format!(
                                "{}={:#x}",
                                render_state_name(r.state).unwrap_or("?"),
                                r.value
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                println!("        render states: {}", rs(&i.render_states));
                if !i.restore_states.is_empty() {
                    println!("        on exit:       {}", rs(&i.restore_states));
                }
                for (label, regs) in [("c", &i.vertex_constants), ("v", &i.vertex_defaults)] {
                    for r in regs.iter() {
                        let o = if r.origin == Origin::Engine {
                            " (engine)"
                        } else {
                            ""
                        };
                        println!("        {label}[{}] = {:?}{o}", r.index, r.value);
                    }
                }
                let def = shaders::PixelShaderDef::from_bytes(&i.pixel_shader)
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                println!(
                    "        pixel shader{}{}: c0 mask {:#x} c1 mask {:#x}",
                    if i.variant { " (variant)" } else { "" },
                    if i.pixel_constants_from_engine {
                        " (engine constants)"
                    } else {
                        ""
                    },
                    i.c0_written,
                    i.c1_written
                );
                for line in shaders::psh::disassemble(&def).lines() {
                    println!("          {line}");
                }
                for a in &i.animated {
                    println!(
                        "        animated {:?} target {} ({} property refs)",
                        a.kind,
                        a.binding.target,
                        a.properties.len()
                    );
                }
                for e in &i.engine_render_states {
                    println!("        engine render state binding {:02x?}", e.bytes);
                }
                for e in &i.engine_stage_states {
                    println!("        engine stage state binding {:02x?}", e.bytes);
                }
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct Counts {
    maps: usize,
    shad: usize,
    stem: usize,
    spas: usize,
    names: [HashSet<String>; 3],
    implementations: usize,
    variants: usize,
    no_vertex_shader: usize,
    static_without_combiners: usize,
    variant_with_combiners: usize,
    shared_blocks: usize,
    blocks: usize,
    materials: usize,
    layers: usize,
    passes: usize,
    resolved: usize,
    aliased_blocks: usize,
    animated_shaders: usize,
    tree_mismatch: usize,
    texture_sources: BTreeMap<String, usize>,
    render_states: BTreeMap<u8, usize>,
    psdefs: HashSet<[u8; shader::PSDEF_SIZE]>,
    failures: usize,
    examples: Vec<String>,
}
impl Counts {
    fn fail(&mut self, what: String) {
        self.failures += 1;
        if self.examples.len() < 20 {
            self.examples.push(what);
        }
    }
}

fn validate(which: &str, disc: Option<PathBuf>) -> Result<usize> {
    let mut c = Counts::default();
    if which == "all" {
        let disc = disc.map(Ok).unwrap_or_else(super::default_disc)?;
        let mut iso = super::open_iso(&disc)?;
        let part = iso.partition_offset();
        let entries = iso.entries()?;
        for e in entries
            .iter()
            .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
        {
            let mut map = super::maps::disc_map(&disc, part, e)?;
            check_map(&e.path, &mut map, &mut c)?;
        }
    } else {
        let mut map = super::maps::load_map(which, disc)?;
        check_map(which, &mut map, &mut c)?;
    }
    // Every distinct resolved pixel shader must translate cleanly and validate.
    let mut warned = 0;
    for p in &c.psdefs {
        let def = shaders::PixelShaderDef::from_bytes(p).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let ps = shaders::psh::translate(&def, &Default::default());
        if !ps.warnings.is_empty() {
            warned += 1;
        }
        if let Err(e) = shaders::validate(&ps.module()) {
            c.failures += 1;
            c.examples
                .push(format!("resolved pixel shader failed naga: {e}"));
        }
    }
    println!("{} maps", c.maps);
    println!("shad {} tags ({} unique names)", c.shad, c.names[0].len());
    println!("stem {} tags ({} unique names)", c.stem, c.names[1].len());
    println!(
        "spas {} tags ({} unique names), {} implementations ({} variant pixel shaders, {} without a vertex shader)",
        c.spas,
        c.names[2].len(),
        c.implementations,
        c.variants,
        c.no_vertex_shader
    );
    println!(
        "materials {} resolved: {} (lod, layer) pass lists, {} passes, {} implementations",
        c.materials, c.layers, c.passes, c.resolved
    );
    println!(
        "{} distinct resolved pixel shaders translate and pass naga ({} with translation warnings)",
        c.psdefs.len(),
        warned
    );
    println!(
        "{} nonempty postprocess blocks: {} point outside their own tag, {} share an address with another block of the same tag",
        c.blocks, c.aliased_blocks, c.shared_blocks
    );
    println!(
        "implementations: {} static with no general combiners, {} variants whose embedded PSDEF has combiners",
        c.static_without_combiners, c.variant_with_combiners
    );
    println!(
        "{} shaders carry animated parameters; {} shad/stem trees differ in shape",
        c.animated_shaders, c.tree_mismatch
    );
    println!("texture sources: {:?}", c.texture_sources);
    let rs: Vec<String> = c
        .render_states
        .iter()
        .map(|(s, n)| format!("{}:{n}", render_state_name(*s).unwrap_or("?")))
        .collect();
    println!("render states used: {}", rs.join(" "));
    println!("{} failures", c.failures);
    for e in &c.examples {
        println!("  {e}");
    }
    Ok(c.failures)
}

fn check_map(label: &str, map: &mut Map<BufReader<File>>, c: &mut Counts) -> Result<()> {
    c.maps += 1;
    let mut lib = ShaderLibrary::read(map)?;
    let tags: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| [SHAD, STEM, SPAS].contains(&t.group))
        .map(|t| {
            (
                t.id,
                t.group,
                t.name.clone().unwrap_or_default(),
                t.location,
            )
        })
        .collect();
    let groups: HashMap<TagId, FourCC> = map.tags().iter().map(|t| (t.id, t.group)).collect();
    let ref_ok = |r: shader::TagRef, want: FourCC| r.is_null() || groups.get(&r.id) == Some(&want);
    for (id, group, name, loc) in tags {
        let what = format!("{label} {group} {name}");
        if group == SHAD {
            c.shad += 1;
            c.names[0].insert(name.clone());
            let s = match lib.shader(id) {
                Ok(s) => s,
                Err(e) => {
                    c.fail(format!("{what}: {e}"));
                    continue;
                }
            };
            aliasing(
                c,
                lib.metadata(),
                loc,
                0x20,
                shader::SHAD_POSTPROCESS,
                &SHAD_PP_BLOCKS,
            );
            if s.postprocess.len() != 1 {
                c.fail(format!(
                    "{what}: {} postprocess elements",
                    s.postprocess.len()
                ));
                continue;
            }
            let pp = &s.postprocess[0];
            if pp.template != s.template.id || !ref_ok(s.template, STEM) || s.template.is_null() {
                c.fail(format!("{what}: template reference mismatch"));
            }
            if let Err(e) = pp.validate() {
                c.fail(format!("{what}: {e}"));
            }
            for b in &pp.bitmaps {
                if b.bitmap != TagId::NULL && groups.get(&b.bitmap) != Some(&shader::BITM) {
                    c.fail(format!("{what}: bitmap parameter does not resolve"));
                }
            }
            if !pp.properties.is_empty() {
                c.animated_shaders += 1;
            }
            if let Ok(t) = lib
                .template(s.template.id)
                .and_then(|t| t.compiled().cloned())
                && (t.lods.len() != pp.lods.len()
                    || t.layers.len() != pp.layers.len()
                    || t.passes.len() != pp.passes.len()
                    || t.implementations.len() != pp.implementations.len())
            {
                c.tree_mismatch += 1;
            }
            match lib.material(id, None) {
                Ok(m) => count_material(c, &m),
                Err(e) => c.fail(format!("{what}: material: {e}")),
            }
        } else if group == STEM {
            c.stem += 1;
            c.names[1].insert(name.clone());
            aliasing(
                c,
                lib.metadata(),
                loc,
                0x58,
                shader::STEM_POSTPROCESS,
                &STEM_PP_BLOCKS,
            );
            match lib.template(id).cloned() {
                Ok(t) => {
                    if t.postprocess.len() != 1 {
                        c.fail(format!(
                            "{what}: {} postprocess elements",
                            t.postprocess.len()
                        ));
                        continue;
                    }
                    if let Err(e) = t.postprocess[0].validate() {
                        c.fail(format!("{what}: {e}"));
                    }
                    for r in t.references {
                        if !ref_ok(r, SHAD) {
                            c.fail(format!("{what}: shad reference does not resolve"));
                        }
                    }
                    for p in &t.postprocess[0].passes {
                        if p.pass.group != SPAS || !ref_ok(p.pass, SPAS) || p.pass.is_null() {
                            c.fail(format!("{what}: pass reference does not resolve"));
                        }
                    }
                }
                Err(e) => c.fail(format!("{what}: {e}")),
            }
        } else {
            c.spas += 1;
            c.names[2].insert(name.clone());
            aliasing(
                c,
                lib.metadata(),
                loc,
                0x1c,
                shader::SPAS_POSTPROCESS,
                &SPAS_PP_BLOCKS,
            );
            match lib.pass(id).cloned() {
                Ok(p) => {
                    if p.postprocess.len() != 1 {
                        c.fail(format!(
                            "{what}: {} postprocess elements",
                            p.postprocess.len()
                        ));
                        continue;
                    }
                    let pp = &p.postprocess[0];
                    if let Err(e) = pp.validate() {
                        c.fail(format!("{what}: {e}"));
                    }
                    c.implementations += pp.implementations.len();
                    for i in &pp.implementations {
                        if !i.fragments.is_empty() {
                            c.variants += 1;
                        }
                        let combiners =
                            u32::from_le_bytes(i.pixel_shader[0xd4..0xd8].try_into().unwrap())
                                & 0xff;
                        if i.fragments.is_empty() == (combiners == 0) && i.fragments.is_empty() {
                            c.static_without_combiners += 1;
                        }
                        if !i.fragments.is_empty() && combiners != 0 {
                            c.variant_with_combiners += 1;
                        }
                        if i.vertex_shader.is_null() {
                            c.no_vertex_shader += 1;
                        }
                        if !i.vertex_shader.is_null()
                            && (i.vertex_shader.group != VRTX || !ref_ok(i.vertex_shader, VRTX))
                        {
                            c.fail(format!("{what}: vertex shader reference does not resolve"));
                        }
                    }
                    for r in &pp.render_states {
                        *c.render_states.entry(r.state).or_default() += 1;
                        if render_state_name(r.state).is_none() {
                            c.fail(format!("{what}: unknown render state {}", r.state));
                        }
                    }
                }
                Err(e) => c.fail(format!("{what}: {e}")),
            }
        }
    }
    Ok(())
}

const SHAD_PP_BLOCKS: [u32; 11] = [
    0x04, 0x0c, 0x14, 0x1c, 0x24, 0x2c, 0x34, 0x3c, 0x44, 0x4c, 0x54,
];
const STEM_PP_BLOCKS: [u32; 5] = [0x00, 0x08, 0x10, 0x18, 0x20];
const SPAS_PP_BLOCKS: [u32; 8] = [0x00, 0x08, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38];

/// Counts block descriptors of the (single) postprocess element that point
/// outside the owning tag or at the same address as another descriptor.
fn aliasing(
    c: &mut Counts,
    meta: &shader::Metadata,
    loc: Option<map::TagLocation>,
    root: u32,
    stride: usize,
    blocks: &[u32],
) {
    let Some(l) = loc else { return };
    // Descriptors are raw metadata words, so every address is computed with
    // checked arithmetic; one that wraps or lands outside the region reads as
    // an empty block here and is reported by the decoder.
    let word = |a: Option<u32>| {
        a.and_then(|a| meta.bytes(a, 4).ok())
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .unwrap_or(0)
    };
    let end = l.address.saturating_add(l.size);
    let mut seen = HashSet::new();
    let mut check = |c: &mut Counts, count: u32, address: u32| {
        if count == 0 {
            return;
        }
        c.blocks += 1;
        if address < l.address || address >= end {
            c.aliased_blocks += 1;
        }
        if !seen.insert(address) {
            c.shared_blocks += 1;
        }
    };
    let at = |base: u32, o: u32| base.checked_add(o);
    let (n, p) = (
        word(at(l.address, root)),
        word(at(l.address, root).and_then(|a| at(a, 4))),
    );
    check(c, n, p);
    if n == 0 {
        return;
    }
    let _ = stride;
    for &o in blocks {
        let d = at(p, o);
        check(c, word(d), word(d.and_then(|a| at(a, 4))));
    }
}

fn count_material(c: &mut Counts, m: &Material) {
    c.materials += 1;
    for l in &m.layers {
        c.layers += 1;
        for p in &l.passes {
            c.passes += 1;
            for i in &p.implementations {
                c.resolved += 1;
                c.psdefs.insert(i.pixel_shader);
                for t in i.textures.iter().flatten() {
                    let k = match t.source {
                        TextureSource::Bitmap { .. } => "bitmap".to_string(),
                        TextureSource::Engine(n) => format!("engine{n}"),
                        other => format!("{other:?}"),
                    };
                    *c.texture_sources.entry(k).or_default() += 1;
                }
            }
        }
    }
}
