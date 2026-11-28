//! The map and raw resource files behind the native render loaders.
//!
//! A live open serves every selector through the raw overlay
//! ([`vfs::RawOverlay::global`], docs/editor/realtime-edits.md, "The edit
//! overlay"), so a world, model or image loaded after a live raw edit
//! decodes the edited bytes; and reads the `Map` itself through it, so the
//! tag metadata it decodes holds every live tag write too.

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex, MutexGuard, Weak, mpsc};

use assets::{Region, ResourceFiles};
use map::{Map, ResourceFile, TagId};
use native_render::load::Error;
use vfs::{FileSource, Vfs};

use crate::pad_speaker::SHARED_MAPS;

pub trait MapReader: Read + Seek + Send {}
impl<R: Read + Seek + Send> MapReader for R {}

pub type NativeMap = Map<Box<dyn MapReader>>;

/// A queued GPU pass can hold the last reference to an old map after
/// its replacement has started drawing. Its final release sends the
/// resources here to be destroyed away from the render thread.
pub(crate) type RetireQueue = mpsc::Sender<(timeline::setup::Stage, Box<dyn Send>)>;

thread_local! {
    static RETIRING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn retire_queue() -> std::io::Result<RetireQueue> {
    static QUEUE: Mutex<Option<RetireQueue>> = Mutex::new(None);
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(send) = queue.as_ref() {
        return Ok(send.clone());
    }
    let (send, receive) = mpsc::channel::<(timeline::setup::Stage, Box<dyn Send>)>();
    background::spawn("native-map retire", move || {
        RETIRING.set(true);
        for (stage, payload) in receive {
            let _timer = stage.worker();
            drop(payload);
        }
    })?;
    *queue = Some(send.clone());
    Ok(send)
}

struct RetiredPayload<T> {
    _payload: T,
    _pending: mem_watch::Gauge,
}

fn tracked_retirement(payload: impl Send + 'static) -> Box<dyn Send> {
    static RETIRING_PAYLOADS: mem_watch::Counter =
        mem_watch::Counter::new("native.map_retired_payloads");
    Box::new(RetiredPayload { _payload: payload, _pending: RETIRING_PAYLOADS.track(1) })
}

/// Call from Drop after detaching the payload's sender so the worker
/// destroys the payload directly.
pub(crate) fn retire<T: Default + Send + 'static>(send: RetireQueue, stage: timeline::setup::Stage, payload: &mut T) {
    if !RETIRING.try_with(std::cell::Cell::get).unwrap_or(false) {
        let _ = send.send((stage, tracked_retirement(std::mem::take(payload))));
    }
}

/// Disconnects a map slot on activation and destroys its payload on the
/// retire worker. Passes already handed to the renderer keep their own pins.
pub(crate) fn retire_owned(stage: timeline::setup::Stage, payload: impl Send + 'static) {
    if let Ok(send) = retire_queue() {
        let _ = send.send((stage, tracked_retirement(payload)));
    }
}

struct RetireFence(mpsc::Sender<()>);

impl Drop for RetireFence {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

/// Map builds take turns per lane, so each lane's scratch allocation
/// headroom holds one build at a time. The world (BSP, screen effects) has
/// a lane of its own: it shares no state with the object definitions,
/// first-person models and light models, so a long object preload does not
/// keep the level on D3D. Waits on a loader thread for an older build in
/// the same lane to leave and for detached resources to be destroyed.
pub(crate) fn begin_load(
    stage: timeline::setup::Stage,
    current: impl Fn() -> bool,
) -> Option<MutexGuard<'static, ()>> {
    let guard = lane(stage).lock().unwrap_or_else(|e| e.into_inner());
    if !current() {
        return None;
    }
    let (done, wait) = mpsc::channel();
    retire_queue().ok()?.send((stage, Box::new(RetireFence(done)))).ok()?;
    wait.recv().ok()?;
    current().then_some(guard)
}

/// The loader lane a stage's build waits in: the world's, or the models'
/// (objects, first person, lights).
fn lane(stage: timeline::setup::Stage) -> &'static Mutex<()> {
    static WORLD: Mutex<()> = Mutex::new(());
    static MODELS: Mutex<()> = Mutex::new(());
    match stage {
        timeline::setup::Stage::World => &WORLD,
        _ => &MODELS,
    }
}

