//! The Textures tool's windows (docs/editor/modeling.md, "The tools in the
//! window"): the map's bitmaps on the left, the 2D canvas in the centre
//! (zoom, face and slice pickers, the tools) and the tool's settings, the
//! image's format, the adjustments and PNG in and out in the inspector.
//! State lives in [`crate::app::textures::Textures`] and
//! [`bitmap_edit::Bitmap`]; everything goes back as [`Action`]s, the same
//! commands the `bitmap.*` methods run.
//!
//! The canvas shows the layer's pixels through a texture that is updated
//! from the bitmap's dirty rectangles (`Bitmap::take_view_dirty`), so a
//! stroke shows as it is made and an undo shows the tiles it swapped. Undo
//! and Redo take the bitmap's document (`Action::UndoDoc`), its paint and
//! shape steps beside the bitmap's own writes.

use std::path::PathBuf;

use bitmap_edit::{Adjust, BitmapOp, LayerId, Rect};
use dear_imgui_rs::{Key, MouseButton, TextureId, TreeNodeFlags, Ui};
use dear_imgui_wgpu::ExternalTextureId;
use map::TagId;
use tags::PixelFormat;
use tags::bitmap_grow::SpriteView;

use crate::app::image_gen::ImageAction;
use crate::app::textures::{BitmapAction, PaintTool};
use crate::app::{Action, App, Doc};
use crate::theme::{self, Fonts, icon};
use crate::viewport::Gpu;

struct Texture {
    texture: wgpu::Texture,
    id: ExternalTextureId,
    tag: TagId,
    layer: LayerId,
    size: (u32, u32),
    generation: u64,
}

/// A drag with the selection tool: where it began, in image pixels.
#[derive(Clone, Copy)]
struct Marquee {
    from: [f32; 2],
}

/// The tool's UI-only state.
pub struct TexturesUi {
    texture: Option<Texture>,
    /// A brush or eraser drag is in progress.
    painting: bool,
    last: [f32; 2],
    marquee: Option<Marquee>,
    levels: [f32; 3],
    hsv: [f32; 3],
    /// The Resize fields and the image they were filled from.
    resize: Option<((TagId, usize), [i32; 2], PixelFormat)>,
    /// The sequence fields being typed (label, first image, count) and the
    /// sequence they belong to, committed when the field is left.
    seq_edit: Option<((TagId, usize), String, [i32; 2])>,
    /// The sprite fields being typed, committed when a field is left.
    sprite_edit: Option<((TagId, usize, usize), SpriteFields)>,
}

/// A sprite's fields as typed.
#[derive(Clone, Copy)]
struct SpriteFields {
    image: i32,
    /// Left, right, top, bottom.
    bounds: [f32; 4],
    registration: [f32; 2],
}

impl Default for TexturesUi {
    fn default() -> Self {
        TexturesUi {
            texture: None,
            painting: false,
            last: [f32::NAN; 2],
            marquee: None,
            levels: [0.0, 1.0, 1.0],
            hsv: [0.0, 1.0, 1.0],
            resize: None,
            seq_edit: None,
            sprite_edit: None,
        }
    }
}

/// The formats Resize offers: those a save writes.
const FORMATS: [PixelFormat; 12] = [
    PixelFormat::A8,
    PixelFormat::Y8,
    PixelFormat::Ay8,
    PixelFormat::A8Y8,
    PixelFormat::Rgb565,
    PixelFormat::Argb1555,
    PixelFormat::Argb4444,
    PixelFormat::Xrgb8888,
    PixelFormat::Argb8888,
    PixelFormat::Dxt1,
    PixelFormat::Dxt3,
    PixelFormat::Dxt5,
];

fn heading(ui: &Ui, fonts: &Fonts, t: &str) {
    let f = ui.push_font(fonts.bold);
    ui.text(t);
    f.pop();
}

fn short(name: &str) -> &str {
    name.rsplit('\\').next().unwrap_or(name)
}

fn to_f32(c: [u8; 4]) -> [f32; 4] {
    c.map(|v| f32::from(v) / 255.0)
}

fn to_u8(c: [f32; 4]) -> [u8; 4] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

