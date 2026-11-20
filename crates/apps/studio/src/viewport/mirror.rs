//! One interpolated sample supplies both the viewport and its snapshot.
//! Object records retain the simulation's parent-relative coordinates.
use std::collections::{BTreeSet, HashMap};

use agent_proto::live::Object;
use live_mirror::Sample;
use map::TagId;
use scenario_edit::geometry::{Camera, Vec3, add, cross, dot, scale, sub};
use scenario_edit::render::{Instance, object_light};
use scenario_edit::{BlockKind, Session};

use crate::App;

#[derive(Clone, Copy)]
struct Pose {
    position: Vec3,
    axes: [Vec3; 3],
    scale: f32,
}

impl Pose {
    fn of(object: &Object) -> Self {
        Self {
            position: object.position,
            axes: [object.forward, cross(object.up, object.forward), object.up],
            scale: object.scale,
        }
    }

    fn direction(self, vector: Vec3) -> Vec3 {
        let [x, y, z] = self.axes;
        add(
            add(scale(x, vector[0]), scale(y, vector[1])),
            scale(z, vector[2]),
        )
    }

    fn child(self, child: Self) -> Self {
        Self {
            position: add(
                self.position,
                scale(self.direction(child.position), self.scale),
            ),
            axes: child.axes.map(|axis| self.direction(axis)),
            scale: self.scale * child.scale,
        }
    }
}

pub(crate) struct WorldPoses(HashMap<u32, Option<Pose>>);

impl WorldPoses {
    pub(crate) fn new(sample: &Sample) -> Self {
        let by_id: HashMap<_, _> = sample.objects.iter().map(|o| (o.id, o)).collect();
        let mut poses = HashMap::<u32, Option<Pose>>::with_capacity(by_id.len());
        let mut chain = Vec::new();
        for object in &sample.objects {
            if poses.contains_key(&object.id) {
                continue;
            }
            let mut current = object;
            let mut base = loop {
                // A pending entry also breaks a cyclic parent chain.
                if let Some(base) = poses.get(&current.id) {
                    break *base;
                }
                poses.insert(current.id, None);
                let Some(parent) = current.parent else {
                    let root = Some(Pose::of(current));
                    poses.insert(current.id, root);
                    break root;
                };
                chain.push(current);
                let Some(&next) = by_id.get(&parent) else {
                    break None;
                };
                current = next;
            };
            for child in chain.drain(..).rev() {
                base = base.map(|p| p.child(Pose::of(child)));
                poses.insert(child.id, base);
            }
        }
        Self(poses)
    }

    pub(crate) fn position(&self, id: u32) -> Option<Vec3> {
        Some(self.0.get(&id).copied().flatten()?.position)
    }
}

#[derive(Default)]
pub(crate) struct Scene {
    pub(crate) instances: Vec<(TagId, Instance)>,
    pub(crate) linked: BTreeSet<(BlockKind, usize)>,
    links: Vec<(Vec3, Vec3)>,
    players: Vec<Player>,
}

struct Player {
    pos: Vec3,
    aim: Vec3,
    label: String,
    color: Color,
    body: f32,
    shield: f32,
}

type Color = [u8; 3];

fn team_color(team: i16) -> Color {
    match team {
        0 => [255, 110, 95],
        1 => [105, 175, 255],
        2 => [84, 215, 144],
        3 => [255, 193, 72],
        4 => [174, 144, 255],
        5 => [255, 156, 206],
        6 => [112, 217, 225],
        7 => [191, 155, 127],
        _ => [225, 230, 235],
    }
}

impl Scene {
    pub(crate) fn shown(app: &App) -> Self {
        let Some(sample) = app.pairing.shown.as_ref() else {
            return Self::default();
        };
        if !app.pairing.show_objects || !app.pairing.matches_sample(app, sample) {
            return Self::default();
        }
        Self::new(sample, &app.session)
    }

    fn new(sample: &Sample, session: &Session) -> Self {
        let poses = WorldPoses::new(sample);
        let markers: HashMap<_, _> = session
            .markers()
            .iter()
            .filter_map(|m| {
                let kind = scenario_edit::layout::block(m.block).object_type?;
                Some(((kind, m.index), m))
            })
            .collect();
        let players: HashMap<_, _> = sample
            .players
            .iter()
            .filter_map(|p| Some((p.unit?, p)))
            .collect();
        let mut scene = Self::default();
        let selected = session.selection();
        for object in &sample.objects {
            let Some(pose) = poses.0.get(&object.id).copied().flatten() else {
                continue;
            };
            let placed = object
                .placement
                .and_then(|i| markers.get(&(i16::from(object.kind), usize::from(i))));
            let selected_here = placed.is_some_and(|m| selected.contains(&(m.block, m.index)));
            if let Some(tag) = object.tag {
                let mut instance =
                    Instance::at(pose.position, pose.axes, pose.scale, selected_here);
                if let Some(light) = session.scene().and_then(|s| s.light_at(pose.position)) {
                    instance.tint[..3].copy_from_slice(&object_light(light));
                }
                scene.instances.push((TagId(tag), instance));
            }
            if let Some(m) = placed {
                scene.linked.insert((m.block, m.index));
                let delta = sub(m.position, pose.position);
                if dot(delta, delta) > 1.0e-8 {
                    scene.links.push((m.position, pose.position));
                }
            }
            if let Some(player) = players.get(&object.id) {
                let label = if player.team < 0 {
                    player.name.clone()
                } else {
                    format!("{} · Team {}", player.name, player.team)
                };
                scene.players.push(Player {
                    pos: pose.position,
                    aim: player.aim,
                    label,
                    color: team_color(player.team),
                    body: object.body,
                    shield: object.shield,
                });
            }
        }
        scene
    }

