//! Shadow technique comparison on tag geometry (docs/graphics/e3-lighting.md,
//! "What this means for the technique"): stencil shadow volumes against
//! high-resolution shadow maps,
//! rendered offscreen with wgpu under a sun and a point light, at several
//! resolution scales, with GPU timings and closed-mesh statistics.
//!
//!   shadowlab [--disc ISO] --out DIR [options] <map>
//!   shadowlab --out DIR --synthetic [options]       invented scene, no disc
//!   shadowlab [--disc ISO] --list-models <map>      mode tag names in a map
//!
//! Options:
//!   --model TAG[@x,y,z[,yaw]]  a caster (repeatable); without a position it
//!                              stands on the level near the level's centre
//!   --bsp N                    which of the scenario's BSPs (default 0)
//!   --lod N                    model level of detail (default: best)
//!   --level-casts              the level casts shadows too (default: it
//!                              only receives, as the design proposes)
//!   --sun YAW,ELEV             degrees (default 35,50)
//!   --point X,Y,Z              point light position (default above the casters)
//!   --range R                  point light range (default 4x the light's height)
//!   --camera EX,EY,EZ,TX,TY,TZ eye and target (default framed on the casters)
//!   --base WxH                 1x resolution (default 640x480, the guest's)
//!   --scales 1,2,4             resolution scales
//!   --map N                    sun shadow map edge at 1x (default 2048)
//!   --cube N                   point-light cube face edge at 1x (default 1024)
//!   --frames N                 timed frames per configuration (default 30)
//!   --force                    replace files already in DIR
//!
//! Writes PNGs, `report.md` and `timings.csv` into DIR, which must be outside
//! the repository: images and numbers from the disc are never committed.
#[path = "../../disc.rs"]
#[allow(dead_code)]
mod disc;
#[path = "../../level.rs"]
mod level;
#[path = "../../mesh.rs"]
#[allow(dead_code)]
mod mesh;
#[path = "../../out.rs"]
#[allow(dead_code)]
mod out;

mod compare;
mod geom;
mod gpu;
mod smap;
mod volume;
mod world;

use anyhow::{Context, Result, bail};
use geom::{TopoStats, Topology, TriMesh, V3, WELD_EPSILON, add, length, mul, sub};
use gpu::{
    Clock, Lab, LightRun, ShadowProjection, Technique, TechniqueOutput, View, flat_vertices,
};
use level::Tri;
use smap::Camera;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;
use volume::{Light, Volume, VolumeStats};
use world::{Caster, World};

const USAGE: &str =
    "usage: shadowlab [--disc ISO] --out DIR [--model TAG[@x,y,z[,yaw]]]... [--bsp N] [--lod N]
                 [--level-casts] [--sun YAW,ELEV] [--point X,Y,Z] [--range R]
                 [--camera EX,EY,EZ,TX,TY,TZ] [--base WxH] [--scales 1,2,4] [--map N]
                 [--cube N] [--frames N] [--force] <map>
       shadowlab --out DIR --synthetic [options]
       shadowlab [--disc ISO] --list-models <map>";

#[derive(Clone, Debug)]
struct ModelArg {
    tag: String,
    at: Option<V3>,
    yaw: f32,
}

#[derive(Clone, Debug)]
struct Args {
    disc: Option<PathBuf>,
    out: Option<PathBuf>,
    synthetic: bool,
    list_models: bool,
    models: Vec<ModelArg>,
    bsp: usize,
    lod: Option<usize>,
    level_casts: bool,
    sun: (f32, f32),
    point: Option<V3>,
    range: Option<f32>,
    camera: Option<(V3, V3)>,
    base: (u32, u32),
    scales: Vec<u32>,
    map: u32,
    cube: u32,
    frames: u32,
    force: bool,
    map_name: Option<String>,
}

