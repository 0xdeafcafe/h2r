//! `render.scene`'s source (docs/runtime/agent-api.md): the frame step's
//! last sealed `FrameScene` (`render_bridge::scene_step`, docs/graphics/framescene.md),
//! summarised per view, and written whole as JSON on `dump`; with the
//! scene stages each typed stage queued to the render thread and dropped
//! there (`d3d::scene_stage`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use agent_proto::{SceneReply, SceneViewRecord};
use frame_scene::FrameScene;

/// Registers the source with the agent.
pub fn install() {
    agent::render::set_scene_source(source);
}

fn source(dump: bool) -> Result<SceneReply, String> {
    let counters: BTreeMap<String, u64> = render_bridge::scene_step::counters()
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .chain(d3d::scene_stage::counters())
        .collect();
    let Some(scene) = render_bridge::scene_step::last() else {
        return Ok(SceneReply { counters, ..SceneReply::default() });
    };
    let mut reply = reply(&scene, counters);
    if dump {
        reply.dump = Some(write_dump(&scene)?);
    }
    Ok(reply)
}

/// The summary of `scene`.
fn reply(scene: &FrameScene, counters: BTreeMap<String, u64>) -> SceneReply {
    SceneReply {
        sealed: true,
        map_epoch: scene.id.map_epoch as u32,
        frame: scene.id.frame,
        tick: scene.id.tick,
        hash: format!("{:016x}", frame_scene::summary_hash(scene)),
        cut: scene.cut.names().into_iter().map(str::to_owned).collect(),
        views: scene
            .views
            .iter()
            .map(|v| {
                let c = scene.counts(v);
                SceneViewRecord {
                    key: v.key.to_string(),
                    cut: v.cut,
                    clusters: c.clusters,
                    instances: c.instances,
                    parts: c.parts,
                    rows: c.rows,
                    lights: c.lights,
                    shadows: c.shadows,
                    light_layers: light_layers(scene, v),
                    layer_items: c.layers.iter().fold(BTreeMap::new(), |mut m, &(layer, items, _)| {
                        *m.entry(layer.to_string()).or_insert(0) += items;
                        m
                    }),
                    layer_remainder: c.layers.iter().map(|&(_, _, r)| r).sum(),
                    remainder: c.remainder,
                    part_flags: c.flagged.iter().map(|&(name, n)| (name.to_owned(), n)).collect(),
                    carried: BTreeMap::new(),
                    camera_use: BTreeMap::new(),
                }
            })
            .collect(),
        counters,
        dump: None,
    }
}

/// The view's lights by each layer their passes draw.
fn light_layers(scene: &FrameScene, view: &frame_scene::View) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    for light in scene.lights_of(view) {
        for step in scene.steps_of(light).iter().filter(|s| s.draws) {
            *out.entry(step.layer.to_string()).or_insert(0) += 1;
        }
    }
    out
}

/// The cache directory: `$XDG_CACHE_HOME/h2recomp`, else
/// `~/Library/Caches/h2recomp` on macOS and `~/.cache/h2recomp` elsewhere.
fn cache_dir() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(x).join("h2recomp");
    }
    match std::env::var_os("HOME") {
        Some(h) if cfg!(target_vendor = "apple") => PathBuf::from(h).join("Library/Caches/h2recomp"),
        Some(h) => PathBuf::from(h).join(".cache/h2recomp"),
        None => std::env::temp_dir().join("h2recomp"),
    }
}

fn write_dump(scene: &FrameScene) -> Result<String, String> {
    let dir = cache_dir().join("scenes");
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let path = dir.join(format!("scene-{}-{}.json", scene.id.frame, scene.id.tick));
    let json = serde_json::to_vec(scene).map_err(|e| format!("scene to JSON: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{
        InstanceInput, InstanceKey, ItemKey, LayerItem, LayerOwner, LayerPass, LightStep, LightTarget, NodeMatrix, PartFlags,
        RenderCamera, SceneBuilder, SceneId, SceneLight, Stamp, ViewKey, ViewKind,
    };

    #[test]
    fn a_scene_summarises_per_view() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId { map_epoch: 3, frame: 40, tick: 12 });
        let camera = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.0,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 100.0,
        };
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera).unwrap();
        b.set_clusters([1, 2]).unwrap();
        let nodes = [NodeMatrix::IDENTITY];
        let i = b
            .push_instance(InstanceInput {
                key: InstanceKey(5),
                definition: 1,
                model: 2,
                nodes: &nodes,
                sphere: ([0.0; 3], 1.0),
                lighting: None,
                defaults: None,
            })
            .unwrap();
        let key = ItemKey { record: 1, shader: 2, section: 0, part: 0, implementation: 0 };
        b.push_part(i, 4, key, 3, &[[0.0; 4]; 3]).unwrap();
        b.push_part_flagged(i, 5, key, 3, PartFlags::STENCIL_MARK, None, &[[0.0; 4]; 3]).unwrap();
        b.add_remainder(1).unwrap();
        let step = |layer| LightStep { layer, stencil: Some(0), target: LightTarget::View, draws: true };
        let light = |handle, flags| SceneLight {
            handle, definition: 9, flags, definition_flags: 0, additional_casters: false, steps: 0..0,
        };
        b.push_light(light(1, 4), &[step(11), step(12)]).unwrap();
        b.push_light(light(2, 0), &[step(12)]).unwrap();
        b.push_layer(LayerPass { layer: 5, stencil_off: false, texaccum: false, items: 0..0, remainder: 0 }).unwrap();
        let world = LayerItem { slot: 0, key, owner: LayerOwner::Cluster(3), model: u32::MAX, rows: 0..0, lighting: None };
        b.push_layer_item(world.clone(), &[]).unwrap();
        b.push_layer_item(LayerItem { slot: 1, ..world }, &[]).unwrap();
        b.add_layer_remainder(1).unwrap();
        let scene = b.seal(Stamp::default()).unwrap();
        let r = reply(&scene, BTreeMap::new());
        assert!(r.sealed);
        assert_eq!((r.map_epoch, r.frame, r.tick), (3, 40, 12));
        assert_eq!(r.hash.len(), 16);
        let v = &r.views[0];
        assert_eq!(v.key, "game:0:0");
        assert_eq!((v.clusters, v.instances, v.parts, v.rows, v.remainder), (2, 1, 2, 6, 1));
        assert_eq!((v.lights, v.shadows), (2, 1));
        assert_eq!(v.light_layers, BTreeMap::from([("11".to_owned(), 1), ("12".to_owned(), 2)]));
        assert_eq!((v.layer_items.clone(), v.layer_remainder), (BTreeMap::from([("5".to_owned(), 2)]), 1));
        assert_eq!(v.part_flags, BTreeMap::from([("stencil_mark".to_owned(), 1)]));
        assert!(serde_json::to_string(&*scene).unwrap().contains("\"slot\":4"));
    }
}
