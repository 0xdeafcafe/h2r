//! Offset-preserving, sparse server copies. The guest and network layer can
//! keep their original header, resource descriptors and copy-time checksum.
use anyhow::{Context, Result, bail, ensure};
use map::{FourCC, Map, ResourceFile};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{BufReader, Read, Seek, SeekFrom, Write},
    ops::Range,
    path::Path,
};
use tags::{DecodeMode, DecodedTag, TagData, sound::{Compression, Gestalt}};

const HEADER: u64 = 0x800;
const CHUNK: usize = 64 * 1024;
const USAGE: &str = "usage: h2 server-map strip ORIGINAL.map OUTPUT.map
       h2 server-map verify ORIGINAL.map SERVER.map";

pub fn run(args: Vec<String>) -> Result<()> {
    let [mode, original, output] = args.as_slice() else {
        bail!("{USAGE}");
    };
    let original = Path::new(original);
    let output = Path::new(output);
    let mut source = Map::open(original)?;
    source.validate_checksum().context("validating the original cache")?;
    let plan = Plan::read(&mut source)?;
    match mode.as_str() {
        "strip" => {
            let stage = output.with_extension(format!("{}.stage", live_proto::sign::random_id()));
            let mut file = OpenOptions::new().write(true).create_new(true).open(&stage)?;
            let result = (|| {
                strip(File::open(original)?, &mut file, &plan)?;
                file.sync_all()?;
                drop(file);
                verify(original, &stage, &plan)?;
                std::fs::hard_link(&stage, output)
                    .with_context(|| format!("publishing {}", output.display()))?;
                Ok::<_, anyhow::Error>(())
            })();
            let _ = std::fs::remove_file(&stage);
            result?;
        }
        "verify" => verify(original, output, &plan)?,
        _ => bail!("{USAGE}"),
    }
    Ok(())
}

struct Plan {
    size: u64,
    /// Disjoint ranges of sample/pixel bytes, in file order.
    holes: Vec<Range<u64>>,
}

impl Plan {
    fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Self> {
        let h = map.header();
        let limit = u64::from(h.file_size);
        let mut keep = vec![0..HEADER, u64::from(h.index_offset)..limit];
        for t in map.tags() {
            if let Some(loc) = t.location {
                keep.push(loc.space.file_offset..loc.space.file_offset + u64::from(loc.space.size));
            }
        }
        let table = |at: u32, size: u64| u64::from(at)..u64::from(at) + size;
        for t in [&h.strings, &h.tag_names] {
            keep.push(table(t.index_offset, 4 * u64::from(t.count)));
            keep.push(table(t.data_offset, u64::from(t.data_size)));
        }
        keep.push(table(h.fixed_strings_offset, 128 * u64::from(h.strings.count)));
        let tags: Vec<_> = map.tags().iter().filter(|t| t.location.is_some())
            .map(|t| (t.id, t.group)).collect();
        let mut holes = Vec::new();
        let local = |file, offset: u32, size: u32| -> Result<Option<Range<u64>>> {
            if file != ResourceFile::Local || size == 0 {
                return Ok(None);
            }
            let r = table(offset, u64::from(size));
            ensure!(r.start >= HEADER && r.end <= limit,
                "raw payload range {r:?} is outside the map body");
            Ok(Some(r))
        };
        for &(id, group) in &tags {
            if group != FourCC(*b"bitm") {
                continue;
            }
            let DecodedTag { data: TagData::Bitmap(bitmap), .. } =
                DecodedTag::read_with_mode(map, id, DecodeMode::Lenient)?
            else {
                bail!("bitmap tag {id} has a different class");
            };
            for image in bitmap.images {
                for raw in image.lods.into_iter().chain([image.low_detail]).flatten() {
                    holes.extend(local(raw.pointer.file, raw.pointer.offset, raw.size)?);
                }
            }
        }

        if tags.iter().any(|&(_, group)| group == FourCC(*b"ugh!")) {
            let gestalt = Gestalt::read(map)?;
            // All aliases must be safe. Unreferenced chunks, WMA, and
            // unknown formats may require sample inspection by the codec.
            let mut seen = vec![false; gestalt.chunks.len()];
            let mut unsafe_format = seen.clone();
            for &(id, group) in &tags {
                if group != FourCC(*b"snd!") {
                    continue;
                }
                let h = tags::sound::read_header(map, id)?;
                let safe = matches!(h.compression, Compression::XboxAdpcm
                    | Compression::NoneBigEndian | Compression::NoneLittleEndian);
                for range in gestalt.resolve(&h)?.ranges {
                    for p in range.permutations {
                        let first = p.first_chunk as usize;
                        for i in first..first + p.chunk_count as usize {
                            seen[i] = true;
                            unsafe_format[i] |= !safe;
                        }
                    }
                }
            }
            for (i, c) in gestalt.chunks.into_iter().enumerate() {
                if let Some(r) = local(c.pointer.file, c.pointer.offset, c.size)? {
                    if seen[i] && !unsafe_format[i] {
                        holes.push(r);
                    } else {
                        keep.push(r);
                    }
                }
            }
        }

        Ok(Self {
            size: map.storage_length(),
            holes: subtract(union(holes), &union(keep)),
        })
    }

    /// Reserve a word in a discarded extent for the XOR correction. The
    /// entire qword must be in the engine's checksum span.
    fn parity_at(&self) -> Option<u64> {
        self.holes.iter().find_map(|r| {
            let at = r.start.next_multiple_of(8);
            (at + 8 <= r.end).then_some(at)
        })
    }
}

