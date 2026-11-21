//! The map and raw resource files behind the native render loaders.

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::sync::{Arc, Mutex, mpsc};

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

static VFS: Mutex<Option<Arc<Mutex<Vfs>>>> = Mutex::new(None);

pub fn set_vfs(vfs: Option<Arc<Mutex<Vfs>>>) {
    *VFS.lock().unwrap_or_else(|e| e.into_inner()) = vfs;
}

pub fn open(name: &str) -> Result<(NativeMap, ResourceFiles), Error> {
    let vfs = VFS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or("the map's VFS is not mounted")?;
    open_from_vfs(&vfs, name)
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
    let (local, candidates, shared) = {
        let mut mounted = vfs.lock().unwrap_or_else(|e| e.into_inner());
        let mut candidates = Vec::new();
        let local = if let Some(path) = slot_source {
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
    Ok((map, files))
}

fn register(files: &mut ResourceFiles, file: ResourceFile, source: FileSource) -> assets::Result<()> {
    match source {
        FileSource::Host(f) => files.set(
            file,
            Region {
                path: f.path,
                base: f.offset,
                length: f.len,
            },
        ),
        FileSource::Memory(bytes) => {
            files.set_bytes(file, bytes);
            Ok(())
        }
    }
}