/// The map's bitmaps; a `*` marks one with edits.
pub fn list(ui: &Ui, app: &mut App, _st: &mut TexturesUi, actions: &mut Vec<Action>) {
    if !app.session.is_open() {
        ui.text_disabled("Open a map in the Map tool to edit its bitmaps.");
        return;
    }
    ui.set_next_item_width(-1.0);
    ui.input_text("##textures-filter", &mut app.textures.filter)
        .hint(format!("{} Filter bitmaps", icon::SEARCH))
        .build();
    let needle = app.textures.filter.to_lowercase();
    let tags = app.texture_tags();
    if tags.is_empty() {
        ui.text_disabled("The open map has no bitmaps.");
        return;
    }
    let selected = app.textures.selected;
    for (id, name) in tags
        .iter()
        .filter(|(_, n)| needle.is_empty() || n.to_lowercase().contains(&needle))
    {
        let edited = app
            .textures
            .open
            .get(id)
            .is_some_and(|o| app.textures.edited(o));
        let mark = if edited { "* " } else { "" };
        let label = format!("{mark}{}##tex-{}", short(name), id.0);
        if ui
            .selectable_config(&label)
            .selected(selected == Some(*id))
            .build()
        {
            actions.push(Action::Bitmap(BitmapAction::Select { tag: *id, image: 0 }));
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(name);
        }
    }
}

/// What the canvas header needs of the selected image, owned so the
/// pixels can be borrowed mutably after.
struct Head {
    name: String,
    images: usize,
    kind: String,
    format: String,
    faces: Vec<u32>,
    slices: u32,
    locked: Option<String>,
    error: Option<String>,
    size: Option<(u32, u32)>,
}

fn head(app: &App, tag: TagId, layer: LayerId) -> Option<Head> {
    let open = app.textures.open.get(&tag)?;
    let info = open.images.get(layer.image as usize)?;
    let mut faces: Vec<u32> = info.layers.iter().map(|l| l.face).collect();
    faces.sort_unstable();
    faces.dedup();
    let slices = info.layers.iter().map(|l| l.slice + 1).max().unwrap_or(1);
    Some(Head {
        name: open.name.clone(),
        images: open.images.len(),
        kind: info.kind.clone(),
        format: info.format.clone(),
        faces,
        slices,
        locked: info.locked.clone(),
        error: info.error.clone(),
        size: open
            .bitmap
            .layer(layer)
            .map(|l| (l.canvas.width(), l.canvas.height())),
    })
}

