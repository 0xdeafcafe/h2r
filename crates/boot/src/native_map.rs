//! The map and raw resource files behind the native render loaders.

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::sync::{Arc, Mutex};

use assets::{Region, ResourceFiles};
use map::{Map, ResourceFile};
use native_render::load::Error;
use vfs::{FileSource, Vfs};

use crate::pad_speaker::SHARED_MAPS;

pub(crate) trait MapReader: Read + Seek + Send {}
impl<R: Read + Seek + Send> MapReader for R {}

pub(crate) type NativeMap = Map<Box<dyn MapReader>>;

static VFS: Mutex<Option<Arc<Mutex<Vfs>>>> = Mutex::new(None);

pub(crate) fn set_vfs(vfs: Option<Arc<Mutex<Vfs>>>) {
    *VFS.lock().unwrap_or_else(|e| e.into_inner()) = vfs;
}

pub(crate) fn open(name: &str) -> Result<(NativeMap, ResourceFiles), Error> {
    let vfs = VFS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or("the map's VFS is not mounted")?;
    open_from_vfs(&vfs, name)
}

pub(crate) fn open_from_vfs(
    vfs: &Mutex<Vfs>,
    name: &str,
) -> Result<(NativeMap, ResourceFiles), Error> {
    let (local, shared) = {
        let mut vfs = vfs.lock().unwrap_or_else(|e| e.into_inner());
        let local = if let Some(path) = vfs.content_map_path(name) {
            vfs.file_source(&path)
        } else {
            let path = format!("{}\\maps\\{name}.map", vfs::CDROM_DEVICE);
            vfs.file_source(&path)
                .or_else(|| crate::map_override::map_file(&mut vfs, name).map(FileSource::Host))
        }
        .ok_or("the map's file is not found")?;
        // Raw pointers select fixed DVD paths, independent of the loaded map's
        // name and any installed content with the same name.
        let shared = SHARED_MAPS.map(|(file, path)| (file, vfs.file_source(path)));
        (local, shared)
    };

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
