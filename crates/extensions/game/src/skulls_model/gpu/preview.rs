//! A preview of the model picture from your disc, for a look before
//! landing, without the game: the skull in three states as the page
//! shows it in a 640 by 480, a 1280 by 960 and a 3840 by 2160 window
//! (drawn and composited as the game does, `showcase::Fit`), the model
//! from four sides with a glyph (where the forehead decal lands), a
//! sheet of the model as authored under each of the 24 axis-aligned
//! orientations, sheets of yaw by pitch and of roll about the orientation
//! in use, a sheet of candidate orientations with the glyph, and a sheet
//! of every glyph. `H2_SKULL_YAW`, `H2_SKULL_PITCH` and `H2_SKULL_ROLL`
//! (degrees) override the fine-tune constants here only, not in the
//! game; `H2_SKULL_CANDIDATES` (`yaw/pitch/roll,...`) adds candidates to
//! [`CANDIDATES`]. Writes only to `H2_SKULLS_PREVIEW_DIR`, which must be
//! outside the repository (disc-derived pixels are never committed);
//! besides the pictures it writes the model's vertices and triangles
//! (`mesh.txt`) and its base maps there, for working an orientation out
//! from the geometry. Prints the map and tag it loaded and the model's
//! bounds.
//!
//! `H2_DISC=/path/halo2.iso H2_SKULLS_PREVIEW_DIR=/somewhere cargo test -p
//! boot --lib skulls_model::gpu::preview -- --ignored --nocapture`

use super::*;
use crate::skulls::{ABOUT, SkullTier};
use crate::ui_kit::preview::Canvas;
use std::path::{Path, PathBuf};

/// The menu's dark blue, behind the pictures.
const BACKGROUND: [u8; 4] = [12, 22, 38, 255];

fn slug(name: &str) -> String {
    name.to_ascii_lowercase().replace(' ', "-")
}

/// `rgba` (premultiplied) over [`BACKGROUND`].
fn over_background(rgba: &[u8], width: u32, height: u32) -> Canvas {
    let mut c = Canvas::new(width, height, BACKGROUND);
    for (o, p) in c.rgba.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        let a = u32::from(p[3]);
        for k in 0..3 {
            o[k] = (u32::from(p[k]) + u32::from(o[k]) * (255 - a) / 255).min(255) as u8;
        }
    }
    c
}

/// `text` with only what [`Canvas::label`]'s font draws (letters, digits,
/// space and `-./_`): an apostrophe or anything else is dropped.
fn label_text(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || " -./_".contains(*c))
        .collect()
}

/// `text` split at spaces into lines of at most `width` characters.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.len() + 1 + word.len() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

/// Sets `look`'s glyph and answers how the showcase draws it at `yaw`.
fn shown(ctx: &gfx::GfxContext, showcase: &mut Showcase, look: &Look, yaw: f32) -> showcase::Look {
    match look.glyph() {
        Some(strokes) => {
            let field = skull_glyphs::distance_field(strokes, skull_glyphs::FIELD);
            showcase.set_glyph(&ctx.device, &ctx.queue, skull_glyphs::FIELD, &field);
        }
        None => showcase.set_glyph(&ctx.device, &ctx.queue, 0, &[]),
    }
    showcase::Look {
        yaw,
        tint: look.tint(),
        silhouette: look.silhouette(),
        glyph: look.glyph().is_some(),
    }
}

/// The picture alone, `size` texels, over the background.
fn render(
    ctx: &gfx::GfxContext,
    showcase: &mut Showcase,
    look: &Look,
    yaw: f32,
    size: (u32, u32),
) -> Canvas {
    let shown = shown(ctx, showcase, look, yaw);
    draw(ctx, showcase, &shown, size)
}

