//! The presentation body a machine draws for a monitor (docs/game/
//! forge-monitor-appearance.md, ML4): the host model list — census entries
//! as models a renderer holds, named by census entry and never by a tag
//! datum — and the one instance a seat draws at a pose.
//!
//! Nothing here reads guest memory, a GPU or the disc, and nothing here is
//! read by the tick: a body is derived only from a look, a pose and the
//! render clock, so a machine that draws it differently, or not at all,
//! runs the same game. The glue hands the [`Draw`]s to the native
//! renderer's object path and paints the markers (the looks this machine
//! draws no model for) into the HUD canvas.
use serde::{Deserialize, Serialize};

use super::census::Entry;
use super::{MonitorLook, PALETTE, TINT_MAX};

/// The host model list: the census entries of the catalogue a session
/// starts with, each one a model the renderer holds under the id it names
/// here. A model is identified by its census entry — the key and the
/// source map's hash — never by a tag datum, so a model read from another
/// map file sits beside the loaded map's own as an equal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bodies {
    models: Vec<Model>,
}

/// One model of the host list.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// The id the list names it by: its index.
    pub id: u32,
    /// The census entry it was made for.
    pub entry: Entry,
}

/// A monitor's pose: the camera's position, yaw and pitch
/// (`forge::camera::Camera`'s shape).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// How far the body rises and sinks, in world units, and how many seconds
/// a rise-and-sink takes: the slow vertical bob the render clock drives.
pub const BOB_HEIGHT: f32 = 0.08;
pub const BOB_SECONDS: f32 = 3.0;

/// The body's offset at `seconds` of the render clock.
pub fn bob(seconds: f32) -> f32 {
    BOB_HEIGHT * (std::f32::consts::TAU * seconds / BOB_SECONDS).sin()
}

/// The alpha of each tint step: the lit colour is mixed towards primary
/// by this much (`ObjectInput::highlight`'s alpha), a fixed step apart.
fn tint_alpha(tint: u8) -> u8 {
    [0, 64, 128, 192][usize::from(tint.min(TINT_MAX))]
}

/// The light a body's origin carries: the palette colour, one of two
/// fixed radii, no shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    /// A palette index; the RGB is read from the loaded map.
    pub colour: u8,
    pub radius: f32,
}

/// One seat's drawn body at a moment: what the renderer draws for it.
/// `model: None` is a marker — the compositor's ring, not an instance.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    /// The seat whose body this is. The view of the seat's own machine
    /// never draws it: the camera is inside it.
    pub seat: u8,
    /// The host model list's id of the model drawn, or `None` for the
    /// marker.
    pub model: Option<u32>,
    /// Where the body sits and how it is turned: the pose with the bob
    /// applied, in the model's rest pose.
    pub frame: crate::Frame,
    /// A sphere around the body, for culling.
    pub sphere: ([f32; 3], f32),
    /// The change colours, as palette indices: the RGB is read from the
    /// loaded map where the model has a change-colour map.
    pub primary: u8,
    pub secondary: u8,
    /// The highlight mix: the primary colour's RGB with the tint step's
    /// alpha, `[0; 4]` when there is no tint (`ObjectInput::highlight`).
    pub highlight: [u8; 4],
    /// The light at the body's origin, when the look has one.
    pub light: Option<Light>,
}

impl Bodies {
    /// The host list of a catalogue, in its order: one model an entry.
    pub fn new(catalogue: &[Entry]) -> Bodies {
        Bodies {
            models: catalogue
                .iter()
                .enumerate()
                .map(|(id, entry)| Model {
                    id: id as u32,
                    entry: entry.clone(),
                })
                .collect(),
        }
    }

    pub fn models(&self) -> &[Model] {
        &self.models
    }

    /// The id of the model a look names, in this machine's list: the
    /// marker names none.
    pub fn find(&self, look: &MonitorLook) -> Option<u32> {
        if look.is_marker() {
            return None;
        }
        self.models
            .iter()
            .find(|m| m.entry.key == look.model)
            .map(|m| m.id)
    }

