//! `render.scene`'s source (docs/runtime/agent-api.md): the frame step's
//! last sealed `FrameScene` (`render_bridge::scene_step`, docs/graphics/framescene.md),
//! summarised per view (with its fog, water and post passes, slice 5), and
//! written whole as JSON on `dump`; with the
//! scene stages each typed stage queued to the render thread and dropped
//! there (`d3d::scene_stage`); with `frame-scene.census`, the entry
//! census (`render_bridge::entry_census`) over the device's count of draws
//! handed to the D3D backend; and with `frame-scene.sink`, the last
//! frame's carried part as the device's scene sink filed it
//! (`d3d::scene_sink`, slice 6). The live mirror's `camera` layer reads
//! the same last sealed scene ([`install_cameras`]).

use std::collections::BTreeMap;

use agent_proto::{
    SceneCarried, SceneEntries, SceneEntryRecord, SceneLateRecord, SceneOpCounts, ScenePostRecord, SceneReply, SceneSideTarget,
    SceneViewRecord,
};
use frame_scene::{CarriedSummary, FrameScene, OpCounts, Readback, Slot, ViewKind};

/// Registers the source with the agent, the D3D draw counter with the
/// entry census, and `render.residual`'s wiring: the adapter's entry calls
/// to `render_bridge::residual_census`, its host hooks, and the function
/// lookup that names callers (docs/graphics/retire-d3d.md, slice R0).
pub fn install() {
    agent::render::set_scene_source(source);
    render_bridge::entry_census::set_draw_counter(d3d::device::backend_draws);
    d3d::residual::set_sink(residual_call);
    render_bridge::residual_census::set_host(render_bridge::residual_census::Host {
        arm: d3d::residual::arm,
        tick: || agent::render::HOSTED.last_tick(),
        map: || d3d::pipeline_store::current_map().1,
    });
    agent::render::residual::set_functions(recomp::fallback::function_at);
}

/// One armed entry call, forwarded (the device is locked; the census takes
/// only its own lock).
fn residual_call(c: &d3d::residual::EntryCall) {
    render_bridge::residual_census::call(&render_bridge::residual_census::Call {
        entry: c.name,
        ret: c.ret,
        inline: crate::d3d_glue::residual_tally(c.inline),
        own: crate::d3d_glue::residual_tally(c.own),
        ns: c.ns,
        ran: c.ran,
        present: c.present,
    });
}

/// The live mirror's `camera` layer (docs/editor/live-mirror.md,
/// "Cameras"): each view of the last sealed scene and the objects it drew.
/// The stream worker calls it; the sealed scene is a shared immutable
/// value, so the game thread does nothing for it.
pub fn install_cameras(agent: &agent::Agent) {
    agent.set_camera_source(std::sync::Arc::new(cameras));
}

fn cameras(out: &mut Vec<agent_proto::live::View>) -> Result<(), String> {
    if let Some(scene) = render_bridge::scene_step::last() {
        out.extend(scene.views.iter().map(|v| live_view(&scene, v)));
    }
    Ok(())
}

fn live_view(scene: &FrameScene, v: &frame_scene::View) -> agent_proto::live::View {
    // The order of agent_proto::live::VIEW_KINDS.
    let kind = match v.key.kind {
        ViewKind::Game => 0,
        ViewKind::Cinematic => 1,
        ViewKind::Alternate => 2,
        ViewKind::FirstPerson => 3,
    };
    let c = &v.camera;
    agent_proto::live::View {
        key: agent_proto::live::view_key(kind, v.key.player, v.key.view),
        scene_tick: scene.id.tick,
        position: c.position,
        forward: c.forward,
        up: c.up,
        fov: c.fov,
        near: c.near,
        far: c.far,
        viewport: c.viewport,
        cut: v.cut,
        drawn: scene.instances_of(v).iter().map(|i| i.key.0).collect(),
    }
}

