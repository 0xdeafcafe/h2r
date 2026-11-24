//! The Shader tab of a `shad` (docs/editor/rich-editors.md, "Shaders",
//! slice S1): a header naming the template, the level of detail and
//! every refusal of the preview; the preview on a sphere, a flat square
//! or an object's model (a picker dialog chooses); then the slots in
//! folding sections: **Bitmaps** with thumbnails and a bitmap picker that
//! shows them, **Colours** as the panel's own colour rows (the C1 dialog),
//! **Vectors** as four numbers, **Animation** as the preview clock and, per
//! animated property, its curve (the curve editor's plot and dialog), its
//! value at the clock and what it drives (slice S2), **Code** as each pass's vertex and pixel programs
//! (listing, translated WGSL and the translator's warnings; slice S4,
//! read only) and an **Override** per pass (slice S5: the linked WGSL edited, checked, pushed to the paired game and reverted). Each row has its `Used by` line.
//!
//! The tab reads through `app::shader` (the shown map with the
//! document's edits) and writes through the field panel's own commands:
//! a bitmap pick is `tag.set`'s write at the slot's path, a colour the
//! colour dialog's. It holds no document state: what it caches is keyed
//! by the document's revision.

use std::collections::HashMap;
use std::sync::Arc;

use dear_imgui_rs::{
    ColorButtonFlags, InputTextMultilineFlags, ListClipper, TreeNodeFlags, Ui, WindowFlags,
};
use dear_imgui_wgpu::ExternalTextureId;
use map::{FourCC, TagId};
use scenario_edit::native_view::code::{PassCode, ProgramCode};
use scenario_edit::native_view::preview::{self as material, Shape};
use serde_json::{Value, json};

use super::rich::{FIELDS_TAB, ViewCx};
use super::{PanelState, colour, curve, picker};
use crate::App;
use crate::app::map_pack::Picture;
use crate::app::shader::{
    Animated, BACKGROUND, CONSTANTS_HELD, NAMES_NOT_READ, PreviewPasses, ShaderRead, Slot,
    SlotKind, SlotValue,
};
use crate::app::shader_override as ovr;
use crate::app::{Action, NavAction, TagAction, TagDoc, tag_api};
use crate::theme::{self, icon};
use crate::viewport::Gpu;

const BITM: FourCC = FourCC(*b"bitm");
const OBJE: FourCC = FourCC(*b"obje");
/// A collection stands for its first entry, as the viewport draws it.
const COLLECTIONS: [FourCC; 2] = [FourCC(*b"itmc"), FourCC(*b"vehc")];

/// The preview picture's size, in pixels.
const PICTURE: [u32; 2] = [320, 240];
/// A thumbnail's side, in points.
const THUMB: f32 = 40.0;
/// The bitmap picker's width and the rows it shows before it scrolls.
const PICK_WIDTH: f32 = 460.0;
const PICK_ROWS: usize = 9;
/// Thumbnails decoded per frame while the picker scrolls.
const THUMBS_PER_FRAME: usize = 2;

/// What the cached read and picture are of.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    tag: TagId,
    rev: u64,
    map: Option<String>,
}

/// The drawn preview on the GPU.
struct Preview {
    id: ExternalTextureId,
    _texture: wgpu::Texture,
    width: u32,
    height: u32,
    passes: PreviewPasses,
}

/// The bitmap picker while open: the slot it writes.
struct Pick {
    slot: usize,
    query: String,
    opening: bool,
}

/// The shape picker while open.
struct ShapePick {
    query: String,
    opening: bool,
}

