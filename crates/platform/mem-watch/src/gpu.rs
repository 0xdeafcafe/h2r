//! Live wgpu resources and native renderer caches. Each renderer owns its
//! registration, so the watchdog cannot keep a closed window's device alive.

use std::collections::BTreeMap;

#[cfg(feature = "gpu")]
pub use devices::DeviceRegistration;

/// Bytes have the suffix `_bytes`. Each measure describes its own view of
/// GPU memory: driver allocation, HAL allocations, or renderer cache contents.
/// A process without a renderer returns an empty census.
pub fn census() -> BTreeMap<String, u64> {
    let mut totals = BTreeMap::new();
    #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
    devices::collect(&mut totals);
    for (name, value) in crate::probe_values() {
        if name.starts_with("native.") || name.starts_with("gfx.") {
            totals.insert(name, value);
        }
    }
    totals
}

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
mod devices {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{Arc, Mutex, Weak};

    struct TrackedDevice {
        instance: wgpu::Instance,
        device: wgpu::Device,
        backend: wgpu::Backend,
    }

    static DEVICES: Mutex<Vec<Weak<TrackedDevice>>> = Mutex::new(Vec::new());

    /// May be owned by several renderers sharing the same logical device.
    pub struct DeviceRegistration {
        _device: Arc<TrackedDevice>,
    }

    impl DeviceRegistration {
        pub fn new(
            instance: &wgpu::Instance,
            adapter: &wgpu::Adapter,
            device: &wgpu::Device,
        ) -> Self {
            let mut tracked = DEVICES.lock().unwrap_or_else(|err| err.into_inner());
            tracked.retain(|entry| entry.strong_count() != 0);
            if let Some(shared) = tracked
                .iter()
                .filter_map(Weak::upgrade)
                .find(|d| d.device == *device)
            {
                return Self { _device: shared };
            }
            let shared = Arc::new(TrackedDevice {
                instance: instance.clone(),
                device: device.clone(),
                backend: adapter.get_info().backend,
            });
            tracked.push(Arc::downgrade(&shared));
            Self { _device: shared }
        }
    }

    fn add(totals: &mut BTreeMap<String, u64>, key: &str, value: u64) {
        *totals.entry(key.to_owned()).or_default() += value;
    }

    pub(super) fn collect(totals: &mut BTreeMap<String, u64>) {
        // Release the registration lock before entering wgpu.
        let devices: Vec<_> = DEVICES
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        // wgpu's `Instance` orders by identity, not by its interior state.
        #[allow(clippy::mutable_key_type)]
        let mut instances = BTreeSet::new();
        #[cfg(target_os = "macos")]
        let mut metal_registry_ids = BTreeSet::new();

        for tracked in devices {
            if instances.insert(tracked.instance.clone())
                && let Some(report) = tracked.instance.generate_report()
            {
                let h = report.hub;
                for (name, registry) in [
                    ("wgpu.texture_count", h.textures),
                    ("wgpu.texture_view_count", h.texture_views),
                    ("wgpu.buffer_count", h.buffers),
                    ("wgpu.bind_group_count", h.bind_groups),
                    ("wgpu.bind_group_layout_count", h.bind_group_layouts),
                    ("wgpu.render_pipeline_count", h.render_pipelines),
                    ("wgpu.compute_pipeline_count", h.compute_pipelines),
                    ("wgpu.pipeline_cache_count", h.pipeline_caches),
                    ("wgpu.shader_module_count", h.shader_modules),
                    ("wgpu.sampler_count", h.samplers),
                    ("wgpu.command_buffer_count", h.command_buffers),
                ] {
                    add(totals, name, registry.num_kept_from_user as u64);
                }
            }

            // These backends maintain per-category byte counters. In wgpu 30
            // Metal maintains object counts but leaves both byte fields zero.
            if matches!(tracked.backend, wgpu::Backend::Vulkan | wgpu::Backend::Dx12) {
                let hal = tracked.device.get_internal_counters().hal;
                add(
                    totals,
                    "hal.buffer_bytes",
                    hal.buffer_memory.read().max(0) as u64,
                );
                add(
                    totals,
                    "hal.texture_bytes",
                    hal.texture_memory.read().max(0) as u64,
                );
            }

            #[cfg(target_os = "macos")]
            if tracked.backend == wgpu::Backend::Metal {
                use objc2_metal::MTLDevice;
                // SAFETY: Read-only queries on the live MTLDevice. The HAL
                // guard covers both calls and we never retain the raw handle.
                if let Some(hal) = unsafe { tracked.device.as_hal::<wgpu::hal::api::Metal>() } {
                    let metal = hal.raw_device();
                    // Logical devices on one GPU see the same driver total.
                    if metal_registry_ids.insert(metal.registryID()) {
                        add(
                            totals,
                            "metal.current_allocated_bytes",
                            metal.currentAllocatedSize() as u64,
                        );
                    }
                }
            }
        }
    }
}

// Browser WebGPU has no native device report or cross-thread device access.
#[cfg(all(feature = "gpu", target_arch = "wasm32"))]
mod devices {
    pub struct DeviceRegistration;

    impl DeviceRegistration {
        pub fn new(_: &wgpu::Instance, _: &wgpu::Adapter, _: &wgpu::Device) -> Self {
            Self
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_census_carries_renderer_cache_probes_and_no_others() {
        crate::register_probe("gfx.test_pipeline_entries", || 3);
        crate::register_probe("native.test_world_buffers", || 4);
        crate::register_probe("agent.test_pending", || 5);
        let census = super::census();
        assert_eq!(census.get("gfx.test_pipeline_entries"), Some(&3));
        assert_eq!(census.get("native.test_world_buffers"), Some(&4));
        assert!(!census.contains_key("agent.test_pending"));
    }
}