/// The entry census, when it is on.
fn entries() -> Option<SceneEntries> {
    let (counts, unbalanced) = render_bridge::entry_census::snapshot()?;
    Some(SceneEntries {
        by_entry: counts
            .into_iter()
            .map(|(name, c)| {
                (name, SceneEntryRecord {
                    calls: c.calls,
                    d3d_draws: c.d3d_draws,
                    seed_steps: c.seed_steps,
                    effects_seed_steps: c.effects_seed_steps,
                    seed_unmatched: c.seed_unmatched,
                    animation_gets: c.animation_gets,
                    particle_updates: c.particle_updates,
                })
            })
            .collect(),
        unbalanced,
        draws_counted: render_bridge::entry_census::draws_counted(),
    })
}

fn counters(sink: bool) -> BTreeMap<String, u64> {
    let mut counts: BTreeMap<String, u64> = render_bridge::scene_step::counters()
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .chain(d3d::scene_stage::counters())
        .chain(d3d::gap::counters())
        .chain(render_bridge::native_layers::counters())
        .chain(render_bridge::native_hud::kept::counters())
        .chain(render_bridge::native_targets::counters())
        .chain(render_bridge::native_flare_query::counters())
        .chain(render_bridge::native_flare_draw::counters())
        .chain(render_bridge::native_transparents::sort::counters())
        .chain(render_bridge::native_hud::walk::counters())
        .chain(render_bridge::native_hud::strings::counters())
        .chain(crate::hud_native::refusal_counters())
        .chain(render_bridge::native_sky::counters())
        .chain(render_bridge::native_first_person::counters())
        .chain(render_bridge::native_world::stipple_counters().map(|(k, v)| (k.to_owned(), v)))
        .chain(sink.then(d3d::scene_sink::counters).into_iter().flatten())
        .chain(render_bridge::render_frame::publish::counters())
        .collect();
    #[cfg(feature = "gpu")]
    counts.extend(crate::native_post_glue::counters());
    #[cfg(feature = "gpu")]
    counts.extend(native_render::world_layers_gpu::pool_recording_counters());
    #[cfg(feature = "gpu")]
    if sink { counts.extend(crate::native_objects_glue::palette_counters()); }
    if sink && let Some(residency) = d3d::scene_stage::residency::counters() {
        counts.extend(residency.map(|(name, n)| (name.to_owned(), n)));
    }
    if sink && let Some(window) = present_window() { counts.extend(window.counters()); }
    if let Some(queue) = crate::frame_owner::private_mailbox_stats() {
        counts.insert("scene_private_published".into(), queue.published);
        counts.insert("scene_private_taken".into(), queue.taken);
        counts.insert("scene_private_replaced".into(), queue.dropped);
        counts.insert("scene_private_waiting".into(), queue.waiting as u64);
    }
    if let Some(owner) = d3d::threaded::owner::stats() {
        counts.insert("scene_owned_presented".into(), owner.presented);
        counts.insert("scene_owned_declined".into(), owner.declined);
        counts.insert("scene_owned_flare_queries".into(), owner.flare_queries);
        counts.insert("scene_owned_candidates".into(), owner.candidates);
        counts.insert("scene_owned_barriers".into(), owner.barriers);
        counts.insert("scene_owned_mismatches".into(), owner.mismatches);
        counts.insert("scene_owned_target_refusals".into(), owner.target_refusals);
    }
    #[cfg(feature = "gpu")]
    if d3d::threaded::owner::stats().is_some() {
        counts.extend(d3d::scaled::scene_counters());
    }
    counts
}

type Window = d3d::scene_sink::window::PresentWindow;
static WINDOW: std::sync::Mutex<std::sync::Weak<Window>> = std::sync::Mutex::new(std::sync::Weak::new());

pub(crate) fn set_present_window(window: Option<&std::sync::Arc<Window>>) {
    *WINDOW.lock().unwrap_or_else(|e| e.into_inner()) = window.map(std::sync::Arc::downgrade).unwrap_or_default();
}