    /// One seat's drawn body: the look's model in this machine's list, at
    /// the pose with the bob of `seconds` applied, in the look's colours.
    /// A look this machine draws no model for (the marker, a model the
    /// catalogue lacks) draws the marker: `model: None`.
    pub fn draw(
        &self,
        seat: u8,
        look: &MonitorLook,
        pose: Pose,
        seconds: f32,
        rgb: impl Fn(u8) -> [u8; 3],
    ) -> Draw {
        let model = self.find(look);
        let position = [pose.position[0], pose.position[1], pose.position[2] + bob(seconds)];
        // The body sits at the camera and turns to its yaw and pitch; the
        // model's rest pose is the renderer's.
        let frame = crate::Frame {
            position,
            rotation: [pose.yaw, pose.pitch, 0.],
        };
        let [r, g, b] = rgb(look.primary.min(PALETTE - 1));
        let highlight = (look.tint > 0).then(|| [r, g, b, tint_alpha(look.tint)]);
        Draw {
            seat,
            model,
            frame,
            // The sphere a biped-sized body covers, whatever the model:
            // a renderer culls by it, never draws by it.
            sphere: (position, 1.),
            primary: look.primary,
            secondary: look.secondary,
            highlight: highlight.unwrap_or([0; 4]),
            light: (look.light > 0).then(|| Light {
                colour: look.light_colour,
                radius: if look.light == 2 { 4. } else { 2. },
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::{MARKER, VERSION};

    fn entry(key: &str, colours: bool) -> Entry {
        Entry {
            key: key.into(),
            name: "Sentinel".into(),
            source: "05a_deltaapproach".into(),
            sha256: "abc123".into(),
            variants: vec!["default".into(), "major".into()],
            change_colours: colours,
            light: true,
            nodes: 20,
            triangles: 900,
        }
    }

    fn look(model: &str) -> MonitorLook {
        MonitorLook {
            version: VERSION,
            model: model.into(),
            variant: None,
            primary: 3,
            secondary: 5,
            tint: 2,
            light: 1,
            light_colour: 7,
        }
    }

    fn pose() -> Pose {
        Pose {
            position: [1., 2., 3.],
            yaw: 0.5,
            pitch: -0.1,
        }
    }

    fn rgb(index: u8) -> [u8; 3] {
        [index, 10 * index, 100 * index]
    }

    #[test]
    fn the_list_names_one_model_a_census_entry() {
        let bodies = Bodies::new(&[entry("sentinel", true), entry("dervish", false)]);
        assert_eq!(bodies.models().len(), 2);
        assert_eq!(bodies.models()[1].id, 1);
        assert_eq!(bodies.models()[1].entry.key, "dervish");
        assert_eq!(bodies.find(&look("dervish")), Some(1));
        assert_eq!(bodies.find(&look("sentinel")), Some(0));
        // A model the catalogue lacks, and the marker, name none.
        assert_eq!(bodies.find(&look("monitor")), None);
        assert_eq!(bodies.find(&look(MARKER)), None);
    }

    #[test]
    fn a_seat_draws_one_instance_at_the_pose_with_the_looks_colours() {
        let bodies = Bodies::new(&[entry("sentinel", true)]);
        let draw = bodies.draw(4, &look("sentinel"), pose(), 0., rgb);
        assert_eq!(draw.seat, 4);
        assert_eq!(draw.model, Some(0), "the only model in the list");
        assert_eq!(draw.frame.position, [1., 2., 3.], "no bob at zero");
        assert_eq!(draw.frame.rotation, [0.5, -0.1, 0.]);
        assert_eq!(draw.sphere.0, [1., 2., 3.]);
        assert_eq!((draw.primary, draw.secondary), (3, 5));
        // The highlight mixes towards primary by the tint step's alpha.
        assert_eq!(draw.highlight, [3, 30, 97, 128]);
        assert_eq!(
            draw.light,
            Some(Light {
                colour: 7,
                radius: 2.,
            })
        );
        // The bob of the render clock moves the body and nothing else.
        let up = bodies.draw(4, &look("sentinel"), pose(), BOB_SECONDS / 4., rgb);
        assert!((up.frame.position[2] - 3. - BOB_HEIGHT).abs() < 1e-4);
        assert_eq!(up.sphere.0, up.frame.position);
        // A bright light takes the larger radius.
        let mut bright = look("sentinel");
        bright.light = 2;
        assert_eq!(bodies.draw(4, &bright, pose(), 0., rgb).light.unwrap().radius, 4.);
    }

    #[test]
    fn a_marker_look_draws_no_instance_and_no_light() {
        let bodies = Bodies::new(&[entry("sentinel", true)]);
        let mut marker = MonitorLook::default_for(9);
        marker.light = 0;
        let draw = bodies.draw(1, &marker, pose(), 0., rgb);
        assert_eq!(draw.model, None, "the compositor's ring draws instead");
        assert_eq!(draw.highlight, [0; 4]);
        assert_eq!(draw.light, None);
        assert_eq!(draw.primary, 9, "the ring's colour is the look's");
    }

    #[test]
    fn a_model_the_catalogue_lacks_draws_the_marker() {
        let bodies = Bodies::new(&[entry("sentinel", true)]);
        let draw = bodies.draw(2, &look("dervish"), pose(), 0., rgb);
        assert_eq!(draw.model, None);
        assert_eq!(draw.primary, 3, "the marker keeps the look's colours");
    }

    #[test]
    fn a_body_is_derived_from_nothing_but_the_look_the_pose_and_the_clock() {
        // The rule the design holds: nothing a body is made of is read by
        // the tick or written to guest memory, so the same inputs draw the
        // same body on every machine and a machine that draws none runs
        // the same game.
        let bodies = Bodies::new(&[entry("sentinel", true)]);
        let a = bodies.draw(0, &look("sentinel"), pose(), 12.5, rgb);
        let b = bodies.draw(0, &look("sentinel"), pose(), 12.5, rgb);
        assert_eq!(a, b);
    }
}
