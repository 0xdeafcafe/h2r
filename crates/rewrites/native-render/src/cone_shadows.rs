//! E3 lighting's shadows from local lights: which cones cast, the
//! projection of each one's tile, and what it casts on
//! (docs/graphics/e3-lighting.md, "Shadows from local lights").
//!
//! - **Cones cast; spheres do not.** A light casts when its effective
//!   volume is a frustum (`LightVolume::Projected`, shape code 2) whose
//!   apex, axis and far extents make a projection ([`Cone::of`]).
//! - **The no-shadow flag is ignored for cones.** Definition bit 3
//!   ([`NO_SHADOW`]) is the content's "no stencil shadow"; E3 casts from a
//!   cone that has it. A light with a supplied tile keeps it
//!   (`light_tiles_gpu`) and is [`Source::Guest`];
//!   the rest are the renderer's own ([`Source::Renderer`]).
//! - **Four slots** ([`SHADOW_CAP`]) a frame, shared between the views of
//!   split screen as `light_choice` shares its sixteen ([`slot_caps`]).
//!   Only a view's lit lights (`light_choice::choose`) are candidates. In
//!   a view, the view's own flashlight (definition bit 5, the light
//!   `light_choice` places first) takes the first slot, then the lights
//!   the guest shadowed, then the rest in the lit order, which is score
//!   order. The guest's shadowed lights keep their shadow past the cap:
//!   the guest draws them whatever the renderer chooses.
//! - **Tiles.** One slot of the local lights' atlas each ([`ATLAS_EDGE`],
//!   four [`SLOT_EDGE`] squares): `crate::render_scale::shadow_scale` times the
//!   guest's 512-square level, at most [`SLOT_EDGE`]; the flashlight's is
//!   [`FLASHLIGHT_EDGE`] ([`tile_edge`]).
//! - **Casters and receivers** are the view's instances whose bounding
//!   sphere meets the cone ([`Cone::meets`]), and the structure parts of
//!   the view's clusters that do (`sun_casters::WorldReceivers::cone_parts`).
//!   The light's owner does not cast in its own light: the flashlight's
//!   holder stands at the apex. An instance whose sphere holds the apex
//!   does not cast either; it would cover the whole tile.
//!
//! This module is pure. `cone_shadows_gpu` draws the tiles and the
//! receivers' shadow term.

use frame_scene::{FrameScene, InstanceKey, Light, LightVolume, ViewKey};

use crate::light_choice::{LightChoice, own};

/// Shadowed lights in a frame, shared by the views of split screen.
pub const SHADOW_CAP: usize = 4;
/// Definition flag bit 3: no shadow (`0x00031e02`).
pub const NO_SHADOW: u32 = 1 << 3;
/// The effective volume's shape code for a frustum.
pub const FRUSTUM: u16 = 2;
/// The guest's shadow level's edge (render target 9).
pub const GUEST_LEVEL: u32 = 512;
/// One slot of the local lights' atlas.
pub const SLOT_EDGE: u32 = 2048;
/// The flashlight's tile edge.
pub const FLASHLIGHT_EDGE: u32 = 2048;
/// The local lights' atlas: four slots in a 2x2 grid.
pub const ATLAS_EDGE: u32 = 2 * SLOT_EDGE;

/// A tile's edge for a render scale's `shadow_scale` factor.
pub fn tile_edge(factor: u32, flashlight: bool) -> u32 {
    if flashlight {
        return FLASHLIGHT_EDGE;
    }
    GUEST_LEVEL.saturating_mul(factor.max(2)).min(SLOT_EDGE)
}

/// Slot `slot`'s rectangle in the atlas, `x, y, width, height`, for a
/// tile of `edge`.
pub fn slot_rect(slot: usize, edge: u32) -> [u32; 4] {
    let edge = edge.min(SLOT_EDGE);
    let (col, row) = ((slot % 2) as u32, (slot / 2 % 2) as u32);
    [col * SLOT_EDGE, row * SLOT_EDGE, edge, edge]
}

/// Each of `views` distinct views' share of [`SHADOW_CAP`], in order: an
/// equal share, the remainder one each to the first views.
pub fn slot_caps(views: usize) -> Vec<usize> {
    if views == 0 {
        return Vec::new();
    }
    let (share, rest) = (SHADOW_CAP / views, SHADOW_CAP % views);
    (0..views).map(|v| share + usize::from(v < rest)).collect()
}

