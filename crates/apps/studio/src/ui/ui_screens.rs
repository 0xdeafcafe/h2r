//! The UI Screens tool's windows: the screens and their states (ours, then
//! the disc's retail ones) in the outliner, the picture with its overlays
//! in the centre, and the page's pad, the checks' findings, the hovered
//! widget's JSON and the export in the inspector. State lives in
//! [`crate::app::UiScreens`]; everything goes back as [`Action`]s.

use dear_imgui_rs::{TextureId, Ui};
use dear_imgui_wgpu::ExternalTextureId;

use boot::ui_kit::input::Input;

use crate::app::ui_screens::browse::{self, Browse, Folder, Item, Root};
use crate::app::{Action, App, Pick, UiScreens};
use crate::theme::{self, Fonts, icon};
use crate::viewport::Gpu;

mod edit;
mod transport;

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

mod gallery;

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
    /// The grid of previews.
    gallery: gallery::GalleryUi,
    /// Selecting, dragging and typing on the picture.
    edit: edit::EditUi,
    /// The animation transport.
    transport: transport::Transport,
}

/// What the outliner's folders and rows need while they draw.
struct Cx<'a> {
    pick: Option<Pick>,
    at: Option<Browse>,
    /// A filter is typed: every folder with a match opens.
    open: bool,
    actions: &'a mut Vec<Action>,
}

/// The outliner: one filter over both trees, then our screens and the
/// retail screens, each a root that shows all of it when clicked and a tree
/// of folders that shows all of one when clicked (the arrow folds it).
/// Our screens fold out to their states, retail ones are rows. The trees
/// sit in the outliner itself, so a short window scrolls the outliner
/// rather than squeezing the retail tree into what is left below ours.
pub fn tree(
    ui: &Ui,
    app: &mut App,
    st: &mut UiScreensUi,
    fonts: &Fonts,
    actions: &mut Vec<Action>,
) {
    ui.set_next_item_width(-1.0);
    ui.input_text("##ui-filter", &mut st.filter)
        .hint(format!("{} Filter screens", icon::SEARCH))
        .build();
    let filter = st.filter.to_ascii_lowercase();
    let shown = |name: &str| filter.is_empty() || name.to_ascii_lowercase().contains(&filter);
    let mut cx = Cx {
        pick: app.ui_screens.pick.clone(),
        at: app.ui_screens.gallery.at.clone(),
        open: !filter.is_empty(),
        actions,
    };

    root_node(ui, fonts, &mut cx, Root::Ours, |cx| {
        let ours = browse::ours_tree();
        let view = ours.tree.filtered(&|i| {
            shown(&i.name)
                || ours
                    .states
                    .get(&i.name)
                    .is_some_and(|s| s.iter().any(|s| shown(s)))
        });
        if view.is_empty() {
            ui.text_disabled("No screen of ours matches the filter.");
        } else {
            walk(ui, &view, Root::Ours, cx, ours_row);
        }
    });

    root_node(ui, fonts, &mut cx, Root::Retail, |cx| {
        let Some(retail) = app.ui_screens.retail_tree() else {
            ui.text_wrapped(match app.ui_screens.disc_error() {
                Some(e) => format!("The disc did not open: {e}"),
                None => "Open a disc image to list the game's own screens.".to_owned(),
            });
            return;
        };
        let view = retail.tree.filtered(&|i| shown(&i.name));
        if view.is_empty() {
            ui.text_disabled("No retail screen matches the filter.");
        } else {
            walk(ui, &view, Root::Retail, cx, retail_row);
        }
    });
}

/// A root's row: the arrow folds it, a click on its name shows all of it
/// as a grid. `body` draws what is under it while it is open.
fn root_node(ui: &Ui, fonts: &Fonts, cx: &mut Cx<'_>, root: Root, body: impl FnOnce(&mut Cx<'_>)) {
    let this = Browse::root(root);
    let f = ui.push_font(fonts.bold);
    let node = ui
        .tree_node_config(format!("ui-root-{}", root.name()))
        .label(root.title())
        .default_open(true)
        .open_on_arrow(true)
        .span_avail_width(true)
        .selected(cx.at.as_ref() == Some(&this))
        .push();
    f.pop();
    if ui.is_item_clicked() && !ui.is_item_toggled_open() {
        cx.actions.push(Action::UiBrowse(Some(this)));
    }
    if let Some(_node) = node {
        body(cx);
    }
}

/// `folder`'s folders (each a node that shows all of it when clicked), then
/// its own screens through `row`.
fn walk(ui: &Ui, folder: &Folder, root: Root, cx: &mut Cx<'_>, row: fn(&Ui, &Item, &mut Cx<'_>)) {
    for sub in &folder.folders {
        let this = Browse {
            root,
            folder: sub.id.clone(),
        };
        let holds_pick = match &cx.pick {
            Some(Pick::Native { screen, .. }) => root == Root::Ours && sub.contains(screen),
            Some(Pick::Retail(tag)) => root == Root::Retail && sub.contains(tag),
            None => false,
        };
        let node = ui
            .tree_node_config(format!("ui-folder-{}-{}", root.name(), sub.id))
            .label(format!("{}  {}", icon::FOLDER, sub.label))
            .default_open(cx.open || holds_pick)
            .open_on_arrow(true)
            .span_avail_width(true)
            .selected(cx.at.as_ref() == Some(&this))
            .push();
        if ui.is_item_clicked() && !ui.is_item_toggled_open() {
            cx.actions.push(Action::UiBrowse(Some(this)));
        }
        if let Some(_node) = node {
            walk(ui, sub, root, cx, row);
        }
    }
    for item in &folder.items {
        row(ui, item, cx);
    }
}

