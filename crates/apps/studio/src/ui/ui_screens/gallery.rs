//! The UI Screens tool's grid of previews in the centre panel: every
//! screen of a root or folder as a tile (the thumbnail, its name, a badge
//! for its findings), and the breadcrumb above a picked screen that goes
//! back to its folder. The thumbnails are CPU drawn in
//! [`crate::app::UiScreens::draw_thumbs`], a bounded time per frame and
//! only for tiles on screen; this module uploads them as textures.
//! docs/editor/editor.md "UI Screens".

use std::collections::HashMap;
use std::time::Duration;

use dear_imgui_rs::{TextureId, Ui};

use crate::app::ui_screens::browse::{self, Browse, Folder, Root};
use crate::app::ui_screens::gallery::{THUMB, Thumb};
use crate::app::{Action, App, Pick};
use crate::theme::{self, icon};
use crate::viewport::Gpu;

use super::Texture;

/// What a frame may spend drawing thumbnails.
const DRAW_BUDGET: Duration = Duration::from_millis(12);
/// Textures kept; those not shown this frame go beyond it.
const KEEP_TEXTURES: usize = 96;
/// Under each picture: its name and badge.
const LABEL_HEIGHT: f32 = 22.0;
const GAP: f32 = 10.0;

struct Tex {
    texture: Texture,
    seen: u64,
}

/// The grid's UI-only state: a texture per tile on screen.
#[derive(Default)]
pub struct GalleryUi {
    textures: HashMap<Pick, Tex>,
    frame: u64,
}

fn no_disc(app: &App) -> String {
    match app.ui_screens.disc_error() {
        Some(e) => format!("The disc did not open: {e}"),
        None => "Open a disc image to list the game's own screens.".to_owned(),
    }
}

/// The crumbs of folder `folder` under `root`: the root, then each folder
/// down to it, as buttons that go there. With `current` (a picked screen)
/// every folder is a button and the screen follows as plain text; without,
/// the last folder (the one shown) is plain. Returns where one went.
fn crumbs(
    ui: &Ui,
    root: Root,
    tree: &Folder,
    folder: &str,
    current: Option<&str>,
) -> Option<Browse> {
    let mut go = None;
    let parts = tree.crumbs(folder);
    let last_plain = current.is_none();
    if parts.is_empty() && last_plain {
        ui.text(root.title());
    } else if ui.small_button(format!("{}##crumb-root", root.title())) {
        go = Some(Browse::root(root));
    }
    for (i, (label, id)) in parts.iter().enumerate() {
        ui.same_line();
        ui.text_disabled(">");
        ui.same_line();
        if last_plain && i + 1 == parts.len() {
            ui.text(label);
        } else if ui.small_button(format!("{label}##crumb-{id}")) {
            go = Some(Browse {
                root,
                folder: id.clone(),
            });
        }
    }
    if let Some(c) = current {
        ui.same_line();
        ui.text_disabled(">");
        ui.same_line();
        ui.text(c);
    }
    go
}

/// The breadcrumb above a picked screen's picture: its root and folders,
/// each a way back to that folder's grid. True when one was clicked (the
/// grid shows now, so the picture is not drawn this frame).
pub fn pick_crumbs(ui: &Ui, app: &mut App) -> bool {
    let Some(pick) = app.ui_screens.pick.clone() else {
        return false;
    };
    let (root, name, current) = match &pick {
        Pick::Native { screen, state } => (Root::Ours, screen.clone(), format!("{screen} / {state}")),
        Pick::Retail(tag) => (
            Root::Retail,
            tag.clone(),
            tag.rsplit('\\').next().unwrap_or(tag).to_owned(),
        ),
    };
    let retail = app.ui_screens.retail_tree();
    let tree: &Folder = match root {
        Root::Ours => &browse::ours_tree().tree,
        Root::Retail => match &retail {
            Some(r) => &r.tree,
            None => return false,
        },
    };
    let folder = tree.folder_of(&name).unwrap_or_default();
    let go = crumbs(ui, root, tree, &folder, Some(&current));
    ui.separator();
    match go {
        Some(b) => {
            app.dispatch(Action::UiBrowse(Some(b)));
            true
        }
        None => false,
    }
}

