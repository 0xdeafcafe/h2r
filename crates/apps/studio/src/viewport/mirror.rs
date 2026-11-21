//! One interpolated sample supplies both the viewport and its snapshot.
//! Object records retain the simulation's parent-relative coordinates.
use std::collections::{BTreeSet, HashMap};

use agent_proto::live::Object;
use live_mirror::Sample;
use map::TagId;
use scenario_edit::geometry::{
    self, Bounds, Camera, Gameplay, Vec3, add, cross, dot, scale, sub, transform,
};
use scenario_edit::layers::LayerId;
use scenario_edit::render::{Instance, object_light};
use scenario_edit::{BlockKind, Session};

use crate::{App, theme};

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
    /// One camera target does not need a pose table for the whole sample.
    pub(crate) fn unit_position(sample: &Sample, unit: &Object) -> Option<Vec3> {
        let mut pose = Pose::of(unit);
        let mut next = unit.parent;
        for _ in 0..sample.objects.len() {
            let Some(id) = next else {
                return Some(pose.position);
            };
            let parent = sample.objects.iter().find(|o| o.id == id)?;
            pose = Pose::of(parent).child(pose);
            next = parent.parent;
        }
        next.is_none().then_some(pose.position)
    }

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

    pub(crate) fn sphere(&self, id: u32, offset: Vec3, radius: f32) -> Option<(Vec3, f32)> {
        let pose = self.0.get(&id).copied().flatten()?;
        let center = add(pose.position, scale(pose.direction(offset), pose.scale));
        let radius = radius * pose.scale.abs();
        (radius > 0.0 && radius.is_finite()).then_some((center, radius))
    }

    pub(crate) fn position(&self, id: u32) -> Option<Vec3> {
        Some(self.0.get(&id).copied().flatten()?.position)
    }
}

/// CPU picking also needs the bounds of definitions first seen in the stream.
#[derive(Debug, Default)]
pub(crate) struct ModelBounds {
    world: Option<u64>,
    entries: HashMap<TagId, Option<Bounds>>,
}

impl ModelBounds {
    pub(crate) fn prepare(&mut self, session: &Session, sample: &Sample) {
        if self.world != Some(session.world_rev()) {
            self.world = Some(session.world_rev());
            self.entries.clear();
        }
        let mut missing = Vec::new();
        for tag in sample.objects.iter().filter_map(|o| o.tag.map(TagId)) {
            let std::collections::hash_map::Entry::Vacant(entry) = self.entries.entry(tag) else {
                continue;
            };
            // Composite placement art includes attached children. Live children
            // have their own records, so use the bare model for the parent.
            let mesh = session.scene().and_then(|s| {
                if s.composite_objects.contains(&tag) {
                    return None;
                }
                s.models.get(*s.objects.get(&tag)?)
            });
            let bounds = mesh.map(|m| m.bounds).filter(|b| !b.is_empty());
            entry.insert(bounds);
            if bounds.is_none() {
                missing.push(tag);
            }
        }
        if !missing.is_empty()
            && let Some(source) = session.source()
        {
            self.load_streamed(source, missing);
        }
    }

    fn load_streamed(&mut self, source: &scenario_edit::MapSource, tags: Vec<TagId>) {
        let Ok(mut map) = source.open() else { return };
        let Ok(ctx) = tags::object::Context::read(&mut map) else {
            return;
        };
        let Ok(local) = source.open() else { return };
        let mut files = vec![(map::ResourceFile::Local, local)];
        files.extend(
            geometry::shared_sources(source)
                .into_iter()
                .filter_map(|(kind, source)| Some((kind, source.open().ok()?))),
        );
        let mut raw = geometry::RawFiles::new(files);
        for tag in tags {
            let Some(model) = scenario_edit::scene::render_model_of(&ctx, tag) else {
                continue;
            };
            let Ok(model) = tags::model::RenderModelGeometry::read(&mut map, model, &mut raw)
            else {
                continue;
            };
            let triangles = model
                .default_sections()
                .into_iter()
                .filter_map(|index| model.sections.get(index))
                .flat_map(|section| {
                    section
                        .geometry
                        .parts
                        .iter()
                        .filter(|p| p.is_drawn())
                        .flat_map(move |part| section.geometry.part_triangles(part))
                        .filter_map(move |tri| {
                            if tri[0] == tri[1] || tri[1] == tri[2] || tri[2] == tri[0] {
                                return None;
                            }
                            let [a, b, c] =
                                tri.map(|i| section.vertices.positions.get(usize::from(i)));
                            Some([*a?, *b?, *c?])
                        })
                });
            self.entries.insert(tag, streamed_bounds(triangles));
        }
    }

