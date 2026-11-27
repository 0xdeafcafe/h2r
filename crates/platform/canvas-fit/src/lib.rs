//! The one widescreen rule (docs/graphics/high-resolution.md, "Window
//! shapes"). The guest draws a 640x480 canvas; the window stretches it to
//! its own shape. Pure arithmetic with no dependencies, so the device
//! (`d3d::backend::UiFit`, `ScaledBackend::anamorphic_guest_frame`) and the
//! hooks (`render_bridge::ui_layout`) share it.
//!
//! The rule depends on the window's aspect alone. The render scale changes
//! how many raster pixels fill the window, not their shape, so every
//! function here gives the same answer for `(w, h)` and `(w*k, h*k)`.

/// The guest window the game draws in.
pub const CANVAS: [f32; 2] = [640.0, 480.0];

/// Whether `output` has the canvas's own 4:3 shape, in whole pixels.
pub fn is_canvas_shape(output: (u32, u32)) -> bool {
    u64::from(output.0) * 3 == u64::from(output.1) * 4
}

/// The window's horizontal stretch of the canvas over its vertical one:
/// above 1 on a window wider than 4:3, below 1 on a narrower one. `None` on
/// a canvas-shaped or degenerate window.
pub fn stretch_ratio(output: (u32, u32)) -> Option<f32> {
    let (w, h) = output;
    (w != 0 && h != 0 && !is_canvas_shape(output))
        .then(|| (w as f32 / CANVAS[0]) / (h as f32 / CANVAS[1]))
}

/// Factors on the guest projection's clip terms that give square pixels on
/// the window. A window wider than 4:3 grows the horizontal lens (`x` below
/// 1, `y` 1); a narrower one grows the vertical lens (`x` 1, `y` below 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lens {
    /// Factor on the horizontal clip term.
    pub x: f32,
    /// Factor on the vertical clip term.
    pub y: f32,
}

impl Lens {
    /// The retail 4:3 lens.
    pub const RETAIL: Self = Self { x: 1.0, y: 1.0 };

    /// The lens for a window of `output`'s aspect, whatever its raster size.
    pub fn for_window(output: (u32, u32)) -> Self {
        match stretch_ratio(output) {
            Some(s) if s > 1.0 => Self { x: s.recip(), y: 1.0 },
            Some(s) => Self { x: 1.0, y: s },
            None => Self::RETAIL,
        }
    }

    /// The lens as the one scalar the projection pipeline carries (the
    /// projection factor): below 1 it scales the horizontal terms, above 1
    /// it scales the vertical terms by its inverse.
    pub fn factor(self) -> f32 {
        self.x / self.y
    }

    /// The lens a projection factor stands for ([`Lens::factor`]'s inverse).
    pub fn from_factor(factor: f32) -> Self {
        Self { x: factor.min(1.0), y: factor.recip().min(1.0) }
    }
}

/// The factor on the horizontal terms of a projection factor: below 1 only.
pub fn horizontal_term(factor: f32) -> f32 {
    Lens::from_factor(factor).x
}

/// The factor on the vertical terms of a projection factor: below 1 when
/// the projection factor is above 1.
pub fn vertical_term(factor: f32) -> f32 {
    Lens::from_factor(factor).y
}

/// A uniform fit from a ratio between the axes' stretches, about a pivot
/// (guest window units): `[sx, sy, ox, oy]` with `x' = x*sx + ox`,
/// `y' = y*sy + oy`. A ratio below 1 narrows x, above 1 shortens y. `None`
/// when the ratio is not a finite, non-unit number: the identity needs no
/// transform.
pub fn fit_about(ratio: f32, pivot: [f32; 2]) -> Option<[f32; 4]> {
    if !ratio.is_finite() || ratio <= 0.0 || ratio == 1.0 {
        return None;
    }
    let x = ratio.min(1.0);
    let y = ratio.recip().min(1.0);
    Some([x, y, pivot[0] * (1.0 - x), pivot[1] * (1.0 - y)])
}

/// The canvas fitted uniformly on `output` about `pivot`: the largest
/// centred 4:3 box the window holds. `None` on a 4:3 or degenerate window.
pub fn window_fit(output: (u32, u32), pivot: [f32; 2]) -> Option<[f32; 4]> {
    fit_about(stretch_ratio(output)?.recip(), pivot)
}

