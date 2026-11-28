//! The one UI layout rule (docs/graphics/high-resolution.md, "The game's
//! 640x480 assumptions"): the menus and the HUD place every draw with the
//! same transform from the guest's 640x480 canvas to the window. Pure
//! arithmetic, no guest reads; the hooks in `game::render_aspect` call it
//! per view and the overlay's menu runs apply its result at their pass.
//!
//! The guest frame stretches to fill the window (anamorphic 640x480 shown
//! at the window's aspect), so a uniform fit is expressed in guest window
//! units: each axis of the canvas is scaled by the stretch divided into
//! the smaller of the two stretches, about a pivot. On a 16:9 window the
//! canvas is then drawn `min(w/640, h/480)` display pixels per guest
//! pixel, centred — the retail 4:3 safe area. A HUD group is held at its
//! anchor by the one anchor rule (`canvas_fit::anchor_fit`): a corner or
//! an edge's middle on the window's real edge, so the meters hug the
//! corners of any shape of window, the middles on the window's middle,
//! while tracked elements keep their projected point.

/// The guest window the game draws its UI in.
pub const CANVAS: [f32; 2] = canvas_fit::CANVAS;

/// A draw's placement in guest window units: `x' = x * sx + ox`,
/// `y' = y * sy + oy`, before the display's raster scale.
pub type Fit = [f32; 4];

/// The identity: the canvas drawn 1:1, its edges on the window's edges.
pub const IDENTITY: Fit = [1.0, 1.0, 0.0, 0.0];

/// Where a group is held on its view ([`canvas_fit::Anchor`]).
pub use canvas_fit::Anchor;

/// The first anchor above the HUD's four corners (the crosshair's and
/// tracked points': they keep their projected location).
pub const ANCHOR_CENTRE: u32 = 4;

/// Whether `output` has the canvas's own 4:3 shape, in whole pixels: the
/// window's stretch is then uniform and nothing needs a fit.
pub fn is_canvas_shape(output: (u32, u32)) -> bool {
    canvas_fit::is_canvas_shape(output)
}

/// The window's horizontal stretch of the canvas over its vertical one
/// (above 1 on a window wider than 4:3). `None` on a canvas-shaped or
/// degenerate window.
pub fn stretch_ratio(output: (u32, u32)) -> Option<f32> {
    canvas_fit::stretch_ratio(output)
}

/// Display pixels per guest pixel of the uniform fit on `output`:
/// `min(w/640, h/480)`.
pub fn uniform_scale(output: (u32, u32)) -> f32 {
    (output.0 as f32 / CANVAS[0]).min(output.1 as f32 / CANVAS[1])
}

/// The guest canvas drawn uniformly on `output` about `pivot` (guest
/// window units): the scale is `min(w/640, h/480)` display pixels per
/// guest pixel, so the fitted canvas covers the largest centred 4:3 box
/// the window holds. `None` on a 4:3 window (the stretch is already
/// uniform) or a degenerate one: the identity needs no transform.
pub fn menu_fit(output: (u32, u32), pivot: [f32; 2]) -> Option<Fit> {
    canvas_fit::window_fit(output, pivot)
}

/// A uniform fit from a ratio between the axes' stretches, about a pivot
/// (guest window units). A ratio below 1 narrows x, above 1 shortens y,
/// so both shapes of window take the same rule. `None` when the ratio is
/// not a finite, non-unit number: the identity needs no transform.
pub fn about(ratio: f32, pivot: [f32; 2]) -> Option<Fit> {
    canvas_fit::fit_about(ratio, pivot)
}

/// One HUD group's fit on `output`, for the player's raster `view`
/// (`x, y, width, height`, guest pixels). `anchor` names the group's
/// corner id (0 to 3, clockwise from the top right) or, at
/// [`ANCHOR_CENTRE`] and up, the tracked point the game gave it
/// ([`Anchor::hud`]); none is the view's centre. [`group_fit`] with the
/// game's own anchor.
pub fn hud_fit(
    output: (u32, u32),
    view: (u32, u32, u32, u32),
    pinned: bool,
    projection_factor: f32,
    projected_points_fitted: bool,
    anchor: Option<(u32, [f32; 2])>,
) -> Option<Fit> {
    let anchor = anchor.map_or(Anchor::Centre, |(id, point)| Anchor::hud(id, point));
    group_fit(output, view, pinned, projection_factor, projected_points_fitted, anchor)
}