/// The grid of `at`: its breadcrumb and controls, then the tiles by
/// folder. Clicking a tile picks the screen.
pub fn show(ui: &Ui, app: &mut App, st: &mut GalleryUi, gpu: &mut Gpu<'_>, at: Browse) {
    st.frame += 1;
    let retail = app.ui_screens.retail_tree();
    let tree: &Folder = match at.root {
        Root::Ours => &browse::ours_tree().tree,
        Root::Retail => match &retail {
            Some(r) => &r.tree,
            None => {
                ui.text(at.root.title());
                ui.text_wrapped(no_disc(app));
                return;
            }
        },
    };
    let mut go = crumbs(ui, at.root, tree, &at.folder, None);

    let g = &mut app.ui_screens.gallery;
    ui.same_line();
    ui.set_next_item_width(140.0);
    ui.slider_f32("Size", &mut g.zoom, 0.4, 1.0);
    if at.root == Root::Ours {
        ui.same_line();
        ui.checkbox("All states", &mut g.states);
    }
    let (zoom, states) = (g.zoom, g.states);
    let sections = app.ui_screens.sections(&at, states);
    let total: usize = sections.iter().map(|s| s.tiles.len()).sum();
    if total == 0 {
        ui.text_disabled("No screens here.");
        return;
    }
    ui.text_disabled(match at.root {
        Root::Ours => format!("{total} previews, as drawn here (click one to open it)"),
        Root::Retail => format!(
            "{total} previews, as the disc draws them{}",
            retail
                .as_ref()
                .filter(|r| !r.stripped.is_empty())
                .map_or(String::new(), |r| format!(" (under {})", r.stripped))
        ),
    });
    ui.separator();

    let picked = app.ui_screens.pick.clone();
    let (tw, th) = (THUMB[0] as f32 * zoom, THUMB[1] as f32 * zoom);
    let avail = ui.content_region_avail()[0];
    let cols = (((avail + GAP) / (tw + GAP)).floor() as usize).max(1);
    let row = th + LABEL_HEIGHT + GAP;
    let mut wanted: Vec<Pick> = Vec::new();
    let mut clicked: Option<Pick> = None;
    for (si, section) in sections.iter().enumerate() {
        if !section.title.is_empty() {
            ui.spacing();
            ui.separator_with_text(&section.title);
        }
        let origin = ui.cursor_screen_pos();
        for (i, tile) in section.tiles.iter().enumerate() {
            let min = [
                origin[0] + (i % cols) as f32 * (tw + GAP),
                origin[1] + (i / cols) as f32 * row,
            ];
            let max = [min[0] + tw, min[1] + th + LABEL_HEIGHT];
            ui.set_cursor_screen_pos(min);
            let hit = ui.invisible_button(format!("##tile-{si}-{i}"), [tw, th + LABEL_HEIGHT]);
            if !ui.is_rect_visible_min_max(min, max) {
                continue;
            }
            let hovered = ui.is_item_hovered();
            let dl = ui.get_window_draw_list();
            dl.add_rect(min, max, theme::RAISED)
                .filled(true)
                .rounding(4.0)
                .build();
            let thumb = app.ui_screens.gallery.thumb(&tile.pick);
            let mut message = None;
            match thumb {
                None => {
                    wanted.push(tile.pick.clone());
                    message = Some("drawing...".to_owned());
                }
                Some(t) => match tile_texture(st, gpu, &tile.pick, t) {
                    Some((id, w, h)) => {
                        let size = [w as f32 * zoom, h as f32 * zoom];
                        let pos = [min[0] + (tw - size[0]) * 0.5, min[1] + (th - size[1]) * 0.5];
                        dl.add_image(
                            id,
                            pos,
                            [pos[0] + size[0], pos[1] + size[1]],
                            [0.0, 0.0],
                            [1.0, 1.0],
                            [1.0, 1.0, 1.0, 1.0],
                        );
                    }
                    None => message = Some(t.error.clone().unwrap_or_else(|| "no texture".into())),
                },
            }
            if let Some(m) = message {
                let _clip = dl.push_clip_rect(min, [max[0], min[1] + th], true);
                dl.add_text([min[0] + 6.0, min[1] + 6.0], theme::DIM, &m);
            }
            let on = picked.as_ref() == Some(&tile.pick);
            let (edge, width) = if on {
                (theme::WIDGET_FOCUS, 2.0)
            } else if hovered {
                (theme::ACCENT, 2.0)
            } else {
                (theme::RULE, 1.0)
            };
            dl.add_rect(min, max, edge).rounding(4.0).thickness(width).build();

            // The name, and a badge: how many checks found something.
            let badge = thumb.filter(|t| t.checked).map(|t| {
                if t.findings > 0 {
                    (format!("{} {}", icon::ALERT, t.findings), theme::ERROR)
                } else {
                    (icon::CHECK.to_owned(), theme::OK)
                }
            });
            let badge_w = badge
                .as_ref()
                .map_or(0.0, |(b, _)| ui.calc_text_size(b)[0] + 6.0);
            let y = min[1] + th + 3.0;
            {
                let _clip = dl.push_clip_rect(
                    [min[0] + 4.0, y],
                    [max[0] - badge_w - 2.0, max[1]],
                    true,
                );
                dl.add_text([min[0] + 6.0, y], theme::TEXT, &tile.label);
            }
            if let Some((b, colour)) = &badge {
                dl.add_text([max[0] - badge_w, y], *colour, b);
            }
            if hovered {
                ui.tooltip_text(&tile.name);
            }
            if hit {
                clicked = Some(tile.pick.clone());
            }
        }
        let rows = section.tiles.len().div_ceil(cols);
        ui.set_cursor_screen_pos([origin[0], origin[1] + rows as f32 * row]);
        ui.dummy([1.0, 1.0]);
    }

    // Draw what was missing, those on screen first, for what is left of
    // the frame; the textures upload on the next.
    app.ui_screens.draw_thumbs(&wanted, DRAW_BUDGET);
    forget_unseen(st, gpu);
    if let Some(p) = clicked {
        app.dispatch(Action::UiPick(p));
    } else if let Some(b) = go.take() {
        app.dispatch(Action::UiBrowse(Some(b)));
    }
}

