//! `patch.lvl` (version 2, what Halo 2 1.0 reads) and `patch_v3.lvl`
//! (version 3, what the title update reads): the multiplayer map list
//! entries a content package adds. Layouts and the reading code's addresses
//! are in docs/game/map-pack.md section 8.
//!
//! A v3 record is a v2 record with one dword (the map's cache header
//! checksum) inserted at `+0x10c`. So a v2 `patch.lvl` is made from one v3
//! record by pure mapping: version 2, `+0x8..+0x10c` copied, the inserted
//! dword dropped, the rest shifted down by 4, and the CRC recomputed.
//!
//! [`crate::map_list`] builds a v2 record from scratch for a new map.

/// A record or file that 1.0 or the update would refuse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LvlError(pub String);

type Result<T> = std::result::Result<T, LvlError>;

macro_rules! bail {
    ($($t:tt)*) => { return Err(LvlError(format!($($t)*))) };
}

/// `patch.lvl` is exactly one record of this size; 1.0 reads this many bytes
/// and no more (`0x19bfd0`).
pub const V2_LEN: usize = 0x2d3fc;
/// One `patch_v3.lvl` record.
pub const V3_RECORD_LEN: usize = 0x2d400;
/// `patch_v3.lvl` starts with version and record count.
pub const V3_HEADER_LEN: usize = 8;

pub(crate) const VERSION: usize = 0x0;
pub(crate) const CRC: usize = 0x4;
pub(crate) const KIND: usize = 0x8;
/// Record type: the only one 1.0's applier (`0x19be00`) accepts.
pub(crate) const KIND_MAP: u32 = 1;
pub(crate) const SCENARIO: usize = 0xc;
pub(crate) const SCENARIO_LEN: usize = 0x100;
/// v2: the map id. v3: the inserted checksum, with the map id after it.
const INSERTED: usize = 0x10c;
pub(crate) const V2_MAP_ID: usize = 0x10c;
#[cfg(test)]
const V3_MAP_ID: usize = 0x110;

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// The game's CRC (`0x163ba0`): reflected CRC-32, polynomial `0xedb88320`,
/// initial value `0xffffffff`, and no final XOR.
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32_update(0xffff_ffff, bytes)
}

fn crc32_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    crc
}

/// The CRC of a record as the validators compute it: over the whole record
/// with its CRC field read as zero.
pub fn record_crc(record: &[u8]) -> u32 {
    let crc = crc32_update(crc32(&record[..CRC]), &[0; 4]);
    crc32_update(crc, &record[CRC + 4..])
}

fn validate(record: &[u8], version: u32, len: usize) -> Result<()> {
    if record.len() != len {
        bail!("record is {:#x} bytes, not {len:#x}", record.len());
    }
    if u32_at(record, VERSION) != version {
        bail!("record version is {}, not {version}", u32_at(record, VERSION));
    }
    let (have, want) = (u32_at(record, CRC), record_crc(record));
    if have != want {
        bail!("record CRC is {have:#010x}, computed {want:#010x}");
    }
    Ok(())
}

/// 1.0's check of `patch.lvl` (`0x19bdc0`, then the type test in `0x19be00`):
/// exactly one record, version 2, a good CRC, record type 1.
pub fn validate_v2(record: &[u8]) -> Result<()> {
    validate(record, 2, V2_LEN)?;
    if u32_at(record, KIND) != KIND_MAP {
        bail!("record type is {}, not {KIND_MAP}", u32_at(record, KIND));
    }
    Ok(())
}

/// The update's check of one `patch_v3.lvl` record (`0x19f270`), plus the
/// record type.
pub fn validate_v3_record(record: &[u8]) -> Result<()> {
    validate(record, 3, V3_RECORD_LEN)?;
    if u32_at(record, KIND) != KIND_MAP {
        bail!("record type is {}, not {KIND_MAP}", u32_at(record, KIND));
    }
    Ok(())
}

