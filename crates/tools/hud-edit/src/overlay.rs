//! The HUD as the overlay stage draws it (docs/graphics/retire-d3d.md,
//! slice R6; `frame_scene::overlay`): the preview's walk
//! ([`crate::render::draw_answered`]), the same gate, anchors, sprites,
//! answers and text, emitting ordered overlay items in place of pixels.
//! A bitmap widget is a quad over its bitmap's image, a text widget a text
//! run of its font's glyphs. Nothing is decoded: the renderer holds the
//! images and the glyph atlas.
//!
//! What the list draws differently from the preview, and why:
//!
//! - A meter names its shader ([`Program::Shader`]): the fill is the
//!   shader's, not a fixed combiner's, and the renderer refuses the item
//!   until tag shaders translate. The preview's fill is its own stand-in.
//! - Plain sprites draw with the fixed texture combiner, alpha blended:
//!   the preview's assumption (S: that a plain HUD shader draws its texels
//!   as they are).
//! - Text without the disc's fonts has no glyphs, so it is not listed; the
//!   preview draws a stand-in.

use std::io::{Read, Seek};

use frame_scene::{
    ClipRect, Combiner, Overlay, OverlayBlend, OverlayError, OverlayGlyph, OverlayMaterial,
    OverlayTexture, OverlayVertex, Program, Sampler,
};
use map::Map;

use crate::model::{Hud, Widget, WidgetKey, WidgetKind};
use crate::place::{self, Frame};
use crate::render::{self, Answers, Art, Fonts, H, W};
use crate::state::HudState;

/// One widget of the walk: the overlay item it added, or why none.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub hud: usize,
    pub key: WidgetKey,
    pub name: String,
    /// Its index in the overlay's items.
    pub item: Option<u32>,
    /// Why it drew nothing, or what was assumed.
    pub notes: Vec<String>,
}

/// The part of a sprite `[u0, u1, v0, v1]` (fractions of it) a crop
/// keeps, snapped to its texels as [`render::Sprite::cropped`] snaps
/// them: the fractions kept and the texels across and down.
fn crop(px: [u32; 2], [u0, u1, v0, v1]: [f32; 4]) -> ([f32; 4], [u32; 2]) {
    let [u0, u1, v0, v1] = [u0, u1, v0, v1].map(|v| v.clamp(0.0, 1.0));
    let snap = |a: f32, b: f32, n: u32| {
        let x0 = (a * n as f32).round() as u32;
        let x1 = ((b * n as f32).round() as u32).max(x0);
        (x0, x1)
    };
    let (x0, x1) = snap(u0, u1, px[0]);
    let (y0, y1) = snap(v0, v1, px[1]);
    let frac = |v: u32, n: u32| if n == 0 { 0.0 } else { v as f32 / n as f32 };
    (
        [frac(x0, px[0]), frac(x1, px[0]), frac(y0, px[1]), frac(y1, px[1])],
        [x1 - x0, y1 - y0],
    )
}

/// Adds the HUDs' widgets to `overlay` in the order the engine draws them
/// (`huds[0]` first, then those drawn with it, E `0x1394b9`), each HUD's
/// widgets in its order. The overlay's target is the HUD's 640 by 480
/// canvas; one with no size is given it.
pub fn draw_list<R: Read + Seek>(
    map: &mut Map<R>,
    art: &mut Art,
    fonts: Option<&Fonts>,
    huds: &[&Hud],
    st: &HudState,
    answers: &dyn Answers,
    overlay: &mut Overlay,
) -> Vec<Listed> {
    if overlay.size == [0, 0] {
        overlay.set_size([W, H]);
    }
    let mode = st.mode;
    let frame = Frame::for_mode(mode);
    let masks = st.masks();
    let mut listed = Vec::new();
    for (h, hud) in huds.iter().enumerate() {
        for w in &hud.widgets {
            let mut notes = Vec::new();
            let (shown, point) =
                render::shown_and_point(&frame, mode, masks, st, (h, answers), w, &mut notes);
            let item = match (shown, point) {
                (false, _) => {
                    notes.push("hidden".into());
                    None
                }
                (true, None) => None,
                (true, Some(point)) => {
                    let result = match w.key.kind {
                        WidgetKind::Bitmap => {
                            bitmap(map, art, overlay, w, st, (h, answers), point)
                        }
                        WidgetKind::Text => text(map, fonts, overlay, w, st, (h, answers), point),
                    };
                    match result {
                        Ok((item, note)) => {
                            notes.extend(note);
                            Some(item)
                        }
                        Err(why) => {
                            notes.push(why);
                            None
                        }
                    }
                }
            };
            listed.push(Listed { hud: h, key: w.key, name: w.name.clone(), item, notes });
        }
    }
    listed
}

