//! The menus, the loading screen, the fade and the bars as the overlay
//! stage draws them (docs/graphics/retire-d3d.md, slice R7;
//! `frame_scene::overlay`): what the guest's UI draws after a view's HUD,
//! as typed state ([`UiFrame`]), turned into ordered overlay items by
//! [`draw_list`]. The UI's logic, input and state stay the guest's; this
//! replaces only its drawing.
//!
//! A frame is the guest's draws in its call order: the fade `0x00040890`,
//! the solid rectangles of `0x00052ce7` (menu fade rectangles, cinematic
//! bars, split-screen dividers), the loading draw's bitmaps
//! (`0x000289e0`) and the screens `0x00147af1` draws. A screen's widgets
//! are drawn in the order the guest's sort `0x002353a5` gives them
//! ([`sort_widgets`]); a dialog's screen first fills the target with the
//! backdrop colour and blurs it (`0x0023029a`, docs/formats/ui.md, "Draw
//! order").
//!
//! Nothing here reads the guest: the values a frame holds (colours,
//! blends, the blur's steps and tap) are read from the running image by
//! whoever fills it, never committed.

use frame_scene::{
    ClipRect, Combiner, Overlay, OverlayBlend, OverlayBlur, OverlayError, OverlayGlyph,
    OverlayMaterial, OverlayModelView, OverlayTexture, OverlayVertex, Program, Sampler,
};

/// The UI's canvas, guest pixels: its rectangles are a 640 by 480 frame
/// about the centre (docs/formats/ui.md, "Coordinates").
pub const CANVAS: [u32; 2] = [640, 480];

/// How a text block sits in its rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    #[default]
    Left,
    Right,
    Centre,
}

/// A textured rectangle outside a screen (the loading draw's background,
/// sweep and spinner) or a bitmap widget's art.
#[derive(Clone, Debug, PartialEq)]
pub struct UiBitmap {
    pub texture: OverlayTexture,
    /// `[x0, y0, x1, y1]`, guest pixels (a widget's own is its rectangle).
    pub rect: [f32; 4],
    /// `[u0, v0, u1, v1]`.
    pub uv: [f32; 4],
    pub color: [f32; 4],
    pub combiner: Combiner,
    pub sampler: Sampler,
    pub blend: OverlayBlend,
}

/// What a widget draws.
#[derive(Clone, Debug, PartialEq)]
pub enum UiContent {
    /// A container (screen, list, list item, page selector, table): no
    /// pixels of its own.
    None,
    /// A bitmap widget (type 8); its `rect` is ignored for the widget's.
    Bitmap(UiBitmap),
    /// A shader bitmap (type 9): the shader's own program over `uv`.
    Shader {
        tag: u32,
        uv: [f32; 4],
        color: [f32; 4],
        blend: OverlayBlend,
    },
    /// A text widget or a button's label, the string as the widget holds
    /// it after substitution (UTF-16).
    Text {
        font: u16,
        text: Vec<u16>,
        color: [f32; 4],
        justify: Justify,
    },
    /// A model scene (type 7): its view into the widget's rectangle.
    Model(OverlayModelView),
}

/// One widget as the guest holds it (the common header `0x0022e27b`).
#[derive(Clone, Debug, PartialEq)]
pub struct UiWidget {
    /// The widget type at `+4`.
    pub kind: u32,
    /// The sort depth at `+0x60` (the Z of the animated offset).
    pub depth: f32,
    /// The render depth bias at `+0x6a`.
    pub bias: i16,
    /// The rectangle at `+0x20`: top, left, bottom, right, about the
    /// centre with Y up.
    pub rect: [i16; 4],
    /// The animated offset's X and Y at `+0x58`, Y up.
    pub offset: [f32; 2],
    /// The alpha at `+0x64`: multiplies the content's.
    pub alpha: f32,
    pub content: UiContent,
}

/// The fill and blur a dialog's screen draws under its widgets.
#[derive(Clone, Debug, PartialEq)]
pub struct UiBackdrop {
    /// The shared globals' colour at `+0x6c`, RGBA, 0 to 1.
    pub color: [f32; 4],
    /// The screen's transition fraction: the fill's alpha is the colour's
    /// times it.
    pub fraction: f32,
    /// The blur's steps at that fraction (`0x00235756`).
    pub blur: OverlayBlur,
}

