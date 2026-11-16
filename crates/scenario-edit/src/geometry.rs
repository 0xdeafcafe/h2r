//! What the viewport draws and how the mouse maps into the world: the
//! structure BSP as triangles, a marker per placement, the camera, picking
//! and plane dragging. All coordinates are game units with z up.
//!
//! The BSP walk is the same as `cli::live_geometry::bsp_triangles` (clusters
//! as stored, instanced geometry placed); it is repeated here in a few lines
//! rather than pulling the CLI's dependencies into the editor.
use crate::layout::{BlockKind, Role};
use crate::save::MapSource;
use crate::scenario::{Scenario, Value};
use map::{Map, RawPointer, ResourceFile, TagId};
use std::io::{Read, Seek};
use tags::geometry::{RawSource, Section, TagView};
use tags::structure::StructureRender;

pub type Vec3 = [f32; 3];
pub type Mat4 = [[f32; 4]; 4];

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub fn normalize(a: Vec3) -> Vec3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 { scale(a, 1.0 / l) } else { a }
}

/// One BSP triangle with a face normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tri {
    pub p: [Vec3; 3],
    pub n: Vec3,
}

/// Axis-aligned bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}
impl Bounds {
    pub const EMPTY: Self = Self {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    pub fn add(&mut self, p: Vec3) {
        for (k, v) in p.into_iter().enumerate() {
            self.min[k] = self.min[k].min(v);
            self.max[k] = self.max[k].max(v);
        }
    }
    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0]
    }
    pub fn center(&self) -> Vec3 {
        scale(add(self.min, self.max), 0.5)
    }
    pub fn radius(&self) -> f32 {
        dot(sub(self.max, self.min), sub(self.max, self.min)).sqrt() * 0.5
    }
}

/// Reads raw section data from the map itself and its shared caches.
pub struct RawFiles<R> {
    files: Vec<(ResourceFile, Map<R>)>,
}
impl<R: Read + Seek> RawFiles<R> {
    pub fn new(files: Vec<(ResourceFile, Map<R>)>) -> Self {
        Self { files }
    }
}
impl<R: Read + Seek> RawSource for RawFiles<R> {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        let (_, map) = self
            .files
            .iter_mut()
            .find(|(f, _)| *f == p.file)
            .ok_or(tags::Error::MissingResource(p.file))?;
        Ok(map.read_file_range(p.offset.into(), size.into())?)
    }
}

/// Open the shared caches next to a map: siblings in the same directory for
/// a file, other maps on the same disc for a disc map. Missing ones are
/// skipped; sections that need them then fail to load and are counted.
pub fn shared_sources(source: &MapSource) -> Vec<(ResourceFile, MapSource)> {
    let names = [
        (ResourceFile::MainMenu, "mainmenu"),
        (ResourceFile::Shared, "shared"),
        (ResourceFile::SinglePlayerShared, "single_player_shared"),
    ];
    match source.region {
        Some(_) => {
            let maps = MapSource::disc_maps(&source.path).unwrap_or_default();
            names
                .iter()
                .filter_map(|(f, n)| {
                    let (_, s) = maps.iter().find(|(m, _)| m.eq_ignore_ascii_case(n))?;
                    Some((*f, s.clone()))
                })
                .collect()
        }
        None => names
            .iter()
            .filter_map(|(f, n)| {
                let p = source.path.with_file_name(format!("{n}.map"));
                p.is_file().then(|| (*f, MapSource::file(p)))
            })
            .collect(),
    }
}

/// The scenario's structure BSP tags (`scnr+0x210`, stride 0x44, tag ID at
/// `+0x14`; docs/live-art.md).
pub fn scenario_bsps<R: Read + Seek>(map: &mut Map<R>) -> crate::Result<Vec<TagId>> {
    let scenario = map
        .scenario()
        .ok_or(crate::Error::Invalid("no scenario"))?
        .clone();
    let location = scenario
        .location
        .ok_or(crate::Error::Invalid("no scenario payload"))?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    Ok(view
        .block(view.at(0, 0x218)?, 0x210, 0x44)?
        .into_iter()
        .map(|b| TagId(u32::from_le_bytes([b[0x14], b[0x15], b[0x16], b[0x17]])))
        .collect())
}