/// The tab's state: what it shows, never what the document holds.
#[derive(Default)]
pub struct ShaderUi {
    shape: Shape,
    read: Option<(Key, Result<ShaderRead, String>)>,
    preview: Option<(Key, Shape, Result<Preview, String>)>,
    thumbs: HashMap<TagId, Result<Arc<Picture>, String>>,
    thumbs_map: Option<String>,
    pick: Option<Pick>,
    shape_pick: Option<ShapePick>,
    /// The preview clock in seconds, and whether it runs. The picture is
    /// drawn at `drawn_time`; it is not redrawn while the clock runs.
    time: f64,
    playing: bool,
    drawn_time: f64,
    /// The Code section's passes read so far, by pass number
    /// (implementation 0), for one document revision.
    code: Option<(Key, HashMap<usize, Result<PassCode, String>>)>,
    /// The game's program id typed into the Override box (16 hex digits).
    program: String,
}

/// The tab: [`super::rich::TagView::draw`] of the `shader` view.
pub fn draw(ui: &Ui, app: &App, tag: TagId, cx: &mut ViewCx<'_, '_>, actions: &mut Vec<Action>) {
    let doc = app.shown_tags();
    let key = Key {
        tag,
        rev: doc.rev(),
        map: app.shown_map_name().map(str::to_owned),
    };
    let st = &mut *cx.shader;
    if st.thumbs_map != key.map {
        st.thumbs.clear();
        st.thumbs_map = key.map.clone();
    }
    if st.read.as_ref().is_none_or(|(k, _)| *k != key) {
        st.read = Some((key.clone(), app.shader_read(tag)));
    }
    let read = match st.read.as_ref().map(|(_, r)| r) {
        Some(Ok(r)) => r.clone(),
        Some(Err(e)) => {
            ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT));
            ui.text_disabled("The Fields tab shows every field of this tag.");
            return;
        }
        None => return,
    };
    header(ui, app, &read, actions);
    preview(ui, app, &read, &key, cx);
    if !read.names_known {
        ui.text_disabled(NAMES_NOT_READ);
    }
    if !read.bitmaps.is_empty()
        && ui.collapsing_header(
            format!("Bitmaps ({})##shader-bitmaps", read.bitmaps.len()),
            TreeNodeFlags::DEFAULT_OPEN,
        )
    {
        for s in &read.bitmaps {
            bitmap_row(ui, app, &read, s, cx.gpu, cx.pictures, cx.shader, actions);
        }
    }
    if !read.colours.is_empty()
        && ui.collapsing_header(
            format!("Colours ({})##shader-colours", read.colours.len()),
            TreeNodeFlags::DEFAULT_OPEN,
        )
    {
        for s in &read.colours {
            colour_row(ui, doc, &read, s, cx.panel);
        }
    }
    if !read.vectors.is_empty()
        && ui.collapsing_header(
            format!("Vectors ({})##shader-vectors", read.vectors.len()),
            TreeNodeFlags::DEFAULT_OPEN,
        )
    {
        for s in &read.vectors {
            vector_row(ui, &read, s, cx.panel, actions);
        }
    }
    if !read.animations.is_empty()
        && ui.collapsing_header(
            format!("Animation ({})##shader-animation", read.animations.len()),
            TreeNodeFlags::empty(),
        )
    {
        clock(ui, cx.shader, &read);
        let values = read.values_at(cx.shader.time);
        for (a, v) in read.animations.iter().zip(&values) {
            animation_row(ui, doc, &read, a, v, cx.shader.time, cx.panel);
        }
        ui.text_disabled(CONSTANTS_HELD);
    }
    code_section(ui, app, &read, &key, cx.shader, actions);
    bitmap_picker(ui, app, &read, cx, actions);
    shape_picker(ui, app, cx.shader);
    colour::draw(ui, doc, cx.panel, actions);
    curve::draw(ui, doc, cx.panel, actions);
}

/// The preview clock: play or pause, and the time in the loop. Dragging
/// it stops it; the picture is redrawn at the time it is left at.
fn clock(ui: &Ui, st: &mut ShaderUi, read: &ShaderRead) {
    let length = read.clock_length();
    if st.playing {
        st.time = (st.time + f64::from(ui.io().delta_time())) % length;
    } else {
        st.time = st.time.clamp(0.0, length);
    }
    if ui.button(if st.playing {
        "Pause##shader-clock"
    } else {
        "Play##shader-clock"
    }) {
        st.playing = !st.playing;
    }
    ui.same_line();
    let mut t = st.time as f32;
    if ui
        .slider_config("Preview clock (s)##shader-clock-time", 0.0, length as f32)
        .build(&mut t)
    {
        st.time = f64::from(t);
        st.playing = false;
    }
}