/// The canvas: the layer's pixels at `zoom`, with the tools.
pub fn canvas(
    ui: &Ui,
    app: &mut App,
    st: &mut TexturesUi,
    fonts: &Fonts,
    gpu: &mut Gpu<'_>,
    actions: &mut Vec<Action>,
) {
    let Some((tag, layer)) = app.textures.layer() else {
        ui.text_disabled("Pick a bitmap on the left.");
        return;
    };
    let Some(h) = head(app, tag, layer) else {
        ui.text_disabled("The bitmap is not open.");
        return;
    };
    heading(ui, fonts, short(&h.name));
    ui.same_line();
    ui.text_disabled(format!("{} {}", h.kind, h.format));
    if h.images > 1 {
        ui.same_line();
        ui.set_next_item_width(90.0);
        if let Some(_c) = ui.begin_combo("##tex-image", format!("Image {}", layer.image)) {
            for i in 0..h.images {
                if ui
                    .selectable_config(format!("Image {i}##tex-img-{i}"))
                    .selected(i == layer.image as usize)
                    .build()
                {
                    actions.push(Action::Bitmap(BitmapAction::Select { tag, image: i }));
                }
            }
        }
    }
    if h.faces.len() > 1 {
        ui.same_line();
        ui.set_next_item_width(90.0);
        if let Some(_c) = ui.begin_combo("##tex-face", format!("Face {}", layer.face)) {
            for &f in &h.faces {
                if ui
                    .selectable_config(format!("Face {f}##tex-face-{f}"))
                    .selected(f == layer.face)
                    .build()
                {
                    actions.push(Action::Bitmap(BitmapAction::Layer {
                        face: f,
                        slice: layer.slice,
                    }));
                }
            }
        }
    }
    if h.slices > 1 {
        ui.same_line();
        ui.set_next_item_width(120.0);
        let mut s = layer.slice as i32;
        if ui.slider_i32("Slice##tex-slice", &mut s, 0, h.slices as i32 - 1) {
            actions.push(Action::Bitmap(BitmapAction::Layer {
                face: layer.face,
                slice: s.max(0) as u32,
            }));
        }
    }
    // The tools (B, E, G, I), the zoom and the undo.
    for t in PaintTool::ALL {
        if ui.radio_button_bool(format!("{}##tex-tool-{t:?}", t.label()), app.textures.tool == t) {
            app.textures.tool = t;
        }
        ui.same_line();
    }
    ui.set_next_item_width(110.0);
    ui.slider_f32("Zoom##tex-zoom", &mut app.textures.zoom, 0.25, 32.0);
    if let Some(why) = &h.error {
        ui.text_colored(theme::ERROR, format!("{} {why}", icon::ALERT));
    }
    if let Some(why) = &h.locked {
        ui.text_colored(theme::WARN, format!("{} Read only: {why}", icon::INFO));
    }
    let Some((w, hgt)) = h.size else {
        ui.text_disabled("This image has no pixels to show.");
        return;
    };
    let Some(texture) = upload(st, gpu, app, tag, layer) else {
        ui.text_disabled("The canvas could not create its texture.");
        return;
    };
    let zoom = app.textures.zoom.clamp(0.25, 64.0);
    let origin = ui.cursor_screen_pos();
    ui.image(texture, [w as f32 * zoom, hgt as f32 * zoom]);
    let hovered = ui.is_item_hovered();
    let io = ui.io();
    let m = io.mouse_pos();
    let px = [(m[0] - origin[0]) / zoom, (m[1] - origin[1]) / zoom];
    let inside = px[0] >= 0.0 && px[1] >= 0.0 && px[0] < w as f32 && px[1] < hgt as f32;
    let editable = h.locked.is_none() && h.error.is_none();

    // Keys: B brush, E eraser, G fill, I eyedropper, X swap, [ and ] size.
    if (hovered || ui.is_window_focused()) && !io.want_text_input() {
        let t = &mut app.textures;
        for (key, tool) in [
            (Key::B, PaintTool::Brush),
            (Key::E, PaintTool::Eraser),
            (Key::G, PaintTool::Fill),
            (Key::I, PaintTool::Eyedropper),
        ] {
            if ui.is_key_pressed(key) {
                t.tool = tool;
            }
        }
        if ui.is_key_pressed(Key::X) {
            std::mem::swap(&mut t.fg, &mut t.bg);
        }
        if ui.is_key_pressed(Key::LeftBracket) {
            t.brush.size = (t.brush.size * 0.8).max(1.0);
        }
        if ui.is_key_pressed(Key::RightBracket) {
            t.brush.size = (t.brush.size * 1.25).min(512.0);
        }
    }
    if hovered && (io.key_ctrl() || io.key_super()) && io.mouse_wheel() != 0.0 {
        let f = if io.mouse_wheel() > 0.0 { 1.25 } else { 0.8 };
        app.textures.zoom = (app.textures.zoom * f).clamp(0.25, 32.0);
    }

    // The mouse.
    let tool = app.textures.tool;
    if hovered && ui.is_mouse_clicked(MouseButton::Left) && inside {
        match tool {
            PaintTool::Brush | PaintTool::Eraser if editable => {
                actions.push(Action::Bitmap(BitmapAction::StrokeBegin { tag, layer }));
                actions.push(Action::Bitmap(BitmapAction::StrokeTo { x: px[0], y: px[1] }));
                st.painting = true;
                st.last = px;
            }
            PaintTool::Fill if editable => {
                let t = &app.textures;
                actions.push(Action::Bitmap(BitmapAction::Apply {
                    tag,
                    op: BitmapOp::Fill {
                        layer,
                        at: Some([px[0] as u32, px[1] as u32]),
                        rect: t.selection,
                        color: t.fg,
                        tolerance: t.tolerance,
                        channels: t.channels,
                    },
                }));
            }
            PaintTool::Eyedropper => {
                let c = app
                    .textures
                    .open
                    .get(&tag)
                    .and_then(|o| o.bitmap.layer(layer))
                    .map(|l| l.canvas.pixel(px[0] as u32, px[1] as u32));
                if let Some(c) = c {
                    app.textures.fg = c;
                }
            }
            PaintTool::Select => st.marquee = Some(Marquee { from: px }),
            _ => {}
        }
    }
    if st.painting {
        if ui.is_mouse_down(MouseButton::Left) {
            if px != st.last {
                actions.push(Action::Bitmap(BitmapAction::StrokeTo { x: px[0], y: px[1] }));
                st.last = px;
            }
        } else {
            actions.push(Action::Bitmap(BitmapAction::StrokeEnd));
            st.painting = false;
        }
    }
    let dl = ui.get_window_draw_list();
    let at = |p: [f32; 2]| [origin[0] + p[0] * zoom, origin[1] + p[1] * zoom];
    let clamp = |p: [f32; 2]| [p[0].clamp(0.0, w as f32), p[1].clamp(0.0, hgt as f32)];
    if let Some(mq) = st.marquee {
        let (a, b) = (clamp(mq.from), clamp(px));
        dl.add_rect(at(a), at(b), theme::WIDGET_FOCUS).thickness(1.0).build();
        if !ui.is_mouse_down(MouseButton::Left) {
            st.marquee = None;
            let (x0, y0) = (a[0].min(b[0]).floor() as u32, a[1].min(b[1]).floor() as u32);
            let (x1, y1) = (a[0].max(b[0]).ceil() as u32, a[1].max(b[1]).ceil() as u32);
            app.textures.selection = (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0));
        }
    } else if let Some(r) = app.textures.selection {
        dl.add_rect(
            at([r.x as f32, r.y as f32]),
            at([r.right() as f32, r.bottom() as f32]),
            theme::WIDGET_FOCUS,
        )
        .thickness(1.0)
        .build();
    }
    // The sprites cut from this image, outlined over it.
    if let Some(open) = app.textures.open.get(&tag) {
        let mut i = 0;
        while let Some(seq) = open.sequence(i) {
            for rec in &seq.sprites {
                let v = SpriteView::read(rec);
                if i32::from(v.image) == layer.image as i32 {
                    let (fw, fh) = (w as f32, hgt as f32);
                    dl.add_rect(
                        at([v.left * fw, v.top * fh]),
                        at([v.right * fw, v.bottom * fh]),
                        theme::SCREEN_WIDGET,
                    )
                    .thickness(1.0)
                    .build();
                }
            }
            i += 1;
        }
    }
    if hovered && matches!(tool, PaintTool::Brush | PaintTool::Eraser) {
        let r = app.textures.brush.size * 0.5 * zoom;
        dl.add_circle(m, r.max(1.0), theme::WIDGET_FOCUS).thickness(1.0).build();
    }
    if hovered && inside {
        let c = app
            .textures
            .open
            .get(&tag)
            .and_then(|o| o.bitmap.layer(layer))
            .map(|l| l.canvas.pixel(px[0] as u32, px[1] as u32));
        if let Some(c) = c {
            ui.tooltip_text(format!(
                "({}, {})  {} {} {} {}",
                px[0] as u32, px[1] as u32, c[0], c[1], c[2], c[3]
            ));
        }
    }
}