fn present_window() -> Option<std::sync::Arc<Window>> {
    WINDOW.lock().unwrap_or_else(|e| e.into_inner()).upgrade()
}

pub(crate) fn next_game() {
    if let Some(window) = present_window() { window.next_game(); }
}

static ACTIVATION: std::sync::OnceLock<agent_proto::SceneCensusActivation> = std::sync::OnceLock::new();

/// Called by level boot at the same boundary as its first-level-loaded mark.
/// Taking the baseline here preserves the earliest frames after activation
/// even if an agent attaches much later.
pub(crate) fn level_activated(map: &str, present: u64) {
    if render_bridge::carried_slots::slot().is_some() {
        ACTIVATION.get_or_init(|| {
            if let Some(window) = present_window() { window.activate(); }
            let mut c = counters(true);
            let frame = c["scene_carried_frames"];
            c.retain(|k, _| {
                k.starts_with("scene_dropped_") || k.starts_with("scene_readback")
                    || k.starts_with("scene_owned_") || k.starts_with("scene_residency_") || k == "scene_unresident" || k == "scene_carried_game_frames"
            });
            agent_proto::SceneCensusActivation { map: map.into(), present, frame, counters: c }
        });
    }
}

fn source(dump: bool) -> Result<SceneReply, String> {
    // An inactive sink must not report zero measured readbacks.
    let sink = render_bridge::carried_slots::slot().is_some();
    let counters = counters(sink);
    let summary = if sink { d3d::scene_sink::last() } else { None };
    let census = if sink { d3d::scene_sink::census().map(carried_census) } else { None };
    let checkpoint = if sink {
        present_window().and_then(|w| w.checkpoint()).map(|p| agent_proto::SceneCensusCheckpoint {
            present: d3d::scene_sink::window::CORPUS_PRESENT,
            sealed: p.sealed,
            inventory: p.inventory.map(|c| carried_census((*c).clone())),
            counters: p.counters,
        })
    } else {
        None
    };
    let Some(scene) = render_bridge::scene_step::last() else {
        return Ok(SceneReply {
            counters,
            entries: entries(),
            carried: summary.as_ref().map(carried),
            carried_census: census,
            carried_checkpoint: checkpoint,
            carried_activation: ACTIVATION.get().cloned(),
            visibility_first_mismatch: render_bridge::scene_step::visibility_first_mismatch(),
            light_visibility_first_mismatch: render_bridge::scene_step::light_visibility_first_mismatch(),
            first_person_visibility_first_mismatch: render_bridge::native_lights::slot_visibility::first_mismatch(),
            view_record_first_mismatch: render_bridge::scene_step::view_record_first_mismatch(),
            ..SceneReply::default()
        });
    };
    let mut reply = reply(&scene, counters, summary.as_ref());
    reply.entries = entries();
    reply.carried_census = census;
    reply.carried_checkpoint = checkpoint;
    reply.carried_activation = ACTIVATION.get().cloned();
    if dump {
        reply.dump = Some(write_dump(&scene)?);
    }
    Ok(reply)
}

fn ops(c: &OpCounts) -> SceneOpCounts {
    SceneOpCounts { draws: c.draws, binds: c.binds, clears: c.clears, copies: c.copies, uploads: c.uploads, samples: c.samples }
}

/// By name, the non-zero entries of a per-[`Readback`] count.
fn readbacks(counts: &[u32; Readback::COUNT]) -> BTreeMap<String, u32> {
    Readback::ALL.iter().zip(counts).filter(|(_, n)| **n != 0).map(|(k, n)| (k.name().to_owned(), *n)).collect()
}

/// By slot name, the non-zero entries of a per-[`Slot`] count.
fn by_slot(counts: impl IntoIterator<Item = u32>) -> BTreeMap<String, u32> {
    Slot::ALL.iter().zip(counts).filter(|(_, n)| *n != 0).map(|(s, n)| (s.name().to_owned(), n)).collect()
}

