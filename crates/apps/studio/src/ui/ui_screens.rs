//! The UI Screens tool's windows: the screens and their states (ours, then
//! the disc's retail ones) in the outliner, the picture with its overlays
//! in the centre, and the page's pad, the checks' findings, the hovered
//! widget's JSON and the export in the inspector. State lives in
//! [`crate::app::UiScreens`]; everything goes back as [`Action`]s.

use dear_imgui_rs::{TextureId, Ui};
use dear_imgui_wgpu::ExternalTextureId;

use boot::ui_kit::animation::{Phase, SAMPLES};
use boot::ui_kit::input::Input;

use crate::app::{Action, App, Pick, UiScreens};
use crate::theme::{self, Fonts, icon};
use crate::viewport::Gpu;

/// Widget rectangles.
const RECT: [f32; 4] = [0.35, 0.8, 1.0, 0.55];
/// The hovered widget.
const HOVER: [f32; 4] = [1.0, 1.0, 1.0, 0.95];
/// Widgets a finding names.
const BAD: [f32; 4] = [1.0, 0.3, 0.25, 1.0];
/// The title-safe area.
const SAFE: [f32; 4] = [0.4, 1.0, 0.45, 0.8];
/// Reserved areas.
const RESERVED: [f32; 4] = [1.0, 0.75, 0.2, 0.8];

/// The pad as the inspector lays it out: rows of (input, label).
const PAD: [&[(Input, &str)]; 3] = [
    &[(Input::Up, "Up"), (Input::Y, "Y"), (Input::Back, "Back")],
    &[
        (Input::Left, "Left"),
        (Input::Right, "Right"),
        (Input::X, "X"),
        (Input::B, "B"),
    ],
    &[
        (Input::Down, "Down"),
        (Input::A, "A"),
        (Input::Start, "Start"),
    ],
];

/// The picture on the GPU.
struct Texture {
    texture: wgpu::Texture,
    id: ExternalTextureId,
    rev: u64,
}

/// The tool's UI-only state.
#[derive(Default)]
pub struct UiScreensUi {
    texture: Option<Texture>,
    /// The widget under the mouse in the picture.
    pub hovered: Option<String>,
    /// Where the last frame showed the picture: top left in points, and
    /// points per UI pixel.
    pub shown: Option<([f32; 2], f32)>,
    /// The finding under the mouse in the inspector: its widgets light up.
    hot: Option<usize>,
    filter: String,
}

/// The outliner: our screens and their states, then the retail screens.
pub fn tree(
    ui: &Ui,
    app: &mut App,
    st: &mut UiScreensUi,
    fonts: &Fonts,
    actions: &mut Vec<Action>,
) {
    let f = ui.push_font(fonts.bold);
    ui.text("Our screens");
    f.pop();
    let pick = app.ui_screens.pick.clone();
    for (screen, states) in UiScreens::tree() {
        let node = ui
            .tree_node_config(format!("ui-{screen}"))
            .label(format!("{}  {screen}", icon::LAYERS))
            .default_open(matches!(&pick, Some(Pick::Native { screen: s, .. }) if s == screen))
            .span_avail_width(true)
            .push();
        let Some(_node) = node else { continue };
        for state in states {
            let this = Pick::Native {
                screen: screen.to_string(),
                state: state.clone(),
            };
            let on = pick.as_ref() == Some(&this);
            if ui
                .selectable_config(format!("{state}##ui-{screen}-{state}"))
                .selected(on)
                .build()
            {
                actions.push(Action::UiPick(this));
            }
        }
    }
    ui.spacing();
    ui.separator();
    let f = ui.push_font(fonts.bold);
    ui.text("Retail screens");
    f.pop();
    let retail = app.ui_screens.retail();
    if retail.is_empty() {
        ui.text_wrapped(match app.ui_screens.disc_error() {
            Some(e) => format!("The disc did not open: {e}"),
            None => "Open a disc image to list the game's own screens.".to_owned(),
        });
        return;
    }
    ui.input_text("##ui-filter", &mut st.filter)
        .hint(format!("{} filter", icon::SEARCH))
        .build();
    let filter = st.filter.to_ascii_lowercase();
    ui.child_window("##ui-retail").build(ui, || {
        for tag in retail {
            if !filter.is_empty() && !tag.to_ascii_lowercase().contains(&filter) {
                continue;
            }
            let short = tag.rsplit('\\').next().unwrap_or(&tag).to_owned();
            let this = Pick::Retail(tag.clone());
            let on = pick.as_ref() == Some(&this);
            if ui
                .selectable_config(format!("{short}##ui-retail-{tag}"))
                .selected(on)
                .build()
            {
                actions.push(Action::UiPick(this));
            }
            ui.set_item_tooltip(&tag);
        }
    });
}

