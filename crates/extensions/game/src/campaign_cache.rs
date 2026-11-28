//! Local raw sound chunks are streamed from the campaign's normal cache file.
//! Retail has two independent limits. The header validator calls 0x214ed0
//! for the per-type limit. Slot initialization at 0x2139d4 and slot selection
//! at 0x214bd0 both read {first slot, last slot, capacity} from 0x448f98.

use std::path::Path;

use anyhow::{Result, ensure};
use hooks::{BuildError, Builder, Flow, Manifest};
use map::writer::NATIVE_CAMPAIGN_FILE_LIMIT;
use runtime::guest::{Guest, PAGE_SIZE, Permissions};

const CAMPAIGN_ROW: u32 = 0x0044_8fc8;
const RETAIL: u32 = 0x1180_0000;

/// The retail selector assigns multiplayer to slot 3 and campaigns to
/// slots 4 and 5 (kernel/tests/cache_slots.rs). Only a converted campaign
/// header can use the enlarged campaign table and its validator override.
pub fn reload_limits(
    memory: &Guest,
    name: &str,
    extended_campaign: bool,
) -> std::result::Result<(map::CacheType, u32), String> {
    let slot = memory.u32(crate::map_slots::CURRENT).map_err(|e| e.to_string())?;
    match slot {
        3 => Ok((map::CacheType::Multiplayer, 0x0500_0000)),
        4 | 5 => {
            let capacity = memory.u32(CAMPAIGN_ROW + 8).map_err(|e| e.to_string())?;
            let limit = if extended_campaign && is_h1_level(name.as_bytes()) {
                NATIVE_CAMPAIGN_FILE_LIMIT
            } else {
                RETAIL
            };
            Ok((map::CacheType::Campaign, capacity.min(limit)))
        }
        _ => Err("the active slot is not a campaign or multiplayer map".into()),
    }
}

/// Both asset variants and loose maps can be selected during a session.
/// Reserve enough room for the largest at boot, rounded up to a MiB.
fn slot_size<'a>(
    maps: impl Iterator<Item = &'a vfs::MapSource>,
    overrides: Option<&Path>,
) -> u32 {
    let installed = maps.filter_map(vfs::MapSource::len_on_disk);
    // A loose map can have a retail filename and a converted header name.
    let loose = overrides
        .and_then(|dir| dir.read_dir().ok())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|entry| entry.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("map")));
    let loose_sizes = loose
        .filter_map(|path| {
            let meta = path.metadata().ok()?;
            if !meta.is_file() || meta.len() <= u64::from(RETAIL) {
                return None;
            }
            let file = vfs::HostFile {
                generation: None,
                path,
                offset: 0,
                len: meta.len(),
            };
            let name = crate::map_override::header_name(&file)?;
            is_h1_level(name.as_bytes()).then_some(file.len)
        });
    capacity(installed.chain(loose_sizes).max().unwrap_or(0))
}

fn capacity(bytes: u64) -> u32 {
    bytes
        .clamp(RETAIL.into(), NATIVE_CAMPAIGN_FILE_LIMIT.into())
        .next_multiple_of(1 << 20) as u32
}

fn is_h1_level(name: &[u8]) -> bool {
    crate::health::HALO1_LEVELS
        .iter()
        .any(|n| n.as_bytes().eq_ignore_ascii_case(name))
}

struct CampaignLimit;

/// The validator passes the type in ECX and the header in ESI. Take the
/// capacity from the same word the native allocator and selector use.
fn converted_limit(
    memory: &Guest,
    kind: u32,
    header: u32,
) -> Result<Option<u32>, runtime::GuestFault> {
    if kind != 0 {
        return Ok(None);
    }
    let name = memory.read::<4>(header.wrapping_add(0x198))?;
    if name[3] == 0 && is_h1_level(&name[..3]) {
        return Ok(Some(memory.u32(CAMPAIGN_ROW + 8)?));
    }
    Ok(None)
}