/// The sink's last frame (docs/graphics/framescene.md, slice 6).
fn carried(s: &CarriedSummary) -> SceneCarried {
    SceneCarried {
        slots: Slot::ALL
            .iter()
            .zip(&s.slots)
            .filter(|(_, c)| c.total() != 0)
            .map(|(slot, c)| (slot.name().to_owned(), ops(c)))
            .collect(),
        side_targets: s
            .side_targets
            .iter()
            .map(|t| SceneSideTarget {
                aux: u32::from(t.aux),
                width: t.target.width,
                height: t.target.height,
                format: t.target.format,
                depth: t.target.depth,
                ops: by_slot(t.ops),
            })
            .collect(),
        readbacks: readbacks(&s.readbacks),
        readbacks_outside: readbacks(&s.readbacks_outside),
    }
}

fn carried_census(census: d3d::scene_sink::Census) -> agent_proto::SceneCarriedCensus {
    let uses = |u: d3d::scene_sink::Uses| agent_proto::SceneUses {
        count: u.count, first_frame: u.first_frame, last_frame: u.last_frame,
    };
    agent_proto::SceneCarriedCensus {
        frames: census.frames,
        game_frames: census.game_frames,
        targets: census.targets.into_iter().map(|(t, u)| agent_proto::SceneTargetUse {
            width: t.width, height: t.height, format: t.format, depth: t.depth,
            slot: t.slot.name().into(), operation: t.operation.into(), uses: uses(u),
        }).collect(),
        reads: census.reads.into_iter().map(|((slot, kind), u)| agent_proto::SceneReadUse {
            slot: slot.name().into(), kind: kind.into(), uses: uses(u),
        }).collect(),
    }
}

/// A game view's carried ops by slot, from the sink's frame: those of the
/// guest view of its index.
fn view_carried(summary: Option<&CarriedSummary>, view: &frame_scene::View) -> BTreeMap<String, u32> {
    let Some(s) = summary.filter(|_| view.key.kind == ViewKind::Game) else { return BTreeMap::new() };
    s.views.iter().find(|(v, _)| *v == view.key.view).map_or_else(BTreeMap::new, |(_, slots)| by_slot(slots.iter().map(OpCounts::total)))
}