    pub(crate) fn sphere(&self, tag: Option<u32>) -> (Vec3, f32) {
        self.bounds(tag)
            .map_or(([0.0; 3], geometry::MARKER_RADIUS), |b| {
                (b.center(), b.radius())
            })
    }

    fn bounds(&self, tag: Option<u32>) -> Option<Bounds> {
        self.entries.get(&TagId(tag?)).copied().flatten()
    }
}

/// The renderer rejects the entire bare mesh when its drawable triangles
/// exceed this limit. Both picking and the fallback glyph use this result.
fn streamed_bounds(triangles: impl IntoIterator<Item = [Vec3; 3]>) -> Option<Bounds> {
    let mut bounds = Bounds::EMPTY;
    for (i, triangle) in triangles.into_iter().enumerate() {
        if i == scenario_edit::scene::MAX_MODEL_TRIANGLES {
            return None;
        }
        for point in triangle {
            bounds.add(point);
        }
    }
    (!bounds.is_empty()).then_some(bounds)
}

/// The interpolator can move an object within one tick. Compare its displayed
/// transforms before reusing a scene, and release the old scene when unpaired.
#[derive(Default)]
pub(super) struct SceneCache {
    scene: std::sync::Arc<Scene>,
    inputs: Option<(u64, u64, u64, Option<u32>, u32)>,
    objects: Vec<Object>,
    players: Vec<agent_proto::live::Player>,
}

impl SceneCache {
    pub(super) fn shown(&mut self, app: &App) -> (std::sync::Arc<Scene>, bool) {
        let mut changed = false;
        let sample =
            app.pairing.shown.as_ref().filter(|_| {
                app.layers.visible(LayerId::Live) && crate::play::select::matching(app)
            });
        if let Some(sample) = sample {
            let inputs = Some((
                app.session.world_rev(),
                app.session.markers_rev(),
                sample.epoch,
                crate::play::select::selected_object(app),
                placement_mask(app),
            ));
            if self.inputs != inputs
                || self.objects != sample.objects
                || self.players != sample.players
            {
                self.scene = std::sync::Arc::new(Scene::shown(app));
                changed = true;
                self.inputs = inputs;
                self.objects.clone_from(&sample.objects);
                self.players.clone_from(&sample.players);
            }
        } else if self.inputs.take().is_some() {
            self.scene = Default::default();
            changed = true;
            self.objects.clear();
            self.players.clear();
        }
        (self.scene.clone(), changed)
    }
}

#[derive(Default)]
pub(crate) struct Scene {
    pub(crate) instances: Vec<(TagId, Instance)>,
    pub(crate) linked: BTreeSet<(BlockKind, usize)>,
    links: Vec<(Vec3, Vec3)>,
    players: Vec<Player>,
    glyphs: Vec<(Vec3, f32, bool)>,
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

pub(crate) struct Label {
    position: Vec3,
    text: String,
    color: Color,
}

pub(crate) fn gameplay_labels(app: &App) -> Vec<Label> {
    if !app.layers.visible(LayerId::Markers) {
        return Vec::new();
    }
    let scenario = app.session.scenario();
    app.session
        .markers()
        .iter()
        .filter(|m| app.layers.placement(m.block))
        .filter_map(|marker| {
            let game = Gameplay::of(scenario, marker);
            let text = game.label()?;
            let color = theme::placement_color(marker.block, game.team);
            Some(Label {
                position: marker.position,
                text,
                color: color.map(|c| (255.0 * c).round() as u8),
            })
        })
        .collect()
}

fn projected_labels<'a>(
    cam: &Camera,
    [w, h]: [f32; 2],
    labels: &'a [Label],
) -> impl Iterator<Item = Mark<'a>> {
    let eye = cam.eye();
    let matrix = cam.view_proj(w / h);
    labels.iter().filter_map(move |label| {
        let delta = sub(label.position, eye);
        if dot(delta, delta) > theme::LABEL_DISTANCE.powi(2) {
            return None;
        }
        let [x, y, z] = label.position;
        let [cx, cy, cz, cw] = transform(&matrix, [x, y, z, 1.0]);
        if !(cw > 0.0 && (0.0..=cw).contains(&cz) && cx.abs() <= cw && cy.abs() <= cw) {
            return None;
        }
        let [dx, dy] = theme::LABEL_OFFSET;
        let at = [
            (1.0 + cx / cw) * w * 0.5 + dx,
            (1.0 - cy / cw) * h * 0.5 + dy,
        ];
        Some(Mark::Text(at, &label.text, label.color))
    })
}

