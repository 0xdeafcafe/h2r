//! The level collision the session reads with the map
//! ([`crate::collision_view`]; docs/editor/polish-plan.md, slice P10): the
//! surfaces, a surface pick and a surface selection. Read only: a
//! collision surface is edited through the barrier edits, which rebuild
//! the BSP.
use super::*;
use crate::collision_view::{CollisionSurface, LevelCollision, SurfaceClass};

/// Whether a surface of `class` is drawn with `layers`: every surface
/// with Level collision, the uncovered ones with Collision only.
pub fn surface_shown(layers: &Layers, class: SurfaceClass) -> bool {
    layers.visible(LayerId::LevelCollision)
        || (class == SurfaceClass::CollisionOnly && layers.visible(LayerId::CollisionOnly))
}

impl Session {
    /// The open map's level collision, classified.
    pub fn collision(&self) -> Option<&LevelCollision> {
        self.doc.as_ref().map(|d| &d.collision)
    }

    /// The selected collision surface's index.
    pub fn selected_surface(&self) -> Option<usize> {
        self.surface
            .filter(|&i| self.collision().is_some_and(|c| i < c.surfaces.len()))
    }

    /// The selected collision surface.
    pub fn selected_surface_data(&self) -> Option<&CollisionSurface> {
        self.collision()?.surfaces.get(self.selected_surface()?)
    }

    /// Select a collision surface (clearing the placements and the
    /// volume), or none.
    pub fn select_surface(&mut self, index: Option<usize>) {
        self.select(None);
        self.surface = index;
        self.markers_rev += 1;
    }

    /// The nearest shown collision surface a camera ray meets.
    pub fn pick_surface(&self, ray: &Ray, layers: &Layers) -> Option<usize> {
        self.collision()?
            .pick(ray, |class| surface_shown(layers, class))
            .map(|(_, i)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{self, ROOM_AT};

    fn open(bytes: Vec<u8>, name: &str) -> Session {
        let dir = std::env::temp_dir().join(format!(
            "scenario-edit-collision-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("collision.map");
        std::fs::write(&path, &bytes).unwrap();
        let mut ed = Session::new();
        ed.build_scenes = false;
        ed.open(MapSource::file(&path)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        ed
    }

    /// The invented structure BSP's room reads with the map. It has no
    /// render geometry, so every surface is collision only; a ray at the
    /// far wall picks it while a collision layer shows it, and selecting
    /// it clears the placements.
    #[test]
    fn the_invented_rooms_collision_reads_and_picks() {
        let mut ed = open(fixture::fixture_with_collision(), "room");
        let c = ed.collision().unwrap();
        assert_eq!(c.bsps, [fixture::id(4)]);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert_eq!(c.surfaces.len(), 3);
        assert_eq!(c.count(SurfaceClass::CollisionOnly), 3);
        let room = fixture::collision_room(ROOM_AT);
        for s in &c.surfaces {
            let ring: Vec<Vec3> = room
                .surface_ring(s.surface)
                .into_iter()
                .map(|v| room.vertices[v].point)
                .collect();
            assert_eq!(s.ring, ring);
        }

        let ray = Ray {
            origin: [ROOM_AT[0] + 5.0, ROOM_AT[1] + 5.0, 1.0],
            dir: [1.0, 0.0, 0.0],
        };
        let mut layers = Layers::default();
        assert_eq!(ed.pick_surface(&ray, &layers), None, "hidden by default");
        layers.set(LayerId::CollisionOnly, true);
        assert_eq!(ed.pick_surface(&ray, &layers), Some(2));
        layers.set(LayerId::CollisionOnly, false);
        layers.set(LayerId::LevelCollision, true);
        assert_eq!(ed.pick_surface(&ray, &layers), Some(2));

        ed.select(Some((BlockKind::Weapons, 0)));
        ed.select_surface(Some(2));
        assert_eq!(ed.selected(), None);
        assert_eq!(ed.selected_surface(), Some(2));
        assert_eq!(ed.selected_surface_data().unwrap().flags, 0x08);
        ed.select_volume(Some(0));
        assert_eq!(ed.selected_surface(), None);
        ed.select_surface(Some(9));
        assert_eq!(ed.selected_surface(), None, "past the end");
    }

    /// A map whose scenario names no structure BSP has no collision, and
    /// one whose BSP does not read names it.
    #[test]
    fn no_bsp_reads_as_no_collision_and_a_bad_one_is_named() {
        let ed = open(fixture::fixture(), "none");
        assert!(ed.collision().unwrap().surfaces.is_empty());
        let mut bytes = fixture::fixture_with_collision();
        // The collision block's count past the payload.
        let field = fixture::META_FILE + fixture::SBSP + tags::pathfinding::SBSP_COLLISION_FIELD;
        fixture::put(&mut bytes, field, 0x100);
        let ed = open(bytes, "bad");
        let c = ed.collision().unwrap();
        assert!(c.surfaces.is_empty());
        assert_eq!(c.errors.len(), 1);
        assert_eq!(c.errors[0].0, 0);
    }
}
