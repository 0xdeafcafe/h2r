//! Transfer a sealed film into the library. Only fully verified files get
//! the library extension. An interruption leaves the last complete range.
use anyhow::{Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    ops::Range,
    path::Path,
};

fn private_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn checksum(file: &mut File) -> Result<String> {
    file.rewind()?;
    let mut digest = Sha256::new();
    std::io::copy(file, &mut digest)?;
    Ok(format!("{:x}", digest.finalize()))
}

/// Content identity for an account that already downloaded this recording.
pub fn revision(path: &Path) -> Result<(u64, String)> {
    let mut file = File::open(path)?;
    Ok((file.metadata()?.len(), checksum(&mut file)?))
}

pub fn download(
    path: &Path,
    size: u64,
    sha256: &str,
    mut range: impl FnMut(Range<u64>) -> Result<Vec<u8>>,
) -> Result<()> {
    ensure!(
        (1..=live_proto::film::MAX_UPLOAD_BYTES).contains(&size),
        "invalid film size"
    );
    ensure!(
        sha256.len() == 64
            && sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
        "invalid film digest"
    );
    let lock = private_file(&path.with_extension("lock"))?;
    lock.lock()?;
    if let Ok(mut existing) = File::open(path)
        && existing.metadata()?.len() == size
        && checksum(&mut existing)? == sha256
    {
        return Ok(());
    }
    let part = path.with_extension("part");
    let revision = path.with_extension("revision");
    let mut file = private_file(&part)?;
    if fs::read_to_string(&revision).ok().as_deref() != Some(sha256)
        || file.metadata()?.len() > size
    {
        file.set_len(0)?;
        fs::write(&revision, sha256)?;
    }
    let mut start = file.seek(SeekFrom::End(0))?;
    while start < size {
        let end = (start + (1 << 20)).min(size);
        let bytes = range(start..end)?;
        ensure!(bytes.len() as u64 == end - start, "short film range");
        file.write_all(&bytes)?;
        start = end;
    }
    if checksum(&mut file)? != sha256 {
        file.set_len(0)?;
        bail!("downloaded film digest differs from Live");
    }
    file.sync_all()?;
    drop(file);
    fs::rename(part, path)?;
    let _ = fs::remove_file(revision);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_download_resumes_verifies_and_reuses_the_library_file() {
        let dir = std::env::temp_dir().join(live_proto::sign::random_id());
        fs::create_dir(&dir).unwrap();
        let path = dir.join("game.h2film");
        let bytes = vec![23; (1 << 20) + 3];
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let size = bytes.len() as u64;
        assert!(
            download(&path, size, &digest, |r| {
                if r.start != 0 {
                    bail!("interrupted");
                }
                Ok(bytes[..r.end as usize].into())
            })
            .is_err()
        );
        assert!(!path.exists());
        download(&path, size, &digest, |r| {
            assert_eq!(r, (1 << 20)..size);
            Ok(vec![23; 3])
        })
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        download(&path, size, &digest, |_| panic!("fetched twice")).unwrap();
        assert!(download(&path, 1, &digest, |_| Ok(vec![0])).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_dir_all(dir).unwrap();
    }
}

/// Build the numbered-part declaration without retaining the file in memory.
pub fn object(path: &Path) -> Result<live_proto::film::sharing::Object> {
    use live_proto::film::{
        PART_BYTES,
        sharing::{MAX_FILM, Object},
    };
    use std::io::Read;
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    ensure!(
        (1..=MAX_FILM).contains(&size),
        "object exceeds the 256 MiB file-share limit or is empty"
    );
    let mut parts = Vec::new();
    let mut full = Sha256::new();
    let mut buf = vec![0; (1 << 16) as usize];
    for i in 0..size.div_ceil(PART_BYTES) {
        let mut left = (size - i * PART_BYTES).min(PART_BYTES);
        let mut part = Sha256::new();
        while left != 0 {
            let len = left.min(buf.len() as u64) as usize;
            f.read_exact(&mut buf[..len])?;
            full.update(&buf[..len]);
            part.update(&buf[..len]);
            left -= len as u64;
        }
        parts.push(format!("{:x}", part.finalize()));
    }
    Ok(Object {
        size,
        sha256: format!("{:x}", full.finalize()),
        parts,
    })
}

pub(super) fn read_seed(
    f: &mut File,
) -> Result<(
    [u8; live_proto::film::sharing::HEADER],
    live_proto::film::sharing::Layout,
    serde_json::Value,
)> {
    use live_proto::film::sharing::{HEADER, Layout, MAX_SEED, clean_seed};
    use std::io::Read;
    f.rewind()?;
    let mut header = [0; HEADER];
    f.read_exact(&mut header)?;
    let layout = Layout::parse(&header, f.metadata()?.len()).map_err(anyhow::Error::msg)?;
    ensure!(
        header[48..64] == [0; 16],
        "film requires an unknown compression dictionary"
    );
    let mut seed = vec![0; layout.seed_end as usize - HEADER];
    f.read_exact(&mut seed)?;
    let raw = zstd::bulk::decompress(&seed, MAX_SEED)?;
    let clean = clean_seed(&raw).map_err(anyhow::Error::msg)?;
    Ok((header, layout, clean))
}

/// Produce the post-game upload copy. Tick bytes (including recorded
/// digests) survive recompression verbatim. No sidecar or machine snapshot
/// is consulted. A legacy machine film needs the split recorder first.
pub fn prepare(input: &Path, out: &mut File) -> Result<()> {
    use live_proto::film::sharing::MAX_FILM;
    use std::io::{Cursor, Read};
    let mut f = File::open(input)?;
    let (mut header, layout, clean) = read_seed(&mut f)?;
    let game = clean["game_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing film game id"))?;
    header[16..32].copy_from_slice(&live_proto::film::sharing::header_id(game));
    let seed = zstd::bulk::compress(&serde_json::to_vec(&clean)?, 9)?;
    header[36..40].copy_from_slice(&u32::try_from(seed.len())?.to_le_bytes());
    out.rewind()?;
    out.set_len(0)?;
    out.write_all(&header)?;
    out.write_all(&seed)?;
    let mut source_offsets = Sha256::new();
    let mut offsets = Vec::new();
    for i in 0..layout.chunks {
        let pos = f.stream_position()?;
        source_offsets.update(pos.to_le_bytes());
        let mut block = [0; 5];
        f.read_exact(&mut block)?;
        let len = u32::from_le_bytes(block[1..].try_into()?) as usize;
        ensure!(
            block[0] == 0
                && (1..=crate::range::MAX_TICK_PACKED).contains(&len)
                && pos + 5 + len as u64 <= layout.trailer,
            "invalid input film block"
        );
        let mut packed = vec![0; len];
        f.read_exact(&mut packed)?;
        let raw = zstd::bulk::decompress(&packed, crate::range::MAX_TICK_BYTES)?;
        let ticks = (layout.ticks - i * 256).min(256) as usize;
        ensure!(
            raw.len() >= 4 + 3 * ticks + 1 + 8
                && raw[..2] == (ticks as u16).to_le_bytes()
                && raw[4..4 + ticks].iter().all(|flag| flag & !7 == 0),
            "invalid input film tick count or flags"
        );
        let mut encoder = zstd::stream::Encoder::new(Vec::new(), 9)?;
        encoder.include_checksum(true)?;
        encoder.set_pledged_src_size(Some(raw.len() as u64))?;
        std::io::copy(&mut Cursor::new(raw), &mut encoder)?;
        let compact = encoder.finish()?;
        offsets.push(out.stream_position()?);
        block[1..].copy_from_slice(&u32::try_from(compact.len())?.to_le_bytes());
        out.write_all(&block)?;
        out.write_all(&compact)?;
        ensure!(
            out.stream_position()? <= MAX_FILM,
            "compacted file exceeds quota"
        );
    }
    ensure!(
        f.stream_position()? == layout.trailer,
        "unexpected data before film trailer"
    );
    let mut count = [0; 4];
    f.read_exact(&mut count)?;
    ensure!(
        u32::from_le_bytes(count) == layout.chunks,
        "invalid trailer length"
    );
    let mut trailer_hash = Sha256::new();
    std::io::copy(&mut f, &mut trailer_hash)?;
    ensure!(
        trailer_hash.finalize() == source_offsets.finalize(),
        "invalid film block directory"
    );
    let trailer = out.stream_position()?;
    out.write_all(&count)?;
    for offset in offsets {
        out.write_all(&offset.to_le_bytes())?;
    }
    ensure!(
        out.stream_position()? <= MAX_FILM,
        "compacted file exceeds quota"
    );
    out.seek(SeekFrom::Start(40))?;
    out.write_all(&trailer.to_le_bytes())?;
    out.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod prepare_tests {
    use super::*;
    use live_proto::film::sharing::{HEADER, Layout, MAX_SEED};
    use serde_json::json;

    #[test]
    fn prepared_film_keeps_tick_digests_and_drops_recording_identity_and_sidecars() {
        let dir = std::env::temp_dir().join(live_proto::sign::random_id());
        fs::create_dir(&dir).unwrap();
        let source = dir.join("game.h2film");
        let sidecar = dir.join("game.keyframes");
        fs::write(&sidecar, b"KEYF private local bytes").unwrap();
        let hash = "0".repeat(64);
        let seed = json!({"game_id":"test-game","map":"lockout","map_sha256":hash,
            "seed":10,"compat":{"xbe":hash,"contract":hash,"hooks":hash,"abi":"h2-sim/2"},
            "server":"synthetic.example","roster":[{"seat":0,"name":"Recorder","team":1,
            "xuid":"synthetic-account","address":"synthetic-machine"}]});
        let packed = zstd::bulk::compress(&serde_json::to_vec(&seed).unwrap(), 1).unwrap();
        let raw = [1u8, 0, 0, 0, 1, 0x55, 0x66, 0, 1, 2, 3, 4, 5, 6, 7, 8];
        let tick = zstd::bulk::compress(&raw, 1).unwrap();
        let mut bytes = vec![0; HEADER];
        bytes[..8].copy_from_slice(b"H2FILM4!");
        bytes[8..16].copy_from_slice(&[4, 0, 2, 0, 30, 16, 0, 1]);
        bytes[32..36].copy_from_slice(&1u32.to_le_bytes());
        bytes[36..40].copy_from_slice(&(packed.len() as u32).to_le_bytes());
        bytes.extend(packed);
        let first = bytes.len() as u64;
        bytes.push(0);
        bytes.extend((tick.len() as u32).to_le_bytes());
        bytes.extend(tick);
        let trailer = bytes.len() as u64;
        bytes[40..48].copy_from_slice(&trailer.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(first.to_le_bytes());
        fs::write(&source, &bytes).unwrap();
        let library = crate::Library::open(&dir).unwrap();
        let row = library.list().unwrap().pop().unwrap();
        assert_eq!(row.meta.map, "lockout");
        assert_eq!(row.version, 4);
        assert_eq!(library.get(&row.meta.id).unwrap().path, source);
        let dest = dir.join("share");
        prepare(&source, &mut File::create(&dest).unwrap()).unwrap();
        let shared = fs::read(dest).unwrap();
        let layout = Layout::parse(&shared[..HEADER], shared.len() as u64).unwrap();
        let seed =
            zstd::bulk::decompress(&shared[HEADER..layout.seed_end as usize], MAX_SEED).unwrap();
        let clean: serde_json::Value = serde_json::from_slice(&seed).unwrap();
        assert_eq!(
            clean["roster"][0],
            json!({"seat":0,"name":"Recorder","team":1})
        );
        assert!(clean.get("server").is_none());
        let tick = zstd::bulk::decompress(
            &shared[layout.seed_end as usize + 5..layout.trailer as usize],
            1024,
        )
        .unwrap();
        assert_eq!(tick, raw);
        assert_eq!(
            shared[16..32],
            live_proto::film::sharing::header_id("test-game")
        );
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert_eq!(fs::read(sidecar).unwrap(), b"KEYF private local bytes");
        bytes[10] |= 1;
        fs::write(&source, &bytes).unwrap();
        assert!(prepare(&source, &mut File::create(dir.join("refused")).unwrap()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
