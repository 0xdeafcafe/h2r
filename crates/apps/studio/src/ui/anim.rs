//! The Animation window shares the app's documents and pose pipeline with
//! `anim.*`. The timeline, Curves tab, and viewport key the same channels.

use anim_edit::{AnimDoc, Gesture};
use dear_imgui_rs::{Condition, MouseButton, Ui};
use dear_imgui_wgpu::ExternalTextureId;
use dear_imguizmo::{GuizmoExt, Mode, Operation};
use map::TagId;
use scenario_edit::geometry::{self, Camera, Mat4};
use tags::anim_keys::{Kind, Tolerance};
use tags::anim_sample::hamilton;

use crate::app::anim::{
    AnimError, AnimInfo, AnimRead, GraphKey, HZ, PoseAnswer, PoseModel, PoseNode, Transport,
};
use crate::app::{App, ToolKind};
use crate::theme::{self, icon};
use crate::viewport::Gpu;

mod edit;
mod save;

const PICTURE: [u32; 2] = [480, 360];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Clip {
    Retail(usize),
    Draft(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PictureKey {
    clip: Clip,
    model: usize,
    frame: u32,
    revision: u64,
    base: Option<(usize, Option<u64>)>,
}

struct Preview {
    id: ExternalTextureId,
    _texture: wgpu::Texture,
}

impl Preview {
    fn render(gpu: &mut Gpu<'_>, model: &PoseModel) -> Option<Self> {
        let texture = crate::app::anim::texture(&gpu.device, &gpu.queue, model, PICTURE);
        let view = texture.create_view(&Default::default());
        let id = gpu.renderer.register_external_texture(&view).ok()?;
        Some(Self {
            id,
            _texture: texture,
        })
    }
}

struct Picture {
    key: PictureKey,
    main: Option<Preview>,
    base: Option<Preview>,
    pose: Result<PoseAnswer, AnimError>,
}

struct Gizmo {
    gesture: Gesture,
    camera: Camera,
}

pub struct AnimUi {
    info: Option<(GraphKey, Result<AnimInfo, AnimError>)>,
    clip: Clip,
    model: usize,
    base: Option<usize>,
    transport: Transport,
    read: Option<(usize, Result<AnimRead, AnimError>)>,
    picture: Option<Picture>,
    edit: edit::State,
    save: save::State,
    gizmo: Option<Gizmo>,
    name: String,
    new_frames: i32,
    problem: Option<String>,
}

impl Default for AnimUi {
    fn default() -> Self {
        Self {
            info: None,
            clip: Clip::Retail(0),
            model: 0,
            base: None,
            transport: Transport::default(),
            read: None,
            picture: None,
            edit: edit::State::default(),
            save: save::State::default(),
            gizmo: None,
            name: String::new(),
            new_frames: 30,
            problem: None,
        }
    }
}

impl AnimUi {
    fn pick(&mut self, clip: Clip, overlay: bool, info: &AnimInfo) {
        self.clip = clip;
        self.base = overlay
            .then(|| {
                info.animations
                    .iter()
                    .find(|a| a.kind == "base" && !a.aim_grid && a.error.is_none())
            })
            .flatten()
            .map(|a| a.index);
        self.transport.scrub(0.0, 0);
        self.edit.reset_selection();
        self.gizmo = None;
        self.problem = None;
    }

    fn clear_picture(&mut self, gpu: &mut Gpu<'_>) {
        if let Some(picture) = self.picture.take() {
            for preview in picture.main.into_iter().chain(picture.base) {
                let _ = gpu.renderer.unregister_external_texture(preview.id);
            }
        }
    }
}

/// The window, when the Tags tool has an animation graph selected.
pub fn window(ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, st: &mut AnimUi) {
    let graph = (app.tool == ToolKind::Tags)
        .then(|| app.shown_tags().selected)
        .flatten()
        .and_then(|id| {
            let entry = app.shown_tags().tag(id)?;
            (entry.group.0 == *b"jmad").then(|| (id, entry.name.clone()))
        });
    let Some((id, name)) = graph else {
        st.clear_picture(gpu);
        st.gizmo = None;
        st.edit.end_pointer_edit();
        return;
    };
    ui.window("Animation###animation")
        .size([560.0, 730.0], Condition::FirstUseEver)
        .build(|| {
            ui.text(format!("{} {name}", icon::RUN));
            body(ui, app, gpu, st, id);
        });
    if !ui.is_mouse_down(MouseButton::Left) {
        st.gizmo = None;
        st.edit.end_pointer_edit();
    }
}

fn body(ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, st: &mut AnimUi, id: TagId) {
    let key = app.anim_graph_key(id);
    if st.info.as_ref().map(|i| &i.0) != Some(&key) {
        st.clear_picture(gpu);
        let info = app.anim_info(id);
        if let Ok(info) = &info {
            let overlay = info.animations.first().is_some_and(|a| a.kind == "overlay");
            st.pick(Clip::Retail(0), overlay, info);
        }
        st.info = Some((key, info));
        st.model = 0;
        st.read = None;
    }
    let info = match st.info.as_ref().map(|i| &i.1) {
        Some(Ok(info)) => info.clone(),
        Some(Err(e)) => {
            ui.text_colored(theme::WARN, e.to_string());
            if ui.button("Try again##anim-info") {
                st.info = None;
            }
            return;
        }
        None => return,
    };
    picker(ui, app, st, id, &info);
    let (doc, source) = match st.clip {
        Clip::Draft(doc) => (Some(doc), None),
        Clip::Retail(index) => {
            let Some(animation) = info.animations.get(index) else {
                return;
            };
            if !animation.aim_grid && st.read.as_ref().map(|r| r.0) != Some(index) {
                st.read = Some((index, app.anim_read(id, index, Tolerance::DEFAULT)));
            }
            let read = st.read.as_ref().filter(|r| r.0 == index).map(|r| &r.1);
            let doc = match read {
                Some(Ok(read)) => match app.anim_open_read(id, read) {
                    Ok(doc) => Some(doc),
                    Err(e) => {
                        ui.text_colored(theme::WARN, e.to_string());
                        None
                    }
                },
                Some(Err(e)) => {
                    ui.text_colored(theme::WARN, e.to_string());
                    None
                }
                _ => None,
            };
            (doc, Some(animation))
        }
    };
    if let Some(animation) = source
        .cloned()
        .or_else(|| doc.and_then(|d| app.anim_doc_info(d).ok().cloned()))
    {
        if animation.kind == "overlay" {
            overlay_picker(ui, st, &info);
        }
        transport(ui, st, animation.frames, animation.aim_grid);
        if let Some(doc) = doc {
            if let Ok(clip) = app.anim_doc_mut(doc) {
                st.edit.controls(ui, clip);
            }
            st.save.draw(ui, app, doc, source.is_some());
        }
        picture(ui, app, gpu, st, id, &info, doc);
        if let Some(doc) = doc
            && let Ok(clip) = app.anim_doc_mut(doc)
        {
            let reduction = source.and_then(|animation| {
                st.read
                    .as_ref()
                    .filter(|r| r.0 == animation.index)?
                    .1
                    .as_ref()
                    .ok()
            });
            st.edit.draw(ui, clip, &mut st.transport, reduction);
        }
    }
}

fn picker(ui: &Ui, app: &mut App, st: &mut AnimUi, tag: TagId, info: &AnimInfo) {
    let drafts = app.anim_docs(tag);
    let title = match st.clip {
        Clip::Retail(index) => info.animations.get(index).map(|row| row.name.as_str()),
        Clip::Draft(id) => app.anim_doc(id).ok().map(AnimDoc::name),
    };
    if let Some(_combo) = ui.begin_combo("Clip##anim-clip", title.unwrap_or("Choose a clip")) {
        for row in &info.animations {
            let clip = Clip::Retail(row.index);
            let grid = if row.aim_grid { " · aim grid" } else { "" };
            if ui
                .selectable_config(format!(
                    "{} · {} · {} frames{grid}##retail-{}",
                    row.name, row.kind, row.frames, row.index
                ))
                .selected(st.clip == clip)
                .build()
            {
                st.pick(clip, row.kind == "overlay", info);
            }
            if let Some(e) = &row.error {
                ui.text_colored(theme::WARN, e);
            }
        }
        for id in drafts {
            if let Ok(row) = app.anim_doc_info(id) {
                let clip = Clip::Draft(id);
                if ui
                    .selectable_config(format!(
                        "{} · {} · {} frames##draft-{id}",
                        row.name, row.kind, row.frames
                    ))
                    .selected(st.clip == clip)
                    .build()
                {
                    st.pick(clip, row.kind == "overlay", info);
                }
            }
        }
    }
    if info.users.len() > 1 {
        let name = info.users.get(st.model).map_or("", |m| m.name.as_str());
        if let Some(_combo) = ui.begin_combo("Model##anim-model", name) {
            for (index, model) in info.users.iter().enumerate() {
                if ui
                    .selectable_config(format!("{}##model-{index}", model.name))
                    .selected(st.model == index)
                    .build()
                {
                    st.model = index;
                    st.gizmo = None;
                }
            }
        }
    }
    ui.set_next_item_width(160.0);
    ui.input_text("Name##anim-draft", &mut st.name).build();
    ui.same_line();
    ui.set_next_item_width(90.0);
    ui.input_int("Frames##anim-new", &mut st.new_frames);

    let mut made = None;
    if ui.button("New animation") {
        made = Some(app.anim_new(tag, &st.name, st.new_frames.max(0) as usize));
    }
    ui.same_line();
    let duplicate_enabled = match st.clip {
        Clip::Draft(_) => true,
        Clip::Retail(index) => info
            .animations
            .get(index)
            .is_some_and(|a| !a.aim_grid && a.error.is_none()),
    };
    let disabled = ui.begin_disabled_with_cond(!duplicate_enabled);
    if ui.button("Duplicate") {
        made = Some(match st.clip {
            Clip::Draft(id) => app.anim_duplicate_doc(id, &st.name),
            Clip::Retail(index) => app
                .anim_open(tag, index)
                .and_then(|id| app.anim_duplicate_doc(id, &st.name)),
        });
    }
    drop(disabled);
    ui.same_line();
    if ui.button("Retarget from...##anim-retarget") {
        ui.open_popup("##anim-retarget-from");
    }
    if let Some(_popup) = ui.begin_popup("##anim-retarget-from") {
        made = retarget_picker(ui, app, st, tag, info).or(made);
    }
    match made {
        Some(Ok(id)) => {
            let overlay = app.anim_doc_info(id).is_ok_and(|row| row.kind == "overlay");
            st.pick(Clip::Draft(id), overlay, info);
            st.name.clear();
        }
        Some(Err(e)) => st.problem = Some(e.to_string()),
        None => {}
    }
    if let Some(problem) = &st.problem {
        ui.text_colored(theme::WARN, problem);
    }
}

/// The sources a retarget onto this graph can start from: every base
/// document of the session (any map) and this graph's retail clips.
/// Picking one makes a document on this graph's skeleton.
fn retarget_picker(
    ui: &Ui,
    app: &mut App,
    st: &mut AnimUi,
    tag: TagId,
    info: &AnimInfo,
) -> Option<Result<u64, AnimError>> {
    let mut source = None;
    for (id, name, kind, map) in app.anim_all_docs() {
        if kind == "overlay" {
            continue;
        }
        let from = map.map_or(String::new(), |m| format!(" · {m}"));
        if ui
            .selectable(format!("{name}{from}##retarget-draft-{id}"))
        {
            source = Some((Ok(id), name));
        }
    }
    ui.separator();
    for row in &info.animations {
        if row.kind == "base"
            && row.error.is_none()
            && !row.aim_grid
            && ui.selectable(format!("{}##retarget-retail-{}", row.name, row.index))
        {
            source = Some((app.anim_open(tag, row.index), row.name.clone()));
        }
    }
    let (opened, from_name) = source?;
    let name = if st.name.trim().is_empty() {
        format!("{from_name} retargeted")
    } else {
        st.name.clone()
    };
    let to_model = (info.users.len() > 1)
        .then(|| info.users.get(st.model).map(|m| m.name.clone()))
        .flatten();
    let made = opened.and_then(|doc| {
        app.anim_retarget(doc, tag, &name, &[], [None, to_model.as_deref()], None)
            .map(|(id, _)| id)
    });
    ui.close_current_popup();
    Some(made)
}

fn overlay_picker(ui: &Ui, st: &mut AnimUi, info: &AnimInfo) {
    let label = st
        .base
        .and_then(|index| info.animations.get(index))
        .map_or("Rest pose", |row| row.name.as_str());
    if let Some(_combo) = ui.begin_combo("Base##anim-overlay-base", label) {
        if ui
            .selectable_config("Rest pose##overlay-rest")
            .selected(st.base.is_none())
            .build()
        {
            st.base = None;
        }
        for row in &info.animations {
            if row.kind == "base"
                && row.error.is_none()
                && !row.aim_grid
                && ui
                    .selectable_config(format!("{}##overlay-base-{}", row.name, row.index))
                    .selected(st.base == Some(row.index))
                    .build()
            {
                st.base = Some(row.index);
            }
        }
    }
}

fn transport(ui: &Ui, st: &mut AnimUi, frames: usize, aim_grid: bool) {
    if aim_grid {
        ui.text_disabled("Aim grid · scrub the cells");
    } else {
        if ui.button(if st.transport.playing {
            "Pause##anim-play"
        } else {
            "Play##anim-play"
        }) {
            st.transport.toggle(frames);
        }
        ui.same_line();
        if ui.button("<##anim-step-back") {
            st.transport.step(-1, frames);
        }
        ui.same_line();
        if ui.button(">##anim-step-on") {
            st.transport.step(1, frames);
        }
        ui.same_line();
        ui.checkbox("Loop##anim-loop", &mut st.transport.looping);
        ui.same_line();
        ui.set_next_item_width(70.0);
        if let Some(_combo) = ui.begin_combo("##anim-speed", format!("{}x", st.transport.speed)) {
            for speed in Transport::SPEEDS {
                if ui
                    .selectable_config(format!("{speed}x##anim-speed-{speed}"))
                    .selected(st.transport.speed == speed)
                    .build()
                {
                    st.transport.speed = speed;
                }
            }
        }
    }
    let mut at = st.transport.frame;
    ui.set_next_item_width(-90.0);
    if ui.slider_f32(
        "##anim-scrub",
        &mut at,
        0.0,
        frames.saturating_sub(1) as f32,
    ) {
        st.transport.scrub(at.round(), frames);
    }
    ui.same_line();
    ui.text_disabled(format!(
        "{} / {} · {:.2}s",
        st.transport.frame.round() as usize,
        frames.saturating_sub(1),
        st.transport.frame / HZ
    ));
    st.transport.advance(ui.io().delta_time(), frames);
}

fn picture(
    ui: &Ui,
    app: &mut App,
    gpu: &mut Gpu<'_>,
    st: &mut AnimUi,
    tag: TagId,
    info: &AnimInfo,
    doc: Option<u64>,
) {
    let model = info.users.get(st.model).map(|m| m.name.as_str());
    let revision = doc
        .and_then(|id| app.anim_doc(id).ok())
        .map_or(0, AnimDoc::revision);
    let base = st.base.map(|index| {
        let revision = app
            .anim_opened(tag, index)
            .and_then(|id| app.anim_doc(id).ok())
            .map(AnimDoc::revision);
        (index, revision)
    });
    let at = st.transport.frame.round() as u32;
    let key = PictureKey {
        clip: st.clip,
        model: st.model,
        frame: at,
        revision,
        base,
    };
    if st.picture.as_ref().map(|p| p.key) != Some(key) {
        let mut pose = match (doc, st.base, st.clip) {
            (Some(doc), Some(base), _) => app.anim_doc_pose_over(doc, at as f32, base, model),
            (Some(doc), None, _) => app.anim_doc_pose(doc, at as f32, None, model),
            (None, _, Clip::Retail(index)) => app.anim_pose(tag, index, at as f32, None, model),
            (None, _, Clip::Draft(_)) => return,
        };
        let mut main = None;
        let mut base = None;
        if let Ok(answer) = &mut pose
            && let Some(posed) = &mut answer.model
        {
            if let Some(drag) = &st.gizmo {
                posed.camera = drag.camera;
            }
            main = Preview::render(gpu, posed);
            if let Some(index) = st.base {
                let comparison = match app.anim_opened(tag, index) {
                    Some(base) => app.anim_doc_pose(base, at as f32, None, model),
                    None => app.anim_pose(tag, index, at as f32, None, model),
                };
                if let Ok(answer) = comparison
                    && let Some(mut reference) = answer.model
                {
                    reference.camera = posed.camera;
                    base = Preview::render(gpu, &reference);
                }
            }
        }
        st.clear_picture(gpu);
        st.picture = Some(Picture {
            key,
            main,
            base,
            pose,
        });
    }
    let Some(picture) = &st.picture else {
        return;
    };
    let answer = match &picture.pose {
        Ok(answer) => answer,
        Err(e) => {
            ui.text_colored(theme::WARN, e.to_string());
            return;
        }
    };
    if let (Some(main), Some(model)) = (&picture.main, &answer.model) {
        let mut width = ui.content_region_avail()[0].max(64.0);
        if picture.base.is_some() {
            width = (width - 12.0) * 0.5;
        }
        let size = [width, width * PICTURE[1] as f32 / PICTURE[0] as f32];
        let base_origin = ui.cursor_screen_pos();
        if let Some(base) = &picture.base {
            ui.image(base.id.texture_id(), size);
            ui.same_line();
        }
        let origin = ui.cursor_screen_pos();
        ui.image(main.id.texture_id(), size);
        if picture.base.is_some() {
            let dl = ui.get_window_draw_list();
            for (at, label) in [(base_origin, "Base"), (origin, "Overlay")] {
                dl.add_text([at[0] + 6.0, at[1] + 6.0], theme::TEXT, label);
            }
        }
        if let Some(doc) = doc
            && let Ok(doc) = app.anim_doc_mut(doc)
        {
            gizmo(
                ui,
                &mut st.edit,
                &mut st.gizmo,
                &mut st.transport,
                doc,
                answer,
                model.camera,
                origin,
                size,
            );
        }
    } else {
        for note in &answer.notes {
            ui.text_disabled(note);
        }
        if answer.model.is_some() {
            ui.text_colored(theme::WARN, "Could not display the model texture");
        }
    }
}

fn gizmo(
    ui: &Ui,
    edit: &mut edit::State,
    drag: &mut Option<Gizmo>,
    transport: &mut Transport,
    doc: &mut AnimDoc,
    answer: &PoseAnswer,
    camera: Camera,
    origin: [f32; 2],
    size: [f32; 2],
) {
    let aspect = size[0] / size[1];
    let hovered = ui.is_item_hovered();
    let clicked = ui.is_mouse_clicked(MouseButton::Left);
    let mouse = ui.io().mouse_pos();
    let positions = answer
        .nodes
        .iter()
        .map(|node| {
            let [x, y, z, _] = node.world?[3];
            let [u, v] = camera.project([x, y, z], aspect)?;
            (u >= 0.0 && u <= 1.0 && v >= 0.0 && v <= 1.0)
                .then_some([origin[0] + size[0] * u, origin[1] + size[1] * v])
        })
        .collect::<Vec<_>>();
    let dl = ui.get_window_draw_list();
    for (i, point) in positions.iter().enumerate() {
        let Some([x, y]) = *point else { continue };
        if let Ok(parent) = usize::try_from(answer.nodes[i].parent)
            && let Some(Some(p)) = positions.get(parent)
        {
            dl.add_line(*p, [x, y], theme::ACCENT).build();
        }
        let colour = if edit.node == i {
            theme::WIDGET_FOCUS
        } else {
            theme::DIM
        };
        dl.add_rect([x - 2.0, y - 2.0], [x + 2.0, y + 2.0], colour)
            .filled(true)
            .build();
    }
    drop(dl);

    let mut over = false;
    if let Some(node) = answer.nodes.get(edit.node)
        && let Some(world) = node.world
    {
        let g = ui.guizmo();
        g.set_drawlist_window();
        g.set_rect(origin[0], origin[1], size[0], size[1]);
        g.set_orthographic(false);
        let view = geometry::flat(&camera.view());
        let projection = geometry::flat(&camera.proj(aspect));
        let op = match edit.channel {
            Kind::Translation => Operation::TRANSLATE,
            Kind::Rotation => Operation::ROTATE,
            Kind::Scale => Operation::SCALE_UNIFORM,
        };
        let mut matrix = geometry::flat(&world);
        let changed = g
            .manipulate_config(&view, &projection, &mut matrix)
            .operation(op)
            .mode(Mode::Local)
            .build();
        over = g.is_over() || g.is_using();
        if (changed || g.is_using()) && drag.is_none() {
            transport.scrub(answer.frame, doc.frames());
            *drag = Some(Gizmo {
                gesture: doc.begin_gesture(),
                camera,
            });
        }
        if changed {
            let parent = usize::try_from(node.parent)
                .ok()
                .and_then(|p| answer.nodes.get(p))
                .and_then(|n| n.world);
            let result = parent_local(matrix, parent).and_then(|local| {
                key_gizmo(
                    doc,
                    edit.node,
                    edit.channel,
                    answer.frame as usize,
                    local,
                    answer.base_nodes.as_ref().and_then(|n| n.get(edit.node)),
                    drag.as_ref().map(|drag| drag.gesture),
                )
            });
            edit.record_result(result);
        }
        if !g.is_using() {
            *drag = None;
        }
    }
    if hovered && clicked && !over {
        let nearest = positions
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let p = (*p)?;
                let d = (mouse[0] - p[0]).powi(2) + (mouse[1] - p[1]).powi(2);
                (d <= 64.0).then_some((i, d))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = nearest {
            edit.select_track(i, edit.channel);
        }
    }
}

/// The world matrix has only rigid transforms and uniform scale. Invert the
/// parent basis directly so a gizmo edits the child in its own local space.
fn parent_local(world: [f32; 16], parent: Option<Mat4>) -> Result<[f32; 16], String> {
    let Some(parent) = parent else {
        return Ok(world);
    };
    let xyz = |c: [f32; 4]| [c[0], c[1], c[2]];
    let [x, y, z, pos] = parent.map(xyz);
    let scale2 = geometry::dot(x, x);
    if !(scale2 > 0.0 && scale2.is_finite()) {
        return Err("The parent has a degenerate scale".to_owned());
    }
    let mut local = world;
    for c in 0..4 {
        let start = 4 * c;
        let mut v = [world[start], world[start + 1], world[start + 2]];
        if c == 3 {
            v = geometry::sub(v, pos);
        }
        for (r, axis) in [x, y, z].into_iter().enumerate() {
            local[start + r] = geometry::dot(axis, v) / scale2;
        }
    }
    Ok(local)
}

/// Convert an orthonormal basis using its largest quaternion component,
/// which remains well-conditioned even for a half-turn or at Euler lock.
fn matrix_rotation([x, y, z]: [[f32; 3]; 3], scale: f32) -> [f32; 4] {
    let [[xx, xy, xz], [yx, yy, yz], [zx, zy, zz]] = [x, y, z].map(|c| c.map(|v| v / scale));
    let candidates = [
        1.0 + xx - yy - zz,
        1.0 - xx + yy - zz,
        1.0 - xx - yy + zz,
        1.0 + xx + yy + zz,
    ];
    let i = (0..4)
        .max_by(|&a, &b| candidates[a].total_cmp(&candidates[b]))
        .expect("four components");
    let largest = candidates[i].sqrt() * 0.5;
    let d = 4.0 * largest;
    match i {
        0 => [largest, (xy + yx) / d, (xz + zx) / d, (yz - zy) / d],
        1 => [(xy + yx) / d, largest, (yz + zy) / d, (zx - xz) / d],
        2 => [(xz + zx) / d, (yz + zy) / d, largest, (xy - yx) / d],
        _ => [(yz - zy) / d, (zx - xz) / d, (xy - yx) / d, largest],
    }
}

/// Store the same local channel as an RPC key. An overlay key is relative
/// to the base's local channel at this frame.
pub(crate) fn key_gizmo(
    doc: &mut AnimDoc,
    node: usize,
    kind: Kind,
    frame: usize,
    local: [f32; 16],
    base: Option<&PoseNode>,
    gesture: Option<Gesture>,
) -> Result<bool, String> {
    let value = match kind {
        Kind::Translation => {
            let delta = base.map_or([0.0; 3], |base| base.translation);
            [
                local[12] - delta[0],
                local[13] - delta[1],
                local[14] - delta[2],
                0.0,
            ]
        }
        Kind::Rotation | Kind::Scale => {
            let x = [local[0], local[1], local[2]];
            let y = [local[4], local[5], local[6]];
            let z = [local[8], local[9], local[10]];
            let sign = geometry::dot(x, geometry::cross(y, z)).signum();
            let scale = geometry::dot(x, x).sqrt() * sign;
            if !scale.is_finite() || scale == 0.0 {
                return Err("The node has a degenerate scale".to_owned());
            }
            if kind == Kind::Scale {
                [scale / base.map_or(1.0, |base| base.scale), 0.0, 0.0, 0.0]
            } else {
                let q = matrix_rotation([x, y, z], scale);
                match base {
                    Some(base) => {
                        let [x, y, z, w] = base.rotation;
                        hamilton([-x, -y, -z, w], q)
                    }
                    None => q,
                }
            }
        }
    };
    doc.set_key_during_gesture(node, kind, frame, value, None, gesture)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anim_edit::{Node, angles_to_rotation};
    use tags::anim_keys::distance;
    use tags::anim_sample::NodeTransform;

    #[test]
    fn gizmo_rotation_is_local_and_relative_to_the_overlay_base() {
        let basis = angles_to_rotation([10.0, 31.0, -15.0]);
        let delta = angles_to_rotation([20.0, 90.0, -18.0]);
        let local = NodeTransform {
            rotation: hamilton(basis, delta),
            translation: [1.0, 0.0, 0.0],
            scale: -2.0,
            ..NodeTransform::default()
        }
        .matrix();
        let parent = NodeTransform {
            rotation: angles_to_rotation([60.0, 2.0, 20.0]),
            translation: [1.0, 2.0, 3.0],
            scale: 0.66,
            ..NodeTransform::default()
        }
        .matrix();
        let world = geometry::flat(&geometry::mul(&parent, &local));
        let local = parent_local(world, Some(parent)).unwrap();
        let mut clip = AnimDoc::new(
            "invented".to_owned(),
            1,
            vec![Node {
                name: None,
                parent: -1,
            }],
        )
        .unwrap();
        let base = PoseNode {
            name: None,
            parent: -1,
            rotation: basis,
            translation: [0.0; 3],
            scale: 1.0,
            has: [true; 3],
            world: None,
        };
        key_gizmo(&mut clip, 0, Kind::Rotation, 0, local, Some(&base), None).unwrap();
        let stored = clip.key(0, Kind::Rotation, 0).unwrap().value;
        assert!(distance(Kind::Rotation, stored, delta) < 0.00001);
    }
}
