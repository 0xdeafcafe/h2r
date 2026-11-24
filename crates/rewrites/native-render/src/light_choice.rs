//! E3 lighting's own choice of lights (docs/graphics/e3-lighting.md,
//! "Lights: which, and how many"). Retail mode draws the lights the guest
//! drew, in its order. E3 mode chooses from every candidate the scene
//! holds for a view (`FrameScene::lights`), before retail's ranking and
//! trim:
//!
//! - **Candidates.** A light with its candidate record that passed the
//!   view's owner and camera tests (`LightCandidate::view_eligible`), or
//!   that the guest drew.
//! - **Score.** Retail's projected-size priority times the light's
//!   intensity; a missing, negative or non-finite product scores zero.
//! - **Lit.** The view's own lights first, then the highest scores, ties
//!   by lower datum index, then by handle: a total order, so the same
//!   scene gives the same lights every run whatever the candidates'
//!   order. At most [`LIT_CAP`] in a frame, shared between the views of
//!   split screen ([`view_caps`]).
//! - **Host lights** (`Light::host`, docs/game/forge-monitor-appearance.md,
//!   ML5) come after every game light, by seat: the first a view's cap
//!   leaves unlit.
//! - **The first-person pass** draws the first [`FIRST_PERSON_CAP`] of a
//!   view's lit lights again over its first-person parts, as retail's
//!   recorded-light pass does.
//!
//! A view's own light is a first-person light (definition bit 5), which
//! the view test admits only for the view's local first-person owner. The
//! scene does not carry the view's unit or an owner chain, so a light
//! attached to the view's weapon takes its place by score.

use std::collections::HashSet;

use frame_scene::{FrameScene, Light, ViewKey};

/// Lights lit in a frame, shared by the views of split screen.
pub const LIT_CAP: usize = 16;

/// Lights retail records for its first-person pass (`0x004c6b00`, stride
/// `0x408`, count `[0x00509440]`).
pub const FIRST_PERSON_CAP: usize = 8;

/// Definition flag bit 5: the first-person light. The view test
/// `0x00031520` requires the local first-person owner for it.
pub const FIRST_PERSON_LIGHT: u32 = 1 << 5;

/// Whether `light` may be lit in its view.
pub fn candidate(light: &Light) -> bool {
    light
        .candidate
        .as_ref()
        .is_some_and(|c| c.view_eligible == Some(true) || light.guest_selected)
}

/// `light`'s score: projected-size priority times intensity, zero when
/// either is missing or the product is negative or not finite. `None`
/// for a light that is not a [`candidate`].
pub fn score(light: &Light) -> Option<f32> {
    if !candidate(light) {
        return None;
    }
    let c = light.candidate.as_ref()?;
    let s = c.priority.unwrap_or(0.0) * c.intensity;
    Some(if s.is_finite() && s > 0.0 { s } else { 0.0 })
}

/// Whether `light` is its view's own: placed ahead of every score.
pub fn own(light: &Light) -> bool {
    light.definition_flags & FIRST_PERSON_LIGHT != 0
}

/// Each of `views` distinct views' share of [`LIT_CAP`], in order: an
/// equal share, the remainder one each to the first views.
pub fn view_caps(views: usize) -> Vec<usize> {
    if views == 0 {
        return Vec::new();
    }
    let (share, rest) = (LIT_CAP / views, LIT_CAP % views);
    (0..views).map(|v| share + usize::from(v < rest)).collect()
}

/// One view's choice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewChoice {
    /// Into `FrameScene::views`.
    pub view: u32,
    /// Into `FrameScene::lights`, in draw order.
    pub lit: Vec<u32>,
    /// The first of [`Self::lit`], drawn again over the first-person
    /// parts.
    pub first_person: Vec<u32>,
    /// Candidates the view's share left unlit.
    pub over_cap: u32,
}

/// The frame's choice, a view at a time in the scene's order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightChoice {
    pub views: Vec<ViewChoice>,
    /// Whether each view is the first record of its key: a second record
    /// of a view is chosen alike and counted once.
    first_of_key: Vec<bool>,
}

impl LightChoice {
    /// Lights lit in the frame.
    pub fn lit(&self) -> u32 {
        self.counted().map(|v| v.lit.len() as u32).sum()
    }

    /// Candidates left unlit by the cap.
    pub fn over_cap(&self) -> u32 {
        self.counted().map(|v| v.over_cap).sum()
    }

    fn counted(&self) -> impl Iterator<Item = &ViewChoice> {
        self.views
            .iter()
            .zip(&self.first_of_key)
            .filter(|(_, first)| **first)
            .map(|(v, _)| v)
    }
}

/// A candidate's place in its view's order.
struct Rank {
    own: bool,
    host: Option<u8>,
    score: f32,
    handle: u32,
    index: u32,
}