pub(crate) fn draw_labels(ui: &dear_imgui_rs::Ui, cam: &Camera, rect: [f32; 4], labels: &[Label]) {
    draw_marks(ui, rect, projected_labels(cam, [rect[2], rect[3]], labels));
}

pub(crate) fn snapshot_labels(rgba: &mut [u8], size: [u32; 2], cam: &Camera, labels: &[Label]) {
    paint_marks(rgba, size, projected_labels(cam, size.map(|n| n as f32), labels));
}

/// Only placements whose block still has the game's index layout can link.
fn placement_mask(app: &App) -> u32 {
    scenario_edit::layout::BLOCKS
        .iter()
        .filter_map(|def| {
            let kind = u32::try_from(def.object_type?).ok()?;
            app.pairing
                .placement_matches(&app.session, def.kind)
                .then_some(1 << kind)
        })
        .fold(0, |a, b| a | b)
}

impl Scene {
    pub(crate) fn shown(app: &App) -> Self {
        let Some(sample) = app.pairing.shown.as_ref() else {
            return Self::default();
        };
        if !app.layers.visible(LayerId::Live) || !crate::play::select::matching(app) {
            return Self::default();
        }
        let mut bounds = app.pairing.selection.bounds.borrow_mut();
        bounds.prepare(&app.session, sample);
        Self::new(
            sample,
            &app.session,
            &bounds,
            crate::play::select::selected_object(app),
            placement_mask(app),
        )
    }

    fn new(
        sample: &Sample,
        session: &Session,
        bounds: &ModelBounds,
        selected_object: Option<u32>,
        linked_kinds: u32,
    ) -> Self {
        let poses = WorldPoses::new(sample);
        let markers: HashMap<_, _> = session
            .markers()
            .iter()
            .filter_map(|m| {
                let kind = scenario_edit::layout::block(m.block).object_type?;
                (linked_kinds & (1 << kind) != 0).then_some(((kind, m.index), m))
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
            let selected_here = Some(object.id) == selected_object
                || placed.is_some_and(|m| selected.contains(&(m.block, m.index)));
            if bounds.bounds(object.tag).is_none() {
                scene.glyphs.push((
                    pose.position,
                    geometry::MARKER_RADIUS * pose.scale.abs(),
                    selected_here,
                ));
            }
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
                    color: theme::team_color(i64::from(player.team)),
                    body: object.body,
                    shield: object.shield,
                });
            }
        }
        scene
    }

    /// Pixel coordinates relative to the viewport, at this camera and size.
    fn annotations<'a>(
        &'a self,
        camera: &'a Camera,
        size: [f32; 2],
    ) -> impl Iterator<Item = Mark<'a>> {
        let project = move |p| {
            camera
                .project(p, size[0] / size[1])
                .map(|q| [q[0] * size[0], q[1] * size[1]])
                .filter(|q| q.iter().all(|v| v.is_finite()))
        };
        let glyphs = self
            .glyphs
            .iter()
            .flat_map(move |&(center, radius, selected)| {
                let color = if selected { theme::ACCENT } else { theme::DIM };
                let color = [color[0], color[1], color[2]].map(|c| (c * 255.0).round() as u8);
                (0..3).filter_map(move |axis| {
                    let mut extent = [0.0; 3];
                    extent[axis] = radius;
                    let a = project(sub(center, extent))?;
                    let b = project(add(center, extent))?;
                    Some(Mark::Line(a, b, color, 1.0))
                })
            });
        let links = self.links.iter().filter_map(move |&(a, b)| {
            Some(Mark::Line(project(a)?, project(b)?, [190, 176, 140], 1.0))
        });
        let players = self.players.iter().filter_map(move |p| {
            let pos = project(p.pos)?;
            let aim = project(add(p.pos, scale(p.aim, 2.5)))
                .map(|end| Mark::Line(pos, end, p.color, 2.0));
            let origin = [pos[0] + 7.0, pos[1] - 25.0];
            let label = Some(Mark::Text(origin, &p.label, p.color));
            let bar = |offset: f32, value: f32, color| {
                let a = [origin[0], origin[1] + offset];
                let track = Mark::Line(a, [a[0] + 38.0, a[1]], [40, 44, 48], 3.0);
                let fill = (value > 0.0).then(|| {
                    Mark::Line(a, [a[0] + 38.0 * value.min(1.0), a[1]], color, 3.0)
                });
                [Some(track), fill]
            };
            let [st, sf] = bar(18.0, p.shield, [96, 194, 255]);
            let [bt, bf] = bar(22.0, p.body, [112, 220, 134]);
            Some([aim, label, st, sf, bt, bf].into_iter().flatten())
        }).flatten();
        glyphs.chain(links).chain(players)
    }

    pub(super) fn overlay(&self, ui: &dear_imgui_rs::Ui, cam: &Camera, rect: [f32; 4]) {
        draw_marks(ui, rect, self.annotations(cam, [rect[2], rect[3]]));
    }

    pub(crate) fn snapshot(&self, rgba: &mut [u8], size: [u32; 2], cam: &Camera) {
        paint_marks(rgba, size, self.annotations(cam, size.map(|n| n as f32)));
    }
}