fn write(gpu: &mut Gpu<'_>, texture: &wgpu::Texture, r: Rect, rgba: &[u8]) {
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: r.x, y: r.y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * r.w),
            rows_per_image: Some(r.h),
        },
        wgpu::Extent3d {
            width: r.w,
            height: r.h,
            depth_or_array_layers: 1,
        },
    );
}

/// The layer on the GPU: made whole when the layer (or the bitmap's
/// decode) changes, then updated by the rectangles edits touched.
fn upload(
    st: &mut TexturesUi,
    gpu: &mut Gpu<'_>,
    app: &mut App,
    tag: TagId,
    layer: LayerId,
) -> Option<TextureId> {
    let generation = app.textures.generation;
    let open = app.textures.open.get_mut(&tag)?;
    let dirty = open.bitmap.take_view_dirty();
    let canvas = &open.bitmap.layer(layer)?.canvas;
    let (w, h) = (canvas.width(), canvas.height());
    let fresh = st
        .texture
        .as_ref()
        .is_none_or(|t| (t.tag, t.layer, t.size, t.generation) != (tag, layer, (w, h), generation));
    if fresh {
        if let Some(old) = st.texture.take() {
            let _ = gpu.renderer.unregister_external_texture(old.id);
        }
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("texture canvas"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Not sRGB: Dear ImGui takes texels as sRGB-encoded already,
            // and the canvas is.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let id = match gpu.renderer.register_external_texture(&view) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("texture canvas: {e}");
                return None;
            }
        };
        write(gpu, &texture, canvas.bounds(), &canvas.to_rgba());
        st.texture = Some(Texture {
            texture,
            id,
            tag,
            layer,
            size: (w, h),
            generation,
        });
    } else if let Some(t) = &st.texture {
        for d in dirty.iter().filter(|d| d.layer == layer) {
            let (r, data) = canvas.read_rect(d.rect);
            if !r.is_empty() {
                write(gpu, &t.texture, r, &data);
            }
        }
    }
    st.texture.as_ref().map(|t| t.id.texture_id())
}

