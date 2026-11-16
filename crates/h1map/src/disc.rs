//! The Halo 1 Xbox disc: recognising it and picking its maps.
//!
//! Only the directory tree and `default.xbe`'s certificate are read here.
//! Copying the maps and writing the manifest is the caller's job (`h2 import
//! halo1`); [`copy_hashed`] and [`manifest`] are the pieces it uses.

use std::io::{Read, Seek, Write};

use sha2::{Digest, Sha256};
use xiso::{Entry, Xiso};

use crate::{Error, Result};

/// Halo: Combat Evolved's title ID in the XBE certificate.
pub const HALO1_TITLE_ID: u32 = 0x4d53_0004;
/// The campaign levels, in play order, by map file stem.
pub const CAMPAIGN_LEVELS: [&str; 10] = [
    "a10", "a30", "a50", "b30", "b40", "c10", "c20", "c40", "d20", "d40",
];
/// Other maps a campaign import takes when the disc has them: the UI map,
/// and the shared resource maps PC builds use (an Xbox disc may not have
/// them).
pub const CAMPAIGN_EXTRAS: [&str; 3] = ["ui", "bitmaps", "sounds"];
/// Where the maps are on the disc.
pub const MAPS_DIR: &str = "/maps/";
/// The XBE is a few MB; anything far bigger is not one.
const XBE_MAX: u32 = 64 << 20;

/// What the disc is, from its XBE certificate and directory tree.
#[derive(Debug, Clone)]
pub struct Disc {
    /// The certificate's title ID: [`HALO1_TITLE_ID`].
    pub title_id: u32,
    /// The certificate's title name.
    pub title_name: String,
    /// The certificate's version.
    pub version: u32,
    /// Every `.map` file directly under [`MAPS_DIR`], sorted by name.
    pub maps: Vec<Entry>,
}

impl Disc {
    /// Reads the certificate and the map list; fails unless this is Halo 1.
    pub fn open<R: Read + Seek>(iso: &mut Xiso<R>) -> Result<Self> {
        let entries = iso.entries().map_err(disc_err)?;
        let xbe_entry = entries
            .iter()
            .find(|e| !e.is_dir && e.path.eq_ignore_ascii_case("/default.xbe"))
            .ok_or_else(|| Error::Disc("no default.xbe: not a game disc".into()))?;
        if xbe_entry.size > XBE_MAX {
            return Err(Error::Disc(format!(
                "default.xbe is {} bytes, too big for an XBE",
                xbe_entry.size
            )));
        }
        let xbe = iso.read(xbe_entry).map_err(disc_err)?;
        let cert = certificate(&xbe)?;
        Ok(Self {
            title_id: cert.title_id,
            title_name: cert.title_name,
            version: cert.version,
            maps: maps(&entries),
        })
    }

    /// The map named `name` (a stem such as `a30`, or a file name).
    pub fn map(&self, name: &str) -> Option<&Entry> {
        let want = name.strip_suffix(".map").unwrap_or(name);
        self.maps
            .iter()
            .find(|e| map_stem(e).eq_ignore_ascii_case(want))
    }

    /// The maps a campaign import copies: every campaign level, which must
    /// all be there, then whichever [`CAMPAIGN_EXTRAS`] the disc has.
    pub fn campaign_maps(&self) -> Result<Vec<&Entry>> {
        let missing: Vec<&str> = CAMPAIGN_LEVELS
            .iter()
            .copied()
            .filter(|l| self.map(l).is_none())
            .collect();
        if !missing.is_empty() {
            return Err(Error::Disc(format!(
                "missing campaign maps: {}",
                missing.join(", ")
            )));
        }
        Ok(CAMPAIGN_LEVELS
            .iter()
            .chain(&CAMPAIGN_EXTRAS)
            .filter_map(|n| self.map(n))
            .collect())
    }
}

/// The certificate of a Halo 1 XBE; fails for any other title.
pub fn certificate(xbe: &[u8]) -> Result<xbe::Certificate> {
    let x = xbe::Xbe::parse(xbe).map_err(|e| Error::Disc(format!("default.xbe: {e}")))?;
    let c = x.certificate;
    if c.title_id != HALO1_TITLE_ID {
        return Err(Error::Disc(format!(
            "title {:08X} ({}) is not Halo: Combat Evolved ({HALO1_TITLE_ID:08X})",
            c.title_id, c.title_name
        )));
    }
    Ok(c)
}

/// Every `.map` file directly under [`MAPS_DIR`], sorted by name.
pub fn maps(entries: &[Entry]) -> Vec<Entry> {
    let mut out: Vec<Entry> = entries
        .iter()
        .filter(|e| !e.is_dir)
        .filter(|e| {
            let lower = e.path.to_ascii_lowercase();
            lower
                .strip_prefix(MAPS_DIR)
                .is_some_and(|rest| !rest.contains('/') && rest.ends_with(".map"))
        })
        .cloned()
        .collect();
    out.sort_by_key(|e| e.path.to_ascii_lowercase());
    out
}

/// A map entry's file name.
pub fn file_name(e: &Entry) -> &str {
    e.path.rsplit('/').next().unwrap_or_default()
}

/// A map entry's file name without `.map`.
pub fn map_stem(e: &Entry) -> &str {
    let n = file_name(e);
    n.len()
        .checked_sub(4)
        .filter(|&i| n.is_char_boundary(i) && n[i..].eq_ignore_ascii_case(".map"))
        .map_or(n, |i| &n[..i])
}

/// Copies `r` to `w`, returning the byte count and the SHA-256 of the bytes.
pub fn copy_hashed(r: &mut impl Read, w: &mut impl Write) -> std::io::Result<(u64, [u8; 32])> {
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    loop {
        let got = r.read(&mut buf)?;
        if got == 0 {
            return Ok((n, hash.finalize().into()));
        }
        hash.update(&buf[..got]);
        w.write_all(&buf[..got])?;
        n += got as u64;
    }
}

/// One copied file, for the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copied {
    /// File name as written.
    pub name: String,
    /// Bytes.
    pub size: u64,
    /// SHA-256 of the bytes.
    pub sha256: [u8; 32],
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The import manifest: a comment naming the disc, then one
/// `<sha256>  <size>  <name>` line per file.
pub fn manifest(disc: &Disc, files: &[Copied]) -> String {
    let mut s = format!(
        "# h2 import halo1: title {:08X} version {:#x}\n# sha256  size  name\n",
        disc.title_id, disc.version
    );
    for f in files {
        s += &format!("{}  {}  {}\n", hex(&f.sha256), f.size, f.name);
    }
    s
}

fn disc_err(e: xiso::Error) -> Error {
    Error::Disc(e.to_string())
}