/// Why a lit cone has no tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConeRefusal {
    /// No candidate record, so no volume.
    NoVolume,
    /// A sphere, or a projected shape other than the frustum.
    Shape,
    /// Its apex, axis or extents are not finite, or make no frustum.
    Degenerate,
}

/// A frustum light's projection: a perspective from its apex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cone {
    pub apex: [f32; 3],
    /// Unit vectors: the axis, and the tile's x and y across it.
    pub axis: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    /// Half the width and height per unit of depth from the apex.
    pub slopes: [f32; 2],
    /// Depths from the apex along the axis.
    pub near: f32,
    pub far: f32,
}

impl Cone {
    /// The cone of `light`'s effective volume. The volume's origin is the
    /// frustum's apex (`0x000c1a80` sets it behind the light by the near
    /// extent over the slope); its axis has unit depth along the light's
    /// forward, its far centre is the light's position plus the axis
    /// times the far depth, and its far extents are half the width and
    /// height there.
    pub fn of(light: &Light) -> Result<Self, ConeRefusal> {
        let c = light.candidate.as_ref().ok_or(ConeRefusal::NoVolume)?;
        let Some(LightVolume::Projected {
            kind,
            origin,
            axis,
            near,
            far,
            far_extent,
        }) = c.volume
        else {
            return Err(ConeRefusal::Shape);
        };
        if kind != FRUSTUM {
            return Err(ConeRefusal::Shape);
        }
        let finite = |v: &[f32]| v.iter().all(|x| x.is_finite());
        if !finite(&origin)
            || !finite(&axis)
            || !finite(&c.position)
            || !finite(&c.up)
            || !finite(&far_extent)
            || !near.is_finite()
            || !far.is_finite()
        {
            return Err(ConeRefusal::Degenerate);
        }
        let d = normalize(axis).ok_or(ConeRefusal::Degenerate)?;
        let far_centre = add(c.position, scale(axis, far));
        let far_depth = dot(sub(far_centre, origin), d);
        let behind = dot(sub(c.position, origin), d);
        if far_depth <= 0.0 || behind < 0.0 || far_extent[0] <= 0.0 || far_extent[1] <= 0.0 {
            return Err(ConeRefusal::Degenerate);
        }
        let hint = sub(c.up, scale(d, dot(c.up, d)));
        let up = normalize(hint)
            .or_else(|| {
                let other = if d[2].abs() < 0.9 {
                    [0.0, 0.0, 1.0]
                } else {
                    [1.0, 0.0, 0.0]
                };
                normalize(sub(other, scale(d, dot(other, d))))
            })
            .ok_or(ConeRefusal::Degenerate)?;
        let right = cross(d, up);
        let near_depth = (behind + near.max(0.0)).max(far_depth * 1e-3).max(1e-3);
        if near_depth >= far_depth {
            return Err(ConeRefusal::Degenerate);
        }
        Ok(Self {
            apex: origin,
            axis: d,
            right,
            up,
            slopes: [far_extent[0] / far_depth, far_extent[1] / far_depth],
            near: near_depth,
            far: far_depth,
        })
    }

    /// World to tile clip, as rows applied to `[x, y, z, 1]`: x and y in
    /// `[-w, w]` across the cone, z / w in `[0, 1]` from the near to the
    /// far depth, w the depth from the apex.
    pub fn rows(&self) -> [[f32; 4]; 4] {
        let a = self.apex;
        let row =
            |v: [f32; 3], s: f32, off: f32| [v[0] * s, v[1] * s, v[2] * s, -dot(v, a) * s + off];
        let k = self.far / (self.far - self.near);
        [
            row(self.right, 1.0 / self.slopes[0], 0.0),
            row(self.up, 1.0 / self.slopes[1], 0.0),
            row(self.axis, k, -self.near * k),
            row(self.axis, 1.0, 0.0),
        ]
    }

    /// Whether a sphere meets the cone: it is inside every plane of the
    /// frustum or within its radius of one.
    pub fn meets(&self, centre: [f32; 3], radius: f32) -> bool {
        if !radius.is_finite() || !centre.iter().all(|v| v.is_finite()) {
            return false;
        }
        let q = sub(centre, self.apex);
        let depth = dot(q, self.axis);
        if depth < self.near - radius || depth > self.far + radius {
            return false;
        }
        [(self.right, self.slopes[0]), (self.up, self.slopes[1])]
            .into_iter()
            .all(|(across, slope)| {
                let side = dot(q, across).abs() - slope * depth;
                side <= radius * (1.0 + slope * slope).sqrt()
            })
    }