fn refused(e: OverlayError) -> String {
    match e {
        OverlayError::NoTarget => "the overlay has no target".into(),
        OverlayError::Clip => "off the screen".into(),
        OverlayError::Material => "the material does not fit the item".into(),
        OverlayError::Geometry => "a corner or colour is not finite".into(),
    }
}

fn bitmap<R: Read + Seek>(
    map: &mut Map<R>,
    art: &mut Art,
    overlay: &mut Overlay,
    w: &Widget,
    st: &HudState,
    (h, answers): (usize, &dyn Answers),
    point: [f32; 2],
) -> Result<(u32, Option<String>), String> {
    let mode = st.mode;
    let id = w.bitmap().ok_or("no bitmap")?;
    let at = art.sprite_place(map, id, w.sequence(mode))?;
    let image = u16::try_from(at.image).map_err(|_| "an image past 65535".to_owned())?;
    // The texels across and down, as the preview counts them.
    let px = at.size.map(|v| v.round().max(0.0) as u32);
    let (part, px) = match answers.crop(h, w) {
        Some(c) => crop(px, c),
        None => ([0.0, 1.0, 0.0, 1.0], px),
    };
    let size = [
        if px[0] == 0 { 0.0 } else { at.size[0] * px[0] as f32 / at.size[0].round().max(1.0) },
        if px[1] == 0 { 0.0 } else { at.size[1] * px[1] as f32 / at.size[1].round().max(1.0) },
    ];
    let rect = place::bitmap_rect(point, size, w.registration(mode));
    let [x0, y0] = [rect.x0.round(), rect.y0.round()];
    let [x1, y1] = [x0 + px[0] as f32, y0 + px[1] as f32];
    // The sprite's part in its image.
    let [b0, b1, c0, c1] = at.bounds;
    let u = |f: f32| b0 + (b1 - b0) * f;
    let v = |f: f32| c0 + (c1 - c0) * f;
    let (mut ul, mut ur) = (u(part[0]), u(part[1]));
    let (mut vt, mut vb) = (v(part[2]), v(part[3]));
    // Flags bits 0 and 1 mirror the texture (0x22b29c).
    let flags = w.flags();
    if flags & 1 != 0 {
        std::mem::swap(&mut ul, &mut ur);
    }
    if flags & 2 != 0 {
        std::mem::swap(&mut vt, &mut vb);
    }
    let look = answers.look(h, w);
    let alpha = look.map_or(1.0, |l| l.alpha.clamp(0.0, 1.0));
    let corner = |x: f32, y: f32, s: f32, t: f32| OverlayVertex {
        position: [x, y],
        uv: [s, t],
        color: [1.0, 1.0, 1.0, alpha],
    };
    let mut corners = [
        corner(x0, y0, ul, vt),
        corner(x1, y0, ur, vt),
        corner(x1, y1, ur, vb),
        corner(x0, y1, ul, vb),
    ];
    // A turn about the sprite's centre, clockwise on the screen; the
    // sprite's own rectangle clips it, as the preview's corners turned out
    // of it are not drawn.
    let full = ClipRect::full(overlay.size);
    let clip = match answers.angle(h, w).filter(|a| *a != 0.0) {
        Some(a) => {
            let (sin, cos) = a.sin_cos();
            let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            for c in &mut corners {
                let (dx, dy) = (c.position[0] - cx, c.position[1] - cy);
                c.position = [cx + cos * dx - sin * dy, cy + sin * dx + cos * dy];
            }
            let edge = |v: f32| v.max(0.0) as u32;
            ClipRect {
                x: edge(x0),
                y: edge(y0),
                width: edge(x1).saturating_sub(edge(x0)),
                height: edge(y1).saturating_sub(edge(y0)),
            }
        }
        None => full,
    };
    let (program, note) = match render::meter_fill(map, w, st, (h, answers)) {
        Some(_) => match w.shader() {
            Some(shader) => (
                Program::Shader { tag: shader.0 },
                Some("a meter: drawn by its shader, not a fixed combiner".to_owned()),
            ),
            None => return Err("a meter with no shader".into()),
        },
        None if look.is_some_and(|l| l.grey) => (Program::Fixed(Combiner::TextureGrey), None),
        None => (Program::Fixed(Combiner::Texture), None),
    };
    let material = OverlayMaterial {
        program,
        texture: OverlayTexture::Bitmap { tag: id.0, image },
        sampler: Sampler { wrap: false, point: true },
        blend: OverlayBlend::ALPHA,
    };
    let item = overlay.push_quad(material, clip, corners).map_err(refused)?;
    Ok((item, note))
}