/// The picture as the page shows it in a `window` sized target: the
/// picture box's texels there, the picture drawn at its
/// [`showcase::Fit`] and composited over the background as the game's
/// pass does. The canvas is the box alone.
fn seen(
    ctx: &gfx::GfxContext,
    showcase: &mut Showcase,
    look: &Look,
    yaw: f32,
    window: (u32, u32),
) -> Canvas {
    let [_, _, w, h] = crate::skulls_model::guest_rect(crate::skulls_menu::PICTURE_BOX, window);
    let fit = showcase::Fit::of([0.0, 0.0, w, h]).expect("a box");
    let (width, height) = (fit.rect[2] as u32, fit.rect[3] as u32);
    let shown = shown(ctx, showcase, look, yaw);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("skulls preview"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let channel = |k: usize| f64::from(BACKGROUND[k]) / 255.0;
    drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("skulls preview"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: channel(0),
                    g: channel(1),
                    b: channel(2),
                    a: 1.0,
                }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    assert!(showcase.draw(&ctx.device, &mut enc, fit.width, fit.height, &shown));
    assert!(showcase.composite(&ctx.device, &mut enc, &view, format, fit.rect, 1.0));
    ctx.queue.submit([enc.finish()]);
    let rgba =
        gfx::readback::read_texture(ctx, &target, wgpu::TextureAspect::All, 4, (width, height));
    println!(
        "  a {} by {} window: the box is {width} by {height}, the picture {} by {}",
        window.0, window.1, fit.width, fit.height
    );
    Canvas {
        width,
        height,
        rgba,
    }
}

/// The model's geometry and base maps, for working its orientation out:
/// `mesh.txt` holds a `v x y z nx ny nz u v` line a vertex (as authored)
/// and a `t a b c part` line a triangle; `base-<material>.png` each base
/// map.
fn dump(skull: &Skull, dir: &Path) {
    use std::fmt::Write as _;
    let mesh = &skull.rest;
    let mut text = String::new();
    for v in &mesh.vertices {
        let ([x, y, z], [nx, ny, nz], [tu, tv]) = (v.position, v.normal, v.uv);
        writeln!(text, "v {x} {y} {z} {nx} {ny} {nz} {tu} {tv}").unwrap();
    }
    for (n, part) in mesh.parts.iter().enumerate() {
        let range = part.first_index as usize..(part.first_index + part.index_count) as usize;
        for t in mesh.indices[range].chunks_exact(3) {
            writeln!(text, "t {} {} {} {n}", t[0], t[1], t[2]).unwrap();
        }
    }
    let path = dir.join("mesh.txt");
    std::fs::write(&path, text).unwrap();
    println!("wrote {}", path.display());
    for (material, image) in &skull.bases {
        let canvas = Canvas {
            width: image.width,
            height: image.height,
            rgba: image.rgba.clone(),
        };
        write(&canvas, dir, &format!("base-{material}.png"));
    }
}

fn draw(
    ctx: &gfx::GfxContext,
    showcase: &mut Showcase,
    shown: &showcase::Look,
    (width, height): (u32, u32),
) -> Canvas {
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    assert!(showcase.draw(&ctx.device, &mut enc, width, height, shown));
    ctx.queue.submit([enc.finish()]);
    let texture = showcase.picture().expect("drawn");
    let rgba =
        gfx::readback::read_texture(ctx, texture, wgpu::TextureAspect::All, 4, (width, height));
    over_background(&rgba, width, height)
}

fn write(canvas: &Canvas, dir: &Path, name: &str) {
    let path = dir.join(name);
    canvas.write_png(&path).unwrap();
    println!("wrote {}", path.display());
}

/// Every glyph on a grid, its number and name under it.
fn glyph_sheet(dir: &Path) {
    const COLUMNS: u32 = 7;
    const CELL: (u32, u32) = (232, 168);
    const GLYPH: u32 = 128;
    let rows = (ABOUT.len() as u32).div_ceil(COLUMNS);
    let mut sheet = Canvas::new(COLUMNS * CELL.0, rows * CELL.1, BACKGROUND);
    for (i, about) in ABOUT.iter().enumerate() {
        let (cx, cy) = ((i as u32 % COLUMNS) * CELL.0, (i as u32 / COLUMNS) * CELL.1);
        let mask = skull_glyphs::rasterise(skull_glyphs::glyph(i).expect("a glyph"), GLYPH);
        let left = cx + (CELL.0 - GLYPH) / 2;
        for (n, cov) in mask.iter().enumerate() {
            let (x, y) = (left + n as u32 % GLYPH, cy + 8 + n as u32 / GLYPH);
            let o = ((y * sheet.width + x) * 4) as usize;
            for k in 0..3 {
                let bg = u32::from(BACKGROUND[k]);
                sheet.rgba[o + k] = (bg + (255 - bg) * u32::from(*cov) / 255) as u8;
            }
        }
        // The label font is 8 pixels a character: two lines at most.
        let text = label_text(&format!("{i} {}", about.name));
        for (n, line) in wrap(&text, (CELL.0 / 8 - 1) as usize)
            .iter()
            .take(2)
            .enumerate()
        {
            sheet.label(
                line,
                cx + 4,
                cy + 8 + GLYPH + 6 + 12 * n as u32,
                [200, 210, 230],
            );
        }
    }
    write(&sheet, dir, "glyphs.png");
}

/// One sheet cell: the model as authored turned by `orientation`, its
/// label, and whether it is the orientation in use (labelled in gold).
struct Cell {
    orientation: Orientation,
    label: String,
    chosen: bool,
}

/// `cells` on a grid `columns` wide, each the picture's size with a label
/// band under it, drawn as `shown` says with no sway; written as `name`.
/// The showcase is left holding the picture's own mesh.
#[allow(clippy::too_many_arguments)]
fn sheet(
    ctx: &gfx::GfxContext,
    showcase: &mut Showcase,
    skull: &Skull,
    shown: &showcase::Look,
    picture: (u32, u32),
    columns: u32,
    cells: &[Cell],
    dir: &Path,
    name: &str,
) {
    const BAND: u32 = 22;
    let rows = (cells.len() as u32).div_ceil(columns);
    let cell = (picture.0, picture.1 + BAND);
    let mut sheet = Canvas::new(columns * cell.0, rows * cell.1, BACKGROUND);
    println!("{name}, left to right, top to bottom:");
    for (i, c) in cells.iter().enumerate() {
        let (cx, cy) = ((i as u32 % columns) * cell.0, (i as u32 / columns) * cell.1);
        let mesh = skull.rest.oriented(&c.orientation);
        showcase.set_mesh(&ctx.device, &ctx.queue, &mesh, &skull.bases);
        let canvas = draw(ctx, showcase, shown, picture);
        sheet.blit(&canvas, cx, cy);
        let colour = if c.chosen {
            [250, 210, 110]
        } else {
            [200, 210, 230]
        };
        sheet.label(&label_text(&c.label), cx + 4, cy + picture.1 + 6, colour);
        println!(
            "  {i:2}: {}{}",
            c.label,
            if c.chosen { "  (in use)" } else { "" }
        );
    }
    write(&sheet, dir, name);
    showcase.set_mesh(&ctx.device, &ctx.queue, &skull.mesh, &skull.bases);
}

/// How a turned skull sits in the picture's space (x to the camera, y
/// across, z up), measured on its drawn vertices against its bounds.
#[derive(Clone, Copy, Debug)]
struct Facing {
    /// The mean distance from each vertex, mirrored left to right through
    /// the bounds' middle, to the nearest vertex, over the bounds' width:
    /// small only when the skull's own mirror plane is the picture's.
    mirror: f32,
    /// The teeth (the second part): their middle across, from the bounds'
    /// middle, over the width; their front, from the middle, over the
    /// depth (0.5 is the bounds' front); their middle's height, from the
    /// middle, over the height (-0.5 is the bottom).
    teeth_across: f32,
    teeth_front: f32,
    teeth_height: f32,
}

impl Facing {
    /// `None` for a mesh with no second part.
    fn of(mesh: &ShowcaseMesh) -> Option<Self> {
        let position = |i: &u32| mesh.vertices[*i as usize].position;
        let mut drawn: Vec<u32> = mesh.indices.clone();
        drawn.sort_unstable();
        drawn.dedup();
        let points: Vec<[f32; 3]> = drawn.iter().map(position).collect();
        let b = mesh.bounds;
        let centre = [0, 1, 2].map(|k| 0.5 * (b.min[k] + b.max[k]));
        let size = [0, 1, 2].map(|k| (b.max[k] - b.min[k]).max(f32::EPSILON));
        let nearest = |p: [f32; 3]| {
            points
                .iter()
                .map(|q| {
                    let d = [0, 1, 2].map(|k| p[k] - q[k]);
                    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
                })
                .fold(f32::INFINITY, f32::min)
        };
        let mirror = points
            .iter()
            .map(|p| nearest([p[0], 2.0 * centre[1] - p[1], p[2]]))
            .sum::<f32>()
            / points.len() as f32
            / size[1];
        let part = mesh.parts.get(1)?;
        let range = part.first_index as usize..(part.first_index + part.index_count) as usize;
        let mut teeth: Vec<u32> = mesh.indices.get(range)?.to_vec();
        teeth.sort_unstable();
        teeth.dedup();
        let teeth: Vec<[f32; 3]> = teeth.iter().map(position).collect();
        let mean = |k: usize| teeth.iter().map(|p| p[k]).sum::<f32>() / teeth.len() as f32;
        let front = teeth.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
        Some(Self {
            mirror,
            teeth_across: (mean(1) - centre[1]) / size[1],
            teeth_front: (front - centre[0]) / size[0],
            teeth_height: (mean(2) - centre[2]) / size[2],
        })
    }

    /// Upright and face-on: the mirror plane is the picture's middle, the
    /// teeth are in the middle across, at the front and near the bottom.
    /// None of the 24 axis-aligned orientations is.
    fn upright(&self) -> bool {
        self.mirror < 0.04
            && self.teeth_across.abs() < 0.03
            && self.teeth_front > 0.3
            && self.teeth_height < -0.35
    }
}

/// Orientations worth a look side by side, as `(yaw, pitch, roll)`
/// degrees from `MODEL_FORWARD` and `MODEL_UP`: `candidates.png` shows
/// each with Iron's glyph, the one in use first. One that is not
/// [`Facing::upright`] is left off the sheet and said.
///
/// These look at the face from 4 degrees higher and 4 and 8 degrees
/// lower than the one in use, about the model's side-to-side axis.
const CANDIDATES: &[(f32, f32, f32)] = &[(35.1, 38.0, 11.7), (39.3, 44.3, 5.3), (42.0, 47.4, 1.5)];

/// [`CANDIDATES`], then `H2_SKULL_CANDIDATES`'s (`yaw/pitch/roll`
/// triples, comma separated).
fn candidates() -> Vec<(f32, f32, f32)> {
    let mut out = CANDIDATES.to_vec();
    if let Ok(text) = std::env::var("H2_SKULL_CANDIDATES") {
        for triple in text.split(',').filter(|t| !t.trim().is_empty()) {
            let angles: Vec<f32> = triple
                .split('/')
                .map(|a| {
                    a.trim().parse().unwrap_or_else(|_| {
                        panic!("H2_SKULL_CANDIDATES: {triple:?} is not yaw/pitch/roll")
                    })
                })
                .collect();
            let [yaw, pitch, roll] = angles[..] else {
                panic!("H2_SKULL_CANDIDATES: {triple:?} is not yaw/pitch/roll");
            };
            out.push((yaw, pitch, roll));
        }
    }
    out
}

/// The 24 axis-aligned orientations (`orientations.png`), to choose
/// `MODEL_FORWARD` and `MODEL_UP`; then, about `in_use`, yaw by pitch
/// (`orientations-fine.png`) and roll (`orientations-roll.png`) 30
/// degrees either way in steps of 10, to check the fine-tune (the middle
/// cell is `in_use`, labelled in gold); and `in_use` beside the
/// candidates, each twice the picture's size with `marked`'s glyph
/// (`candidates.png`).
fn orientation_sheets(
    ctx: &gfx::GfxContext,
    showcase: &mut Showcase,
    skull: &Skull,
    marked: &Look,
    picture: (u32, u32),
    in_use: Orientation,
    dir: &Path,
) {
    showcase.set_glyph(&ctx.device, &ctx.queue, 0, &[]);
    let plain = showcase::Look {
        yaw: 0.0,
        tint: marked.tint(),
        silhouette: false,
        glyph: false,
    };
    let axes: Vec<Cell> = Orientation::all()
        .into_iter()
        .enumerate()
        .map(|(i, o)| Cell {
            label: format!("{i} {}", o.label()),
            chosen: o.forward == in_use.forward && o.up == in_use.up,
            orientation: o,
        })
        .collect();
    sheet(
        ctx,
        showcase,
        skull,
        &plain,
        picture,
        6,
        &axes,
        dir,
        "orientations.png",
    );

    let offsets: Vec<f32> = (-3..=3).map(|k| k as f32 * 10.0).collect();
    let mut fine = Vec::new();
    for dp in &offsets {
        for dy in &offsets {
            let (yaw, pitch) = (in_use.yaw + dy, in_use.pitch + dp);
            fine.push(Cell {
                orientation: Orientation {
                    yaw,
                    pitch,
                    ..in_use
                },
                label: format!("yaw {yaw:.0} pitch {pitch:.0}"),
                chosen: *dy == 0.0 && *dp == 0.0,
            });
        }
    }
    sheet(
        ctx,
        showcase,
        skull,
        &plain,
        picture,
        7,
        &fine,
        dir,
        "orientations-fine.png",
    );
    let roll: Vec<Cell> = offsets
        .iter()
        .map(|dr| {
            let roll = in_use.roll + dr;
            Cell {
                orientation: Orientation { roll, ..in_use },
                label: format!("roll {roll:.0}"),
                chosen: *dr == 0.0,
            }
        })
        .collect();
    sheet(
        ctx,
        showcase,
        skull,
        &plain,
        picture,
        7,
        &roll,
        dir,
        "orientations-roll.png",
    );

    let mut cells = vec![Cell {
        orientation: in_use,
        label: format!(
            "in use {:.1}/{:.1}/{:.1}",
            in_use.yaw, in_use.pitch, in_use.roll
        ),
        chosen: true,
    }];
    for (yaw, pitch, roll) in candidates() {
        let orientation = Orientation {
            yaw,
            pitch,
            roll,
            ..in_use
        };
        let facing = Facing::of(&skull.rest.oriented(&orientation));
        println!("candidate {yaw}/{pitch}/{roll}: {facing:?}");
        if facing.is_some_and(|f| f.upright()) {
            cells.push(Cell {
                orientation,
                label: format!("{yaw:.1}/{pitch:.1}/{roll:.1}"),
                chosen: false,
            });
        } else {
            println!("  not upright and face-on: left off candidates.png");
        }
    }
    let with_glyph = shown(ctx, showcase, marked, 0.0);
    sheet(
        ctx,
        showcase,
        skull,
        &with_glyph,
        (picture.0 * 2, picture.1 * 2),
        4,
        &cells,
        dir,
        "candidates.png",
    );
}

/// `name` from the environment as degrees, or `default` when unset: the
/// preview's fine-tune overrides (H2_SKULL_YAW, H2_SKULL_PITCH,
/// H2_SKULL_ROLL), so the angles can be tried without a code change. The
/// game reads only the constants.
fn angle(name: &str, default: f32) -> f32 {
    match std::env::var(name) {
        Ok(text) => {
            let value: f32 = text
                .trim()
                .parse()
                .unwrap_or_else(|_| panic!("{name}={text:?} is not a number of degrees"));
            println!("{name}={value} overrides the constant's {default}");
            value
        }
        Err(_) => default,
    }
}

#[test]
#[ignore = "reads the disc named by H2_DISC; writes PNGs to H2_SKULLS_PREVIEW_DIR"]
fn preview() {
    let disc = PathBuf::from(std::env::var_os("H2_DISC").expect("set H2_DISC to the disc image"));
    let dir = PathBuf::from(
        std::env::var_os("H2_SKULLS_PREVIEW_DIR")
            .expect("set H2_SKULLS_PREVIEW_DIR (outside the repository)"),
    );
    std::fs::create_dir_all(&dir).unwrap();
    glyph_sheet(&dir);

    // The VFS's storage goes under the output and is removed after.
    let data = dir.join(".vfs-data");
    let config = vfs::Config::new(data.clone(), 0x4D53_0064);
    let vfs = Mutex::new(vfs::Vfs::with_disc(config, &disc).expect("mount the disc"));
    let skull = load_with(&|name: &str| crate::native_map::open_from_vfs(&vfs, name));
    drop(vfs);
    let _ = std::fs::remove_dir_all(&data);
    let mut skull = skull.expect("the skull model");
    dump(&skull, &dir);
    let orientation = Orientation {
        yaw: angle("H2_SKULL_YAW", MODEL_YAW_DEGREES),
        pitch: angle("H2_SKULL_PITCH", MODEL_PITCH_DEGREES),
        roll: angle("H2_SKULL_ROLL", MODEL_ROLL_DEGREES),
        ..MODEL_ORIENTATION
    };
    skull.mesh = skull.rest.oriented(&orientation);
    let facing = Facing::of(&skull.mesh).expect("the skull has its teeth as a second part");
    println!("the orientation in use: {facing:?}");
    let axis_aligned_upright = Orientation::all()
        .iter()
        .filter(|o| Facing::of(&skull.rest.oriented(o)).is_some_and(|f| f.upright()))
        .count();
    println!("  {axis_aligned_upright} of the 24 axis-aligned orientations pass the same check");
    // The constants must pass; an override is a try and may not.
    assert!(
        facing.upright() || orientation != MODEL_ORIENTATION,
        "the orientation in use is not upright and face-on: {facing:?}"
    );
    let b = skull.mesh.bounds;
    let placement = showcase::Placement::of(&b).expect("a placement");
    println!("loaded {}", skull.source);
    println!(
        "  {} vertices, {} triangles",
        skull.mesh.vertices.len(),
        skull.mesh.indices.len() / 3,
    );
    for part in &skull.mesh.parts {
        let base = skull
            .bases
            .iter()
            .find(|(m, _)| *m == part.material)
            .map_or("none, drawn in the bone tint".to_owned(), |(_, i)| {
                format!("{}x{}", i.width, i.height)
            });
        println!(
            "  part: material {}, {} triangles, base map {base}",
            part.material,
            part.index_count / 3
        );
    }
    let r = skull.rest.bounds;
    println!(
        "  bounds as authored x {:.3}..{:.3}, y {:.3}..{:.3}, z {:.3}..{:.3}",
        r.min[0], r.max[0], r.min[1], r.max[1], r.min[2], r.max[2]
    );
    println!(
        "  facing: {} yaw {} pitch {} roll {} degrees (MODEL_FORWARD, MODEL_UP and the \
         MODEL_*_DEGREES constants or their H2_SKULL_* overrides); turned, bounds x \
         {:.3}..{:.3}, y {:.3}..{:.3}, z {:.3}..{:.3}. The picture shows the turned model's +x \
         to the camera, its +y on the picture's right, +z up. The glyph is centred at ({:.3}, \
         {:.3}, {:.3}), radius {:.3}. facing-yaw-*.png show it turned 0, 90, 180 and 270 \
         degrees with Iron's glyph: in facing-yaw-0.png it should sit on the forehead.",
        orientation.label(),
        orientation.yaw,
        orientation.pitch,
        orientation.roll,
        b.min[0],
        b.max[0],
        b.min[1],
        b.max[1],
        b.min[2],
        b.max[2],
        placement.decal_centre[0],
        placement.decal_centre[1],
        placement.decal_centre[2],
        placement.decal_radius,
    );

    let Ok(ctx) = gfx::GfxContext::headless() else {
        println!("no GPU adapter: only glyphs.png was written");
        return;
    };
    let mut showcase = Showcase::new(&ctx.device, &ctx.queue);
    showcase.set_mesh(&ctx.device, &ctx.queue, &skull.mesh, &skull.bases);
    let iron = ABOUT.iter().position(|a| a.name == "Iron").expect("Iron");
    let silver = ABOUT
        .iter()
        .position(|a| a.tier == SkullTier::Silver)
        .expect("a silver skull");
    let states = [
        (
            "locked-iron".to_owned(),
            Look {
                skull: iron,
                collected: false,
            },
        ),
        (
            "gold-iron".to_owned(),
            Look {
                skull: iron,
                collected: true,
            },
        ),
        (
            format!("silver-{}", slug(ABOUT[silver].name)),
            Look {
                skull: silver,
                collected: true,
            },
        ),
    ];
    // As the page shows it in three windows.
    for (name, look) in &states {
        for window in [(640, 480), (1280, 960), (3840, 2160)] {
            let canvas = seen(&ctx, &mut showcase, look, 0.0, window);
            write(&canvas, &dir, &format!("{name}-{}.png", window.1));
        }
    }
    // The sway's ends, as seen.
    let gold = Look {
        skull: iron,
        collected: true,
    };
    for (name, degrees) in [
        ("left", -showcase::SWAY_DEGREES),
        ("right", showcase::SWAY_DEGREES),
    ] {
        let canvas = seen(
            &ctx,
            &mut showcase,
            &gold,
            degrees.to_radians(),
            (1280, 960),
        );
        write(&canvas, &dir, &format!("sway-{name}.png"));
    }
    // The picture box at a 640 by 480 target, for the sheets.
    let [top, left, bottom, right] = crate::skulls_menu::PICTURE_BOX;
    let size = ((right - left) as u32, (top - bottom) as u32);
    orientation_sheets(&ctx, &mut showcase, &skull, &gold, size, orientation, &dir);
    for degrees in [0u32, 90, 180, 270] {
        let yaw = (degrees as f32).to_radians();
        let canvas = render(&ctx, &mut showcase, &gold, yaw, (size.0 * 4, size.1 * 4));
        write(&canvas, &dir, &format!("facing-yaw-{degrees}.png"));
    }
}