/// Resolve a live datum by its map tag index when the salt differs.
pub fn tag_of(map: &NativeMap, datum: u32) -> Option<TagId> {
    if let Ok(tag) = map.tag(TagId(datum)) {
        return Some(tag.id);
    }
    map.tags()
        .iter()
        .find(|tag| tag.id.0 & 0xffff == datum & 0xffff)
        .map(|tag| tag.id)
}

/// Boot owns the mount. Render workers can borrow it until boot returns.
pub struct MountedVfs {
    data_root: std::path::PathBuf,
    vfs: Arc<Mutex<Vfs>>,
    rebind: Option<vfs::MapRebind>,
    active: Mutex<Option<ActiveSource>>,
    /// The file the game loaded the running map from.
    loaded: Mutex<Option<LoadedSlot>>,
    /// The running map's name, with or without a recorded path.
    running: Mutex<Option<String>>,
}

/// The running map's name and the VFS path of the file its cache slot was
/// copied from (`map_slots::active_file`): a disc map, a mod map in the
/// data partition's content store, a Forge build, or a rewritten file.
#[derive(Clone)]
struct LoadedSlot {
    name: String,
    path: String,
}

#[derive(Clone)]
struct ActiveSource {
    name: String,
    path: String,
    generation: Option<vfs::HostFile>,
}

static SOURCES: Mutex<Weak<MountedVfs>> = Mutex::new(Weak::new());

#[must_use]
pub fn mount(vfs: Arc<Mutex<Vfs>>, rebind: Option<vfs::MapRebind>, data_root: &std::path::Path) -> Arc<MountedVfs> {
    let sources = Arc::new(MountedVfs {
        data_root: data_root.to_owned(),
        vfs,
        rebind,
        active: Mutex::new(None),
        loaded: Mutex::new(None),
        running: Mutex::new(None),
    });
    *SOURCES.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&sources);
    sources
}

/// The active boot's data root, including a command-line override.
pub(crate) fn data_root() -> Option<std::path::PathBuf> {
    let sources = SOURCES.lock().unwrap_or_else(|e| e.into_inner()).upgrade()?;
    Some(sources.data_root.clone())
}

/// A file of the mounted VFS as the guest reads it, the disc's rewrites
/// applied: `None` without a mount or for a file the VFS does not have.
pub fn guest_file(path: &str) -> Option<Vec<u8>> {
    let sources = SOURCES.lock().unwrap_or_else(|e| e.into_inner()).upgrade()?;
    read_from(&sources.vfs, path)
}

/// A file of `vfs` as the guest reads it. The VFS is held only to find
/// where the file is, never while it is read.
fn read_from(vfs: &Mutex<Vfs>, path: &str) -> Option<Vec<u8>> {
    let source = vfs.lock().unwrap_or_else(|e| e.into_inner()).file_source(path)?;
    read_source(source)
}

/// A VFS file's bytes: a host file's region, or the memory it is served from.
fn read_source(source: FileSource) -> Option<Vec<u8>> {
    match source {
        FileSource::Host(file) => {
            let mut reader = File::open(&file.path).ok()?;
            reader.seek(SeekFrom::Start(file.offset)).ok()?;
            let mut bytes = vec![0; usize::try_from(file.len).ok()?];
            reader.read_exact(&mut bytes).ok()?;
            Some(bytes)
        }
        FileSource::Memory(bytes) => Some(bytes.to_vec()),
        FileSource::Spliced(file) => file.read_all().ok(),
    }
}

/// Only the editor's activation observer publishes a private generation.
pub(crate) fn set_active_source(name: &str, path: Option<String>, generation: Option<vfs::HostFile>) {
    let Some(sources) = SOURCES.lock().unwrap_or_else(|e| e.into_inner()).upgrade() else {
        return;
    };
    *sources.active.lock().unwrap_or_else(|e| e.into_inner()) = path.map(|path| ActiveSource {
        name: name.to_owned(), path, generation,
    });
}

