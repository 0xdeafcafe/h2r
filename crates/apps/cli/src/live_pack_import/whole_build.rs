//! Identify whole multiplayer caches in a community build and pair them
//! with the owner's disc. Only selected maps reach the package differ.

use super::{Candidate, source};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Take};
use std::path::{Path, PathBuf};
use vfs::{Backend, HostFile};

pub(super) const HEADER_BYTES: usize = 0x800;

struct Identity {
    name: String,
    scenario: String,
}

fn field(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase()
}

// Read just the header; an old mod may have rebuilt tag tables or an
// altered build string. See docs/formats/map-format.md for cache fields.
fn multiplayer(mut reader: impl Read) -> Result<Option<Identity>> {
    let mut header = [0; HEADER_BYTES];
    reader.read_exact(&mut header)?;
    if &header[..4] != b"daeh"
        || &header[0x7fc..] != b"toof"
        || u32::from_le_bytes(header[4..8].try_into().unwrap()) != 8
    {
        bail!("expected an Xbox v8 map header");
    }
    Ok(
        (crate::live_art::cache_type(&header) == Some(map::CacheType::Multiplayer)).then(|| {
            Identity {
                name: field(&header[0x198..0x1b8]),
                scenario: field(&header[0x1bc..0x2bc]).replace('/', "\\"),
            }
        }),
    )
}

pub(super) fn is_multiplayer(head: &[u8]) -> Result<bool> {
    Ok(multiplayer(head)?.is_some())
}

fn open_region(source: &HostFile) -> Result<Take<File>> {
    let mut file = File::open(&source.path).with_context(|| source.path.display().to_string())?;
    file.seek(SeekFrom::Start(source.offset))?;
    Ok(file.take(source.len))
}

/// Read the real DVD's `maps` directory with the same case folding and
/// partition-offset handling used to load retail bases.
fn level_files(disc: &Path) -> Result<Vec<(String, HostFile, Identity)>> {
    let mut dvd = vfs::open_disc(disc).with_context(|| format!("opening {}", disc.display()))?;
    let mut levels = Vec::new();
    for entry in dvd.read_directory("maps").context("listing maps on disc")? {
        if entry.metadata.is_directory() || !entry.name.to_ascii_lowercase().ends_with(".map") {
            continue;
        }
        let source = dvd
            .host_file(&format!("maps\\{}", entry.name))
            .with_context(|| format!("missing disc entry {}", entry.name))?;
        if let Some(level) = multiplayer(open_region(&source)?)
            .with_context(|| format!("{} in {}", entry.name, disc.display()))?
        {
            levels.push((entry.name, source, level));
        }
    }
    Ok(levels)
}

/// The importer owns `scratch` and removes its contents when the
/// import finishes. The paths on the DVD are display metadata only.
pub(super) fn discover_iso(iso: &Path, scratch: &Path) -> Result<Vec<Candidate>> {
    let levels = level_files(iso)?;
    if levels.len() > source::MAX_FILES {
        bail!("XISO has more than {} multiplayer maps", source::MAX_FILES);
    }
    let mut budget = source::MAX_DISC;
    for (name, region, _) in &levels {
        if region.len > source::MAX_MAP || region.len > budget {
            bail!("{name} exceeds the import size limit in {}", iso.display());
        }
        budget -= region.len;
    }
    std::fs::create_dir_all(scratch)?;
    let mut out = Vec::new();
    for (i, (name, source, _)) in levels.into_iter().enumerate() {
        let file = scratch.join(format!("{i}.map"));
        let mut output = File::create_new(&file)?;
        let copied = io::copy(&mut open_region(&source)?, &mut output)?;
        if copied != source.len {
            bail!("{name} ended early in {}", iso.display());
        }
        out.push(Candidate {
            logical: PathBuf::from("maps").join(name),
            file,
        });
    }
    Ok(out)
}

struct RetailMap {
    identity: Identity,
    source: HostFile,
}

pub(super) struct RetailMaps(BTreeMap<String, RetailMap>);

impl RetailMaps {
    pub(super) fn open(disc: &Path, mappack: Option<&Path>) -> Result<Self> {
        let mut maps = BTreeMap::new();
        for (name, source, identity) in level_files(disc)? {
            let stem = name[..name.len() - ".map".len()].to_ascii_lowercase();
            maps.insert(stem, RetailMap { identity, source });
        }
        if let Some(iso) = mappack {
            for map in crate::live_mappack::from_iso(iso)? {
                if let Some(identity) = multiplayer(map.head.as_slice())
                    .with_context(|| format!("{} in {}", map.stem, iso.display()))?
                {
                    let source = HostFile {
                        path: map.host,
                        offset: map.base,
                        len: map.size.into(),
                    };
                    // The normal map loader prefers the main disc on a duplicate.
                    maps.entry(map.stem)
                        .or_insert(RetailMap { identity, source });
                }
            }
        }
        Ok(Self(maps))
    }

    pub(super) fn stems(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// Use the retail filename or the internal identity of a renamed
    /// level. Conflicting matches would create a patch over the wrong map.
    pub(super) fn changed_base(&self, logical: &Path, file: &Path) -> Result<Option<String>> {
        let mut input = File::open(file)?;
        let Some(identity) =
            multiplayer(&mut input).with_context(|| logical.display().to_string())?
        else {
            return Ok(None);
        };
        let file_stem = logical.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let mut matches = self.0.iter().filter(|(stem, base)| {
            stem.eq_ignore_ascii_case(file_stem)
                || !identity.name.is_empty() && base.identity.name == identity.name
                || !identity.scenario.is_empty() && base.identity.scenario == identity.scenario
        });
        let (stem, base) = matches.next().with_context(|| {
            format!(
                "{} is multiplayer but matches no base on the retail disc or Multiplayer Map Pack",
                logical.display()
            )
        })?;
        if let Some((other, _)) = matches.next() {
            bail!(
                "{} has conflicting retail level identities ({stem} and {other})",
                logical.display()
            );
        }
        if input.metadata()?.len() == base.source.len {
            input.rewind()?;
            if same_bytes(&mut input, open_region(&base.source)?, base.source.len)? {
                return Ok(None);
            }
        }
        Ok(Some(stem.clone()))
    }
}

fn same_bytes(mut a: impl Read, mut b: impl Read, mut remaining: u64) -> io::Result<bool> {
    let mut left = vec![0; 64 * 1024];
    let mut right = vec![0; left.len()];
    while remaining != 0 {
        let n = remaining.min(left.len() as u64) as usize;
        a.read_exact(&mut left[..n])?;
        b.read_exact(&mut right[..n])?;
        if left[..n] != right[..n] {
            return Ok(false);
        }
        remaining -= n as u64;
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