fn floats(v: &str, n: usize, what: &str) -> Result<Vec<f32>> {
    let out: Vec<f32> = v
        .split(',')
        .map(|s| s.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .with_context(|| format!("{what}: numbers separated by commas"))?;
    if out.len() != n || out.iter().any(|f| !f.is_finite()) {
        bail!("{what} needs {n} finite numbers, got {v:?}");
    }
    Ok(out)
}

fn parse_model(v: &str) -> Result<ModelArg> {
    let Some((tag, rest)) = v.split_once('@') else {
        return Ok(ModelArg {
            tag: v.into(),
            at: None,
            yaw: 0.0,
        });
    };
    let f: Vec<f32> = rest
        .split(',')
        .map(|s| s.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .context("--model TAG@x,y,z[,yaw]")?;
    if !(f.len() == 3 || f.len() == 4) || f.iter().any(|x| !x.is_finite()) {
        bail!("--model TAG@x,y,z[,yaw]: got {rest:?}");
    }
    Ok(ModelArg {
        tag: tag.into(),
        at: Some([f[0], f[1], f[2]]),
        yaw: f.get(3).copied().unwrap_or(0.0).to_radians(),
    })
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut a = Args {
        disc: None,
        out: None,
        synthetic: false,
        list_models: false,
        models: Vec::new(),
        bsp: 0,
        lod: None,
        level_casts: false,
        sun: (35.0, 50.0),
        point: None,
        range: None,
        camera: None,
        base: (640, 480),
        scales: vec![1, 2, 4],
        map: 2048,
        cube: 1024,
        frames: 30,
        force: false,
        map_name: None,
    };
    let mut it = args.into_iter();
    while let Some(s) = it.next() {
        let mut next = || it.next().with_context(|| format!("{s} needs a value"));
        match s.as_str() {
            "--disc" => a.disc = Some(next()?.into()),
            "--out" => a.out = Some(next()?.into()),
            "--synthetic" => a.synthetic = true,
            "--list-models" => a.list_models = true,
            "--model" => a.models.push(parse_model(&next()?)?),
            "--bsp" => a.bsp = next()?.parse()?,
            "--lod" => a.lod = Some(next()?.parse()?),
            "--level-casts" => a.level_casts = true,
            "--sun" => {
                let f = floats(&next()?, 2, "--sun")?;
                if !(0.0..=90.0).contains(&f[1]) || f[1] == 0.0 {
                    bail!("--sun elevation must be in (0, 90] degrees");
                }
                a.sun = (f[0], f[1]);
            }
            "--point" => {
                let f = floats(&next()?, 3, "--point")?;
                a.point = Some([f[0], f[1], f[2]]);
            }
            "--range" => {
                let r: f32 = next()?.parse()?;
                if !(r.is_finite() && r > 0.0) {
                    bail!("--range must be positive");
                }
                a.range = Some(r);
            }
            "--camera" => {
                let f = floats(&next()?, 6, "--camera")?;
                let (e, t) = ([f[0], f[1], f[2]], [f[3], f[4], f[5]]);
                if length(sub(t, e)) < 1e-4 {
                    bail!("--camera eye and target coincide");
                }
                a.camera = Some((e, t));
            }
            "--base" => {
                let v = next()?;
                let (w, h) = v.split_once('x').context("--base WxH")?;
                let (w, h): (u32, u32) = (w.parse()?, h.parse()?);
                if !(16..=4096).contains(&w) || !(16..=4096).contains(&h) {
                    bail!("--base edges must be 16..=4096");
                }
                a.base = (w, h);
            }
            "--scales" => {
                a.scales = next()?
                    .split(',')
                    .map(|s| s.trim().parse::<u32>())
                    .collect::<Result<_, _>>()
                    .context("--scales 1,2,4")?;
                if a.scales.is_empty() || a.scales.iter().any(|&s| !(1..=8).contains(&s)) {
                    bail!("--scales must be 1..=8");
                }
            }
            "--map" => a.map = edge(&next()?, "--map")?,
            "--cube" => a.cube = edge(&next()?, "--cube")?,
            "--frames" => {
                a.frames = next()?.parse()?;
                if !(1..=1000).contains(&a.frames) {
                    bail!("--frames must be 1..=1000");
                }
            }
            "--force" => a.force = true,
            "-h" | "--help" => bail!("{USAGE}"),
            other if other.starts_with("--") => bail!("unknown option {other}\n{USAGE}"),
            _ => {
                if a.map_name.replace(s.clone()).is_some() {
                    bail!("one map only\n{USAGE}");
                }
            }
        }
    }
    Ok(a)
}

fn edge(v: &str, what: &str) -> Result<u32> {
    let n: u32 = v.parse()?;
    if !(64..=16384).contains(&n) {
        bail!("{what} must be 64..=16384");
    }
    Ok(n)
}

/// The repository this binary was built from.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Whether `out` lies inside `root`, judged on the deepest existing ancestor
/// with symlinks resolved, so a not-yet-created directory is caught too.
fn inside(out: &Path, root: &Path) -> Result<bool> {
    let root = std::fs::canonicalize(root)?;
    let abs = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::env::current_dir()?.join(out)
    };
    let mut existing = abs.as_path();
    let mut rest = Vec::new();
    while !existing.exists() {
        rest.push(
            existing
                .file_name()
                .context("output path has no existing ancestor")?,
        );
        existing = existing
            .parent()
            .context("output path has no existing ancestor")?;
    }
    let mut full = std::fs::canonicalize(existing)?;
    for c in rest.iter().rev() {
        full.push(c);
    }
    Ok(full.starts_with(&root))
}

// ---- Loading ----------------------------------------------------------

fn sections_for(model: &tags::model::RenderModelGeometry, lod: Option<usize>) -> Vec<usize> {
    let Some(lod) = lod else {
        return model.default_sections();
    };
    let mut out = Vec::new();
    for r in &model.regions {
        if let Some(s) = r
            .permutations
            .first()
            .and_then(|p| p.lod_sections[..=lod.min(5)].iter().rev().flatten().next())
            && !out.contains(s)
        {
            out.push(*s);
        }
    }
    out
}

fn model_soup(model: &tags::model::RenderModelGeometry, sections: &[usize]) -> Result<Vec<Tri>> {
    let m = mesh::Mesh::build(model, sections)?;
    Ok(m.indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| m.vertices[i as usize].position))
        .collect())
}