    /// Whether a sphere holds the apex.
    pub fn holds_apex(&self, centre: [f32; 3], radius: f32) -> bool {
        let q = sub(centre, self.apex);
        dot(q, q) <= radius * radius
    }

    /// World units per texel across the tile at `depth`, for a tile of
    /// `edge` texels.
    pub fn texel(&self, depth: f32, edge: u32) -> f32 {
        2.0 * self.slopes[0].max(self.slopes[1]) * depth / edge.max(1) as f32
    }
}

/// Who draws a shadowed light's tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// The guest shadowed it: its tile is the guest's shadow map, replayed.
    Guest,
    /// The renderer: its tile is a slot of the local lights' atlas.
    Renderer,
}

/// One shadowed light of a view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowedCone {
    /// Into `FrameScene::views`.
    pub view: u32,
    /// Into `FrameScene::lights`.
    pub light: u32,
    pub handle: u32,
    pub source: Source,
    /// The view's own flashlight.
    pub flashlight: bool,
    /// The content set its no-shadow flag.
    pub flagged_off: bool,
    pub cone: Cone,
    /// Its slot in the atlas, for [`Source::Renderer`].
    pub slot: Option<usize>,
}

impl ShadowedCone {
    /// Its tile's edge at a `shadow_scale` factor.
    pub fn edge(&self, factor: u32) -> u32 {
        tile_edge(factor, self.flashlight)
    }

    /// Its tile's rectangle in the atlas at a `shadow_scale` factor.
    pub fn rect(&self, factor: u32) -> Option<[u32; 4]> {
        self.slot.map(|s| slot_rect(s, self.edge(factor)))
    }
}

/// What one frame's choice of shadowed lights did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConeWork {
    /// Lights the renderer shadows itself, one tile each.
    pub cones_shadowed: u32,
    /// Of every shadowed light, those whose content flag was off.
    pub cones_flagged_off: u32,
    /// Views whose own flashlight holds a slot.
    pub flashlight_shadowed: u32,
    /// Lit cones the slots left unshadowed.
    pub shadows_over_cap: u32,
    /// Lit lights that would have cast but make no frustum
    /// ([`ConeRefusal::Degenerate`]); a sphere is not counted.
    pub cones_refused: u32,
}

/// The frame's shadowed lights, a view at a time in the scene's order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConeChoice {
    pub cones: Vec<ShadowedCone>,
    pub work: ConeWork,
}

impl ConeChoice {
    /// The renderer's own tiles, with their slots.
    pub fn renderer(&self) -> impl Iterator<Item = &ShadowedCone> {
        self.cones.iter().filter(|c| c.source == Source::Renderer)
    }

    /// Whether the renderer shadows `light` (into `FrameScene::lights`)
    /// in place of the guest's decision.
    pub fn shadows(&self, light: u32) -> bool {
        self.cones.iter().any(|c| c.light == light)
    }
}

