//! The Maps list's Map Pack group: the installed Multiplayer Map Pack maps
//! in the game's Select Map order, each with its record's picture (a
//! thumbnail beside the name, the whole picture and description on hover).
//! With nothing installed the group stays, with a line on how to install
//! it. State lives in [`crate::app::map_pack`]; clicks go back as
//! [`Action::OpenMapPack`].
use std::collections::HashMap;
use std::sync::Arc;

use dear_imgui_rs::{TextureId, Ui};
use dear_imgui_wgpu::ExternalTextureId;

use crate::app::map_pack::{HINT, Picture};
use crate::app::{Action, App};
use crate::theme::icon;
use crate::viewport::Gpu;

/// The thumbnail's height in points (the picture is 220x207).
const THUMB: f32 = 18.0;

struct Texture {
    _texture: wgpu::Texture,
    id: ExternalTextureId,
    /// The picture it holds.
    from: Arc<Picture>,
}

/// The group's UI-only state: the pictures on the GPU, by offer.
#[derive(Default)]
pub struct MapPackUi {
    textures: HashMap<String, Texture>,
    /// What the last frame drew: the hint, else a row a map (its name
    /// and whether its picture was shown). For scripts and tests.
    pub shown: Option<Shown>,
}

/// What the group drew.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    Hint,
    Maps(Vec<(String, bool)>),
}

impl MapPackUi {
    /// The picture for `offer` on the GPU, uploaded when new or changed.
    fn texture(&mut self, gpu: &mut Gpu<'_>, offer: &str, pic: &Arc<Picture>) -> Option<TextureId> {
        if let Some(t) = self.textures.get(offer)
            && Arc::ptr_eq(&t.from, pic)
        {
            return Some(t.id.texture_id());
        }
        if let Some(old) = self.textures.remove(offer) {
            let _ = gpu.renderer.unregister_external_texture(old.id);
        }
        let size = wgpu::Extent3d {
            width: pic.width,
            height: pic.height,
            depth_or_array_layers: 1,
        };
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("map pack picture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Dear ImGui takes texels as sRGB-encoded already (the
            // viewport's note), and the record's pixels are.
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
                eprintln!("map pack picture: {e}");
                return None;
            }
        };
        let tid = id.texture_id();
        self.textures.insert(
            offer.to_owned(),
            Texture {
                _texture: texture,
                id,
                from: Arc::clone(pic),
            },
        );
        Some(tid)
    }
}

/// The Map Pack group: its heading, then a row a map, or the hint.
pub fn group(
    ui: &Ui,
    app: &App,
    st: &mut MapPackUi,
    gpu: &mut Gpu<'_>,
    heading: impl Fn(&Ui, &str),
    actions: &mut Vec<Action>,
) {
    ui.spacing();
    heading(ui, "Map Pack");
    if app.map_pack().is_empty() {
        ui.text_disabled(format!("{} {HINT}", icon::INFO));
        st.shown = Some(Shown::Hint);
        return;
    }
    let mut rows = Vec::new();
    let opened = app.opened_pack_map();
    for m in app.map_pack() {
        let thumb = m
            .picture
            .as_ref()
            .and_then(|p| st.texture(gpu, &m.offer, p).map(|t| (t, p.width, p.height)));
        if let Some((t, w, h)) = thumb {
            ui.image(t, [THUMB * w as f32 / h as f32, THUMB]);
            ui.same_line();
        }
        rows.push((m.name.clone(), thumb.is_some()));
        let on = opened.is_some_and(|o| o.eq_ignore_ascii_case(&m.map));
        if ui
            .selectable_config(format!("{}##pack-{}", m.name, m.map))
            .selected(on)
            .build()
            && !on
        {
            actions.push(Action::OpenMapPack(m.map.clone()));
        }
        if ui.is_item_hovered() {
            ui.tooltip(|| {
                if let Some((t, w, h)) = thumb {
                    ui.image(t, [w as f32, h as f32]);
                }
                ui.text(&m.name);
                if !m.description.is_empty() {
                    let _wrap = ui.push_text_wrap_pos(ui.cursor_pos()[0] + 260.0);
                    ui.text_disabled(&m.description);
                }
                ui.text_disabled(format!(
                    "{}.map, the Map Pack's, in T:\\$c\\{}\nopens with the disc's shared maps; \
                     edits save as a new package",
                    m.map, m.offer
                ));
            });
        }
    }
    st.shown = Some(Shown::Maps(rows));
}