fn load_world(a: &Args) -> Result<World> {
    if a.synthetic {
        return Ok(world::synthetic());
    }
    let name = a.map_name.as_deref().context(USAGE)?;
    let disc = disc::Disc::open(&disc::disc_path(a.disc.clone())?)?;
    let lvl = level::load_level(&disc, name, a.bsp)?;
    let mut level_soup = lvl.clusters.clone();
    level_soup.extend_from_slice(&lvl.instances);
    let mut defs = TopoStats::default();
    let mut closed_defs = 0;
    let mut used = 0;
    for (d, n) in lvl.definitions.iter().zip(&lvl.placements) {
        if *n == 0 || d.is_empty() {
            continue;
        }
        used += 1;
        let s = world::stats(d);
        closed_defs += usize::from(s.closed());
        defs.add(&s);
    }
    let level_pieces = vec![
        ("clusters".to_string(), world::stats(&lvl.clusters)),
        (
            format!("instance definitions ({closed_defs} of {used} closed)"),
            defs,
        ),
    ];

    let mut casters = Vec::new();
    if !a.models.is_empty() {
        let mut map = disc.open_map(name)?;
        let mut raw = disc.resources(name)?;
        let (centre, _) = world::core_bounds(&level_soup)
            .map(|(lo, hi)| ([0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k])), 0.0))
            .unwrap_or(([0.0; 3], 0.0));
        let mut x_cursor = centre[0];
        for m in &a.models {
            let id = map
                .tag(map::TagQuery::NameInGroup(&m.tag, map::FourCC(*b"mode")))
                .with_context(|| format!("mode tag {:?} in {name} (try --list-models)", m.tag))?
                .id;
            let model = tags::model::RenderModelGeometry::read(&mut map, id, &mut raw)?;
            let local = model_soup(&model, &sections_for(&model, a.lod))?;
            let (lo, hi) = world::soup_bounds(&local).context("model has no triangles")?;
            let at = match m.at {
                Some(p) => p,
                None => {
                    // Stand it on the level, models side by side along x.
                    let x = x_cursor - lo[0];
                    x_cursor += (hi[0] - lo[0]) * 1.5;
                    let y = centre[1];
                    let top = world::soup_bounds(&level_soup).map_or(centre[2], |b| b.1[2]);
                    let ground = world::ground_below(&level_soup, x, y, top).unwrap_or(centre[2]);
                    [x, y, ground - lo[2]]
                }
            };
            let mut lods = Vec::new();
            let mut seen: Vec<Vec<usize>> = Vec::new();
            for lod in 0..6 {
                let s = sections_for(&model, Some(lod));
                if seen.contains(&s) {
                    continue;
                }
                lods.push((format!("lod {lod}"), world::stats(&model_soup(&model, &s)?)));
                seen.push(s);
            }
            casters.push(Caster {
                name: m.tag.clone(),
                soup: world::place(&local, at, m.yaw),
                lods,
            });
        }
    }
    Ok(World {
        title: format!("{name} bsp {}", a.bsp),
        level: level_soup,
        level_pieces,
        casters,
    })
}

// ---- The comparison ----------------------------------------------------