/// The lights E3 lights in `scene`.
pub fn choose(scene: &FrameScene) -> LightChoice {
    let mut keys: Vec<ViewKey> = Vec::new();
    for v in &scene.views {
        if !keys.contains(&v.key) {
            keys.push(v.key);
        }
    }
    let caps = view_caps(keys.len());
    let mut out = LightChoice::default();
    let mut seen_keys = HashSet::new();
    for (index, view) in scene.views.iter().enumerate() {
        let cap = keys
            .iter()
            .position(|k| *k == view.key)
            .map_or(0, |k| caps[k]);
        let mut ranked: Vec<Rank> = Vec::new();
        let mut handles = HashSet::new();
        let lights = scene
            .lights
            .get(view.lights.start as usize..view.lights.end as usize)
            .unwrap_or(&[]);
        for (k, light) in lights.iter().enumerate() {
            let Some(s) = score(light) else { continue };
            if !handles.insert((light.handle, light.host)) {
                continue;
            }
            ranked.push(Rank {
                own: own(light),
                host: light.host,
                score: s,
                handle: light.handle,
                index: view.lights.start + k as u32,
            });
        }
        ranked.sort_by(|a, b| {
            b.own
                .cmp(&a.own)
                .then(a.host.is_some().cmp(&b.host.is_some()))
                .then(a.host.cmp(&b.host))
                .then(b.score.total_cmp(&a.score))
                .then((a.handle & 0xffff).cmp(&(b.handle & 0xffff)))
                .then(a.handle.cmp(&b.handle))
        });
        let lit: Vec<u32> = ranked.iter().take(cap).map(|r| r.index).collect();
        out.first_of_key.push(seen_keys.insert(view.key));
        out.views.push(ViewChoice {
            view: index as u32,
            first_person: lit.iter().copied().take(FIRST_PERSON_CAP).collect(),
            over_cap: ranked.len().saturating_sub(lit.len()) as u32,
            lit,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{LightCandidate, RenderCamera, SceneBuilder, SceneId, Stamp, ViewKind};

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

    /// A candidate light: datum index `index` with salt `salt`, its
    /// priority and intensity.
    fn light(index: u32, salt: u32, priority: f32, intensity: f32) -> Light {
        Light {
            handle: salt << 16 | index,
            definition: 0xe700_0001,
            candidate: Some(LightCandidate {
                position: [5.0, 0.0, 0.0],
                forward: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                colour_a: [1.0; 3],
                colour_b: [1.0; 3],
                intensity,
                bounds: ([5.0, 0.0, 0.0], 2.0),
                owner: None,
                definition_kind: 0,
                volume: None,
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

    /// A scene of one view per entry of `views`, each with its lights.
    fn scene(views: &[(u8, Vec<Light>)]) -> FrameScene {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        for (player, lights) in views {
            b.open_view(view(*player), camera()).unwrap();
            b.add_light_candidates(lights.clone()).unwrap();
        }
        (*b.seal(Stamp::default()).unwrap()).clone()
    }

    fn handles(s: &FrameScene, lit: &[u32]) -> Vec<u32> {
        lit.iter().map(|&i| s.lights[i as usize].handle).collect()
    }

    /// Twenty candidates with distinct scores draw the sixteen highest,
    /// highest first; the other four are over the cap. The same lights in
    /// any order, chosen again, give the same lights in the same order.
    #[test]
    fn twenty_candidates_draw_the_sixteen_highest_in_score_order_every_run() {
        // Index k scores (k + 1) * 3 / 2, so 4..=19 are the highest.
        let lights: Vec<Light> = (0..20)
            .map(|k| light(k, 0xe100 + k, (k + 1) as f32, 1.5))
            .collect();
        let want: Vec<u32> = (4..20).rev().map(|k| (0xe100 + k) << 16 | k).collect();
        let s = scene(&[(0, lights.clone())]);
        let got = choose(&s);
        assert_eq!(got.views.len(), 1);
        assert_eq!(handles(&s, &got.views[0].lit), want);
        assert_eq!((got.lit(), got.over_cap()), (16, 4));
        assert_eq!(handles(&s, &got.views[0].first_person), want[..8]);
        assert_eq!(choose(&s), got, "the same scene, the same choice");
        // Every rotation and the reversal of the candidates' order.
        for turn in 0..20 {
            let mut order = lights.clone();
            order.rotate_left(turn);
            if turn % 2 == 1 {
                order.reverse();
            }
            let s = scene(&[(0, order)]);
            assert_eq!(handles(&s, &choose(&s).views[0].lit), want, "turn {turn}");
        }
    }

    /// Equal scores go to the lower datum index, whatever the salt; a
    /// light the view test refused, or with no candidate record, is not
    /// lit unless the guest drew it; a zero or broken score still lights
    /// under the cap, last.
    #[test]
    fn ties_take_the_lower_index_and_only_candidates_are_lit() {
        let mut refused = light(1, 0x9000, 50.0, 1.0);
        refused.candidate.as_mut().unwrap().view_eligible = Some(false);
        let mut unread = light(2, 0x9000, 50.0, 1.0);
        unread.candidate.as_mut().unwrap().view_eligible = None;
        let mut no_record = light(3, 0x9000, 50.0, 1.0);
        no_record.candidate = None;
        let mut broken = light(9, 0x1, f32::NAN, 1.0);
        broken.candidate.as_mut().unwrap().priority = Some(f32::NAN);
        let s = scene(&[(
            0,
            vec![
                light(7, 0x1, 2.0, 1.0),
                light(5, 0xffff, 1.0, 2.0),
                refused,
                unread,
                no_record,
                broken,
                light(8, 0x2, 0.0, 1.0),
            ],
        )]);
        let lit = handles(&s, &choose(&s).views[0].lit);
        assert_eq!(lit, [0xffff_0005, 0x0001_0007, 0x0002_0008, 0x0001_0009]);
        // The guest drew the refused one: it is lit by its score.
        let mut drawn = s.clone();
        let i = drawn.lights.iter().position(|l| l.handle == 0x9000_0001).unwrap();
        drawn.lights[i].guest_selected = true;
        assert_eq!(handles(&drawn, &choose(&drawn).views[0].lit)[0], 0x9000_0001);
    }

    /// The view's first-person light is lit first even with the lowest
    /// score, and so leads the first-person pass.
    #[test]
    fn the_views_own_light_is_lit_first() {
        let mut lights: Vec<Light> = (0..20).map(|k| light(k, 1, 10.0 + k as f32, 1.0)).collect();
        let mut flashlight = light(40, 7, 0.5, 1.0);
        flashlight.definition_flags = FIRST_PERSON_LIGHT;
        lights.push(flashlight);
        let s = scene(&[(0, lights)]);
        let got = choose(&s);
        let lit = handles(&s, &got.views[0].lit);
        assert_eq!(lit.len(), LIT_CAP);
        assert_eq!(lit[0], 7 << 16 | 40);
        assert_eq!(lit[1], 1 << 16 | 19, "then the highest score");
        assert_eq!(handles(&s, &got.views[0].first_person)[0], 7 << 16 | 40);
        assert_eq!(got.over_cap(), 5);
    }

    /// Host lights (ML5) are lit after every game light whatever their
    /// score, by seat, so they are the first a full view leaves unlit;
    /// two seats' lights, with no datum to tell them apart, are both
    /// candidates.
    #[test]
    fn host_lights_are_last_in_budget_order() {
        let mut bright = Light::host_light(5, [1.0; 3], [0.0, 0.0, 1.0], 4.0);
        bright.candidate.as_mut().unwrap().priority = Some(1000.0);
        let dim = Light::host_light(2, [2.0; 3], [1.0, 0.0, 0.0], 2.0);
        let mut lights = vec![bright, dim];
        lights.extend((0..3).map(|k| light(k, 1, 1.0 + k as f32, 1.0)));
        let s = scene(&[(0, lights.clone())]);
        let lit = &choose(&s).views[0].lit;
        let order: Vec<(u32, Option<u8>)> = lit
            .iter()
            .map(|&i| (s.lights[i as usize].handle, s.lights[i as usize].host))
            .collect();
        assert_eq!(
            order,
            [
                (1 << 16 | 2, None),
                (1 << 16 | 1, None),
                (1 << 16, None),
                (frame_scene::NO_DATUM, Some(2)),
                (frame_scene::NO_DATUM, Some(5)),
            ]
        );
        // Over budget: sixteen game lights leave both host lights unlit.
        lights.extend((3..16).map(|k| light(k, 1, 1.0 + k as f32, 1.0)));
        let s = scene(&[(0, lights)]);
        let got = choose(&s);
        assert!(got.views[0].lit.iter().all(|&i| s.lights[i as usize].host.is_none()));
        assert_eq!(got.over_cap(), 2);
    }

    /// Split screen shares the sixteen: three views take six, five and
    /// five, and a second record of a view is chosen alike but counted
    /// once.
    #[test]
    fn split_screen_shares_the_cap() {
        assert_eq!(view_caps(0), Vec::<usize>::new());
        assert_eq!(view_caps(1), [16]);
        assert_eq!(view_caps(2), [8, 8]);
        assert_eq!(view_caps(3), [6, 5, 5]);
        assert_eq!(view_caps(4), [4, 4, 4, 4]);
        let many = |p: u32| (0..10).map(|k| light(k, p, 1.0 + k as f32, 1.0)).collect::<Vec<_>>();
        let s = scene(&[(0, many(1)), (1, many(2)), (2, many(3)), (0, many(1))]);
        let got = choose(&s);
        let sizes: Vec<usize> = got.views.iter().map(|v| v.lit.len()).collect();
        assert_eq!(sizes, [6, 5, 5, 6]);
        assert_eq!(handles(&s, &got.views[3].lit), handles(&s, &got.views[0].lit));
        assert_eq!((got.lit(), got.over_cap()), (16, 4 + 5 + 5));
    }
}