/// One of our screens: a node with its states under it.
fn ours_row(ui: &Ui, item: &Item, cx: &mut Cx<'_>) {
    let screen = item.name.as_str();
    let states = browse::ours_tree()
        .states
        .get(screen)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let picked = matches!(&cx.pick, Some(Pick::Native { screen: s, .. }) if s == screen);
    let node = ui
        .tree_node_config(format!("ui-{screen}"))
        .label(format!("{}  {}", icon::LAYERS, item.label))
        .default_open(cx.open || picked)
        .span_avail_width(true)
        .push();
    let Some(_node) = node else { return };
    for state in states {
        let this = Pick::Native {
            screen: screen.to_string(),
            state: state.clone(),
        };
        let on = cx.pick.as_ref() == Some(&this);
        if ui
            .selectable_config(format!("{state}##ui-{screen}-{state}"))
            .selected(on)
            .build()
        {
            cx.actions.push(Action::UiPick(this));
        }
    }
}

/// A retail screen: a row named by its tag's last part, the tag in the
/// tooltip.
fn retail_row(ui: &Ui, item: &Item, cx: &mut Cx<'_>) {
    let this = Pick::Retail(item.name.clone());
    let on = cx.pick.as_ref() == Some(&this);
    if ui
        .selectable_config(format!("{}##ui-retail-{}", item.label, item.name))
        .selected(on)
        .build()
    {
        cx.actions.push(Action::UiPick(this));
    }
    ui.set_item_tooltip(&item.name);
}

/// The picture: zoom, the overlays' switches, then the screen at 640 by
/// 480 times the zoom, with its rectangles, findings and areas over it.
pub fn preview(
    ui: &Ui,
    app: &mut App,
    st: &mut UiScreensUi,
    gpu: &mut Gpu<'_>,
    actions: &mut Vec<Action>,
) {
    // A folder or root shows its grid of previews; a picked screen says
    // where it is and goes back to its folder's grid.
    if let Some(at) = app.ui_screens.gallery.at.clone() {
        gallery::show(ui, app, &mut st.gallery, gpu, at);
        return;
    }
    if gallery::pick_crumbs(ui, app) {
        return;
    }
    let picture = preview_picture(ui, app, st, gpu);
    // The editing draws on its own draw list: the picture's is let go.
    if let Some((origin, zoom, hovered)) = picture {
        let at = |px: [f32; 2]| [origin[0] + px[0] * zoom, origin[1] + px[1] * zoom];
        transport::onion(ui, &mut app.ui_screens, &st.transport, &at);
        edit::canvas(ui, app, &mut st.edit, origin, zoom, hovered, actions);
    }
}

/// The picture and its overlays: where it is (top left, zoom) and whether
/// the mouse is over it, when drawn.
fn preview_picture(
    ui: &Ui,
    app: &mut App,
    st: &mut UiScreensUi,
    gpu: &mut Gpu<'_>,
) -> Option<([f32; 2], f32, bool)> {
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
        transport::bar(ui, s, &mut st.transport);
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
        return None;
    };
    ui.same_line();
    ui.text_disabled(if shot.from_disc {
        format!("{} from the disc", icon::DISC)
    } else {
        format!("{} synthetic (no disc)", icon::INFO)
    });
    let Some(texture) = upload(st, gpu, shot) else {
        ui.text_disabled("The preview could not create its texture.");
        return None;
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
        boxed(
            ui_rect(r.top, r.left, r.bottom, r.right),
            theme::SAFE_FRAME,
            1.0,
        );
    }
    if reserved {
        for (_, r) in &shot.rules.reserved {
            boxed(
                ui_rect(r.top, r.left, r.bottom, r.right),
                theme::RESERVED_AREA,
                1.5,
            );
        }
    }
    let bad: Vec<&str> = shot
        .findings
        .iter()
        .flat_map(|f| f.ids.iter().map(String::as_str))
        .collect();
    if let Some(view) = shot.view.as_ref().or(shot.retail_view.as_ref()) {
        for w in view.widgets.iter().filter(|w| w.alpha != 0) {
            let r = w.pixels();
            if hot.contains(&w.id) {
                boxed(r, theme::WIDGET_ERROR, 3.0);
            } else if bad.contains(&w.id.as_str()) {
                boxed(r, theme::WIDGET_ERROR, 1.5);
            } else if rects {
                boxed(r, theme::SCREEN_WIDGET, 1.0);
            }
            if st.hovered.as_deref() == Some(w.id.as_str()) {
                boxed(r, theme::WIDGET_FOCUS, 2.0);
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
    Some((origin, zoom, hovered))
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
    if s.pick.is_some() {
        edit::selected(ui, app, &mut st.edit, fonts, actions);
        if ui.collapsing_header("Widgets##ui-widgets", dear_imgui_rs::TreeNodeFlags::empty()) {
            edit::widget_list(ui, app, &mut st.edit, actions);
        }
        ui.spacing();
        ui.separator();
    }
    let s = &mut app.ui_screens;
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