fn union(mut ranges: Vec<Range<u64>>) -> Vec<Range<u64>> {
    ranges.sort_unstable_by_key(|r| r.start);
    let mut out: Vec<Range<u64>> = Vec::new();
    for r in ranges.into_iter().filter(|r| !r.is_empty()) {
        if let Some(last) = out.last_mut().filter(|last| last.end >= r.start) {
            last.end = last.end.max(r.end);
        } else {
            out.push(r);
        }
    }
    out
}

fn subtract(holes: Vec<Range<u64>>, keep: &[Range<u64>]) -> Vec<Range<u64>> {
    let mut out = Vec::new();
    for hole in holes {
        let mut at = hole.start;
        let first = keep.partition_point(|r| r.end <= at);
        for r in &keep[first..] {
            if r.start >= hole.end {
                break;
            }
            if r.start > at {
                out.push(at..r.start);
            }
            at = at.max(r.end);
        }
        if at < hole.end {
            out.push(at..hole.end);
        }
    }
    out
}

fn parts(plan: &Plan) -> impl Iterator<Item = (bool, Range<u64>)> + '_ {
    let mut at = 0;
    plan.holes.iter().flat_map(move |r| {
        let kept = at..r.start;
        at = r.end;
        [(true, kept), (false, r.clone())]
    }).chain(std::iter::once((true, plan.holes.last().map_or(0, |r| r.end)..plan.size)))
}

/// The two XOR streams have the same byte length. The fixed seed in
/// map::XorChecksum cancels, leaving the XOR of bytes removed at each
/// position modulo four. A partial last qword does not participate.
fn xor_removed(xor: &mut [u8; 4], at: u64, bytes: &[u8], size: u64) {
    for (i, &b) in bytes.iter().enumerate() {
        let offset = at + i as u64;
        if offset < size / 8 * 8 {
            xor[(offset & 3) as usize] ^= b;
        }
    }
}

fn strip(input: File, out: &mut File, plan: &Plan) -> Result<()> {
    let mut input = BufReader::new(input);
    let mut buffer = vec![0; CHUNK];
    let mut xor = [0; 4];
    out.set_len(plan.size)?;
    for (keep, span) in parts(plan) {
        if keep {
            out.seek(SeekFrom::Start(span.start))?;
        }
        let mut at = span.start;
        while at < span.end {
            let n = buffer.len().min((span.end - at) as usize);
            let bytes = &mut buffer[..n];
            input.read_exact(bytes)?;
            if keep {
                out.write_all(bytes)?;
            } else {
                xor_removed(&mut xor, at, bytes, plan.size);
            }
            at += n as u64;
        }
    }
    if xor != [0; 4] {
        let at = plan.parity_at().context("no aligned raw extent for checksum correction")?;
        out.seek(SeekFrom::Start(at))?;
        out.write_all(&xor)?;
    }
    Ok(())
}

fn verify(original: &Path, stripped: &Path, plan: &Plan) -> Result<()> {
    let mut a = BufReader::new(File::open(original)?);
    let mut b = BufReader::new(File::open(stripped)?);
    ensure!(b.get_ref().metadata()?.len() == plan.size, "map storage size changed");
    let (mut a_buf, mut b_buf) = (vec![0; CHUNK], vec![0; CHUNK]);
    let (mut a_hash, mut b_hash) = (Sha256::new(), Sha256::new());
    let parity = plan.parity_at();
    let mut found = [0; 4];
    let mut want = [0; 4];
    for (keep, span) in parts(plan) {
        let mut at = span.start;
        while at < span.end {
            let n = CHUNK.min((span.end - at) as usize);
            let (a_bytes, b_bytes) = (&mut a_buf[..n], &mut b_buf[..n]);
            a.read_exact(a_bytes)?;
            b.read_exact(b_bytes)?;
            if keep {
                ensure!(a_bytes == b_bytes, "preserved bytes changed in the range at {at:#x}");
                a_hash.update(a_bytes);
                b_hash.update(b_bytes);
            } else {
                xor_removed(&mut want, at, a_bytes, plan.size);
                for (i, &byte) in b_bytes.iter().enumerate() {
                    let offset = at + i as u64;
                    if let Some(j) = parity.and_then(|p| offset.checked_sub(p)).filter(|j| *j < 4) {
                        found[j as usize] = byte;
                    } else {
                        ensure!(byte == 0, "unstripped payload byte at {offset:#x}");
                    }
                }
            }
            at += n as u64;
        }
    }
    ensure!(found == want, "checksum correction differs");
    let digest = a_hash.finalize();
    ensure!(digest == b_hash.finalize(), "preserved bytes hash changed");
    let removed: u64 = plan.holes.iter().map(|r| r.end - r.start).sum();
    println!("preserved_sha256 {digest:x}");
    println!("logical_bytes {}  blanked_payload_bytes {removed}", plan.size);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        println!("allocated_bytes {}", b.get_ref().metadata()?.blocks() * 512);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_spans_win_over_overlapping_raw_references() {
        let holes = union(vec![18..36, 7..20, 12..16]);
        let keep = union(vec![0..8, 9..14, 14..16, 22..28, 30..42]);
        assert_eq!(subtract(holes, &keep), vec![8..9, 16..22, 28..30]);
    }

    #[test]
    fn sparse_correction_preserves_retail_xor_even_for_unaligned_holes() {
        let body: Vec<u8> = (0..93).collect();
        let mut changed = body.clone();
        let mut xor = [0; 4];
        for r in [3..7, 9..30, 89..93] {
            xor_removed(&mut xor, r.start as u64, &body[r.clone()], body.len() as u64);
            changed[r].fill(0);
        }
        changed[16..20].copy_from_slice(&xor);
        assert_eq!(map::checksum::engine_xor(&changed), map::checksum::engine_xor(&body));
    }
}
