//! The Animation window: the `jmad` tag selected in the Tags tool posed
//! on the model that uses it (docs/editor/rich-editors.md, "Model poses",
//! slice M1), with its timeline (slice AN1 of
//! docs/editor/studio-redesign.md). A list of the graph's animations, a
//! model picker when several use it, the posed model drawn in the native
//! view, a transport (play, step, loop, speed, a scrub bar) over the
//! animation's frames at the game's 30 Hz, and under the picture the
//! timeline: a ruler in frames, one row per node with a key marker for
//! each key of its rotation, translation and scale, the scrub head over
//! them, and the events the animation declares. A click or drag on the
//! ruler or the rows scrubs. Sampling and the key reduction are the app's
//! ([`crate::app::anim`]); the agent API's `anim.list`, `anim.pose` and
//! `anim.read` use the same calls.
//!
//! The window is one scrolling body. The picture has a height of its own;
//! it never takes the leftover height. Aim-grid clips (codec 8) are
//! labelled and scrubbed as the cells they are, not played as time.

use dear_imgui_rs::{Condition, MouseButton, Ui};
use dear_imgui_wgpu::ExternalTextureId;
use map::TagId;

use tags::anim_keys::{Kind, Tolerance};

use crate::app::anim::{AnimError, AnimInfo, AnimRead, HZ, Transport};
use crate::app::{App, ToolKind};
use crate::theme::{self, icon};
use crate::viewport::Gpu;

/// The map shown and the tag.
type Key = (Option<String>, TagId);

/// The picture's size, pixels.
const PICTURE: [u32; 2] = [480, 360];

/// The window's own state: what was read for the key shown, the pick, the
/// scrub position, and the picture last drawn, so a frame does not decode
/// or draw again (and a refusal is not asked for again each frame).
#[derive(Default)]
pub struct AnimUi {
    info: Option<(Key, Result<AnimInfo, AnimError>)>,
    /// Animation and model picked (the model's index among the users).
    pick: (usize, usize),
    /// The scrub head and play state.
    transport: Transport,
    /// The timeline's rows for (the key shown, the animation): the clip
    /// decoded and reduced once, not each frame.
    read: Option<((Key, usize), Result<AnimRead, AnimError>)>,
    /// The picture last drawn, and the (animation, model, frame) it shows.
    picture: Option<(Picture, (usize, usize, u32))>,
}

struct Picture {
    texture: Option<wgpu::Texture>,
    id: Option<ExternalTextureId>,
    /// Why there is no picture, or a note on the one drawn.
    note: Option<String>,
}

/// The window, when the Tags tool has an animation graph selected.
pub fn window(ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, st: &mut AnimUi) {
    if app.tool != ToolKind::Tags {
        return;
    }
    let Some(id) = app.shown_tags().selected else {
        return;
    };
    let Some(entry) = app.shown_tags().tag(id) else {
        return;
    };
    if entry.group.0 != *b"jmad" {
        return;
    }
    let name = entry.name.clone();
    let key: Key = (app.shown_map_name().map(str::to_owned), id);
    ui.window("Animation###animation")
        .size([540.0, 680.0], Condition::FirstUseEver)
        .build(|| {
            ui.text(format!("{} {name}", icon::RUN));
            body(ui, app, gpu, st, id, key);
        });
}

