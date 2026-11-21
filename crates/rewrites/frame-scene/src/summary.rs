//! A scene's summary hash: what `render.scene` and `render.capture`
//! report, so two runs can say whether they extracted the same frame.
//! FNV-1a over the content, in order: the tick, the cut, and every view's
//! key, camera, clusters, world state, remainder, auxiliary depth,
//! instances (key, definition, model, nodes, sphere, lighting, defaults),
//! parts (slot, key, layer, flags, subparts, rows), lights (handle,
//! definition, flags, candidate inputs and constants, guest selection,
//! additional casters, steps, gathered groups), object
//! shadows (identity, groups), layer draws (layer, flags, remainder,
//! material inputs, items with their owner, model, rows and lighting) and
//! decorator draws (`crate::decorator::summary_words`), and the late passes
//! (fog, water and post: `crate::late::summary_words`). A group hashes its
//! layer, site, projection, remainder and entries (slot, record, owner,
//! model, stream, pose with its rows, binding).
//! Floats by their
//! bits. Not the map epoch, the present number or the stamp: they count
//! this process, not the frame.

use crate::light::{Light, LightConstants, LightVolume};
use crate::light_group::{LightGroup, LightPose};
use crate::object::{Defaults, Lighting};
use crate::scene::FrameScene;

struct Fnv(u64);

