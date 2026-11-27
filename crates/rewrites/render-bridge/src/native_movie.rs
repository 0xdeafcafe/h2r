//! The movie's draw as an overlay item (docs/graphics/retire-d3d-display.md,
//! "Movies"): the quad `0x00156960` would push through the screen-quad
//! batch its record begins, placed as that batch's transform places it,
//! over the movie's upload. Pure: the host places the pass.
//!
//! The record (`0x004e91c4`, filled by the wrapper's start) is read as the
//! begin reads it ([`native_render::hud::Record`]): its blend mode, its
//! filter and address mode, its bitmap's size for the coordinates, and the
//! screen transform. A record whose state the overlay does not draw as the
//! begin would (it keeps the caller's state, binds more than one stage, a
//! second constant block or a pixel offset) is refused, as is a viewport
//! other than the full screen: the overlay's positions are the canvas's.

use frame_scene::{
    ClipRect, Combiner, Overlay, OverlayBlend, OverlayMaterial, OverlayMovie, OverlayTexture, OverlayVertex, Program,
    Sampler,
};
use native_render::camera::GuestRead;
use native_render::hud::{Batch, Bitmap, Blend, Record, ScreenTransform, Vertex, text_quad};
use timeline::native::Ui;

/// Why a movie's draw is not on the overlay. It shows nothing: the
/// guest's texture is never filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovieRefusal {
    /// The record or its bitmap does not read, or sets state the overlay
    /// does not draw as the begin does.
    Record,
    /// The screen quads' viewport is not the full screen.
    Frame,
    /// The host did not place the pass.
    Pass,
}

impl MovieRefusal {
    pub fn counter(self) -> Ui {
        match self {
            MovieRefusal::Record => Ui::MovieRefusedRecord,
            MovieRefusal::Frame => Ui::MovieRefusedFrame,
            MovieRefusal::Pass => Ui::MovieRefusedPass,
        }
    }
}

/// The movie being drawn: its id (the HBINK handle), the frame and the
/// picture's size in texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Movie {
    pub movie: u32,
    pub frame: u32,
    pub size: [u32; 2],
}

fn short(mem: &dyn GuestRead, addr: u32) -> Option<i16> {
    let w = mem.read_u32(addr & !3)?;
    Some((if addr & 2 == 0 { w } else { w >> 16 }) as u16 as i16)
}

