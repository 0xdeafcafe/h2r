//! The HUD tool's windows: the map, its HUDs and the HUD's widgets in the
//! outliner; the HUD drawn at 640 by 480 with the HUD frame, the widgets'
//! rectangles and the state it is drawn at in the centre; the selected
//! widget's fields, the checks, the HUDs drawn with it, the placement rules
//! and the save in the inspector. State lives in [`crate::app::HudTool`]
//! and [`hud_edit::HudSession`]; everything goes back as [`Action`]s.
//!
//! The selected widget's fields ([`properties`]) are the studio's generic
//! field panel ([`crate::fields::panel`]) over the widget's element, named
//! by the `nhdt` definition (`tags/src/defs/nhdt.txt`); its writes come
//! back as [`Action::HudWrite`], so the HUD tool keeps one history and one
//! save. Fields the panel cannot edit (bytes the schema types as a raw
//! pointer or size: the text justification) keep a row from
//! `hud_edit::layout`'s field tables.

use std::path::PathBuf;

use dear_imgui_rs::{MouseButton, TextureId, TreeNodeFlags, Ui};
use dear_imgui_wgpu::ExternalTextureId;
use editor_core::map_groups::MapGroup;

use hud_edit::checks::Severity;
use hud_edit::layout::{Field, Kind, Mode};
use hud_edit::place::{self, Proof};
use hud_edit::state::{self, Grenade, Source};
use hud_edit::{HudState, Value, WidgetKey, WidgetKind};

use tags::registry::Kind as RegKind;

use crate::app::map_cards::MapEntry;
use crate::app::{Action, App, HudTool, TagAction};
use crate::fields::{self, PanelState};
use crate::theme::{self, Fonts, icon};
use crate::viewport::Gpu;

use super::map_list;

const RECT: [f32; 4] = [0.35, 0.8, 1.0, 0.45];
const SELECTED: [f32; 4] = [1.0, 1.0, 1.0, 0.95];
const BAD: [f32; 4] = [1.0, 0.3, 0.25, 1.0];
const FRAME: [f32; 4] = [0.4, 1.0, 0.45, 0.8];
const HIDDEN: [f32; 4] = [0.6, 0.6, 0.6, 0.5];
const OTHER: [f32; 4] = [1.0, 0.75, 0.2, 0.5];

struct Texture {
    texture: wgpu::Texture,
    id: ExternalTextureId,
    rev: u64,
}

/// A drag in progress: the widget, the mouse and its offset at the start.
#[derive(Clone, Copy)]
struct Drag {
    key: WidgetKey,
    from: [f32; 2],
    offset: (i16, i16),
    moved: bool,
}

/// The tool's UI-only state.
#[derive(Default)]
pub struct HudUi {
    texture: Option<Texture>,
    /// Where the last frame showed the picture: top left in points, and
    /// points per HUD pixel.
    pub shown: Option<([f32; 2], f32)>,
    drag: Option<Drag>,
    /// The generic field panel's state for the selected widget.
    panel: PanelState,
    /// The finding under the mouse: its widgets light up.
    hot: Option<usize>,
    filter: String,
}

fn heading(ui: &Ui, fonts: &Fonts, t: &str) {
    let f = ui.push_font(fonts.bold);
    ui.text(t);
    f.pop();
}