    /// Pixel coordinates relative to the viewport, at this camera and size.
    fn annotations(&self, camera: &Camera, size: [f32; 2]) -> Vec<Mark<'_>> {
        let project = |p| {
            camera
                .project(p, size[0] / size[1])
                .map(|q| [q[0] * size[0], q[1] * size[1]])
                .filter(|q| q.iter().all(|v| v.is_finite()))
        };
        let mut marks = Vec::new();
        for &(a, b) in &self.links {
            if let (Some(a), Some(b)) = (project(a), project(b)) {
                marks.push(Mark::Line(a, b, [190, 176, 140], 1.0));
            }
        }
        for p in &self.players {
            let Some(pos) = project(p.pos) else { continue };
            if let Some(end) = project(add(p.pos, scale(p.aim, 2.5))) {
                marks.push(Mark::Line(pos, end, p.color, 2.0));
            }
            let origin = [pos[0] + 7.0, pos[1] - 25.0];
            marks.push(Mark::Text(origin, &p.label, p.color));
            for (offset, value, color) in [
                (18.0, p.shield, [96, 194, 255]),
                (22.0, p.body, [112, 220, 134]),
            ] {
                let a = [origin[0], origin[1] + offset];
                marks.push(Mark::Line(a, [a[0] + 38.0, a[1]], [40, 44, 48], 3.0));
                if value > 0.0 {
                    marks.push(Mark::Line(
                        a,
                        [a[0] + 38.0 * value.min(1.0), a[1]],
                        color,
                        3.0,
                    ));
                }
            }
        }
        marks
    }

    pub(super) fn overlay(&self, ui: &dear_imgui_rs::Ui, cam: &Camera, rect: [f32; 4]) {
        let [x, y, w, h] = rect;
        let offset = |p: [f32; 2]| [x + p[0], y + p[1]];
        let rgba = |[r, g, b]: Color| {
            [
                f32::from(r) / 255.0,
                f32::from(g) / 255.0,
                f32::from(b) / 255.0,
                1.0,
            ]
        };
        let list = ui.get_window_draw_list();
        let _clip = list.push_clip_rect([x, y], [x + w, y + h], true);
        for mark in self.annotations(cam, [w, h]) {
            match mark {
                Mark::Line(a, b, c, thickness) => {
                    list.add_line(offset(a), offset(b), rgba(c))
                        .thickness(thickness)
                        .build();
                }
                Mark::Text(a, text, c) => {
                    list.add_text(offset([a[0] + 1.0, a[1] + 1.0]), [0.0, 0.0, 0.0, 1.0], text);
                    list.add_text(offset(a), rgba(c), text);
                }
            }
        }
    }

    pub(crate) fn snapshot(&self, rgba: &mut [u8], size: [u32; 2], cam: &Camera) {
        let mut canvas = Pixels { rgba, size };
        for mark in self.annotations(cam, size.map(|v| v as f32)) {
            match mark {
                Mark::Line(a, b, c, thickness) => canvas.line(a, b, thickness, c),
                Mark::Text(a, text, c) => {
                    canvas.text([a[0] + 1.0, a[1] + 1.0], text, [0; 3]);
                    canvas.text(a, text, c);
                }
            }
        }
    }
}

enum Mark<'a> {
    Line([f32; 2], [f32; 2], Color, f32),
    Text([f32; 2], &'a str, Color),
}

// The API's snapshot has no ImGui context. Composite its screen-space
// annotations onto the renderer's readback with the window's own font.
struct Pixels<'a> {
    rgba: &'a mut [u8],
    size: [u32; 2],
}

