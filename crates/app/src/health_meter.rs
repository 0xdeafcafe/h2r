//! A Halo 1 style health meter drawn over the game image
//! (docs/game/health.md "HUD"): a row of segments under where Halo 2 draws
//! its shield meter, for a level whose own HUD has no health meter (a
//! retail level under the `H2_HEALTH_MODEL=halo1` debug switch). A
//! converted Halo 1 map's HUD draws its own, and then this shows nothing.
//!
//! The value comes from a closure (boot's `health::overlay_meter`): `None`
//! draws nothing. Rasterised on the CPU into one RGBA image, drawn with the
//! banner's sprite pass.

use std::sync::Arc;

use crate::overlay::Sprite;
use crate::scale::Viewport;

/// Segments in the meter.
pub const SEGMENTS: u32 = 8;
/// At or below this fraction the lit segments turn red.
pub const LOW: f32 = 0.25;
/// Segment size and gap, in meter pixels (times the scale).
const SEG_W: u32 = 9;
const SEG_H: u32 = 5;
const GAP: u32 = 2;
const PAD: u32 = 2;

const LIT: [u8; 4] = [96, 168, 255, 230];
const LIT_LOW: [u8; 4] = [255, 72, 56, 235];
const UNLIT: [u8; 4] = [40, 56, 84, 140];
const PLATE: [u8; 4] = [0, 0, 0, 90];

/// Where the meter sits by default: its right edge and top as fractions of
/// the game image (top right, under Halo 2's shield meter).
pub const DEFAULT_AT: (f32, f32) = (0.955, 0.115);

/// Shared handle to the meter's value. Cloning shares it.
#[derive(Clone)]
pub struct HealthMeter {
    value: Arc<dyn Fn() -> Option<f32> + Send + Sync>,
    /// Right edge and top, fractions of the game image.
    pub at: (f32, f32),
}

impl std::fmt::Debug for HealthMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HealthMeter").field("at", &self.at).finish()
    }
}

impl HealthMeter {
    pub fn new(value: impl Fn() -> Option<f32> + Send + Sync + 'static) -> Self {
        Self {
            value: Arc::new(value),
            at: DEFAULT_AT,
        }
    }
    /// Places the meter: `right` and `top` as fractions of the game image.
    pub fn at(mut self, right: f32, top: f32) -> Self {
        self.at = (right.clamp(0.0, 1.0), top.clamp(0.0, 1.0));
        self
    }
    /// The value to draw now.
    pub fn get(&self) -> Option<f32> {
        (self.value)()
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(0.0, 1.0))
    }
}

/// `"right,top"` fractions (what `H2_HEALTH_METER_AT` holds).
pub fn parse_at(s: &str) -> Option<(f32, f32)> {
    let (a, b) = s.split_once(',')?;
    let (a, b) = (a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?);
    ((0.0..=1.0).contains(&a) && (0.0..=1.0).contains(&b)).then_some((a, b))
}

/// Segments lit for `fraction`: any health above zero lights at least one.
pub fn lit(fraction: f32) -> u32 {
    if fraction <= 0.0 {
        0
    } else {
        ((fraction * SEGMENTS as f32).ceil() as u32).clamp(1, SEGMENTS)
    }
}

/// What the image depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub lit: u32,
    pub low: bool,
    pub scale: u32,
}

impl Look {
    pub fn of(fraction: f32, scale: u32) -> Self {
        Look {
            lit: lit(fraction),
            low: fraction <= LOW,
            scale: scale.max(1),
        }
    }
}

/// Pixels per meter pixel for a game image `height` pixels tall.
pub fn scale_for(height: u32) -> u32 {
    (height / 480).clamp(1, 6)
}

/// The meter's RGBA image (width, height, pixels).
pub fn rasterize(look: Look) -> (u32, u32, Vec<u8>) {
    let s = look.scale;
    let w = (PAD * 2 + SEGMENTS * SEG_W + (SEGMENTS - 1) * GAP) * s;
    let h = (PAD * 2 + SEG_H) * s;
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        px.extend_from_slice(&PLATE);
    }
    let on = if look.low { LIT_LOW } else { LIT };
    for i in 0..SEGMENTS {
        let colour = if i < look.lit { on } else { UNLIT };
        let x0 = (PAD + i * (SEG_W + GAP)) * s;
        for y in PAD * s..(PAD + SEG_H) * s {
            for x in x0..x0 + SEG_W * s {
                let o = ((y * w + x) * 4) as usize;
                px[o..o + 4].copy_from_slice(&colour);
            }
        }
    }
    (w, h, px)
}