/// One animated property: what drives it, its curve (the small plot with
/// the clock's place marked, and **Edit curve...**), its value at the
/// clock, what it drives and any target the plan does not animate.
fn animation_row(
    ui: &Ui,
    doc: &TagDoc,
    read: &ShaderRead,
    a: &Animated,
    value: &Result<f32, String>,
    time: f64,
    panel: &mut PanelState,
) {
    let _id = ui.push_id(format!("shader-animation-{}", a.index).as_str());
    let input = match a.input {
        Some(s) => format!("driven by {}", doc.string(s).unwrap_or("an object function")),
        None => match a.period {
            Some(p) => format!("game time, every {p} s"),
            None => "game time".to_owned(),
        },
    };
    ui.text(format!("Property {}: {input}", a.index));
    let path = a.path();
    match tag_api::resolve(doc, read.tag, &path) {
        Ok((at, layout, field)) => {
            let address = at + field.offset;
            match curve::function(doc, read.tag, address) {
                Ok((_, _, f)) => {
                    let marker = a
                        .period
                        .filter(|p| p.is_finite() && *p > 0.0)
                        .map(|p| ((time / f64::from(p)) % 1.0) as f32);
                    curve::animation_plot(ui, &f, marker);
                    ui.same_line();
                    if ui.small_button("Edit curve...##shader-curve") {
                        panel.curve = Some(curve::Open::new(
                            read.tag,
                            address,
                            &field,
                            &layout,
                            &format!("Property {}", a.index),
                        ));
                    }
                }
                Err(e) => ui.text_disabled(format!("    No curve: {e}")),
            }
        }
        Err(e) => ui.text_disabled(format!("    No curve: {e}")),
    }
    match value {
        Ok(v) => ui.text_disabled(format!("    {v:.3} at {time:.2} s")),
        Err(why) => ui.text_disabled(format!("    No value: {why}")),
    }
    for d in &a.drives {
        ui.text_disabled(format!("    drives {d}"));
    }
    if let Some(why) = a.refusal() {
        ui.text_colored(theme::WARN, format!("{} {why}", icon::ALERT));
    }
}

/// The **Code** section: a node per pass, read when it is opened.
fn code_section(
    ui: &Ui,
    app: &App,
    read: &ShaderRead,
    key: &Key,
    st: &mut ShaderUi,
    actions: &mut Vec<Action>,
) {
    let total: usize = read.passes.values().sum();
    if total == 0
        || !ui.collapsing_header(
            format!("Code ({total})##shader-code"),
            TreeNodeFlags::empty(),
        )
    {
        return;
    }
    ui.text_disabled(
        "What the renderer is given. The listings are read only and nothing here is written into the map; an Override edits the linked program for the game's renderer.",
    );
    if st.code.as_ref().is_none_or(|(k, _)| k != key) {
        st.code = Some((key.clone(), HashMap::new()));
    }
    let layers: Vec<usize> = read
        .passes
        .iter()
        .flat_map(|(&l, &n)| std::iter::repeat_n(l, n))
        .collect();
    for (pass, layer) in layers.into_iter().enumerate() {
        let Some(_node) = ui
            .tree_node_config(format!("shader-code-{pass}"))
            .label(format!("Pass {pass} in layer {layer}"))
            .push()
        else {
            continue;
        };
        let Some((_, cache)) = st.code.as_mut() else {
            continue;
        };
        let code = cache
            .entry(pass)
            .or_insert_with(|| app.shader_code(read.tag, pass, 0));
        match code {
            Err(e) => ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT)),
            Ok(c) => {
                if c.implementations > 1 {
                    ui.text_disabled(format!(
                        "Implementation 0 of {}; `shader.code` reads the others.",
                        c.implementations
                    ));
                }
                program(ui, &format!("{pass}-vertex"), "Vertex program", &c.vertex);
                program(ui, &format!("{pass}-pixel"), "Pixel program", &c.pixel);
                let ok = OverrideKey {
                    key: ovr::Key { tag: read.tag, pass, implementation: 0 },
                    linked: &c.linked,
                };
                override_box(ui, app, &ok, &mut st.program, actions);
            }
        }
    }
}