/// The summary of `scene`, with the sink's last frame when it records.
fn reply(scene: &FrameScene, counters: BTreeMap<String, u64>, summary: Option<&CarriedSummary>) -> SceneReply {
    let hash = format!("{:016x}", frame_scene::summary_hash(scene));
    let private_frame = agent::render::private_graph::last_stats().filter(|p| {
        p.map_epoch == scene.id.map_epoch && p.frame == scene.id.frame && p.tick == scene.id.tick && p.scene_hash == hash
    });
    let display_frame = agent::render::private_graph::display_stats().filter(|p| {
        p.map_epoch == scene.id.map_epoch && p.frame == scene.id.frame && p.tick == scene.id.tick
    });
    SceneReply {
        display_frame,
        private_frame,
        sealed: true,
        map_epoch: scene.id.map_epoch as u32,
        frame: scene.id.frame,
        tick: scene.id.tick,
        hash,
        cut: scene.cut.names().into_iter().map(str::to_owned).collect(),
        views: scene
            .views
            .iter()
            .enumerate()
            .map(|(index, v)| {
                let gather = scene.cluster_gathers.get(index).and_then(Option::as_ref);
                let c = scene.counts(v);
                let d = scene.decorator_counts(v);
                SceneViewRecord {
                    key: v.key.to_string(),
                    cut: v.cut,
                    clusters: c.clusters,
                    visibility_clusters: gather.map(|g| g.visible.len() as u32),
                    structure_clusters: gather.map(|g| g.structure.len() as u32),
                    visibility_objects: gather.map(|g| g.objects.len() as u32),
                    visibility_source: gather.map(|g| g.source.name().to_owned()),
                    instances: c.instances,
                    parts: c.parts,
                    rows: c.rows,
                    lights: c.lights,
                    lights_candidates: c.lights_candidates,
                    lights_with_constants: c.lights_with_constants,
                    shadows: c.shadows,
                    light_layers: light_layers(scene, v),
                    layer_items: c.layers.iter().fold(BTreeMap::new(), |mut m, &(layer, items, _)| {
                        *m.entry(layer.to_string()).or_insert(0) += items;
                        m
                    }),
                    layer_remainder: c.layers.iter().map(|&(_, _, r)| r).sum(),
                    remainder: c.remainder,
                    part_flags: c.flagged.iter().map(|&(name, n)| (name.to_owned(), n)).collect(),
                    carried: view_carried(summary, v),
                    camera_use: if v.key.kind == ViewKind::Game {
                        crate::frame_owner::camera_counts(scene.id, v.key.view).map_or_else(BTreeMap::new, |counts| {
                            ["screen", "view", "unknown"].into_iter().map(str::to_owned).zip(counts).collect()
                        })
                    } else {
                        BTreeMap::new()
                    },
                    decorators: d.draws,
                    decorator_instances: d.instances,
                    decorator_remainder: d.remainder,
                    fog: v.fog.as_ref().map(|l| late(scene, l)),
                    water: v.water.as_ref().map(|l| late(scene, l)),
                    post: v.post.as_ref().map(|p| ScenePostRecord {
                        colour: p.colour,
                        late: p.late,
                        view_kind: p.view_kind,
                        screen_effects: p.screen_effects,
                        screen_effects_gated: p.screen_effects_gated,
                        colour_pass: p.colour_pass.is_some(),
                        effects: p.effects.len() as u32,
                    }),
                }
            })
            .collect(),
        counters,
        entries: None,
        carried: summary.map(carried),
        carried_census: None,
        carried_checkpoint: None,
        carried_activation: None,
        visibility_first_mismatch: render_bridge::scene_step::visibility_first_mismatch(),
        light_visibility_first_mismatch: render_bridge::scene_step::light_visibility_first_mismatch(),
        first_person_visibility_first_mismatch: render_bridge::native_lights::slot_visibility::first_mismatch(),
        view_record_first_mismatch: render_bridge::scene_step::view_record_first_mismatch(),
        dump: None,
    }
}

