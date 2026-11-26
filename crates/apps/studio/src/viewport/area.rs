//! The handles on the selected netgame flag's area
//! (docs/game/gametype-markers.md, "Studio"): a radius has one on its rim;
//! a box has one on each corner and one on the middle of each side, on
//! the marker's own axes; a sphere has one on its equator and one on its
//! pole; a path has one a point (drag to move, Delete to remove) and one
//! on the middle of each edge (drag to add a point there); the top and
//! bottom of the wall have one each for the heights, except a sphere's. A drag that `Area::validate`
//! refuses leaves the document as it was, and its sentence goes to the
//! status line. The geometry is `scenario_edit::area_edit`; this draws
//! the handles, picks one and turns the mouse into a point of the world.
use dear_imgui_rs::{Key, MouseButton, Ui};
use scenario_edit::BlockKind;
use scenario_edit::area_edit::{self, Handle};
use scenario_edit::geometry::{Ray, Vec3};

use crate::app::{Action, App};

/// The merge key of one handle drag of flag `index`: a drag is one undo
/// step.
fn merge_key(index: usize) -> u64 {
    0xa7ea_1000_0000 | index as u64
}

/// How close, in points, the mouse picks a handle.
const REACH: f32 = 9.0;
/// A handle's half-size, in points.
const SIZE: f32 = 5.0;

/// The drag in progress and the last refusal said.
#[derive(Default)]
pub struct AreaHandles {
    /// The flag and the handle being dragged.
    drag: Option<(usize, Handle)>,
    /// The sentence last put in the status line, so a held refused drag
    /// says it once.
    said: Option<String>,
}

/// Where the ray meets the horizontal plane at height `z`.
pub fn on_floor(ray: &Ray, z: f32) -> Option<Vec3> {
    if ray.dir[2].abs() < 1e-6 {
        return None;
    }
    let t = (z - ray.origin[2]) / ray.dir[2];
    (t > 0.0).then(|| {
        [
            ray.origin[0] + ray.dir[0] * t,
            ray.origin[1] + ray.dir[1] * t,
            z,
        ]
    })
}

/// Where the ray meets the upright plane through `at` that faces the
/// viewer, for a handle that moves up and down.
pub fn on_wall(ray: &Ray, at: Vec3) -> Option<Vec3> {
    let flat = ray.dir[0].hypot(ray.dir[1]);
    if flat < 1e-6 {
        return None;
    }
    let n = [ray.dir[0] / flat, ray.dir[1] / flat];
    let denom = n[0] * ray.dir[0] + n[1] * ray.dir[1];
    let t = (n[0] * (at[0] - ray.origin[0]) + n[1] * (at[1] - ray.origin[1])) / denom;
    (t > 0.0).then(|| {
        [
            ray.origin[0] + ray.dir[0] * t,
            ray.origin[1] + ray.dir[1] * t,
            ray.origin[2] + ray.dir[2] * t,
        ]
    })
}

impl AreaHandles {
    /// Draws the handles of the selected flag's area and runs their
    /// drags. Returns whether the mouse is over a handle and whether one
    /// is being dragged, so the camera and picking leave the mouse alone.
    pub fn run(
        &mut self,
        ui: &Ui,
        app: &mut App,
        origin: [f32; 2],
        size: [f32; 2],
        aspect: f32,
    ) -> (bool, bool) {
        let selected = app
            .session
            .selected()
            .filter(|&(kind, _)| kind == BlockKind::NetgameFlags && app.layers.placement(kind));
        let Some((_, index)) = selected else {
            return self.reset();
        };
        let Some((centre, area, _)) = app.session.editable_area(index) else {
            return self.reset();
        };
        let camera = *app.session.camera();
        let yaw = app.session.flag_yaw(index);
        let handles = area_edit::handles(centre, yaw, &area);
        let px = |p: Vec3| {
            camera
                .project(p, aspect)
                .map(|uv| [origin[0] + uv[0] * size[0], origin[1] + uv[1] * size[1]])
        };
        let mouse = ui.io().mouse_pos();
        let uv = [(mouse[0] - origin[0]) / size[0], (mouse[1] - origin[1]) / size[1]];
        let inside = (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]);
        let near = |p: [f32; 2]| (p[0] - mouse[0]).hypot(p[1] - mouse[1]);
        let hovered = handles
            .iter()
            .filter_map(|&(h, at)| Some((h, px(at)?)))
            .map(|(h, p)| (h, near(p)))
            .filter(|&(_, d)| d <= REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h)
            .filter(|_| inside);
        let active = self.drag.filter(|&(i, _)| i == index).map(|(_, h)| h);