/// The pass an override box is for and the text it starts from.
struct OverrideKey<'a> {
    key: ovr::Key,
    linked: &'a Result<String, String>,
}

/// The **Override** node of a pass: the linked program as editable text,
/// its check under it, and Push, Revert and Discard. The override changes
/// the game's renderer only; the map is not touched.
fn override_box(
    ui: &Ui,
    app: &App,
    o: &OverrideKey<'_>,
    program: &mut String,
    actions: &mut Vec<Action>,
) {
    let id = format!("shader-override-{}", o.key.pass);
    let Some(_node) = ui.tree_node_config(id.clone()).label("Override (WGSL)").push() else {
        return;
    };
    ui.text_disabled(
        "Changes how our renderer draws: saved in the override directory and sent to the paired game; never written into the map.",
    );
    let draft = app.shader_overrides.draft(&o.key);
    let Some(start) = draft.or(o.linked.as_deref().ok()) else {
        if let Err(why) = o.linked {
            ui.text_colored(theme::WARN, format!("{} {why}", icon::ALERT));
        }
        return;
    };
    let mut text = start.to_owned();
    let lines = text.lines().count().clamp(4, 24) as f32;
    ui.input_text_multiline(
        format!("##{id}-text"),
        &mut text,
        [-1.0, lines * ui.text_line_height_with_spacing()],
    )
    .flags(InputTextMultilineFlags::ALLOW_TAB_INPUT)
    .build();
    if text != start {
        actions.push(Action::ShaderOverride(ovr::OverrideAction::Edit(o.key, text.clone())));
    }
    let checked = ovr::check(&text);
    match &checked {
        Ok(()) => ui.text_disabled("Parses; the game checks its interface against the program."),
        Err(e) => ui.text_colored(
            theme::WARN,
            format!("{} {}", icon::ALERT, ovr::describe(e)),
        ),
    }
    ui.text("Game program");
    ui.same_line();
    ui.input_text(format!("##{id}-program"), program)
        .hint("16 hex digits (render.programs)")
        .build();
    let id_ok = ovr::parse_program(program.trim());
    if ui.button(format!("Push##{id}")) {
        match id_ok {
            Some(p) if checked.is_ok() => {
                actions.push(Action::ShaderOverride(ovr::OverrideAction::Push(o.key, p)));
            }
            _ => {}
        }
    }
    if ui.is_item_hovered() {
        ui.tooltip_text(match (&checked, id_ok) {
            (Err(_), _) => "The text does not parse.",
            (_, None) => "Name the game's program: 16 hex digits from render.programs.",
            _ => "Write the override file and tell the paired game to read it.",
        });
    }
    let pushed = app.shader_overrides.pushed_for(&o.key);
    ui.same_line();
    if ui.button(format!("Revert##{id}")) && !pushed.is_empty() {
        actions.push(Action::ShaderOverride(ovr::OverrideAction::Revert(o.key)));
    }
    ui.same_line();
    if ui.button(format!("Discard edits##{id}")) {
        actions.push(Action::ShaderOverride(ovr::OverrideAction::Discard(o.key)));
    }
    for (p, pushed) in pushed {
        let line = format!("{}: {}", ovr::program_text(p), pushed.state.id());
        match pushed.state.why() {
            Some(why) => ui.text_colored(theme::WARN, format!("{} {line} ({why})", icon::ALERT)),
            None => ui.text_disabled(line),
        }
    }
}

