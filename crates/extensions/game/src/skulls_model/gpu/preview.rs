//! A preview of the model picture from your disc, for a look before
//! landing, without the game: the skull in three states at the picture's
//! in-game size and at four times it, the model from four sides with a
//! glyph (where the forehead decal lands), and a sheet of every glyph.
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

fn render(
    ctx: &gfx::GfxContext,
    showcase: &mut Showcase,
    look: &Look,
    yaw: f32,
    (width, height): (u32, u32),
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
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    assert!(showcase.draw(&ctx.device, &mut enc, width, height, &shown));
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
        sheet.label(
            &format!("{i} {}", about.name),
            cx + 4,
            cy + 8 + GLYPH + 10,
            [200, 210, 230],
        );
    }
    write(&sheet, dir, "glyphs.png");
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
        "  {} vertices, {} triangles, base map {}",
        skull.mesh.vertices.len(),
        skull.mesh.indices.len() / 3,
        skull
            .base
            .as_ref()
            .map_or("none".to_owned(), |i| format!("{}x{}", i.width, i.height)),
    );
    println!(
        "  bounds x {:.3}..{:.3}, y {:.3}..{:.3}, z {:.3}..{:.3}",
        b.min[0], b.max[0], b.min[1], b.max[1], b.min[2], b.max[2]
    );
    println!(
        "  facing: the picture assumes the face looks along +x (camera at +x, the model's +y \
         on the picture's right, +z up). The glyph is centred at ({:.3}, {:.3}, {:.3}), \
         radius {:.3}. facing-yaw-*.png show the model turned 0, 90, 180 and 270 degrees \
         with Iron's glyph: in facing-yaw-0.png it should sit on the forehead.",
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
    showcase.set_mesh(&ctx.device, &ctx.queue, &skull.mesh, skull.base.as_ref());
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
    for degrees in [0u32, 90, 180, 270] {
        let yaw = (degrees as f32).to_radians();
        let canvas = render(&ctx, &mut showcase, &gold, yaw, (size.0 * 4, size.1 * 4));
        write(&canvas, &dir, &format!("facing-yaw-{degrees}.png"));
    }
}