        // Start a drag, or end it.
        if active.is_none() && ui.is_mouse_clicked(MouseButton::Left) && let Some(h) = hovered {
            self.drag = Some((index, h));
            self.said = None;
        }
        if self.drag.is_some() && !ui.is_mouse_down(MouseButton::Left) {
            self.drag = None;
            app.dispatch(Action::SealEdit);
        }
        // Delete removes the point under the mouse.
        if let Some(Handle::Point(i)) = hovered
            && self.drag.is_none()
            && ui.is_key_pressed(Key::Delete)
        {
            match area_edit::remove_point(&area, i) {
                Ok(next) => app.dispatch(Action::SetArea {
                    index,
                    area: Some(next),
                    merge: None,
                }),
                Err(e) => app.error(e.to_string()),
            }
        }
        // Follow the mouse.
        if let Some((_, h)) = self.drag.filter(|&(i, _)| i == index) {
            let ray = camera.ray(uv[0], uv[1], aspect);
            let to = if h.flat() {
                on_floor(&ray, centre[2])
            } else {
                on_wall(&ray, centre)
            };
            if let Some(to) = to {
                match area_edit::drag(&area, h, centre, yaw, to) {
                    Ok(next) => {
                        self.said = None;
                        // An edge handle made a point: the drag goes on
                        // with that point.
                        if let Handle::Edge(i) = h {
                            self.drag = Some((index, Handle::Point(i + 1)));
                        }
                        app.dispatch(Action::SetArea {
                            index,
                            area: Some(next),
                            merge: Some(merge_key(index)),
                        });
                    }
                    Err(e) => {
                        // The document keeps the last good area: the
                        // handle snaps back to it.
                        let why = e.to_string();
                        if self.said.as_deref() != Some(&why) {
                            app.error(why.clone());
                            self.said = Some(why);
                        }
                    }
                }
            }
        }

        // Draw them: a diamond each, white while hovered or dragged.
        let live = self.drag.map(|(_, h)| h).or(hovered);
        let list = ui.get_window_draw_list();
        let _clip = list.push_clip_rect(origin, [origin[0] + size[0], origin[1] + size[1]], true);
        for &(h, at) in &handles {
            let Some(p) = px(at) else { continue };
            let colour = if live == Some(h) {
                [1.0, 1.0, 1.0, 1.0]
            } else if matches!(h, Handle::Edge(_) | Handle::Side(_)) {
                [0.55, 0.85, 1.0, 0.9]
            } else {
                [1.0, 0.85, 0.2, 1.0]
            };
            let corners = [
                [p[0], p[1] - SIZE],
                [p[0] + SIZE, p[1]],
                [p[0], p[1] + SIZE],
                [p[0] - SIZE, p[1]],
            ];
            for k in 0..4 {
                list.add_line(corners[k], corners[(k + 1) % 4], colour)
                    .thickness(2.0)
                    .build();
            }
        }
        (hovered.is_some(), self.drag.is_some())
    }

    fn reset(&mut self) -> (bool, bool) {
        self.drag = None;
        self.said = None;
        (false, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ray(origin: Vec3, dir: Vec3) -> Ray {
        Ray { origin, dir }
    }

    #[test]
    fn a_ray_meets_the_floor_plane_at_the_handles_height() {
        let r = ray([0.0, 0.0, 10.0], [1.0, 0.0, -1.0]);
        assert_eq!(on_floor(&r, 2.0), Some([8.0, 0.0, 2.0]));
        // Parallel to the floor, or the plane behind the eye: no point.
        assert_eq!(on_floor(&ray([0.0; 3], [1.0, 0.0, 0.0]), 2.0), None);
        assert_eq!(on_floor(&ray([0.0, 0.0, 1.0], [1.0, 0.0, 1.0]), 0.0), None);
    }

    #[test]
    fn a_ray_meets_the_upright_plane_through_the_marker() {
        // Looking along +x and down: the plane x = 5 is met at z = 4.
        let r = ray([0.0, 0.0, 9.0], [1.0, 0.0, -1.0]);
        let hit = on_wall(&r, [5.0, 0.0, 0.0]).unwrap();
        assert!((hit[0] - 5.0).abs() < 1e-5 && (hit[2] - 4.0).abs() < 1e-5, "{hit:?}");
        // Looking straight down has no facing plane.
        assert_eq!(on_wall(&ray([0.0; 3], [0.0, 0.0, -1.0]), [5.0, 0.0, 0.0]), None);
    }
}