/// One program: its warnings, then its listing and its WGSL, each in
/// a read-only box.
fn program(ui: &Ui, id: &str, title: &str, p: &ProgramCode) {
    let Some(_node) = ui
        .tree_node_config(format!("shader-program-{id}"))
        .label(title)
        .push()
    else {
        return;
    };
    for w in &p.warnings {
        ui.text_colored(theme::WARN, format!("{} {w}", icon::ALERT));
    }
    for (what, text) in [("Listing", &p.listing), ("WGSL", &p.wgsl)] {
        let Some(text) = text else {
            ui.text_disabled(format!("{what}: none"));
            continue;
        };
        ui.text(what);
        let lines = text.lines().count().clamp(2, 16) as f32;
        // A copy each frame: the box is for reading and selecting, and
        // what is typed into it is dropped.
        let mut shown = text.clone();
        ui.input_text_multiline(
            format!("##shader-{id}-{what}"),
            &mut shown,
            [-1.0, lines * ui.text_line_height_with_spacing()],
        )
        .read_only(true)
        .build();
    }
}

/// The template with **Open**, the level of detail, why the tag is
/// read-only, and what did not read.
fn header(ui: &Ui, app: &App, read: &ShaderRead, actions: &mut Vec<Action>) {
    let doc = app.shown_tags();
    ui.text(format!("Template  {}", doc.tag_label(read.template.0)));
    if read.template != TagId::NULL {
        ui.same_line();
        if ui.small_button("Open##shader-template") {
            actions.push(Action::Navigate(NavAction::Go {
                tag: read.template,
                path: None,
                element: None,
            }));
        }
    }
    ui.text_disabled(format!(
        "Level of detail {} of {} (the one a near object draws)",
        read.lod + 1,
        read.lods.max(1)
    ));
    if let Some(why) = &read.read_only {
        ui.text_colored(
            theme::WARN,
            format!("{} {why}. The tab shows it and its preview works.", icon::ALERT),
        );
    }
    for p in &read.problems {
        ui.text_colored(theme::WARN, format!("{} {p}", icon::ALERT));
    }
}

/// The preview picture, redrawn when the document or the shape changes,
/// the shape's **Change...** and every refusal under it.
fn preview(
    ui: &Ui,
    app: &App,
    read: &ShaderRead,
    key: &Key,
    cx: &mut ViewCx<'_, '_>,
) {
    let st = &mut *cx.shader;
    let stale = st
        .preview
        .as_ref()
        .is_none_or(|(k, s, _)| k != key || *s != st.shape)
        || (!st.playing && st.drawn_time != st.time);
    if stale {
        if let Some((_, _, Ok(old))) = st.preview.take() {
            let _ = cx.gpu.renderer.unregister_external_texture(old.id);
        }
        let drawn = render(cx.gpu, app, read, st.shape, st.time);
        st.drawn_time = st.time;
        st.preview = Some((key.clone(), st.shape, drawn));
    }
    ui.text(format!("Preview on {}", shape_words(app, st.shape)));
    ui.same_line();
    if ui.small_button("Change...##shader-shape") {
        st.shape_pick = Some(ShapePick {
            query: String::new(),
            opening: true,
        });
    }
    match st.preview.as_ref().map(|(_, _, p)| p) {
        Some(Ok(p)) => {
            let w = ui.content_region_avail()[0].clamp(64.0, p.width as f32);
            let h = w * p.height as f32 / p.width.max(1) as f32;
            ui.image(p.id.texture_id(), [w, h]);
            let n = p.passes.drawn;
            ui.text_disabled(format!(
                "{n} {} drawn, the game's own material path with no game",
                if n == 1 { "pass" } else { "passes" }
            ));
            for s in p.passes.sentences() {
                ui.text_colored(theme::WARN, format!("{} {s}", icon::ALERT));
            }
        }
        Some(Err(e)) => ui.text_colored(theme::WARN, format!("{} No preview: {e}", icon::ALERT)),
        None => {}
    }
}