/// The picture: zoom, the overlays' switches, then the screen at 640 by
/// 480 times the zoom, with its rectangles, findings and areas over it.
pub fn preview(ui: &Ui, app: &mut App, st: &mut UiScreensUi, gpu: &mut Gpu<'_>) {
    let s = &mut app.ui_screens;
    ui.set_next_item_width(140.0);
    ui.slider_f32("Zoom", &mut s.zoom, 0.5, 3.0);
    ui.same_line();
    ui.checkbox("Rects", &mut s.show_rects);
    ui.same_line();
    ui.checkbox("Title safe", &mut s.show_safe);
    ui.same_line();
    ui.checkbox("Reserved", &mut s.show_reserved);
    if matches!(s.pick.as_ref(), Some(Pick::Native { .. })) {
        transition_picker(ui, s);
    }
    let (zoom, rects, safe, reserved) = (s.zoom, s.show_rects, s.show_safe, s.show_reserved);
    let hot: Vec<String> = st
        .hot
        .and_then(|i| {
            s.shot()
                .and_then(|sh| sh.findings.get(i))
                .map(|f| f.ids.clone())
        })
        .unwrap_or_default();
    let Some(shot) = s.shot() else {
        ui.text_disabled("Pick a screen and a state on the left.");
        return;
    };
    ui.same_line();
    ui.text_disabled(if shot.from_disc {
        format!("{} from the disc", icon::DISC)
    } else {
        format!("{} synthetic (no disc)", icon::INFO)
    });
    let Some(texture) = upload(st, gpu, shot) else {
        ui.text_disabled("The preview could not create its texture.");
        return;
    };
    let size = [640.0 * zoom, 480.0 * zoom];
    let origin = ui.cursor_screen_pos();
    ui.image(texture, size);
    st.shown = Some((origin, zoom));
    let hovered = ui.is_item_hovered();
    let at = |px: [f32; 2]| [origin[0] + px[0] * zoom, origin[1] + px[1] * zoom];
    let dl = ui.get_window_draw_list();
    // Art past the screen's edge is drawn past it in the game too (or not
    // at all); its rectangle stops at the picture's.
    let _clip = dl.push_clip_rect(origin, [origin[0] + size[0], origin[1] + size[1]], true);
    let boxed = |r: [i32; 4], c: [f32; 4], t: f32| {
        let [x, y, w, h] = r.map(|v| v as f32);
        dl.add_rect(at([x, y]), at([x + w, y + h]), c)
            .thickness(t)
            .build();
    };
    // A rectangle in UI units (Y up, centre origin) as pixels.
    let ui_rect = |top: i16, left: i16, bottom: i16, right: i16| {
        let [t, l, b, r] = [top, left, bottom, right].map(i32::from);
        [320 + l, 240 - t, r - l, t - b]
    };
    if safe {
        let r = shot.rules.safe;
        boxed(ui_rect(r.top, r.left, r.bottom, r.right), SAFE, 1.0);
    }
    if reserved {
        for (_, r) in &shot.rules.reserved {
            boxed(ui_rect(r.top, r.left, r.bottom, r.right), RESERVED, 1.5);
        }
    }
    let bad: Vec<&str> = shot
        .findings
        .iter()
        .flat_map(|f| f.ids.iter().map(String::as_str))
        .collect();
    if let Some(view) = &shot.view {
        for w in view.widgets.iter().filter(|w| w.alpha != 0) {
            let r = w.pixels();
            if hot.contains(&w.id) {
                boxed(r, BAD, 3.0);
            } else if bad.contains(&w.id.as_str()) {
                boxed(r, BAD, 1.5);
            } else if rects {
                boxed(r, RECT, 1.0);
            }
            if st.hovered.as_deref() == Some(w.id.as_str()) {
                boxed(r, HOVER, 2.0);
            }
        }
    }
    st.hovered = None;
    if hovered {
        let m = ui.io().mouse_pos();
        let px = [(m[0] - origin[0]) / zoom, (m[1] - origin[1]) / zoom];
        if let Some(id) = s.widget_at(px) {
            ui.tooltip_text(&id);
            st.hovered = Some(id);
        }
    }
}