fn text<R: Read + Seek>(
    map: &mut Map<R>,
    fonts: Option<&Fonts>,
    overlay: &mut Overlay,
    w: &Widget,
    st: &HudState,
    (h, answers): (usize, &dyn Answers),
    point: [f32; 2],
) -> Result<(u32, Option<String>), String> {
    let mode = st.mode;
    if w.shader().is_none() {
        return Err("no shader: the engine draws nothing (0x22aa2f)".into());
    }
    let font = crate::layout::text_font(w.font(mode))
        .ok_or_else(|| format!("font byte {} names no font: not drawn", w.font(mode)))?;
    let string = answers.text(h, w).unwrap_or_else(|| {
        let string = crate::model::string(map, w.string());
        render::widget_text(w, string.as_deref(), st)
    });
    let fonts = fonts.ok_or("the disc's fonts were not found: the text has no glyphs")?;
    let (laid, line) = fonts
        .layout(font, &string)
        .ok_or_else(|| format!("font {font} does not lay out {string:?}"))?;
    let size = [laid.width as f32, (line).max(laid.height as i32) as f32];
    let rect = place::text_rect(point, size, w.justification());
    let [ox, oy] = [rect.x0.round(), rect.y0.round()];
    let glyphs: Vec<_> = laid
        .glyphs
        .iter()
        .filter(|g| g.width != 0 && g.height != 0)
        .map(|g| {
            let (x, y) = (ox + g.x as f32, oy + g.y as f32);
            OverlayGlyph {
                index: g.index,
                rect: [x, y, x + f32::from(g.width), y + f32::from(g.height)],
            }
        })
        .collect();
    if glyphs.is_empty() {
        return Err("no glyph with pixels".into());
    }
    let colour = answers.colour(h, w).unwrap_or(render::TEXT_COLOUR);
    let font = u16::try_from(font).map_err(|_| format!("font {font} is negative"))?;
    let item = overlay
        .push_text_run(
            OverlayMaterial::text(font),
            ClipRect::full(overlay.size),
            colour.map(|c| f32::from(c) / 255.0),
            &glyphs,
        )
        .map_err(refused)?;
    Ok((item, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;
    use crate::render::Look;
    use frame_scene::OverlayKind;
    use scenario_edit::MapSource;

    struct Kit;
    impl Answers for Kit {
        fn shown(&self, _: usize, w: &Widget) -> Option<bool> {
            (w.name == "stray").then_some(false)
        }
        fn angle(&self, _: usize, w: &Widget) -> Option<f32> {
            (w.name == "panel").then_some(std::f32::consts::FRAC_PI_2)
        }
        fn look(&self, _: usize, w: &Widget) -> Option<Look> {
            (w.name == "panel").then_some(Look { alpha: 0.5, grey: true })
        }
        fn crop(&self, _: usize, w: &Widget) -> Option<[f32; 4]> {
            (w.name == "meter").then_some([0.0, 0.5, 0.0, 1.0])
        }
    }

    /// The fixture's player HUD (crate::fixture) as overlay items: the
    /// panel and the stray as quads over the bitmap where the preview puts
    /// them, the meter by its shader, the number not listed without fonts.
    #[test]
    fn the_hud_lists_its_widgets_in_order_where_the_preview_draws_them() {
        let (bytes, player, _, _) = fixture::build();
        let source = MapSource { bytes: Some(bytes.into()), ..MapSource::file("hudtest.map") };
        let mut map = source.open().unwrap();
        let hud = crate::model::read(&mut map, player).unwrap();
        let mut art = Art::new(scenario_edit::scene::resource_files(&source));
        let st = HudState::default();
        let mut overlay = Overlay::default();
        let listed = draw_list(&mut map, &mut art, None, &[&hud], &st, &render::Retail, &mut overlay);
        assert_eq!(overlay.size, [640, 480]);
        assert_eq!(overlay.check(), Ok(()));
        let names: Vec<_> = listed.iter().map(|l| (l.name.as_str(), l.item)).collect();
        assert_eq!(names, [("panel", Some(0)), ("meter", Some(1)), ("stray", Some(2)), ("count", None)]);
        let corners = |n: usize| overlay.vertices_of(&overlay.items[n]).iter().map(|v| v.position).collect::<Vec<_>>();
        // The preview's rectangles (tests.rs, the_preview_places_and_draws_the_art).
        assert_eq!(corners(0), [[48.0, 404.0], [112.0, 404.0], [112.0, 420.0], [48.0, 420.0]]);
        assert_eq!(corners(1)[0], [48.0, 388.0]);
        assert_eq!(corners(1)[2], [80.0, 404.0]);
        assert_eq!(corners(2)[0], [88.0, 396.0]);
        let panel = &overlay.items[0];
        assert_eq!(panel.kind, OverlayKind::Quad);
        assert_eq!(panel.material.program, Program::Fixed(Combiner::Texture));
        let bitmap = hud.widgets[0].bitmap().unwrap();
        assert_eq!(panel.material.texture, OverlayTexture::Bitmap { tag: bitmap.0, image: 0 });
        assert_eq!(overlay.vertices_of(panel)[2].uv, [1.0, 1.0]);
        // The meter is sequence 1, the left half sprite, drawn by its shader.
        let meter = &overlay.items[1];
        assert_eq!(meter.material.program, Program::Shader { tag: hud.widgets[1].shader().unwrap().0 });
        assert_eq!(overlay.vertices_of(meter)[1].uv, [0.5, 0.0]);
        assert!(listed[3].notes.iter().any(|n| n.contains("fonts")));
        assert_eq!(overlay.counts().shader_materials, 1);
    }

    /// What a hook answers changes the items as it changes the preview: a
    /// hidden widget is not listed, a crop narrows the quad and its
    /// coordinates, a turn turns the corners inside the sprite's own clip,
    /// a look greys and dims.
    #[test]
    fn answers_change_the_items() {
        let (bytes, player, _, _) = fixture::build();
        let source = MapSource { bytes: Some(bytes.into()), ..MapSource::file("hudtest.map") };
        let mut map = source.open().unwrap();
        let hud = crate::model::read(&mut map, player).unwrap();
        let mut art = Art::new(scenario_edit::scene::resource_files(&source));
        let mut overlay = Overlay::default();
        let listed = draw_list(&mut map, &mut art, None, &[&hud], &HudState::default(), &Kit, &mut overlay);
        let by = |n: &str| listed.iter().find(|l| l.name == n).unwrap().clone();
        assert_eq!(by("stray").item, None);
        assert!(by("stray").notes.contains(&"hidden".to_owned()));
        let panel = &overlay.items[by("panel").item.unwrap() as usize];
        assert_eq!(panel.material.program, Program::Fixed(Combiner::TextureGrey));
        assert_eq!(panel.clip, ClipRect { x: 48, y: 404, width: 64, height: 16 });
        let v = overlay.vertices_of(panel);
        assert!(v.iter().all(|c| c.color == [1.0, 1.0, 1.0, 0.5]));
        // A quarter turn about (80, 412): the top left corner goes to the top right.
        assert!((v[0].position[0] - 88.0).abs() < 1e-3 && (v[0].position[1] - 380.0).abs() < 1e-3, "{:?}", v[0]);
        // The meter's left half of its 32 texels: 16 across, a quarter of the image.
        let meter = &overlay.items[by("meter").item.unwrap() as usize];
        let m = overlay.vertices_of(meter);
        assert_eq!(m[1].position[0] - m[0].position[0], 16.0);
        assert_eq!(m[1].uv[0], 0.25);
    }

    #[test]
    fn a_crop_snaps_to_the_sprites_texels() {
        assert_eq!(crop([32, 16], [0.0, 0.5, 0.0, 1.0]), ([0.0, 0.5, 0.0, 1.0], [16, 16]));
        assert_eq!(crop([10, 10], [0.12, 0.5, -1.0, 2.0]), ([0.1, 0.5, 0.0, 1.0], [4, 10]));
        assert_eq!(crop([0, 4], [0.0, 1.0, 0.0, 1.0]).1, [0, 4]);
    }
}