/// The preview drawn and put on the GPU for the window.
fn render(
    gpu: &mut Gpu<'_>,
    app: &App,
    read: &ShaderRead,
    shape: Shape,
    time: f64,
) -> Result<Preview, String> {
    let p = app.shader_preview(read.tag, shape, time)?;
    let texture = material::render_material(gpu.device, gpu.queue, &p, PICTURE, BACKGROUND);
    let view = texture.create_view(&Default::default());
    let id = gpu
        .renderer
        .register_external_texture(&view)
        .map_err(|e| format!("{e}"))?;
    Ok(Preview {
        id,
        width: texture.width(),
        height: texture.height(),
        _texture: texture,
        passes: PreviewPasses::of(read, &material::material_draws(&p)),
    })
}

fn shape_words(app: &App, shape: Shape) -> String {
    match shape {
        Shape::Object(o) => app.shown_tags().tag_label(o.0),
        s => s.label().to_lowercase(),
    }
}

/// A row's `Used by` lines, dim under it.
fn used_by(ui: &Ui, s: &Slot) {
    if s.used_by.is_empty() {
        ui.text_disabled("    Used by nothing at this level of detail");
    }
    for u in &s.used_by {
        ui.text_disabled(format!("    Used by {}", u.text()));
    }
}

/// A bitmap's thumbnail, decoded once per map and kept.
fn thumb(
    app: &App,
    gpu: &mut Gpu<'_>,
    pictures: &mut crate::ui::map_list::MapListUi,
    st: &mut ShaderUi,
    id: TagId,
    decode: bool,
) -> Option<dear_imgui_rs::TextureId> {
    if !st.thumbs.contains_key(&id) {
        if !decode {
            return None;
        }
        st.thumbs.insert(id, app.bitmap_picture(id, 0).map(Arc::new));
    }
    let pic = st.thumbs.get(&id)?.as_ref().ok()?.clone();
    pictures.picture(gpu, &format!("shader-thumb-{:x}", id.0), &pic)
}

#[allow(clippy::too_many_arguments)]
fn bitmap_row(
    ui: &Ui,
    app: &App,
    read: &ShaderRead,
    s: &Slot,
    gpu: &mut Gpu<'_>,
    pictures: &mut crate::ui::map_list::MapListUi,
    st: &mut ShaderUi,
    actions: &mut Vec<Action>,
) {
    let SlotValue::Bitmap { bitmap, image } = s.value else {
        return;
    };
    let doc = app.shown_tags();
    let id = format!("shader-bitmap-{}", s.index);
    let _id = ui.push_id(&id);
    if bitmap != TagId::NULL {
        match thumb(app, gpu, pictures, st, bitmap, true) {
            Some(tex) => ui.image(tex, [THUMB, THUMB]),
            None => ui.dummy([THUMB, THUMB]),
        }
        ui.same_line();
    }
    ui.group(|| {
        ui.text(s.label());
        let name = if bitmap == TagId::NULL {
            "No bitmap: the rasterizer's default texture".to_owned()
        } else if image != 0 {
            format!("{}, image {image}", doc.tag_label(bitmap.0))
        } else {
            doc.tag_label(bitmap.0)
        };
        ui.text_disabled(name);
        if bitmap != TagId::NULL {
            if ui.small_button("Open") {
                actions.push(Action::Navigate(NavAction::Go {
                    tag: bitmap,
                    path: None,
                    element: None,
                }));
            }
            ui.same_line();
        }
        let _off = ui.begin_disabled_with_cond(read.read_only.is_some());
        if ui.small_button("Change...") {
            st.pick = Some(Pick {
                slot: s.index,
                query: String::new(),
                opening: true,
            });
        }
    });
    used_by(ui, s);
}

