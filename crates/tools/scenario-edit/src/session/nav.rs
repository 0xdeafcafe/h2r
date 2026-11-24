//! The navigation the session shows with the map ([`crate::nav_view`];
//! docs/editor/editor.md, "What the viewport shows, and what it does not
//! yet"): the structure BSPs' pathfinding
//! data, read when the map opens, the bots' mesh and spots the editor
//! hands over, a pick and a selection. Read only.
use super::*;
use crate::nav_view::{self, BotNav, LevelPathfinding, NavPick};

impl Session {
    /// The open map's pathfinding data.
    /// While AI ground is shown ([`Self::set_ground_pathfinding`]) it is
    /// the ground's.
    pub fn pathfinding(&self) -> Option<&LevelPathfinding> {
        self.doc
            .as_ref()
            .map(|d| d.ground_pathfinding.as_ref().unwrap_or(&d.pathfinding))
    }

    /// The bots' data for the open map, once the editor has given it.
    pub fn bot_nav(&self) -> Option<&Arc<BotNav>> {
        self.doc.as_ref()?.bot_nav.as_ref()
    }

    /// Give the open map its bots' data (or take it away). A selected
    /// bot polygon, link or spot is cleared; nothing happens with no map
    /// open.
    pub fn set_bot_nav(&mut self, nav: Option<Arc<BotNav>>) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        doc.bot_nav = nav;
        if !matches!(self.nav, None | Some(NavPick::Sector(_))) {
            self.nav = None;
        }
        self.markers_rev += 1;
    }

    /// Whether `pick` names something the open map has.
    pub fn has_nav(&self, pick: NavPick) -> bool {
        let bots = self.bot_nav();
        match pick {
            NavPick::Sector(i) => self.pathfinding().is_some_and(|p| i < p.sectors.len()),
            NavPick::BotPoly(i) => bots.is_some_and(|b| i < b.polys.len()),
            NavPick::BotLink(i) => bots.is_some_and(|b| i < b.links.len()),
            NavPick::Spot(i) => bots.is_some_and(|b| i < b.spots.len()),
        }
    }

    /// The selected navigation thing.
    pub fn selected_nav(&self) -> Option<NavPick> {
        self.nav.filter(|&p| self.has_nav(p))
    }

    /// Select a navigation thing (clearing the placements, the volume and
    /// the surface), or none. One the map does not have selects nothing.
    pub fn select_nav(&mut self, pick: Option<NavPick>) {
        self.select(None);
        self.nav = pick.filter(|&p| self.has_nav(p));
        self.markers_rev += 1;
    }

    /// The nearest navigation thing a camera ray meets among the shown
    /// layers ([`nav_view::pick`]).
    pub fn pick_nav(&self, ray: &Ray, layers: &Layers) -> Option<NavPick> {
        self.nav_hit(ray, |id| layers.visible(id)).map(|(_, p)| p)
    }

    /// The nearest navigation thing a ray meets among the layers `shown`
    /// accepts, and how far along the ray.
    pub fn nav_hit(&self, ray: &Ray, shown: impl Fn(LayerId) -> bool) -> Option<(f32, NavPick)> {
        nav_view::pick(
            self.pathfinding(),
            self.bot_nav().map(|b| b.as_ref()),
            ray,
            shown,
        )
    }

    /// The navigation thing at `p` ([`nav_view::at_point`]).
    pub fn nav_at(
        &self,
        p: Vec3,
        within: f32,
        shown: impl Fn(LayerId) -> bool,
    ) -> Option<(f32, NavPick)> {
        nav_view::at_point(
            self.pathfinding(),
            self.bot_nav().map(|b| b.as_ref()),
            p,
            within,
            shown,
        )
    }

    /// A sphere round a navigation thing, for framing it.
    pub fn nav_sphere(&self, pick: NavPick) -> Option<(Vec3, f32)> {
        let bots = self.bot_nav();
        match pick {
            NavPick::Sector(i) => {
                let s = self.pathfinding()?.sectors.get(i)?;
                Some((s.center(), s.radius().max(0.5)))
            }
            NavPick::BotPoly(i) => {
                let p = bots?.polys.get(i)?;
                Some((p.center(), p.radius().max(0.5)))
            }
            NavPick::BotLink(i) => {
                let b = bots?;
                let [p, q] = b.links.get(i)?.line(&b.polys);
                let c = geometry::scale(geometry::add(p, q), 0.5);
                let d = geometry::sub(q, p);
                Some((c, (0.5 * geometry::dot(d, d).sqrt()).max(0.5)))
            }
            NavPick::Spot(i) => Some((bots?.spots.get(i)?.stand, 1.0)),
        }
    }

    pub(super) fn selected_nav_sphere(&self) -> Option<(Vec3, f32)> {
        self.nav_sphere(self.selected_nav()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{self, ROOM_AT};
    use crate::nav_view::{BotLink, BotLinkKind, BotPoly, BotSpot, SpotKind, SpotState};

    fn open(bytes: Vec<u8>, name: &str) -> Session {
        let dir =
            std::env::temp_dir().join(format!("scenario-edit-nav-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nav.map");
        std::fs::write(&path, &bytes).unwrap();
        let mut ed = Session::new();
        ed.build_scenes = false;
        ed.open(MapSource::file(&path)).unwrap();
        ed
    }

    /// The invented room's two sectors read with the map, each its half
    /// of the floor, with its flags and its ring of links; the orphan
    /// link is left out. A ray picks the right-hand sector only while
    /// the layer shows it, and selecting it replaces the placements.
    #[test]
    fn the_invented_bsps_pathfinding_draws_two_sectors_and_picks_one() {
        let mut ed = open(fixture::fixture_with_pathfinding(), "sectors");
        let p = ed.pathfinding().unwrap();
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.bsps.len(), 1);
        let bsp = &p.bsps[0];
        assert!(bsp.present);
        assert_eq!(
            (
                bsp.sectors,
                bsp.links,
                bsp.nodes,
                bsp.vertices,
                bsp.refs,
                bsp.orphan_links
            ),
            (2, 8, 1, 6, 3, 1)
        );
        assert!(bsp.broken.is_empty());
        assert_eq!(p.sectors.len(), 2);
        assert_eq!(p.links.len(), 7, "the orphan link is left out");
        assert_eq!(p.note(), None);
        let at = |x: f32, y: f32| [ROOM_AT[0] + x, ROOM_AT[1] + y, ROOM_AT[2]];
        let left = &p.sectors[0];
        assert_eq!(left.flags, fixture::SECTOR_FLAGS[0]);
        assert_eq!(left.links, [0, 1, 2, 3]);
        assert_eq!(
            left.ring,
            [at(0.0, 0.0), at(5.0, 0.0), at(5.0, 10.0), at(0.0, 10.0)]
        );
        assert!(left.floor());
        let right = &p.sectors[1];
        assert_eq!(right.flags, fixture::SECTOR_FLAGS[1]);
        assert_eq!(right.links, [4, 5, 6, 1]);
        assert_eq!(
            right.ring,
            [at(5.0, 0.0), at(10.0, 0.0), at(10.0, 10.0), at(5.0, 10.0)]
        );
        let shared = p.links.iter().find(|l| l.link == 1).unwrap();
        assert_eq!((shared.left, shared.right), (Some(0), Some(1)));

        let ray = Ray {
            origin: [ROOM_AT[0] + 7.5, ROOM_AT[1] + 5.0, 10.0],
            dir: [0.0, 0.0, -1.0],
        };
        let mut layers = Layers::default();
        assert_eq!(ed.pick_nav(&ray, &layers), None, "hidden by default");
        layers.set(LayerId::Pathfinding, true);
        assert_eq!(ed.pick_nav(&ray, &layers), Some(NavPick::Sector(1)));
        assert_eq!(
            ed.nav_at(at(2.0, 2.0), 0.05, |_| true).map(|h| h.1),
            Some(NavPick::Sector(0))
        );

        ed.select(Some((BlockKind::Weapons, 0)));
        ed.select_nav(Some(NavPick::Sector(1)));
        assert_eq!(ed.selected(), None);
        assert_eq!(ed.selected_nav(), Some(NavPick::Sector(1)));
        let (c, _) = ed.selection_sphere().unwrap();
        assert!((c[0] - (ROOM_AT[0] + 7.5)).abs() < 1e-4, "{c:?}");
        ed.select_surface(Some(0));
        assert_eq!(ed.selected_nav(), None);
        ed.select_nav(Some(NavPick::Sector(5)));
        assert_eq!(ed.selected_nav(), None, "past the end");
    }

    /// A map whose BSP carries no pathfinding data shows the layer empty,
    /// with a note saying why.
    #[test]
    fn a_bsp_without_pathfinding_is_empty_with_a_note() {
        let ed = open(fixture::fixture_with_collision(), "none");
        let p = ed.pathfinding().unwrap();
        assert!(p.is_empty());
        assert!(!p.bsps[0].present);
        assert!(
            p.note()
                .unwrap()
                .contains("no structure BSP carries pathfinding")
        );
        let plain = open(fixture::fixture(), "no-bsp");
        assert_eq!(
            plain.pathfinding().unwrap().note().as_deref(),
            Some("the map has no structure BSP")
        );
    }

    /// The bots' data goes with the map: given, it picks under its layers;
    /// replacing it clears a bot selection but keeps a sector.
    #[test]
    fn bot_nav_given_to_the_session_picks_and_clears_its_selection() {
        let mut ed = open(fixture::fixture_with_pathfinding(), "bots");
        let floor = |x: f32| {
            let verts = vec![
                [ROOM_AT[0] + x, ROOM_AT[1], 0.0],
                [ROOM_AT[0] + x + 5.0, ROOM_AT[1], 0.0],
                [ROOM_AT[0] + x + 5.0, ROOM_AT[1] + 10.0, 0.0],
                [ROOM_AT[0] + x, ROOM_AT[1] + 10.0, 0.0],
            ];
            BotPoly {
                bsp: 0,
                poly: 0,
                normal: nav_view::ring_normal(&verts),
                verts,
                out_of_map: false,
            }
        };
        let mut nav = BotNav {
            map: "fixture".into(),
            polys: vec![floor(0.0), floor(5.0)],
            links: vec![BotLink {
                bsp: 0,
                link: 0,
                from: 0,
                to: 1,
                a: [ROOM_AT[0] + 5.0, ROOM_AT[1], 0.0],
                b: [ROOM_AT[0] + 5.0, ROOM_AT[1] + 10.0, 0.0],
                kind: BotLinkKind::Jump,
                one_way: false,
            }],
            spots: vec![BotSpot {
                kind: Some(SpotKind::Crouch),
                stand: [ROOM_AT[0] + 1.0, ROOM_AT[1] + 1.0, 0.0],
                state: Some(SpotState::Proven),
                ..BotSpot::default()
            }],
            ..BotNav::default()
        };
        nav.mark_one_way();
        assert!(nav.links[0].one_way);
        ed.set_bot_nav(Some(Arc::new(nav)));
        let down = |x: f32, y: f32| Ray {
            origin: [ROOM_AT[0] + x, ROOM_AT[1] + y, 10.0],
            dir: [0.0, 0.0, -1.0],
        };
        let mut layers = Layers::default();
        layers.apply(crate::layers::Preset::Navigation);
        assert_eq!(
            ed.pick_nav(&down(1.0, 1.0), &layers),
            Some(NavPick::Spot(0))
        );
        assert_eq!(
            ed.pick_nav(&down(8.5, 2.0), &layers),
            Some(NavPick::BotPoly(1))
        );
        layers.set(LayerId::BotNav, false);
        assert_eq!(
            ed.pick_nav(&down(8.5, 2.0), &layers),
            Some(NavPick::Sector(1))
        );

        ed.select_nav(Some(NavPick::BotLink(0)));
        assert_eq!(ed.selected_nav(), Some(NavPick::BotLink(0)));
        ed.set_bot_nav(None);
        assert_eq!(ed.selected_nav(), None);
        ed.select_nav(Some(NavPick::Sector(0)));
        ed.set_bot_nav(Some(Arc::new(BotNav::default())));
        assert_eq!(ed.selected_nav(), Some(NavPick::Sector(0)));
    }
}