fn body(ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, st: &mut AnimUi, id: TagId, key: Key) {
    if st.info.as_ref().map(|i| &i.0) != Some(&key) {
        st.info = Some((key.clone(), app.anim_info(id)));
        st.pick = (0, 0);
        st.transport.scrub(0.0, 0);
        st.picture = None;
        st.read = None;
    }
    let info = match st.info.as_ref().map(|i| &i.1) {
        Some(Ok(info)) => info.clone(),
        Some(Err(e)) => {
            ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT));
            if ui.button("Try again##anim-info") {
                st.info = None;
            }
            return;
        }
        None => return,
    };
    ui.text_disabled(format!(
        "{} nodes, {} animations",
        info.nodes,
        info.animations.len()
    ));
    ui.separator();

    // The animation picker.
    for a in &info.animations {
        let label = format!(
            "{}  {}  {}  {} frames{}##anim-{}",
            match a.kind {
                "overlay" => icon::PLUS,
                _ => icon::RUN,
            },
            a.name,
            a.kind,
            a.frames,
            if a.aim_grid {
                "  (aim grid: frames are cells, not time)"
            } else {
                ""
            },
            a.index,
        );
        if ui
            .selectable_config(label)
            .selected(st.pick.0 == a.index)
            .build()
        {
            st.pick.0 = a.index;
            st.transport.scrub(0.0, 0);
            st.picture = None;
        }
        if let Some(e) = &a.error {
            ui.indent();
            ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT));
            ui.unindent();
        }
    }
    ui.separator();

    // The model picker, when several use the graph.
    if info.users.len() > 1 {
        for (i, u) in info.users.iter().enumerate() {
            if ui
                .selectable_config(format!("{}  {}##anim-model-{i}", icon::BOX, u.name))
                .selected(st.pick.1 == i)
                .build()
            {
                st.pick.1 = i;
                st.picture = None;
            }
        }
        ui.separator();
    }
    let picked = info.animations.iter().find(|a| a.index == st.pick.0);
    let Some(animation) = picked else {
        return;
    };
    let frames = animation.frames;
    let last = frames.saturating_sub(1) as f32;

    // The transport, above the picture.
    if animation.aim_grid {
        ui.text_disabled("aim grid: scrub the cells");
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
        if let Some(_combo) =
            ui.begin_combo("##anim-speed", format!("{}x", st.transport.speed))
        {
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
    ui.set_next_item_width(-70.0);
    if ui.slider_f32("##anim-scrub", &mut at, 0.0, last) {
        st.transport.scrub(at, frames);
    }
    ui.same_line();
    ui.text_disabled(format!(
        "{:.1}/{frames}  {:.2}s",
        st.transport.frame,
        st.transport.frame / HZ
    ));
    st.transport.advance(ui.io().delta_time(), frames);

    picture(ui, app, gpu, st, id, &info);
    if !animation.aim_grid {
        timeline(ui, app, st, id, &key);
    }
}

/// The width of the node names beside the timeline, pixels.
const LABEL_WIDTH: f32 = 130.0;
/// The height of a node's row and of the ruler, pixels.
const ROW_HEIGHT: f32 = 18.0;
/// A key marker's half-size, pixels.
const MARKER: f32 = 2.5;

/// The colour of a channel's keys: rotation, translation, scale.
fn channel_colour(kind: Kind) -> [f32; 4] {
    match kind {
        Kind::Rotation => theme::ACCENT,
        Kind::Translation => theme::OK,
        Kind::Scale => theme::WARN,
    }
}

/// How many levels below the root `node` is, through the rows' parents.
fn depth(read: &AnimRead, node: usize) -> usize {
    let mut depth = 0;
    let mut at = node;
    while let Some(parent) = read
        .nodes
        .get(at)
        .and_then(|n| usize::try_from(n.parent).ok())
    {
        depth += 1;
        at = parent;
        if depth > 64 {
            break;
        }
    }
    depth
}

/// The ruler's step: the first of 1, 2, 5, 10... frames that leaves
/// labels 40 pixels apart.
fn ruler_step(frames: usize, width: f32) -> usize {
    let per_frame = width / frames.saturating_sub(1).max(1) as f32;
    let mut step = 1;
    for scale in [1, 10, 100, 1000] {
        for m in [1, 2, 5] {
            step = m * scale;
            if step as f32 * per_frame >= 40.0 {
                return step;
            }
        }
    }
    step
}

/// The timeline under the picture: the animation read once into keys
/// (`anim.read`'s own path), a ruler in frames, one row per node with a
/// lane for each of its channels, the scrub head, and the events line.
/// A click or drag on the ruler or the rows puts the head there.
fn timeline(ui: &Ui, app: &mut App, st: &mut AnimUi, id: TagId, key: &Key) {
    let want = (key.clone(), st.pick.0);
    if st.read.as_ref().map(|r| &r.0) != Some(&want) {
        let read = app.anim_read(id, st.pick.0, Tolerance::DEFAULT);
        st.read = Some((want, read));
    }
    ui.separator();
    ui.text(format!("{} Timeline", icon::RUN));
    let read = match st.read.as_ref().map(|r| &r.1) {
        Some(Ok(read)) => read,
        Some(Err(e)) => {
            ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT));
            return;
        }
        None => return,
    };
    let frames = read.frames;
    let last = frames.saturating_sub(1).max(1) as f32;
    ui.same_line();
    ui.text_disabled(format!(
        "{} keys for {} values, within {:.2} degrees",
        read.keys.key_count(),
        read.dense_values,
        read.keys.tolerance.rotation.to_degrees()
    ));
    for note in &read.notes {
        ui.text_disabled(note);
    }

    let rows: Vec<_> = read
        .nodes
        .iter()
        .filter(|n| !n.channels.is_empty())
        .collect();
    let width = ui.content_region_avail()[0].max(LABEL_WIDTH + 100.0);
    let track = width - LABEL_WIDTH - 8.0;
    let height = ROW_HEIGHT * (rows.len() + 2) as f32;
    let origin = ui.cursor_screen_pos();
    let x0 = origin[0] + LABEL_WIDTH;
    let x_of = |frame: f32| x0 + frame / last * track;
    ui.invisible_button("##anim-timeline", [width, height]);
    if ui.is_item_active() && ui.is_mouse_down(MouseButton::Left) {
        let frame = ((ui.io().mouse_pos()[0] - x0) / track * last).round();
        st.transport.scrub(frame, frames);
    }
    let dl = ui.get_window_draw_list();
    dl.add_rect(
        origin,
        [origin[0] + width, origin[1] + height],
        theme::RAISED,
    )
    .filled(true)
    .build();

    // The ruler.
    let step = ruler_step(frames, track);
    for f in (0..frames).step_by(step) {
        let x = x_of(f as f32);
        dl.add_line(
            [x, origin[1] + ROW_HEIGHT * 0.6],
            [x, origin[1] + height],
            theme::RULE,
        )
        .build();
        dl.add_text([x + 2.0, origin[1] + 2.0], theme::DIM, &f.to_string());
    }

    // One row per node: its name, its keys in a lane per channel.
    for (i, n) in rows.iter().enumerate() {
        let top = origin[1] + ROW_HEIGHT * (i + 1) as f32;
        let name = n
            .name
            .clone()
            .unwrap_or_else(|| format!("node {}", n.index));
        let indent = 6.0 + 8.0 * depth(read, n.index).min(6) as f32;
        dl.add_text([origin[0] + indent, top + 1.0], theme::TEXT, &name);
        for c in &n.channels {
            let lane = match c.kind {
                Kind::Rotation => 0.0,
                Kind::Translation => 1.0,
                Kind::Scale => 2.0,
            };
            let y = top + ROW_HEIGHT * (lane + 0.5) / 3.0 + 0.5;
            let mut colour = channel_colour(c.kind);
            if !c.moves {
                colour[3] = 0.45;
            }
            if let (true, Some(a), Some(b)) = (c.moves, c.keys.first(), c.keys.last()) {
                let mut line = colour;
                line[3] = 0.3;
                dl.add_line([x_of(a.frame as f32), y], [x_of(b.frame as f32), y], line)
                    .build();
            }
            for k in &c.keys {
                let x = x_of(k.frame as f32);
                dl.add_rect([x - MARKER, y - MARKER], [x + MARKER, y + MARKER], colour)
                    .filled(true)
                    .build();
            }
        }
    }

    // The events line.
    let top = origin[1] + ROW_HEIGHT * (rows.len() + 1) as f32;
    dl.add_text(
        [origin[0] + 6.0, top + 1.0],
        theme::DIM,
        &format!(
            "events: {} frame, {} sound, {} effect (counted, not decoded)",
            read.events[0], read.events[1], read.events[2]
        ),
    );

    // The scrub head, over everything.
    let x = x_of(st.transport.frame.min(last));
    dl.add_line([x, origin[1]], [x, origin[1] + height], theme::WIDGET_FOCUS)
        .thickness(2.0)
        .build();
    ui.text_colored(channel_colour(Kind::Rotation), "R rotation");
    ui.same_line();
    ui.text_colored(channel_colour(Kind::Translation), "T translation");
    ui.same_line();
    ui.text_colored(channel_colour(Kind::Scale), "S scale");
    ui.same_line();
    ui.text_disabled("(a dim key does not change over the clip)");
}

