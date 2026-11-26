//! The Forge monitors' disc models, drawn (docs/game/
//! forge-monitor-appearance.md, ML4 and ML4b): a census entry's biped model
//! read once from its campaign map in the mounted game, in the background,
//! then drawn two ways with the native renderer on. In the world, each
//! other seat's body is a stage of the frame graph
//! (`native_render::bodies`, [`gpu::publish`]). On the game's Appearance
//! page, a monitor look is a picture beside the Spartan in the page's model
//! widget (`gpu::appearance_view`, the Skulls page's showcase).
//!
//! Without the native renderer, or until a model has loaded, or for a model
//! this install cannot read, the seat draws the marker as before. Nothing
//! here is read by the tick.
//!
//! This file holds the arithmetic with no GPU in it.

#[cfg(feature = "gpu")]
pub mod gpu;

/// The screen tag the Appearance page is built on. Not established
/// statically: none of the repository's screen docs or samples name it, so
/// it is `None` and the page's picture draws on no screen until the name is
/// read from a running game (`h2 agent ui.stack` with the page open; see
/// docs/game/forge-monitor-appearance.md, "Not verified").
pub const APPEARANCE_SCREEN: Option<&str> = None;

/// Whether `screen`, the name of the screen that is up, is the one named
/// `want`. Fails closed: no wanted name or no screen read is no.
pub fn is_screen(want: Option<&str>, screen: Option<&str>) -> bool {
    matches!((want, screen), (Some(w), Some(s)) if w.eq_ignore_ascii_case(s))
}

/// Whether the screen that is up is the Appearance page.
pub fn appearance_screen(screen: Option<&str>) -> bool {
    is_screen(APPEARANCE_SCREEN, screen)
}

/// The model widget's canvas: the UI's own 640 by 480 guest pixels.
pub const CANVAS: (f32, f32) = (640.0, 480.0);

/// The share of the page's model widget the monitor's picture takes, from
/// its right edge: the Spartan keeps the rest.
pub const BESIDE_SHARE: f32 = 0.4;

/// The widget's `bounds` (`left, top, right, bottom` in canvas pixels) as
/// `x, y, width, height` of a `guest` sized target: the canvas's height
/// fills the target's and it is centred across, as the UI is
/// (`skulls_model::guest_rect`).
pub fn canvas_rect(bounds: [u16; 4], guest: (u32, u32)) -> [f32; 4] {
    let [left, top, right, bottom] = bounds.map(|v| f32::from(v as i16));
    let scale = guest.1 as f32 / CANVAS.1;
    let across = (guest.0 as f32 - CANVAS.0 * scale) / 2.0;
    [
        across + left * scale,
        top * scale,
        (right - left) * scale,
        (bottom - top) * scale,
    ]
}

/// The part of a widget's `bounds` the monitor's picture fills: the
/// right [`BESIDE_SHARE`] of it, full height. Nothing for an empty widget.
pub fn beside_bounds(bounds: [u16; 4]) -> Option<[u16; 4]> {
    let [left, top, right, bottom] = bounds.map(|v| i32::from(v as i16));
    if right <= left || bottom <= top {
        return None;
    }
    let width = (right - left) as f32;
    let start = right - (width * BESIDE_SHARE).round() as i32;
    Some([start, top, right, bottom].map(|v| v as u16))
}

/// The colour a showcase picture's base map is multiplied by for a body's
/// highlight (`look::body::Draw::highlight`: the primary colour with the
/// tint step's alpha): white where there is no tint, the colour in full at
/// full alpha.
pub fn showcase_tint(highlight: [u8; 4]) -> [f32; 3] {
    let mix = f32::from(highlight[3]) / 255.0;
    [0, 1, 2].map(|i| 1.0 + (f32::from(highlight[i]) / 255.0 - 1.0) * mix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_screens_model_widget_draws_nothing() {
        // The page's own screen matches; any other, none read, or no wanted
        // name does not.
        assert!(is_screen(Some("appearance"), Some("Appearance")));
        assert!(!is_screen(Some("appearance"), Some("lobby_players")));
        assert!(!is_screen(Some("appearance"), None));
        assert!(!is_screen(None, Some("appearance")));
        // Until the name is established nothing matches.
        assert!(!appearance_screen(Some("appearance")));
        assert!(!appearance_screen(None));
    }

    #[test]
    fn the_canvas_fills_the_targets_height_and_is_centred_across() {
        // A 4:3 target: the canvas is the target.
        assert_eq!(canvas_rect([100, 50, 300, 250], (640, 480)), [100., 50., 200., 200.]);
        // A 16:9 target of twice the height: scaled by 2, then centred.
        let wide = canvas_rect([100, 50, 300, 250], (1706, 960));
        let across = (1706. - 1280.) / 2.;
        assert_eq!(wide, [across + 200., 100., 400., 400.]);
    }

    #[test]
    fn the_monitors_picture_is_the_widgets_right_share() {
        // left, top, right, bottom
        assert_eq!(beside_bounds([100, 20, 300, 220]), Some([220, 20, 300, 220]));
        assert_eq!(beside_bounds([100, 20, 100, 220]), None);
        assert_eq!(beside_bounds([100, 220, 300, 20]), None);
    }

    #[test]
    fn an_untinted_body_shows_its_own_colours_and_a_tinted_one_leans_to_primary() {
        assert_eq!(showcase_tint([0; 4]), [1., 1., 1.]);
        assert_eq!(showcase_tint([255, 0, 0, 255]), [1., 0., 0.]);
        let half = showcase_tint([0, 255, 0, 128]);
        assert!((half[0] - (1. - 128. / 255.)).abs() < 1e-5);
        assert_eq!(half[1], 1.);
    }
}
