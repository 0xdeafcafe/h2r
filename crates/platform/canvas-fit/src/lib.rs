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

    /// The same picture as a horizontal factor alone: a pipeline that only
    /// carries `x` narrows the horizontal lens by `y`'s inverse instead of
    /// growing the vertical one.
    pub fn horizontal_only(self) -> f32 {
        self.x / self.y
    }
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
                close(lens.horizontal_only(), want.x / want.y);
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

    #[test]
    fn degenerate_ratios_and_windows_need_no_transform() {
        assert_eq!(window_fit((0, 480), [0.0; 2]), None);
        assert_eq!(Lens::for_window((640, 0)), Lens::RETAIL);
        for ratio in [0.0, -1.0, 1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(fit_about(ratio, [0.0; 2]), None);
        }
    }
}