/// A colour slot as the panel's own colour row: the swatch opens the
/// colour dialog on the slot's field.
fn colour_row(ui: &Ui, doc: &TagDoc, read: &ShaderRead, s: &Slot, panel: &mut PanelState) {
    let SlotValue::Colour(c) = s.value else {
        return;
    };
    let id = format!("shader-colour-{}", s.index);
    let _id = ui.push_id(&id);
    let rgba = [
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
        (c >> 24) as f32 / 255.0,
    ];
    let clicked = ui
        .color_button_config("##swatch", rgba)
        .flags(ColorButtonFlags::ALPHA_PREVIEW_HALF | ColorButtonFlags::NO_TOOLTIP)
        .size([THUMB, THUMB * 0.5])
        .build();
    if ui.is_item_hovered() {
        ui.set_item_tooltip(format!("#{c:08x} (argb8). Click to pick."));
    }
    if clicked {
        let path = s.path();
        if let Ok((at, _, f)) = tag_api::resolve(doc, read.tag, &path) {
            panel.colour = colour::Open::new(doc, read.tag, at + f.offset, &f, &s.label());
        }
    }
    ui.same_line();
    ui.text(format!("{}  #{c:08x}", s.label()));
    used_by(ui, s);
}

/// A vector slot: its four numbers, and **Edit in Fields**, which shows
/// the field focused in the Fields tab.
fn vector_row(
    ui: &Ui,
    read: &ShaderRead,
    s: &Slot,
    panel: &mut PanelState,
    actions: &mut Vec<Action>,
) {
    let SlotValue::Vector(v) = s.value else {
        return;
    };
    let id = format!("shader-vector-{}", s.index);
    let _id = ui.push_id(&id);
    ui.text(s.label());
    ui.text(format!("    {} {} {} {}", v[0], v[1], v[2], v[3]));
    ui.same_line();
    if ui.small_button("Edit in Fields") {
        let mut settings = panel.settings.clone();
        settings.tab = FIELDS_TAB.to_owned();
        panel.set_settings(settings);
        actions.push(Action::Navigate(NavAction::Go {
            tag: read.tag,
            path: Some(s.path()),
            element: None,
        }));
    }
    used_by(ui, s);
}

/// The bitmap picker: **No bitmap**, then every bitmap of the map with
/// its thumbnail (decoded as rows scroll into view), narrowed by a
/// search. A pick is `tag.set`'s write at the slot's path, sealed.
fn bitmap_picker(
    ui: &Ui,
    app: &App,
    read: &ShaderRead,
    cx: &mut ViewCx<'_, '_>,
    actions: &mut Vec<Action>,
) {
    let (gpu, pictures, st) = (&mut *cx.gpu, &mut *cx.pictures, &mut *cx.shader);
    let Some(mut pick) = st.pick.take() else {
        return;
    };
    let title = format!("Bitmap {}###shader-bitmap-picker", pick.slot);
    if std::mem::take(&mut pick.opening) {
        ui.open_popup(&title);
    }
    let Some(_modal) = ui
        .begin_modal_popup_config(&title)
        .flags(WindowFlags::ALWAYS_AUTO_RESIZE | WindowFlags::NO_SAVED_SETTINGS)
        .begin()
    else {
        return;
    };
    let doc = app.shown_tags();
    ui.set_next_item_width(PICK_WIDTH);
    ui.input_text("##shader-pick-search", &mut pick.query)
        .hint("Search")
        .build();
    let list = picker::tags(doc, &[BITM], None, &pick.query);
    let current = read
        .bitmaps
        .get(pick.slot)
        .and_then(|s| match s.value {
            SlotValue::Bitmap { bitmap, .. } => Some(bitmap),
            _ => None,
        })
        .unwrap_or(TagId::NULL);
    let mut chosen: Option<Value> = None;
    let height = THUMB + ui.clone_style().item_spacing()[1];
    let mut decoded = 0;
    theme::bounded_list(
        ui,
        "##shader-pick-list",
        PICK_WIDTH,
        height,
        list.len() + 1,
        PICK_ROWS,
        || {
            if ui
                .selectable_config("No bitmap##shader-pick-none")
                .selected(current == TagId::NULL)
                .build()
            {
                chosen = Some(Value::Null);
            }
            for i in ListClipper::new(list.len())
                .items_height(height)
                .begin(ui)
                .iter()
            {
                let t = list[i];
                let decode = decoded < THUMBS_PER_FRAME && !st.thumbs.contains_key(&t.id);
                if decode {
                    decoded += 1;
                }
                match thumb(app, gpu, pictures, st, t.id, decode) {
                    Some(tex) => ui.image(tex, [THUMB, THUMB]),
                    None => ui.dummy([THUMB, THUMB]),
                }
                ui.same_line();
                let label = format!("{}##shader-pick-{i}", t.name);
                if ui
                    .selectable_config(label)
                    .selected(t.id == current)
                    .size([0.0, THUMB])
                    .build()
                {
                    chosen = Some(json!(format!("bitm:{}", t.name)));
                }
            }
        },
    );
    ui.separator();
    let cancel = ui.button("Cancel##shader-pick-cancel");
    if let Some(v) = chosen {
        let path = SlotKind::Bitmap.path(pick.slot);
        match tag_api::set_field(doc, read.tag, &path, &v) {
            Ok(a) => {
                actions.push(Action::Tag(a));
                actions.push(Action::Tag(TagAction::Seal));
            }
            Err(e) => cx.panel.problem = Some(format!("{path}: {e}")),
        }
        ui.close_current_popup();
        return;
    }
    if cancel || ui.is_key_pressed(dear_imgui_rs::Key::Escape) {
        ui.close_current_popup();
        return;
    }
    st.pick = Some(pick);
}