/// Top-left of an image `size` with its right edge and top at `at`
/// (fractions) of `area`, kept inside it.
pub fn place(area: Viewport, size: (u32, u32), at: (f32, f32)) -> (u32, u32) {
    let right = area.x + (area.width as f32 * at.0) as u32;
    let x = right.saturating_sub(size.0).max(area.x);
    let top = area.y + (area.height as f32 * at.1) as u32;
    let y = top.min((area.y + area.height).saturating_sub(size.1));
    (x, y)
}

/// Draws the meter over an already composited target.
pub struct HealthMeterRenderer {
    sprite: Sprite,
    cached: Option<(Look, (u32, u32))>,
}

impl HealthMeterRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            sprite: Sprite::new(device, "app health meter"),
            cached: None,
        }
    }

    /// Records a pass drawing the meter for `fraction` in `area` of `dst`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        fraction: f32,
        at: (f32, f32),
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        dst_size: (u32, u32),
        area: Viewport,
    ) {
        let look = Look::of(fraction, scale_for(area.height));
        let size = match self.cached {
            Some((l, size)) if l == look => size,
            _ => {
                let (w, h, px) = rasterize(look);
                self.sprite.upload(device, queue, (w, h), &px);
                self.cached = Some((look, (w, h)));
                (w, h)
            }
        };
        let pos = place(area, size, at);
        self.sprite
            .draw(device, encoder, dst, dst_format, dst_size, pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_health_lights_a_segment_and_full_lights_all() {
        assert_eq!(lit(0.0), 0);
        assert_eq!(lit(-1.0), 0);
        assert_eq!(lit(0.01), 1);
        assert_eq!(lit(0.5), 4);
        assert_eq!(lit(0.51), 5);
        assert_eq!(lit(1.0), SEGMENTS);
        assert!(Look::of(0.25, 1).low && !Look::of(0.26, 1).low);
    }

    #[test]
    fn the_image_has_lit_and_unlit_segments() {
        let (w, h, px) = rasterize(Look::of(0.5, 2));
        assert_eq!((w, h), ((4 + 8 * 9 + 7 * 2) * 2, (4 + 5) * 2));
        let at = |x: u32, y: u32| {
            let o = ((y * w + x) * 4) as usize;
            [px[o], px[o + 1], px[o + 2], px[o + 3]]
        };
        let mid_y = h / 2;
        let seg_x = |i: u32| (PAD + i * (SEG_W + GAP) + SEG_W / 2) * 2;
        assert_eq!(at(seg_x(0), mid_y), LIT);
        assert_eq!(at(seg_x(3), mid_y), LIT);
        assert_eq!(at(seg_x(4), mid_y), UNLIT);
        assert_eq!(at(0, 0), PLATE);
        let (_, _, low) = rasterize(Look::of(0.2, 1));
        let o = (((PAD + 1) * (4 + 8 * 9 + 7 * 2) + PAD + 1) * 4) as usize;
        assert_eq!(&low[o..o + 4], &LIT_LOW);
    }

    #[test]
    fn the_meter_is_placed_by_its_right_edge_and_kept_inside() {
        let area = Viewport {
            x: 100,
            y: 0,
            width: 1000,
            height: 750,
        };
        assert_eq!(place(area, (200, 20), (0.9, 0.1)), (800, 75));
        assert_eq!(place(area, (200, 20), (0.0, 1.0)), (100, 730));
        assert_eq!(parse_at("0.9, 0.2"), Some((0.9, 0.2)));
        assert_eq!(parse_at("2,0"), None);
        assert_eq!(parse_at("x"), None);
    }

    #[test]
    fn the_handle_clamps_and_drops_nonsense() {
        assert_eq!(HealthMeter::new(|| Some(1.5)).get(), Some(1.0));
        assert_eq!(HealthMeter::new(|| Some(f32::NAN)).get(), None);
        assert_eq!(HealthMeter::new(|| None).get(), None);
        assert_eq!(HealthMeter::new(|| None).at(2.0, 0.5).at, (1.0, 0.5));
    }
}