/// Every drawn triangle of the scenario's BSPs, and how many sections failed.
pub fn world_triangles<R: Read + Seek>(
    map: &mut Map<R>,
    raw: &mut dyn RawSource,
) -> crate::Result<(Vec<Tri>, usize)> {
    let mut out = Vec::new();
    let mut failed = 0;
    for id in scenario_bsps(map)? {
        match StructureRender::read(map, id) {
            Ok(bsp) => failed += bsp_triangles(&bsp, raw, &mut out),
            Err(_) => failed += 1,
        }
    }
    Ok((out, failed))
}

fn bsp_triangles(bsp: &StructureRender, raw: &mut dyn RawSource, out: &mut Vec<Tri>) -> usize {
    let materials = Some(bsp.material_count as usize);
    let mut failed = 0;
    let push = |s: &Section, place: &dyn Fn(Vec3) -> Vec3, out: &mut Vec<Tri>| -> bool {
        let Ok(v) = s.vertices(None) else {
            return false;
        };
        if v.positions.len() != s.vertex_count() {
            return false;
        }
        let placed: Vec<Vec3> = v.positions.iter().map(|&p| place(p)).collect();
        for part in s.parts.iter().filter(|p| p.is_drawn()) {
            for t in s.part_triangles(part) {
                let p = t.map(|i| placed[usize::from(i)]);
                let n = normalize(cross(sub(p[1], p[0]), sub(p[2], p[0])));
                out.push(Tri { p, n });
            }
        }
        true
    };
    for c in &bsp.clusters {
        match c.geometry.load(raw, materials) {
            Ok(Some(s)) => failed += usize::from(!push(&s, &|p| p, out)),
            Ok(None) => {}
            Err(_) => failed += 1,
        }
    }
    let defs: Vec<Option<Section>> = bsp
        .instance_definitions
        .iter()
        .map(|d| {
            d.geometry.load(raw, materials).unwrap_or_else(|_| {
                failed += 1;
                None
            })
        })
        .collect();
    for inst in &bsp.instances {
        let Some(Some(s)) = usize::try_from(inst.definition)
            .ok()
            .and_then(|i| defs.get(i))
        else {
            continue;
        };
        if !inst.is_rigid() || inst.scale <= 0.0 {
            failed += 1;
            continue;
        }
        failed += usize::from(!push(s, &|p| inst.transform(p), out));
    }
    failed
}

/// Rotation from yaw, pitch, roll (radians): `Rz(yaw) · Ry(−pitch) · Rx(roll)`
/// as docs/live-art.md established for placements. Columns are the forward,
/// left and up axes.
pub fn rotation(ypr: Vec3) -> [Vec3; 3] {
    let (sy, cy) = ypr[0].sin_cos();
    let (sp, cp) = ypr[1].sin_cos();
    let (sr, cr) = ypr[2].sin_cos();
    let forward = [cy * cp, sy * cp, sp];
    let left = {
        let y = [0.0, cr, sr]; // Rx(r)·ŷ
        let y = [y[0] * cp - y[2] * sp, y[1], y[0] * sp + y[2] * cp]; // Ry(−p)
        [y[0] * cy - y[1] * sy, y[0] * sy + y[1] * cy, y[2]] // Rz(y)
    };
    let up = cross(forward, left);
    [forward, left, up]
}

/// One placement to draw and pick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marker {
    pub block: BlockKind,
    pub index: usize,
    pub position: Vec3,
    /// Forward, left, up.
    pub axes: [Vec3; 3],
}

/// Every element with a position, in block order.
pub fn markers(scenario: &Scenario) -> Vec<Marker> {
    let mut out = Vec::new();
    for b in &scenario.blocks {
        let Some(pos) = b.def.role(Role::Position) else {
            continue;
        };
        let rot = b.def.role(Role::Rotation);
        let facing = b.def.role(Role::Facing);
        for index in 0..b.len() {
            let get = |name| scenario.get(b.def.kind, index, name).ok();
            let Some(Value::Vec3(position)) = get(pos.name) else {
                continue;
            };
            if !position.iter().all(|x| x.is_finite()) {
                continue;
            }
            let ypr = match (rot, facing) {
                (Some(r), _) => get(r.name).and_then(Value::as_vec3).unwrap_or([0.0; 3]),
                (None, Some(f)) => [get(f.name).and_then(Value::as_f32).unwrap_or(0.0), 0.0, 0.0],
                _ => [0.0; 3],
            };
            let ypr = if ypr.iter().all(|x| x.is_finite()) {
                ypr
            } else {
                [0.0; 3]
            };
            out.push(Marker {
                block: b.def.kind,
                index,
                position,
                axes: rotation(ypr),
            });
        }
    }
    out
}