impl hooks::Feature for CampaignLimit {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "h1-campaign-cache",
            "Stream converted campaign maps from larger cache slots",
        )
        .doc("docs/halo1/sounds.md")
    }

    fn build(&self, b: &mut Builder) -> std::result::Result<(), BuildError> {
        b.raw().before(
            &guest_layout::points::CACHE_TYPE_LIMIT,
            "converted campaign size",
            |c| {
                // ESI is the header at 0x1228b8. Only interpret it at that call.
                let limit = {
                    let mem = c.ctx.runtime().memory();
                    if mem.u32(c.cpu.esp)? != 0x0012_28bd {
                        return Ok(Flow::Continue);
                    }
                    converted_limit(&mem, c.cpu.ecx, c.cpu.esi)?
                };
                if let Some(size) = limit {
                    c.cpu.eax = size;
                    return c.ret(0).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        );
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// The loader maps .rdata one page at a time. Change the capacity word at
/// startup, restoring the page permissions before any guest code runs.
fn resize_slots(memory: &mut Guest, limit: u32) -> Result<()> {
    let at = CAMPAIGN_ROW + 8;
    ensure!(
        memory.u32(CAMPAIGN_ROW)? == 4
            && memory.u32(CAMPAIGN_ROW + 4)? == 5
            && memory.u32(at)? == RETAIL,
        "unrecognized campaign cache slot table"
    );
    let page = at & !(PAGE_SIZE - 1);
    let permissions = memory
        .mappings()
        .iter()
        .find(|m| m.virtual_start == page && m.len == PAGE_SIZE)
        .ok_or_else(|| anyhow::anyhow!("campaign cache table page is missing"))?
        .permissions;
    memory.protect(page, Permissions::RW)?;
    let written = memory.set_u32(at, limit);
    memory.protect(page, permissions)?;
    Ok(written?)
}

pub fn configure(
    features: &mut hooks::Registry,
    rt: &runtime::Runtime,
    cfg: &mut vfs::Config,
    maps: &[(String, vfs::MapSource)],
    overrides: Option<&Path>,
) -> Result<()> {
    let variants = crate::campaigns::halo2_maps(&cfg.data_root);
    let sources = maps.iter().flat_map(|(name, source)| {
        std::iter::once(source).chain(variants.get(name))
    });
    let size = slot_size(sources, overrides);
    if size > RETAIL && features.add(&CampaignLimit).map_err(anyhow::Error::msg)? {
        resize_slots(&mut rt.memory(), size)?;
        // N: holds both campaign slots. XAPI may choose any utility partition.
        cfg.extra_cache_bytes = 2 * (size - RETAIL);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn converted_cache_capacity_matches_whole_mib_within_native_bounds() {
        assert_eq!(capacity(0), RETAIL);
        assert_eq!(capacity(u64::from(RETAIL) + 1), RETAIL + (1 << 20));
        assert_eq!(capacity(u64::MAX), NATIVE_CAMPAIGN_FILE_LIMIT);
        assert!(is_h1_level(b"A30"));
        assert!(!is_h1_level(b"a30_extra"));
        assert!(!is_h1_level(b"05a_deltaapproach"));
    }

    #[test]
    fn size_uses_both_variants_and_the_internal_name_of_a_loose_map() {
        use std::io::{Seek, SeekFrom, Write};
        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let path = std::env::temp_dir().join(format!("h2-campaign-slot-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let dir = Temp(path);
        std::fs::create_dir(dir.0.join("h2")).unwrap();
        std::fs::create_dir(dir.0.join("loose")).unwrap();
        let write = |path: PathBuf, name: &str, size: u32| {
            let mut f = std::fs::File::create_new(path).unwrap();
            f.set_len(size.into()).unwrap();
            f.seek(SeekFrom::Start(0x198)).unwrap();
            f.write_all(name.as_bytes()).unwrap();
        };
        let primary = dir.0.join("a30.map");
        write(primary.clone(), "a30", RETAIL + 1);
        write(dir.0.join("h2/a30.map"), "a30", RETAIL + (2 << 20));
        let maps = [primary.into(), dir.0.join("h2/a30.map").into()];
        assert_eq!(slot_size(maps.iter(), None), RETAIL + (2 << 20));
        let loose = dir.0.join("loose");
        write(
            loose.join("large.map"),
            "05a_deltaapproach",
            NATIVE_CAMPAIGN_FILE_LIMIT,
        );
        write(loose.join("override.MAP"), "a10", RETAIL + (3 << 20));
        assert_eq!(slot_size(maps.iter(), Some(&loose)), RETAIL + (3 << 20));
    }

    #[test]
    fn reload_uses_the_active_slot_and_its_installed_header_validator() {
        let mut m = Guest::new();
        for at in [crate::map_slots::CURRENT, CAMPAIGN_ROW] {
            m.map_zeroed(at & !(PAGE_SIZE - 1), PAGE_SIZE, Permissions::RW).unwrap();
        }
        m.set_u32(CAMPAIGN_ROW + 8, RETAIL + (1 << 20)).unwrap();
        m.set_u32(crate::map_slots::CURRENT, 3).unwrap();
        assert_eq!(
            reload_limits(&m, "a30", true).unwrap(),
            (map::CacheType::Multiplayer, 0x0500_0000)
        );
        m.set_u32(crate::map_slots::CURRENT, 4).unwrap();
        assert_eq!(
            reload_limits(&m, "a30", true).unwrap(),
            (map::CacheType::Campaign, RETAIL + (1 << 20))
        );
        assert_eq!(reload_limits(&m, "a30", false).unwrap().1, RETAIL);
        assert_eq!(reload_limits(&m, "other", true).unwrap().1, RETAIL);
        m.set_u32(crate::map_slots::CURRENT, 0).unwrap();
        assert!(reload_limits(&m, "a30", true).is_err());
    }

    #[test]
    fn both_slots_get_the_new_capacity_and_the_table_stays_read_only() {
        let mut m = Guest::new();
        let page = CAMPAIGN_ROW & !(PAGE_SIZE - 1);
        m.map_zeroed(page, PAGE_SIZE, Permissions::RW).unwrap();
        for (i, value) in [4, 5, RETAIL].into_iter().enumerate() {
            m.set_u32(CAMPAIGN_ROW + 4 * i as u32, value).unwrap();
        }
        m.protect(page, Permissions::READ).unwrap();
        resize_slots(&mut m, NATIVE_CAMPAIGN_FILE_LIMIT).unwrap();
        assert_eq!(m.u32(CAMPAIGN_ROW + 8).unwrap(), NATIVE_CAMPAIGN_FILE_LIMIT);
        assert!(m.set_u32(CAMPAIGN_ROW + 8, 0).is_err());

        let header = 0x1000;
        m.map_zeroed(header, PAGE_SIZE, Permissions::RW).unwrap();
        m.write(header + 0x198, *b"a30\0").unwrap();
        assert_eq!(
            converted_limit(&m, 0, header).unwrap(),
            Some(NATIVE_CAMPAIGN_FILE_LIMIT)
        );
        assert_eq!(converted_limit(&m, 1, header).unwrap(), None);
        m.write(header + 0x198, *b"05a_").unwrap();
        assert_eq!(converted_limit(&m, 0, header).unwrap(), None);
    }

    #[test]
    #[ignore = "reads the owner's Halo 2 disc ($H2_DISC)"]
    fn retail_slot_table_covers_both_campaign_slots() {
        let path = std::env::var_os("H2_DISC").expect("set H2_DISC");
        let file = crate::read_xbe(Path::new(&path)).unwrap();
        let xbe = xbe::Xbe::parse(&file).unwrap();
        assert_eq!(xbe.read_u32(CAMPAIGN_ROW).unwrap(), 4);
        assert_eq!(xbe.read_u32(CAMPAIGN_ROW + 4).unwrap(), 5);
        assert_eq!(xbe.read_u32(CAMPAIGN_ROW + 8).unwrap(), RETAIL);

        let loaded = runtime::load_xbe_with(&file, &runtime::loader::LoadOptions::halo2()).unwrap();
        let mut mem = loaded.runtime.memory();
        resize_slots(&mut mem, NATIVE_CAMPAIGN_FILE_LIMIT).unwrap();
        assert_eq!(
            mem.u32(CAMPAIGN_ROW + 8).unwrap(),
            NATIVE_CAMPAIGN_FILE_LIMIT
        );
        assert!(mem.set_u32(CAMPAIGN_ROW + 8, RETAIL).is_err());
    }
}
