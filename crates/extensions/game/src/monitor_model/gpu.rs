//! The monitors' models on the GPU: each census entry's biped model read
//! once from its campaign map in the mounted game, in the background
//! ([`asset`]); the frame's bodies published to the frame graph
//! ([`publish`], `native_render::bodies`); and the Appearance page's
//! picture ([`appearance_view`]), a showcase pass composited beside the
//! Spartan in the page's model widget.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use engine::memory::GuestMemory;
use forge::look::body::Draw;
use forge::look::census::{self, Entry};
use native_render::bodies::{Body, BodyModel, Set};
use native_render::showcase::{self, Fit};
use native_render::showcase_gpu::{Showcase, ShowcaseMesh};

/// A model read from its map: what the world's stage draws, and the mesh
/// the page's picture draws.
pub struct Asset {
    pub body: Arc<BodyModel>,
    pub mesh: ShowcaseMesh,
    /// Map and tag, for the log.
    pub source: String,
}

enum State {
    Loading,
    Ready(Arc<Asset>),
    Failed,
}

/// By census identity: the key and the source map's hash.
static ASSETS: Mutex<Option<HashMap<(String, String), State>>> = Mutex::new(None);

/// The model of `entry`, once loaded. The first ask starts the load on a
/// background thread and answers `None`, as every ask does until it has
/// finished, or for good if it failed (said once, with why): the seat draws
/// the marker meanwhile.
pub fn asset(entry: &Entry) -> Option<Arc<Asset>> {
    let id = (entry.key.clone(), entry.sha256.clone());
    {
        let mut guard = ASSETS.lock().unwrap_or_else(|e| e.into_inner());
        let all = guard.get_or_insert_with(HashMap::new);
        match all.get(&id) {
            Some(State::Ready(asset)) => return Some(Arc::clone(asset)),
            Some(State::Loading | State::Failed) => return None,
            None => {}
        }
        all.insert(id.clone(), State::Loading);
    }
    let entry = entry.clone();
    let spawned = background::spawn("monitor model load", move || {
        let next = match load(&entry) {
            Ok(asset) => {
                eprintln!("monitor model: {} is {}", entry.key, asset.source);
                State::Ready(Arc::new(asset))
            }
            Err(why) => {
                eprintln!("monitor model: {} stays a marker: {why}", entry.key);
                State::Failed
            }
        };
        set(&entry.key, &entry.sha256, next);
    });
    if let Err(e) = spawned {
        eprintln!("monitor model: could not start the load: {e}");
        set(&id.0, &id.1, State::Failed);
    }
    None
}

fn set(key: &str, sha256: &str, state: State) {
    if let Some(all) = ASSETS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        all.insert((key.to_owned(), sha256.to_owned()), state);
    }
}

/// Reads `entry`'s biped model, its render model and each opaque
/// material's base map, from the entry's source map in the mounted game.
fn load(entry: &Entry) -> Result<Asset, String> {
    load_with(entry, &crate::native_map::open)
}

pub fn load_with(entry: &Entry, open: &crate::skulls_model::gpu::Open<'_>) -> Result<Asset, String> {
    let name = &entry.source;
    let (mut map, files) = open(name).map_err(|e| format!("{name}: {e}"))?;
    let bipeds: Vec<(String, map::TagId)> = map
        .tags()
        .iter()
        .filter(|t| t.group == map::FourCC(*b"bipd") && t.location.is_some())
        .filter_map(|t| Some((t.name.clone()?, t.id)))
        .collect();
    let tag = census::tag_for(entry, bipeds.iter().map(|(n, _)| n.as_str()))
        .ok_or_else(|| format!("{name} has no biped for {}", entry.key))?;
    let id = bipeds
        .iter()
        .find(|(n, _)| n == tag)
        .map(|(_, id)| *id)
        .expect("the tag came from this list");
    let ctx = tags::object::Context::read(&mut map).map_err(|e| format!("{name}: {e}"))?;
    let mut raw = crate::native_world_glue::Raw(files);
    let loaded = native_render::model_load::load_object_model(&mut map, &ctx, &mut raw, id)
        .map_err(|e| format!("{name} {tag}: {e}"))?;
    let mesh = ShowcaseMesh::from_model(&loaded.model)
        .ok_or_else(|| format!("{name} {tag}: nothing opaque to show"))?;
    let bases = loaded
        .model
        .materials()
        .into_iter()
        .filter_map(|m| {
            let shader = *loaded.shaders.get(usize::from(m))?;
            let bitmap = native_render::load::base_map(&mut map, shader)?;
            let image = native_render::load::decode_first(&mut map, &mut raw.0, bitmap)?;
            Some((m, image))
        })
        .collect();
    Ok(Asset {
        body: Arc::new(BodyModel {
            model: loaded.model,
            bases,
        }),
        mesh,
        source: format!("{tag} in {name}"),
    })
}

