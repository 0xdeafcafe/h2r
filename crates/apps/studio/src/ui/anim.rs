//! The Animation window: the `jmad` tag selected in the Tags tool posed
//! on the model that uses it (docs/editor/rich-editors.md, "Model poses",
//! slice M1). A list of the graph's animations, a model picker when
//! several use it, the posed model drawn in the native view, and a scrub
//! bar with play and loop over the animation's frames at the game's 30
//! Hz. Sampling is the app's ([`crate::app::anim`]); the agent API's
//! `anim.list` and `anim.pose` use the same calls.
//!
//! The window is one scrolling body. The picture has a height of its own;
//! it never takes the leftover height. Aim-grid clips (codec 8) are
//! labelled and scrubbed as the cells they are, not played as time.

use dear_imgui_rs::{Condition, Ui};
use dear_imgui_wgpu::ExternalTextureId;
use map::TagId;

use crate::app::anim::{AnimError, AnimInfo};
use crate::app::{App, ToolKind};
use crate::theme::{self, icon};
use crate::viewport::Gpu;

/// The map shown and the tag.
type Key = (Option<String>, TagId);

/// The animation's rate: the graph's own 30 Hz.
const HZ: f32 = 30.0;
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
    /// Where on the timeline, in frames (fractional while playing).
    frame: f32,
    playing: bool,
    looping: bool,
    /// The picture last drawn, and the (animation, model, frame) it shows.
    picture: Option<(Picture, (usize, usize, u32))>,
}

struct Picture {
    texture: Option<wgpu::Texture>,
    id: Option<ExternalTextureId>,
    /// Why there is no picture, or a note on the one drawn.
    note: Option<String>,
}

impl AnimUi {
    /// Moves the timeline on by `dt` seconds of play.
    fn advance(&mut self, dt: f32, frames: usize) {
        if !self.playing {
            return;
        }
        let last = frames.saturating_sub(1) as f32;
        self.frame += dt * HZ;
        if self.frame > last {
            if self.looping {
                self.frame %= last.max(1.0);
            } else {
                self.frame = last;
                self.playing = false;
            }
        }
    }
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
        .size([440.0, 560.0], Condition::FirstUseEver)
        .build(|| {
            ui.text(format!("{} {name}", icon::RUN));
            body(ui, app, gpu, st, id, key);
        });
}

fn body(ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, st: &mut AnimUi, id: TagId, key: Key) {
    if st.info.as_ref().map(|i| &i.0) != Some(&key) {
        st.info = Some((key.clone(), app.anim_info(id)));
        st.pick = (0, 0);
        st.frame = 0.0;
        st.playing = false;
        st.picture = None;
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
            st.frame = 0.0;
            st.playing = false;
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
    if ui.button(if st.playing {
        "Pause##anim-play"
    } else {
        "Play##anim-play"
    }) {
        if !st.playing && st.frame >= last {
            st.frame = 0.0;
        }
        st.playing = !st.playing;
    }
    ui.same_line();
    ui.checkbox("Loop##anim-loop", &mut st.looping);
    ui.same_line();
    ui.set_next_item_width(-70.0);
    if ui.slider_f32("##anim-scrub", &mut st.frame, 0.0, last) {
        st.playing = false;
    }
    ui.same_line();
    ui.text_disabled(format!("{:.1}/{frames}  {:.2}s", st.frame, st.frame / HZ));
    st.advance(ui.io().delta_time(), frames);

    picture(ui, app, gpu, st, id, &info);
}

/// The posed model, drawn offscreen through the viewport's native object
/// renderer and shown as a picture; re-rendered when what it shows
/// changes. A refusal or a model that does not draw is named under it.
fn picture(ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, st: &mut AnimUi, id: TagId, info: &AnimInfo) {
    let model = info.users.get(st.pick.1);
    let at = st.frame.floor().max(0.0) as u32;
    let key = (st.pick.0, st.pick.1, at);
    if st.picture.as_ref().map(|(_, k)| *k) != Some(key) {
        let built = app.anim_pose(
            id,
            st.pick.0,
            st.frame,
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