/// One screen of a window stack.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiScreen {
    pub backdrop: Option<UiBackdrop>,
    /// In any order: [`draw_list`] sorts them.
    pub widgets: Vec<UiWidget>,
}

/// One of the guest's UI draws, in its call order.
#[derive(Clone, Debug, PartialEq)]
pub enum UiDraw {
    /// The full-screen fade `0x00040890`: its colour (`0x00485874`) and the
    /// blend its mode picks.
    Fade {
        color: [f32; 4],
        blend: OverlayBlend,
    },
    /// A solid rectangle (`0x00052ce7`), guest pixels `[x0, y0, x1, y1]`.
    Rect {
        rect: [f32; 4],
        color: [f32; 4],
        blend: OverlayBlend,
    },
    /// A loading screen's bitmap.
    Bitmap(UiBitmap),
    Screen(UiScreen),
}

/// What the UI draws over a frame's views and HUD.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiFrame {
    pub draws: Vec<UiDraw>,
}

/// Lays out a text block in the renderer's fonts.
pub trait UiText {
    /// `text` in engine font `font`, wrapped and cut to `rect` (`[x0, y0,
    /// x1, y1]`, guest pixels) as the game does and justified in it: each
    /// glyph's index and rectangle, guest pixels. `None` when the font
    /// does not lay it out.
    fn shape(
        &self,
        font: u16,
        text: &[u16],
        rect: [f32; 4],
        justify: Justify,
    ) -> Option<Vec<OverlayGlyph>>;
}

/// No fonts: no text is listed.
pub struct NoText;

impl UiText for NoText {
    fn shape(&self, _: u16, _: &[u16], _: [f32; 4], _: Justify) -> Option<Vec<OverlayGlyph>> {
        None
    }
}

/// One draw or widget of the walk: the overlay item it added, or why none.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    /// Its index in [`UiFrame::draws`].
    pub draw: usize,
    /// A screen's widget: its index in [`UiScreen::widgets`].
    pub widget: Option<usize>,
    /// Its index in the overlay's items; a backdrop lists its fill.
    pub item: Option<u32>,
    pub note: Option<String>,
}

/// Whether `a` sorts before `b` in the guest's comparator `0x002356d8`:
/// depth ascending, values within 0.001 tying, then render depth bias,
/// then widget type.
pub fn sorts_before(a: &UiWidget, b: &UiWidget) -> bool {
    if (a.depth - b.depth).abs() > 0.001 {
        return a.depth < b.depth;
    }
    (a.bias, a.kind) < (b.bias, b.kind)
}

/// The widgets' indices in draw order: a stable insertion by
/// [`sorts_before`], at most 256 (`0x002353a5` collects up to 256).
/// The guest's sort algorithm is not traced; with depths that tie
/// pairwise but not across a chain the order of such a chain is this
/// one's.
pub fn sort_widgets(widgets: &[UiWidget]) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::with_capacity(widgets.len().min(256));
    for i in 0..widgets.len().min(256) {
        let at = order
            .iter()
            .rposition(|&j| !sorts_before(&widgets[i], &widgets[j]))
            .map_or(0, |p| p + 1);
        order.insert(at, i);
    }
    order
}

/// A widget's rectangle in guest pixels, `[x0, y0, x1, y1]`, moved by its
/// animated offset.
pub fn widget_rect(w: &UiWidget) -> [f32; 4] {
    let [top, left, bottom, right] = w.rect.map(f32::from);
    let (cx, cy) = (CANVAS[0] as f32 / 2.0, CANVAS[1] as f32 / 2.0);
    let [dx, dy] = w.offset;
    [
        left + cx + dx,
        cy - top - dy,
        right + cx + dx,
        cy - bottom - dy,
    ]
}

/// The pixels a rectangle touches, as a clip.
fn clip_of([x0, y0, x1, y1]: [f32; 4]) -> ClipRect {
    let (x0, y0) = (x0.floor().max(0.0), y0.floor().max(0.0));
    let (x1, y1) = (x1.ceil().max(x0), y1.ceil().max(y0));
    ClipRect {
        x: x0 as u32,
        y: y0 as u32,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
    }
}

fn corners(
    [x0, y0, x1, y1]: [f32; 4],
    [u0, v0, u1, v1]: [f32; 4],
    color: [f32; 4],
) -> [OverlayVertex; 4] {
    let c = |x, y, u, v| OverlayVertex {
        position: [x, y],
        uv: [u, v],
        color,
    };
    [
        c(x0, y0, u0, v0),
        c(x1, y0, u1, v0),
        c(x1, y1, u1, v1),
        c(x0, y1, u0, v1),
    ]
}

