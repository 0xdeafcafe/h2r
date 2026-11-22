//! A preview of the model picture from your disc, for a look before
//! landing, without the game: the skull in three states at the picture's
//! in-game size and at four times it, the model from four sides with a
//! glyph (where the forehead decal lands), sheets of the model as
//! authored under each of the 24 axis-aligned orientations (to choose
//! `MODEL_FORWARD` and `MODEL_UP`) and, from that choice, under a range
//! of yaw, pitch and roll (to choose the fine-tune), the fine-tune found
//! from the mesh's mirror symmetry, and a sheet of every glyph.
//! Writes only to `H2_SKULLS_PREVIEW_DIR`, which must be outside the
//! repository (disc-derived pixels are never committed); prints the map
//! and tag it loaded and the model's bounds.
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

fn render(
    ctx: &gfx::GfxContext,
    showcase: &mut Showcase,
    look: &Look,
    yaw: f32,
    size: (u32, u32),
) -> Canvas {
    match look.glyph() {
        Some(strokes) => {
            let mask = skull_glyphs::rasterise(strokes, skull_glyphs::SIZE);
            showcase.set_glyph(&ctx.device, &ctx.queue, skull_glyphs::SIZE, &mask);
        }
        None => showcase.set_glyph(&ctx.device, &ctx.queue, 0, &[]),
    }
    let shown = showcase::Look {
        yaw,
        tint: look.tint(),
        silhouette: look.silhouette(),
        glyph: look.glyph().is_some(),
    };
    draw(ctx, showcase, &shown, size)
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
/// band under it, gold with no glyph and no sway; written as `name`. The
/// showcase is left holding the picture's own mesh.
#[allow(clippy::too_many_arguments)]
fn sheet(
    ctx: &gfx::GfxContext,
    showcase: &mut Showcase,
    skull: &Skull,
    tint: [f32; 3],
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
    showcase.set_glyph(&ctx.device, &ctx.queue, 0, &[]);
    let shown = showcase::Look {
        yaw: 0.0,
        tint,
        silhouette: false,
        glyph: false,
    };
    println!("{name}, left to right, top to bottom:");
    for (i, c) in cells.iter().enumerate() {
        let (cx, cy) = ((i as u32 % columns) * cell.0, (i as u32 / columns) * cell.1);
        let mesh = skull.rest.oriented(&c.orientation);
        showcase.set_mesh(&ctx.device, &ctx.queue, &mesh, &skull.bases);
        let canvas = draw(ctx, showcase, &shown, picture);
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

/// The 24 axis-aligned orientations (`orientations.png`), to choose
/// `MODEL_FORWARD` and `MODEL_UP`; then, from that pair, yaw by pitch
/// (`orientations-fine.png`), roll (`orientations-roll.png`), and the
/// fine-tune found from the mesh's mirror symmetry (`auto.png`).
fn orientation_sheets(
    ctx: &gfx::GfxContext,
    showcase: &mut Showcase,
    skull: &Skull,
    tint: [f32; 3],
    picture: (u32, u32),
    dir: &Path,
) {
    let axes: Vec<Cell> = Orientation::all()
        .into_iter()
        .enumerate()
        .map(|(i, o)| Cell {
            label: format!("{i} {}", o.label()),
            chosen: o.forward == MODEL_FORWARD && o.up == MODEL_UP,
            orientation: o,
        })
        .collect();
    sheet(
        ctx,
        showcase,
        skull,
        tint,
        picture,
        6,
        &axes,
        dir,
        "orientations.png",
    );

    let base = Orientation::axes(MODEL_FORWARD, MODEL_UP);
    let steps: Vec<f32> = (-3..=3).map(|k| 15.0 * k as f32).collect();
    let in_use = |yaw: f32, pitch: f32, roll: f32| {
        yaw == MODEL_YAW_DEGREES && pitch == MODEL_PITCH_DEGREES && roll == MODEL_ROLL_DEGREES
    };
    let mut fine = Vec::new();
    for &pitch in &steps {
        for &yaw in &steps {
            fine.push(Cell {
                orientation: Orientation { yaw, pitch, ..base },
                label: format!("yaw {yaw} pitch {pitch}"),
                chosen: in_use(yaw, pitch, 0.0),
            });
        }
    }
    sheet(
        ctx,
        showcase,
        skull,
        tint,
        picture,
        7,
        &fine,
        dir,
        "orientations-fine.png",
    );
    let roll: Vec<Cell> = steps
        .iter()
        .map(|&roll| Cell {
            orientation: Orientation { roll, ..base },
            label: format!("roll {roll}"),
            chosen: in_use(0.0, 0.0, roll),
        })
        .collect();
    sheet(
        ctx,
        showcase,
        skull,
        tint,
        picture,
        7,
        &roll,
        dir,
        "orientations-roll.png",
    );

    // The drawn vertices, once each.
    let mut used = vec![false; skull.rest.vertices.len()];
    let mut points = Vec::new();
    for &i in &skull.rest.indices {
        if let Some(seen) = used.get_mut(i as usize).filter(|seen| !**seen) {
            *seen = true;
            points.push(skull.rest.vertices[i as usize].position);
        }
    }
    let Some(((yaw, pitch, roll), ratio)) = showcase::auto_fine(&points, &base) else {
        println!("auto: too few points to find the mirror plane");
        return;
    };
    println!(
        "auto (mirror plane and long axis, from {}): yaw {yaw:.1} pitch {pitch:.1} roll {roll:.1} \
         degrees; the long axis is {ratio:.2} times the one across it (near 1: the pitch is a \
         guess). As constants: MODEL_YAW_DEGREES = {yaw:.1}, MODEL_PITCH_DEGREES = {pitch:.1}, \
         MODEL_ROLL_DEGREES = {roll:.1}.",
        base.label()
    );
    let auto = [Cell {
        orientation: Orientation {
            yaw,
            pitch,
            roll,
            ..base
        },
        label: format!(
            "auto yaw {} pitch {} roll {}",
            yaw.round(),
            pitch.round(),
            roll.round()
        ),
        chosen: false,
    }];
    let big = (picture.0 * 2, picture.1 * 2);
    sheet(ctx, showcase, skull, tint, big, 1, &auto, dir, "auto.png");
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
    let skull = skull.expect("the skull model");
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
        "  facing: {} yaw {} pitch {} roll {} degrees (MODEL_FORWARD, MODEL_UP, \
         MODEL_YAW_DEGREES, MODEL_PITCH_DEGREES, MODEL_ROLL_DEGREES); turned, bounds x \
         {:.3}..{:.3}, y {:.3}..{:.3}, z {:.3}..{:.3}. The picture shows the turned model's +x \
         to the camera, its +y on the picture's right, +z up. The glyph is centred at ({:.3}, \
         {:.3}, {:.3}), radius {:.3}. facing-yaw-*.png show it turned 0, 90, 180 and 270 \
         degrees with Iron's glyph: in facing-yaw-0.png it should sit on the forehead.",
        MODEL_ORIENTATION.label(),
        MODEL_YAW_DEGREES,
        MODEL_PITCH_DEGREES,
        MODEL_ROLL_DEGREES,
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
    // The picture box at a 640 by 480 target, and four times it.
    let [top, left, bottom, right] = crate::skulls_menu::PICTURE_BOX;
    let size = ((right - left) as u32, (top - bottom) as u32);
    for (name, look) in &states {
        for scale in [1, 4] {
            let canvas = render(
                &ctx,
                &mut showcase,
                look,
                0.0,
                (size.0 * scale, size.1 * scale),
            );
            write(&canvas, &dir, &format!("{name}-{scale}x.png"));
        }
    }
    let gold = Look {
        skull: iron,
        collected: true,
    };
    orientation_sheets(&ctx, &mut showcase, &skull, gold.tint(), size, &dir);
    for degrees in [0u32, 90, 180, 270] {
        let yaw = (degrees as f32).to_radians();
        let canvas = render(&ctx, &mut showcase, &gold, yaw, (size.0 * 4, size.1 * 4));
        write(&canvas, &dir, &format!("facing-yaw-{degrees}.png"));
    }
}