/// A late layer's summary (docs/graphics/framescene.md, slice 5).
fn late(scene: &FrameScene, layer: &frame_scene::LateLayer) -> SceneLateRecord {
    let c = scene.late_counts(layer);
    SceneLateRecord {
        draws: c.draws,
        items: c.items,
        materials: c.materials,
        parts: c.parts,
        drawn: c.drawn,
        kept: c.kept,
        device_words: c.device_words,
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

fn write_dump(scene: &FrameScene) -> Result<String, String> {
    let dir = agent_proto::cache_dir().join("scenes");
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
    fn the_camera_layer_takes_each_views_camera_and_drawn_handles() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId {
            map_epoch: 2,
            frame: 9,
            tick: 77,
        });
        let camera = RenderCamera {
            position: [1.0, 2.0, 3.0],
            forward: [0.0, 1.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.2,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.0625,
            far: 1024.0,
        };
        b.open_view(
            ViewKey {
                kind: ViewKind::FirstPerson,
                player: 1,
                view: 2,
            },
            camera,
        )
        .unwrap();
        let nodes = [NodeMatrix::IDENTITY];
        for key in [0x0001_0005, 0x0002_0009] {
            b.push_instance(InstanceInput {
                key: InstanceKey(key),
                definition: 1,
                model: 2,
                nodes: &nodes,
                sphere: ([0.0; 3], 1.0),
                lighting: None,
                defaults: None,
            })
            .unwrap();
        }
        let scene = b.seal(Stamp::default()).unwrap();
        let view = live_view(&scene, &scene.views[0]);
        assert_eq!(view.name(), "first_person:1:2");
        assert_eq!(view.scene_tick, 77);
        assert_eq!(
            (view.position, view.forward, view.up),
            (camera.position, camera.forward, camera.up)
        );
        assert_eq!(
            (view.fov, view.near, view.far, view.viewport),
            (camera.fov, camera.near, camera.far, camera.viewport)
        );
        assert_eq!(view.cut, scene.views[0].cut);
        assert_eq!(view.drawn, [0x0001_0005, 0x0002_0009]);
    }

    #[test]
    fn a_scene_summarises_per_view() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId { map_epoch: 3, frame: 40, tick: 12 });
        let camera = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.0,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 100.0,
        };
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera).unwrap();
        b.set_clusters([1, 2]).unwrap();
        b.set_cluster_gather(frame_scene::scene::ClusterGather {
            mode: 0, visible: vec![1, 2, 3, 4], structure: vec![3, 1, 2],
            objects: vec![0x0001_0005], source: frame_scene::VisibilitySource::Rust,
        }).unwrap();
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
            handle, definition: 9, flags, definition_flags: 0, additional_casters: false, steps: 0..0, ..Default::default()
        };
        b.push_light(light(1, 4), &[step(11), step(12)]).unwrap();
        b.push_light(light(2, 0), &[step(12)]).unwrap();
        b.push_layer(LayerPass { layer: 5, stencil_off: false, texaccum: false, items: 0..0, remainder: 0, inputs: Default::default() }).unwrap();
        let world = LayerItem {
            slot: 0, key, level: 0, pass: 0, owner: LayerOwner::Cluster(3), model: u32::MAX, rows: 0..0, lighting: None,
            flags: frame_scene::PartFlags::NONE, subparts: None, refusal: None, prt: None,
        };
        b.push_layer_item(world.clone(), &[]).unwrap();
        b.push_layer_item(LayerItem { slot: 1, ..world }, &[]).unwrap();
        b.add_layer_remainder(1).unwrap();
        let decorator = frame_scene::DecoratorDraw {
            slot: 6, layer: 3, set: 1, shader: 2, variant: 0, pass: 0, argument: 0, lod_constant: [0.0; 4],
            sphere: [0.0, 0.0, 0.0, f32::INFINITY], instances: 0..0,
        };
        let instance = frame_scene::DecoratorInstance { index: 0, model: 0, first_index: 0, index_count: 3, registers: [[0.0; 4]; 5] };
        b.push_decorator(decorator, &[instance, instance]).unwrap();
        b.add_decorator_remainder(1).unwrap();
        let key = ViewKey { kind: ViewKind::Game, player: 0, view: 0 };
        let parts = [frame_scene::LatePart { owner: frame_scene::Owner::Cluster(3), part: 0 }];
        let device = [Some(1), Some(3), Some(0), Some(0), Some(0), Some(0), None];
        b.set_late(key, 16, frame_scene::LateInput { draws: true, device, items: Some(4), materials: &[], parts: &parts }).unwrap();
        b.late_drawn(key, 16).unwrap();
        b.set_post(key, frame_scene::PostParams {
            colour: true, late: false, view_kind: 1, screen_effects: 1, screen_effects_gated: true, colour_pass: None, effects: 0..0,
        })
        .unwrap();
        b.post_colour(key, [0; 7]).unwrap();
        let scene = b.seal(Stamp::default()).unwrap();
        let r = reply(&scene, BTreeMap::new(), None);
        assert!(r.sealed);
        assert_eq!((r.map_epoch, r.frame, r.tick), (3, 40, 12));
        assert_eq!(r.hash.len(), 16);
        let v = &r.views[0];
        assert_eq!(v.key, "game:0:0");
        assert_eq!((v.clusters, v.instances, v.parts, v.rows, v.remainder), (2, 1, 2, 6, 1));
        assert_eq!((v.visibility_clusters, v.structure_clusters), (Some(4), Some(3)));
        assert_eq!((v.visibility_objects, v.visibility_source.as_deref()), (Some(1), Some("rust")));
        let dump = serde_json::to_value(&*scene).unwrap();
        assert_eq!(dump["cluster_gathers"][0]["structure"], serde_json::json!([3, 1, 2]));
        assert_eq!((v.lights, v.shadows), (2, 1));
        assert_eq!((v.lights_candidates, v.lights_with_constants), (2, 0));
        assert_eq!(v.light_layers, BTreeMap::from([("11".to_owned(), 1), ("12".to_owned(), 2)]));
        assert_eq!((v.layer_items.clone(), v.layer_remainder), (BTreeMap::from([("5".to_owned(), 2)]), 1));
        assert_eq!(v.part_flags, BTreeMap::from([("stencil_mark".to_owned(), 1)]));
        assert_eq!((v.decorators, v.decorator_instances, v.decorator_remainder), (1, 2, 1));
        let fog = v.fog.expect("the fog ran");
        assert_eq!((fog.draws, fog.items, fog.parts, fog.drawn, fog.kept, fog.device_words), (true, 4, 1, true, 3, 6));
        assert!(v.water.is_none(), "the water did not run");
        let post = v.post.expect("the post passes ran");
        assert_eq!((post.colour, post.colour_pass, post.effects, post.view_kind), (true, true, 0, 1));
        assert!(serde_json::to_string(&*scene).unwrap().contains("\"slot\":4"));
        assert!(r.carried.is_none() && v.carried.is_empty(), "no sink, no carried part");
    }

    #[test]
    fn the_sinks_frame_is_reported_by_slot_and_per_game_view() {
        use frame_scene::{AuxTarget, Carried, GuestOp, SlotAt, TargetRef};
        let at = |slot, view| SlotAt { slot, view };
        let mut c: Carried<()> = Carried::default();
        c.aux.push(AuxTarget { width: 128, height: 64, format: 6, depth: false });
        c.push(at(Slot::Opaque, 0), GuestOp::Bind { colour: TargetRef::SceneColour, depth: TargetRef::SceneDepth });
        c.push(at(Slot::Opaque, 0), GuestOp::Draw(()));
        c.push(at(Slot::AfterLights, 0), GuestOp::Clear { colour: TargetRef::Aux(0), depth: TargetRef::None, rects: false });
        c.push(at(Slot::Overlay, 0), GuestOp::Draw(()));
        c.readback(at(Slot::Transparents, 0), Readback::PixelCount);
        let s = c.summary();
        let k = carried(&s);
        assert_eq!(k.slots.keys().map(String::as_str).collect::<Vec<_>>(), ["after_lights", "opaque", "overlay"]);
        assert_eq!(k.slots["opaque"], SceneOpCounts { draws: 1, binds: 1, ..Default::default() });
        assert_eq!(k.side_targets.len(), 1);
        assert_eq!((k.side_targets[0].width, k.side_targets[0].height), (128, 64));
        assert_eq!(k.side_targets[0].ops, BTreeMap::from([("after_lights".to_owned(), 1)]));
        assert_eq!(k.readbacks, BTreeMap::from([("pixel_count".to_owned(), 1)]));
        assert!(k.readbacks_outside.is_empty());

        let mut b = SceneBuilder::default();
        b.begin(SceneId { map_epoch: 1, frame: 2, tick: 3 });
        let camera = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.0,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 100.0,
        };
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera).unwrap();
        b.open_view(ViewKey { kind: ViewKind::FirstPerson, player: 0, view: 0 }, camera).unwrap();
        let scene = b.seal(Stamp::default()).unwrap();
        let r = reply(&scene, BTreeMap::new(), Some(&s));
        assert_eq!(r.views[0].carried, BTreeMap::from([("opaque".to_owned(), 2), ("after_lights".to_owned(), 1)]));
        assert!(r.views[1].carried.is_empty(), "the first person's view is the game view's guest view");
        assert_eq!(r.carried, Some(k));
    }
}