/// Where a group of HUD or UI draws is held on the retail canvas: the
/// point of its view the game laid it out from, and so the point that
/// keeps its place when the window is not 4:3 ([`anchor_fit`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    TopRight,
    TopLeft,
    BottomLeft,
    BottomRight,
    /// The middle of the top edge.
    Top,
    /// The middle of the bottom edge: a line centred across the frame at
    /// a height from its bottom.
    Bottom,
    Left,
    Right,
    Centre,
    /// A point the game projected (the crosshair, a tracked mark), in
    /// guest window units.
    Point([f32; 2]),
}

impl Anchor {
    /// Every anchor held on the view: the corners, the edges' middles and
    /// the centre.
    pub const HELD: [Anchor; 9] = [
        Anchor::TopRight,
        Anchor::TopLeft,
        Anchor::BottomLeft,
        Anchor::BottomRight,
        Anchor::Top,
        Anchor::Bottom,
        Anchor::Left,
        Anchor::Right,
        Anchor::Centre,
    ];

    /// The anchor of the game's HUD anchor id: 0 to 3 are the corners,
    /// clockwise from the top right; 4 and up are the point the game gave
    /// (the view's centre when it is not finite).
    pub fn hud(id: u32, point: [f32; 2]) -> Anchor {
        match id {
            0 => Anchor::TopRight,
            1 => Anchor::TopLeft,
            2 => Anchor::BottomLeft,
            3 => Anchor::BottomRight,
            _ if point.iter().all(|v| v.is_finite()) => Anchor::Point(point),
            _ => Anchor::Centre,
        }
    }

    /// Where the anchor is on its view, as fractions of the view's width
    /// and height from its top left. `None` for a point.
    pub const fn at(self) -> Option<[f32; 2]> {
        Some(match self {
            Anchor::TopRight => [1.0, 0.0],
            Anchor::TopLeft => [0.0, 0.0],
            Anchor::BottomLeft => [0.0, 1.0],
            Anchor::BottomRight => [1.0, 1.0],
            Anchor::Top => [0.5, 0.0],
            Anchor::Bottom => [0.5, 1.0],
            Anchor::Left => [0.0, 0.5],
            Anchor::Right => [1.0, 0.5],
            Anchor::Centre => [0.5, 0.5],
            Anchor::Point(_) => return None,
        })
    }
}

/// The scale a ratio between the axes' stretches gives each axis: below 1
/// narrows x, above 1 shortens y ([`fit_about`]); 1 on both for a ratio
/// that is not a finite, positive number.
fn axes(ratio: f32) -> [f32; 2] {
    if !ratio.is_finite() || ratio <= 0.0 {
        return [1.0, 1.0];
    }
    [ratio.min(1.0), ratio.recip().min(1.0)]
}

/// The one anchor rule: the fit (`[sx, sy, ox, oy]`, guest window units,
/// as [`fit_about`]) of a group of draws held at `anchor` of `view`
/// (`x, y, width, height`, the player's raster in guest pixels).
///
/// The group's artwork is scaled by `art` (the ratio [`fit_about`] takes;
/// on a window whose pixels the game does not know are wide, the window's
/// own [`stretch_ratio`] inverted, so the art is uniform) about the
/// anchor's point. That point lands, on each axis:
///
/// - at a view edge: on the window's own edge when `pinned`, else on the
///   edge of the safe area, the centred box `safe` scales the view to
///   (`stretch_ratio(output).recip()`, 1 at 4:3);
/// - at the view's middle: on the window's middle, pinned or not;
/// - a point: where it is.
///
/// So an authored distance from the anchor is the same number of display
/// pixels on both axes, at every aspect, and at 4:3 the fit is the
/// identity.
pub fn anchor_fit(anchor: Anchor, view: [f32; 4], art: f32, safe: f32, pinned: bool) -> [f32; 4] {
    let (s, q) = (axes(art), axes(safe));
    let mut fit = [s[0], s[1], 0.0, 0.0];
    for a in 0..2 {
        let centre = view[a] + view[2 + a] * 0.5;
        fit[2 + a] = match (anchor, anchor.at()) {
            (Anchor::Point(p), _) => p[a] * (1.0 - s[a]),
            (_, Some(at)) => {
                let c = view[a] + view[2 + a] * at[a];
                if pinned {
                    c * (1.0 - s[a])
                } else {
                    centre * (1.0 - s[a]) + (c - centre) * (q[a] - s[a])
                }
            }
            (_, None) => centre * (1.0 - s[a]),
        };
    }
    fit
}