fn draw_marks<'a>(
    ui: &dear_imgui_rs::Ui,
    rect: [f32; 4],
    marks: impl IntoIterator<Item = Mark<'a>>,
) {
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
    for mark in marks {
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

fn paint_marks<'a>(rgba: &mut [u8], size: [u32; 2], marks: impl IntoIterator<Item = Mark<'a>>) {
    let mut canvas = Pixels { rgba, size };
    for mark in marks {
        match mark {
            Mark::Line(a, b, c, thickness) => canvas.line(a, b, thickness, c),
            Mark::Text(a, text, c) => {
                canvas.text([a[0] + 1.0, a[1] + 1.0], text, [0; 3]);
                canvas.text(a, text, c);
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
    fn oversized_streamed_mesh_uses_the_pick_sphere_and_glyph_fallback() {
        let triangle = [[7.0, 0.0, 0.0], [8.0, 0.0, 0.0], [7.0, 1.0, 0.0]];
        let limit = scenario_edit::scene::MAX_MODEL_TRIANGLES;
        assert!(streamed_bounds(std::iter::repeat_n(triangle, limit)).is_some());
        let rejected = streamed_bounds(std::iter::repeat_n(triangle, limit + 1));
        assert!(rejected.is_none());
        let mut bounds = ModelBounds::default();
        bounds.entries.insert(TagId(42), rejected);
        assert_eq!(bounds.sphere(Some(42)), ([0.0; 3], geometry::MARKER_RADIUS));
        let sample = Sample {
            epoch: 1,
            tick: 1,
            stale: false,
            players: vec![],
            objects: vec![Object {
                id: 1,
                tag: Some(42),
                scale: 2.0,
                ..Default::default()
            }],
        };
        let scene = Scene::new(&sample, &Session::default(), &bounds, None, 0);
        assert_eq!(
            scene.glyphs,
            vec![([0.0; 3], geometry::MARKER_RADIUS * 2.0, false)]
        );
    }

    #[test]
    fn gameplay_labels_are_culled_by_distance_and_viewport() {
        let cam = Camera {
            target: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            distance: 10.0,
            ..Default::default()
        };
        let label = |x, y, text: &str| Label {
            position: [x, y, 0.0],
            text: text.to_owned(),
            color: [255; 3],
        };
        let labels = [
            label(0.0, 0.0, "in view"),
            label(-theme::LABEL_DISTANCE - 1.0, 0.0, "far"),
            label(11.0, 0.0, "behind camera"),
            label(0.0, 30.0, "offscreen"),
        ];
        let visible: Vec<_> = projected_labels(&cam, [256.0; 2], &labels).collect();
        assert!(matches!(&visible[..], [Mark::Text(_, "in view", _)]));
    }

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

        let scene = Scene::new(&sample, &session, &ModelBounds::default(), None, u32::MAX);
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
