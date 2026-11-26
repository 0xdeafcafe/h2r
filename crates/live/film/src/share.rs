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