fn faded([r, g, b, a]: [f32; 4], alpha: f32) -> [f32; 4] {
    [r, g, b, a * alpha]
}

fn refused(e: OverlayError) -> String {
    format!("the overlay refused it: {e:?}")
}

fn bitmap(
    overlay: &mut Overlay,
    b: &UiBitmap,
    rect: [f32; 4],
    alpha: f32,
) -> Result<u32, OverlayError> {
    let material = OverlayMaterial {
        program: Program::Fixed(b.combiner),
        texture: b.texture,
        sampler: b.sampler,
        blend: b.blend,
    };
    overlay.push_quad(
        material,
        ClipRect::full(overlay.size),
        corners(rect, b.uv, faded(b.color, alpha)),
    )
}

fn widget(overlay: &mut Overlay, text: &dyn UiText, w: &UiWidget) -> Result<Option<u32>, String> {
    let rect = widget_rect(w);
    let all = ClipRect::full(overlay.size);
    match &w.content {
        UiContent::None => Ok(None),
        UiContent::Bitmap(b) => bitmap(overlay, b, rect, w.alpha).map(Some).map_err(refused),
        UiContent::Shader {
            tag,
            uv,
            color,
            blend,
        } => {
            let material = OverlayMaterial {
                program: Program::Shader { tag: *tag },
                texture: OverlayTexture::None,
                sampler: Sampler::default(),
                blend: *blend,
            };
            overlay
                .push_quad(material, all, corners(rect, *uv, faded(*color, w.alpha)))
                .map(Some)
                .map_err(refused)
        }
        UiContent::Text {
            font,
            text: string,
            color,
            justify,
        } => {
            if string.is_empty() {
                return Ok(None);
            }
            let glyphs = text
                .shape(*font, string, rect, *justify)
                .ok_or_else(|| format!("font {font} does not lay out the text"))?;
            let glyphs: Vec<_> = glyphs
                .into_iter()
                .filter(|g| g.rect[2] > g.rect[0] && g.rect[3] > g.rect[1])
                .collect();
            if glyphs.is_empty() {
                return Ok(None);
            }
            overlay
                .push_text_run(
                    OverlayMaterial::text(*font),
                    clip_of(rect),
                    faded(*color, w.alpha),
                    &glyphs,
                )
                .map(Some)
                .map_err(refused)
        }
        UiContent::Model(view) => overlay
            .push_model_view(clip_of(rect), view.clone())
            .map(Some)
            .map_err(refused),
    }
}