/// The outliner: the map open (a combo of the source's maps and the
/// packages, so the lists below keep the height), the open map's HUDs by
/// who uses them, and the HUD's widgets.
pub fn tree(ui: &Ui, app: &mut App, st: &mut HudUi, fonts: &Fonts, actions: &mut Vec<Action>) {
    let opened = app.hud.opened.clone();
    let shown = opened.clone().unwrap_or_else(|| "pick a map".to_owned());
    ui.set_next_item_width(-1.0);
    if let Some(_combo) = ui.begin_combo("##hud-map-combo", format!("{}  {shown}", icon::MAP)) {
        ui.set_next_item_width(-1.0);
        ui.input_text("##hud-filter", &mut st.filter)
            .hint(format!("{} filter maps", icon::SEARCH))
            .build();
        let filter = st.filter.to_lowercase();
        let keep = |s: &str| filter.is_empty() || s.to_lowercase().contains(&filter);
        if app.maps().is_empty() && app.packages().is_empty() {
            ui.text_disabled("Open a disc, a map or a folder of maps (File > Open).");
        }
        let mut entries = app.halo2_map_entries();
        let shared = MapEntry::take_shared(&mut entries);
        let mut last = None;
        let draw_source = |entries: &[MapEntry<'_>],
                           last: &mut Option<MapGroup>,
                           actions: &mut Vec<Action>| {
            for entry in entries {
                let name = entry.stem;
                let label = entry.label;
                if !keep(name) && !keep(label) {
                    continue;
                }
                map_list::section(ui, last, entry.group);
                let glyph = map_list::map_icon(entry.group);
                if ui
                    .selectable_config(format!("{glyph}  {label}##hud-map-{name}"))
                    .selected(opened.as_deref() == Some(name))
                    .build()
                {
                    actions.push(Action::HudOpenMap(name.to_owned()));
                }
            }
        };
        draw_source(&entries, &mut last, actions);
        for entry in app.packages() {
            let label = entry.label();
            if !keep(&label) {
                continue;
            }
            let group = entry.map_group();
            map_list::section(ui, &mut last, group);
            let key = entry.key();
            let glyph = map_list::map_icon(group);
            if ui
                .selectable_config(format!("{glyph}  {label}##hud-pkg-{key}"))
                .selected(opened.as_deref() == Some(label.as_str()))
                .build()
            {
                actions.push(Action::HudOpenPackage(key));
            }
        }
        draw_source(&shared, &mut last, actions);
    }
    ui.separator();
    let Some(s) = app.hud.session.as_ref() else {
        ui.text_disabled("Pick a map to list its HUDs.");
        return;
    };
    heading(ui, fonts, "HUDs");
    let current = s.current;
    let edited = s.edited();
    let mut last = "";
    // A third of what is left, the rest for the widgets.
    let list = (ui.content_region_avail()[1] / 3.0).clamp(48.0, 220.0);
    ui.child_window("##hud-list").size([0.0, list]).build(ui, || {
        for e in &s.huds {
            if e.category() != last {
                last = e.category();
                ui.text_disabled(last);
            }
            let short = e.name.rsplit('\\').next().unwrap_or(&e.name);
            let mark = if edited.contains(&e.id) { "  *" } else { "" };
            if ui
                .selectable_config(format!("{short}{mark}##hud-{}", e.id.0))
                .selected(current == Some(e.id))
                .build()
            {
                actions.push(Action::HudPick(e.id));
            }
            let owners: Vec<String> =
                e.owners.iter().map(|o| format!("{} ({})", o.name, o.at)).collect();
            ui.set_item_tooltip(format!(
                "{}\n{}",
                e.name,
                if owners.is_empty() {
                    "named by no biped, vehicle, weapon or globals".to_owned()
                } else {
                    format!("named by: {}", owners.join(", "))
                }
            ));
        }
    });
    ui.separator();
    heading(ui, fonts, "Widgets");
    let Some(hud) = s.hud() else { return };
    let selected = app.hud.selected;
    ui.child_window("##hud-widgets").build(ui, || for w in &hud.widgets {
        let kind = match w.key.kind {
            WidgetKind::Bitmap => icon::BOX,
            WidgetKind::Text => icon::FILE,
        };
        let mark = if w.is_dirty() { "  *" } else { "" };
        if ui
            .selectable_config(format!("{kind}  {}{mark}##hud-w-{:?}-{}", w.name, w.key.kind, w.key.index))
            .selected(selected == Some(w.key))
            .build()
        {
            actions.push(Action::HudSelect(Some(w.key)));
        }
    });
}

/// The picture with its overlays, the toolbar above it and the state below.
pub fn preview(ui: &Ui, app: &mut App, st: &mut HudUi, fonts: &Fonts, gpu: &mut Gpu<'_>, actions: &mut Vec<Action>) {
    let mut state = app.hud.state.clone();
    let h = &mut app.hud;
    ui.set_next_item_width(130.0);
    if let Some(_c) = ui.begin_combo("##hud-mode", state.mode.label()) {
        for m in Mode::ALL {
            if ui.selectable_config(m.label()).selected(state.mode == m).build() {
                state.mode = m;
            }
        }
    }
    ui.same_line();
    ui.set_next_item_width(120.0);
    ui.slider_f32("Zoom", &mut h.zoom, 0.5, 3.0);
    ui.same_line();
    ui.checkbox("Rects", &mut h.show_rects);
    ui.same_line();
    ui.checkbox("HUD frame", &mut h.show_frame);
    let (zoom, rects, frame) = (h.zoom, h.show_rects, h.show_frame);
    let selected = h.selected;
    let hot: Vec<(usize, WidgetKey)> = st
        .hot
        .and_then(|i| h.shot().and_then(|s| s.findings.get(i)).map(|f| f.widgets.clone()))
        .unwrap_or_default();
    let Some(shot) = h.shot() else {
        ui.text_disabled("Pick a map and a HUD on the left.");
        state_panel(ui, &mut state, fonts, None);
        if state != app.hud.state {
            actions.push(Action::HudState(Box::new(state)));
        }
        return;
    };
    let Some(texture) = upload(st, gpu, shot) else {
        ui.text_disabled("The preview could not create its texture.");
        return;
    };
    let size = [640.0 * zoom, 480.0 * zoom];
    let origin = ui.cursor_screen_pos();
    ui.image(texture, size);
    let hovered = ui.is_item_hovered();
    st.shown = Some((origin, zoom));
    let at = |px: [f32; 2]| [origin[0] + px[0] * zoom, origin[1] + px[1] * zoom];
    let dl = ui.get_window_draw_list();
    let clip = dl.push_clip_rect(origin, [origin[0] + size[0], origin[1] + size[1]], true);
    let boxed = |r: place::Rect, c: [f32; 4], t: f32| {
        dl.add_rect(at([r.x0, r.y0]), at([r.x1, r.y1]), c).thickness(t).build();
    };
    if frame {
        boxed(shot.picture.frame.rect(), FRAME, 1.0);
    }
    let bad: Vec<(usize, WidgetKey)> = shot
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Warning)
        .flat_map(|f| f.widgets.iter().copied())
        .collect();
    for p in &shot.picture.placed {
        let Some(r) = p.rect else { continue };
        let id = (p.hud, p.key);
        if hot.contains(&id) {
            boxed(r, BAD, 3.0);
        } else if p.hud == 0 && selected == Some(p.key) {
            boxed(r, SELECTED, 2.0);
        } else if bad.contains(&id) {
            boxed(r, BAD, 1.5);
        } else if rects && !p.shown {
            boxed(r, HIDDEN, 1.0);
        } else if rects {
            boxed(r, if p.hud == 0 { RECT } else { OTHER }, 1.0);
        }
    }
    drop(clip);
    // Select with a click; a drag moves the widget (its offset in the
    // mode shown), one undo step per drag.
    let m = ui.io().mouse_pos();
    let px = [(m[0] - origin[0]) / zoom, (m[1] - origin[1]) / zoom];
    if hovered && ui.is_mouse_clicked(MouseButton::Left) {
        let hit = app.hud.widget_at(px);
        actions.push(Action::HudSelect(hit));
        st.drag = hit.and_then(|key| {
            Some(Drag {
                key,
                from: m,
                offset: app.hud.offset(key)?,
                moved: false,
            })
        });
    }
    if let Some(d) = &mut st.drag {
        if ui.is_mouse_down(MouseButton::Left) {
            let dx = ((m[0] - d.from[0]) / zoom).round() as i32;
            let dy = ((m[1] - d.from[1]) / zoom).round() as i32;
            let to = (
                (i32::from(d.offset.0) + dx).clamp(-32768, 32767) as i16,
                (i32::from(d.offset.1) + dy).clamp(-32768, 32767) as i16,
            );
            if app.hud.offset(d.key) != Some(to) {
                actions.push(Action::HudMove(d.key, to, d.moved));
                d.moved = true;
            }
        } else {
            st.drag = None;
        }
    }
    if hovered && st.drag.is_none() {
        let hit = app.hud.widget_at(px);
        let name = hit.and_then(|k| {
            app.hud.shot().and_then(|s| {
                s.picture.placed.iter().find(|p| p.hud == 0 && p.key == k).map(|p| p.name.clone())
            })
        });
        ui.tooltip_text(format!(
            "({:.0}, {:.0}){}",
            px[0],
            px[1],
            name.map(|n| format!("  {n}")).unwrap_or_default()
        ));
    }
    state_panel(ui, &mut state, fonts, app.hud.session.as_ref().and_then(|s| s.hud()));
    if state != app.hud.state {
        actions.push(Action::HudState(Box::new(state)));
    }
}

