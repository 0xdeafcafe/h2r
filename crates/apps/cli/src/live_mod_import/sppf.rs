//! Super PPF patches compare four regions independently. Their boundaries
//! move with the map header. See docs/game/maps.md, "Patch formats".

use anyhow::{Context, Result, bail, ensure};

const HEADER: usize = 0x800;
const LENGTH_TABLE: usize = 16;

fn word(bytes: &[u8], at: usize) -> Result<usize> {
    let word = bytes.get(at..at + 4).context("truncated SPPF header")?;
    Ok(u32::from_le_bytes(word.try_into()?) as usize)
}

fn regions(header: &[u8]) -> Result<[usize; 5]> {
    ensure!(
        header.starts_with(b"daeh") && word(header, 4)? == 8 && &header[0x7fc..HEADER] == b"toof",
        "SPPF needs a Halo 2 Xbox cache header"
    );
    // The fixed string slots follow the BSPs. Low-detail bitmap data
    // follows the string/name tables, and the tag index starts the last region.
    let starts = [
        0,
        word(header, 0x160)?,
        word(header, 0x158)?,
        word(header, 0x10)?,
        word(header, 8)?,
    ];
    ensure!(
        starts[1] >= HEADER && starts.windows(2).all(|w| w[0] <= w[1]),
        "invalid SPPF map region offsets"
    );
    Ok(starts)
}

fn sections(patch: &[u8]) -> Result<[&[u8]; 4]> {
    let mut data = patch
        .get(LENGTH_TABLE..)
        .context("truncated SPPF length table")?;
    let mut sections = [&[][..]; 4];
    for (index, section) in sections.iter_mut().enumerate() {
        let length = word(patch, index * 4)?;
        (*section, data) = data
            .split_at_checked(length)
            .context("truncated SPPF section")?;
    }
    ensure!(
        data.is_empty(),
        "trailing bytes after the fourth SPPF section"
    );
    Ok(sections)
}

fn records(mut section: &[u8], mut write: impl FnMut(usize, &[u8]) -> Result<()>) -> Result<()> {
    while !section.is_empty() {
        let offset = word(section, 0)?;
        let size = word(section, 4)?;
        let (bytes, rest) = section[8..]
            .split_at_checked(size)
            .context("truncated SPPF record")?;
        write(offset, bytes)?;
        section = rest;
    }
    Ok(())
}

pub(super) fn apply(base: &[u8], patch: &[u8]) -> Result<Vec<u8>> {
    let mut header = base
        .get(..HEADER)
        .context("truncated SPPF base map header")?
        .to_vec();
    let before = regions(&header)?;
    ensure!(before[4] <= base.len(), "truncated SPPF base map");
    let sections = sections(patch)?;
    // Header edits establish where the other sections land, even when a
    // particular record overlaps the end of the header.
    records(sections[0], |at, bytes| {
        if let Some(dst) = header.get_mut(at..) {
            let take = bytes.len().min(dst.len());
            dst[..take].copy_from_slice(&bytes[..take]);
        }
        Ok(())
    })?;
    let after = regions(&header)?;
    ensure!(
        after[4] <= super::MAX_CONVERTED_MAP,
        "SPPF target exceeds the map size limit"
    );
    let mut result = vec![0u8; after[4]];
    for (index, section) in sections.into_iter().enumerate() {
        let src = &base[before[index]..before[index + 1]];
        let dst = &mut result[after[index]..after[index + 1]];
        let keep = src.len().min(dst.len());
        dst[..keep].copy_from_slice(&src[..keep]);
        records(section, |offset, bytes| {
            let Some(run) = dst
                .get_mut(offset..)
                .and_then(|tail| tail.get_mut(..bytes.len()))
            else {
                bail!("SPPF record crosses region {}", index + 1)
            };
            run.copy_from_slice(bytes);
            Ok(())
        })?;
    }
    Ok(result)
}