/// The posed model, drawn offscreen through the viewport's native object
/// renderer and shown as a picture; re-rendered when what it shows
/// changes. A refusal or a model that does not draw is named under it.
fn picture(ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, st: &mut AnimUi, id: TagId, info: &AnimInfo) {
    let model = info.users.get(st.pick.1);
    let at = st.transport.frame.floor().max(0.0) as u32;
    let key = (st.pick.0, st.pick.1, at);
    if st.picture.as_ref().map(|(_, k)| *k) != Some(key) {
        let built = app.anim_pose(
            id,
            st.pick.0,
            st.transport.frame,
            None,
            model.map(|m| m.name.as_str()),
        );
        let picture = match built {
            Ok(answer) => render(gpu, answer.model.as_ref()),
            Err(e) => blank(gpu, e.to_string()),
        };
        st.picture = Some((picture, key));
    }
    let (picture, _) = st.picture.as_ref().expect("built above");
    if let Some(id) = picture.id {
        let width = ui.content_region_avail()[0].max(64.0);
        let height = width * PICTURE[1] as f32 / PICTURE[0] as f32;
        ui.image(id.texture_id(), [width, height]);
    }
    if let Some(note) = &picture.note {
        ui.text_colored(theme::WARN, format!("{} {note}", icon::ALERT));
    }
}

