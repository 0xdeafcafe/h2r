//! Editor map overrides and map identity through
//! the VFS. Installed mods use their own content paths.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// The environment variable naming the override directory.
pub const ENV: &str = "H2_MAP_DIR";

/// Prepare a play copy below the game thread's scheduling priority.
pub fn prepare_on_worker<T: Send + 'static>(
    prepare: impl FnOnce() -> T + Send + 'static,
) -> Result<std::sync::mpsc::Receiver<T>, String> {
    let (send, receive) = std::sync::mpsc::channel();
    background::spawn("editor map preparation", move || {
        let _ = send.send(prepare());
    })
    .map_err(|e| format!("cannot start the map preparation worker: {e}"))?;
    Ok(receive)
}

/// Pass the region to the telemetry worker with the lifetime of its source.
pub fn telemetry_file(file: vfs::HostFile) -> crate::telemetry::MapFile {
    crate::telemetry::MapFile::new(file.path, file.offset, file.len, file.generation)
}

/// The override directory an `H2_MAP_DIR` value names; unset or empty is none.
pub fn dir_from(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The override directory from the environment.
pub fn dir() -> Option<PathBuf> {
    dir_from(std::env::var_os(ENV))
}

/// Look up a map by its catalog name, for callers without an active game.
pub fn map_id(vfs: &mut vfs::Vfs, name: &str) -> Option<content::MapId> {
    if let Some(path) = vfs.content_map_path(name) {
        return vfs.map_id(&path);
    }
    vfs.map_id(&format!("{}\\maps\\{name}.map", vfs::CDROM_DEVICE))
}

/// The identity reported at game start comes from the exact slot source.
pub fn active_map_id(
    vfs: &mut vfs::Vfs,
    guest: &runtime::Guest,
    binding: Option<&vfs::MapRebind>,
) -> Option<(content::MapId, Option<String>)> {
    if binding.is_some_and(vfs::MapRebind::has_ever_bound)
        && let Some(path) = crate::map_slots::active_file(guest)
        && vfs.query_path(&path).is_ok()
    {
        // A stock disc source has no id, even if a package shares its name.
        return Some((vfs.map_id(&path)?, vfs.content_base_at(&path)));
    }
    let name = crate::telemetry::loaded_map_name(guest).ok()?;
    Some((map_id(vfs, &name)?, vfs.content_base_name(&name)))
}

/// The content package selected in the multiplayer lobby, or the map
/// currently loaded in a match. The selected list slot is authoritative
/// in the main menu, where the cache header still says `mainmenu`.
pub fn lobby_map_id(
    vfs: &mut vfs::Vfs,
    g: &runtime::Guest,
    binding: Option<&vfs::MapRebind>,
) -> Option<content::MapId> {
    if let Some(session) = crate::coop_probe::read_session(g).filter(|s| s.live()) {
        if !session.is_host() || session.is_campaign() {
            return None;
        }
        if binding.is_some_and(vfs::MapRebind::has_ever_bound)
            && crate::telemetry::loaded_map_name(g).ok().is_some_and(|n| !n.is_empty() && n != "mainmenu")
        {
            return active_map_id(vfs, g, binding).map(|(id, _)| id);
        }
        if let Some(path) = crate::map_select::selected_path(session.map) {
            return vfs.map_id(&path);
        }
        let path = crate::mod_slots::selected_path(g, session.map)?;
        if let Some(file) = crate::map_slots::slot_file(&path)
            .or_else(|| crate::map_slots::slot_file(&format!("{path}.map")))
        {
            return vfs.map_id(&file);
        }
        // Disc slots can name scenarios\multi\… instead of d:\maps\….
        let stem = path.rsplit('\\').next()?.to_ascii_lowercase();
        return map_id(vfs, stem.strip_suffix(".map").unwrap_or(&stem));
    }
    let loaded = crate::telemetry::loaded_map_name(g).ok();
    if let Some(name) = loaded
        .as_deref()
        .filter(|n| !n.is_empty() && *n != "mainmenu")
    {
        return if binding.is_some_and(vfs::MapRebind::has_ever_bound) {
            active_map_id(vfs, g, binding).map(|(id, _)| id)
        } else {
            map_id(vfs, name)
        };
    }

    use crate::layout_glue::Gm;
    let id = Gm::u32(g, crate::h1_options_row::LOBBY_MAP)?;
    vfs.map_id(&crate::map_select::selected_path(id)?)
}

/// The host file for a loaded map. A streamed overlay has no contiguous
/// host file, even though the VFS can read its bytes.
pub fn map_file(vfs: &mut vfs::Vfs, name: &str) -> Option<vfs::HostFile> {
    if let Some(path) = vfs.content_map_path(name) {
        return vfs.host_file(&path);
    }
    let dir = format!("{}\\maps", vfs::CDROM_DEVICE);
    if let Some(file) = vfs.host_file(&format!("{dir}\\{name}.map")) {
        return Some(file);
    }
    disc_map_paths(vfs, &dir)
        .into_iter()
        .filter_map(|path| vfs.host_file(&path))
        .find(|file| header_name(file).is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

/// Use the cache slot when it resolves, including aliases and direct drive paths.
/// A missing slot falls back to the loaded header's name, as ordinary boots do.
pub fn map_file_at(vfs: &mut vfs::Vfs, name: &str, slot: Option<&str>) -> Option<vfs::HostFile> {
    if let Some(path) = slot {
        if let Some(file) = vfs.host_file(path) {
            return Some(file);
        }
        // A streamed overlay can be present without a contiguous host file.
        if vfs.query_path(path).is_ok() {
            return None;
        }
    }
    map_file(vfs, name)
}

pub fn active_map_file(
    vfs: &mut vfs::Vfs,
    guest: &runtime::Guest,
    binding: Option<&vfs::MapRebind>,
) -> Option<vfs::HostFile> {
    let name = crate::telemetry::loaded_map_name(guest).ok()?;
    if name.is_empty() {
        return None;
    }
    let slot = binding
        .filter(|b| b.has_ever_bound())
        .and_then(|_| crate::map_slots::active_file(guest));
    map_file_at(vfs, &name, slot.as_deref())
}

/// The loaded map's bytes for the telemetry worker: its host region where it
/// has one, else, for a map the VFS rewrites in memory (the pogo kit, the
/// screens: no contiguous host file), those bytes. Frame names come from them
/// (docs/live/live-telemetry.md, "Frame species, aim, weapon and shots").
pub fn active_map_source(
    vfs: &mut vfs::Vfs,
    guest: &runtime::Guest,
    binding: Option<&vfs::MapRebind>,
) -> Option<crate::telemetry::MapFile> {
    let name = crate::telemetry::loaded_map_name(guest).ok()?;
    if name.is_empty() {
        return None;
    }
    let slot = binding
        .filter(|b| b.has_ever_bound())
        .and_then(|_| crate::map_slots::active_file(guest));
    match map_source(vfs, &name, slot.as_deref())? {
        vfs::FileSource::Host(file) => Some(telemetry_file(file)),
        vfs::FileSource::Memory(bytes) => Some(crate::telemetry::MapFile::memory(
            format!("{name}.map (rewritten in memory)"),
            bytes,
        )),
        vfs::FileSource::Spliced(file) => Some(crate::telemetry::MapFile::opened(
            format!("{name}.map (rewritten over the disc's)"),
            file.len(),
            std::sync::Arc::new(move || -> std::io::Result<Box<dyn crate::telemetry::MapRead>> {
                Ok(Box::new(std::io::BufReader::new(file.reader()?)))
            }),
        )),
    }
}

/// [`map_file_at`], and where the map has no host file because the VFS
/// rewrites it, its bytes by the same slot, package and disc names.
pub fn map_source(vfs: &mut vfs::Vfs, name: &str, slot: Option<&str>) -> Option<vfs::FileSource> {
    if let Some(file) = map_file_at(vfs, name, slot) {
        return Some(vfs::FileSource::Host(file));
    }
    let dir = format!("{}\\maps", vfs::CDROM_DEVICE);
    // What the VFS rewrites: whole in memory, or spliced over its original.
    let rewritten = |vfs: &mut vfs::Vfs, path: &str| match vfs.file_source(path)? {
        vfs::FileSource::Host(_) => None,
        other => Some(other),
    };
    let named = slot
        .map(str::to_owned)
        .into_iter()
        .chain(vfs.content_map_path(name))
        .chain([format!("{dir}\\{name}.map")])
        .find_map(|path| rewritten(vfs, &path));
    named.or_else(|| {
        disc_map_paths(vfs, &dir).into_iter().find_map(|path| {
            let source = rewritten(vfs, &path)?;
            source_header_name(&source)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
                .then_some(source)
        })
    })
}

/// The internal name in a rewritten file's header.
fn source_header_name(source: &vfs::FileSource) -> Option<String> {
    match source {
        vfs::FileSource::Memory(bytes) => header_name_of(bytes),
        vfs::FileSource::Spliced(file) => {
            let mut header = vec![0; 0x198 + 32];
            content::patch::Source::read_exact_at(&mut file.open().ok()?, 0, &mut header).ok()?;
            header_name_of(&header)
        }
        vfs::FileSource::Host(_) => None,
    }
}

/// Candidate DVD paths for a map whose on-disc filename differs from its header.
pub(crate) fn disc_map_paths(vfs: &mut vfs::Vfs, dir: &str) -> Vec<String> {
    vfs.list_directory(dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| {
            let name = entry.name.as_bytes();
            !entry.metadata.is_directory()
                && name.len() > 4
                && name[name.len() - 4..].eq_ignore_ascii_case(b".map")
        })
        .map(|entry| format!("{dir}\\{}", entry.name))
        .collect()
}

/// The map name in a map file's header: 32 bytes at `+0x198`, as
/// `map::Map` reads it; `None` for a file too short or not text.
pub fn header_name(file: &vfs::HostFile) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    const NAME: u64 = 0x198;
    const LEN: usize = 32;
    if file.len < NAME + LEN as u64 {
        return None;
    }
    let mut f = std::fs::File::open(&file.path).ok()?;
    f.seek(SeekFrom::Start(file.offset + NAME)).ok()?;
    let mut b = [0u8; LEN];
    f.read_exact(&mut b).ok()?;
    let end = b.iter().position(|c| *c == 0).unwrap_or(LEN);
    let name = std::str::from_utf8(&b[..end]).ok()?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// [`header_name`] of a map held in memory.
pub fn header_name_of(bytes: &[u8]) -> Option<String> {
    let field = bytes.get(0x198..0x198 + 32)?;
    let end = field.iter().position(|c| *c == 0).unwrap_or(field.len());
    let name = std::str::from_utf8(&field[..end]).ok()?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// Where map packages live: `<data>/maps`.
pub fn packages_dir(data_root: &Path) -> PathBuf {
    data_root.join("maps")
}

/// The DVD slot the running game occupies and will request again. Retail's
/// path builder (0x214db0) preserves T: and W: paths and maps the basename
/// of a scenario path to D:\maps. The cache name can differ from that basename.
pub fn reload_slot(source: &str, scenario: &str) -> Result<String, String> {
    let lower = scenario.to_ascii_lowercase();
    if lower.starts_with(r"t:\") || lower.starts_with(r"w:\") {
        return Err(format!(
            "the active scenario {scenario} loads outside the DVD map mount"
        ));
    }
    let slot = crate::map_slots::map_name(source)
        .ok_or_else(|| format!("the active map source {source} is outside the DVD map mount"))?;
    if !scenario
        .rsplit('\\')
        .next()
        .is_some_and(|s| s.eq_ignore_ascii_case(&slot))
    {
        return Err(format!(
            "the active scenario {scenario} does not load its current DVD slot {source}"
        ));
    }
    Ok(slot)
}

/// Mount storage and the DVD, with any explicit editor override and
/// campaign additions. The data partition serves registered mod paths.
pub fn mount<R: vfs::Rewrite + 'static>(
    cfg: vfs::Config,
    disc: &Path,
    fonts: Option<R>,
    maps: Option<&Path>,
    campaign_maps: &[(String, vfs::MapSource)],
) -> vfs::Result<vfs::Vfs> {
    if maps.is_none()
        && cfg.server_maps.is_none()
        && cfg.sim_store.is_none()
        && cfg.store_maps.is_none()
        && campaign_maps.is_empty()
    {
        return match fonts {
            Some(r) => vfs::Vfs::with_disc_rewrite(cfg, disc, r),
            None => vfs::Vfs::with_disc(cfg, disc),
        };
    }
    // A level's Halo 2-assets map, when there is one, is served while the
    // pregame screen chooses Halo 2's assets (docs/halo1/campaign.md §5).
    // A partial store directory still serves the other immutable maps from DVD.
    let dvd = vfs::open_disc_with_mapping(disc, cfg.mmap_disc)?;
    // Sim stores sit under every other map layer: an editor override or a
    // verified server copy still wins where one is set, and everything
    // they do not serve comes from the store before the disc
    // (crates/runtime/vfs/src/sim_store.rs).
    // Store maps (`store-maps`, docs/engine/asset-store.md) replace the
    // disc's map files under everything else: the disc's are never read
    // (crates/runtime/vfs/src/store_map.rs). A level with packages over it
    // is served from its image (`cfg.store_levels`).
    let levels = cfg.store_levels.clone();
    match (cfg.store_maps.clone(), cfg.sim_store.clone()) {
        (Some(root), Some(dir)) => over_disc(
            cfg,
            vfs::SimStoreBackend::new(
                vfs::StoreMapBackend::new(dvd, root).with_levels(levels),
                dir,
            ),
            fonts,
            maps,
            campaign_maps,
        ),
        (Some(root), None) => over_disc(
            cfg,
            vfs::StoreMapBackend::new(dvd, root).with_levels(levels),
            fonts,
            maps,
            campaign_maps,
        ),
        (None, Some(dir)) => over_disc(
            cfg,
            vfs::SimStoreBackend::new(dvd, dir),
            fonts,
            maps,
            campaign_maps,
        ),
        (None, None) => over_disc(cfg, dvd, fonts, maps, campaign_maps),
    }
}

/// Offer a reload handle on writable hosts. Its first file request attaches
/// the layer to the mounted DVD, preserving its selected overrides and opens.
pub fn mount_for_agent<R: vfs::Rewrite + 'static>(
    cfg: vfs::Config,
    disc: &Path,
    fonts: Option<R>,
    maps: Option<&Path>,
    campaign_maps: &[(String, vfs::MapSource)],
    allow_rebind: bool,
) -> vfs::Result<(vfs::Vfs, Option<vfs::MapRebind>)> {
    let writable = allow_rebind && cfg.server_maps.is_none();
    let vfs = mount(cfg, disc, fonts, maps, campaign_maps)?;
    Ok((vfs, writable.then(vfs::MapRebind::default)))
}

/// The disc's own map layers over `dvd`: campaign additions, verified
/// server copies, explicit editor overrides, then the fonts rewrite.
fn over_disc<B: vfs::Backend + 'static, R: vfs::Rewrite + 'static>(
    cfg: vfs::Config,
    dvd: B,
    fonts: Option<R>,
    maps: Option<&Path>,
    campaign_maps: &[(String, vfs::MapSource)],
) -> vfs::Result<vfs::Vfs> {
    let disc = vfs::CampaignMapsBackend::new(dvd, campaign_maps.iter().cloned()).with_assets(
        crate::h1_variants::campaign_choice(),
        crate::campaigns::halo2_maps(&cfg.data_root),
    );
    if !campaign_maps.is_empty() {
        eprintln!(
            "campaigns: {} map(s) served from the tag store",
            campaign_maps.len()
        );
    }
    if let Some(server_maps) = cfg.server_maps.clone() {
        let disc = vfs::MapOverrideBackend::server_maps(disc, server_maps);
        return match maps {
            Some(maps) => finish(cfg, vfs::MapOverrideBackend::new(disc, maps), fonts),
            None => finish(cfg, disc, fonts),
        };
    }
    match maps {
        Some(maps) => {
            eprintln!(
                "map override: maps from {} replace the disc's",
                maps.display()
            );
            finish(cfg, vfs::MapOverrideBackend::new(disc, maps), fonts)
        }
        None => finish(cfg, disc, fonts),
    }
}

fn finish<B: vfs::Backend + 'static, R: vfs::Rewrite + 'static>(
    cfg: vfs::Config,
    disc: B,
    fonts: Option<R>,
) -> vfs::Result<vfs::Vfs> {
    match fonts {
        // Fonts live under `fonts\`, maps under `maps\`: the layers never meet.
        Some(r) => vfs::Vfs::with_disc_backend(cfg, vfs::RewriteBackend::new(disc, r)),
        None => vfs::Vfs::with_disc_backend(cfg, disc),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(crate) struct Temp(pub(crate) PathBuf);
    impl Temp {
        pub(crate) fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let base = std::env::var_os("TMPDIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let p = base.join(format!(
                "h2-boot-map-override-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(p.join("maps")).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dirent(right: u16, sector: u32, size: u32, directory: bool, name: &str) -> Vec<u8> {
        let mut b = vec![0; 14];
        b[2..4].copy_from_slice(&right.to_le_bytes());
        b[4..8].copy_from_slice(&sector.to_le_bytes());
        b[8..12].copy_from_slice(&size.to_le_bytes());
        b[12] = if directory { 0x10 } else { 0 };
        b[13] = name.len() as u8;
        b.extend_from_slice(name.as_bytes());
        b.resize(b.len().next_multiple_of(4), 0);
        b
    }
    /// An XISO with `fonts\font` ("glyphs") and `maps\level.map` ("disc map").
    fn iso(t: &Temp) -> PathBuf {
        iso_at(&t.0, b"disc map")
    }
    /// That XISO in `dir`, with `level` as `maps\level.map` (at most one
    /// sector).
    pub fn iso_at(dir: &Path, level: &[u8]) -> PathBuf {
        let mut image = vec![0; 40 * 2048];
        image[32 * 2048..][..20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
        image[32 * 2048 + 20..][..4].copy_from_slice(&33u32.to_le_bytes());
        let first = dirent(0, 34, 2048, true, "fonts");
        let root = [
            dirent((first.len() / 4) as u16, 34, 2048, true, "fonts"),
            dirent(0, 35, 2048, true, "maps"),
        ]
        .concat();
        image[32 * 2048 + 24..][..4].copy_from_slice(&(root.len() as u32).to_le_bytes());
        image[33 * 2048..][..root.len()].copy_from_slice(&root);
        let fonts = dirent(0, 36, 6, false, "font");
        image[34 * 2048..][..fonts.len()].copy_from_slice(&fonts);
        let maps = dirent(0, 37, level.len() as u32, false, "level.map");
        image[35 * 2048..][..maps.len()].copy_from_slice(&maps);
        image[36 * 2048..][..6].copy_from_slice(b"glyphs");
        image[37 * 2048..][..level.len()].copy_from_slice(level);
        let path = dir.join("disc.iso");
        std::fs::write(&path, image).unwrap();
        path
    }
    /// Upper-cases every file under `fonts\`.
    pub struct Upper;
    impl vfs::Rewrite for Upper {
        fn wants(&self, path: &str) -> bool {
            path.to_ascii_lowercase().starts_with("fonts\\")
        }
        fn rewrite(&mut self, _: &str, original: &[u8]) -> Option<Vec<u8>> {
            Some(original.to_ascii_uppercase())
        }
    }
    fn read(v: &mut vfs::Vfs, name: &str) -> Vec<u8> {
        let h = v.open(name, vfs::OpenOptions::default()).unwrap().handle;
        let mut buf = vec![0; v.query_information(h).unwrap().metadata.end_of_file as usize];
        assert_eq!(
            v.read(h, vfs::Offset::Absolute(0), &mut buf).unwrap(),
            buf.len()
        );
        v.close(h).unwrap();
        buf
    }
    fn mounted(t: &Temp, fonts: bool, maps: Option<&Path>) -> vfs::Vfs {
        mounted_with(t, fonts, maps, None)
    }
    fn mounted_with(
        t: &Temp,
        fonts: bool,
        maps: Option<&Path>,
        _packages: Option<&Path>,
    ) -> vfs::Vfs {
        let cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        let mut v = mount(cfg, &iso(t), fonts.then_some(Upper), maps, &[]).unwrap();
        v.namespace
            .create_symbolic_link("\\??\\D:", vfs::CDROM_DEVICE)
            .unwrap();
        v
    }

    #[test]
    fn unset_or_empty_env_means_no_override() {
        assert_eq!(dir_from(None), None);
        assert_eq!(dir_from(Some(OsString::new())), None);
        assert_eq!(
            dir_from(Some("/x/maps".into())),
            Some(PathBuf::from("/x/maps"))
        );
    }

    #[test]
    fn a_sim_store_mounts_without_other_map_overrides() {
        let t = Temp::new();
        let disc = iso(&t);
        let stores = t.0.join("stores");
        vfs::sim_store::build(&disc, "level", &[(0, 4)], &stores.join("level.h2store")).unwrap();
        let mut cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        cfg.mmap_disc = true;
        cfg.sim_store = Some(stores);
        let mut v = mount(cfg, &disc, None::<Upper>, None, &[]).unwrap();
        let path = r"\Device\CdRom0\maps\level.map";
        assert!(v.host_file(path).is_none());
        let h = v.open(path, vfs::OpenOptions::default()).unwrap().handle;
        let mut bytes = [0; 4];
        assert_eq!(v.read(h, vfs::Offset::Absolute(0), &mut bytes), Ok(4));
        assert_eq!(&bytes, b"disc");
        assert_eq!(v.read(h, vfs::Offset::Current, &mut bytes), Err(vfs::Error::NotInStore));
        v.close(h).unwrap();
    }

    #[test]
    fn without_a_directory_the_disc_and_fonts_are_served_as_before() {
        let t = Temp::new();
        std::fs::write(t.0.join("maps").join("level.map"), b"edited!!!").unwrap();
        let mut v = mounted(&t, true, None);
        assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"disc map");
        assert_eq!(read(&mut v, "D:\\fonts\\font"), b"GLYPHS");
        let mut v = mounted(&t, false, None);
        assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"disc map");
        assert_eq!(read(&mut v, "D:\\fonts\\font"), b"glyphs");
    }

    #[test]
    fn with_a_directory_maps_are_overridden_and_fonts_still_rewritten() {
        let t = Temp::new();
        let dir = t.0.join("maps");
        std::fs::write(dir.join("LEVEL.MAP"), b"edited!!!").unwrap();
        for fonts in [true, false] {
            let mut v = mounted(&t, fonts, Some(&dir));
            assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"edited!!!");
            assert_eq!(
                v.host_file("\\Device\\CdRom0\\maps\\level.map")
                    .unwrap()
                    .len,
                9
            );
            let font: &[u8] = if fonts { b"GLYPHS" } else { b"glyphs" };
            assert_eq!(read(&mut v, "D:\\fonts\\font"), font);
        }
    }

    /// A synthetic scenario and globals tag in a map whose header names it `name`.
    pub(crate) fn named_map(name: &str) -> Vec<u8> {
        use map::build::{Block, TagPayload, TagSet};
        let mut set = TagSet::new(map::CacheType::Multiplayer, name, "test");
        let globals = map::FourCC(*b"matg");
        let scenario = map::FourCC(*b"scnr");
        set.add_group(globals, None, None);
        set.add_group(scenario, None, None);
        set.globals = set.add_tag(globals, "globals", TagPayload::Built(Block::zeroed(4))).unwrap();
        set.scenario = set.add_tag(scenario, "test", TagPayload::Built(Block::zeroed(0x218))).unwrap();
        set.build().unwrap().0
    }

    #[test]
    fn names_fall_back_to_the_loaded_header_when_the_slot_is_unavailable() {
        let temp = Temp::new();
        let mut vfs = mounted(&temp, false, None);
        let from_name = map_file(&mut vfs, "level").unwrap();
        for slot in [None, Some(r"W:\missing.map")] {
            assert_eq!(map_file_at(&mut vfs, "level", slot).unwrap(), from_name);
        }
        let mut guest = runtime::Guest::new();
        let name = crate::health::CACHE_MAP_NAME;
        guest.map_zeroed(name & !0xfff, 0x1000, runtime::guest::Permissions::RW).unwrap();
        guest.write_bytes(name, b"level\0", 1).unwrap();
        assert_eq!(active_map_file(&mut vfs, &guest, None).unwrap(), from_name);
    }

    /// Replaces every map with a named synthetic one, as the pogo kit does.
    struct Maps;
    impl vfs::Rewrite for Maps {
        fn wants(&self, path: &str) -> bool {
            path.to_ascii_lowercase().starts_with("maps\\")
        }
        fn rewrite(&mut self, _: &str, _: &[u8]) -> Option<Vec<u8>> {
            Some(named_map("level"))
        }
    }

    /// Serves every map as an overlay over the disc's: its last byte
    /// written as `!`, the rest read from the disc by offset.
    struct Spliced;
    impl vfs::Rewrite for Spliced {
        fn wants(&self, path: &str) -> bool {
            path.to_ascii_lowercase().starts_with("maps\\")
        }
        fn rewrite(&mut self, _: &str, _: &[u8]) -> Option<Vec<u8>> {
            unreachable!("the VFS asks for the streamed rewrite")
        }
        fn rewrite_streamed(
            &mut self,
            _: &str,
            source: &mut dyn content::patch::Source,
        ) -> std::io::Result<Option<vfs::Rewritten>> {
            let mut o = content::patch::Overlay::new(source.size());
            o.write(source.size() - 1, b"!");
            Ok(Some(vfs::Rewritten::Over(o)))
        }
    }

    #[test]
    fn a_map_the_vfs_splices_is_read_over_the_disc_image() {
        let t = Temp::new();
        let cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        let mut v = mount(cfg, &iso(&t), Some(Spliced), None, &[]).unwrap();
        assert!(map_file(&mut v, "level").is_none());
        let Some(vfs::FileSource::Spliced(file)) = map_source(&mut v, "level", None) else {
            panic!("no spliced map")
        };
        // The base is the entry inside the image, not the image's start.
        assert!(file.base.offset > 0);
        assert_eq!(file.read_all().unwrap(), b"disc ma!");
        let mut r = file.reader().unwrap();
        let mut tail = [0; 3];
        std::io::Seek::seek(&mut r, std::io::SeekFrom::End(-3)).unwrap();
        std::io::Read::read_exact(&mut r, &mut tail).unwrap();
        assert_eq!(&tail, b"ma!");
    }

    #[test]
    fn a_map_the_vfs_rewrites_names_its_tags_from_memory() {
        let t = Temp::new();
        let cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        let mut v = mount(cfg, &iso(&t), Some(Maps), None, &[]).unwrap();
        // The rewrite leaves no contiguous host file, so nothing locates one.
        assert!(map_file(&mut v, "level").is_none());
        let Some(vfs::FileSource::Memory(bytes)) = map_source(&mut v, "level", None) else {
            panic!("no in-memory map")
        };
        assert_eq!(header_name_of(&bytes).as_deref(), Some("level"));
        // By a slot of its own, too.
        assert!(matches!(
            map_source(&mut v, "other", Some(r"\Device\CdRom0\maps\level.map")),
            Some(vfs::FileSource::Memory(_))
        ));
        let file = crate::telemetry::MapFile::memory("level", bytes);
        let names = crate::telemetry::TagNames::load(&file).unwrap();
        assert!(names.is_for(map::Map::from_reader(std::io::Cursor::new(named_map("level"))).unwrap().scenario().unwrap().id.0));
    }

    #[test]
    fn an_agent_mount_reloads_a_dvd_slot_without_an_override_directory() {
        use crate::map_slots::{CURRENT, HEADER, PATH, RECORDS, STRIDE, active_file};
        use runtime::guest::Permissions;

        let t = Temp::new();
        let cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        let offers = cfg.content_maps.clone();
        let (mut v, handle) = mount_for_agent(cfg, &iso(&t), Some(Upper), None, &[], true).unwrap();
        let slot = reload_slot(r"D:\maps\LEVEL.map", r"scenarios\test\level").unwrap();
        let path = r"\Device\CdRom0\maps\level.map";
        assert_eq!(read(&mut v, path), b"disc map");
        let package = content::Library::open(level_package(&t)).packages()[0].clone();
        let package_id = package.id;
        offers.replace([(
            r"TDATA\4d530064\$c\0000000000000001\other_name.map".into(),
            vfs::ContentMap {
                package,
                base: v.host_file(path),
                alias: Some("other_name".into()),
            },
        )]);
        let replacement = named_map("other_name");
        let file = t.0.join("other_name.map");
        std::fs::write(&file, &replacement).unwrap();
        let handle = handle.unwrap();
        handle.attach(&mut v).unwrap();
        handle.prepare(&slot, "other_name", &file, map::CacheType::Multiplayer, 0x0500_0000)
            .unwrap()
            .reserve(&v)
            .unwrap()
            .commit();
        assert_eq!(read(&mut v, path), replacement);
        assert_eq!(map_id(&mut v, "other_name"), Some(package_id));

        let mut guest = runtime::Guest::new();
        let key = RECORDS + 3 * STRIDE + HEADER + PATH;
        guest.map_zeroed(key & !0xfff, 0x1000, Permissions::RW).unwrap();
        guest.map_zeroed(CURRENT & !0xfff, 0x1000, Permissions::RW).unwrap();
        guest.set_u32(CURRENT, 3).unwrap();
        guest.write_bytes(key, br"d:\maps\level.map", 1).unwrap();
        let source = active_file(&guest).unwrap();
        let actual_id = v.map_id(path).unwrap();
        assert_ne!(actual_id, package_id);
        let name = crate::health::CACHE_MAP_NAME;
        guest.map_zeroed(name & !0xfff, 0x1000, Permissions::RW).unwrap();
        guest.write_bytes(name, b"other_name\0", 1).unwrap();
        // The unadopted catalog lookup and the bound host select different maps.
        assert_eq!(active_map_id(&mut v, &guest, None).unwrap().0, package_id);
        assert_eq!(active_map_id(&mut v, &guest, Some(&handle)), Some((actual_id, None)));
        let host = v.host_file(&source).unwrap();
        assert_eq!(host, v.host_file(path).unwrap());
        assert_eq!(std::fs::read(host.path).unwrap(), replacement);
    }

    #[test]
    fn same_header_replacement_refreshes_consumers_only_on_activation() {
        use crate::map_slots::{CURRENT, HEADER, PATH, RECORDS, STRIDE};
        use std::sync::{Arc, Mutex};

        let t = Temp::new();
        let cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        let (vfs, handle) = mount_for_agent(cfg, &iso(&t), None::<Upper>, None, &[], true).unwrap();
        let handle = handle.unwrap();
        let mount = Arc::new(Mutex::new(vfs));
        handle.attach(&mut mount.lock().unwrap()).unwrap();
        let sources = crate::loaded_map::EditorSources::new(&mount, handle.clone());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::clone(&seen);
        sources.on_change(move |f| events.lock().unwrap().push(f.map(|f| f.path.clone())));

        let mut guest = guest_layout::testing::FakeGuest::new();
        let key = RECORDS + 3 * STRIDE + HEADER + PATH;
        guest.map(key, 0x100);
        guest.map(CURRENT, 4);
        guest.put(CURRENT, &3u32.to_le_bytes());
        guest.put(key, br"d:\maps\level.map");
        sources.activated(&guest);
        assert!(seen.lock().unwrap().is_empty());
        let file = t.0.join("level.map");
        std::fs::write(&file, named_map("level")).unwrap();
        let publish = || {
            handle.prepare("level", "level", &file, map::CacheType::Multiplayer, 0x0500_0000)
                .unwrap().reserve(&mount.lock().unwrap()).unwrap().commit();
        };
        publish();
        assert!(seen.lock().unwrap().is_empty());
        sources.activated(&guest);
        sources.activated(&guest);
        publish();
        assert_eq!(seen.lock().unwrap().len(), 1);
        sources.activated(&guest);
        let events = seen.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events[0].is_some() && events[0] != events[1]);
    }

    #[test]
    fn the_telemetry_worker_keeps_its_map_until_it_has_read_the_names() {
        let t = Temp::new();
        let cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        let (mut vfs, handle) = mount_for_agent(cfg, &iso(&t), None::<Upper>, None, &[], true).unwrap();
        let handle = handle.unwrap();
        handle.attach(&mut vfs).unwrap();
        let file = t.0.join("fixture.map");
        std::fs::write(&file, named_map("fixture")).unwrap();
        handle.prepare("level", "fixture", &file, map::CacheType::Multiplayer, 0x0500_0000)
            .unwrap().reserve(&vfs).unwrap().commit();

        let region = telemetry_file(vfs.host_file(r"\Device\CdRom0\maps\level.map").unwrap());
        let path = region.path.clone();
        handle.clear_on_shutdown();
        crate::telemetry::TagNames::load(&region).unwrap();
        drop(region);
        assert!(!path.exists());
    }

    #[test]
    fn the_speaker_index_releases_a_displaced_map_generation() {
        let t = Temp::new();
        let cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        let (mut vfs, handle) = mount_for_agent(cfg, &iso(&t), None::<Upper>, None, &[], true).unwrap();
        let handle = handle.unwrap();
        handle.attach(&mut vfs).unwrap();
        let file = t.0.join("fixture.map");
        std::fs::write(&file, named_map("fixture")).unwrap();
        let publish = |vfs: &vfs::Vfs| {
            handle.prepare("level", "fixture", &file, map::CacheType::Multiplayer, 0x0500_0000)
                .unwrap().reserve(vfs).unwrap().commit();
        };
        publish(&vfs);
        let source = vfs.host_file(r"\Device\CdRom0\maps\level.map").unwrap();
        let mut index = crate::pad_speaker::PadIndexer::new(&Default::default(), [None, None, None]);
        // This map has no sound gestalt, so the indexer records its region once.
        assert!(index.add(&source).is_err());
        assert!(index.add(&source).unwrap().is_none());
        let path = source.path.clone();
        drop(source);
        publish(&vfs);
        assert!(!path.exists(), "the finished index still owns the displaced file");
    }

    #[test]
    fn a_server_map_mount_has_no_rebind_handle() {
        let t = Temp::new();
        let mut cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        cfg.server_maps = Some(t.0.join("maps"));
        let (_, handle) = mount_for_agent(cfg, &iso(&t), None::<Upper>, None, &[], true).unwrap();
        assert!(handle.is_none());
    }

    #[test]
    fn a_content_or_different_scenario_cannot_rebind_a_dvd_slot() {
        let dvd = r"d:\maps\level.map";
        assert!(reload_slot(dvd, r"T:\$c\0123456789abcdef\level").is_err());
        assert!(reload_slot(dvd, r"W:\level").is_err());
        assert!(reload_slot(dvd, r"scenarios\multi\other").is_err());
        assert!(reload_slot(r"T:\$c\0123456789abcdef\level.map", "level").is_err());
    }

    #[test]
    fn a_map_is_found_by_its_file_name_or_by_the_name_in_its_header() {
        let t = Temp::new();
        let dir = t.0.join("maps");
        // A converted map served as the disc's level.map, named otherwise.
        let bytes = named_map("h1_bloodgulch");
        std::fs::write(dir.join("level.map"), &bytes).unwrap();
        let mut v = mounted(&t, false, Some(&dir));
        let by_header = map_file(&mut v, "h1_bloodgulch").unwrap();
        assert_eq!(by_header.path, dir.canonicalize().unwrap().join("level.map"));
        assert!(by_header.generation.is_none());
        assert_eq!(std::fs::read(&by_header.path).unwrap(), bytes);
        assert_eq!((by_header.offset, by_header.len), (0, bytes.len() as u64));
        assert_eq!(header_name(&by_header).as_deref(), Some("h1_bloodgulch"));
        // By its file name, as before.
        assert_eq!(map_file(&mut v, "level"), Some(by_header));
        assert_eq!(map_file(&mut v, "h1_sidewinder"), None);
        // Without the override the disc's level.map ("disc map", no
        // header) names nothing.
        let mut v = mounted(&t, false, None);
        assert!(map_file(&mut v, "level").is_some());
        assert_eq!(map_file(&mut v, "h1_bloodgulch"), None);
    }

    #[test]
    fn a_missing_directory_boots_from_the_disc() {
        let t = Temp::new();
        let mut v = mounted(&t, false, Some(&t.0.join("absent")));
        assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"disc map");
    }

    /// A package in `<data>/maps` for `level`: an IPS patch over the disc's
    /// map ("disc map" becomes "DISC map").
    fn level_package(t: &Temp) -> PathBuf {
        let root = packages_dir(&t.0.join("data"));
        let dir = root.join("level-mod");
        std::fs::create_dir_all(&dir).unwrap();
        let patch = b"PATCH\0\0\0\0\x04DISCEOF";
        std::fs::write(dir.join("a.ips"), patch).unwrap();
        let mut m = content::Manifest::retail("level", content::Sha256::of(b"disc map"));
        m.patches.push(content::manifest::Layer {
            file: "a.ips".into(),
            sha256: content::Sha256::of(patch),
        });
        std::fs::write(dir.join("manifest.toml"), m.to_toml()).unwrap();
        root
    }

    /// Everything the guest sees of the disc, for comparing mounts.
    fn observe(v: &mut vfs::Vfs) -> Vec<String> {
        let mut seen = Vec::new();
        for name in ["D:\\maps\\level.map", "D:\\fonts\\font"] {
            seen.push(format!("{:?}", v.query_path(name)));
            seen.push(format!("{:?}", read(v, name)));
            seen.push(format!("{:?}", v.host_file(name)));
        }
        seen
    }

    #[test]
    fn without_packages_the_mount_is_unchanged() {
        let t = Temp::new();
        for fonts in [true, false] {
            let expected = observe(&mut mounted(&t, fonts, None));
            let root = packages_dir(&t.0.join("data"));
            // Missing, empty, holding only a cache or a broken package.
            assert_eq!(
                observe(&mut mounted_with(&t, fonts, None, Some(&root))),
                expected
            );
            std::fs::create_dir_all(root.join("cache")).unwrap();
            std::fs::create_dir_all(root.join("broken")).unwrap();
            std::fs::write(root.join("broken").join("manifest.toml"), "x").unwrap();
            assert_eq!(
                observe(&mut mounted_with(&t, fonts, None, Some(&root))),
                expected
            );
            std::fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn legacy_packages_do_not_shadow_disc_or_fonts() {
        let t = Temp::new();
        let root = level_package(&t);
        for fonts in [true, false] {
            let mut v = mounted_with(&t, fonts, None, Some(&root));
            assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"disc map");
            let font: &[u8] = if fonts { b"GLYPHS" } else { b"glyphs" };
            assert_eq!(read(&mut v, "D:\\fonts\\font"), font);
        }
    }

    #[test]
    fn retail_has_no_package_id_even_with_legacy_packages() {
        let t = Temp::new();
        let root = level_package(&t);
        let mut v = mounted_with(&t, false, None, Some(&root));
        assert_eq!(map_id(&mut v, "level"), None);
        let dir = t.0.join("maps");
        std::fs::write(dir.join("level.map"), b"edited!!!").unwrap();
        let mut v = mounted_with(&t, true, Some(&dir), Some(&root));
        let override_id = map_id(&mut v, "level").unwrap();
        // An editor publishes a replacement. The current mount holds its
        // original file open and caches its identity on first use.
        std::fs::write(dir.join("next.map"), b"edited again").unwrap();
        std::fs::rename(dir.join("next.map"), dir.join("level.map")).unwrap();
        assert_eq!(map_id(&mut v, "level"), Some(override_id));
        assert_eq!(read(&mut v, r"D:\maps\level.map"), b"edited!!!");
        drop(v);
        let mut v = mounted_with(&t, true, Some(&dir), Some(&root));
        assert_eq!(read(&mut v, r"D:\maps\level.map"), b"edited again");
        assert_ne!(map_id(&mut v, "level").unwrap(), override_id);
    }
}
