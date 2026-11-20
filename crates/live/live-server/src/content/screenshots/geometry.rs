//! Compare strong structure in a small thumbnail with projected map edges.
//! Outlines are admin-owned and have to describe the exact published map.

use image::{DynamicImage, GrayImage};
use live_proto::game::Outline;
use live_proto::screenshots::CameraPose;

const WIDTH: usize = 48;
const HEIGHT: usize = 27;
const MAX_EDGES: usize = 12_000;
const EDGE_CONTRAST: u8 = 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Match {
    Plausible,
    Mismatch,
    Unavailable,
}

pub(super) fn compare(image: &DynamicImage, camera: &CameraPose, outline: &Outline) -> Match {
    let position = camera.position.map(f64::from);
    for (axis, coordinate) in position.into_iter().enumerate() {
        let lo = outline.bounds.min[axis];
        let hi = outline.bounds.max[axis];
        if !lo.is_finite() || !hi.is_finite() || lo > hi {
            return Match::Unavailable;
        }
        let margin = ((hi - lo) * 0.08).clamp(3.0, 25.0);
        if coordinate < lo - margin || coordinate > hi + margin {
            return Match::Mismatch;
        }
    }
    if image.width() < 128 || image.height() < 72 {
        return Match::Mismatch;
    }

    let mut predicted = [false; WIDTH * HEIGHT];
    let forward = camera.forward.map(f64::from);
    let up = camera.up.map(f64::from);
    let right = [
        forward[1] * up[2] - forward[2] * up[1],
        forward[2] * up[0] - forward[0] * up[2],
        forward[0] * up[1] - forward[1] * up[0],
    ];
    let vertical = f64::from(image.height()) / f64::from(image.width());
    // A broad horizontal field of view and a pixel neighborhood absorb
    // camera and outline approximation, including a short level change.
    let project = |point: [f64; 3]| {
        let delta = std::array::from_fn::<_, 3, _>(|i| point[i] - position[i]);
        let dot = |axis: [f64; 3]| delta.into_iter().zip(axis).map(|(d, a)| d * a).sum::<f64>();
        let depth = dot(forward);
        if depth < 0.2 {
            return None;
        }
        let x = (0.5 + dot(right) / depth * 0.65) * WIDTH as f64;
        let y = (0.5 - dot(up) / depth * 0.65 / vertical) * HEIGHT as f64;
        (x >= 2.0 && x < (WIDTH - 2) as f64 && y >= 2.0 && y < (HEIGHT - 2) as f64)
            .then_some((x as usize, y as usize))
    };

    let mut edges = 0;
    for layer in &outline.layers {
        let z = (layer.z[0] + layer.z[1]) * 0.5;
        for polygon in &layer.polygons {
            for (a, b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
                edges += 1;
                if edges > MAX_EDGES {
                    return Match::Unavailable;
                }
                for t in [0.0, 0.25, 0.5, 0.75] {
                    if let Some((x, y)) =
                        project([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, z])
                    {
                        predicted[y * WIDTH + x] = true;
                    }
                }
            }
        }
    }
    let expected = predicted.into_iter().filter(|p| *p).count();
    if expected < 12 {
        return Match::Unavailable;
    }
    let thumbnail = image
        .thumbnail_exact(WIDTH as u32, HEIGHT as u32)
        .into_luma8();
    let mut supported = 0;
    let mut background = 0;
    let mut background_total = 0;
    for y in 2..HEIGHT - 2 {
        for x in 2..WIDTH - 2 {
            let sharp = (y - 1..=y + 1)
                .any(|row| (x - 1..=x + 1).any(|col| gradient(&thumbnail, col, row)));
            if predicted[y * WIDTH + x] {
                supported += usize::from(sharp);
            } else {
                background += usize::from(sharp);
                background_total += 1;
            }
        }
    }
    // A busy frame can match by chance. Predicted edges must explain
    // measurably more structure than the rest of the frame does.
    if supported * 3 >= expected
        && background_total > 0
        && supported * background_total * 4 > background * expected * 5
    {
        Match::Plausible
    } else {
        Match::Mismatch
    }
}

fn gradient(thumb: &GrayImage, x: usize, y: usize) -> bool {
    let at = |x, y| thumb.get_pixel(x as u32, y as u32).0[0];
    at(x - 1, y).abs_diff(at(x + 1, y)) >= EDGE_CONTRAST
        || at(x, y - 1).abs_diff(at(x, y + 1)) >= EDGE_CONTRAST
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::game::{Bounds, Layer};

    #[test]
    fn geometry_looks_for_the_projected_structure_in_the_image() {
        let outline = Outline {
            map: "synthetic".into(),
            bounds: Bounds {
                min: [0.0, -10.0, -3.0],
                max: [20.0, 10.0, 3.0],
            },
            layers: vec![Layer {
                z: [0.0, 0.0],
                polygons: vec![vec![[10.0, -5.0], [10.0, 5.0], [20.0, 5.0], [20.0, -5.0]]],
            }],
        };
        let camera = CameraPose {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        };
        let image = GrayImage::from_fn(480, 270, |_, y| {
            image::Luma([if y < 135 { 10 } else { 200 }])
        });
        let blank = DynamicImage::ImageLuma8(GrayImage::new(480, 270));
        assert_eq!(
            compare(&DynamicImage::ImageLuma8(image), &camera, &outline),
            Match::Plausible
        );
        assert_eq!(compare(&blank, &camera, &outline), Match::Mismatch);
        let away = CameraPose {
            forward: [-1.0, 0.0, 0.0],
            ..camera
        };
        assert_eq!(compare(&blank, &away, &outline), Match::Unavailable);
        let outside = CameraPose {
            position: [100.0, 0.0, 0.0],
            ..camera
        };
        assert_eq!(compare(&blank, &outside, &outline), Match::Mismatch);
    }
}