/// Marker colour per block (linear RGB).
pub fn block_color(kind: BlockKind) -> Vec3 {
    use BlockKind::*;
    match kind {
        Scenery => [0.55, 0.55, 0.45],
        Vehicles => [0.2, 0.5, 1.0],
        Equipment => [0.2, 0.9, 0.9],
        Weapons => [1.0, 0.6, 0.1],
        Machines => [0.6, 0.4, 0.9],
        SoundScenery => [0.9, 0.9, 0.2],
        Crates => [0.6, 0.35, 0.2],
        StartingLocations => [0.2, 1.0, 0.3],
        NetgameFlags => [1.0, 0.2, 0.6],
        NetgameEquipment => [1.0, 0.25, 0.2],
        StartingEquipment => [0.7, 0.7, 0.7],
    }
}

/// Marker radius in world units (a standing player is under one unit tall).
pub const MARKER_RADIUS: f32 = 0.25;

/// A marker's triangles: a diamond stretched along its forward axis, so the
/// facing reads at a glance.
pub fn marker_triangles(m: &Marker) -> Vec<[Vec3; 3]> {
    let r = MARKER_RADIUS;
    let [f, l, u] = m.axes;
    let at = |v: Vec3, s: f32| add(m.position, scale(v, s));
    let tip = at(f, 2.0 * r);
    let back = at(f, -r);
    let ring = [at(l, r), at(u, r), at(l, -r), at(u, -r)];
    let mut out = Vec::new();
    for i in 0..4 {
        let a = ring[i];
        let b = ring[(i + 1) % 4];
        out.push([tip, b, a]);
        out.push([back, a, b]);
    }
    out
}

/// A ray from the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}
impl Ray {
    /// Distance along the ray to a sphere, if hit.
    pub fn sphere(&self, center: Vec3, radius: f32) -> Option<f32> {
        let oc = sub(self.origin, center);
        let b = dot(oc, self.dir);
        let c = dot(oc, oc) - radius * radius;
        let disc = b * b - c;
        if disc < 0.0 {
            return None;
        }
        let t = -b - disc.sqrt();
        let t = if t >= 0.0 { t } else { -b + disc.sqrt() };
        (t >= 0.0).then_some(t)
    }
    /// Where the ray meets the plane through `point` with `normal`.
    pub fn plane(&self, point: Vec3, normal: Vec3) -> Option<Vec3> {
        let d = dot(self.dir, normal);
        if d.abs() < 1e-6 {
            return None;
        }
        let t = dot(sub(point, self.origin), normal) / d;
        (t >= 0.0).then(|| add(self.origin, scale(self.dir, t)))
    }
    /// Möller-Trumbore distance to a triangle.
    pub fn triangle(&self, t: &[Vec3; 3]) -> Option<f32> {
        let e1 = sub(t[1], t[0]);
        let e2 = sub(t[2], t[0]);
        let p = cross(self.dir, e2);
        let det = dot(e1, p);
        if det.abs() < 1e-9 {
            return None;
        }
        let inv = 1.0 / det;
        let s = sub(self.origin, t[0]);
        let u = dot(s, p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = cross(s, e1);
        let v = dot(self.dir, q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let d = dot(e2, q) * inv;
        (d > 0.0).then_some(d)
    }
}

/// The nearest marker the ray hits.
pub fn pick(markers: &[Marker], ray: &Ray) -> Option<(BlockKind, usize)> {
    markers
        .iter()
        .filter_map(|m| ray.sphere(m.position, MARKER_RADIUS * 1.5).map(|t| (t, m)))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, m)| (m.block, m.index))
}

/// The nearest BSP surface the ray hits, for dropping a placement.
pub fn surface(tris: &[Tri], ray: &Ray) -> Option<Vec3> {
    tris.iter()
        .filter_map(|t| ray.triangle(&t.p))
        .min_by(f32::total_cmp)
        .map(|t| add(ray.origin, scale(ray.dir, t)))
}