/// The world's frame: each drawn body with a loaded model as a [`Body`],
/// the models it uses once each. `model_of` gives the host list's model
/// (`Draw::model`) as a loaded [`BodyModel`], or none for a model not
/// loaded (yet): that seat draws nothing here.
pub fn build_set(
    draws: &[Draw],
    model_of: impl Fn(u32) -> Option<Arc<BodyModel>>,
) -> Set {
    let mut set = Set::default();
    let mut used: Vec<(u32, usize)> = Vec::new();
    for draw in draws {
        let Some(id) = draw.model else { continue };
        let index = match used.iter().find(|(m, _)| *m == id) {
            Some((_, index)) => *index,
            None => {
                let Some(model) = model_of(id) else { continue };
                set.models.push(model);
                used.push((id, set.models.len() - 1));
                set.models.len() - 1
            }
        };
        set.bodies.push(Body {
            model: index,
            position: draw.frame.position,
            yaw: draw.frame.rotation[0],
            pitch: draw.frame.rotation[1],
            highlight: draw.highlight,
            sphere: draw.sphere,
        });
    }
    set
}

/// Publishes the frame's bodies to the frame graph: `draws` with models
/// from `catalogue` (the host list's order, `look::body::Bodies::new`).
/// Starts the load of a model not yet read.
pub fn publish(draws: &[Draw], catalogue: &[Entry]) {
    let set = build_set(draws, |id| {
        let entry = catalogue.get(id as usize)?;
        asset(entry).map(|a| Arc::clone(&a.body))
    });
    native_render::bodies::publish(Some(set));
}

/// No bodies this frame.
pub fn clear() {
    native_render::bodies::publish(None);
}

/// The renderer of the page's picture, made on the first device that draws.
#[derive(Default)]
struct Renderer {
    showcase: Option<(wgpu::Device, Showcase)>,
    /// The mesh uploaded, by address.
    mesh: Option<usize>,
}

static RENDERER: LazyLock<Arc<Mutex<Renderer>>> = LazyLock::new(Arc::default);

/// The monitor's picture, drawn into the UI's colour target at its box.
struct Pass {
    asset: Arc<Asset>,
    bounds: [u16; 4],
    highlight: [u8; 4],
    seconds: f64,
    renderer: Arc<Mutex<Renderer>>,
}

impl native_render::Stage for Pass {
    fn name(&self) -> &'static str {
        "monitor model"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &native_render::program::ViewInputs) -> native_render::StageWork {
        let renderer = Arc::clone(&self.renderer);
        let mut r = renderer.lock().unwrap_or_else(|e| e.into_inner());
        let r = &mut *r;
        if r.showcase.as_ref().is_none_or(|(d, _)| *d != t.device) {
            r.showcase = Some((t.device.clone(), Showcase::new(&t.device, &t.queue)));
            r.mesh = None;
        }
        let (_, showcase) = r.showcase.as_mut().expect("made above");
        let key = Arc::as_ptr(&self.asset) as usize;
        if r.mesh != Some(key) {
            showcase.set_mesh(&t.device, &t.queue, &self.asset.mesh, &self.asset.body.bases);
            r.mesh = Some(key);
        }
        let guest = view.guest_size;
        let [x, y, w, h] = super::canvas_rect(self.bounds, guest);
        let sx = t.size.0 as f32 / guest.0.max(1) as f32;
        let sy = t.size.1 as f32 / guest.1.max(1) as f32;
        let Some(fit) = Fit::of([x * sx, y * sy, w * sx, h * sy]) else {
            unimplemented!("monitor model: an empty picture box");
        };
        let look = showcase::Look {
            // The monitor turns slowly, a full turn in the sway's period
            // times four, so a viewer sees all of it.
            yaw: (self.seconds / (showcase::SWAY_SECONDS / 4.0)).fract() as f32 * std::f32::consts::TAU,
            tint: super::showcase_tint(self.highlight),
            silhouette: false,
            glyph: false,
        };
        let (device, color, color_format) = (t.device.clone(), t.color.clone(), t.color_format);
        crate::native_targets_glue::encoded(move |encoder| {
            let mut r = renderer.lock().unwrap_or_else(|e| e.into_inner());
            let Some((_, showcase)) = r.showcase.as_mut() else { return };
            if showcase.draw(&device, encoder, fit.width, fit.height, &look) {
                showcase.composite(&device, encoder, &color, color_format, fit.rect, 1.0);
            }
        })
    }
}