/// The tool's settings, the image, the adjustments and PNG in and out.
pub fn inspector(
    ui: &Ui,
    app: &mut App,
    st: &mut TexturesUi,
    dialogs: bool,
    actions: &mut Vec<Action>,
) {
    let Some((tag, layer)) = app.textures.layer() else {
        ui.text_disabled("Pick a bitmap to edit it.");
        return;
    };
    let editable = head(app, tag, layer).is_some_and(|h| h.locked.is_none() && h.error.is_none());
    if ui.collapsing_header("Tool##tex-tool", TreeNodeFlags::DEFAULT_OPEN) {
        let t = &mut app.textures;
        ui.slider_f32("Size##tex-size", &mut t.brush.size, 1.0, 256.0);
        ui.slider_f32("Hardness##tex-hardness", &mut t.brush.hardness, 0.0, 1.0);
        ui.slider_f32("Opacity##tex-opacity", &mut t.brush.opacity, 0.0, 1.0);
        ui.slider_f32("Spacing##tex-spacing", &mut t.brush.spacing, 0.05, 2.0);
        ui.slider_f32("Fill tolerance##tex-tol", &mut t.tolerance, 0.0, 1.0);
        let mut fg = to_f32(t.fg);
        if ui.color_edit4("Colour##tex-fg", &mut fg) {
            t.fg = to_u8(fg);
        }
        let mut bg = to_f32(t.bg);
        if ui.color_edit4("Other colour (X swaps)##tex-bg", &mut bg) {
            t.bg = to_u8(bg);
        }
        ui.text("Channels");
        ui.same_line();
        ui.checkbox("R##tex-r", &mut t.channels.r);
        ui.same_line();
        ui.checkbox("G##tex-g", &mut t.channels.g);
        ui.same_line();
        ui.checkbox("B##tex-b", &mut t.channels.b);
        ui.same_line();
        ui.checkbox("A##tex-a", &mut t.channels.a);
        if !t.channels.any() {
            t.channels.a = true;
        }
    }
    if ui.collapsing_header("Selection##tex-sel", TreeNodeFlags::DEFAULT_OPEN) {
        match app.textures.selection {
            Some(r) => {
                ui.text(format!("{} by {} at {}, {}", r.w, r.h, r.x, r.y));
                if ui.button("Clear selection##tex-clear") {
                    app.textures.selection = None;
                }
            }
            None => ui.text_disabled("None: fill, adjust and import take the whole layer."),
        }
    }
    let d = ui.begin_disabled_with_cond(!editable);
    if ui.collapsing_header("Adjust##tex-adjust", TreeNodeFlags::NONE) {
        let (rect, channels) = (app.textures.selection, app.textures.channels);
        let mut adjust = |a: Adjust| {
            actions.push(Action::Bitmap(BitmapAction::Apply {
                tag,
                op: BitmapOp::Adjust {
                    layer,
                    adjust: a,
                    rect,
                    channels,
                },
            }));
        };
        if ui.button("Invert##tex-invert") {
            adjust(Adjust::Invert);
        }
        ui.separator();
        ui.slider_f32("Black##tex-black", &mut st.levels[0], 0.0, 0.99);
        ui.slider_f32("White##tex-white", &mut st.levels[1], 0.01, 1.0);
        ui.slider_f32("Gamma##tex-gamma", &mut st.levels[2], 0.1, 5.0);
        if ui.button("Apply levels##tex-levels") {
            adjust(Adjust::Levels {
                black: st.levels[0],
                white: st.levels[1],
                gamma: st.levels[2],
            });
        }
        ui.separator();
        ui.slider_f32("Hue##tex-hue", &mut st.hsv[0], -180.0, 180.0);
        ui.slider_f32("Saturation##tex-sat", &mut st.hsv[1], 0.0, 2.0);
        ui.slider_f32("Value##tex-val", &mut st.hsv[2], 0.0, 2.0);
        if ui.button("Apply hue, saturation, value##tex-hsv") {
            adjust(Adjust::Hsv {
                hue: st.hsv[0],
                saturation: st.hsv[1],
                value: st.hsv[2],
            });
        }
    }
    d.end();
    if ui.collapsing_header("Image##tex-image-info", TreeNodeFlags::DEFAULT_OPEN) {
        if let Some(info) = app
            .textures
            .open
            .get(&tag)
            .and_then(|o| o.images.get(layer.image as usize))
        {
            ui.text(format!(
                "{} by {}, depth {}, {} {}",
                info.width, info.height, info.depth, info.kind, info.format
            ));
            match &info.added {
                Some(a) => ui.text(format!(
                    "Added, from image {}: the save writes it whole",
                    a.from
                )),
                None => ui.text(format!(
                    "{} mips stored; pixels in the {} file",
                    info.mips, info.pixels
                )),
            }
        }
        image_shape(ui, app, st, tag, layer.image as usize, actions);
        ui.text_wrapped(
            "Edits stay in the editor until the map is saved. An added image or sequence moves the whole bitmap tag to the end of the map's metadata when it is saved.",
        );
    }
    if ui.collapsing_header("PNG##tex-png", TreeNodeFlags::DEFAULT_OPEN) {
        let d = ui.begin_disabled_with_cond(!editable);
        if ui.button(format!("{} Import...##tex-import", icon::FOLDER_OPEN))
            && let Some(path) = import_dialog(dialogs)
        {
            actions.push(Action::Bitmap(BitmapAction::Import {
                tag,
                layer,
                rect: app.textures.selection,
                path,
            }));
        }
        ui.same_line();
        if ui.button(format!("{} Generate...##tex-generate", icon::ZAP)) {
            let name = app.textures.open.get(&tag).map(|o| o.name.clone()).unwrap_or_default();
            actions.push(Action::Image(ImageAction::Open {
                bitmap: name,
                image: layer.image as usize,
            }));
        }
        ui.set_item_tooltip("Make a new picture for this image from a prompt and references you pick");
        d.end();
        ui.same_line();
        if ui.button(format!("{} Export...##tex-export", icon::SAVE)) {
            let name = app
                .textures
                .open
                .get(&tag)
                .map(|o| short(&o.name).to_owned())
                .unwrap_or_else(|| "bitmap".to_owned());
            if let Some(path) = export_dialog(dialogs, &name) {
                actions.push(Action::Bitmap(BitmapAction::Export {
                    tag,
                    layer,
                    mip: 0,
                    path,
                }));
            }
        }
    }
    ui.separator();
    // The bitmap's document: its paint and shape steps beside the bitmap's
    // own writes, as Cmd+Z takes them (docs/editor/studio-flow.md, "Shared
    // patterns").
    let (can_undo, can_redo) = app.doc_can(Doc::Tag(tag));
    let d = ui.begin_disabled_with_cond(!can_undo);
    if ui.button(format!("{} Undo##tex-undo", icon::UNDO)) {
        actions.push(Action::UndoDoc(Doc::Tag(tag)));
    }
    d.end();
    ui.same_line();
    let d = ui.begin_disabled_with_cond(!can_redo);
    if ui.button(format!("{} Redo##tex-redo", icon::REDO)) {
        actions.push(Action::RedoDoc(Doc::Tag(tag)));
    }
    d.end();
    ui.same_line();
    if ui.button("Revert this bitmap##tex-revert") {
        actions.push(Action::Bitmap(BitmapAction::Revert(tag)));
    }
}