impl Fnv {
    fn word(&mut self, w: u32) {
        for b in w.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn float(&mut self, f: f32) {
        self.word(f.to_bits());
    }
    fn floats(&mut self, f: &[f32]) {
        for &x in f {
            self.float(x);
        }
    }
}

/// The summary hash of `scene`.
pub fn summary_hash(scene: &FrameScene) -> u64 {
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    h.word(scene.id.tick);
    h.word(u32::from(scene.cut.0));
    h.word(scene.views.len() as u32);
    for (vi, v) in scene.views.iter().enumerate() {
        h.word(v.key.kind as u32);
        h.word(u32::from(v.key.player));
        h.word(u32::from(v.key.view));
        h.word(u32::from(v.cut));
        let c = &v.camera;
        h.floats(&c.position);
        h.floats(&c.forward);
        h.floats(&c.up);
        h.floats(&[c.fov, c.near, c.far]);
        for x in c.viewport {
            h.word(x as u16 as u32);
        }
        let clusters = scene.clusters_of(v);
        h.word(clusters.len() as u32);
        for &c in clusters {
            h.word(u32::from(c));
        }
        h.word(u32::from(v.world.texaccum));
        h.word(v.remainder);
        match &v.aux_depth {
            Some(a) => {
                h.word(1);
                h.float(a.scale);
                for r in a.rows.iter().chain(&a.rest_rows) {
                    h.floats(r);
                }
                h.floats(&a.clip);
                h.floats(&a.rest_clip);
            }
            None => h.word(0),
        }
        let instances = scene.instances_of(v);
        h.word(instances.len() as u32);
        for i in instances {
            h.word(i.key.0);
            h.word(i.definition);
            h.word(i.model);
            let nodes = scene.nodes_of(i);
            h.word(nodes.len() as u32);
            for n in nodes {
                h.floats(bytemuck::cast_ref::<_, [f32; 13]>(n));
            }
            h.floats(&i.sphere.0);
            h.float(i.sphere.1);
            lighting(&mut h, i.lighting.as_ref());
            defaults(&mut h, i.defaults.as_ref());
            h.word(u32::from(i.teleported));
            let parts = scene.parts_of(i);
            h.word(parts.len() as u32);
            for p in parts {
                h.word(p.slot);
                h.word(u32::from(p.key.record));
                h.word(p.key.shader);
                h.word(u32::from(p.key.section));
                h.word(u32::from(p.key.part));
                h.word(p.key.implementation as u32);
                h.word(u32::from(p.layer));
                h.word(u32::from(p.flags.0));
                match p.subparts {
                    Some(sp) => {
                        h.word(1);
                        h.word(u32::from(sp.first));
                        h.word(u32::from(sp.count));
                        h.word(sp.visible as u32);
                        h.word((sp.visible >> 32) as u32);
                    }
                    None => h.word(0),
                }
                let rows = scene.rows_of(p);
                h.word(rows.len() as u32);
                for r in rows {
                    h.floats(r);
                }
            }
        }
        let lights = scene.lights_of(v);
        h.word(lights.len() as u32);
        for (k, l) in lights.iter().enumerate() {
            h.word(l.handle);
            h.word(l.definition);
            h.word(l.flags);
            h.word(l.definition_flags);
            h.word(u32::from(l.additional_casters));
            light_inputs(&mut h, l);
            let steps = scene.steps_of(l);
            h.word(steps.len() as u32);
            for s in steps {
                h.word(u32::from(s.layer));
                h.word(s.stencil.map_or(u32::MAX, u32::from));
                h.word(match s.target {
                    crate::light::LightTarget::ShadowMap => 0x100,
                    crate::light::LightTarget::View => 0x200,
                    crate::light::LightTarget::Target(n) => u32::from(n),
                });
                h.word(u32::from(s.draws));
            }
            light_groups(&mut h, scene, scene.groups_of(v.lights.start + k as u32));
        }
        let shadows = scene.object_shadows_of(vi as u32);
        h.word(shadows.len() as u32);
        for o in shadows {
            h.word(o.identity[0]);
            h.word(o.identity[1]);
            light_groups(&mut h, scene, scene.shadow_groups_of(o));
        }
        let passes = scene.layers_of(v);
        h.word(passes.len() as u32);
        for p in passes {
            h.word(u32::from(p.layer));
            h.word(u32::from(p.stencil_off));
            h.word(u32::from(p.texaccum));
            h.word(p.remainder);
            inputs(&mut h, &p.inputs);
            let items = scene.items_of(p);
            h.word(items.len() as u32);
            for i in items {
                h.word(i.slot);
                h.word(u32::from(i.key.record));
                h.word(i.key.shader);
                h.word(u32::from(i.key.section));
                h.word(u32::from(i.key.part));
                h.word(i.key.implementation as u32);
                owner(&mut h, i.owner);
                h.word(i.model);
                let rows = slice_rows(scene, &i.rows);
                h.word(rows.len() as u32);
                for r in rows {
                    h.floats(r);
                }
                lighting(&mut h, i.lighting.as_ref());
            }
        }
        crate::decorator::summary_words(scene, v, &mut |w| h.word(w));
        crate::late::summary_words(scene, v, &mut |w| h.word(w));
    }
    h.0
}

fn light_inputs(h: &mut Fnv, light: &Light) {
    h.word(u32::from(light.guest_selected));
    optional(h, light.guest_weight, |h, v| h.float(v));
    let Some(c) = &light.candidate else {
        h.word(0);
        return;
    };
    h.word(1);
    for v in [&c.position, &c.forward, &c.up, &c.colour_a, &c.colour_b, &c.bounds.0] {
        h.floats(v);
    }
    h.float(c.intensity);
    h.float(c.bounds.1);
    h.float(c.weight);
    optional(h, c.priority, |h, v| h.float(v));
    h.word(u32::from(c.definition_kind));
    optional(h, c.view_eligible, |h, v| h.word(u32::from(v)));
    optional(h, c.volume.as_ref(), |h, volume| {
        h.word(u32::from(volume.kind()));
        match volume {
            LightVolume::Sphere { origin, radius } => {
                h.floats(origin);
                h.float(*radius);
            }
            LightVolume::Projected { origin, axis, near, far, far_extent, .. } => {
                h.floats(origin);
                h.floats(axis);
                h.floats(&[*near, *far]);
                h.floats(far_extent);
            }
        }
    });
    optional(h, c.owner, |h, v| h.word(v.0));
    optional(h, c.gel_bitmap, Fnv::word);
    optional(h, c.gel_shader, Fnv::word);
    optional(h, c.constants.as_ref(), |h, variants| {
        light_constants(h, &variants.unshadowed);
        light_constants(h, &variants.shadowed);
        optional(h, variants.projection.as_ref(), |h, projection| {
            for &word in projection.rows.iter().flatten() {
                h.word(word);
            }
            h.word(u32::from(projection.mode));
            for size in projection.size {
                h.word(size as u32);
            }
        });
    });
}

fn optional<T>(h: &mut Fnv, value: Option<T>, hash: impl FnOnce(&mut Fnv, T)) {
    h.word(u32::from(value.is_some()));
    if let Some(value) = value {
        hash(h, value);
    }
}

fn light_constants(h: &mut Fnv, c: &LightConstants) {
    for &word in c.frame.iter().chain(&c.colour_a).chain(&c.colour_b) {
        h.word(word);
    }
    for word in [
        c.flags, c.mode, c.definition_flags, u32::from(c.definition_8a),
        u32::from(c.definition_80), c.falloff_0, u32::from(c.animated_gel),
    ] {
        h.word(word);
    }
    optional(h, c.gel, Fnv::word);
}

fn light_groups(h: &mut Fnv, scene: &FrameScene, groups: &[LightGroup]) {
    h.word(groups.len() as u32);
    for g in groups {
        h.word(u32::from(g.layer));
        h.word(g.site as u32);
        match &g.projection {
            Some(p) => {
                h.word(1);
                for w in p.iter().flatten() {
                    h.word(*w);
                }
            }
            None => h.word(0),
        }
        h.word(g.remainder);
        let entries = scene.entries_of(g);
        h.word(entries.len() as u32);
        for e in entries {
            h.word(u32::from(e.slot));
            h.word(u32::from(e.record));
            owner(h, e.owner);
            h.word(e.model);
            let st = &e.stream;
            for w in [u32::from(st.section), u32::from(st.part), u32::from(st.vertex_class), st.first_index, st.index_count] {
                h.word(w);
            }
            h.word(st.topology as u32);
            match &e.pose {
                LightPose::World => h.word(0),
                LightPose::Placement(p) => h.word(0x1_0000 | u32::from(*p)),
                LightPose::Rows(_) => {
                    let rows = scene.pose_rows(e);
                    h.word(0x2_0000 | rows.len() as u32);
                    for r in rows {
                        h.floats(r);
                    }
                }
            }
            let b = &e.binding;
            for w in [b.shader, u32::from(b.selector), u32::from(b.variant), u32::from(b.implementation)] {
                h.word(w);
            }
        }
    }
}

fn owner(h: &mut Fnv, o: crate::layer::LayerOwner) {
    match o {
        crate::layer::LayerOwner::Cluster(c) => h.word(0x1_0000 | u32::from(c)),
        crate::layer::LayerOwner::Placement(c) => h.word(0x2_0000 | u32::from(c)),
        crate::layer::LayerOwner::Object(k) => {
            h.word(0x3_0000);
            h.word(k.0);
        }
    }
}

fn slice_rows<'a>(scene: &'a FrameScene, r: &std::ops::Range<u32>) -> &'a [crate::scene::Row] {
    scene.rows.get(r.start as usize..r.end as usize).unwrap_or(&[])
}

