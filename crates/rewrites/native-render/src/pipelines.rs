//! Native pipeline compilation and per-program prewarming. The private disk
//! archive contains shader fingerprints and exact pipeline descriptors. The
//! shader and its bind group layouts come from the current map and device.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, Weak, mpsc};

use gfx::pipeline_store::PipelineStore;
use gfx::compile::{Compiler, Pending, PendingPipeline, Priority};
use gfx::pipeline::{PipelineKey, ShaderProgram, create_render_pipeline};

static COMPILER: LazyLock<Arc<Compiler>> = LazyLock::new(|| {
    Compiler::with_worker_setup(background::workers(), "native pipeline", background::lower_current)
});
static ARCHIVE: OnceLock<Option<mpsc::SyncSender<Message>>> = OnceLock::new();
const ARCHIVE_QUEUE: usize = 256;
const ARCHIVE_RECIPES: usize = 8192;
const PROGRAM_PIPELINES: usize = 256;

enum Message {
    Warm(Weak<Program>),
    Record(u64, PipelineKey),
}

/// Install the compile workers before the first draw. The disk archive is
/// optional and only its worker opens or updates it.
pub fn start(path: Option<PathBuf>) {
    LazyLock::force(&COMPILER);
    let Some(path) = path else { return };
    ARCHIVE.get_or_init(|| {
        let (send, receive) = mpsc::sync_channel(ARCHIVE_QUEUE);
        match background::spawn("native pipeline archive", move || run_archive(path, receive, ARCHIVE_RECIPES)) {
            Ok(_) => Some(send),
            Err(error) => {
                eprintln!("native pipeline archive: {error}");
                None
            }
        }
    });
}

/// The native archive's record format (`gfx::pipeline_store` versions every
/// line): bump it when a fingerprint or a stored descriptor changes meaning.
const ARCHIVE_VERSION: u32 = 1;

fn run_archive(path: PathBuf, receive: mpsc::Receiver<Message>, limit: usize) {
    let _scope = mem_watch::scope("render.pipeline");
    let (mut store, loaded) = PipelineStore::open::<u64>(&path, ARCHIVE_VERSION);
    let mut recipes: HashMap<u64, HashSet<PipelineKey>> = HashMap::new();
    let mut recorded = loaded.pipelines.len();
    for item in loaded.pipelines.into_iter().rev().take(limit) {
        if let Some(id) = item.program {
            recipes.entry(id).or_default().insert(item.key);
        }
    }
    for message in receive {
        match message {
            Message::Warm(weak) => {
                if let Some(program) = weak.upgrade()
                    && let Some(keys) = recipes.get(&program.id)
                {
                    for key in keys.iter().take(PROGRAM_PIPELINES) {
                        program.request(key, Priority::Background);
                    }
                }
            }
            Message::Record(id, key) => {
                if recorded < limit && recipes.entry(id).or_default().insert(key.clone()) {
                    recorded += 1;
                    store.record_program(id, &id, None);
                    store.record(Some(id), false, &key);
                }
            }
        }
    }
}