impl Pixels<'_> {
    fn pixel(&mut self, x: i32, y: i32, color: Color, alpha: f32) {
        if x < 0 || y < 0 || x as u32 >= self.size[0] || y as u32 >= self.size[1] {
            return;
        }
        let i = 4 * (y as usize * self.size[0] as usize + x as usize);
        for (dst, src) in self.rgba[i..i + 3].iter_mut().zip(color) {
            *dst = (f32::from(*dst) * (1.0 - alpha) + f32::from(src) * alpha).round() as u8;
        }
    }

    fn line(&mut self, a: [f32; 2], b: [f32; 2], thickness: f32, color: Color) {
        // Clip the segment before walking pixels: a point just in front of
        // the camera can project well outside the image.
        let d = [b[0] - a[0], b[1] - a[1]];
        let mut t0 = 0.0f32;
        let mut t1 = 1.0f32;
        for (axis, extent) in self.size.into_iter().enumerate() {
            let hi = extent as f32;
            if d[axis] == 0.0 {
                if !(0.0..hi).contains(&a[axis]) {
                    return;
                }
            } else {
                let lo_t = -a[axis] / d[axis];
                let hi_t = (hi - a[axis]) / d[axis];
                t0 = t0.max(lo_t.min(hi_t));
                t1 = t1.min(lo_t.max(hi_t));
            }
        }
        if t0 > t1 {
            return;
        }
        let steps = ((t1 - t0) * d[0].abs().max(d[1].abs()))
            .ceil()
            .min(self.size[0].max(self.size[1]) as f32)
            .max(1.0) as u32;
        let r = (thickness / 2.0) as i32;
        for i in 0..=steps {
            let t = t0 + (t1 - t0) * i as f32 / steps as f32;
            let x = (a[0] + t * d[0]).round() as i32;
            let y = (a[1] + t * d[1]).round() as i32;
            for yy in y.saturating_sub(r)..=y.saturating_add(r) {
                for xx in x.saturating_sub(r)..=x.saturating_add(r) {
                    self.pixel(xx, yy, color, 1.0);
                }
            }
        }
    }

    fn text(&mut self, at: [f32; 2], text: &str, color: Color) {
        use ab_glyph::{Font, FontRef, ScaleFont, point};
        let font =
            FontRef::try_from_slice(crate::theme::INTER_REGULAR).expect("bundled Inter font");
        let scaled = font.as_scaled(crate::theme::FONT_SIZE);
        let mut x = at[0];
        let mut previous = None;
        for ch in text.chars() {
            let id = font.glyph_id(ch);
            if let Some(prev) = previous {
                x += scaled.kern(prev, id);
            }
            let glyph =
                id.with_scale_and_position(scaled.scale(), point(x, at[1] + scaled.ascent()));
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                if bounds.max.x >= 0.0
                    && bounds.max.y >= 0.0
                    && bounds.min.x < self.size[0] as f32
                    && bounds.min.y < self.size[1] as f32
                {
                    outline.draw(|dx, dy, coverage| {
                        self.pixel(
                            bounds.min.x as i32 + dx as i32,
                            bounds.min.y as i32 + dy as i32,
                            color,
                            coverage,
                        );
                    });
                }
            }
            x += scaled.h_advance(id);
            previous = Some(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::live::Player as LivePlayer;
    use scenario_edit::Reference;
    use scenario_edit::geometry::Marker;

    #[test]
    fn attached_weapon_uses_the_streamed_parent_and_placement_indices_are_per_block() {
        let vehicle = Object {
            id: 0x10032,
            tag: Some(44),
            kind: 1,
            position: [3.0, 4.0, 5.0],
            forward: [0.0, 1.0, 0.0],
            up: [0.0, 0.0, 1.0],
            scale: 2.0,
            placement: Some(0),
            ..Object::default()
        };
        let weapon = Object {
            id: 0x20010,
            tag: Some(51),
            kind: 2,
            parent: Some(vehicle.id),
            position: [1.0, 0.0, 0.0],
            forward: [1.0, 0.0, 0.0],
            up: vehicle.up,
            scale: 1.0,
            ..Object::default()
        };
        let orphan = Object {
            id: 0x10011,
            parent: Some(0x20032),
            ..weapon
        };
        let sample = Sample {
            epoch: 1,
            tick: 1,
            stale: false,
            objects: vec![weapon, orphan, vehicle],
            players: vec![LivePlayer {
                unit: Some(vehicle.id),
                name: "Rook".to_owned(),
                team: 1,
                aim: [1.0, 0.0, 0.0],
                ..Default::default()
            }],
        };
        let markers = [BlockKind::Scenery, BlockKind::Vehicles].map(|block| Marker {
            block,
            index: 0,
            position: [0.0; 3],
            axes: scenario_edit::geometry::rotation([0.0; 3]),
            object: None,
            scale: 1.0,
        });
        let mut session = Session::new();
        session.open_reference(Reference {
            name: "fixture".into(),
            world: Vec::new(),
            markers: markers.to_vec(),
            scene: None,
        });

        let scene = Scene::new(&sample, &session);
        assert_eq!(scene.instances.len(), 2);
        assert_eq!(scene.instances[0].0, TagId(51));
        assert_eq!(scene.instances[0].1.t, [3.0, 6.0, 5.0, 1.0]);
        assert_eq!(scene.instances[0].1.x, [0.0, 2.0, 0.0, 0.0]);
        assert_eq!(scene.linked, BTreeSet::from([(BlockKind::Vehicles, 0)]));
        assert_eq!(scene.links, vec![([0.0; 3], vehicle.position)]);
        assert_eq!(scene.players[0].label, "Rook · Team 1");
        assert_eq!(scene.players[0].aim, [1.0, 0.0, 0.0]);
    }
}
