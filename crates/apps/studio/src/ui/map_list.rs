//! The Maps list's rows, every group alike: a map with a card
//! ([`crate::app::map_cards`], or a Map Pack record) has a thumbnail of
//! its picture beside the game's name for it, and on hover the whole
//! picture, the name, the description and a dim line saying where the file
//! comes from. A map with none keeps its file name and icon, and its
//! tooltip says what it is. The pictures share one GPU cache, keyed by
//! row. The Halo 2 source's group is drawn here, the Map Pack's in
//! [`super::map_pack`], Packages and Halo 1 in `Studio::library_lists`.
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use dear_imgui_rs::{TextureId, Ui};
use dear_imgui_wgpu::ExternalTextureId;
use editor_core::MapSource;
use editor_core::halo1::H1Source;

use crate::app::map_cards::{halo1_note, halo2_note};
use crate::app::map_pack::Picture;
use crate::app::{Action, App};
use crate::theme::icon;
use crate::viewport::Gpu;

/// The thumbnail's height in points.
pub const THUMB: f32 = 18.0;
/// The tooltip's width for the picture and the description, in points.
pub const WRAP: f32 = 260.0;

struct Texture {
    _texture: wgpu::Texture,
    id: ExternalTextureId,
    /// The picture it holds.
    from: Arc<Picture>,
}

/// The Maps list's groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// The open Halo 2 source's maps.
    Halo2,
    MapPack,
    Packages,
    Halo1,
}

/// What a group drew.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// The Map Pack's line on how to install it.
    Hint,
    /// A row a map: its label and whether its picture was shown.
    Maps(Vec<(String, bool)>),
}

/// The list's UI-only state: the pictures on the GPU, by row key.
#[derive(Default)]
pub struct MapListUi {
    textures: HashMap<String, Texture>,
    /// What the last frame drew, by group. For scripts and tests.
    pub shown: BTreeMap<Group, Shown>,
}

/// One row of the list.
pub struct Row<'a> {
    /// Unique in the list: the widget's ID and the picture's cache key
    /// (`map-<name>`, `pack-<map>`, `pkg-<key>`, `h1-<name>`).
    pub key: String,
    /// The game's name for the map, else its file name.
    pub label: &'a str,
    /// Shown before the label when there is no picture.
    pub icon: &'a str,
    pub selected: bool,
    pub picture: Option<&'a Arc<Picture>>,
    /// Wrapped and dim under the name in the tooltip; may be empty.
    pub description: &'a str,
    /// The tooltip's last, dim line: the file and where it comes from.
    pub from: &'a str,
}

impl MapListUi {
    /// What `group` drew in the last frame.
    pub fn shown(&self, group: Group) -> Option<&Shown> {
        self.shown.get(&group)
    }

    /// Forget what the last frame drew.
    pub fn begin(&mut self) {
        self.shown.clear();
    }

    /// `group` drew its hint and no maps.
    pub fn hint(&mut self, group: Group) {
        self.shown.insert(group, Shown::Hint);
    }

    fn record(&mut self, group: Group, label: &str, pictured: bool) {
        let shown = self
            .shown
            .entry(group)
            .or_insert_with(|| Shown::Maps(Vec::new()));
        if let Shown::Maps(rows) = shown {
            rows.push((label.to_owned(), pictured));
        } else {
            *shown = Shown::Maps(vec![(label.to_owned(), pictured)]);
        }
    }

    /// Draws one row. True when it was clicked while not selected.
    pub fn row(&mut self, ui: &Ui, gpu: &mut Gpu<'_>, group: Group, r: &Row<'_>) -> bool {
        let thumb = r
            .picture
            .and_then(|p| self.texture(gpu, &r.key, p).map(|t| (t, p.width, p.height)));
        let label = match thumb {
            Some((t, w, h)) => {
                ui.image(t, [THUMB * w as f32 / h as f32, THUMB]);
                ui.same_line();
                format!("{}##{}", r.label, r.key)
            }
            None => format!("{}  {}##{}", r.icon, r.label, r.key),
        };
        self.record(group, r.label, thumb.is_some());
        let clicked = ui.selectable_config(label).selected(r.selected).build() && !r.selected;
        if ui.is_item_hovered() {
            ui.tooltip(|| {
                if let Some((t, w, h)) = thumb {
                    // Larger pictures (a package's own) shrink to the
                    // tooltip's width.
                    let scale = (WRAP / w as f32).min(1.0);
                    ui.image(t, [w as f32 * scale, h as f32 * scale]);
                }
                ui.text(r.label);
                let wrap = ui.push_text_wrap_pos(ui.cursor_pos()[0] + WRAP);
                if !r.description.is_empty() {
                    ui.text_disabled(r.description);
                }
                if !r.from.is_empty() {
                    ui.text_disabled(r.from);
                }
                drop(wrap);
            });
        }
        clicked
    }