/// Whether the guest's frame is shown anamorphic: its raster is still the
/// 640x480 canvas (`guest_raster`, render scale one) and the presentation
/// is not 4:3.
pub fn anamorphic_guest_frame(presentation: (u32, u32), guest_raster: bool) -> bool {
    guest_raster && stretch_ratio(presentation).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    // (name, aspect w:h as integers, base window multiple)
    const ASPECTS: [(&str, u32, u32); 6] = [
        ("4:3", 4, 3),
        ("16:9", 16, 9),
        ("21:9", 21, 9),
        ("32:9", 32, 9),
        ("3:4", 3, 4),
        ("9:16", 9, 16),
    ];

    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4, "{a} != {b}");
    }

    #[test]
    fn the_lens_follows_the_aspect_at_every_render_scale() {
        for (name, aw, ah) in ASPECTS {
            let aspect = aw as f32 / ah as f32;
            let want = match name {
                // Wider than 4:3: x grows the lens; narrower: y does.
                "4:3" => Lens::RETAIL,
                _ if aspect > 4.0 / 3.0 => Lens { x: (4.0 / 3.0) / aspect, y: 1.0 },
                _ => Lens { x: 1.0, y: aspect / (4.0 / 3.0) },
            };
            for scale in [0.5f32, 1.0, 2.0] {
                // 480 base keeps every size whole at 0.5x.
                let (w, h) = ((aw * 120) as f32 * scale, (ah * 120) as f32 * scale);
                let output = (w as u32, h as u32);
                let lens = Lens::for_window(output);
                close(lens.x, want.x);
                close(lens.y, want.y);
                // The picture is square-pixel: lens ratio equals the aspect
                // over the canvas's.
                close(lens.factor(), want.x / want.y);
                let back = Lens::from_factor(lens.factor());
                close(back.x, want.x);
                close(back.y, want.y);
                // A wide window never touches y; a narrow one never x.
                assert!(lens.x == 1.0 || lens.y == 1.0, "{name} @{scale}");
                assert_eq!(
                    anamorphic_guest_frame(output, scale == 1.0),
                    name != "4:3" && scale == 1.0
                );
            }
        }
    }

    #[test]
    fn the_uniform_fit_is_the_largest_centred_4_by_3_box_at_every_render_scale() {
        for (name, aw, ah) in ASPECTS {
            for scale in [0.5f32, 1.0, 2.0] {
                let output = (((aw * 120) as f32 * scale) as u32, ((ah * 120) as f32 * scale) as u32);
                let fit = window_fit(output, [320.0, 240.0]);
                if name == "4:3" {
                    assert_eq!(fit, None);
                    continue;
                }
                let [sx, sy, ox, oy] = fit.unwrap();
                let (w, h) = (output.0 as f32, output.1 as f32);
                // Display pixels per guest pixel on each axis.
                let (px, py) = (sx * w / 640.0, sy * h / 480.0);
                close(px, py);
                close(px, (w / 640.0).min(h / 480.0));
                // Centred: equal margins on both axes.
                let left = ox * w / 640.0;
                let right = w - (640.0 * sx + ox) * w / 640.0;
                let top = oy * h / 480.0;
                let bottom = h - (480.0 * sy + oy) * h / 480.0;
                close(left, right);
                close(top, bottom);
                // The box shrinks only the longer axis.
                assert!(sx == 1.0 || sy == 1.0, "{name} @{scale}");
            }
        }
    }

    /// The one table for every anchor: at 4:3, 16:9, 21:9 and portrait
    /// 9:16, pinned and on the safe area, a point authored `d` guest
    /// pixels inside its anchor draws `d` times the uniform scale from
    /// where the anchor lands, on both axes. The anchor lands on the
    /// window's edge (pinned), the safe area's edge (not), or the middle.
    #[test]
    fn every_anchor_keeps_its_place_and_the_uniform_scale_at_every_aspect() {
        const VIEW: [f32; 4] = [0.0, 0.0, 640.0, 480.0];
        for (name, aw, ah) in [("4:3", 4, 3), ("16:9", 16, 9), ("21:9", 21, 9), ("9:16", 9, 16)] {
            let output = (aw * 120, ah * 120);
            let (w, h) = (output.0 as f32, output.1 as f32);
            let unit = (w / 640.0).min(h / 480.0);
            // The safe area: the centred 4:3 box, in display pixels.
            let safe_box = [(w - 640.0 * unit) / 2.0, (h - 480.0 * unit) / 2.0];
            let safe = stretch_ratio(output).map_or(1.0, f32::recip);
            for anchor in Anchor::HELD {
                let at = anchor.at().unwrap();
                for pinned in [true, false] {
                    let fit = anchor_fit(anchor, VIEW, safe, safe, pinned);
                    if name == "4:3" {
                        assert_eq!(fit, [1.0, 1.0, 0.0, 0.0], "{anchor:?}");
                    }
                    let display = |p: [f32; 2]| {
                        [
                            (p[0] * fit[0] + fit[2]) * w / 640.0,
                            (p[1] * fit[1] + fit[3]) * h / 480.0,
                        ]
                    };
                    let home = [640.0 * at[0], 480.0 * at[1]];
                    let landed = display(home);
                    for (a, size) in [(0, w), (1, h)] {
                        let want = if at[a] == 0.5 {
                            size / 2.0
                        } else if pinned {
                            size * at[a]
                        } else {
                            safe_box[a] + (size - 2.0 * safe_box[a]) * at[a]
                        };
                        assert!(
                            (landed[a] - want).abs() < 1e-2,
                            "{name} {anchor:?} pinned {pinned} axis {a}: {} != {want}",
                            landed[a]
                        );
                    }
                    // 40 guest pixels in from the anchor, towards the
                    // middle (across from a middle).
                    let inward = |f: f32| if f == 1.0 { -40.0 } else { 40.0 };
                    let near = display([home[0] + inward(at[0]), home[1] + inward(at[1])]);
                    for a in 0..2 {
                        let step = (near[a] - landed[a]).abs();
                        assert!(
                            (step - 40.0 * unit).abs() < 1e-2,
                            "{name} {anchor:?} pinned {pinned} axis {a}: {step}"
                        );
                    }
                }
            }
            // A projected point keeps its place, its art at the uniform scale.
            let p = [400.0, 300.0];
            let fit = anchor_fit(Anchor::Point(p), VIEW, safe, safe, true);
            close(p[0] * fit[0] + fit[2], p[0]);
            close(p[1] * fit[1] + fit[3], p[1]);
            close(fit[0] * w / 640.0, unit);
            close(fit[1] * h / 480.0, unit);
            // The menu's fit is the centre anchor's.
            let centre = anchor_fit(Anchor::Centre, VIEW, safe, safe, false);
            assert_eq!(window_fit(output, [320.0, 240.0]).unwrap_or([1.0, 1.0, 0.0, 0.0]), centre);
        }
    }

    /// A split-screen view holds its anchors on its own edges, and the
    /// game's HUD anchor ids are the corners clockwise from the top right.
    #[test]
    fn a_view_holds_its_own_anchors_and_the_hud_ids_are_the_corners() {
        let view = [0.0, 240.0, 640.0, 240.0];
        let fit = anchor_fit(Anchor::Bottom, view, 0.75, 0.75, true);
        close(320.0 * fit[0] + fit[2], 320.0);
        close(480.0 * fit[1] + fit[3], 480.0);
        let fit = anchor_fit(Anchor::TopLeft, view, 0.75, 0.75, true);
        close(fit[2], 0.0);
        close(240.0 * fit[1] + fit[3], 240.0);
        let ids = [Anchor::TopRight, Anchor::TopLeft, Anchor::BottomLeft, Anchor::BottomRight];
        for (id, anchor) in ids.into_iter().enumerate() {
            assert_eq!(Anchor::hud(id as u32, [f32::NAN; 2]), anchor);
        }
        assert_eq!(Anchor::hud(4, [74.0, 315.0]), Anchor::Point([74.0, 315.0]));
        assert_eq!(Anchor::hud(4, [f32::NAN, 0.0]), Anchor::Centre);
    }

    #[test]
    fn degenerate_ratios_and_windows_need_no_transform() {
        assert_eq!(window_fit((0, 480), [0.0; 2]), None);
        assert_eq!(Lens::for_window((640, 0)), Lens::RETAIL);
        for ratio in [0.0, -1.0, 1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(fit_about(ratio, [0.0; 2]), None);
        }
    }
}
