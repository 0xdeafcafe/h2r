//! The level collision the session reads with the map
//! ([`crate::collision_view`]; docs/editor/polish-plan.md, slice P10): the
//! surfaces, a surface pick and a surface selection. Read only: a
//! collision surface is edited through the barrier edits, which rebuild
//! the BSP.
use super::*;
use crate::collision_view::{CollisionSurface, LevelCollision, SurfaceClass};
use crate::object_collision::{ObjectCollisions, PlacedShape};

/// Whether `s` is drawn with `layers`: the structure's surfaces with
/// Level collision, instanced geometry's with Instance collision, and the
/// uncovered ones of either with Collision only.
pub fn surface_shown(layers: &Layers, s: &CollisionSurface) -> bool {
    let own = if s.instance.is_some() {
        LayerId::InstanceCollision
    } else {
        LayerId::LevelCollision
    };
    layers.visible(own)
        || (s.class == SurfaceClass::CollisionOnly && layers.visible(LayerId::CollisionOnly))
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

    /// The placed object tags' collision.
    pub fn object_collisions(&self) -> Option<&ObjectCollisions> {
        self.doc.as_ref().map(|d| &d.objects)
    }

    /// Every collision shape the placements place, in world space.
    pub fn placed_object_shapes(&self) -> Vec<PlacedShape<'_>> {
        self.object_collisions()
            .map(|o| o.placed(&self.markers))
            .unwrap_or_default()
    }

    /// How many collision shapes the placements place.
    pub fn object_shape_count(&self) -> usize {
        self.object_collisions()
            .map_or(0, |o| o.count(&self.markers))
    }

    /// The nearest shown collision surface a camera ray meets.
    pub fn pick_surface(&self, ray: &Ray, layers: &Layers) -> Option<usize> {
        self.collision()?
            .pick(ray, |s| surface_shown(layers, s))
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
        // The source stays on disk: an object placed later reads it again.
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

    /// P11: the invented instance's quad is placed at its position, after
    /// the structure's surfaces, classified with them, and picked only
    /// while Instance collision (or Collision only) shows it.
    #[test]
    fn an_invented_instances_collision_is_placed_and_picked() {
        let ed = open(fixture::fixture_with_instance_collision(), "instance");
        let c = ed.collision().unwrap();
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert_eq!(c.surfaces.len(), 4);
        assert_eq!(c.instance_count(), 1);
        let s = &c.surfaces[3];
        assert_eq!((s.instance, s.surface), (Some(0), 0));
        let at = fixture::INSTANCE_POSITION;
        let want: Vec<Vec3> = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
            .into_iter()
            .map(|q| [at[0] + q[0], at[1] + q[1], at[2]])
            .collect();
        assert_eq!(s.ring, want);
        assert_eq!(s.plane, [0.0, 0.0, 1.0, at[2]]);
        assert_eq!(s.class, SurfaceClass::CollisionOnly);

        let ray = Ray {
            origin: [at[0] + 0.25, at[1] + 0.5, 10.0],
            dir: [0.0, 0.0, -1.0],
        };
        let mut layers = Layers::default();
        layers.set(LayerId::LevelCollision, true);
        // The floor below it is the structure's.
        assert_eq!(ed.pick_surface(&ray, &layers), Some(0));
        layers.set(LayerId::InstanceCollision, true);
        assert_eq!(ed.pick_surface(&ray, &layers), Some(3));
        let mut only = Layers::default();
        only.set(LayerId::CollisionOnly, true);
        assert_eq!(ed.pick_surface(&ray, &only), Some(3));
    }

    /// P11: the invented scenery's `coll` cube follows its node one unit
    /// above the root and its `phmo` sphere the root, both moved by the
    /// placement at `[5, 5, 5]`.
    #[test]
    fn an_objects_coll_and_phmo_are_drawn_at_its_placement() {
        use crate::object_collision::ShapeKind;
        let ed = open(fixture::fixture_with_object_collision(), "object");
        let objects = ed.object_collisions().unwrap();
        assert_eq!(objects.error, None);
        let scen = fixture::id(4);
        let model = objects.of(scen).unwrap();
        assert!(model.errors.is_empty(), "{:?}", model.errors);
        assert_eq!(model.coll, Some(fixture::id(6)));
        assert_eq!(model.phmo, Some(fixture::id(7)));
        assert_eq!(model.nodes, 2);
        assert_eq!(model.shapes.len(), 2);
        assert_eq!(ed.object_shape_count(), 2);

        let placed = ed.placed_object_shapes();
        assert_eq!(placed.len(), 2);
        let near = |a: Vec3, b: Vec3| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4);
        let cube = placed
            .iter()
            .find(|p| p.shape.kind == ShapeKind::Bsp)
            .unwrap();
        assert_eq!(cube.marker.block, BlockKind::Scenery);
        assert_eq!(cube.shape.node, Some(1));
        assert!(near(cube.center, [5.0, 5.0, 6.0]), "{:?}", cube.center);
        assert_eq!(cube.lines.len(), 24);
        assert!(
            cube.lines
                .iter()
                .flatten()
                .all(|p| (5.5..=6.5).contains(&p[2]) || (p[2] - 5.5).abs() < 1e-4),
            "the cube sits one unit up"
        );
        let sphere = placed
            .iter()
            .find(|p| p.shape.kind == ShapeKind::Sphere)
            .unwrap();
        assert_eq!(sphere.shape.node, Some(0));
        assert!(near(sphere.center, [5.0, 5.0, 5.5]), "{:?}", sphere.center);
        assert!((sphere.radius - fixture::SPHERE_RADIUS).abs() < 1e-6);
        for [a, _] in &sphere.lines {
            let d = crate::geometry::sub(*a, sphere.center);
            assert!((crate::geometry::dot(d, d).sqrt() - 0.5).abs() < 1e-4);
        }

        // The plain fixture's placements have no collision to draw.
        let plain = open(fixture::fixture(), "object-plain");
        assert_eq!(plain.object_shape_count(), 0);
    }

    /// Gap 4: a scenery that places nothing when the map opens has no
    /// collision read; switching it to the palette's crate reads the
    /// crate's `coll` and `phmo` then, and an undo keeps what was read.
    #[test]
    fn an_object_placed_after_the_map_opens_has_its_collision_read() {
        let mut bytes = fixture::fixture_with_object_collision();
        let at = fixture::sfile(fixture::SCENERY_AT);
        bytes[at..at + 2].copy_from_slice(&(-1i16).to_le_bytes());
        let mut ed = open(bytes, "object-late");
        let scen = fixture::id(4);
        assert_eq!(ed.object_collisions().unwrap().of(scen), None);
        assert_eq!(ed.object_shape_count(), 0);

        let field = ed
            .scenario()
            .unwrap()
            .block(BlockKind::Scenery)
            .def
            .role(Role::PaletteIndex)
            .unwrap()
            .name;
        ed.apply(Command::set(BlockKind::Scenery, 0, field, Value::Int(0)))
            .unwrap();
        let objects = ed.object_collisions().unwrap();
        assert_eq!(objects.error, None);
        let model = objects.of(scen).unwrap();
        assert!(model.errors.is_empty(), "{:?}", model.errors);
        assert_eq!(model.shapes.len(), 2);
        assert_eq!(ed.object_shape_count(), 2);
        assert_eq!(ed.placed_object_shapes().len(), 2);

        assert!(ed.undo());
        assert_eq!(ed.object_shape_count(), 0);
        assert!(ed.object_collisions().unwrap().of(scen).is_some());
        assert!(ed.redo());
        assert_eq!(ed.object_shape_count(), 2);
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
