//! The native BSP loader transfers ownership at the start of a render frame.
//! A receiver belongs to exactly one BSP activation, including same-map reverts.

use super::{BspKey, Ready, StagePalettes};
use crate::{
    native_decorators_glue::DecoratorLooks, native_fog_glue::FogView,
    native_layers_glue::LayersView, native_lights_glue::LightsWorld, native_water_glue::WaterData,
};
use native_render::{fog_gpu::FogSet, light_gpu::LightSet, water_gpu::WaterSet};
use std::sync::{Arc, mpsc};

pub(super) enum StageReady {
    World(Arc<Ready>),
    Lights(Arc<LightsWorld>, Box<LightSet>),
    Fog(Arc<FogView>, StagePalettes, FogSet),
    Water(Arc<WaterData>, StagePalettes, Box<WaterSet>),
    Ocean(Arc<Result<crate::native_ocean_glue::OceanData, String>>),
    Layers(Arc<LayersView>, StagePalettes, Vec<(u8, FogSet)>),
    Decorators(Arc<DecoratorLooks>),
    Sky(Arc<crate::native_sky_glue::SkyMaterials>),
}

#[derive(Default)]
pub struct MapState {
    pub(super) key: Option<BspKey>,
    pub(super) ready: Option<Arc<Ready>>,
    pending: Option<mpsc::Receiver<StageReady>>,
}

impl MapState {
    pub(super) fn loading(key: BspKey, pending: mpsc::Receiver<StageReady>) -> Self {
        Self {
            key: Some(key),
            ready: None,
            pending: Some(pending),
        }
    }

    pub(super) fn take_stale(&mut self) -> Option<Self> {
        self.key.as_ref().is_some_and(|key| !key.current()).then(|| std::mem::take(self))
    }

    pub(super) fn retire(self) {
        if self.key.is_some() {
            crate::native_map::retire_owned(timeline::setup::Stage::World, self);
        }
    }

    pub(super) fn publish(&mut self) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if !self.key.as_ref().is_some_and(BspKey::current) {
            return;
        }
        let _handoff = timeline::setup::dispatch();
        if self.ready.is_none() {
            match pending.try_recv() {
                Ok(StageReady::World(world)) => {
                    let key = self.key.as_ref().expect("a pending load has a key");
                    if let Some(receivers) = world.sun_world.as_ref() {
                        native_render::sun_casters::set_world(key.generation, Some(receivers));
                    }
                    self.ready = Some(world);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    return;
                }
                _ => return,
            }
        }
        let ready = self.ready.as_ref().expect("world was published");
        // A render thread may still be encoding the previous frame. Leave
        // additions in the queue until another boundary if it holds the GPU.
        let Ok(mut gpu) = ready.gpu.try_lock() else {
            return;
        };
        let Some(gpu) = gpu.as_mut() else { return };
        loop {
            match pending.try_recv() {
                Ok(StageReady::Lights(view, set)) => {
                    gpu.set_lights(*set);
                    let _ = ready.lights.set(view);
                }
                Ok(StageReady::Fog(view, palettes, set)) => {
                    gpu.set_fog(set);
                    let _ = ready.fog.set((view, palettes));
                }
                Ok(StageReady::Water(view, palettes, set)) => {
                    gpu.set_water(*set);
                    let _ = ready.water.set((view, palettes));
                }
                Ok(StageReady::Layers(view, palettes, sets)) => {
                    for (layer, set) in sets {
                        gpu.set_layer(layer, set);
                    }
                    let _ = ready.layers.set((view, palettes));
                }
                Ok(StageReady::Ocean(ocean)) => {
                    let _ = ready.ocean.set(ocean);
                }
                Ok(StageReady::Decorators(view)) => {
                    let _ = ready.decorators.set(view);
                }
                Ok(StageReady::Sky(view)) => {
                    let _ = ready.sky.set(view);
                }
                Ok(StageReady::World(_)) => unreachable!("a BSP worker emits its world once"),
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use render_bridge::native_map_work;

    fn key() -> BspKey {
        BspKey {
            map: "synthetic".into(),
            bsp: 0,
            index: 0,
            generation: native_map_work::generation(),
            bsp_generation: native_map_work::bsp_generation(),
        }
    }

    #[test]
    fn the_last_packet_sends_its_world_to_retirement_once() {
        let (retire, receive) = mpsc::channel();
        let mut world = Ready::default();
        world.retire = Some(retire);
        let world = Arc::new(world);
        let packet = Arc::clone(&world);
        drop(world);
        assert!(matches!(receive.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(packet);
        let (stage, retired) = receive.try_recv().expect("last release sends the world");
        assert!(matches!(stage, timeline::setup::Stage::World));
        drop(retired);
        assert!(matches!(
            receive.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn a_completed_load_waits_for_a_frame_boundary() {
        let (send, receive) = mpsc::channel();
        let mut state = MapState::loading(key(), receive);
        state.publish();
        let world = Arc::new(Ready::default());
        assert!(send.send(StageReady::World(Arc::clone(&world))).is_ok());
        assert!(state.ready.is_none());
        state.publish();
        assert!(Arc::ptr_eq(state.ready.as_ref().unwrap(), &world));
    }

    #[test]
    fn map_and_bsp_activation_detach_the_world_and_unpublished_mailbox() {
        let current = key();
        let old_map = BspKey { generation: current.generation.wrapping_sub(1), ..current.clone() };
        let old_bsp = BspKey { bsp_generation: current.bsp_generation.wrapping_sub(1), ..current };
        for old_key in [old_map, old_bsp] {
            let (send, receive) = mpsc::channel();
            let mut state = MapState::loading(old_key, receive);
            let ready = Arc::new(Ready::default());
            state.ready = Some(Arc::clone(&ready));
            let published = Arc::downgrade(&ready);
            let queued = Arc::new(Ready::default());
            let pending = Arc::downgrade(&queued);
            assert!(send.send(StageReady::World(queued)).is_ok());

            let retired = state.take_stale().expect("activation detaches the old BSP");
            assert!(state.key.is_none() && state.ready.is_none() && state.pending.is_none());
            drop(retired);
            assert!(send.send(StageReady::World(Arc::new(Ready::default()))).is_err());
            assert!(pending.upgrade().is_none());
            drop(ready);
            assert!(published.upgrade().is_none());
        }
    }

    #[test]
    fn a_replacement_load_owns_a_fresh_mailbox() {
        let (send, receive) = mpsc::channel();
        let retired = MapState::loading(key(), receive);
        let (_, receive) = mpsc::channel();
        let mut current = MapState::loading(key(), receive);
        assert!(
            send.send(StageReady::World(Arc::new(Ready::default())))
                .is_ok()
        );
        current.publish();
        assert!(current.ready.is_none());
        drop(retired);
        assert!(
            send.send(StageReady::World(Arc::new(Ready::default())))
                .is_err()
        );
    }
}