/// Splits a `patch_v3.lvl` into its records, each checked.
pub fn v3_records(file: &[u8]) -> Result<Vec<&[u8]>> {
    if file.len() < V3_HEADER_LEN {
        bail!("patch_v3.lvl is {} bytes, too short for its header", file.len());
    }
    if u32_at(file, 0) != 3 {
        bail!("patch_v3.lvl version is {}, not 3", u32_at(file, 0));
    }
    let count = u32_at(file, 4) as usize;
    let want = count.checked_mul(V3_RECORD_LEN).and_then(|n| n.checked_add(V3_HEADER_LEN));
    if want != Some(file.len()) {
        bail!("patch_v3.lvl is {:#x} bytes, not 8 + {count} records of {V3_RECORD_LEN:#x}", file.len());
    }
    let (records, _) = file[V3_HEADER_LEN..].as_chunks::<V3_RECORD_LEN>();
    let records: Vec<&[u8]> = records.iter().map(|r| r.as_slice()).collect();
    for (i, r) in records.iter().enumerate() {
        validate_v3_record(r).map_err(|e| LvlError(format!("patch_v3.lvl record {i}: {e}")))?;
    }
    Ok(records)
}

/// The version 2 `patch.lvl` for one v3 record (docs/game/map-pack.md 8.2).
pub fn v2_from_v3(v3: &[u8]) -> Result<Vec<u8>> {
    validate_v3_record(v3)?;
    let mut v2 = vec![0u8; V2_LEN];
    v2[VERSION..VERSION + 4].copy_from_slice(&2u32.to_le_bytes());
    v2[KIND..INSERTED].copy_from_slice(&v3[KIND..INSERTED]);
    v2[INSERTED..].copy_from_slice(&v3[INSERTED + 4..]);
    let crc = record_crc(&v2);
    v2[CRC..CRC + 4].copy_from_slice(&crc.to_le_bytes());
    Ok(v2)
}

/// The map id (v2 `+0x10c`), the key of the map list slot the record fills.
pub fn v2_map_id(v2: &[u8]) -> u32 {
    u32_at(v2, V2_MAP_ID)
}

/// The map a record names: the basename of its scenario path. 1.0 opens
/// `<content dir>\<this>.map` (`0x19be00`, `0x214db0`). The same field and
/// offset in v2 and v3.
pub fn map_name(record: &[u8]) -> Result<String> {
    let field = &record[SCENARIO..SCENARIO + SCENARIO_LEN];
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    let path = std::str::from_utf8(&field[..end]).ok().filter(|p| p.is_ascii());
    let Some(path) = path else { bail!("the record's scenario path is not ASCII") };
    let base = path.rsplit(['\\', '/']).next().unwrap_or_default();
    if base.is_empty() {
        bail!("the record's scenario path {path:?} has no map name");
    }
    Ok(base.to_string())
}

/// Synthetic records for tests here and in other crates (the map pack
/// import's).
pub mod testing {
    use super::*;

    /// A synthetic v3 record: patterned filler, the given scenario path,
    /// map id and inserted checksum, and a correct CRC.
    pub fn v3_record(scenario: &str, map_id: u32, checksum: u32) -> Vec<u8> {
        let mut r: Vec<u8> = (0..V3_RECORD_LEN).map(|i| (i * 7 + i / 251) as u8).collect();
        r[VERSION..VERSION + 4].copy_from_slice(&3u32.to_le_bytes());
        r[KIND..KIND + 4].copy_from_slice(&KIND_MAP.to_le_bytes());
        r[SCENARIO..SCENARIO + SCENARIO_LEN].fill(0);
        r[SCENARIO..SCENARIO + scenario.len()].copy_from_slice(scenario.as_bytes());
        r[INSERTED..INSERTED + 4].copy_from_slice(&checksum.to_le_bytes());
        r[INSERTED + 4..INSERTED + 8].copy_from_slice(&map_id.to_le_bytes());
        let crc = record_crc(&r);
        r[CRC..CRC + 4].copy_from_slice(&crc.to_le_bytes());
        r
    }