struct Setup {
    camera: Camera,
    sun: Light,
    point: Light,
    range: f32,
    focus_radius: f32,
    casters_bounds: (V3, V3),
    receivers: Vec<gpu::SceneVertex>,
    casters: Vec<gpu::SceneVertex>,
    caster_mesh: TriMesh,
    caster_topo: Topology,
}

fn setup(w: &World, a: &Args) -> Result<Setup> {
    let (centre, r) = world::focus(w);
    let (eye, target) = a.camera.unwrap_or_else(|| {
        let (yaw, pitch) = (225f32.to_radians(), 32f32.to_radians());
        let d = r * 3.2;
        (
            add(
                centre,
                [
                    d * pitch.cos() * yaw.cos(),
                    d * pitch.cos() * yaw.sin(),
                    d * pitch.sin(),
                ],
            ),
            centre,
        )
    });
    let aspect = a.base.0 as f32 / a.base.1 as f32;
    let camera = Camera {
        eye,
        target,
        fovy: 60f32.to_radians(),
        aspect,
        near: (length(sub(target, eye)) * 0.01).max(0.005),
    };
    let (yaw, elev) = (a.sun.0.to_radians(), a.sun.1.to_radians());
    let sun = Light::Directional {
        dir: [
            -elev.cos() * yaw.cos(),
            -elev.cos() * yaw.sin(),
            -elev.sin(),
        ],
    };
    let ppos = a
        .point
        .unwrap_or_else(|| add(centre, [0.6 * r, -0.4 * r, 1.4 * r]));
    let range = a
        .range
        .unwrap_or_else(|| 4.0 * (ppos[2] - (centre[2] - r)).max(r));

    let mut caster_soup: Vec<Tri> = w
        .casters
        .iter()
        .flat_map(|c| c.soup.iter().copied())
        .collect();
    if a.level_casts {
        caster_soup.extend_from_slice(&w.level);
    }
    let caster_mesh = TriMesh::from_soup(&caster_soup, WELD_EPSILON);
    let caster_topo = Topology::build(&caster_mesh);
    let casters_bounds = world::soup_bounds(&caster_soup).unwrap_or(([0.0; 3], [0.0; 3]));
    let mut casters = Vec::new();
    for c in &w.casters {
        casters.extend(flat_vertices(&c.soup, [0.85, 0.7, 0.5]));
    }
    Ok(Setup {
        camera,
        sun,
        point: Light::Point { pos: ppos },
        range,
        focus_radius: r,
        casters_bounds,
        receivers: flat_vertices(&w.level, [0.75, 0.78, 0.82]),
        casters,
        caster_mesh,
        caster_topo,
    })
}

struct LightResult {
    name: &'static str,
    volume: VolumeStats,
    volume_tris: usize,
    build_ms: f64,
    per_scale: Vec<ScaleResult>,
}
struct ScaleResult {
    scale: u32,
    size: (u32, u32),
    map_size: u32,
    texel_world: f32,
    outputs: Vec<TechniqueOutput>,
}

fn shadow_for(
    s: &Setup,
    light: &Light,
    map: u32,
    cube: u32,
    scale: u32,
    max: u32,
) -> ShadowProjection {
    match *light {
        Light::Directional { dir } => {
            let size = (map * scale).min(max);
            let far = length(sub(s.camera.target, s.camera.eye)) * 2.5;
            let receivers = s.camera.frustum_corners(s.camera.near, far);
            let (matrix, texel_world) = smap::sun_matrix(dir, &receivers, s.casters_bounds, size);
            ShadowProjection::Sun {
                matrix,
                size,
                texel_world,
            }
        }
        Light::Point { pos } => {
            let size = (cube * scale).min(max);
            let far = s.range;
            let near = (far * 1e-3).max(0.01);
            let c = [0, 1, 2].map(|k| 0.5 * (s.casters_bounds.0[k] + s.casters_bounds.1[k]));
            let dist = length(sub(c, pos)).max(near);
            ShadowProjection::Cube {
                matrices: smap::cube_matrices(pos, near, far),
                size,
                texel_world: 2.0 * dist / size as f32,
            }
        }
    }
}