/// The lights E3 shadows in `scene`, from `lit`, its choice of lit lights.
pub fn choose(scene: &FrameScene, lit: &LightChoice) -> ConeChoice {
    let mut keys: Vec<ViewKey> = Vec::new();
    for v in &scene.views {
        if !keys.contains(&v.key) {
            keys.push(v.key);
        }
    }
    let caps = slot_caps(keys.len());
    let mut out = ConeChoice::default();
    let mut slots = 0usize;
    // Each view key's first record's renderer slots, by light handle. A
    // second record of a view is chosen alike from its own lights, shares
    // the first's tiles and is counted once.
    let mut first_slots: Vec<(ViewKey, Vec<(u32, usize)>)> = Vec::new();
    for choice in &lit.views {
        let Some(view) = scene.views.get(choice.view as usize) else {
            continue;
        };
        let cap = keys
            .iter()
            .position(|k| *k == view.key)
            .map_or(0, |k| caps[k]);
        let earlier = first_slots.iter().position(|(k, _)| *k == view.key);
        let mut work = ConeWork::default();
        let cones = ranked(|i| scene.lights.get(i as usize), &choice.lit, &mut work);
        let mut taken = 0usize;
        let mut mine = Vec::new();
        for (rank, order, light, cone) in cones {
            let guest = guest_shadowed(light);
            if taken >= cap && !guest {
                work.shadows_over_cap += 1;
                continue;
            }
            taken += 1;
            let flashlight = rank == 0;
            let flagged_off = light.definition_flags & NO_SHADOW != 0;
            let source = if guest {
                Source::Guest
            } else {
                Source::Renderer
            };
            let slot = match (source, earlier) {
                (Source::Guest, _) => None,
                (Source::Renderer, Some(k)) => {
                    // The first record's tile of the same light; a light
                    // it did not shadow has none to share.
                    let found = first_slots[k]
                        .1
                        .iter()
                        .find(|(h, _)| *h == light.handle)
                        .map(|(_, s)| *s);
                    if found.is_none() {
                        continue;
                    }
                    found
                }
                (Source::Renderer, None) => {
                    slots += 1;
                    Some(slots - 1)
                }
            };
            work.cones_shadowed += u32::from(source == Source::Renderer);
            work.cones_flagged_off += u32::from(flagged_off);
            work.flashlight_shadowed += u32::from(flashlight);
            mine.push(ShadowedCone {
                view: choice.view,
                light: choice.lit[order],
                handle: light.handle,
                source,
                flashlight,
                flagged_off,
                cone,
                slot,
            });
        }
        if earlier.is_none() {
            let owned = mine
                .iter()
                .filter_map(|c| c.slot.map(|s| (c.handle, s)))
                .collect();
            first_slots.push((view.key, owned));
            let w = &mut out.work;
            w.cones_shadowed += work.cones_shadowed;
            w.cones_flagged_off += work.cones_flagged_off;
            w.flashlight_shadowed += work.flashlight_shadowed;
            w.shadows_over_cap += work.shadows_over_cap;
            w.cones_refused += work.cones_refused;
        }
        out.cones.extend(mine);
    }
    out
}

/// A view's lit cones in slot order, as `(rank, order, light, cone)`:
/// `lit` is its choice of lit lights (into `FrameScene::lights`, which
/// `light` reads), `order` the place in it. The view's own flashlight
/// ranks 0, a light the guest shadowed 1, the rest 2, each rank in the lit
/// order. A host light is left out, and a frustum that makes no
/// projection counted in `work.cones_refused`.
fn ranked<'a>(
    light: impl Fn(u32) -> Option<&'a Light>,
    lit: &[u32],
    work: &mut ConeWork,
) -> Vec<(u8, usize, &'a Light, Cone)> {
    let mut cones = Vec::new();
    for (order, &index) in lit.iter().enumerate() {
        let Some(light) = light(index) else {
            continue;
        };
        if light.host.is_some() {
            continue;
        }
        match Cone::of(light) {
            Ok(cone) => {
                let rank = if own(light) {
                    0
                } else if guest_shadowed(light) {
                    1
                } else {
                    2
                };
                cones.push((rank, order, light, cone));
            }
            Err(ConeRefusal::Degenerate) => work.cones_refused += 1,
            Err(_) => {}
        }
    }
    cones.sort_by_key(|(rank, order, ..)| (*rank, *order));
    cones
}

/// The lights one view shadows, as [`choose`] chooses them for the
/// view's first record: `lights` is the view's slice of
/// `FrameScene::lights` from index `first`, `lit` its choice
/// (`light_choice::choose_view`) and `slots` its share of [`SHADOW_CAP`]
/// ([`slot_share_of`]). Returns indices into `FrameScene::lights`, in slot
/// order, the guest's shadowed lights among them. The frame step asks
/// this of the lights it draws itself
/// (`render_bridge::native_lights::chosen`), before the scene is sealed.
pub fn view_shadows(lights: &[Light], first: u32, lit: &[u32], slots: usize) -> Vec<u32> {
    let light = |i: u32| i.checked_sub(first).and_then(|k| lights.get(k as usize));
    let mut work = ConeWork::default();
    let mut taken = 0usize;
    let mut out = Vec::new();
    for (_, order, light, _) in ranked(light, lit, &mut work) {
        if taken >= slots && !guest_shadowed(light) {
            continue;
        }
        taken += 1;
        out.push(lit[order]);
    }
    out
}

/// `key`'s share of [`SHADOW_CAP`] among `keys` (in the scene's order,
/// repeats ignored), as [`choose`] gives it; a key not in `keys` is
/// counted after them.
pub fn slot_share_of(keys: &[ViewKey], key: ViewKey) -> usize {
    let mut distinct: Vec<ViewKey> = Vec::new();
    for k in keys.iter().copied().chain([key]) {
        if !distinct.contains(&k) {
            distinct.push(k);
        }
    }
    let at = distinct.iter().position(|k| *k == key).unwrap_or(0);
    slot_caps(distinct.len())[at]
}