/// Drops the textures of tiles that were not shown this frame, once there
/// are many.
fn forget_unseen(st: &mut GalleryUi, gpu: &mut Gpu<'_>) {
    if st.textures.len() <= KEEP_TEXTURES {
        return;
    }
    let frame = st.frame;
    let gone: Vec<Pick> = st
        .textures
        .iter()
        .filter(|(_, t)| t.seen != frame)
        .map(|(p, _)| p.clone())
        .collect();
    for p in gone {
        if let Some(t) = st.textures.remove(&p) {
            let _ = gpu.renderer.unregister_external_texture(t.texture.id);
        }
    }
}

/// The thumbnail on the GPU (uploaded when new), with its size.
fn tile_texture(
    st: &mut GalleryUi,
    gpu: &mut Gpu<'_>,
    pick: &Pick,
    thumb: &Thumb,
) -> Option<(TextureId, u32, u32)> {
    let c = thumb.canvas.as_ref()?;
    let fresh = st
        .textures
        .get(pick)
        .is_none_or(|t| t.texture.texture.width() != c.width || t.texture.texture.height() != c.height);
    if fresh {
        if let Some(old) = st.textures.remove(pick) {
            let _ = gpu.renderer.unregister_external_texture(old.texture.id);
        }
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui screens thumbnail"),
            size: wgpu::Extent3d {
                width: c.width,
                height: c.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Not sRGB, as the picture's.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let id = match gpu.renderer.register_external_texture(&view) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("ui screens thumbnail texture: {e}");
                return None;
            }
        };
        st.textures.insert(
            pick.clone(),
            Tex {
                texture: Texture {
                    texture,
                    id,
                    rev: u64::MAX,
                },
                seen: 0,
            },
        );
    }
    let t = st.textures.get_mut(pick)?;
    t.seen = st.frame;
    if t.texture.rev != thumb.id {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t.texture.texture,
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
        t.texture.rev = thumb.id;
    }
    Some((t.texture.id.texture_id(), c.width, c.height))
}
