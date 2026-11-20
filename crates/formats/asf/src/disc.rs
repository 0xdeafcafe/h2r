//! Opt-in private-disc corpus access shared by validation tools. No assets are
//! embedded or written. Stable indices use mainmenu then shared gestalt order,
//! deduplicated by physical chunk extents (including unreferenced permutations).
use map::{Map, RawPointer, ResourceFile};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
use tags::sound::{Gestalt, ResourceReader};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[derive(Debug)]
pub struct Stream {
    pub resource: usize,
    pub permutation: usize,
    spans: Vec<(u64, u32)>,
}
pub struct Disc {
    path: PathBuf,
    pub streams: Vec<Stream>,
    xbe: (u64, u32),
}
struct Reader {
    file: File,
    spans: [(u64, u64); 4],
}
fn slot(r: ResourceFile) -> usize {
    match r {
        ResourceFile::Local => 0,
        ResourceFile::MainMenu => 1,
        ResourceFile::Shared => 2,
        ResourceFile::SinglePlayerShared => 3,
    }
}
impl ResourceReader for Reader {
    fn check(&self, p: RawPointer, n: u32) -> tags::Result<()> {
        if u64::from(p.offset) + u64::from(n) > self.spans[slot(p.file)].1 {
            return Err(tags::Error::Invalid("resource extent"));
        }
        Ok(())
    }
    fn read_raw(&mut self, p: RawPointer, n: u32) -> tags::Result<Vec<u8>> {
        self.check(p, n)?;
        let io = |_| tags::Error::Invalid("resource read");
        self.file
            .seek(SeekFrom::Start(
                self.spans[slot(p.file)].0 + u64::from(p.offset),
            ))
            .map_err(io)?;
        let mut out = vec![0; n as usize];
        self.file.read_exact(&mut out).map_err(io)?;
        Ok(out)
    }
}
impl Disc {
    pub fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err("disc path must be absolute".into());
        }
        let mut iso = xiso::Xiso::open(BufReader::new(File::open(path)?))?;
        let entries = iso.entries()?;
        let entry = |name: &str| -> Result<(u64, u32)> {
            let e = entries
                .iter()
                .find(|e| e.path.trim_start_matches('/').eq_ignore_ascii_case(name))
                .ok_or("missing disc file")?;
            Ok((
                iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR,
                e.size,
            ))
        };
        let xbe = entry("default.xbe")?;
        let mut spans = [(0, 0); 4];
        for (i, name) in [
            (1, "maps/mainmenu.map"),
            (2, "maps/shared.map"),
            (3, "maps/single_player_shared.map"),
        ] {
            let (a, n) = entry(name)?;
            spans[i] = (a, n.into());
        }
        let mut streams = Vec::new();
        let mut seen = BTreeSet::new();
        for resource in [1, 2] {
            spans[0] = spans[resource];
            let mut map =
                Map::from_region(BufReader::new(File::open(path)?), spans[0].0, spans[0].1)?;
            let g = Gestalt::read(&mut map)?;
            let mut reader = Reader {
                file: File::open(path)?,
                spans,
            };
            for (permutation, p) in g.permutations.iter().enumerate() {
                let first = usize::from(p.first_chunk);
                let end = first + usize::from(p.chunk_count);
                let chunks = g.chunks.get(first..end).ok_or("chunk bounds")?;
                let Some(c) = chunks.first() else { continue };
                if c.size < 16 || reader.read_raw(c.pointer, 16)? != crate::HEADER {
                    continue;
                }
                let mut key = Vec::new();
                let mut size = 0u64;
                for c in chunks {
                    reader.check(c.pointer, c.size)?;
                    size += u64::from(c.size);
                    key.push((
                        spans[slot(c.pointer.file)].0 + u64::from(c.pointer.offset),
                        c.size,
                    ));
                }
                if size > 256 * 1024 * 1024 {
                    return Err("permutation exceeds 256 MiB".into());
                }
                if seen.insert(key.clone()) {
                    streams.push(Stream {
                        resource,
                        permutation,
                        spans: key,
                    });
                }
            }
        }
        Ok(Self {
            path: path.to_owned(),
            streams,
            xbe,
        })
    }
    pub fn read_stream(&self, index: usize) -> Result<Vec<u8>> {
        self.read_spans(
            &self
                .streams
                .get(index)
                .ok_or("stream index out of bounds")?
                .spans,
        )
    }
    pub fn read_xbe(&self) -> Result<Vec<u8>> {
        if self.xbe.1 > 16 * 1024 * 1024 {
            return Err("XBE exceeds 16 MiB".into());
        }
        self.read_spans(&[self.xbe])
    }
    fn read_spans(&self, spans: &[(u64, u32)]) -> Result<Vec<u8>> {
        let mut file = File::open(&self.path)?;
        let mut out = Vec::new();
        for &(offset, size) in spans {
            let start = out.len();
            out.resize(start + size as usize, 0);
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut out[start..])?;
        }
        Ok(out)
    }
}