/// The guest ran `light`'s passes with its shadow (flag bit 2).
fn guest_shadowed(light: &Light) -> bool {
    light.guest_selected && light.shadowed()
}

/// The instances of `cone`'s view that cast into its tile: those whose
/// sphere meets the cone, but the light's owner and any whose sphere
/// holds the apex. In the view's order.
pub fn casters(scene: &FrameScene, cone: &ShadowedCone) -> Vec<InstanceKey> {
    let owner = scene
        .lights
        .get(cone.light as usize)
        .and_then(|l| l.candidate.as_ref())
        .and_then(|c| c.owner);
    let Some(view) = scene.views.get(cone.view as usize) else {
        return Vec::new();
    };
    scene
        .instances_of(view)
        .iter()
        .filter(|i| Some(i.key) != owner)
        .filter(|i| {
            let (centre, radius) = i.sphere;
            cone.cone.meets(centre, radius) && !cone.cone.holds_apex(centre, radius)
        })
        .map(|i| i.key)
        .collect()
}

/// The instances of `cone`'s view that receive its tile: those whose
/// sphere meets the cone, the owner included. In the view's order.
pub fn receivers(scene: &FrameScene, cone: &ShadowedCone) -> Vec<InstanceKey> {
    let Some(view) = scene.views.get(cone.view as usize) else {
        return Vec::new();
    };
    scene
        .instances_of(view)
        .iter()
        .filter(|i| cone.cone.meets(i.sphere.0, i.sphere.1))
        .map(|i| i.key)
        .collect()
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let n = dot(a, a).sqrt();
    (n.is_finite() && n > 1e-6).then(|| scale(a, 1.0 / n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light_choice::{self, FIRST_PERSON_LIGHT};
    use frame_scene::{
        InstanceInput, LightCandidate, NodeMatrix, RenderCamera, SceneBuilder, SceneId, Stamp,
        ViewKind,
    };

    fn camera() -> RenderCamera {
        RenderCamera {
            position: [0.0, 0.0, 0.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: std::f32::consts::FRAC_PI_2,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 100.0,
        }
    }

    fn view(player: u8) -> ViewKey {
        ViewKey {
            kind: ViewKind::Game,
            player,
            view: 0,
        }
    }

    /// A frustum light at `at` looking down -z: apex one unit above it,
    /// reaching ten units below it with half extents of five there, so
    /// its half slopes are 5 / 11.
    fn cone_light(index: u32, priority: f32, at: [f32; 3]) -> Light {
        Light {
            handle: 0x7000 << 16 | index,
            definition: 0xe700_0002,
            definition_flags: NO_SHADOW,
            candidate: Some(LightCandidate {
                position: at,
                forward: [0.0, 0.0, -1.0],
                up: [1.0, 0.0, 0.0],
                colour_a: [1.0; 3],
                colour_b: [1.0; 3],
                intensity: 1.0,
                bounds: ([at[0], at[1], at[2] - 5.0], 8.0),
                owner: None,
                definition_kind: FRUSTUM,
                volume: Some(LightVolume::Projected {
                    kind: FRUSTUM,
                    origin: [at[0], at[1], at[2] + 1.0],
                    axis: [0.0, 0.0, -1.0],
                    near: 0.0,
                    far: 10.0,
                    far_extent: [5.0, 5.0],
                }),
                view_eligible: Some(true),
                priority: Some(priority),
                weight: 1.0,
                gel_bitmap: None,
                gel_shader: None,
                constants: None,
            }),
            ..Default::default()
        }
    }

    fn sphere_light(index: u32, priority: f32) -> Light {
        let mut l = cone_light(index, priority, [0.0; 3]);
        let c = l.candidate.as_mut().unwrap();
        c.volume = Some(LightVolume::Sphere {
            origin: [0.0; 3],
            radius: 4.0,
        });
        c.definition_kind = 0;
        l
    }

    struct Scene {
        views: Vec<(u8, Vec<Light>, Vec<(u32, ([f32; 3], f32))>)>,
    }

    impl Scene {
        fn build(&self) -> FrameScene {
            let mut b = SceneBuilder::default();
            b.begin(SceneId::default());
            let nodes = [NodeMatrix::IDENTITY];
            for (player, lights, instances) in &self.views {
                b.open_view(view(*player), camera()).unwrap();
                for &(key, sphere) in instances {
                    b.push_instance(InstanceInput {
                        key: InstanceKey(key),
                        definition: 0xe500_0001,
                        model: 0xe600_0001,
                        nodes: &nodes,
                        sphere,
                        lighting: None,
                        defaults: None,
                    })
                    .unwrap();
                }
                b.add_light_candidates(lights.clone()).unwrap();
            }
            (*b.seal(Stamp::default()).unwrap()).clone()
        }
    }

    fn one_view(lights: Vec<Light>) -> FrameScene {
        Scene {
            views: vec![(0, lights, Vec::new())],
        }
        .build()
    }

    fn handles(s: &FrameScene, c: &ConeChoice) -> Vec<u32> {
        c.cones
            .iter()
            .map(|c| s.lights[c.light as usize].handle)
            .collect()
    }

    /// The design's synthetic check: a cone the content flagged off casts
    /// in E3, from a slot of the renderer's atlas, and a sphere does not.
    #[test]
    fn a_no_shadow_cone_casts_and_a_sphere_does_not() {
        let s = one_view(vec![
            cone_light(1, 4.0, [0.0, 0.0, 10.0]),
            sphere_light(2, 9.0),
        ]);
        let got = choose(&s, &light_choice::choose(&s));
        assert_eq!(handles(&s, &got), [0x7000_0001]);
        let c = got.cones[0];
        assert_eq!(
            (c.source, c.flagged_off, c.slot),
            (Source::Renderer, true, Some(0))
        );
        assert_eq!(
            got.work,
            ConeWork {
                cones_shadowed: 1,
                cones_flagged_off: 1,
                ..ConeWork::default()
            }
        );
        assert!(got.shadows(c.light));
    }

    /// Six lit cones: the four highest scores cast, the other two are
    /// over the cap, and the slots are 0 to 3 in that order.
    #[test]
    fn four_slots_by_score() {
        let lights: Vec<Light> = (0..6)
            .map(|k| cone_light(k, 1.0 + k as f32, [k as f32 * 20.0, 0.0, 10.0]))
            .collect();
        let s = one_view(lights);
        let got = choose(&s, &light_choice::choose(&s));
        assert_eq!(
            handles(&s, &got),
            [0x7000_0005, 0x7000_0004, 0x7000_0003, 0x7000_0002]
        );
        let slots: Vec<_> = got.cones.iter().map(|c| c.slot).collect();
        assert_eq!(slots, [Some(0), Some(1), Some(2), Some(3)]);
        assert_eq!(got.work.cones_shadowed, 4);
        assert_eq!(got.work.shadows_over_cap, 2);
    }

    /// The view's own flashlight holds the first slot whatever its score,
    /// and its tile is the flashlight's edge.
    #[test]
    fn the_flashlight_holds_a_slot() {
        let mut lights: Vec<Light> = (0..5)
            .map(|k| cone_light(k, 10.0 + k as f32, [k as f32 * 20.0, 0.0, 10.0]))
            .collect();
        let mut flashlight = cone_light(9, 0.1, [0.0, 5.0, 2.0]);
        flashlight.definition_flags |= FIRST_PERSON_LIGHT;
        lights.push(flashlight);
        let s = one_view(lights);
        let got = choose(&s, &light_choice::choose(&s));
        assert_eq!(handles(&s, &got)[0], 0x7000_0009);
        assert!(got.cones[0].flashlight);
        assert_eq!(got.cones[0].edge(1), FLASHLIGHT_EDGE);
        assert_eq!(got.cones[1].edge(1), 1024);
        assert_eq!(got.cones.len(), SHADOW_CAP);
        assert_eq!(got.work.flashlight_shadowed, 1);
        assert_eq!(got.work.shadows_over_cap, 2);
    }

    /// A light the guest shadowed keeps its own tile and takes a slot
    /// ahead of the renderer's; past the cap it is still shadowed.
    #[test]
    fn the_guests_shadowed_lights_keep_their_tiles() {
        let mut lights: Vec<Light> = (0..6)
            .map(|k| cone_light(k, 10.0 + k as f32, [k as f32 * 20.0, 0.0, 10.0]))
            .collect();
        for l in &mut lights[..5] {
            l.guest_selected = true;
            l.flags |= 4;
            l.definition_flags &= !NO_SHADOW;
        }
        let s = one_view(lights);
        let got = choose(&s, &light_choice::choose(&s));
        assert_eq!(got.cones.len(), 5);
        assert!(
            got.cones
                .iter()
                .all(|c| c.source == Source::Guest && c.slot.is_none())
        );
        assert_eq!(got.work.cones_shadowed, 0);
        assert_eq!(got.work.cones_flagged_off, 0);
        assert_eq!(got.work.shadows_over_cap, 1, "the renderer's cone");
    }

    /// Split screen shares the four: two views take two each, and a
    /// second record of a view is chosen alike and counted once.
    #[test]
    fn split_screen_shares_the_slots() {
        assert_eq!(slot_caps(1), [4]);
        assert_eq!(slot_caps(3), [2, 1, 1]);
        assert_eq!(slot_caps(4), [1, 1, 1, 1]);
        let many = || {
            (0..3)
                .map(|k| cone_light(k, 1.0 + k as f32, [k as f32 * 20.0, 0.0, 10.0]))
                .collect::<Vec<_>>()
        };
        let s = Scene {
            views: vec![
                (0, many(), Vec::new()),
                (1, many(), Vec::new()),
                (0, many(), Vec::new()),
            ],
        }
        .build();
        let got = choose(&s, &light_choice::choose(&s));
        let per_view: Vec<u32> = got.cones.iter().map(|c| c.view).collect();
        assert_eq!(per_view, [0, 0, 1, 1, 2, 2]);
        assert_eq!(got.work.cones_shadowed, 4);
        assert_eq!(got.work.shadows_over_cap, 2);
        let renderer_slots: Vec<_> = got.cones.iter().map(|c| c.slot).collect();
        assert_eq!(
            renderer_slots,
            [Some(0), Some(1), Some(2), Some(3), Some(0), Some(1)],
            "the second record shares the first's tiles"
        );
    }

    /// A projected shape other than the frustum is not a cone; a frustum
    /// with no extent is refused and counted.
    #[test]
    fn only_a_whole_frustum_casts() {
        let mut box_light = cone_light(1, 3.0, [0.0, 0.0, 10.0]);
        if let Some(LightVolume::Projected { kind, .. }) =
            &mut box_light.candidate.as_mut().unwrap().volume
        {
            *kind = 1;
        }
        let mut flat = cone_light(2, 2.0, [20.0, 0.0, 10.0]);
        if let Some(LightVolume::Projected { far_extent, .. }) =
            &mut flat.candidate.as_mut().unwrap().volume
        {
            *far_extent = [0.0, 5.0];
        }
        let s = one_view(vec![box_light, flat]);
        let got = choose(&s, &light_choice::choose(&s));
        assert!(got.cones.is_empty());
        assert_eq!(got.work.cones_refused, 1);
    }

    fn apply(m: [[f32; 4]; 4], p: [f32; 3]) -> [f32; 4] {
        let q = [p[0], p[1], p[2], 1.0];
        m.map(|r| r[0] * q[0] + r[1] * q[1] + r[2] * q[2] + r[3] * q[3])
    }

    /// The tile's projection: the far corners at the tile's corners, the
    /// near depth at 0 and the far at 1, and depth growing away from the
    /// light.
    #[test]
    fn the_projection_spans_the_cone() {
        let cone = Cone::of(&cone_light(1, 1.0, [0.0, 0.0, 10.0])).unwrap();
        assert_eq!(cone.apex, [0.0, 0.0, 11.0]);
        assert!((cone.far - 11.0).abs() < 1e-5);
        assert!((cone.slopes[0] - 5.0 / 11.0).abs() < 1e-6);
        let rows = cone.rows();
        let ndc = |p| {
            let c = apply(rows, p);
            [c[0] / c[3], c[1] / c[3], c[2] / c[3]]
        };
        let far_corner = ndc([5.0, 5.0, 0.0]);
        assert!(
            far_corner[2] > 0.999 && far_corner[2] < 1.001,
            "{far_corner:?}"
        );
        assert!(
            (far_corner[0].abs() - 1.0).abs() < 1e-4 && (far_corner[1].abs() - 1.0).abs() < 1e-4
        );
        let centre = ndc([0.0, 0.0, 5.0]);
        assert!(centre[0].abs() < 1e-6 && centre[1].abs() < 1e-6);
        let near = ndc(add(cone.apex, scale(cone.axis, cone.near)));
        assert!(near[2].abs() < 1e-4, "{near:?}");
        assert!(ndc([0.0, 0.0, 4.0])[2] < ndc([0.0, 0.0, 2.0])[2]);
    }

    /// Spheres in the cone meet it; one beside it, above the apex or past
    /// the far depth does not.
    #[test]
    fn spheres_meet_the_cone() {
        let cone = Cone::of(&cone_light(1, 1.0, [0.0, 0.0, 10.0])).unwrap();
        assert!(cone.meets([0.0, 0.0, 5.0], 0.5));
        assert!(cone.meets([4.8, 0.0, 0.5], 0.5), "near the far edge");
        assert!(cone.meets([6.0, 0.0, 0.0], 1.0), "overlapping a side");
        assert!(!cone.meets([9.0, 0.0, 0.0], 1.0), "beside the cone");
        assert!(!cone.meets([0.0, 0.0, 14.0], 1.0), "behind the apex");
        assert!(!cone.meets([0.0, 0.0, -5.0], 1.0), "past the far depth");
        assert!(!cone.meets([0.0, 0.0, 5.0], f32::NAN));
        assert!(cone.holds_apex([0.0, 0.0, 10.5], 1.0));
    }

    /// The flashlight's holder stands at the apex and neither its owner
    /// nor a sphere around the apex casts; a crate under the light casts,
    /// and the holder still receives.
    #[test]
    fn the_owner_does_not_cast_in_its_own_light() {
        let mut flashlight = cone_light(1, 1.0, [0.0, 0.0, 10.0]);
        flashlight.definition_flags |= FIRST_PERSON_LIGHT;
        flashlight.candidate.as_mut().unwrap().owner = Some(InstanceKey(0xa));
        let s = Scene {
            views: vec![(
                0,
                vec![flashlight],
                vec![
                    (0xa, ([0.0, 0.0, 6.0], 1.0)),
                    (0xb, ([0.0, 0.0, 3.0], 1.0)),
                    (0xc, ([0.0, 0.0, 11.0], 2.0)),
                    (0xd, ([40.0, 0.0, 3.0], 1.0)),
                ],
            )],
        }
        .build();
        let got = choose(&s, &light_choice::choose(&s));
        let cone = got.cones[0];
        assert_eq!(casters(&s, &cone), [InstanceKey(0xb)]);
        assert_eq!(
            receivers(&s, &cone),
            [InstanceKey(0xa), InstanceKey(0xb), InstanceKey(0xc)]
        );
    }

    /// The frame step's view-at-a-time choice is the scene's: for each
    /// view's first record, the lights `view_shadows` gives from the
    /// view's slice and its slot share are those `choose` shadows there,
    /// in the same order.
    #[test]
    fn a_views_shadows_from_its_slice_match_the_frames() {
        let many = |base: u32| {
            let mut v: Vec<Light> = (0..5)
                .map(|k| cone_light(base + k, 1.0 + k as f32, [k as f32 * 20.0, 0.0, 10.0]))
                .collect();
            v[0].guest_selected = true;
            v[0].flags |= 4;
            v
        };
        let s = Scene {
            views: vec![(0, many(0), Vec::new()), (1, many(10), Vec::new())],
        }
        .build();
        let lit = light_choice::choose(&s);
        let got = choose(&s, &lit);
        let keys: Vec<ViewKey> = s.views.iter().map(|v| v.key).collect();
        for (index, v) in s.views.iter().enumerate() {
            let slice = &s.lights[v.lights.start as usize..v.lights.end as usize];
            let mine = view_shadows(
                slice,
                v.lights.start,
                &lit.views[index].lit,
                slot_share_of(&keys, v.key),
            );
            let want: Vec<u32> = got
                .cones
                .iter()
                .filter(|c| c.view == index as u32)
                .map(|c| c.light)
                .collect();
            assert_eq!(mine, want, "view {index}");
        }
        assert_eq!(slot_share_of(&keys, keys[1]), 2);
        assert_eq!(slot_share_of(&keys[..1], keys[1]), 2, "a new key counts");
        assert_eq!(slot_share_of(&[], keys[0]), SHADOW_CAP);
    }

    #[test]
    fn tile_edges_follow_the_shadow_scale() {
        assert_eq!(tile_edge(1, false), 1024);
        assert_eq!(tile_edge(2, false), 1024);
        assert_eq!(tile_edge(4, false), 2048);
        assert_eq!(tile_edge(6, false), SLOT_EDGE);
        assert_eq!(tile_edge(1, true), FLASHLIGHT_EDGE);
        assert_eq!(slot_rect(3, 1024), [SLOT_EDGE, SLOT_EDGE, 1024, 1024]);
        assert_eq!(slot_rect(1, 4096), [SLOT_EDGE, 0, SLOT_EDGE, SLOT_EDGE]);
    }
}