/// The fit of a HUD group held at `anchor` of the player's raster `view`
/// on `output`: the one anchor rule, [`canvas_fit::anchor_fit`]. A view
/// edge lands on the window's edge when `pinned` (the Xbox widescreen
/// layout) and on the safe area's edge otherwise, a middle on the
/// window's middle, and the artwork is scaled by `projection_factor`
/// about the anchor. A point pivots where the game projects it, in the
/// retail lens, so its art moves with the world unless
/// `projected_points_fitted` says the projection was already widened.
/// `None` when the identity needs no transform.
pub fn group_fit(
    output: (u32, u32),
    view: (u32, u32, u32, u32),
    pinned: bool,
    projection_factor: f32,
    projected_points_fitted: bool,
    anchor: Anchor,
) -> Option<Fit> {
    let view = [view.0 as f32, view.1 as f32, view.2 as f32, view.3 as f32];
    let safe = stretch_ratio(output).map_or(1.0, f32::recip);
    let mut fit = canvas_fit::anchor_fit(anchor, view, projection_factor, safe, pinned);
    if let Anchor::Point(point) = anchor
        && !projected_points_fitted
    {
        // The tracked point is in the retail lens. Move its centre with
        // the world; the artwork is fitted about that point.
        fit[2] += (point[0] - (view[0] + view[2] * 0.5)) * (projection_factor - 1.0);
    }
    (fit != IDENTITY).then_some(fit)
}

/// A guest-window point under `fit`.
pub fn map_point(fit: Fit, p: [f32; 2]) -> [f32; 2] {
    [p[0] * fit[0] + fit[2], p[1] * fit[1] + fit[3]]
}

