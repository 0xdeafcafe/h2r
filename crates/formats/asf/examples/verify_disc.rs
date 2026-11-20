//! Opt-in private-disc validation; writes no assets. Use an absolute ISO path.
use map::{Map, RawPointer, ResourceFile};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::PathBuf,
};
use tags::sound::{Gestalt, ResourceReader};

struct Resources {
    file: File,
    spans: [(u64, u64); 4],
}
impl ResourceReader for Resources {
    fn check(&self, p: RawPointer, n: u32) -> tags::Result<()> {
        let (_, len) = self.spans[slot(p.file)];
        if u64::from(p.offset) + u64::from(n) > len {
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
fn slot(r: ResourceFile) -> usize {
    match r {
        ResourceFile::Local => 0,
        ResourceFile::MainMenu => 1,
        ResourceFile::Shared => 2,
        ResourceFile::SinglePlayerShared => 3,
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let samples = std::env::args_os().nth(2).map(PathBuf::from);
    if let Some(dir) = &samples
        && (!dir.is_absolute() || !dir.is_dir())
    {
        return Err("sample directory must already exist and be absolute".into());
    }
    let path = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("provide absolute disc path")?,
    );
    if !path.is_absolute() {
        return Err("disc path must be absolute".into());
    }
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path)?))?;
    let entries = iso.entries()?;
    let mut spans = [(0, 0); 4];
    for (i, name) in [
        (1, "/maps/mainmenu.map"),
        (2, "/maps/shared.map"),
        (3, "/maps/single_player_shared.map"),
    ] {
        let e = entries
            .iter()
            .find(|e| e.path == name)
            .ok_or("missing resource file")?;
        spans[i] = (
            iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR,
            e.size.into(),
        );
    }
    let (mut count, mut failures, mut packets, mut objects, mut bytes) = (0, 0, 0, 0, 0u64);
    let mut formats = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for index in [1, 2] {
        spans[0] = spans[index];
        let mut map = Map::from_region(BufReader::new(File::open(&path)?), spans[0].0, spans[0].1)?;
        let g = Gestalt::read(&mut map)?;
        let mut resources = Resources {
            file: File::open(&path)?,
            spans,
        };
        let mut map_count = 0;
        // Enumerate gestalt permutations, including entries not referenced by snd!.
        for (pi, p) in g.permutations.iter().enumerate() {
            let first = usize::from(p.first_chunk);
            let end = first + usize::from(p.chunk_count);
            let chunks = g.chunks.get(first..end).ok_or("chunk bounds")?;
            let Some(c) = chunks.first() else { continue };
            if c.size < 16 || resources.read_raw(c.pointer, 16)? != asf::HEADER {
                continue;
            }
            let key: Vec<_> = chunks
                .iter()
                .map(|c| {
                    (
                        resources.spans[slot(c.pointer.file)].0 + u64::from(c.pointer.offset),
                        c.size,
                    )
                })
                .collect();
            if !seen.insert(key) {
                continue;
            }
            count += 1;
            map_count += 1;
            let encoded = g.read_encoded(p, &mut resources)?;
            if map_count <= 3
                && let Some(dir) = &samples
            {
                use std::io::Write;
                let mut out = File::options()
                    .write(true)
                    .create_new(true)
                    .open(dir.join(format!("resource-{index}-sample-{map_count}.wma")))?;
                out.write_all(&encoded)?;
            }
            let result = (|| -> asf::Result<()> {
                let asf = asf::Asf::parse(&encoded)?;
                if asf.streams.len() != 1 {
                    return Err(asf::Error::Unsupported("stream count"));
                }
                let stream = &asf.streams[0];
                stream.check_audio_spread()?;
                let wave = stream
                    .audio
                    .as_ref()
                    .ok_or(asf::Error::Invalid("no audio"))?;
                *formats
                    .entry((
                        wave.format_tag,
                        wave.channels,
                        wave.sample_rate,
                        wave.bytes_per_second,
                        wave.block_align,
                    ))
                    .or_insert(0) += 1;
                let mut reassembly = asf::Reassembler::new(stream.number, 1 << 20)?;
                for packet in asf.packets() {
                    let packet = packet?;
                    packets += 1;
                    for payload in packet.payloads {
                        if payload.stream != stream.number {
                            return Err(asf::Error::Invalid("unknown stream"));
                        }
                        if let Some(object) = reassembly.push(&payload)? {
                            if object.data.len() % usize::from(wave.block_align) != 0 {
                                return Err(asf::Error::Invalid("codec block alignment"));
                            }
                            objects += 1;
                            bytes += object.data.len() as u64;
                        }
                    }
                }
                reassembly.finish()
            })();
            if let Err(e) = result {
                failures += 1;
                eprintln!("resource {index} permutation {pi}: {e}");
            }
        }
        println!("resource {index}: {map_count} unique ASF permutations");
    }
    println!(
        "streams={count} passed={} failed={failures} packets={packets} media_objects={objects} codec_bytes={bytes}",
        count - failures
    );
    for (format, n) in formats {
        println!("format={format:?} streams={n}");
    }
    if failures > 0 || count != 485 {
        return Err("validation incomplete".into());
    }
    Ok(())
}
