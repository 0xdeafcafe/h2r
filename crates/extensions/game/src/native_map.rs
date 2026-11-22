//! The map and raw resource files behind the native render loaders.
//!
//! A live open serves every selector through the raw overlay
//! ([`vfs::RawOverlay::global`], docs/editor/realtime-edits.md, "The edit
//! overlay"), so a world, model or image loaded after a live raw edit
//! decodes the edited bytes.

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::sync::{Arc, Mutex, Weak, mpsc};

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

pub(crate) fn retire_queue() -> std::io::Result<RetireQueue> {
    static QUEUE: Mutex<Option<RetireQueue>> = Mutex::new(None);
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(send) = queue.as_ref() {
        return Ok(send.clone());
    }
    let (send, receive) = mpsc::channel::<(timeline::setup::Stage, Box<dyn Send>)>();
    background::spawn("native-map retire", move || {
        for (stage, payload) in receive {
            let _timer = stage.worker();
            drop(payload);
        }
    })?;
    *queue = Some(send.clone());
    Ok(send)
}

/// Call from Drop after detaching the payload's sender so the worker
/// destroys the payload directly.
pub(crate) fn retire<T: Default + Send + 'static>(send: RetireQueue, stage: timeline::setup::Stage, payload: &mut T) {
    let _ = send.send((stage, Box::new(std::mem::take(payload))));
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
    vfs: Arc<Mutex<Vfs>>,
    rebind: Option<vfs::MapRebind>,
    active: Mutex<Option<ActiveSource>>,
}

#[derive(Clone)]
struct ActiveSource {
    name: String,
    path: String,
    generation: Option<vfs::HostFile>,
}

static SOURCES: Mutex<Weak<MountedVfs>> = Mutex::new(Weak::new());

#[must_use]
pub fn mount(vfs: Arc<Mutex<Vfs>>, rebind: Option<vfs::MapRebind>) -> Arc<MountedVfs> {
    let sources = Arc::new(MountedVfs {
        vfs,
        rebind,
        active: Mutex::new(None),
    });
    *SOURCES.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&sources);
    sources
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

pub fn open(name: &str) -> Result<(NativeMap, ResourceFiles), Error> {
    let sources = SOURCES.lock().unwrap_or_else(|e| e.into_inner())
        .upgrade()
        .ok_or("the map's VFS is not mounted")?;
    let active = sources.rebind.as_ref().filter(|r| r.has_ever_bound()).and_then(|_| {
        sources.active.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            .filter(|a| a.name.eq_ignore_ascii_case(name))
            .cloned()
    });
    open_resolved(&sources.vfs, name, None, active, true)
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
            // An explicit replay source must match the recorded slot.
            mounted.file_source(path)
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
    let map = match &local {
        FileSource::Host(file) => {
            let reader: Box<dyn MapReader> = Box::new(BufReader::new(File::open(&file.path)?));
            Map::from_region(reader, file.offset, file.len)?
        }
        FileSource::Memory(bytes) => {
            let reader: Box<dyn MapReader> = Box::new(Cursor::new(Arc::clone(bytes)));
            Map::from_reader(reader)?
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_override::tests::{Temp, named_map};

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
