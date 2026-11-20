//! A scene's summary hash: what `render.scene` and `render.capture`
//! report, so two runs can say whether they extracted the same frame.
//! FNV-1a over the content, in order: the tick, the cut, and every view's
//! key, camera, clusters, world state, remainder, auxiliary depth,
//! instances (key, definition, model, nodes, sphere, lighting, defaults),
//! parts (slot, key, layer, flags, subparts, rows), lights (handle,
//! definition, flags, additional casters, steps) and layer draws (layer,
//! flags, remainder, items with their owner, model, rows and lighting)
//! and decorator draws (`crate::decorator::summary_words`). Floats by their
//! bits. Not the map epoch, the present number or the stamp: they count
//! this process, not the frame.

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
    for v in &scene.views {
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
        for l in lights {
            h.word(l.handle);
            h.word(l.definition);
            h.word(l.flags);
            h.word(l.definition_flags);
            h.word(u32::from(l.additional_casters));
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
        }
        let passes = scene.layers_of(v);
        h.word(passes.len() as u32);
        for p in passes {
            h.word(u32::from(p.layer));
            h.word(u32::from(p.stencil_off));
            h.word(u32::from(p.texaccum));
            h.word(p.remainder);
            let items = scene.items_of(p);
            h.word(items.len() as u32);
            for i in items {
                h.word(i.slot);
                h.word(u32::from(i.key.record));
                h.word(i.key.shader);
                h.word(u32::from(i.key.section));
                h.word(u32::from(i.key.part));
                h.word(i.key.implementation as u32);
                match i.owner {
                    crate::layer::LayerOwner::Cluster(c) => h.word(0x1_0000 | u32::from(c)),
                    crate::layer::LayerOwner::Placement(c) => h.word(0x2_0000 | u32::from(c)),
                    crate::layer::LayerOwner::Object(k) => {
                        h.word(0x3_0000);
                        h.word(k.0);
                    }
                }
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
    }
    h.0
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
                handle: 4, definition: 5, flags: 0, definition_flags: 0, additional_casters: false, steps: 0..1,
            });
            s.views[0].lights = 0..1;
            summary_hash(&s)
        };
        assert_ne!(with(Some(0)), summary_hash(&scene(1, 0.0)), "a light");
        assert_ne!(with(Some(0)), with(None), "its stencil change");
    }
}