/// Resize, Add image and the sequences of the bitmap on the canvas.
fn image_shape(
    ui: &Ui,
    app: &mut App,
    st: &mut TexturesUi,
    tag: TagId,
    image: usize,
    actions: &mut Vec<Action>,
) {
    let Some(open) = app.textures.open.get(&tag) else {
        return;
    };
    let Some(info) = open.images.get(image) else {
        return;
    };
    let writable = info.locked.is_none() && info.error.is_none();
    let asked = app.textures.reshape.get(&(tag, image)).copied();
    let stored = info.source.as_ref().map(|s| {
        (
            [i32::from(s.dimensions[0]), i32::from(s.dimensions[1])],
            s.format,
        )
    });
    let Some((stored_size, stored_format)) = stored else {
        return;
    };
    let now = asked.map_or((stored_size, stored_format), |r| {
        ([r.width as i32, r.height as i32], r.format)
    });
    if st.resize.as_ref().is_none_or(|(k, ..)| *k != (tag, image)) {
        st.resize = Some(((tag, image), now.0, now.1));
    }
    let movable = open.parts.as_ref().err().cloned();
    let sequences = open.sequences();
    let images = open.images.len();
    let name = open.name.clone();

    ui.separator();
    let d = ui.begin_disabled_with_cond(!writable);
    let (_, size, format) = st.resize.as_mut().expect("just set");
    ui.set_next_item_width(90.0);
    ui.input_int("W##tex-resize-w", &mut size[0]);
    ui.same_line();
    ui.set_next_item_width(90.0);
    ui.input_int("H##tex-resize-h", &mut size[1]);
    ui.set_next_item_width(140.0);
    let preview = format!("{format:?}");
    if let Some(_c) = ui.begin_combo("Format##tex-resize-format", &preview) {
        for f in FORMATS {
            let label = format!("{f:?}");
            if ui.selectable_config(&label).selected(f == *format).build() {
                *format = f;
            }
        }
    }
    ui.same_line();
    if ui.button("Resize##tex-resize") {
        let (w, h) = (size[0].clamp(1, 4096) as u32, size[1].clamp(1, 4096) as u32);
        actions.push(Action::Bitmap(BitmapAction::Reshape {
            tag,
            image,
            width: Some(w),
            height: Some(h),
            format: Some(*format),
        }));
    }
    ui.set_item_tooltip(
        "Save this image at a new size or format: every layer resampled, every mip written again. One undo step.",
    );
    if let Some(r) = asked {
        ui.text_disabled(format!(
            "Saves as {} by {} {:?} (stored {} by {} {stored_format:?})",
            r.width, r.height, r.format, stored_size[0], stored_size[1]
        ));
    }
    d.end();
    let d = ui.begin_disabled_with_cond(movable.is_some() || info_kind_is_not_2d(app, tag, image));
    if ui.button(format!("{} Add image##tex-add-image", icon::PLUS)) {
        actions.push(Action::Bitmap(BitmapAction::AddImage {
            tag,
            from: Some(image),
            width: None,
            height: None,
            format: None,
        }));
    }
    ui.set_item_tooltip(
        "Add an image after the last, the size and format of this one, transparent until you paint, import or generate into it",
    );
    ui.same_line();
    if ui.button(format!("{} New image, generated...##tex-add-generate", icon::ZAP)) {
        actions.push(Action::Image(ImageAction::NewSlot {
            bitmap: name.clone(),
            like: image,
        }));
    }
    d.end();
    if let Some(why) = &movable {
        ui.text_disabled(format!("No image can be added: {why}"));
    }
    if let Some(_node) = ui
        .tree_node_config("##tex-sequences")
        .label(format!("Sequences ({})", sequences.len()))
        .push()
    {
        if sequences.is_empty() {
            ui.text_disabled("None: the bitmap's images are picked by index.");
        }
        for s in &sequences {
            sequence_rows(ui, app, st, tag, s, images, movable.is_some(), actions);
        }
        let d = ui.begin_disabled_with_cond(movable.is_some() || image >= images);
        if ui.button(format!("{} Add a sequence for image {image}##tex-add-seq", icon::PLUS)) {
            actions.push(Action::Bitmap(BitmapAction::AddSequence {
                tag,
                first: image,
                count: 1,
                label: String::new(),
            }));
        }
        d.end();
    }
}