    /// A picture on the GPU under `key` (its own namespace beside the
    /// rows'), uploaded when new or changed: for other windows' pictures
    /// (the Tags tool's bitmap preview).
    pub fn picture(&mut self, gpu: &mut Gpu<'_>, key: &str, pic: &Arc<Picture>) -> Option<TextureId> {
        self.texture(gpu, key, pic)
    }

    /// The picture for row `key` on the GPU, uploaded when new or changed.
    fn texture(&mut self, gpu: &mut Gpu<'_>, key: &str, pic: &Arc<Picture>) -> Option<TextureId> {
        if let Some(t) = self.textures.get(key)
            && Arc::ptr_eq(&t.from, pic)
        {
            return Some(t.id.texture_id());
        }
        if let Some(old) = self.textures.remove(key) {
            let _ = gpu.renderer.unregister_external_texture(old.id);
        }
        if pic.width == 0
            || pic.height == 0
            || pic.rgba.len() != (pic.width * pic.height * 4) as usize
        {
            return None;
        }
        let size = wgpu::Extent3d {
            width: pic.width,
            height: pic.height,
            depth_or_array_layers: 1,
        };
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("map picture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Dear ImGui takes texels as sRGB-encoded already (the
            // viewport's note), and the pictures are.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pic.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * pic.width),
                rows_per_image: Some(pic.height),
            },
            size,
        );
        let view = texture.create_view(&Default::default());
        let id = match gpu.renderer.register_external_texture(&view) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("map picture: {e}");
                return None;
            }
        };
        let tid = id.texture_id();
        self.textures.insert(
            key.to_owned(),
            Texture {
                _texture: texture,
                id,
                from: Arc::clone(pic),
            },
        );
        Some(tid)
    }
}

/// Where the open Halo 2 source's maps come from, for the dim line.
pub fn halo2_place(source: &MapSource) -> String {
    match source {
        MapSource::Disc(_) => "on the Halo 2 disc".to_owned(),
        MapSource::Folder(dir) => format!("in {}", dir.display()),
        MapSource::Loose(file) => match file.parent() {
            Some(dir) => format!("in {}", dir.display()),
            None => String::new(),
        },
    }
}

/// Where the open Halo 1 source's maps come from, for the dim line.
pub fn halo1_place(source: &H1Source) -> String {
    match source {
        H1Source::Disc { .. } => "on the Halo 1 disc (read only)".to_owned(),
        H1Source::Folder { dir, .. } => format!("in {} (read only)", dir.display()),
    }
}

/// The open Halo 2 source's maps, in its order, each named and pictured
/// by its card once the main menu's lists are read.
pub fn halo2(ui: &Ui, app: &App, st: &mut MapListUi, gpu: &mut Gpu<'_>, actions: &mut Vec<Action>) {
    let Some(source) = app.source() else {
        return;
    };
    let place = halo2_place(source);
    let loaded = app
        .loaded_map()
        .filter(|_| app.opened_package().is_none() && app.opened_pack_map().is_none());
    for name in app.maps() {
        let card = app.halo2_card(name);
        let note = match card {
            Some(_) => "",
            None if app.halo2_cards_loading() => "Reading the main menu's level lists...",
            None => halo2_note(name),
        };
        let from = format!("{name}.map, {place}");
        let selected = loaded == Some(name.as_str());
        let row = Row {
            key: format!("map-{name}"),
            label: card.map_or(name.as_str(), |c| c.name.as_str()),
            icon: icon::MAP,
            selected,
            picture: card.and_then(|c| c.picture.as_ref()),
            description: card.map_or(note, |c| c.description.as_str()),
            from: &from,
        };
        if st.row(ui, gpu, Group::Halo2, &row) {
            actions.push(Action::SelectMap(name.clone()));
        }
    }
}

/// The open Halo 1 source's maps, read only, each named and pictured by
/// `ui.map`'s lists once read.
pub fn halo1(ui: &Ui, app: &App, st: &mut MapListUi, gpu: &mut Gpu<'_>, actions: &mut Vec<Action>) {
    let Some(source) = app.halo1_source() else {
        return;
    };
    let place = halo1_place(source);
    let loaded = app.loaded_halo1();
    for name in app.halo1_maps() {
        let card = app.halo1_card(name);
        let from = format!("{name}.map, {place}");
        let row = Row {
            key: format!("h1-{name}"),
            label: card.map_or(name.as_str(), |c| c.name.as_str()),
            icon: icon::EYE,
            selected: loaded == Some(name.as_str()),
            picture: card.and_then(|c| c.picture.as_ref()),
            description: card.map_or(halo1_note(name), |c| c.description.as_str()),
            from: &from,
        };
        if st.row(ui, gpu, Group::Halo1, &row) {
            actions.push(Action::SelectHalo1Map(name.clone()));
        }
    }
}