/// FNV-1a over the stage family, entry points and WGSL. A collision can
/// only schedule an extra compile: the live program owns all GPU handles.
fn fingerprint(source: &str, shader: &ShaderProgram, label: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for field in [label, &shader.vs_entry, &shader.fs_entry, source] {
        for byte in field.bytes().chain(std::iter::once(0)) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

static NATIVE_PIPELINES: mem_watch::Counter = mem_watch::Counter::new("native.pipeline_cache_entries");

struct Entry {
    _count: mem_watch::Gauge,
    pipeline: Arc<PendingPipeline>,
    priority: Priority,
}

struct Program {
    device: wgpu::Device,
    shader: ShaderProgram,
    label: &'static str,
    id: u64,
    // The archive can seed this table while the guest requests a variant.
    // The lock never covers shader compilation or file I/O.
    entries: Mutex<HashMap<PipelineKey, Entry>>,
}

impl Program {
    fn request(self: &Arc<Self>, key: &PipelineKey, priority: Priority) -> Arc<PendingPipeline> {
        let mut entries = self.entries.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = entries.get_mut(key) {
            if entry.priority == Priority::Background && priority == Priority::Draw {
                entry.priority = Priority::Draw;
                if !entry.pipeline.is_ready() && entry.pipeline.built_on().is_none() {
                    COMPILER.submit(&entry.pipeline, priority);
                }
            }
            return Arc::clone(&entry.pipeline);
        }
        if entries.len() == PROGRAM_PIPELINES {
            entries.clear();
        }
        // Queued jobs may outlive the map. They only retain this small key
        // until claimed; their device, modules and layouts belong to the map.
        let _scope = mem_watch::scope("render.pipeline");
        let weak = Arc::downgrade(self);
        let recipe = key.clone();
        let pipeline = Pending::new(move || {
            let _scope = mem_watch::scope("render.pipeline");
            let program = weak.upgrade()?;
            let _timer = timeline::setup::Stage::Pipelines.worker();
            let result = gfx::program::validated_resource(&program.device, || {
                create_render_pipeline(&program.device, &recipe, &program.shader, program.label)
            });
            if result.is_some() && let Some(Some(send)) = ARCHIVE.get() {
                let _ = send.try_send(Message::Record(program.id, recipe));
            }
            result
        });
        COMPILER.submit(&pipeline, priority);
        entries.insert(key.clone(), Entry { _count: NATIVE_PIPELINES.track(1), pipeline: Arc::clone(&pipeline), priority });
        pipeline
    }
}

/// One live shader, layout and table of validated pipelines.
#[derive(Clone)]
pub struct Pipelines(Arc<Program>);

impl Pipelines {
    pub fn new(device: &wgpu::Device, source: &str, shader: ShaderProgram, label: &'static str) -> Self {
        let id = fingerprint(source, &shader, label);
        let program = Arc::new(Program {
            device: device.clone(), shader, label, id, entries: Mutex::default(),
        });
        if let Some(Some(send)) = ARCHIVE.get() {
            let _ = send.try_send(Message::Warm(Arc::downgrade(&program)));
        }
        Self(program)
    }

    /// Queue the exact live variant ahead of speculative prewarms. Admission
    /// reads `previous_frame`; recording only consumes admitted variants.
    pub fn request(&self, device: &wgpu::Device, key: &PipelineKey) -> Arc<PendingPipeline> {
        debug_assert_eq!(device, &self.0.device);
        // The live shader already selects the table. Diagnostic IDs assigned
        // by encounter order can differ when the same map is loaded again.
        let mut recipe = key.clone();
        recipe.shader = gfx::pipeline::ShaderId(0);
        self.0.request(&recipe, Priority::Draw)
    }
}

/// Queue live native shader translation and module creation on the same
/// low-QoS workers as pipeline creation.
pub fn background<T: Send + Sync + 'static>(build: impl FnOnce() -> T + Send + 'static) -> Arc<Pending<T>> {
    let pending = Pending::new(move || {
        let _timer = timeline::setup::Stage::Pipelines.worker();
        let _scope = mem_watch::scope("render.pipeline");
        build()
    });
    COMPILER.submit(&pending, Priority::Draw);
    pending
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::pipeline::ShaderId;
    use gfx::vertex::OwnedVertexBufferLayout;

    const SOURCE: &str = "
        @vertex fn vs_main(@location(0) p: vec4f) -> @builtin(position) vec4f { return p; }
        @fragment fn fs_main() -> @location(0) vec4f { return vec4f(1.0); }";

    fn program(device: &wgpu::Device) -> Pipelines {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None, source: wgpu::ShaderSource::Wgsl(SOURCE.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[], immediate_size: 0,
        });
        Pipelines::new(device, SOURCE, ShaderProgram {
            module, layout, vs_entry: "vs_main".into(), fs_entry: "fs_main".into(), count_entry: None,
        }, "archive test")
    }

    #[test]
    fn archive_recipe_limit_survives_reopening() {
        let key = PipelineKey {
            shader: ShaderId(0),
            vertex_buffers: Vec::new(),
            primitive: Default::default(),
            depth_stencil: None,
            color_target: wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL,
            },
            samples: 1,
        };
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("h2-native-archive-limit-{}-{stamp}", std::process::id()));
        for ids in [1..5, 5..10] {
            let (send, recv) = mpsc::channel();
            for id in ids {
                send.send(Message::Record(id, key.clone())).unwrap();
            }
            drop(send);
            run_archive(path.clone(), recv, 2);
            let (store, _) = PipelineStore::open::<u64>(&path, ARCHIVE_VERSION);
            assert_eq!(store.len(), (2, 2));
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_reloaded_shader_warms_the_exact_instanced_descriptor_from_disk() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let key = PipelineKey {
            shader: ShaderId(73),
            vertex_buffers: vec![OwnedVertexBufferLayout {
                stride: 16, step_mode: wgpu::VertexStepMode::Instance,
                attributes: wgpu::vertex_attr_array![0 => Float32x4].to_vec(),
            }],
            primitive: Default::default(), depth_stencil: None,
            color_target: wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL,
            },
            samples: 1,
        };
        let original = program(&ctx.device);
        assert!(original.request(&ctx.device, &key).get().is_some());
        let mut recipe = key.clone();
        recipe.shader = ShaderId(0);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("h2-native-archive-{}-{stamp}", std::process::id()));
        let (send, recv) = mpsc::channel();
        send.send(Message::Record(original.0.id, recipe.clone())).unwrap();
        drop(send);
        run_archive(path.clone(), recv, ARCHIVE_RECIPES);
        drop(original);
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains("vs_main"));
        assert!(file.contains("instance"));

        let reloaded = program(&ctx.device);
        let (send, recv) = mpsc::channel();
        send.send(Message::Warm(Arc::downgrade(&reloaded.0))).unwrap();
        drop(send);
        run_archive(path.clone(), recv, ARCHIVE_RECIPES);
        let warm = reloaded.0.entries.lock().unwrap()[&recipe].pipeline.clone();
        let mut current = key;
        current.shader = ShaderId(95);
        assert!(Arc::ptr_eq(&warm, &reloaded.request(&ctx.device, &current)));
        assert!(warm.get().is_some());
        std::fs::remove_file(path).unwrap();
    }
}