fn lighting(h: &mut Fnv, l: Option<&Lighting>) {
    let Some(l) = l else {
        h.word(0);
        return;
    };
    h.word(1);
    for &w in &l.words {
        h.word(w);
    }
    match l.root {
        Some(r) => {
            h.word(1);
            h.word(r);
        }
        None => h.word(0),
    }
}

/// A layer draw's material inputs: the time's bits, then each given
/// input after its mask.
fn inputs(h: &mut Fnv, i: &crate::layer::LayerInputs) {
    match i.time {
        Some(t) => {
            let bits = t.to_bits();
            h.word(1);
            h.word(bits as u32);
            h.word((bits >> 32) as u32);
        }
        None => h.word(0),
    }
    for mask in [i.scalars_given, i.vectors_given] {
        h.word(mask as u32);
        h.word((mask >> 32) as u32);
    }
    for (n, &w) in i.scalars.iter().enumerate() {
        if i.scalars_given >> n & 1 != 0 {
            h.word(w);
        }
    }
    for (n, v) in i.vectors.iter().enumerate() {
        if i.vectors_given >> n & 1 != 0 {
            for &w in v {
                h.word(w);
            }
        }
    }
    h.word(u32::from(i.ints_given));
    for (n, &w) in i.ints.iter().enumerate() {
        if i.ints_given >> n & 1 != 0 {
            h.word(w);
        }
    }
    defaults(h, i.defaults.as_ref());
}