fn info_kind_is_not_2d(app: &App, tag: TagId, image: usize) -> bool {
    app.textures
        .open
        .get(&tag)
        .and_then(|o| o.images.get(image))
        .is_none_or(|i| i.kind != "D2")
}

/// A PNG to import.
pub fn import_dialog(dialogs: bool) -> Option<PathBuf> {
    if !dialogs {
        return None;
    }
    rfd::FileDialog::new()
        .set_title("Import a PNG")
        .add_filter("PNG", &["png"])
        .pick_file()
}

/// Where to write a PNG.
pub fn export_dialog(dialogs: bool, name: &str) -> Option<PathBuf> {
    if !dialogs {
        return None;
    }
    rfd::FileDialog::new()
        .set_title("Export as PNG")
        .add_filter("PNG", &["png"])
        .set_file_name(format!("{name}.png"))
        .save_file()
}

/// The File menu's items for the Textures tool.
pub fn file_menu(ui: &Ui, app: &App, dialogs: bool, actions: &mut Vec<Action>) {
    let layer = app.textures.layer();
    let can = layer.is_some();
    if ui.menu_item_enabled_selected_with_shortcut(
        format!("{} Import PNG...", icon::FOLDER_OPEN),
        "",
        false,
        can,
    ) && let Some((tag, layer)) = layer
        && let Some(path) = import_dialog(dialogs)
    {
        actions.push(Action::Bitmap(BitmapAction::Import {
            tag,
            layer,
            rect: app.textures.selection,
            path,
        }));
    }
    if ui.menu_item_enabled_selected_with_shortcut(
        format!("{} Export PNG...", icon::SAVE),
        "",
        false,
        can,
    ) && let Some((tag, layer)) = layer
    {
        let name = app
            .textures
            .open
            .get(&tag)
            .map(|o| short(&o.name).to_owned())
            .unwrap_or_else(|| "bitmap".to_owned());
        if let Some(path) = export_dialog(dialogs, &name) {
            actions.push(Action::Bitmap(BitmapAction::Export {
                tag,
                layer,
                mip: 0,
                path,
            }));
        }
    }
}