/// The overlay of one movie draw: `corners` the guest's four `(x, y, u,
/// v)` (`bink_hle::MovieQuad`), `record` its batch's record. One
/// `MovieFrame` item over the canvas, its corners where the begin's
/// transform and the full-screen viewport put them, its coordinates the
/// texels the guest's texture holds over the picture's size.
pub fn overlay(mem: &dyn GuestRead, record: u32, corners: &[[f32; 4]; 4], movie: Movie) -> Result<Overlay, MovieRefusal> {
    let r = Record::read(mem, record).ok_or(MovieRefusal::Record)?;
    if r.keep_state
        || r.bitmaps[0] == 0
        || r.bitmaps[1] != 0
        || r.bitmaps[2] != 0
        || r.flips != [0; 3]
        || r.pairs != [0; 3]
        || r.second != [0.0; 4]
        || r.offset != 0
    {
        return Err(MovieRefusal::Record);
    }
    let canvas = crate::native_hud::ui_overlay::FULL_SCREEN;
    let at = crate::native_hud::ui_overlay::SCREEN_RECT;
    let viewport = [short(mem, at), short(mem, at + 2), short(mem, at + 4), short(mem, at + 6)];
    if viewport != canvas.map(Some) {
        return Err(MovieRefusal::Frame);
    }
    let blend = match Blend::read(mem, r.blend_mode).ok_or(MovieRefusal::Record)? {
        Blend::Off => OverlayBlend::Off,
        Blend::On { source, dest, equation } => OverlayBlend::On { source, dest, equation },
    };
    let transform = ScreenTransform::read(mem, &r).ok_or(MovieRefusal::Frame)?;
    let bitmap = Bitmap::read(mem, r.bitmaps[0]).ok_or(MovieRefusal::Record)?;
    if movie.size[0] == 0 || movie.size[1] == 0 {
        return Err(MovieRefusal::Record);
    }
    let batch = Batch {
        record,
        bitmap,
        wrap: r.wrap[0],
        point: r.point,
        blend: Blend::Off,
        transform,
        coordinate_scale: r.coordinate_scale,
    };
    let quad = corners.map(|[x, y, u, v]| Vertex { x, y, u, v, argb: 0xffff_ffff });
    let size = [canvas[3] as u32, canvas[2] as u32];
    let placed = text_quad(&batch, [0.0, 0.0, size[0] as f32, size[1] as f32], &quad);
    // The begin's coordinates name the guest texture's texels; the
    // picture is those texels from its corner, at its own size.
    let scale = [
        f32::from(bitmap.width) / movie.size[0] as f32,
        f32::from(bitmap.height) / movie.size[1] as f32,
    ];
    let vertices = placed.map(|t| OverlayVertex {
        position: t.window,
        uv: [t.coords[0] * scale[0], t.coords[1] * scale[1]],
        color: t.color,
    });
    let material = OverlayMaterial {
        program: Program::Fixed(Combiner::Texture),
        texture: OverlayTexture::None,
        sampler: Sampler { wrap: r.wrap[0], point: r.point },
        blend,
    };
    let mut overlay = Overlay::new(size);
    overlay
        .push_movie_frame(material, ClipRect::full(size), vertices, OverlayMovie { movie: movie.movie, frame: movie.frame })
        .map_err(|_| MovieRefusal::Record)?;
    Ok(overlay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_render::hud::{BLEND_DEST, BLEND_EQUATION, BLEND_SOURCE, DEPTH, SCALE_X, SCALE_Y};
    use std::collections::HashMap;

    struct Mem(HashMap<u32, u32>);
    impl GuestRead for Mem {
        fn read_u32(&self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied()
        }
    }

    const RECORD: u32 = 0x004e_91c4;
    const BITMAP: u32 = 0x0071_0000;

    fn world() -> Mem {
        let mut m = Mem(HashMap::new());
        for o in (0..native_render::hud::RECORD_SIZE).step_by(4) {
            m.0.insert(RECORD + o, 0);
        }
        m.0.insert(RECORD + 0x0c, BITMAP);
        m.0.insert(RECORD + 0x40, 1.0f32.to_bits());
        m.0.insert(RECORD + 0x44, 1.0f32.to_bits());
        // Blend mode 0, linear filtering, the record's own state.
        m.0.insert(RECORD + 0x94, 0);
        m.0.insert(BLEND_SOURCE, 0x302);
        m.0.insert(BLEND_DEST, 0x303);
        m.0.insert(BLEND_EQUATION, 0x8006);
        // A linear 1024 x 512 bitmap (flags bit 4).
        m.0.insert(BITMAP + 4, 1024 | 512 << 16);
        m.0.insert(BITMAP + 0xc, 0x10 << 16);
        m.0.insert(BITMAP + 0x50, 0x0072_0000);
        let at = crate::native_hud::ui_overlay::SCREEN_RECT;
        m.0.insert(at, 0);
        m.0.insert(at + 4, 480 | 640 << 16);
        m.0.insert(SCALE_X, 2.0f32.to_bits());
        m.0.insert(SCALE_Y, (-2.0f32).to_bits());
        m.0.insert(DEPTH, 0.0f32.to_bits());
        m
    }

    fn quad() -> [[f32; 4]; 4] {
        [[20.0, 60.0, 0.0, 0.0], [620.0, 60.0, 600.0, 0.0], [620.0, 420.0, 600.0, 360.0], [20.0, 420.0, 0.0, 360.0]]
    }

    const MOVIE: Movie = Movie { movie: 0xa000_0000, frame: 3, size: [600, 360] };

    #[test]
    fn the_movie_is_one_frame_item_over_the_canvas() {
        let o = overlay(&world(), RECORD, &quad(), MOVIE).unwrap();
        assert_eq!(o.size, [640, 480]);
        assert_eq!(o.items.len(), 1);
        let item = &o.items[0];
        assert_eq!(item.kind, frame_scene::OverlayKind::MovieFrame);
        assert_eq!(item.material.texture, OverlayTexture::Movie { movie: 0xa000_0000 });
        assert_eq!(item.material.blend, OverlayBlend::On { source: 0x302, dest: 0x303, equation: 0x8006 });
        assert_eq!(o.movie_of(item), Some(&OverlayMovie { movie: 0xa000_0000, frame: 3 }));
        let v = o.vertices_of(item);
        let near = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3;
        // The begin's pixel-centre transform: a half pixel up and left.
        assert!(near(v[0].position, [19.5, 59.5]), "{:?}", v[0].position);
        assert!(near(v[2].position, [619.5, 419.5]), "{:?}", v[2].position);
        // The whole picture, whatever the texture's size.
        assert!(near(v[0].uv, [0.0, 0.0]) && near(v[2].uv, [1.0, 1.0]), "{:?} {:?}", v[0].uv, v[2].uv);
        assert_eq!(v[1].color, [1.0; 4]);
        assert_eq!(o.check(), Ok(()));
    }

    #[test]
    fn state_the_overlay_does_not_draw_is_refused_by_name() {
        let mut m = world();
        m.0.insert(RECORD + 0x94, 1 << 24);
        assert_eq!(overlay(&m, RECORD, &quad(), MOVIE), Err(MovieRefusal::Record), "keeps the caller's state");
        let mut m = world();
        m.0.insert(RECORD + 0x10, BITMAP);
        assert_eq!(overlay(&m, RECORD, &quad(), MOVIE), Err(MovieRefusal::Record), "a second stage");
        let mut m = world();
        m.0.insert(RECORD + 0x04, 0x0073_0000);
        assert_eq!(overlay(&m, RECORD, &quad(), MOVIE), Err(MovieRefusal::Record), "a pixel offset");
        let mut m = world();
        m.0.insert(crate::native_hud::ui_overlay::SCREEN_RECT + 4, 240 | 320 << 16);
        assert_eq!(overlay(&m, RECORD, &quad(), MOVIE), Err(MovieRefusal::Frame), "a split viewport");
        assert_eq!(overlay(&Mem(HashMap::new()), RECORD, &quad(), MOVIE), Err(MovieRefusal::Record));
        assert_eq!(MovieRefusal::Pass.counter() as usize, Ui::MovieRefusedPass as usize);
    }
}