/// An orbit camera with a fly mode: it looks at `target` from `distance`
/// along yaw and pitch; flying moves the target with the eye.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            target: [0.0; 3],
            yaw: 0.8,
            pitch: 0.6,
            distance: 30.0,
            fov_y: 60f32.to_radians(),
        }
    }
}
impl Camera {
    /// Frame a bounding box.
    pub fn frame(bounds: &Bounds) -> Self {
        let mut c = Self::default();
        if !bounds.is_empty() {
            c.target = bounds.center();
            c.distance = (bounds.radius() * 1.8).max(5.0);
        }
        c
    }
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        // Looking down at the target from above when pitch > 0.
        [-cy * cp, -sy * cp, -sp]
    }
    pub fn eye(&self) -> Vec3 {
        sub(self.target, scale(self.forward(), self.distance))
    }
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw += dx;
        self.pitch = (self.pitch + dy).clamp(-1.5, 1.5);
    }
    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(0.5, 5000.0);
    }
    /// Move the target in camera space: right, up (world z), forward.
    pub fn fly(&mut self, right: f32, up: f32, forward: f32) {
        let f = self.forward();
        let r = normalize(cross(f, [0.0, 0.0, 1.0]));
        self.target = add(
            self.target,
            add(scale(r, right), add(scale(f, forward), [0.0, 0.0, up])),
        );
    }
    /// Pan in the view plane, in pixels scaled to world units at the target.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        let units = 2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_h.max(1.0);
        let f = self.forward();
        let r = normalize(cross(f, [0.0, 0.0, 1.0]));
        let u = cross(r, f);
        self.target = add(
            self.target,
            add(scale(r, -dx * units), scale(u, dy * units)),
        );
    }
    fn near_far(&self) -> (f32, f32) {
        (
            (self.distance * 0.01).max(0.05),
            self.distance * 4.0 + 2000.0,
        )
    }
    /// Column-major view-projection for wgpu clip space (depth 0..1).
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let eye = self.eye();
        let f = self.forward();
        let r = normalize(cross(f, [0.0, 0.0, 1.0]));
        let u = cross(r, f);
        let view: Mat4 = [
            [r[0], u[0], -f[0], 0.0],
            [r[1], u[1], -f[1], 0.0],
            [r[2], u[2], -f[2], 0.0],
            [-dot(r, eye), -dot(u, eye), dot(f, eye), 1.0],
        ];
        let (n, fa) = self.near_far();
        let t = 1.0 / (self.fov_y * 0.5).tan();
        let proj: Mat4 = [
            [t / aspect.max(1e-3), 0.0, 0.0, 0.0],
            [0.0, t, 0.0, 0.0],
            [0.0, 0.0, fa / (n - fa), -1.0],
            [0.0, 0.0, n * fa / (n - fa), 0.0],
        ];
        mul(&proj, &view)
    }
    /// The ray through a viewport point given in 0..1 (x right, y down).
    pub fn ray(&self, u: f32, v: f32, aspect: f32) -> Ray {
        let f = self.forward();
        let r = normalize(cross(f, [0.0, 0.0, 1.0]));
        let up = cross(r, f);
        let t = (self.fov_y * 0.5).tan();
        let x = (2.0 * u - 1.0) * t * aspect;
        let y = (1.0 - 2.0 * v) * t;
        Ray {
            origin: self.eye(),
            dir: normalize(add(f, add(scale(r, x), scale(up, y)))),
        }
    }
    /// Project a world point to 0..1 viewport coordinates, if in front.
    pub fn project(&self, p: Vec3, aspect: f32) -> Option<[f32; 2]> {
        let m = self.view_proj(aspect);
        let c = transform(&m, [p[0], p[1], p[2], 1.0]);
        (c[3] > 0.0).then(|| [(c[0] / c[3] + 1.0) * 0.5, (1.0 - c[1] / c[3]) * 0.5])
    }
}

pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}
pub fn transform(m: &Mat4, v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (r, o) in out.iter_mut().enumerate() {
        *o = (0..4).map(|k| m[k][r] * v[k]).sum();
    }
    out
}