/// The state the HUD is drawn at, and the inputs its widgets read.
fn state_panel(ui: &Ui, s: &mut HudState, fonts: &Fonts, hud: Option<&hud_edit::Hud>) {
    ui.spacing();
    heading(ui, fonts, "State");
    let pct = |ui: &Ui, label: &str, v: &mut f32| {
        let mut p = *v * 100.0;
        ui.set_next_item_width(160.0);
        if ui.slider_f32(label, &mut p, 0.0, 100.0) {
            *v = p / 100.0;
        }
    };
    pct(ui, "Shield %", &mut s.shield);
    ui.same_line();
    pct(ui, "Health %", &mut s.body);
    let count = |ui: &Ui, label: &str, v: &mut u16, hi: i32| {
        let mut x = i32::from(*v);
        ui.set_next_item_width(160.0);
        if ui.slider_i32(label, &mut x, 0, hi) {
            *v = x.clamp(0, hi) as u16;
        }
    };
    let mut f = u16::from(s.frags);
    count(ui, "Frags", &mut f, 9);
    s.frags = f as u8;
    ui.same_line();
    let mut p = u16::from(s.plasmas);
    count(ui, "Plasmas", &mut p, 9);
    s.plasmas = p as u8;
    if ui.radio_button_bool("Frag selected", s.grenade == Grenade::Frag) {
        s.grenade = Grenade::Frag;
    }
    ui.same_line();
    if ui.radio_button_bool("Plasma selected", s.grenade == Grenade::Plasma) {
        s.grenade = Grenade::Plasma;
    }
    ui.same_line();
    ui.checkbox("Zoomed", &mut s.zoomed);
    count(ui, "Rounds loaded", &mut s.rounds_loaded, 999);
    ui.same_line();
    count(ui, "Magazine", &mut s.magazine, 999);
    pct(ui, "Heat %", &mut s.heat);
    ui.same_line();
    pct(ui, "Battery %", &mut s.battery);
    ui.checkbox("Gate widgets by their state flags", &mut s.gate);
    ui.set_item_tooltip(
        "The engine shows a widget when one of its four show words meets the state's word \
         (A, B, C, D) and none of its hide words does (0x13ac87). Which game state sets \
         which bit is mostly not known: grenade type (A 0x04 frag, 0x08 plasma) and zoom \
         (A 0x40 unzoomed) are set from the state above; set the rest here.",
    );
    if s.gate {
        for (i, w) in s.words.iter_mut().enumerate() {
            let mut x = i32::from(*w);
            ui.set_next_item_width(110.0);
            if ui.input_int(format!("Word {}##hud-word-{i}", ["A", "B", "C", "D"][i]), &mut x) {
                *w = x.clamp(0, 0xffff) as u16;
            }
            if i % 2 == 0 {
                ui.same_line();
            }
        }
    }
    // The inputs the HUD's widgets read, with their values.
    let Some(hud) = hud else { return };
    let mut used: Vec<u8> = hud.widgets.iter().flat_map(|w| w.inputs()).filter(|i| *i != 0).collect();
    used.sort_unstable();
    used.dedup();
    if used.is_empty() || !ui.collapsing_header("Inputs##hud-inputs", TreeNodeFlags::NONE) {
        return;
    }
    for n in used {
        let (v, src) = s.input(n);
        let what = match src {
            Source::Known(name) => name.to_owned(),
            Source::Manual => "set by hand".to_owned(),
            Source::Unknown => "not known: 1 used".to_owned(),
        };
        let mut x = v;
        ui.set_next_item_width(120.0);
        if ui.input_float(format!("input {n} ({what})##hud-in-{n}"), &mut x) && x.is_finite() {
            s.inputs.insert(n, x);
        }
        if state::input_name(n).is_none() && s.inputs.contains_key(&n) {
            ui.same_line();
            if ui.small_button(format!("clear##hud-in-clear-{n}")) {
                s.inputs.remove(&n);
            }
        }
    }
}

