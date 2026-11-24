//! A map served from a host file or reconstructed from the tag store.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use crate::{Error, HostFile, Result, store_map::StoreFile};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MapSource {
    File(PathBuf),
    Stored {
        root: PathBuf,
        stem: String,
        hash: Option<content::Sha256>,
    },
}

impl From<PathBuf> for MapSource {
    fn from(path: PathBuf) -> Self {
        Self::File(path)
    }
}

impl MapSource {
    pub(crate) fn open(&self) -> Result<OpenMap> {
        match self {
            Self::File(path) => Ok(OpenMap::File(File::open(path)?)),
            Self::Stored { root, stem, hash } => {
                let Some(hash) = hash else {
                    eprintln!("store maps: {stem}: the converted level is missing");
                    return Err(Error::ObjectNameNotFound);
                };
                let file = StoreFile::open(root, stem).map_err(|e| {
                    eprintln!("store maps: {stem}: {e}");
                    Error::FileCorrupt
                })?;
                if hash.to_string() != file.file_hash() {
                    eprintln!("store maps: {stem}: the stored map does not match the converted level");
                    return Err(Error::FileCorrupt);
                }
                Ok(OpenMap::Stored(Box::new(file)))
            }
        }
    }

    pub fn len_on_disk(&self) -> Option<u64> {
        match self {
            Self::File(path) => path.metadata().ok().filter(|m| m.is_file()).map(|m| m.len()),
            Self::Stored { root, stem, hash } => {
                let extents = StoreFile::extents(root, stem).ok()?;
                (hash.as_ref()?.to_string() == extents.file).then_some(extents.len)
            }
        }
    }

    pub(crate) fn host_file(&self, len: u64) -> Option<HostFile> {
        match self {
            Self::File(path) => Some(HostFile { generation: None, path: path.clone(), offset: 0, len }),
            Self::Stored { .. } => None,
        }
    }
}

pub(crate) enum OpenMap {
    File(File),
    Stored(Box<StoreFile>),
}

impl OpenMap {
    pub(crate) fn len(&self) -> Result<u64> {
        match self {
            Self::File(file) => {
                let meta = file.metadata()?;
                if !meta.is_file() {
                    return Err(Error::ObjectNameNotFound);
                }
                Ok(meta.len())
            }
            Self::Stored(file) => Ok(file.len()),
        }
    }

    pub(crate) fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        match self {
            Self::File(file) => {
                let len = file.metadata()?.len();
                let n = len.saturating_sub(offset).min(buffer.len() as u64) as usize;
                if n == 0 {
                    return Ok(0);
                }
                file.seek(SeekFrom::Start(offset))?;
                match file.read_exact(&mut buffer[..n]) {
                    Ok(()) => Ok(n),
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(Error::FileCorrupt),
                    Err(e) => Err(e.into()),
                }
            }
            Self::Stored(file) => file.read_at(offset, buffer).map_err(|e| {
                eprintln!("store maps: {}: {e}", file.map());
                Error::FileCorrupt
            }),
        }
    }

    pub(crate) fn film_source(&self) -> Result<Option<crate::FilmMapSource>> {
        match self {
            Self::File(file) => Ok(Some(crate::FilmMapSource::region(file.try_clone()?, 0))),
            Self::Stored(_) => Ok(None),
        }
    }
}