fn defaults(h: &mut Fnv, d: Option<&Defaults>) {
    let Some(d) = d else {
        h.word(0);
        return;
    };
    h.word(1);
    for w in d.vector.into_iter().chain(d.colour) {
        h.word(w);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::builder::{InstanceInput, SceneBuilder};
    use crate::camera::tests::camera;
    use crate::node::NodeMatrix;
    use crate::object::ItemKey;
    use crate::scene::{InstanceKey, SceneId, Stamp, ViewKey, ViewKind};

    fn scene(frame: u64, x: f32) -> Arc<FrameScene> {
        let mut b = SceneBuilder::default();
        b.begin(SceneId { map_epoch: frame, frame, tick: 7 });
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera()).unwrap();
        let nodes = [NodeMatrix { position: [x, 0.0, 0.0], ..NodeMatrix::IDENTITY }];
        let i = b
            .push_instance(InstanceInput {
                key: InstanceKey(9),
                definition: 1,
                model: 2,
                nodes: &nodes,
                sphere: ([0.0; 3], 1.0),
                lighting: None,
                defaults: None,
            })
            .unwrap();
        let key = ItemKey { record: 1, shader: 2, section: 0, part: 0, implementation: 1 };
        b.push_part(i, 3, key, 3, &[[1.0, 2.0, 3.0, 4.0]]).unwrap();
        b.seal(Stamp { published: Some(std::time::Instant::now()), ticks_run: 1 }).unwrap()
    }

    #[test]
    fn the_hash_follows_the_content_not_the_process() {
        assert_eq!(summary_hash(&scene(1, 0.0)), summary_hash(&scene(5, 0.0)), "epoch, frame and stamp do not count");
        assert_ne!(summary_hash(&scene(1, 0.0)), summary_hash(&scene(1, 0.5)), "a node moved");
        assert_ne!(summary_hash(&scene(1, 0.0)), summary_hash(&scene(1, -0.0)), "floats by their bits");
    }

    #[test]
    fn a_lights_steps_count_in_the_hash() {
        use crate::light::{LightStep, LightTarget, SceneLight};
        let with = |stencil| {
            let mut s = (*scene(1, 0.0)).clone();
            s.light_steps.push(LightStep { layer: 12, stencil, target: LightTarget::View, draws: true });
            s.lights.push(SceneLight {
                handle: 4, definition: 5, flags: 0, definition_flags: 0, additional_casters: false, steps: 0..1, guest_selected: true, ..Default::default()
            });
            s.views[0].lights = 0..1;
            summary_hash(&s)
        };
        assert_ne!(with(Some(0)), summary_hash(&scene(1, 0.0)), "a light");
        assert_ne!(with(Some(0)), with(None), "its stencil change");
    }

    #[test]
    fn a_layer_draws_material_inputs_count_in_the_hash() {
        use crate::layer::{LayerInputs, LayerPass};
        let with = |inputs: LayerInputs| {
            let mut s = (*scene(1, 0.0)).clone();
            s.layer_passes.push(LayerPass { layer: 5, stencil_off: false, texaccum: false, items: 0..0, remainder: 0, inputs });
            s.views[0].layers = 0..1;
            summary_hash(&s)
        };
        let none = with(LayerInputs::default());
        assert_ne!(with(LayerInputs { time: Some(1.5), ..LayerInputs::default() }), none, "the render time");
        let mut given = LayerInputs::default();
        given.set_scalar(12, 1.0f32.to_bits());
        assert_ne!(with(given), none, "a given scalar");
        let mut hidden = LayerInputs::default();
        hidden.scalars[12] = 1.0f32.to_bits();
        assert_eq!(with(hidden), none, "a word not given does not count");
    }

    #[test]
    fn a_lights_groups_and_an_object_shadows_count_in_the_hash() {
        use crate::layer::LayerOwner;
        use crate::light::SceneLight;
        use crate::light_group::{GroupSite, LightBinding, LightEntry, LightStream, Topology};
        let entry = LightEntry {
            slot: 1,
            record: 2,
            owner: LayerOwner::Cluster(3),
            model: u32::MAX,
            stream: LightStream { section: 3, part: 0, vertex_class: 0, first_index: 0, index_count: 6, topology: Topology::List },
            pose: LightPose::World,
            binding: LightBinding { shader: 4, selector: 0, variant: 0, implementation: 2 },
        };
        let group = |light, site| LightGroup { light, layer: 10, site, projection: None, entries: 0..1, remainder: 0 };
        let base = || {
            let mut s = (*scene(1, 0.0)).clone();
            s.lights.push(SceneLight {
                handle: 4, definition: 5, flags: 4, definition_flags: 0, additional_casters: false, steps: 0..0, ..Default::default()
            });
            s.views[0].lights = 0..1;
            s.light_entries.push(entry.clone());
            s
        };
        let plain = summary_hash(&base());
        let mut lit = base();
        lit.light_groups.push(group(0, GroupSite::Light));
        let lit_hash = summary_hash(&lit);
        assert_ne!(lit_hash, plain, "a group");
        lit.light_entries[0].binding.implementation = 3;
        assert_ne!(summary_hash(&lit), lit_hash, "its entry's binding");
        lit.light_entries[0].binding.implementation = 2;
        lit.light_groups[0].projection = Some([[1; 4]; crate::light_group::PROJECTION_ROWS]);
        assert_ne!(summary_hash(&lit), lit_hash, "its projection");
        let mut shadowed = base();
        shadowed.object_shadows.push(crate::light_group::ObjectShadow { view: 0, identity: [1, 2], groups: 0..1 });
        shadowed.shadow_groups.push(group(0, GroupSite::ObjectShadow));
        let shadow_hash = summary_hash(&shadowed);
        assert_ne!(shadow_hash, plain, "an object shadow");
        assert_ne!(shadow_hash, lit_hash, "not the light's group");
        shadowed.object_shadows[0].identity = [1, 3];
        assert_ne!(summary_hash(&shadowed), shadow_hash, "its identity");
    }
}