/// One sequence: its label, first image and count, and its sprites, each
/// field committed (one undo step) when it is left.
#[allow(clippy::too_many_arguments)]
fn sequence_rows(
    ui: &Ui,
    app: &App,
    st: &mut TexturesUi,
    tag: TagId,
    s: &crate::app::textures::SequenceInfo,
    images: usize,
    locked: bool,
    actions: &mut Vec<Action>,
) {
    let label = if s.label.is_empty() { "(no label)" } else { &s.label };
    let last = i32::from(s.first) + i32::from(s.count) - 1;
    let title = format!(
        "{}  {label}: images {} to {last}, {} sprites{}##tex-seq-{}",
        s.index,
        s.first,
        s.sprites,
        if s.added { " (added)" } else { "" },
        s.index,
    );
    let Some(_node) = ui.tree_node_config(&title).push() else {
        return;
    };
    let d = ui.begin_disabled_with_cond(locked);
    let key = (tag, s.index);
    if st.seq_edit.as_ref().is_none_or(|(k, ..)| *k != key) {
        st.seq_edit = Some((key, s.label.clone(), [i32::from(s.first), i32::from(s.count)]));
    }
    let (_, text, range) = st.seq_edit.as_mut().expect("just set");
    let mut done = false;
    ui.set_next_item_width(160.0);
    ui.input_text(format!("Label##tex-seq-label-{}", s.index), text)
        .build();
    done |= ui.is_item_deactivated_after_edit();
    ui.set_next_item_width(70.0);
    ui.input_int(format!("First##tex-seq-first-{}", s.index), &mut range[0]);
    done |= ui.is_item_deactivated_after_edit();
    ui.same_line();
    ui.set_next_item_width(70.0);
    ui.input_int(format!("Count##tex-seq-count-{}", s.index), &mut range[1]);
    done |= ui.is_item_deactivated_after_edit();
    if done {
        let (_, text, range) = st.seq_edit.take().expect("set above");
        actions.push(Action::Bitmap(BitmapAction::SetSequence {
            tag,
            sequence: s.index,
            label: Some(text),
            first: usize::try_from(range[0]).ok(),
            count: u16::try_from(range[1]).ok(),
        }));
    }
    let open = &app.textures.open[&tag];
    let mut remove = None;
    for k in 0..s.sprites {
        let Some(v) = open.sprite(s.index, k) else {
            break;
        };
        let key = (tag, s.index, k);
        let mut f = match st.sprite_edit {
            Some((kk, f)) if kk == key => f,
            _ => SpriteFields {
                image: i32::from(v.image),
                bounds: [v.left, v.right, v.top, v.bottom],
                registration: v.registration,
            },
        };
        let (mut changed, mut done) = (false, false);
        ui.text(format!("Sprite {k}"));
        ui.same_line();
        ui.set_next_item_width(60.0);
        changed |= ui.input_int(format!("Image##tex-sp-image-{}-{k}", s.index), &mut f.image);
        done |= ui.is_item_deactivated_after_edit();
        for (n, name) in ["Left", "Right", "Top", "Bottom"].into_iter().enumerate() {
            ui.set_next_item_width(70.0);
            changed |= ui.input_float(
                format!("{name}##tex-sp-b{n}-{}-{k}", s.index),
                &mut f.bounds[n],
            );
            done |= ui.is_item_deactivated_after_edit();
            if n % 2 == 0 {
                ui.same_line();
            }
        }
        for (n, name) in ["Reg x", "Reg y"].into_iter().enumerate() {
            ui.set_next_item_width(70.0);
            changed |= ui.input_float(
                format!("{name}##tex-sp-r{n}-{}-{k}", s.index),
                &mut f.registration[n],
            );
            done |= ui.is_item_deactivated_after_edit();
            ui.same_line();
        }
        if ui.small_button(format!("{} Remove##tex-sp-rm-{}-{k}", icon::TRASH, s.index)) {
            remove = Some(k);
        }
        if done {
            st.sprite_edit = None;
            actions.push(Action::Bitmap(BitmapAction::SetSprite {
                tag,
                sequence: s.index,
                sprite: Some(k),
                image: usize::try_from(f.image).ok(),
                bounds: Some(f.bounds),
                registration: Some(f.registration),
            }));
        } else if changed {
            st.sprite_edit = Some((key, f));
        }
    }
    if let Some(sprite) = remove {
        st.sprite_edit = None;
        actions.push(Action::Bitmap(BitmapAction::RemoveSprite {
            tag,
            sequence: s.index,
            sprite,
        }));
    }
    if ui.button(format!("{} Add a sprite##tex-sp-add-{}", icon::PLUS, s.index)) {
        actions.push(Action::Bitmap(BitmapAction::SetSprite {
            tag,
            sequence: s.index,
            sprite: None,
            image: None,
            bounds: None,
            registration: None,
        }));
    }
    ui.set_item_tooltip(
        "A copy of the last sprite, or over all of the first image. Bounds are fractions of the image; the sprite shows outlined on the canvas.",
    );
    d.end();
}