/// A guest-pixel rectangle (`x, y, width, height`) under `fit`, each edge
/// mapped and rounded: the clip a fitted draw keeps its scissor in. An edge
/// on the canvas boundary (0 or the 640x480 far side) of a shrunk axis stays
/// on the display boundary, so a clip that is the whole canvas spans the
/// whole display (the same rule as `gfx::external::UiFit::map_rect`).
pub fn map_rect(fit: Fit, r: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    let axis = |a: usize, start: u32, len: u32| {
        let size = CANVAS[a] as u32;
        let edge = |e: u32| {
            if fit[a] < 1.0 && (e == 0 || e == size) { e } else { (e as f32 * fit[a] + fit[a + 2]).round() as u32 }
        };
        let lo = edge(start);
        (lo, edge(start.saturating_add(len)).saturating_sub(lo))
    };
    let (x, w) = axis(0, r.0, r.2);
    let (y, h) = axis(1, r.1, r.3);
    (x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-3, "{a} != {b}");
    }

    fn close_fit(a: Fit, b: [f32; 4]) {
        for (x, y) in a.iter().zip(b) {
            close(*x, y);
        }
    }

    /// The display pixels a guest point draws at, through the fit and the
    /// window's stretch.
    fn at(fit: Fit, output: (u32, u32), p: [f32; 2]) -> [f32; 2] {
        let m = map_point(fit, p);
        [m[0] * output.0 as f32 / 640.0, m[1] * output.1 as f32 / 480.0]
    }

    /// Every anchor of a HUD group at 4:3, 16:9, 21:9 and portrait 9:16,
    /// pinned and not: 4:3 needs no transform; elsewhere the anchor's own
    /// point lands on the window's edge (pinned), the safe area's edge
    /// (not) or the window's middle, and the artwork is uniform.
    #[test]
    fn every_hud_anchor_lands_where_retail_holds_it_at_every_aspect() {
        const VIEW: (u32, u32, u32, u32) = (0, 0, 640, 480);
        for output in [(1440, 1080), (1920, 1080), (2520, 1080), (1080, 1920)] {
            let (w, h) = (output.0 as f32, output.1 as f32);
            let unit = uniform_scale(output);
            let factor = stretch_ratio(output).map_or(1.0, f32::recip);
            for anchor in Anchor::HELD {
                let f = anchor.at().unwrap();
                for pinned in [true, false] {
                    let fit = group_fit(output, VIEW, pinned, factor, false, anchor);
                    if output == (1440, 1080) {
                        assert_eq!(fit, None, "{anchor:?}");
                        continue;
                    }
                    let fit = fit.unwrap_or(IDENTITY);
                    let home = [640.0 * f[0], 480.0 * f[1]];
                    let landed = at(fit, output, home);
                    let boxed = [(w - 640.0 * unit) / 2.0, (h - 480.0 * unit) / 2.0];
                    for (a, size) in [(0, w), (1, h)] {
                        let want = if f[a] == 0.5 || pinned {
                            size * f[a]
                        } else {
                            boxed[a] + (size - 2.0 * boxed[a]) * f[a]
                        };
                        assert!(
                            (landed[a] - want).abs() < 1e-2,
                            "{output:?} {anchor:?} pinned {pinned} axis {a}: {} != {want}",
                            landed[a]
                        );
                    }
                    let next = at(fit, output, [home[0] + 8.0, home[1] + 8.0]);
                    close(next[0] - landed[0], 8.0 * unit);
                    close(next[1] - landed[1], 8.0 * unit);
                }
            }
        }
    }

    /// The message group's centred lines (the timed line, the respawn
    /// timer: centred across the frame, 40 above its bottom) are held at
    /// the bottom's middle: on the window's centre line at 4:3, 16:9 and
    /// 21:9, where the group's own top-left anchor draws them left of it.
    #[test]
    fn a_line_centred_on_the_frame_stays_on_the_windows_centre_line() {
        const VIEW: (u32, u32, u32, u32) = (0, 0, 640, 480);
        let line = [320.0, 444.0 - 40.0];
        for output in [(1440, 1080), (1920, 1080), (2520, 1080)] {
            let (w, h) = (output.0 as f32, output.1 as f32);
            let factor = stretch_ratio(output).map_or(1.0, f32::recip);
            let centred = group_fit(output, VIEW, true, factor, false, Anchor::Bottom)
                .unwrap_or(IDENTITY);
            let xy = at(centred, output, line);
            close(xy[0], w / 2.0);
            // As far above the window's bottom as at 4:3, in display pixels.
            close(h - xy[1], (480.0 - line[1]) * uniform_scale(output));
            // Text 100 guest pixels wide keeps its width at the uniform scale.
            let right = at(centred, output, [line[0] + 50.0, line[1]]);
            close(right[0] - xy[0], 50.0 * uniform_scale(output));
            let corner = hud_fit(output, VIEW, true, factor, false, Some((1, [48.0, 36.0])))
                .unwrap_or(IDENTITY);
            let off = at(corner, output, line)[0];
            if is_canvas_shape(output) {
                close(off, w / 2.0);
            } else {
                assert!(off < w / 2.0 - 1.0, "{output:?}: {off}");
            }
        }
        // A split-screen view holds the line on its own centre and bottom.
        let half = (0, 240, 640, 240);
        let fit = group_fit((1920, 1080), half, true, 0.75, false, Anchor::Bottom).unwrap();
        close(at(fit, (1920, 1080), [320.0, 480.0])[0], 960.0);
        close(at(fit, (1920, 1080), [320.0, 480.0])[1], 1080.0);
    }

    #[test]
    fn a_4_by_3_window_needs_no_transform() {
        for output in [(640, 480), (1280, 960), (1920, 1440)] {
            assert_eq!(menu_fit(output, [320.0, 240.0]), None);
            assert_eq!(hud_fit(output, (0, 0, 640, 480), true, 1.0, false, Some((2, [0.0; 2]))), None);
        }
    }

    #[test]
    fn degenerate_windows_need_no_transform() {
        assert_eq!(menu_fit((0, 480), [320.0, 240.0]), None);
        assert_eq!(menu_fit((640, 0), [320.0, 240.0]), None);
        assert_eq!(about(0.0, [0.0; 2]), None);
        assert_eq!(about(f32::NAN, [0.0; 2]), None);
        assert_eq!(about(f32::INFINITY, [0.0; 2]), None);
    }

    #[test]
    fn the_menu_fills_the_largest_centred_4_by_3_box() {
        // 16:9: the height rules; the safe area is 1440 wide on 1920.
        let fit = menu_fit((1920, 1080), [320.0, 240.0]).unwrap();
        close_fit(fit, [0.75, 1.0, 80.0, 0.0]);
        close(at(fit, (1920, 1080), [0.0, 0.0])[0], 240.0);
        close(at(fit, (1920, 1080), [640.0, 480.0])[0], 1680.0);
        close(at(fit, (1920, 1080), [320.0, 240.0])[0], 960.0);
        // A pivot away from the centre keeps its place: a 4:3 view inside
        // a split screen fits about its own centre.
        let fit = menu_fit((1920, 1080), [160.0, 120.0]).unwrap();
        close(at(fit, (1920, 1080), [160.0, 120.0])[0], 480.0);
        // 16:10 (1680x1050): the height rules; the safe area is 1400 wide.
        let fit = menu_fit((1680, 1050), [320.0, 240.0]).unwrap();
        close_fit(fit, [5.0 / 6.0, 1.0, 320.0 / 6.0, 0.0]);
        close(at(fit, (1680, 1050), [0.0, 0.0])[0], 140.0);
        close(at(fit, (1680, 1050), [640.0, 0.0])[0], 1540.0);
        // Ultrawide 21:9 (2560x1080): the safe area is 1440 wide.
        let fit = menu_fit((2560, 1080), [320.0, 240.0]).unwrap();
        close(at(fit, (2560, 1080), [0.0, 0.0])[0], (2560.0 - 1440.0) / 2.0);
        close(at(fit, (2560, 1080), [640.0, 480.0])[0], (2560.0 + 1440.0) / 2.0);
        // Portrait-ish 4:5 (960x1200): the width rules; the fit shortens
        // y, and the box is centred top to bottom.
        let fit = menu_fit((960, 1200), [320.0, 240.0]).unwrap();
        close_fit(fit, [1.0, 0.6, 0.0, 96.0]);
        close(at(fit, (960, 1200), [320.0, 0.0])[1], 240.0);
        close(at(fit, (960, 1200), [320.0, 480.0])[1], 960.0);
    }

    #[test]
    fn a_pinned_hud_group_hugs_its_window_corner_at_physical_scale() {
        // The Xbox widescreen HUD: artwork scaled only by the lens (0.75
        // on 16:9), the bottom-left group's pivot on the view's corner.
        let fit = hud_fit((1920, 1080), (0, 0, 640, 480), true, 0.75, false, Some((2, [0.0; 2])))
            .unwrap();
        close_fit(fit, [0.75, 1.0, 0.0, 0.0]);
        // The group's own left edge stays on the window's left edge.
        close(at(fit, (1920, 1080), [0.0, 480.0])[0], 0.0);
        close(at(fit, (1920, 1080), [0.0, 480.0])[1], 1080.0);
        // The top-right group pivots on the top-right corner.
        let fit = hud_fit((1920, 1080), (0, 0, 640, 480), true, 0.75, false, Some((0, [0.0; 2])))
            .unwrap();
        close(at(fit, (1920, 1080), [640.0, 0.0])[0], 1920.0);
        close(at(fit, (1920, 1080), [640.0, 0.0])[1], 0.0);
        // Pinned works in a split-screen view too: the pivot is that
        // view's own corner.
        let fit = hud_fit((1920, 1080), (0, 240, 640, 240), true, 0.75, false, Some((2, [0.0; 2])))
            .unwrap();
        close(at(fit, (1920, 1080), [0.0, 480.0])[1], 1080.0);
    }

    #[test]
    fn a_centred_hud_group_keeps_the_safe_area_edge() {
        // Not pinned: the corner group's edge lands on the safe area's
        // edge, where the retail 4:3 screen had it. The bottom-left
        // group's left edge sits at the box's left, its bottom on the
        // box's bottom.
        let fit = hud_fit((1920, 1080), (0, 0, 640, 480), false, 0.75, false, Some((2, [0.0; 2])))
            .unwrap();
        close(at(fit, (1920, 1080), [0.0, 480.0])[0], 240.0);
        close(at(fit, (1920, 1080), [0.0, 480.0])[1], 1080.0);
        // The artwork still scales only by the lens.
        close(fit[0], 0.75);
        close(fit[1], 1.0);
    }

    #[test]
    fn centre_and_tracked_groups_keep_their_projected_point() {
        // The crosshair: the artwork fits about the point the game
        // projected, and the point itself moves with the retail lens: the
        // widened lens draws guest 400 at 320*3 + 80*0.75*3 = 1140.
        let fit = hud_fit(
            (1920, 1080),
            (0, 0, 640, 480),
            true,
            0.75,
            false,
            Some((ANCHOR_CENTRE, [400.0, 200.0])),
        )
        .unwrap();
        close(at(fit, (1920, 1080), [400.0, 200.0])[0], 1140.0);
        close(at(fit, (1920, 1080), [400.0, 200.0])[1], 450.0);
        // The art's pivot was displaced with the world.
        close(fit[2], 100.0 + (400.0 - 320.0) * (0.75 - 1.0));
        // When the projection was already widened, the point stands.
        let fitted = hud_fit(
            (1920, 1080),
            (0, 0, 640, 480),
            true,
            0.75,
            true,
            Some((5, [400.0, 200.0])),
        )
        .unwrap();
        close(fitted[2], 100.0);
        // A point the game did not give keeps the view's centre.
        let centre = hud_fit(
            (1920, 1080),
            (0, 0, 640, 480),
            true,
            0.75,
            false,
            Some((6, [f32::NAN, 0.0])),
        )
        .unwrap();
        close(centre[2], 80.0);
    }

    #[test]
    fn a_narrow_window_shortens_the_hud_artwork_in_y() {
        // Portrait-ish: the lens correction (the stretch's ratio, 5/3)
        // shortens y instead of x, and the pinned pivot still sits on the
        // window's corner.
        let fit = hud_fit((960, 1200), (0, 0, 640, 480), true, 5.0 / 3.0, false, Some((2, [0.0; 2])))
            .unwrap();
        close_fit(fit, [1.0, 0.6, 0.0, 192.0]);
        close(at(fit, (960, 1200), [0.0, 480.0])[1], 1200.0);
    }

    #[test]
    fn rects_map_edge_to_edge_and_points_axis_by_axis() {
        let fit = [0.75, 1.0, 80.0, 0.0];
        assert_eq!(map_rect(fit, (0, 0, 640, 480)), (0, 0, 640, 480));
        assert_eq!(map_rect(fit, (600, 100, 40, 40)), (530, 100, 110, 40));
        assert_eq!(map_rect(IDENTITY, (10, 20, 30, 40)), (10, 20, 30, 40));
        assert_eq!(map_point(fit, [320.0, 240.0]), [320.0, 240.0]);
        assert_eq!(map_point(fit, [0.0, 480.0]), [80.0, 480.0]);
    }

    #[test]
    fn a_full_canvas_footer_clip_at_21_by_9_maps_inside_the_display() {
        // The main menu's footer clip is the canvas width: it spans the
        // display, and a clip with one interior edge keeps that edge fitted.
        let output = (2560, 1080);
        let fit = menu_fit(output, [320.0, 240.0]).unwrap();
        let footer = map_rect(fit, (0, 420, 640, 60));
        assert_eq!(footer, (0, 420, 640, 60));
        assert!(footer.0 + footer.2 <= 640 && footer.1 + footer.3 <= 480);
        let part = map_rect(fit, (0, 420, 320, 60));
        assert_eq!((part.0, part.2), (0, (320.0 * fit[0] + fit[2]).round() as u32));
    }

    /// Ultrawide windows: the menu keeps the centred 4:3 box at the
    /// window's full height, a pinned HUD group keeps its own window
    /// corner at that same scale, and the crosshair stays on the centre.
    #[test]
    fn ultrawide_windows_keep_the_uniform_scale_and_the_corners() {
        // Display pixels reach the thousands: compare to a part in ten thousand.
        let near = |a: f32, b: f32| assert!((a - b).abs() <= 1e-4 * b.abs().max(1.0), "{a} != {b}");
        for (output, box_width) in [((2560, 1080), 1440.0), ((3440, 1440), 1920.0), ((5120, 1440), 1920.0)] {
            let (w, h) = (output.0 as f32, output.1 as f32);
            near(uniform_scale(output), h / 480.0);
            near(stretch_ratio(output).unwrap(), (w / h) / (4.0 / 3.0));
            let menu = menu_fit(output, [320.0, 240.0]).unwrap();
            near(at(menu, output, [0.0, 0.0])[0], (w - box_width) / 2.0);
            near(at(menu, output, [640.0, 480.0])[0], (w + box_width) / 2.0);
            near(at(menu, output, [640.0, 480.0])[1], h);
            // The lens factor of a 4:3 guest on this window.
            let lens = stretch_ratio(output).unwrap().recip();
            let view = (0, 0, 640, 480);
            let corners = [(0, [640.0, 0.0]), (1, [0.0, 0.0]), (2, [0.0, 480.0]), (3, [640.0, 480.0])];
            for (id, corner) in corners {
                let fit = hud_fit(output, view, true, lens, true, Some((id, [0.0; 2]))).unwrap();
                // The corner stays on the window's corner.
                let [x, y] = at(fit, output, corner);
                near(x, corner[0] / 640.0 * w);
                near(y, corner[1] / 480.0 * h);
                // A 64-pixel meter is 64 uniform pixels wide and tall.
                let inward = [if corner[0] == 0.0 { 64.0 } else { 576.0 }, if corner[1] == 0.0 { 64.0 } else { 416.0 }];
                let [mx, my] = at(fit, output, inward);
                near((mx - x).abs(), 64.0 * uniform_scale(output));
                near((my - y).abs(), 64.0 * uniform_scale(output));
            }
            // The crosshair's art fits about the view's centre, in place.
            let fit = hud_fit(output, view, true, lens, true, Some((ANCHOR_CENTRE, [320.0, 240.0]))).unwrap();
            near(at(fit, output, [320.0, 240.0])[0], w / 2.0);
            near(at(fit, output, [320.0, 240.0])[1], h / 2.0);
            near(at(fit, output, [352.0, 240.0])[0] - w / 2.0, 32.0 * uniform_scale(output));
        }
    }

    #[test]
    fn a_canvas_shaped_or_degenerate_window_has_no_stretch_ratio() {
        for output in [(640, 480), (1600, 1200), (0, 480), (640, 0)] {
            assert_eq!(stretch_ratio(output), None, "{output:?}");
        }
        assert!(is_canvas_shape((1280, 960)));
        assert!(!is_canvas_shape((1280, 961)));
        close(stretch_ratio((960, 1200)).unwrap(), 0.6);
        close(uniform_scale((960, 1200)), 1.5);
    }

    /// Whatever the aspect, the fitted canvas covers the same display
    /// pixels per guest pixel on both axes: the uniform rule.
    #[test]
    fn the_menu_is_uniform_on_every_aspect() {
        for output in [
            (640, 480),
            (1920, 1080),
            (1680, 1050),
            (2560, 1080),
            (5120, 2880),
            (960, 1200),
            (1080, 1920),
        ] {
            let fit = match menu_fit(output, [320.0, 240.0]) {
                Some(fit) => fit,
                None => IDENTITY,
            };
            let x = at(fit, output, [640.0, 0.0])[0] - at(fit, output, [0.0, 0.0])[0];
            let y = at(fit, output, [0.0, 480.0])[1] - at(fit, output, [0.0, 0.0])[1];
            close(x / 640.0, y / 480.0);
            close(x / 640.0, output.0.min(output.1 * 4 / 3) as f32 / 640.0);
            // Centred: equal margins, where the window has them to give.
            let (left, right) = (
                at(fit, output, [0.0, 0.0])[0],
                output.0 as f32 - at(fit, output, [640.0, 0.0])[0],
            );
            close(left, right);
            let (top, bottom) = (
                at(fit, output, [0.0, 0.0])[1],
                output.1 as f32 - at(fit, output, [0.0, 480.0])[1],
            );
            close(top, bottom);
        }
    }
}