fn run_light(
    lab: &Lab,
    s: &Setup,
    a: &Args,
    name: &'static str,
    light: Light,
) -> Result<LightResult> {
    // The extraction is the per-frame CPU cost of volumes: median of five.
    let mut times = Vec::new();
    let mut vol = None;
    for _ in 0..5 {
        let t = Instant::now();
        vol = Some(Volume::build(&s.caster_mesh, &s.caster_topo, &light, true));
        times.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let vol = vol.expect("built");
    let build_ms = gpu::median(&times);
    let expanded = vol.expanded();
    let mut per_scale = Vec::new();
    for &scale in &a.scales {
        let size = (a.base.0 * scale, a.base.1 * scale);
        let shadow = shadow_for(s, &light, a.map, a.cube, scale, lab.max_texture);
        let proj = mul(
            geom::perspective_infinite(s.camera.fovy, s.camera.aspect, s.camera.near),
            s.camera.view(),
        );
        let run = LightRun {
            light,
            color: [0.95, 0.9, 0.8],
            range: s.range,
            volume: &expanded,
            shadow,
            normal_offset: shadow.texel_world() * 1.5,
            volume_offset: s.focus_radius * 1e-3,
        };
        let view = View {
            view_proj: proj,
            eye: s.camera.eye,
            size,
        };
        let outputs = lab.run(&view, &run, a.frames)?;
        per_scale.push(ScaleResult {
            scale,
            size,
            map_size: shadow.size(),
            texel_world: shadow.texel_world(),
            outputs,
        });
    }
    Ok(LightResult {
        name,
        volume: vol.stats,
        volume_tris: vol.tris.len(),
        build_ms,
        per_scale,
    })
}

fn output(r: &ScaleResult, t: Technique) -> &TechniqueOutput {
    r.outputs
        .iter()
        .find(|o| o.technique == t)
        .expect("every technique runs")
}

// ---- Report ----------------------------------------------------------------

fn topo_row(out: &mut String, label: &str, s: &TopoStats) {
    let pct = if s.triangles == 0 {
        0.0
    } else {
        100.0 * s.closed_triangles as f64 / s.triangles as f64
    };
    let _ = writeln!(
        out,
        "| {label} | {} | {} | {} | {} | {} | {} | {} | {}/{} | {pct:.1}% |",
        s.triangles,
        s.vertices,
        s.edges,
        s.open_edges,
        s.nonmanifold_edges,
        s.flipped_edges,
        s.degenerate,
        s.closed_components,
        s.components
    );
}

fn report(w: &World, s: &Setup, a: &Args, lab: &Lab, lights: &[LightResult]) -> (String, String) {
    let mut md = String::new();
    let clock = match lab.clock {
        Clock::Gpu => "GPU timestamps at pass boundaries",
        Clock::Wall => "wall time per technique (no timestamp queries on this adapter)",
    };
    let _ = writeln!(md, "# shadowlab: {}\n", w.title);
    let _ = writeln!(
        md,
        "Adapter: {}. Timing: {clock}, median of {} frames. Base resolution {}x{}. \
         Level casts: {}.\n",
        lab.adapter_name, a.frames, a.base.0, a.base.1, a.level_casts
    );
    let _ = writeln!(md, "## Geometry\n");
    let _ = writeln!(
        md,
        "Welded by position (epsilon {WELD_EPSILON}). Closed means every edge has exactly two \
         oppositely wound triangles.\n"
    );
    let _ = writeln!(
        md,
        "| mesh | triangles | vertices | edges | open | non-manifold | flipped | degenerate | closed pieces | closed tris |"
    );
    let _ = writeln!(md, "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    topo_row(&mut md, "level (all, welded)", &world::stats(&w.level));
    for (label, st) in &w.level_pieces {
        topo_row(&mut md, &format!("level: {label}"), st);
    }
    for c in &w.casters {
        for (lod, st) in &c.lods {
            topo_row(&mut md, &format!("{} {lod}", c.name), st);
        }
    }
    topo_row(
        &mut md,
        "all casters (welded together)",
        &s.caster_topo.stats,
    );

    let _ = writeln!(md, "\n## Volumes\n");
    let _ = writeln!(
        md,
        "| light | lit faces | silhouette edges | side quads | of which open edges | cap tris | volume tris | CPU build ms |"
    );
    let _ = writeln!(md, "|---|---:|---:|---:|---:|---:|---:|---:|");
    for l in lights {
        let v = &l.volume;
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {} | {:.3} |",
            l.name,
            v.lit_faces,
            v.silhouette_edges,
            v.sides,
            v.open_sides,
            v.caps,
            l.volume_tris,
            l.build_ms
        );
    }

    let _ = writeln!(md, "\n## Timing (ms)\n");
    let _ = writeln!(
        md,
        "| light | scale | target | map | texel (world) | technique | passes | total | over none |"
    );
    let _ = writeln!(md, "|---|---:|---|---:|---:|---|---|---:|---:|");
    let mut csv = String::from("light,scale,width,height,map,technique,pass,ms\n");
    for l in lights {
        for r in &l.per_scale {
            let none = output(r, Technique::None).total_ms();
            for o in &r.outputs {
                let passes = o
                    .passes
                    .iter()
                    .map(|(n, ms)| format!("{n} {ms:.3}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(
                    md,
                    "| {} | {}x | {}x{} | {} | {:.4} | {} | {passes} | {:.3} | {:+.3} |",
                    l.name,
                    r.scale,
                    r.size.0,
                    r.size.1,
                    r.map_size,
                    r.texel_world,
                    o.technique.name(),
                    o.total_ms(),
                    o.total_ms() - none
                );
                for (n, ms) in &o.passes {
                    let _ = writeln!(
                        csv,
                        "{},{},{},{},{},{},{n},{ms:.4}",
                        l.name,
                        r.scale,
                        r.size.0,
                        r.size.1,
                        r.map_size,
                        o.technique.name()
                    );
                }
            }
        }
    }

    let _ = writeln!(md, "\n## Agreement\n");
    let _ = writeln!(
        md,
        "Pixels facing the light whose lit/shadowed verdict differs, as a share of those pixels.\n"
    );
    let _ = writeln!(
        md,
        "| light | scale | volume vs map-hard | volume vs map-pcf | map-hard vs map-pcf | volume-only shadow | map-hard-only shadow |"
    );
    let _ = writeln!(md, "|---|---:|---:|---:|---:|---:|---:|");
    for l in lights {
        for r in &l.per_scale {
            let (v, h, p) = (
                &output(r, Technique::Volume).mask,
                &output(r, Technique::MapHard).mask,
                &output(r, Technique::MapPcf).mask,
            );
            let vh = compare::compare(v, h);
            let _ = writeln!(
                md,
                "| {} | {}x | {:.2}% | {:.2}% | {:.2}% | {} | {} |",
                l.name,
                r.scale,
                vh.percent(),
                compare::compare(v, p).percent(),
                compare::compare(h, p).percent(),
                vh.a_only,
                vh.b_only
            );
        }
    }
    let _ = writeln!(
        md,
        "\nImages: `<light>-<scale>x-<technique>.png`, and `<light>-<scale>x-grid.png` with \
         volume, map-hard / map-pcf, and the volume-vs-map-hard mask difference \
         (red: shadowed only by the volume, cyan: only by the map, black: faces away)."
    );
    (md, csv)
}

fn write_outputs(
    dir: &Path,
    force: bool,
    lights: &[LightResult],
    md: &str,
    csv: &str,
) -> Result<Vec<PathBuf>> {
    out::ensure_dir(dir)?;
    let mut written = Vec::new();
    let mut png = |name: String, w: u32, h: u32, px: &[u8]| -> Result<()> {
        let p = dir.join(name);
        out::write_with(&p, force, |f| assets::png::write_rgba(f, w, h, px))
            .with_context(|| format!("write {}", p.display()))?;
        written.push(p);
        Ok(())
    };
    for l in lights {
        for r in &l.per_scale {
            let (w, h) = r.size;
            for o in &r.outputs {
                png(
                    format!("{}-{}x-{}.png", l.name, r.scale, o.technique.name()),
                    w,
                    h,
                    &o.color,
                )?;
            }
            let diff = compare::diff_image(
                &output(r, Technique::Volume).mask,
                &output(r, Technique::MapHard).mask,
            );
            let tiles = [
                output(r, Technique::Volume).color.as_slice(),
                output(r, Technique::MapHard).color.as_slice(),
                output(r, Technique::MapPcf).color.as_slice(),
                diff.as_slice(),
            ];
            let (gw, gh, grid) = compare::grid(&tiles, w as usize, h as usize, 2, 4);
            png(
                format!("{}-{}x-grid.png", l.name, r.scale),
                gw as u32,
                gh as u32,
                &grid,
            )?;
        }
    }
    for (name, text) in [("report.md", md), ("timings.csv", csv)] {
        let p = dir.join(name);
        out::write_with(&p, force, |mut f| {
            std::io::Write::write_all(&mut f, text.as_bytes())
        })
        .with_context(|| format!("write {}", p.display()))?;
        written.push(p);
    }
    Ok(written)
}

fn list_models(a: &Args) -> Result<()> {
    let name = a.map_name.as_deref().context(USAGE)?;
    let disc = disc::Disc::open(&disc::disc_path(a.disc.clone())?)?;
    let map = disc.open_map(name)?;
    let mut names: Vec<&str> = map
        .tags()
        .iter()
        .filter(|t| t.group == map::FourCC(*b"mode"))
        .filter_map(|t| t.name.as_deref())
        .collect();
    names.sort_unstable();
    for n in names {
        println!("{n}");
    }
    Ok(())
}

fn main() -> Result<()> {
    let a = parse(std::env::args().skip(1))?;
    if a.list_models {
        return list_models(&a);
    }
    let out = a
        .out
        .clone()
        .with_context(|| format!("--out DIR is required\n{USAGE}"))?;
    if inside(&out, &repo_root())? {
        bail!(
            "{} is inside the repository; write the comparison somewhere else \
             (nothing derived from the disc may be committed)",
            out.display()
        );
    }
    let w = load_world(&a)?;
    if w.casters.is_empty() && !a.level_casts {
        bail!("nothing casts: give --model TAG (see --list-models) or --level-casts");
    }
    let s = setup(&w, &a)?;
    let mut lab = Lab::new()?;
    lab.set_scene(&s.receivers, &s.casters, a.level_casts);
    println!(
        "{}: {} level triangles, {} caster triangles; adapter {}",
        w.title,
        w.level.len(),
        s.caster_mesh.tris.len(),
        lab.adapter_name
    );
    let mut lights = Vec::new();
    for (name, light) in [("sun", s.sun), ("point", s.point)] {
        let r = run_light(&lab, &s, &a, name, light)?;
        for sc in &r.per_scale {
            let line = sc
                .outputs
                .iter()
                .map(|o| format!("{} {:.3}", o.technique.name(), o.total_ms()))
                .collect::<Vec<_>>()
                .join("  ");
            println!("{name:<5} {}x  {line}", sc.scale);
        }
        lights.push(r);
    }
    let (md, csv) = report(&w, &s, &a, &lab, &lights);
    let files = write_outputs(&out, a.force, &lights, &md, &csv)?;
    println!("wrote {} files to {}", files.len(), out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Result<Args> {
        parse(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_models_lights_and_scales() {
        let a = args(&[
            "--out",
            "x",
            "--model",
            "a\\b@1,2,3,90",
            "--model",
            "c",
            "--sun",
            "10,45",
            "--scales",
            "1,2",
            "zanzibar",
        ])
        .unwrap();
        assert_eq!(a.models.len(), 2);
        assert_eq!(a.models[0].tag, "a\\b");
        assert_eq!(a.models[0].at, Some([1.0, 2.0, 3.0]));
        assert!((a.models[0].yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(a.models[1].at, None);
        assert_eq!(a.sun, (10.0, 45.0));
        assert_eq!(a.scales, [1, 2]);
        assert_eq!(a.map_name.as_deref(), Some("zanzibar"));
    }

    #[test]
    fn rejects_bad_values() {
        for bad in [
            &["--sun", "1"][..],
            &["--sun", "0,0"],
            &["--sun", "0,NaN"],
            &["--scales", "0"],
            &["--scales", ""],
            &["--map", "5"],
            &["--frames", "0"],
            &["--camera", "1,1,1,1,1,1"],
            &["--model", "x@1,2"],
            &["--bogus"],
            &["a", "b"],
        ] {
            assert!(args(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn output_inside_the_repository_is_refused() {
        let root = repo_root();
        assert!(inside(&root.join("not-yet/made/out"), &root).unwrap());
        assert!(inside(&root, &root).unwrap());
        let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        assert!(!inside(&tmp.join("shadowlab-out"), &root).unwrap());
    }

    /// Pixel of a world point in a run's images.
    fn pixel(view: &View, p: V3) -> usize {
        let n = geom::project(&view.view_proj, p);
        let [u, v] = smap::ndc_to_uv(n);
        let (x, y) = (
            (u * view.size.0 as f32) as usize,
            (v * view.size.1 as f32) as usize,
        );
        y * view.size.0 as usize + x
    }

    /// The synthetic scene end to end on the GPU (skipped without an adapter).
    #[test]
    fn synthetic_scene_volumes_and_maps_agree_where_casters_are_closed() {
        let Ok(mut lab) = Lab::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let w = world::synthetic();
        let mut a = args(&[
            "--out",
            "unused",
            "--synthetic",
            "--frames",
            "1",
            "--scales",
            "1",
        ])
        .unwrap();
        a.base = (320, 240);
        a.map = 1024;
        a.cube = 512;
        // Sun from +y at 45 degrees, so shadows fall towards the camera (-y)
        // and stay visible beside their casters.
        a.sun = (90.0, 45.0);
        a.camera = Some(([0.0, -14.0, 16.0], [0.0, 0.0, 0.0]));
        let s = setup(&w, &a).unwrap();
        lab.set_scene(&s.receivers, &s.casters, false);
        let sun = run_light(&lab, &s, &a, "sun", s.sun).unwrap();
        let r = &sun.per_scale[0];
        let view = View {
            view_proj: mul(
                geom::perspective_infinite(s.camera.fovy, s.camera.aspect, s.camera.near),
                s.camera.view(),
            ),
            eye: s.camera.eye,
            size: r.size,
        };
        let mask = |t| &output(r, t).mask;
        let at = |t, p| mask(t)[pixel(&view, p)];
        use compare::{LIT, SHADOWED};
        for t in [Technique::Volume, Technique::MapHard, Technique::MapPcf] {
            // The closed cube's shadow, and open ground beyond it.
            assert_eq!(at(t, [-4.0, -2.5, 0.0]), SHADOWED, "{t:?} cube shadow");
            assert_eq!(at(t, [-4.0, -6.0, 0.0]), LIT, "{t:?} open ground");
            // Light through the hole in the raised plane, and its solid rim.
            assert_eq!(at(t, [4.0, -2.5, 0.0]), LIT, "{t:?} through the hole");
            assert_eq!(at(t, [3.0, -2.5, 0.0]), SHADOWED, "{t:?} plane rim");
        }
        assert_eq!(at(Technique::None, [-4.0, -2.5, 0.0]), LIT);
        // The open box: light enters the open top and meets the inside of
        // the far (-y) wall, a face turned away from the light. A volume is
        // cast by lit faces only, so it lets that light through; the map
        // stores whatever is nearest the light and shadows it.
        assert_eq!(at(Technique::Volume, [0.0, -2.5, 0.0]), LIT);
        assert_eq!(at(Technique::MapHard, [0.0, -2.5, 0.0]), SHADOWED);
        // Elsewhere the two hard techniques agree almost everywhere.
        let d = compare::compare(mask(Technique::Volume), mask(Technique::MapHard));
        assert!(d.compared > 10_000, "{d:?}");
        assert!(d.a_only < d.b_only, "{d:?}");
        assert!(d.percent() < 6.0, "{d:?}");

        // A point light over the cube: its shadow spreads wider than the cube.
        let point = Light::Point {
            pos: [-4.0, 0.0, 6.0],
        };
        let pr = run_light(&lab, &s, &a, "point", point).unwrap();
        let r = &pr.per_scale[0];
        let at = |t, p| output(r, t).mask[pixel(&view, p)];
        for t in [Technique::Volume, Technique::MapHard, Technique::MapPcf] {
            assert_eq!(at(t, [-4.0, -1.5, 0.0]), SHADOWED, "{t:?} cube (point)");
            assert_eq!(at(t, [-4.0, -6.0, 0.0]), LIT, "{t:?} far ground (point)");
        }
        // Every technique's timings were taken.
        assert!(r.outputs.iter().all(|o| !o.passes.is_empty()));

        // The report and files, into a scratch directory outside the repository.
        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("shadowlab-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let lights = [sun, pr];
        let (md, csv) = report(&w, &s, &a, &lab, &lights);
        for heading in ["## Geometry", "## Volumes", "## Timing", "## Agreement"] {
            assert!(md.contains(heading), "{heading}");
        }
        assert!(md.contains("| open box lod 0 | 10 |"), "{md}");
        if lab.clock == Clock::Gpu {
            // Per light: none 1 pass, volume 3, each map 2 (faces summed).
            assert_eq!(csv.lines().count(), 1 + 2 * (1 + 3 + 2 + 2));
        }
        let files = write_outputs(&dir, false, &lights, &md, &csv).unwrap();
        // Four techniques and a grid per light and scale, plus two text files.
        assert_eq!(files.len(), 2 * 5 + 2);
        assert!(files.iter().all(|f| f.is_file()));
        assert!(
            write_outputs(&dir, false, &lights, &md, &csv).is_err(),
            "never clobbers"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