fn transition_picker(ui: &Ui, s: &mut UiScreens) {
    let label = |phase| match phase {
        Phase::In(0) => "Before opening",
        Phase::In(_) => "Opening",
        Phase::Rest => "At rest",
        Phase::Out(4) => "Closing",
        Phase::Out(_) => "Closed",
    };
    ui.set_next_item_width(140.0);
    if let Some(_combo) = ui.begin_combo("Transition", label(s.animation())) {
        for phase in SAMPLES {
            if ui
                .selectable_config(label(phase))
                .selected(s.animation() == phase)
                .build()
            {
                s.set_animation(phase);
            }
        }
    }
}

/// The picture uploaded when it changed.
fn upload(
    st: &mut UiScreensUi,
    gpu: &mut Gpu<'_>,
    shot: &crate::app::ui_screens::Shot,
) -> Option<TextureId> {
    let c = &shot.canvas;
    let fresh = st
        .texture
        .as_ref()
        .is_none_or(|t| t.texture.width() != c.width || t.texture.height() != c.height);
    if fresh {
        if let Some(old) = st.texture.take() {
            let _ = gpu.renderer.unregister_external_texture(old.id);
        }
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui screens preview"),
            size: wgpu::Extent3d {
                width: c.width,
                height: c.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Not sRGB: Dear ImGui takes texels as sRGB-encoded already
            // (the viewport's note), and the canvas is.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let id = match gpu.renderer.register_external_texture(&view) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("ui screens texture: {e}");
                return None;
            }
        };
        st.texture = Some(Texture {
            texture,
            id,
            rev: u64::MAX,
        });
    }
    let t = st.texture.as_mut()?;
    if t.rev != shot.rev {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &c.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * c.width),
                rows_per_image: Some(c.height),
            },
            wgpu::Extent3d {
                width: c.width,
                height: c.height,
                depth_or_array_layers: 1,
            },
        );
        t.rev = shot.rev;
    }
    Some(t.id.texture_id())
}

/// The inspector: the page's state and pad, the findings, the hovered
/// widget, the export.
pub fn inspector(
    ui: &Ui,
    app: &mut App,
    st: &mut UiScreensUi,
    fonts: &Fonts,
    dialogs: bool,
    actions: &mut Vec<Action>,
) {
    let heading = |t: &str| {
        let f = ui.push_font(fonts.bold);
        ui.text(t);
        f.pop();
    };
    let s = &mut app.ui_screens;
    let native = matches!(s.pick.as_ref(), Some(Pick::Native { .. }));
    if native {
        heading("Page");
        if let Some(sum) = s.summary() {
            ui.text_wrapped(sum);
        }
        for row in PAD {
            for (i, (input, label)) in row.iter().enumerate() {
                if i > 0 {
                    ui.same_line();
                }
                if ui.button_with_size(format!("{label}##pad-{label}"), [56.0, 0.0]) {
                    actions.push(Action::UiPress(*input));
                }
            }
        }
        match &s.last {
            Some(l) => ui.text_disabled(l),
            None => ui.text_disabled("Press a button to step the page."),
        }
        ui.spacing();
        ui.separator();
    }
    heading("Checks");
    st.hot = None;
    match s.shot() {
        None => ui.text_disabled("Nothing drawn."),
        Some(shot) if shot.view.is_none() => {
            ui.text_wrapped("A retail screen: drawn as the disc has it, not checked.")
        }
        Some(shot) => {
            if shot.findings.is_empty() {
                ui.text_colored(theme::OK, format!("{} every check passes", icon::CHECK));
            }
            for (i, f) in shot.findings.iter().enumerate() {
                ui.text_colored(theme::ERROR, icon::ALERT);
                ui.same_line();
                ui.text_wrapped(&f.text);
                if ui.is_item_hovered() {
                    st.hot = Some(i);
                }
            }
            if !shot.skipped.is_empty() {
                ui.text_disabled(format!(
                    "not checked without the disc's art: {}",
                    shot.skipped.join(", ")
                ));
            }
        }
    }
    ui.spacing();
    ui.separator();
    heading("Widget");
    match st.hovered.clone().and_then(|id| s.widget_json(&id)) {
        Some(json) => ui.text(json),
        None => ui.text_disabled("Hover a widget in the picture."),
    }
    ui.spacing();
    ui.separator();
    heading("Export");
    ui.text_wrapped(
        "The picture, its widgets as JSON and the sheet of every screen, into a folder \
         outside any Git checkout.",
    );
    if ui.button(format!("{} Export...", icon::SAVE))
        && dialogs
        && let Some(dir) = rfd::FileDialog::new()
            .set_title("Export the screen into a folder")
            .pick_folder()
    {
        actions.push(Action::UiExport(dir));
    }
}
