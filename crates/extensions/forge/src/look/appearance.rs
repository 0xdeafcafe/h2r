//! The game's own Appearance page (docs/game/forge-monitor-appearance.md,
//! ML4b): when the player's saved look is a Forge monitor model this
//! machine draws, the page's model preview shows the monitor beside the
//! Spartan.
//!
//! Like [`super::body`], this is pure: it reads a look, the widget's view
//! and the render clock, never guest memory, and nothing here is read by
//! the tick. The marker, a model the catalogue lacks and no look at all
//! add nothing, so the page's draws are retail's.
use super::body::{Bodies, Draw, Pose};
use super::census::Entry;
use super::saved::{self, Saved};

/// The model widget's view as the page draws it: its camera and the
/// biped it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Widget {
    /// The widget camera's position and its unit forward and up axes.
    pub camera: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
    /// The biped's origin and the radius of a sphere around it.
    pub biped: [f32; 3],
    pub biped_radius: f32,
}

/// The radius the body is placed by: the sphere [`Bodies::draw`] gives
/// every body.
pub const BODY_RADIUS: f32 = 1.0;

/// The space between the biped's sphere and the body's, in world units.
pub const GAP: f32 = 0.25;

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (length.is_finite() && length > 1e-6).then(|| v.map(|c| c / length))
}

/// Where the body stands in `widget`: on the camera's right of the biped,
/// clear of it by [`GAP`], turned to face the camera, level. `None` when
/// the camera's axes give no right (parallel or zero).
pub fn pose(widget: &Widget) -> Option<Pose> {
    let right = unit(cross(widget.forward, widget.up))?;
    let away = widget.biped_radius.max(0.) + BODY_RADIUS + GAP;
    let position = [0, 1, 2].map(|i| widget.biped[i] + right[i] * away);
    let to_camera = [0, 1].map(|i| widget.camera[i] - position[i]);
    Some(Pose {
        position,
        yaw: to_camera[1].atan2(to_camera[0]),
        pitch: 0.,
    })
}

/// The monitor the Appearance page adds beside the biped: the saved look
/// as this machine can use it (`saved::resolve`), drawn at [`pose`] with
/// its colours, tint and light. `None` — the page as retail draws it —
/// for no saved look, the marker, a model or variant the catalogue lacks,
/// or a widget with no right axis.
pub fn beside(
    widget: &Widget,
    saved: Option<&Saved>,
    catalogue: &[Entry],
    seat: u8,
    seconds: f32,
    rgb: impl Fn(u8) -> [u8; 3],
) -> Option<Draw> {
    let look = saved::resolve(saved?, catalogue).look;
    if look.is_marker() {
        return None;
    }
    let draw = Bodies::new(catalogue).draw(seat, &look, pose(widget)?, seconds, rgb);
    draw.model.is_some().then_some(draw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::{MARKER, MonitorLook, VERSION};

    fn entry(key: &str) -> Entry {
        Entry {
            key: key.into(),
            name: "Invented".into(),
            source: "invented_map".into(),
            sha256: "a".repeat(64),
            variants: vec!["default".into()],
            change_colours: true,
            light: true,
            nodes: 3,
            triangles: 12,
        }
    }

    fn saved(model: &str) -> Saved {
        Saved {
            look: MonitorLook {
                version: VERSION,
                model: model.into(),
                variant: None,
                primary: 3,
                secondary: 5,
                tint: 1,
                light: 2,
                light_colour: 7,
            },
            source: (model != MARKER).then(|| "a".repeat(64)),
        }
    }

    /// A page camera 4 units in front of the biped, looking back at it
    /// along -x, z up: the camera's right is +y.
    fn widget() -> Widget {
        Widget {
            camera: [4., 0., 1.],
            forward: [-1., 0., 0.],
            up: [0., 0., 1.],
            biped: [0., 0., 0.],
            biped_radius: 0.5,
        }
    }

    fn rgb(index: u8) -> [u8; 3] {
        [index, 2 * index, 3 * index]
    }

    #[test]
    fn a_monitor_look_adds_one_body_beside_the_biped_in_its_colours() {
        let catalogue = [entry("monitor"), entry("sentinel")];
        let draw = beside(&widget(), Some(&saved("sentinel")), &catalogue, 0, 0., rgb)
            .expect("a drawn monitor");
        assert_eq!(draw.model, Some(1));
        // On the camera's right, clear of the biped's sphere.
        let away = 0.5 + BODY_RADIUS + GAP;
        assert_eq!(draw.frame.position, [0., away, 0.]);
        // Facing the camera, level.
        let yaw = (-away).atan2(4.);
        assert_eq!(draw.frame.rotation, [yaw, 0., 0.]);
        assert_eq!((draw.primary, draw.secondary), (3, 5));
        assert_eq!(draw.highlight, [3, 6, 9, 64]);
        let light = draw.light.expect("the look's light");
        assert_eq!((light.colour, light.radius), (7, 4.));
    }

    #[test]
    fn any_other_look_or_none_leaves_the_page_as_retail() {
        let catalogue = [entry("sentinel")];
        let w = widget();
        assert_eq!(beside(&w, None, &catalogue, 0, 0., rgb), None);
        assert_eq!(beside(&w, Some(&saved(MARKER)), &catalogue, 0, 0., rgb), None);
        // A model this install lacks, or from another source map.
        assert_eq!(beside(&w, Some(&saved("monitor")), &catalogue, 0, 0., rgb), None);
        let mut other = saved("sentinel");
        other.source = Some("b".repeat(64));
        assert_eq!(beside(&w, Some(&other), &catalogue, 0, 0., rgb), None);
        // A camera with no right axis.
        let mut flat = w;
        flat.up = flat.forward;
        assert_eq!(beside(&flat, Some(&saved("sentinel")), &catalogue, 0, 0., rgb), None);
    }

    /// The body is derived from its inputs alone: the same inputs give
    /// the same body, and the saved look and catalogue are untouched, so
    /// nothing the tick reads can differ with or without the page's body.
    #[test]
    fn the_body_is_a_function_of_its_inputs() {
        let catalogue = [entry("sentinel")];
        let look = saved("sentinel");
        let (before, cat_before) = (look.clone(), catalogue.clone());
        let a = beside(&widget(), Some(&look), &catalogue, 2, 1.5, rgb);
        let b = beside(&widget(), Some(&look), &catalogue, 2, 1.5, rgb);
        assert_eq!(a, b);
        assert_eq!((look, catalogue), (before, cat_before));
        assert_eq!(a.unwrap().seat, 2);
    }
}
