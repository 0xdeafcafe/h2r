//! The generated nav drawn in the world while the Nav tool is selected
//! (docs/game/forge-nav.md, "The tool"): the sectors' sides in the map and
//! out of it, and each link but a walk, as world lines the window's pass
//! draws, as the game type markers are ([`super::markers_world`]).
//!
//! The driver [`publish`]es the last **Generate nav**'s lines
//! ([`forge::NavReport::lines`]) each tick, or none; the render owner's
//! capture keeps the ones near the eye ([`groups`]), at most [`BUDGET`], one
//! group a kind so each kind has its own colour.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use forge::NavLine;

/// The most lines a view draws; the nearest are kept.
pub const BUDGET: usize = 6000;
/// A line is drawn when its middle is within this many world units of the
/// eye.
pub const RANGE: f32 = 60.0;
/// A line is at full strength to here and gone at the second.
pub const FADE: (f32, f32) = (40.0, RANGE);

/// A polygon's side the starts reach.
pub const SECTOR: [f32; 3] = [0.25, 0.65, 1.0];
/// A polygon's side no start reaches.
pub const OUT_OF_MAP: [f32; 3] = [0.45, 0.45, 0.5];
/// A jump, crouch jump or drop.
pub const JUMP: [f32; 3] = [1.0, 0.8, 0.2];
/// A teleporter.
pub const TELEPORTER: [f32; 3] = [0.75, 0.45, 1.0];
/// A lift or an elevator.
pub const LIFT: [f32; 3] = [0.3, 0.95, 0.5];

/// The colour of a line kind (`forge.nav`'s names).
pub fn colour(kind: &str) -> [f32; 3] {
    match kind {
        "sector" => SECTOR,
        "out_of_map" => OUT_OF_MAP,
        "teleporter" => TELEPORTER,
        "lift" | "elevator" => LIFT,
        _ => JUMP,
    }
}

/// One kind's lines for the window.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub kind: String,
    pub edges: Vec<[[f32; 3]; 2]>,
    pub colour: [f32; 3],
}

/// How far over the ground the AI tool lifts a sector's sides, so they do
/// not sink into it.
pub const SECTOR_LIFT: f32 = 0.05;

/// The sides of the built ground's sectors, each polygon in ring order, as
/// `sector` lines for the AI tool.
pub fn sector_lines(sectors: &[Vec<[f32; 3]>]) -> Vec<NavLine> {
    let lift = |p: [f32; 3]| [p[0], p[1], p[2] + SECTOR_LIFT];
    sectors
        .iter()
        .flat_map(|ring| {
            (0..ring.len()).map(move |i| NavLine {
                kind: "sector".into(),
                a: lift(ring[i]),
                b: lift(ring[(i + 1) % ring.len()]),
            })
        })
        .collect()
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt()
}

fn middle(l: &NavLine) -> [f32; 3] {
    std::array::from_fn(|i| (l.a[i] + l.b[i]) * 0.5)
}

/// The lines to draw from `eye`: those within [`RANGE`] (all with no eye),
/// the nearest [`BUDGET`] of them, grouped by kind in the order the kinds
/// first appear.
pub fn groups(lines: &[NavLine], eye: Option<[f32; 3]>) -> Vec<Group> {
    let mut near: Vec<(f32, &NavLine)> = lines
        .iter()
        .map(|l| (eye.map_or(0.0, |e| distance(e, middle(l))), l))
        .filter(|(d, _)| *d <= RANGE)
        .collect();
    if near.len() > BUDGET {
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        near.truncate(BUDGET);
    }
    let mut out: Vec<Group> = Vec::new();
    for (_, l) in near {
        let at = match out.iter().position(|g| g.kind == l.kind) {
            Some(at) => at,
            None => {
                out.push(Group {
                    kind: l.kind.clone(),
                    edges: Vec::new(),
                    colour: colour(&l.kind),
                });
                out.len() - 1
            }
        };
        out[at].edges.push([l.a, l.b]);
    }
    out
}

struct Published {
    lines: Arc<Vec<NavLine>>,
    /// The report's lines it was copied from, to copy again only when the
    /// report is another one.
    key: (usize, usize),
}

static PUBLISHED: Mutex<Option<Published>> = Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The lines the window draws from now on; none draws nothing. The same
/// slice (the session's report, unchanged) is not copied again.
pub fn publish(lines: Option<&[NavLine]>) {
    let mut slot = PUBLISHED.lock().unwrap_or_else(|e| e.into_inner());
    match lines.filter(|l| !l.is_empty()) {
        None => {
            ACTIVE.store(false, Ordering::Release);
            *slot = None;
        }
        Some(lines) => {
            let key = (lines.as_ptr() as usize, lines.len());
            if slot.as_ref().is_none_or(|p| p.key != key) {
                *slot = Some(Published {
                    lines: Arc::new(lines.to_vec()),
                    key,
                });
            }
            ACTIVE.store(true, Ordering::Release);
        }
    }
}

/// No session: nothing is drawn.
pub fn clear() {
    publish(None);
}

