//! The movie on the native renderer (docs/graphics/retire-d3d-display.md,
//! "Movies"): the Bink HLE's native draw hands its quad and decoded picture
//! here; `render_bridge::native_movie` makes the overlay and
//! `hud_overlay::place_movie` places it at the guest's point in the frame,
//! the picture uploaded as the stage records. A draw that is refused shows
//! nothing and is counted by its reason (`native_ui_movie_refused_*`).

use std::sync::Arc;

use bink_hle::{MovieDisplay, MovieDraw};
use render_bridge::native_movie::{Movie, MovieRefusal};

struct Display;

struct Read<'a>(&'a runtime::Guest);

impl native_render::camera::GuestRead for Read<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

impl MovieDisplay for Display {
    fn show(&self, mem: &mut runtime::Guest, draw: &MovieDraw) -> bool {
        let p = &draw.picture;
        let movie = Movie { movie: p.movie, frame: p.frame, size: [p.width, p.height] };
        let overlay = match render_bridge::native_movie::overlay(&Read(mem), draw.record, &draw.quad.corners, movie) {
            Ok(o) => o,
            Err(why) => {
                why.counter().count();
                return false;
            }
        };
        let image = native_render::overlay::MovieImage {
            movie: p.movie,
            frame: p.frame,
            width: p.width,
            height: p.height,
            bgra: Arc::clone(&p.bgra),
        };
        if crate::hud_overlay::place_movie(mem, overlay, image) {
            return true;
        }
        MovieRefusal::Pass.counter().count();
        false
    }
}

/// The native renderer's movie display.
pub fn install() {
    bink_hle::set_display(Some(Arc::new(Display)));
}