/// The seconds since the page first showed a monitor: the turn's clock.
static SINCE: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// The name of the screen that is up, when it can be read; none yet.
fn active_screen(_mem: &dyn GuestMemory) -> Option<String> {
    None
}

/// A model widget's view is read (`render_bridge::native_hud::Host::
/// model_view`), with its `bounds` (`left, top, right, bottom` canvas
/// pixels): when the profile's saved look is a monitor this machine has
/// loaded, queue its picture in the widget's right share, beside the
/// Spartan the game draws. Any other look, or none, adds nothing.
pub fn appearance_view(
    mem: &mut dyn GuestMemory,
    bounds: [u16; 4],
    view: &frame_scene::OverlayModelView,
) {
    // Only the Appearance page's widget: any other screen's model widget
    // (the lobby's players, the profile preview) draws nothing. No reader
    // of the active screen's name exists on the render path yet, so none is
    // read and the picture draws nowhere until it is
    // (`super::APPEARANCE_SCREEN`).
    if !super::appearance_screen(active_screen(&*mem).as_deref()) {
        return;
    }
    let Some(beside) = super::beside_bounds(bounds) else {
        return;
    };
    let seconds = {
        let mut since = SINCE.lock().unwrap_or_else(|e| e.into_inner());
        since.get_or_insert_with(std::time::Instant::now).elapsed().as_secs_f32()
    };
    // The widget as the page's camera sees it, with the first object's
    // sphere for the biped's: `forge::look::appearance::Widget`.
    let read = crate::native_world_glue::ReadMemory(&*mem);
    let Some(frame) = view
        .objects
        .first()
        .and_then(|&h| native_render::objects::read_object(&read, h))
    else {
        return;
    };
    let widget = forge::look::appearance::Widget {
        camera: view.camera.position,
        forward: view.camera.forward,
        up: view.camera.up,
        biped: frame.sphere.0,
        biped_radius: frame.sphere.1,
    };
    let Some(draw) = crate::forge_glue::appearance_body(&*mem, &widget, seconds) else {
        return;
    };
    let Some(model) = draw.model else { return };
    let Some(entry) = crate::forge_glue::catalogue_entry(model) else {
        return;
    };
    let Some(asset) = asset(&entry) else { return };
    let pass = Pass {
        asset,
        bounds: beside,
        highlight: draw.highlight,
        seconds: f64::from(seconds),
        renderer: Arc::clone(&RENDERER),
    };
    crate::native_targets_glue::push(&*mem, frame_types::Slot::Overlay, pass);
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge::Frame;
    use native_render::model::Model;

    fn draw(seat: u8, model: Option<u32>) -> Draw {
        Draw {
            seat,
            model,
            frame: Frame {
                position: [1., 2., 3.],
                rotation: [0.5, -0.1, 0.],
            },
            sphere: ([1., 2., 3.], 1.),
            primary: 1,
            secondary: 2,
            highlight: [9, 8, 7, 64],
            light: None,
        }
    }

    fn model() -> Arc<BodyModel> {
        Arc::new(BodyModel {
            model: Model::default(),
            bases: Vec::new(),
        })
    }

    #[test]
    fn a_loaded_model_makes_a_body_at_the_draws_pose() {
        let held = model();
        let set = build_set(&[draw(3, Some(0))], |_| Some(Arc::clone(&held)));
        assert_eq!(set.models.len(), 1);
        let [body] = set.bodies.as_slice() else {
            panic!("one body");
        };
        assert_eq!(body.model, 0);
        assert_eq!(body.position, [1., 2., 3.]);
        assert_eq!((body.yaw, body.pitch), (0.5, -0.1));
        assert_eq!(body.highlight, [9, 8, 7, 64]);
    }

    #[test]
    fn two_seats_with_one_model_share_it_and_a_marker_or_unloaded_one_adds_none() {
        let held = model();
        let draws = [
            draw(1, Some(0)),
            draw(2, None),
            draw(3, Some(0)),
            draw(4, Some(5)),
        ];
        let set = build_set(&draws, |id| (id == 0).then(|| Arc::clone(&held)));
        assert_eq!(set.models.len(), 1, "the model is held once");
        assert_eq!(set.bodies.len(), 2, "the marker and the unloaded model draw none here");
        assert!(set.bodies.iter().all(|b| b.model == 0));
    }
}