fn upload(st: &mut HudUi, gpu: &mut Gpu<'_>, shot: &crate::app::hud::Shot) -> Option<TextureId> {
    let c = &shot.picture.canvas;
    if st.texture.is_none() {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hud preview"),
            size: wgpu::Extent3d {
                width: c.width,
                height: c.height,
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
        let id = match gpu.renderer.register_external_texture(&view) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("hud texture: {e}");
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

/// The inspector: the selected widget's fields, the checks, the HUDs drawn
/// with this one, the placement rules and their proof, and the save.
pub fn inspector(
    ui: &Ui,
    app: &mut App,
    st: &mut HudUi,
    fonts: &Fonts,
    dialogs: bool,
    actions: &mut Vec<Action>,
) {
    let Some(_) = app.hud.session.as_ref() else {
        ui.text_disabled("No map open in the HUD tool.");
        rules(ui, fonts);
        return;
    };
    heading(ui, fonts, "Widget");
    match app.hud.selected {
        Some(key) => properties(ui, &mut app.hud, &mut st.panel, key, actions),
        None => ui.text_disabled("Click a widget in the picture or the list."),
    }
    ui.spacing();
    ui.separator();
    heading(ui, fonts, "Checks");
    st.hot = None;
    if let Some(shot) = app.hud.shot() {
        if shot.findings.is_empty() {
            ui.text_colored(theme::OK, format!("{} no overlaps, nothing off the screen", icon::CHECK));
        }
        for (i, f) in shot.findings.iter().enumerate() {
            let c = match f.severity {
                Severity::Warning => theme::ERROR,
                Severity::Note => theme::WARN,
            };
            ui.text_colored(c, icon::ALERT);
            ui.same_line();
            ui.text_wrapped(&f.text);
            if ui.is_item_hovered() {
                st.hot = Some(i);
            }
        }
        let notes: Vec<String> = shot
            .picture
            .placed
            .iter()
            .filter(|p| p.hud == 0)
            .flat_map(|p| p.notes.iter().map(move |n| format!("{}: {n}", p.name)))
            .collect();
        if !notes.is_empty() {
            ui.text_disabled(notes.join("\n"));
        }
    }
    ui.spacing();
    ui.separator();
    // HUDs drawn with this one: the engine draws a unit's and its weapons'
    // together (0x1394b9).
    if ui.collapsing_header("Draw with (weapon held, vehicle seat)##hud-with", TreeNodeFlags::NONE) {
        let s = app.hud.session.as_ref().unwrap();
        let current = s.current;
        for e in s.huds.iter().filter(|e| Some(e.id) != current) {
            let mut on = s.with.contains(&e.id);
            let short = e.name.rsplit('\\').next().unwrap_or(&e.name);
            if ui.checkbox(format!("{short}##hud-with-{}", e.id.0), &mut on) {
                actions.push(Action::HudWith(e.id, on));
            }
            ui.set_item_tooltip(&e.name);
        }
    }
    rules(ui, fonts);
    ui.spacing();
    ui.separator();
    heading(ui, fonts, "Save");
    let s = app.hud.session.as_ref().unwrap();
    ui.text_disabled(if s.is_dirty() {
        format!("{} unsaved edits in {} HUD(s)", icon::ALERT, s.edited().len())
    } else {
        "no unsaved edits".to_owned()
    });
    let d = ui.begin_disabled_with_cond(s.edited().is_empty());
    if ui.button(format!("{} Save map as...", icon::SAVE))
        && let Some(p) = save_dialog(dialogs, app)
    {
        actions.push(Action::HudSaveAs(p));
    }
    d.end();
    ui.text_wrapped("Writes a new .map: the source byte for byte but for the edited widget fields.");
}

/// The file the HUD tool's map saves to.
pub fn save_dialog(dialogs: bool, app: &App) -> Option<PathBuf> {
    if !dialogs {
        return None;
    }
    let name = app
        .hud
        .session
        .as_ref()
        .map(|s| format!("{}_hud.map", s.name.split(' ').next().unwrap_or("map")))
        .unwrap_or_else(|| "hud.map".into());
    rfd::FileDialog::new()
        .set_title("Save map with HUD edits as")
        .add_filter("Map", &["map"])
        .set_file_name(name)
        .save_file()
}

fn rules(ui: &Ui, _fonts: &Fonts) {
    if !ui.collapsing_header("Placement rules and their proof##hud-rules", TreeNodeFlags::NONE) {
        return;
    }
    for r in place::RULES {
        let c = match r.proof {
            Proof::Engine => theme::OK,
            Proof::Seen => theme::ACCENT,
            Proof::Assumed => theme::WARN,
        };
        ui.text_colored(c, r.proof.tag());
        ui.same_line();
        ui.text_wrapped(r.what);
        ui.set_item_tooltip(r.cite);
    }
    ui.text_disabled("E engine code, V seen on the disc or in a frame, D assumed (unproven).");
}

/// The selected widget's fields: the generic field panel over its element,
/// and a row of the HUD tool's own for each field the panel cannot edit.
fn properties(ui: &Ui, h: &mut HudTool, panel: &mut PanelState, key: WidgetKey, actions: &mut Vec<Action>) {
    let Some(s) = h.session.as_mut() else { return };
    let Some(hud_id) = s.current else { return };
    let Some(w) = s.hud().and_then(|hud| hud.widget(key)).cloned() else {
        ui.text_disabled("The widget is gone.");
        return;
    };
    ui.text(format!("{} ({:?} {})", w.name, key.kind, key.index));
    ui.text_disabled(format!("at {:#010x}{}", w.address, if w.is_dirty() { ", edited" } else { "" }));
    // The widget's element in the generic field panel (names from the
    // nhdt definition); its writes become HUD edits (undo, the dirty
    // marker and the save stay the HUD tool's).
    let block = match key.kind {
        WidgetKind::Bitmap => hud_edit::layout::ROOT_BITMAPS,
        WidgetKind::Text => hud_edit::layout::ROOT_TEXTS,
    };
    let path = [(block as u32, key.index as u32)];
    let layout = h.fields.resolve(hud_id, &path).map(|(_, l)| l);
    // The fields the panel cannot edit (the schema types their bytes as a
    // raw pointer or size, or nothing names them): the HUD tool's rows.
    let own: Vec<&Field> = key
        .kind
        .fields()
        .iter()
        .filter(|f| {
            layout.as_ref().is_none_or(|l| {
                l.fields.iter().find(|d| d.offset as usize <= f.offset && (f.offset as u32) < d.offset + d.kind.width()).is_none_or(|d| {
                    matches!(d.kind, RegKind::Raw | RegKind::RawSize { .. } | RegKind::Unknown { .. } | RegKind::Pad { .. })
                })
            })
        })
        .collect();
    if !own.is_empty() {
        ui.separator_with_text("HUD fields");
        for f in own {
            let _id = ui.push_id(f.key);
            ui.align_text_to_frame_padding();
            ui.text_disabled(f.label);
            ui.set_item_tooltip(format!("{:#x}  {}", f.offset, f.proof));
            ui.same_line_with_pos(130.0);
            ui.set_next_item_width(-1.0);
            if let Some(v) = field(ui, s, &w, f) {
                actions.push(Action::HudSet(key, f.key.to_owned(), v, true));
            }
        }
    }
    ui.separator_with_text("Fields");
    let mut out = Vec::new();
    fields::panel(ui, &h.fields, hud_id, &path, panel, &mut out);
    for a in out {
        match a {
            Action::Tag(TagAction::Write(w)) => actions.push(Action::HudWrite(w)),
            Action::Tag(TagAction::Block { .. }) => {
                actions.push(Action::HudRefused("a widget's blocks (effects) are edited in the Tags tool".into()))
            }
            Action::Tag(TagAction::Seal | TagAction::Select(_)) => {}
            other => actions.push(other),
        }
    }
}

/// One field's editor; the new value when it changed.
fn field(ui: &Ui, s: &mut hud_edit::HudSession, w: &hud_edit::Widget, f: &Field) -> Option<Value> {
    match (f.kind, w.get_field(f)) {
        (Kind::StringId, Value::String(v)) => {
            ui.text(s.string(v).unwrap_or_else(|| format!("{v:#x}")));
            None
        }
        (Kind::TagRef, Value::Tag(id)) => {
            let group = if f.key == "bitmap" { hud_edit::model::BITM } else { map::FourCC(*b"shad") };
            let shown = if id == map::TagId::NULL { "none".to_owned() } else { s.tag_name(id) };
            let _c = ui.begin_combo("##ref", &shown)?;
            let mut out = None;
            for (t, name) in s.tags_of(group) {
                if ui.selectable_config(&name).selected(t == id).build() {
                    out = Some(Value::Tag(t));
                }
            }
            out
        }
        (Kind::F32, Value::Float(v)) => {
            let mut x = v;
            (ui.input_float("##f", &mut x) && x.is_finite() && x != v).then_some(Value::Float(x))
        }
        (Kind::Hex16, Value::Int(v)) => {
            let mut t = format!("{v:04x}");
            if ui.input_text("##hex", &mut t).build() {
                let n = u16::from_str_radix(t.trim().trim_start_matches("0x"), 16).ok()?;
                return (i64::from(n) != v).then_some(Value::Int(i64::from(n)));
            }
            None
        }
        (Kind::U16, Value::Int(v)) if f.key == "anchor" => {
            let names = ["0 top right", "1 top left", "2 bottom left", "3 bottom right", "4 centre"];
            let shown = names.get(v as usize).map_or(format!("{v}"), |n| (*n).to_owned());
            let _c = ui.begin_combo("##anchor", &shown)?;
            let mut out = None;
            for (i, n) in names.iter().enumerate() {
                if ui.selectable_config(*n).selected(v == i as i64).build() && v != i as i64 {
                    out = Some(Value::Int(i as i64));
                }
            }
            out
        }
        (_, Value::Int(v)) => {
            let mut x = v as i32;
            (ui.input_int("##i", &mut x) && i64::from(x) != v).then_some(Value::Int(i64::from(x)))
        }
        _ => None,
    }
}