    /// A `patch_v3.lvl` holding `records`.
    pub fn v3_file(records: &[Vec<u8>]) -> Vec<u8> {
        let mut f = 3u32.to_le_bytes().to_vec();
        f.extend((records.len() as u32).to_le_bytes());
        for r in records {
            f.extend(r);
        }
        f
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn crc_is_crc32_without_the_final_xor() {
        // The CRC-32 check value for "123456789" is 0xcbf43926, after the
        // final XOR this variant leaves out.
        assert_eq!(crc32(b"123456789"), !0xcbf4_3926);
        assert_eq!(crc32(b""), 0xffff_ffff);
    }

    #[test]
    fn record_crc_reads_the_crc_field_as_zero() {
        let mut r: Vec<u8> = (0..64u8).collect();
        let mut zeroed = r.clone();
        zeroed[4..8].fill(0);
        let want = crc32(&zeroed);
        assert_eq!(record_crc(&r), want);
        r[4..8].copy_from_slice(&[1, 2, 3, 4]);
        assert_eq!(record_crc(&r), want, "the field's own value doesn't count");
    }

    #[test]
    fn a_v3_records_own_crc_recomputes() {
        let r = v3_record("scenarios\\multi\\alpha\\alpha", 0x51, 0xdead_beef);
        validate_v3_record(&r).unwrap();
        let mut zeroed = r.clone();
        zeroed[CRC..CRC + 4].fill(0);
        assert_eq!(u32_at(&r, CRC), crc32(&zeroed));
        let mut bad = r.clone();
        bad[0x2000] ^= 1;
        assert!(validate_v3_record(&bad).unwrap_err().to_string().contains("CRC"));
    }

    #[test]
    fn v2_is_the_v3_record_without_the_inserted_dword() {
        let v3 = v3_record("scenarios\\multi\\alpha\\alpha", 0x51, 0xdead_beef);
        let v2 = v2_from_v3(&v3).unwrap();
        assert_eq!(v2.len(), V2_LEN);
        assert_eq!(u32_at(&v2, VERSION), 2);
        assert_eq!(v2[KIND..INSERTED], v3[KIND..INSERTED]);
        assert_eq!(v2[INSERTED..], v3[INSERTED + 4..]);
        assert_eq!(v2_map_id(&v2), 0x51);
        assert_eq!(u32_at(&v3, V3_MAP_ID), 0x51);
        assert!(!v2.windows(4).any(|w| w == 0xdead_beefu32.to_le_bytes()), "the checksum is dropped");
        // The CRC is the v2 record's own, as 1.0 checks it.
        let mut zeroed = v2.clone();
        zeroed[CRC..CRC + 4].fill(0);
        assert_eq!(u32_at(&v2, CRC), crc32(&zeroed));
        validate_v2(&v2).unwrap();
        assert_eq!(map_name(&v2).unwrap(), "alpha");
        assert_eq!(map_name(&v3).unwrap(), "alpha");
    }

    #[test]
    fn lengths_versions_and_types_are_checked() {
        let v3 = v3_record("a\\b", 1, 2);
        assert!(v2_from_v3(&v3[..V3_RECORD_LEN - 4]).is_err());
        let mut long = v3.clone();
        long.push(0);
        assert!(v2_from_v3(&long).is_err());
        let v2 = v2_from_v3(&v3).unwrap();
        assert!(validate_v2(&v2[..V2_LEN - 1]).is_err());
        assert!(validate_v2(&v3).is_err(), "a v3 record is not a patch.lvl");
        assert!(validate_v3_record(&v2).is_err());

        let mut kind = v3_record("a\\b", 1, 2);
        kind[KIND] = 2;
        let crc = record_crc(&kind);
        kind[CRC..CRC + 4].copy_from_slice(&crc.to_le_bytes());
        assert!(v2_from_v3(&kind).unwrap_err().to_string().contains("type"));
    }

    #[test]
    fn v3_files_split_into_checked_records() {
        let a = v3_record("x\\alpha", 1, 2);
        let b = v3_record("x/beta", 3, 4);
        let f = v3_file(&[a.clone(), b.clone()]);
        let recs = v3_records(&f).unwrap();
        assert_eq!(recs, [a.as_slice(), b.as_slice()]);
        assert_eq!(map_name(recs[1]).unwrap(), "beta");
        assert!(v3_records(&f[..f.len() - 1]).is_err());
        assert!(v3_records(&f[..4]).is_err());
        let mut v = f.clone();
        v[0] = 2;
        assert!(v3_records(&v).is_err());
        let mut bad = f.clone();
        bad[V3_HEADER_LEN + V3_RECORD_LEN + 0x400] ^= 0x80;
        assert!(format!("{:#}", v3_records(&bad).unwrap_err()).contains("record 1"));
    }

    #[test]
    fn map_name_needs_a_basename() {
        let mut r = v3_record("x\\", 1, 2);
        assert!(map_name(&r).is_err());
        r[SCENARIO] = 0;
        assert!(map_name(&r).is_err());
        r[SCENARIO] = 0xff;
        r[SCENARIO + 1] = 0;
        assert!(map_name(&r).is_err());
    }
}