/// The shape picker: the sphere, the flat square, or an object tag of the
/// map (its model in its rest pose), narrowed by a search.
fn shape_picker(ui: &Ui, app: &App, st: &mut ShaderUi) {
    let Some(mut pick) = st.shape_pick.take() else {
        return;
    };
    let title = "Preview on###shader-shape-picker";
    if std::mem::take(&mut pick.opening) {
        ui.open_popup(title);
    }
    let Some(_modal) = ui
        .begin_modal_popup_config(title)
        .flags(WindowFlags::ALWAYS_AUTO_RESIZE | WindowFlags::NO_SAVED_SETTINGS)
        .begin()
    else {
        return;
    };
    let mut chosen = None;
    for s in [Shape::Sphere, Shape::Quad] {
        if ui
            .selectable_config(format!("{}##shape-{}", s.label(), s.id()))
            .selected(st.shape == s)
            .build()
        {
            chosen = Some(s);
        }
    }
    ui.separator();
    ui.text_disabled("Or an object's model:");
    ui.set_next_item_width(PICK_WIDTH);
    ui.input_text("##shader-shape-search", &mut pick.query)
        .hint("Search")
        .build();
    let doc = app.shown_tags();
    let mut groups = vec![OBJE];
    groups.extend(COLLECTIONS);
    let list = picker::tags(doc, &groups, None, &pick.query);
    let height = ui.text_line_height_with_spacing();
    theme::bounded_list(ui, "##shader-shape-list", PICK_WIDTH, height, list.len(), PICK_ROWS, || {
        for i in ListClipper::new(list.len()).items_height(height).begin(ui).iter() {
            let t = list[i];
            let label = format!("{}##shape-o{i}", doc.tag_label(t.id.0));
            if ui
                .selectable_config(label)
                .selected(st.shape == Shape::Object(t.id))
                .build()
            {
                chosen = Some(Shape::Object(t.id));
            }
        }
    });
    ui.separator();
    let cancel = ui.button("Cancel##shader-shape-cancel");
    if let Some(s) = chosen {
        st.shape = s;
        ui.close_current_popup();
        return;
    }
    if cancel || ui.is_key_pressed(dear_imgui_rs::Key::Escape) {
        ui.close_current_popup();
        return;
    }
    st.shape_pick = Some(pick);
}
