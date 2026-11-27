//! The quad the weather plate function `0x00037a60` submits once its
//! placement `0x0003a5c0` accepts the plate (`0x00037dbe..0x00038342`):
//! the vertices, the per-layer texture coordinates and constants, the
//! depth and the colours handed to the state function
//! ([`crate::weather_place::kind3`]).
//!
//! One quad strip (`0x003fb440(9)`, D3D `QUADSTRIP`) of four vertices in
//! the order top left, top right, bottom left, bottom right. Each vertex
//! sets four 2-float attributes (`0x003fb280` registers 1 to 4) and then its
//! position (`0x003fb2d0`: x, y, z, w). Attribute 1 is the vertex's own
//! pixel position; attributes 2 to 4 are the three layers' coordinates,
//! which span `-size` to `size` over the sheet (`[0x0045dbcc]` is -1). The
//! three layers' scroll goes to vertex constants `c[21..24]`
//! (`0x003f71d0`). The layers' bitmaps bind to stages 0 to 2 (`0x000143c0`).
//! The vertex shader is engine slot `0x15` (`0x0001cbe0`).

use frame_scene::effect::WeatherPlate;

use crate::weather_place::Colours;

/// `[0x0045dd40]`: the depth scale, 2^24 - 1.
pub const DEPTH_SCALE: f32 = 16_777_215.0;
/// The first vertex constant of the layers' scroll.
pub const SCROLL_CONSTANT: usize = 21;
/// The engine vertex shader slot the quad draws with.
pub const VERTEX_SLOT: u32 = 0x15;

/// One vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    /// Pixels, half a pixel in from the window.
    pub position: [f32; 2],
    /// Attribute registers 2 to 4: the layers' coordinates.
    pub layers: [[f32; 2]; 3],
}

/// The submit.
#[derive(Clone, Debug, PartialEq)]
pub struct Quad {
    /// Strip order: top left, top right, bottom left, bottom right.
    pub vertices: [Vertex; 4],
    /// `c[21..24]`: each layer's scroll in `x`, `y`.
    pub constants: [[f32; 4]; 3],
    /// Position `z` and `w` of every vertex.
    pub z: f32,
    pub w: f32,
    /// The colours for the state function.
    pub colours: Colours,
}

fn clamp(x: f32, hi: f32) -> f32 {
    // `comiss 0, x; jbe` then `comiss x, hi; jbe`: below 0 is 0, above hi
    // is hi, NaN is left as it is.
    if 0.0 > x {
        0.0
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// The depth words for the placement's `result` (`[esp+14h]` after the
/// call), the view's near and far (`[0x00485ad4]`, `[0x00485ad8]`): `z`
/// from the clip range, `w` from the result over far, each clamped to
/// `[0, DEPTH_SCALE]`.
pub fn depth(result: f32, near: f32, far: f32) -> (f32, f32) {
    let k = far / (far - near);
    let z = (k * result - k * near) / result * DEPTH_SCALE;
    let w = result / far * DEPTH_SCALE;
    (clamp(z, DEPTH_SCALE), clamp(w, DEPTH_SCALE))
}

/// The submit of `plate` after a placement that returned `result`.
pub fn quad(plate: &WeatherPlate, result: f32, near: f32, far: f32) -> Quad {
    let [left, top, right, bottom] = plate.window.map(|v| f32::from(v) - 0.5);
    let (z, w) = depth(result, near, far);
    // Layer `i`'s coordinates at the corners: (-size, -size), (size, -size),
    // (-size, size), (size, size).
    let layers = |corner: usize| -> [[f32; 2]; 3] {
        std::array::from_fn(|i| {
            let s = plate.size[i];
            let (u, v) = (if corner & 1 == 0 { s * -1.0 } else { s }, if corner & 2 == 0 { s * -1.0 } else { s });
            [u, v]
        })
    };
    let at = |x: f32, y: f32, corner: usize| Vertex { position: [x, y], layers: layers(corner) };
    let [r, g, b] = plate.tint;
    Quad {
        vertices: [at(left, top, 0), at(right, top, 1), at(left, bottom, 2), at(right, bottom, 3)],
        constants: std::array::from_fn(|i| [plate.scroll[i][0], plate.scroll[i][1], 0.0, 0.0]),
        z,
        w,
        colours: Colours {
            flag: true,
            rgb: [[0.0, r, 0.0], [0.0, g, 0.0], [0.0, b, 0.0], plate.fill],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plate() -> WeatherPlate {
        WeatherPlate {
            element: 0,
            player: 0,
            window: [10, 20, 650, 500],
            bitmaps: [1, 2, 3],
            layer_scale: [1.0; 3],
            scroll: [[0.25, 0.5], [1.0, 2.0], [3.0, 4.0]],
            extent: [1.0, 2.0],
            size: [2.0, 3.0, 4.0],
            tint: [1.0, 0.5, 0.25],
            fill: [0.1, 0.2, 0.3],
        }
    }

    #[test]
    fn the_strip_runs_top_left_top_right_bottom_left_bottom_right() {
        let q = quad(&plate(), 4.0, 0.5, 100.0);
        let p: Vec<_> = q.vertices.iter().map(|v| v.position).collect();
        assert_eq!(p, [[9.5, 19.5], [649.5, 19.5], [9.5, 499.5], [649.5, 499.5]]);
        assert_eq!(q.vertices[0].layers, [[-2.0, -2.0], [-3.0, -3.0], [-4.0, -4.0]]);
        assert_eq!(q.vertices[1].layers[0], [2.0, -2.0]);
        assert_eq!(q.vertices[2].layers[1], [-3.0, 3.0]);
        assert_eq!(q.vertices[3].layers[2], [4.0, 4.0]);
        assert_eq!(q.constants[1], [1.0, 2.0, 0.0, 0.0]);
    }

    #[test]
    fn the_colours_put_each_tint_channel_in_green() {
        let q = quad(&plate(), 4.0, 0.5, 100.0);
        assert_eq!(q.colours.rgb, [[0.0, 1.0, 0.0], [0.0, 0.5, 0.0], [0.0, 0.25, 0.0], [0.1, 0.2, 0.3]]);
        assert!(q.colours.flag);
    }

    #[test]
    fn depth_is_clamped_to_the_scale() {
        let (z, w) = depth(50.0, 0.5, 100.0);
        assert!((w - 0.5 * DEPTH_SCALE).abs() < 1.0);
        assert!(z > 0.0 && z <= DEPTH_SCALE);
        assert_eq!(depth(1000.0, 0.5, 100.0).1, DEPTH_SCALE);
        assert_eq!(depth(-1.0, 0.5, 100.0).1, 0.0);
    }
}