/// Adds `frame`'s draws to `overlay` in the guest's order, each screen's
/// widgets sorted. The overlay's target is the UI's canvas; one with no
/// size is given it. A draw or widget the overlay refuses is listed with
/// the reason and adds nothing.
pub fn draw_list(frame: &UiFrame, text: &dyn UiText, overlay: &mut Overlay) -> Vec<Listed> {
    if overlay.size == [0, 0] {
        overlay.set_size(CANVAS);
    }
    let all = ClipRect::full(overlay.size);
    let whole = [0.0, 0.0, overlay.size[0] as f32, overlay.size[1] as f32];
    let mut listed = Vec::new();
    let mut list = |draw, widget, r: Result<Option<u32>, String>| {
        let (item, note) = match r {
            Ok(item) => (item, None),
            Err(note) => (None, Some(note)),
        };
        listed.push(Listed {
            draw,
            widget,
            item,
            note,
        });
    };
    for (n, draw) in frame.draws.iter().enumerate() {
        match draw {
            UiDraw::Fade { color, blend } => {
                list(
                    n,
                    None,
                    overlay
                        .push_fill(all, whole, *color, *blend)
                        .map(Some)
                        .map_err(refused),
                );
            }
            UiDraw::Rect { rect, color, blend } => {
                list(
                    n,
                    None,
                    overlay
                        .push_fill(all, *rect, *color, *blend)
                        .map(Some)
                        .map_err(refused),
                );
            }
            UiDraw::Bitmap(b) => list(
                n,
                None,
                bitmap(overlay, b, b.rect, 1.0).map(Some).map_err(refused),
            ),
            UiDraw::Screen(screen) => {
                if let Some(b) = &screen.backdrop {
                    let fill = faded(b.color, b.fraction.clamp(0.0, 1.0));
                    let r = overlay
                        .push_fill(all, whole, fill, OverlayBlend::ALPHA)
                        .and_then(|item| overlay.push_blur(all, b.blur.clone()).map(|_| item));
                    list(n, None, r.map(Some).map_err(refused));
                }
                for i in sort_widgets(&screen.widgets) {
                    list(n, Some(i), widget(overlay, text, &screen.widgets[i]));
                }
                if screen.widgets.len() > 256 {
                    let r = Err(format!(
                        "{} widgets past the 256 the guest collects",
                        screen.widgets.len() - 256
                    ));
                    list(n, None, r);
                }
            }
        }
    }
    listed
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{OverlayCamera, OverlayKind};

    fn w(kind: u32, depth: f32, bias: i16) -> UiWidget {
        UiWidget {
            kind,
            depth,
            bias,
            rect: [10, -10, -10, 10],
            offset: [0.0; 2],
            alpha: 1.0,
            content: UiContent::None,
        }
    }

    fn art(tag: u32) -> UiBitmap {
        UiBitmap {
            texture: OverlayTexture::Bitmap { tag, image: 0 },
            rect: [0.0; 4],
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0; 4],
            combiner: Combiner::Texture,
            sampler: Sampler::default(),
            blend: OverlayBlend::ALPHA,
        }
    }

    /// Lays each unit out as an 8 by 10 cell from the rectangle's top left.
    struct Cells;

    impl UiText for Cells {
        fn shape(
            &self,
            font: u16,
            text: &[u16],
            rect: [f32; 4],
            _: Justify,
        ) -> Option<Vec<OverlayGlyph>> {
            (font == 1).then(|| {
                text.iter()
                    .enumerate()
                    .map(|(i, u)| {
                        let x = rect[0] + 8.0 * i as f32;
                        OverlayGlyph {
                            index: u32::from(*u),
                            rect: [x, rect[1], x + 8.0, rect[1] + 10.0],
                        }
                    })
                    .collect()
            })
        }
    }

    #[test]
    fn widgets_sort_by_depth_then_bias_then_type() {
        let widgets = [
            w(8, 0.5, 0),
            w(6, 0.1, 0),
            w(8, 0.1, 0),
            w(3, 0.1, 2),
            w(9, 0.1005, -1),
            w(6, -1.0, 9),
        ];
        // Depths 0.1 and 0.1005 tie; among them bias -1 first, then bias 0
        // by type (6 before 8), then bias 2.
        assert_eq!(sort_widgets(&widgets), [5, 4, 1, 2, 3, 0]);
        let same = [w(6, 0.0, 0), w(6, 0.0, 0), w(6, 0.0, 0)];
        assert_eq!(
            sort_widgets(&same),
            [0, 1, 2],
            "equal widgets keep their order"
        );
        let many: Vec<_> = (0..300).map(|i| w(6, i as f32, 0)).collect();
        assert_eq!(sort_widgets(&many).len(), 256);
    }

    #[test]
    fn rectangles_are_about_the_centre_with_y_up_and_move_by_the_offset() {
        let mut x = w(8, 0.0, 0);
        x.rect = [240, -320, -240, 320];
        assert_eq!(widget_rect(&x), [0.0, 0.0, 640.0, 480.0]);
        x.rect = [100, -50, 60, 30];
        x.offset = [5.0, 10.0];
        assert_eq!(widget_rect(&x), [275.0, 130.0, 355.0, 170.0]);
    }

    /// The UI order test (retire-d3d.md, R7's proof) on a synthetic
    /// frame: the fade, a bar, a loading bitmap, then a dialog's screen
    /// (its backdrop's fill and blur, then its widgets sorted), each
    /// becoming its items in that order.
    #[test]
    fn the_frame_draws_in_the_guests_order() {
        let mut title = w(6, 0.2, 0);
        title.content = UiContent::Text {
            font: 1,
            text: "OK".encode_utf16().collect(),
            color: [1.0, 1.0, 0.5, 1.0],
            justify: Justify::Left,
        };
        title.alpha = 0.5;
        let mut panel = w(8, 0.0, 0);
        panel.content = UiContent::Bitmap(art(0xe001_0001));
        let mut picture = w(7, 0.1, 0);
        let camera = OverlayCamera {
            position: [0.0, -1.0, 0.0],
            forward: [0.0, 1.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov_y: 1.0,
        };
        picture.content = UiContent::Model(OverlayModelView {
            camera,
            objects: vec![0xe100_0004],
        });
        let mut meter = w(9, 0.3, 0);
        meter.content = UiContent::Shader {
            tag: 0xe200_0005,
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0; 4],
            blend: OverlayBlend::ALPHA,
        };
        let list = w(1, 0.05, 0);
        let screen = UiScreen {
            backdrop: Some(UiBackdrop {
                color: [0.0, 0.0, 0.0, 0.5],
                fraction: 0.5,
                blur: OverlayBlur {
                    tap: 0.5,
                    spreads: vec![1.0, 0.875],
                },
            }),
            widgets: vec![title, meter, list, picture, panel],
        };
        let mut loading = art(0xe003_0003);
        loading.rect = [0.0, 0.0, 640.0, 480.0];
        let frame = UiFrame {
            draws: vec![
                UiDraw::Fade {
                    color: [0.0, 0.0, 0.0, 0.25],
                    blend: OverlayBlend::ALPHA,
                },
                UiDraw::Rect {
                    rect: [0.0, 0.0, 640.0, 60.0],
                    color: [0.0, 0.0, 0.0, 1.0],
                    blend: OverlayBlend::Off,
                },
                UiDraw::Bitmap(loading),
                UiDraw::Screen(screen),
            ],
        };
        let mut o = Overlay::default();
        let listed = draw_list(&frame, &Cells, &mut o);
        assert_eq!(o.size, CANVAS);
        let kinds: Vec<_> = o.items.iter().map(|i| i.kind).collect();
        use OverlayKind::*;
        assert_eq!(
            kinds,
            [Fill, Fill, Quad, Fill, Blur, Quad, ModelView, TextRun, Quad]
        );
        let widgets: Vec<_> = listed.iter().filter_map(|l| l.widget).collect();
        assert_eq!(
            widgets,
            [4, 2, 3, 0, 1],
            "panel, list, picture, title, meter"
        );
        let list_row = listed.iter().find(|l| l.widget == Some(2)).unwrap();
        assert_eq!(
            (list_row.item, &list_row.note),
            (None, &None),
            "a container draws nothing"
        );
        assert_eq!(
            o.vertices_of(&o.items[3])[0].color,
            [0.0, 0.0, 0.0, 0.25],
            "the backdrop's alpha times its fraction"
        );
        assert_eq!(o.blur_of(&o.items[4]).unwrap().spreads, [1.0, 0.875]);
        let text = &o.items[7];
        assert_eq!(text.color, [1.0, 1.0, 0.5, 0.5], "the widget's alpha");
        assert_eq!(
            text.clip,
            ClipRect {
                x: 310,
                y: 230,
                width: 20,
                height: 20
            }
        );
        assert_eq!(o.glyphs_of(text)[1].index, u32::from(b'K'));
        assert_eq!(
            o.items[8].material.program,
            Program::Shader { tag: 0xe200_0005 }
        );
        assert_eq!(
            o.items[6].clip,
            ClipRect {
                x: 310,
                y: 230,
                width: 20,
                height: 20
            }
        );
        assert_eq!(o.check(), Ok(()));
    }

    #[test]
    fn a_widget_the_overlay_or_the_fonts_refuse_is_listed_with_why() {
        let mut text = w(6, 0.0, 0);
        text.content = UiContent::Text {
            font: 2,
            text: vec![0x41],
            color: [1.0; 4],
            justify: Justify::Centre,
        };
        let mut off = w(8, 0.1, 0);
        off.rect = [10, 400, -10, 420];
        off.content = UiContent::Model(OverlayModelView {
            camera: OverlayCamera::default(),
            objects: vec![],
        });
        let frame = UiFrame {
            draws: vec![UiDraw::Screen(UiScreen {
                backdrop: None,
                widgets: vec![text, off],
            })],
        };
        let mut o = Overlay::default();
        let listed = draw_list(&frame, &Cells, &mut o);
        assert!(o.is_empty());
        assert!(listed[0].note.as_deref().unwrap().contains("font 2"));
        assert!(
            listed[1].note.as_deref().unwrap().contains("Clip"),
            "a viewport off the canvas"
        );
        let none = draw_list(&frame, &NoText, &mut o);
        assert!(none[0].item.is_none() && none[0].note.is_some());
    }
}