/// Publishes the file the game loaded `name` from, at each game start, so
/// every native reader opens that file rather than a path guessed from the
/// name. `None` when the slot's source is not a map path.
pub(crate) fn set_loaded_slot(name: &str, path: Option<String>) {
    let Some(sources) = SOURCES.lock().unwrap_or_else(|e| e.into_inner()).upgrade() else {
        return;
    };
    if path.is_none() {
        // Named, once per map: the readers then find the map by its name.
        static LOGGED: Mutex<String> = Mutex::new(String::new());
        let mut last = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
        if !last.eq_ignore_ascii_case(name) {
            eprintln!("native map: {name}: the active slot recorded no map path; the readers guess <cdrom>\\maps\\{name}.map");
            *last = name.to_owned();
        }
    }
    *sources.running.lock().unwrap_or_else(|e| e.into_inner()) = Some(name.to_owned());
    *sources.loaded.lock().unwrap_or_else(|e| e.into_inner()) =
        path.map(|path| LoadedSlot { name: name.to_owned(), path });
}

/// The running map's name, set at each game start: `None` before the
/// first or without a mount.
pub(crate) fn running_name() -> Option<String> {
    let sources = SOURCES.lock().unwrap_or_else(|e| e.into_inner()).upgrade()?;
    sources.running.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The boot's mount, or why there is none: no boot has mounted one, or
/// the boot that did has returned (its guest ended, perhaps faulted)
/// while a loader queued behind another map build was still waiting.
fn mounted(sources: &Weak<MountedVfs>) -> Result<Arc<MountedVfs>, Error> {
    if let Some(sources) = sources.upgrade() {
        return Ok(sources);
    }
    if Weak::ptr_eq(sources, &Weak::new()) {
        Err("the map's VFS is not mounted".into())
    } else {
        Err("the game ended before the load ran; its VFS is unmounted".into())
    }
}

pub fn open(name: &str) -> Result<(NativeMap, ResourceFiles), Error> {
    let sources = mounted(&SOURCES.lock().unwrap_or_else(|e| e.into_inner()))?;
    let active = sources.rebind.as_ref().filter(|r| r.has_ever_bound()).and_then(|_| {
        sources.active.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            .filter(|a| a.name.eq_ignore_ascii_case(name))
            .cloned()
    });
    // The running map is read from the file the game loaded it from.
    let slot = sources.loaded.lock().unwrap_or_else(|e| e.into_inner()).clone()
        .filter(|s| s.name.eq_ignore_ascii_case(name));
    open_resolved(&sources.vfs, name, slot.as_ref().map(|s| s.path.as_str()), active, true)
}

pub fn open_from_vfs(
    vfs: &Mutex<Vfs>,
    name: &str,
) -> Result<(NativeMap, ResourceFiles), Error> {
    open_source_from_vfs(vfs, name, None)
}

pub fn open_source_from_vfs(
    vfs: &Mutex<Vfs>,
    name: &str,
    slot_source: Option<&str>,
) -> Result<(NativeMap, ResourceFiles), Error> {
    open_resolved(vfs, name, slot_source, None, false)
}

fn open_resolved(
    vfs: &Mutex<Vfs>,
    name: &str,
    slot_source: Option<&str>,
    active: Option<ActiveSource>,
    live: bool,
) -> Result<(NativeMap, ResourceFiles), Error> {
    let _scope = mem_watch::scope("assets.tag_load");
    let activated = active.as_ref().and_then(|a| a.generation.as_ref())
        .and_then(|file| file.generation.clone());
    let (local, candidates, shared) = {
        let mut mounted = vfs.lock().unwrap_or_else(|e| e.into_inner());
        let mut candidates = Vec::new();
        let from_active_slot = active.and_then(|a| {
            a.generation.map(FileSource::Host).or_else(|| mounted.file_source(&a.path))
        });
        let local = if let Some(file) = from_active_slot {
            Some(file)
        } else if let Some(path) = slot_source {
            // The recorded slot: the file the game (or the film) loaded.
            // A path the VFS cannot serve is an error, not a name to guess.
            let file = mounted.file_source(path)
                .or_else(|| mounted.host_file(path).map(FileSource::Host));
            if file.is_none() {
                return Err(format!("the map's file {path} is not found").into());
            }
            file
        } else if let Some(path) = mounted.content_map_path(name) {
            mounted.file_source(&path)
        } else {
            let dir = format!("{}\\maps", vfs::CDROM_DEVICE);
            let path = format!("{dir}\\{name}.map");
            let local = mounted.file_source(&path)
                .or_else(|| mounted.host_file(&path).map(FileSource::Host));
            if local.is_none() {
                candidates = crate::map_override::disc_map_paths(&mut mounted, &dir);
            }
            local
        };
        // Raw pointers select fixed DVD paths, independent of the loaded map's
        // name and any installed content with the same name.
        let shared = SHARED_MAPS.map(|(file, path)| (file, mounted.file_source(path)));
        (local, candidates, shared)
    };
    let local = local.or_else(|| {
        candidates.into_iter().find_map(|path| {
            let file = vfs.lock().unwrap_or_else(|e| e.into_inner()).host_file(&path)?;
            // The foreground uses this VFS too. Release it before opening and
            // seeking a candidate, even when every map has a different name.
            crate::map_override::header_name(&file)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
                .then_some(FileSource::Host(file))
        })
    }).ok_or("the map's file is not found")?;

    if live && let FileSource::Host(file) = &local
        && file.generation.is_some() && file.generation != activated
    {
        // The DVD can publish the replacement before the old guest world
        // stops drawing. Its native jobs may use only the activated source.
        return Err("the editor map is waiting for guest activation".into());
    }
    // A live open's metadata is the file's with the live tag writes over it.
    let served = |reader: Box<dyn MapReader>, base: u64| -> std::io::Result<Box<dyn MapReader>> {
        if !live {
            return Ok(reader);
        }
        let name = crate::hot_glue::overlay_name(ResourceFile::Local, name);
        Ok(Box::new(vfs::OverlaidReader::new(reader, vfs::RawOverlay::global().clone(), &name, base)?))
    };
    let map = match &local {
        FileSource::Host(file) => {
            let reader: Box<dyn MapReader> = Box::new(BufReader::new(File::open(&file.path)?));
            Map::from_region(served(reader, file.offset)?, file.offset, file.len)?
        }
        FileSource::Memory(bytes) => {
            let reader: Box<dyn MapReader> = Box::new(Cursor::new(Arc::clone(bytes)));
            Map::from_reader(served(reader, 0)?)?
        }
        FileSource::Spliced(file) => {
            let reader: Box<dyn MapReader> = Box::new(BufReader::new(file.reader()?));
            Map::from_reader(served(reader, 0)?)?
        }
    };
    let mut files = ResourceFiles::new();
    register(&mut files, ResourceFile::Local, local)?;
    for (file, source) in shared {
        if let Some(source) = source {
            register(&mut files, file, source)?;
        }
    }
    if live {
        overlay(&mut files, name, vfs::RawOverlay::global());
    }
    Ok((map, files))
}

/// One file's view of the raw overlay, for `assets::ResourceFiles`.
struct Overlaid {
    overlay: vfs::RawOverlay,
    name: String,
}

impl assets::resource::ResourceOverlay for Overlaid {
    fn apply(&self, offset: u64, buf: &mut [u8]) {
        let served = buf.len();
        self.overlay.apply(&self.name, offset, buf, served);
    }
    fn end(&self) -> u64 {
        self.overlay.len(&self.name, 0)
    }
}

/// Serves `files`' four selectors through `overlay`, read at each read.
fn overlay(files: &mut ResourceFiles, map: &str, overlay: &vfs::RawOverlay) {
    for file in [
        ResourceFile::Local,
        ResourceFile::MainMenu,
        ResourceFile::Shared,
        ResourceFile::SinglePlayerShared,
    ] {
        files.set_overlay(file, Arc::new(Overlaid {
            overlay: overlay.clone(),
            name: crate::hot_glue::overlay_name(file, map),
        }));
    }
}

fn register(files: &mut ResourceFiles, file: ResourceFile, source: FileSource) -> assets::Result<()> {
    match source {
        FileSource::Host(f) => {
            let region = Region { path: f.path, base: f.offset, length: f.len };
            if f.generation.is_some() {
                files.set_pinned(file, region)
            } else {
                files.set(file, region)
            }
        }
        FileSource::Memory(bytes) => {
            files.set_bytes(file, bytes);
            Ok(())
        }
        FileSource::Spliced(spliced) => {
            files.set_source(file, Arc::new(Spliced::new(spliced)));
            Ok(())
        }
    }
}

/// A spliced map's served bytes as a resource file, its base opened once.
struct Spliced {
    file: vfs::SplicedFile,
    open: Mutex<Option<vfs::SplicedSource>>,
}

impl Spliced {
    fn new(file: vfs::SplicedFile) -> Self {
        Self { file, open: Mutex::new(None) }
    }
}

impl assets::ResourceSource for Spliced {
    fn len(&self) -> u64 {
        self.file.len()
    }
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if open.is_none() {
            *open = Some(self.file.open()?);
        }
        let source = open.as_mut().expect("opened above");
        content::patch::Source::read_exact_at(source, offset, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_override::tests::{Temp, named_map};

    /// A loader that runs after its boot returned names the ended game,
    /// not a VFS that was never there (a mod map's world load queued
    /// behind its object preload outlived a guest fault).
    #[test]
    fn an_open_after_the_boot_returned_names_the_ended_game() {
        let never = mounted(&Weak::new()).err().expect("no boot mounted a VFS");
        assert_eq!(never.to_string(), "the map's VFS is not mounted");
        let mount = mount_into(Arc::new(Mutex::new(Vfs::new())));
        let weak = Arc::downgrade(&mount);
        assert!(mounted(&weak).is_ok());
        drop(mount);
        let ended = mounted(&weak).err().expect("the boot dropped its mount");
        assert!(ended.to_string().contains("the game ended"), "{ended}");
    }

    fn mount_into(vfs: Arc<Mutex<Vfs>>) -> Arc<MountedVfs> {
        Arc::new(MountedVfs {
            data_root: std::path::PathBuf::new(),
            vfs,
            rebind: None,
            active: Mutex::new(None),
            loaded: Mutex::new(None),
            running: Mutex::new(None),
        })
    }

    /// A world load starts while an object preload holds its lane; a
    /// second object load still waits for the first.
    #[test]
    fn a_world_load_does_not_wait_behind_a_long_object_preload() {
        use std::time::Duration;
        use timeline::setup::Stage;
        let (held, preloading) = mpsc::channel();
        let (finish, finished) = mpsc::channel::<()>();
        let preload = std::thread::spawn(move || {
            let guard = begin_load(Stage::Objects, || true).expect("the preload is current");
            held.send(()).unwrap();
            let _ = finished.recv();
            drop(guard);
        });
        preloading.recv().unwrap();
        let try_load = |stage| {
            let (send, receive) = mpsc::channel();
            std::thread::spawn(move || {
                let started = begin_load(stage, || true).is_some();
                let _ = send.send(started);
            });
            receive
        };
        let world = try_load(Stage::World);
        assert_eq!(world.recv_timeout(Duration::from_secs(10)), Ok(true), "the world waited behind the preload");
        let objects = try_load(Stage::Objects);
        assert!(objects.recv_timeout(Duration::from_millis(200)).is_err(), "two object builds ran at once");
        finish.send(()).unwrap();
        preload.join().unwrap();
        assert_eq!(objects.recv_timeout(Duration::from_secs(10)), Ok(true));
    }

    #[test]
    fn a_private_source_is_visible_to_native_jobs_only_after_activation() {
        let temp = Temp::new();
        let path = format!("{}\\maps\\level.map", vfs::CDROM_DEVICE);
        let edit = temp.0.join("level.map");
        std::fs::write(temp.0.join("maps/level.map"), named_map("level")).unwrap();
        std::fs::write(&edit, named_map("level")).unwrap();
        let mut vfs = Vfs::new();
        vfs.mount(vfs::CDROM_DEVICE, vfs::DirDiscBackend::open(&temp.0).unwrap()).unwrap();
        let binding = vfs::MapRebind::default();
        binding.attach(&mut vfs).unwrap();
        binding.prepare("level", "level", &edit, map::CacheType::Multiplayer, 0x0500_0000)
            .unwrap().reserve(&vfs).unwrap().commit();
        let generation = vfs.host_file(&path).unwrap();
        assert!(generation.generation.is_some());
        let vfs = Mutex::new(vfs);
        let mut old = ActiveSource { name: "level".into(), path, generation: None };
        assert!(open_resolved(&vfs, "level", None, Some(old.clone()), true).is_err());
        assert!(open_resolved(&vfs, "level", None, None, true).is_err());
        old.generation = Some(generation);
        let (activated, _) = open_resolved(&vfs, "level", None, Some(old), true).unwrap();
        assert_eq!(activated.header().name, "level");
    }

    /// A mod or Forge map lives in the data partition's content store, under
    /// a name no `<cdrom>\maps\<name>.map` guess finds. The recorded slot
    /// path opens it; an unserved path is named in the error.
    #[test]
    fn a_mod_map_opens_from_the_file_its_slot_was_copied_from() {
        let temp = Temp::new();
        let name = "m-0123abcd-0123456789abcdef0123";
        let store = temp.0.join("$c/00112233aabbccdd");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join(format!("{name}.map")), named_map(name)).unwrap();
        let mut vfs = Vfs::new();
        vfs.mount(vfs::CDROM_DEVICE, vfs::DirDiscBackend::open(&temp.0).unwrap()).unwrap();
        let data = temp.0.join("data");
        let tdata = data.join(format!("TDATA/{:08X}", crate::new_maps::HALO2_TITLE));
        std::fs::create_dir_all(tdata.join("$c/00112233aabbccdd")).unwrap();
        std::fs::copy(
            store.join(format!("{name}.map")),
            tdata.join(format!("$c/00112233aabbccdd/{name}.map")),
        ).unwrap();
        vfs.mount(vfs::DATA_DEVICE, vfs::DirDiscBackend::open(&data).unwrap()).unwrap();
        let vfs = Mutex::new(vfs);
        // `recorded_file` keeps a `t:` path verbatim for the guest's drive
        // links, which this VFS has none of; `slot_file` is the same file by
        // its object name on the data partition.
        let slot = crate::map_slots::slot_file(&format!("t:\\$c\\00112233aabbccdd\\{name}.map"))
            .unwrap();
        // The name alone is the guess that failed.
        assert!(open_resolved(&vfs, name, None, None, false).is_err());
        let (map, _) = open_resolved(&vfs, name, Some(&slot), None, false).unwrap();
        assert_eq!(map.header().name, name);
        let missing = slot.replace("00112233aabbccdd", "ffffffffffffffff");
        let Err(e) = open_resolved(&vfs, name, Some(&missing), None, false) else {
            panic!("an unserved slot path opened");
        };
        assert!(e.to_string().contains("ffffffffffffffff"), "{e}");
    }

    /// A Forge or mod map installed from a package (`new_maps`) is served
    /// at `T:\$c\<offer>\<map>.map` as an overlay over its base, under
    /// the map's own header name. The slot path the game records, `t:`
    /// kept verbatim, opens it through the guest's drive link.
    #[test]
    fn a_package_map_opens_from_its_t_slot_path() {
        let t = Temp::new();
        let name = "m-7f9e2cb1-b02c9cff29e0ece2ed41";
        let offer = "4d530064ff9e2cb1";
        let base = named_map("base");
        let base_path = t.0.join("base.map");
        std::fs::write(&base_path, &base).unwrap();
        let mut edited = base.clone();
        edited[0x7f0] ^= 0xff;
        let patch = content::patch::diff(&base, &edited, "base");
        let root = t.0.join("packages");
        let dir = root.join("forged");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ppf"), &patch.bytes).unwrap();
        let mut m = content::Manifest::retail("base", content::Sha256::of(&base));
        m.patches.push(content::manifest::Layer {
            file: "a.ppf".into(),
            sha256: content::Sha256::of(&patch.bytes),
        });
        std::fs::write(dir.join("manifest.toml"), m.to_toml()).unwrap();
        let package = content::Library::open(&root).packages()[0].clone();

        let cfg = vfs::Config::new(t.0.join("data"), crate::new_maps::HALO2_TITLE);
        let mut v = Vfs::with_storage(cfg.clone()).unwrap();
        v.create_standard_links(&cfg).unwrap();
        cfg.content_maps.replace([(
            format!("TDATA\\{:08X}\\$c\\{offer}\\{name}.map", crate::new_maps::HALO2_TITLE),
            vfs::ContentMap {
                package,
                base: Some(vfs::HostFile {
                    generation: None,
                    path: base_path,
                    offset: 0,
                    len: base.len() as u64,
                }),
                alias: Some(name.into()),
            },
        )]);
        let vfs = Mutex::new(v);
        let slot = format!("t:\\$c\\{offer}\\{name}.map");
        let (map, _) = open_resolved(&vfs, name, Some(&slot), None, false).unwrap();
        assert_eq!(map.header().name, name);
        let active = ActiveSource { name: name.into(), path: slot, generation: None };
        let (map, _) = open_resolved(&vfs, name, None, Some(active), true).unwrap();
        assert_eq!(map.header().name, name);
    }

    /// A guest file's bytes are the VFS's: a file of the disc's directory
    /// read from the host, and a rewritten one as its rewrite serves it.
    #[test]
    fn a_guest_file_is_read_as_the_vfs_serves_it() {
        struct Double;
        impl vfs::Rewrite for Double {
            fn wants(&self, path: &str) -> bool {
                path.ends_with("doubled")
            }
            fn rewrite(&mut self, _path: &str, original: &[u8]) -> Option<Vec<u8>> {
                Some(original.repeat(2))
            }
        }
        let t = Temp::new();
        let fonts = t.0.join("maps/fonts");
        std::fs::create_dir_all(&fonts).unwrap();
        std::fs::write(fonts.join("plain"), [1, 2, 3]).unwrap();
        std::fs::write(fonts.join("doubled"), [4, 5]).unwrap();
        let mut vfs = Vfs::new();
        let disc = vfs::DirDiscBackend::open(&t.0).unwrap();
        vfs.mount(vfs::CDROM_DEVICE, vfs::RewriteBackend::new(disc, Double)).unwrap();
        let mounted = Mutex::new(vfs);
        let path = |name: &str| format!("{}\\maps\\fonts\\{name}", vfs::CDROM_DEVICE);
        assert_eq!(read_from(&mounted, &path("plain")).unwrap(), [1, 2, 3], "as the directory holds it");
        assert_eq!(read_from(&mounted, &path("doubled")).unwrap(), [4, 5, 4, 5], "as the rewrite serves it");
        assert_eq!(read_from(&mounted, &path("missing")), None);
    }

    /// A live open's resource files read through the overlay: a
    /// replacement in the local map, one in a shared map, and an extent
    /// appended past the local map's end.
    #[test]
    fn resource_reads_serve_the_raw_overlay() {
        let t = Temp::new();
        let local = t.0.join("level.map");
        let shared = t.0.join("shared.map");
        std::fs::write(&local, [1u8; 32]).unwrap();
        std::fs::write(&shared, [2u8; 32]).unwrap();
        let mut files = ResourceFiles::new();
        let region = |path: &std::path::Path| Region { path: path.to_owned(), base: 0, length: 32 };
        files.set(ResourceFile::Local, region(&local)).unwrap();
        files.set(ResourceFile::Shared, region(&shared)).unwrap();
        let o = vfs::RawOverlay::new();
        overlay(&mut files, "Level", &o);
        o.put("level", 4, &[9, 9]);
        o.put("shared", 0, &[8]);
        let at = o.append("level", 32, &[7, 7], 16);
        assert_eq!(at, 32);
        assert_eq!(files.read(ResourceFile::Local, 3, 4).unwrap(), [1, 9, 9, 1]);
        assert_eq!(files.read(ResourceFile::Shared, 0, 2).unwrap(), [8, 2]);
        assert_eq!(files.length(ResourceFile::Local), Some(34));
        assert_eq!(files.read(ResourceFile::Local, 31, 3).unwrap(), [1, 7, 7]);
        o.clear();
        assert_eq!(files.read(ResourceFile::Local, 3, 4).unwrap(), [1, 1, 1, 1]);
        assert!(files.read(ResourceFile::Local, 32, 2).is_err());
    }

    #[test]
    fn direct_drive_sources_follow_the_guest_links_and_missing_sources_use_the_catalog() {
        let t = Temp::new();
        std::fs::write(t.0.join("maps/level.map"), named_map("disc")).unwrap();
        let mut vfs = Vfs::new();
        for drive in ["T", "W"] {
            let dir = t.0.join(drive);
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("local.map"), named_map(drive)).unwrap();
            vfs.namespace.create_symbolic_link(
                &format!(r"\??\{drive}:"),
                &format!("{}\\{drive}", vfs::CDROM_DEVICE),
            ).unwrap();
        }
        vfs.mount(vfs::CDROM_DEVICE, vfs::DirDiscBackend::open(&t.0).unwrap()).unwrap();
        let vfs = Mutex::new(vfs);
        for (path, expected) in [
            (r"t:\local.map", "T"),
            (r"W:\local.map", "W"),
            (r"W:\missing.map", "disc"),
        ] {
            let active = ActiveSource {
                name: "level".into(),
                path: path.into(),
                generation: None,
            };
            let (map, _) = open_resolved(&vfs, "level", None, Some(active), true).unwrap();
            assert_eq!(map.header().name, expected);
        }
    }
}