#[cfg(any(feature = "gpu", test))]
fn published() -> Option<Arc<Vec<NavLine>>> {
    PUBLISHED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|p| p.lines.clone())
}

#[cfg(feature = "gpu")]
pub use gpu::register;

#[cfg(feature = "gpu")]
mod gpu {
    use guest_layout::GuestMemory;

    use super::*;
    use crate::world_lines::{ImageMarks, LineStyle, WorldLine, WorldLineSource};
    use gfx::world_lines::Fade;

    /// Draws the nav while the Nav tool has lines.
    pub struct Source;

    pub fn register(host: &mut crate::Host) {
        host.add_world_line_source(Arc::new(Source));
    }

    /// The first world view's eye in the last frame's scene.
    fn eye() -> Option<[f32; 3]> {
        let scene = render_bridge::scene_step::last()?;
        scene
            .views
            .iter()
            .find(|v| v.key.kind == frame_scene::ViewKind::Game && v.key.view == 0)
            .map(|v| v.camera.position)
    }

    impl WorldLineSource for Source {
        fn active(&self) -> bool {
            ACTIVE.load(Ordering::Acquire)
        }

        fn capture(&self, _: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            let lines = published()?;
            let groups = groups(&lines, eye());
            (!groups.is_empty()).then(|| Arc::new(Sample(groups)) as Arc<dyn ImageMarks>)
        }
    }

    #[derive(Debug)]
    struct Sample(Vec<Group>);

    impl ImageMarks for Sample {
        fn lines(&self, visitor: &mut dyn FnMut(WorldLine<'_>)) {
            for g in &self.0 {
                let Some(first) = g.edges.first() else { continue };
                visitor(WorldLine {
                    edges: &g.edges,
                    anchor: first[0],
                    color: g.colour,
                    dashed: g.kind == "out_of_map",
                    label: None,
                    style: LineStyle {
                        width: 1.5,
                        alpha: 0.9,
                        fade: Some(Fade {
                            start: FADE.0,
                            end: FADE.1,
                        }),
                        ..LineStyle::default()
                    },
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: &str, x: f32) -> NavLine {
        NavLine {
            kind: kind.into(),
            a: [x, 0., 0.],
            b: [x + 1., 0., 0.],
        }
    }

    #[test]
    fn lines_group_by_kind_in_the_colour_of_their_kind() {
        let lines = [line("sector", 0.), line("jump", 1.), line("sector", 2.)];
        let g = groups(&lines, None);
        assert_eq!(g.len(), 2);
        assert_eq!((g[0].kind.as_str(), g[0].edges.len(), g[0].colour), ("sector", 2, SECTOR));
        assert_eq!((g[1].kind.as_str(), g[1].edges.len(), g[1].colour), ("jump", 1, JUMP));
        assert_eq!(colour("teleporter"), TELEPORTER);
        assert_eq!(colour("out_of_map"), OUT_OF_MAP);
        assert_eq!(colour("elevator"), LIFT);
    }

    #[test]
    fn a_sector_is_one_line_a_side_in_the_sector_colour() {
        let lines = sector_lines(&[vec![[0., 0., 0.], [2., 0., 0.], [0., 2., 0.]], vec![]]);
        assert_eq!(lines.len(), 3, "an empty ring has no sides");
        assert!(lines.iter().all(|l| l.kind == "sector"));
        assert_eq!(lines[2].a, [0., 2., SECTOR_LIFT]);
        assert_eq!(lines[2].b, [0., 0., SECTOR_LIFT], "the ring closes");
        assert_eq!(groups(&lines, None)[0].colour, SECTOR);
    }

    #[test]
    fn only_the_lines_near_the_eye_are_drawn() {
        let lines = [line("sector", 0.), line("sector", 500.)];
        let g = groups(&lines, Some([0., 0., 0.]));
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].edges, vec![[[0., 0., 0.], [1., 0., 0.]]]);
        assert!(groups(&lines, Some([900., 0., 0.])).is_empty());
    }

    #[test]
    fn past_the_budget_the_nearest_lines_stay() {
        let lines: Vec<NavLine> = (0..BUDGET + 10).map(|i| line("sector", i as f32 * 0.005)).collect();
        let g = groups(&lines, Some([0., 0., 0.]));
        assert_eq!(g[0].edges.len(), BUDGET);
        let farthest = g[0].edges.iter().map(|e| e[0][0]).fold(0.0, f32::max);
        assert!(farthest < (BUDGET + 10) as f32 * 0.005 - 0.04, "the last ten were cut");
    }

    #[test]
    fn publishing_the_same_report_twice_copies_it_once() {
        let _one = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let lines = vec![line("sector", 0.)];
        publish(Some(&lines));
        let first = published().unwrap();
        publish(Some(&lines));
        assert!(Arc::ptr_eq(&first, &published().unwrap()));
        assert!(ACTIVE.load(Ordering::Acquire));
        publish(Some(&[]));
        assert!(published().is_none());
        assert!(!ACTIVE.load(Ordering::Acquire));
    }

    static TEST_GUARD: Mutex<()> = Mutex::new(());
}