/// The posed model drawn offscreen, read back and uploaded as a picture.
/// What cannot be drawn is named in the picture's note.
fn render(gpu: &mut Gpu<'_>, model: Option<&crate::app::anim::PoseModel>) -> Picture {
    let Some(model) = model else {
        return blank(
            gpu,
            "no model of this map uses this graph: the list and the transforms still answer"
                .to_owned(),
        );
    };
    let (rgba, width, height) =
        match crate::app::anim::draw(&gpu.device, &gpu.queue, model, PICTURE) {
            Ok(r) => r,
            Err(e) => return blank(gpu, format!("no renderer here: {e}")),
        };
    let covered = scenario_edit::native_view::preview::covered(&rgba, crate::app::anim::BACKGROUND);
    let note = (covered == 0)
        .then(|| "the picture is empty: the camera sees none of the model".to_owned());
    upload(gpu, rgba, [width, height], note)
}

/// A one-pixel picture to hold the window's shape when there is nothing
/// to draw; the note under it says why.
fn blank(gpu: &mut Gpu<'_>, note: String) -> Picture {
    let bg: [u8; 4] = std::array::from_fn(|k| {
        if k < 3 {
            (crate::app::anim::BACKGROUND[k].clamp(0.0, 1.0) * 255.0) as u8
        } else {
            255
        }
    });
    upload(gpu, bg.to_vec(), [1, 1], Some(note))
}

/// Bytes of one picture, uploaded as a texture and registered with the
/// renderer. The texture is kept: dropping it would destroy what the
/// registration points at.
fn upload(gpu: &mut Gpu<'_>, rgba: Vec<u8>, size: [u32; 2], note: Option<String>) -> Picture {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("animation picture"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let id = gpu.renderer.register_external_texture(&view).ok();
    let picture = Picture {
        texture: id.is_some().then_some(texture),
        id,
        note,
    };
    if let Some(texture) = &picture.texture {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * size[0]),
                rows_per_image: Some(size[1]),
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
    }
    picture
}
