//! Hot iteration on the agent API (docs/graphics/hot-iteration.md):
//! `edit.*` (live tag edits through the runtime's memory) and `render.*`
//! (the host renderer's WGSL overrides, parameters and toggles, on
//! `hot_render::global()`, which `d3d::nv2a` reads once a frame).
//!
//! Overrides come from `$H2_RENDER_OVERRIDES`, else the cache directory's
//! `render-overrides`, watched for changes. Tag names come from the loaded
//! map's file, found through the VFS as telemetry finds it.
//!
//! Live raw edits (docs/editor/realtime-edits.md) read the map files
//! through the VFS and write [`vfs::RawOverlay::global`]; [`RawGlue`]
//! aliases each cache slot file the guest reads raw data from to the map
//! it holds, and every applied raw act queues its native refresh on
//! `render_bridge::native_edits`.
//!
//! Grown tag blocks go to the edit arena (`hot_tags::arena`, in the HLE
//! reservation, pages mapped through the runtime's memory as it grows).
//! [`install`] hands back its [`hot_tags::arena::Link`] so the boot can
//! give it to `world.placement.edit`'s queue, which grows placement blocks
//! into the same arena.

use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use map::ResourceFile;

/// Finds a map's host file by its active slot's VFS device path.
pub type MapLocator = Box<dyn Fn(&runtime::Guest) -> Option<vfs::HostFile> + Send + Sync>;

/// Adds the hot methods to `agent` and, with `watch`, loads and watches
/// the override directory. Without (the debug menu's local agent, no
/// `H2_AGENT`) the directory is only named: nothing loads until
/// `render.reload` (the menu's Reload row) asks. Returns the edit arena's
/// link for `world.placement.edit` (`agent::sim::calls::Queue::set_arena`).
pub fn install(
    agent: &agent::Agent,
    rt: &Arc<runtime::Runtime>,
    maps: Option<MapLocator>,
    vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    watch: bool,
    replaying: bool,
) -> hot_tags::arena::Link {
    #[cfg(feature = "gpu")]
    install_overrides(agent, watch);
    #[cfg(not(feature = "gpu"))]
    let _ = watch;
    install_edits(agent, rt, maps, vfs, replaying)
}

#[cfg(feature = "gpu")]
fn install_overrides(agent: &agent::Agent, watch: bool) {
    let hub = hot_render::global();
    agent::hot::add_render_methods(agent, hub);
    if !watch {
        hub.set_dir(Some(agent_proto::hot::default_override_dir()));
        return;
    }
    let r = agent::hot::watch_overrides(hub, agent_proto::hot::default_override_dir());
    eprintln!(
        "render overrides: {} ({} loaded{}; docs/graphics/hot-iteration.md)",
        r.dir.as_deref().unwrap_or("-"),
        r.loaded.len(),
        if r.problems.is_empty() {
            String::new()
        } else {
            format!(
                ", {} not taken: render.overrides says why",
                r.problems.len()
            )
        }
    );
}

/// `edit.*` over the runtime's memory; the edit arena's link.
fn install_edits(
    agent: &agent::Agent,
    rt: &Arc<runtime::Runtime>,
    maps: Option<MapLocator>,
    vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    replaying: bool,
) -> hot_tags::arena::Link {
    let arena = hot_tags::arena::shared();
    let mut editor = hot_tags::Editor::new().with_arena(arena.clone());
    let maps = maps.map(Arc::new);
    if let Some(locate) = maps.clone() {
        editor = editor.with_names(Box::new(move |g| names(g, &locate)));
    }
    editor = editor.with_raw(
        Box::new(RawGlue { vfs, overlay: vfs::RawOverlay::global().clone() }),
        Box::new(|r| render_bridge::native_edits::push(native_edit(r))),
    );
    let pages_rt = Arc::clone(rt);
    let rt = Arc::clone(rt);
    agent::hot::add_edit_methods(
        agent,
        editor,
        Arc::new(move |f: &mut dyn FnMut(&mut runtime::Guest)| f(&mut rt.memory())),
        |g| crate::agent_glue::session_view(g),
        move |g| {
            Some(maps.as_ref()?(g)?.path.display().to_string())
        },
        replaying,
    );
    hot_tags::arena::Link {
        arena,
        pages: Arc::new(move |f: &mut dyn FnMut(&mut dyn hot_tags::arena::Pages)| {
            let mut memory = pages_rt.memory();
            let guest: &mut runtime::Guest = &mut memory;
            f(guest)
        }),
    }
}

/// Tag lookup for `sim.give` and `sim.spawn`: the loaded map's tags by
/// datum or by name (names from the map's file), as `edit.*` finds them.
pub fn map_tags(
    rt: &Arc<runtime::Runtime>,
    maps: Option<MapLocator>,
) -> agent::sim::calls::MapTags {
    let mut editor = hot_tags::Editor::new();
    if let Some(locate) = maps {
        editor = editor.with_names(Box::new(move |g| names(g, &locate)));
    }
    let rt = Arc::clone(rt);
    agent::sim::calls::MapTags {
        editor,
        guest: Arc::new(move |f: &mut dyn FnMut(&mut runtime::Guest)| f(&mut rt.memory())),
    }
}

/// The names of the map loaded in `g`, from its file.
fn names(g: &runtime::Guest, locate: &MapLocator) -> Option<hot_tags::Names> {
    let file = locate(g)?;
    let reader = std::fs::File::open(&file.path).ok()?;
    let map = map::Map::from_region(std::io::BufReader::new(reader), file.offset, file.len).ok()?;
    hot_tags::Names::from_map(&map)
}

fn native_edit(r: hot_tags::Refresh) -> render_bridge::native_edits::Edit {
    use render_bridge::native_edits::Edit;
    match r {
        hot_tags::Refresh::Section { bsp, kind, index, grown } => Edit::Section { bsp, kind, index, grown },
        hot_tags::Refresh::Model { tag } => Edit::Model { tag },
        hot_tags::Refresh::Image { bitmap, image } => Edit::Image { bitmap, image },
    }
}

/// The raw overlay's name for the file a raw pointer selects: the loaded
/// map's cache header name for [`ResourceFile::Local`] (what
/// `native_map::open` is called with), the shared maps' stems otherwise.
pub fn overlay_name(file: ResourceFile, map: &str) -> String {
    match file {
        ResourceFile::Local => map.to_ascii_lowercase(),
        ResourceFile::MainMenu => "mainmenu".into(),
        ResourceFile::Shared => "shared".into(),
        ResourceFile::SinglePlayerShared => "single_player_shared".into(),
    }
}

/// The cache slot holding each selector's file (docs/runtime/cache-slots.md,
/// "The slot files"): shared 0, single-player shared 1, main menu 2, the
/// loaded map the current slot.
fn slot_of(g: &runtime::Guest, file: ResourceFile) -> Result<u32, String> {
    Ok(match file {
        ResourceFile::Shared => 0,
        ResourceFile::SinglePlayerShared => 1,
        ResourceFile::MainMenu => 2,
        ResourceFile::Local => g
            .u32(crate::map_slots::CURRENT)
            .ok()
            .filter(|s| (2..=5).contains(s))
            .ok_or("the loaded map's cache slot is unknown")?,
    })
}

/// The guest's object name for slot `slot`'s file: `cache%03d.map` on Z:
/// for slots 0 and 1, on N: for 2 to 5, through the drive links.
fn slot_object(vfs: &vfs::Vfs, slot: u32) -> Result<String, String> {
    let drive = if slot < 2 { 'Z' } else { 'N' };
    vfs.namespace
        .resolve(&format!(r"\??\{drive}:\cache{slot:03}.map"))
        .map_err(|e| format!("cache slot {slot}: {e:?}"))
}

/// The VFS object the guest copied a selector's file from.
fn source_of(g: &runtime::Guest, file: ResourceFile) -> Result<String, String> {
    if file == ResourceFile::Local {
        return crate::map_slots::active_file(g).ok_or_else(|| "the loaded map's file is unknown".into());
    }
    crate::pad_speaker::SHARED_MAPS
        .iter()
        .find(|(f, _)| *f == file)
        .map(|(_, path)| (*path).to_owned())
        .ok_or_else(|| "no shared map for that selector".into())
}

fn loaded_name(g: &runtime::Guest) -> Result<String, String> {
    let mut b = [0u8; 32];
    g.read_bytes(crate::health::CACHE_MAP_NAME, &mut b, 1)
        .map_err(|_| "the loaded map's name is unreadable")?;
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    let name = String::from_utf8_lossy(&b[..end]).into_owned();
    if name.is_empty() {
        return Err("no map is loaded".into());
    }
    Ok(name)
}

/// `hot_tags::RawFiles` over the VFS and the process's raw overlay.
struct RawGlue {
    vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    overlay: vfs::RawOverlay,
}

impl RawGlue {
    fn vfs(&self) -> Result<std::sync::MutexGuard<'_, vfs::Vfs>, String> {
        let vfs = self.vfs.as_ref().ok_or("no VFS is mounted")?;
        Ok(vfs.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn name(&self, g: &runtime::Guest, file: ResourceFile) -> String {
        overlay_name(file, &loaded_name(g).unwrap_or_default())
    }

    /// The selector's own file: its bytes source and length.
    fn source(&self, g: &runtime::Guest, file: ResourceFile) -> Result<(vfs::FileSource, u64), String> {
        let path = source_of(g, file)?;
        let source = {
            let mut vfs = self.vfs()?;
            vfs.file_source(&path)
                .or_else(|| vfs.host_file(&path).map(vfs::FileSource::Host))
        }
        .ok_or_else(|| format!("{path}: no readable source"))?;
        let len = match &source {
            vfs::FileSource::Host(h) => h.len,
            vfs::FileSource::Memory(b) => b.len() as u64,
        };
        Ok((source, len))
    }
}

impl hot_tags::RawFiles for RawGlue {
    fn ready(&mut self, g: &runtime::Guest) -> Result<(), String> {
        let map = loaded_name(g)?;
        let vfs = self.vfs()?;
        for file in [
            ResourceFile::Local,
            ResourceFile::MainMenu,
            ResourceFile::Shared,
            ResourceFile::SinglePlayerShared,
        ] {
            let object = slot_of(g, file).and_then(|slot| slot_object(&vfs, slot));
            match object {
                Ok(object) => self.overlay.alias(&object, &overlay_name(file, &map)),
                // The guest pages the loaded map in from its slot; without
                // it a live raw write would be lost at the next page-in.
                Err(why) if file == ResourceFile::Local => return Err(why),
                Err(_) => {}
            }
        }
        Ok(())
    }

    fn read(&mut self, g: &runtime::Guest, file: ResourceFile, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let (source, own) = self.source(g, file)?;
        let mut buf = vec![0u8; len];
        let inside = own.saturating_sub(offset).min(len as u64) as usize;
        match source {
            vfs::FileSource::Host(h) => {
                let mut f = std::fs::File::open(&h.path).map_err(|e| e.to_string())?;
                f.seek(SeekFrom::Start(h.offset + offset)).map_err(|e| e.to_string())?;
                f.read_exact(&mut buf[..inside]).map_err(|e| e.to_string())?;
            }
            vfs::FileSource::Memory(b) => {
                let at = offset as usize;
                buf[..inside].copy_from_slice(&b[at..at + inside]);
            }
        }
        let name = self.name(g, file);
        let served = self.overlay.apply(&name, offset, &mut buf, inside);
        if served < len {
            return Err(format!("{name}: {len} bytes at {offset:#x} run past the file"));
        }
        Ok(buf)
    }

    fn held(&mut self, g: &runtime::Guest, file: ResourceFile, offset: u64, len: u64) -> Vec<(u64, Vec<u8>)> {
        self.overlay.held(&self.name(g, file), offset, len)
    }

    fn put(&mut self, g: &runtime::Guest, file: ResourceFile, offset: u64, bytes: &[u8]) {
        self.overlay.put(&self.name(g, file), offset, bytes);
    }

    fn restore(&mut self, g: &runtime::Guest, file: ResourceFile, offset: u64, len: u64, runs: &[(u64, Vec<u8>)]) {
        self.overlay.restore(&self.name(g, file), offset, len, runs);
    }

    fn append(&mut self, g: &runtime::Guest, file: ResourceFile, bytes: &[u8], align: u64) -> Result<u64, String> {
        let (_, own) = self.source(g, file)?;
        Ok(self.overlay.append(&self.name(g, file), own, bytes, align))
    }

    fn drop_appended(&mut self, g: &runtime::Guest, file: ResourceFile, offset: u64) -> bool {
        self.overlay.drop_appended(&self.name(g, file), offset)
    }

    fn clear(&mut self) {
        self.overlay.clear();
    }

    fn summary(&self) -> serde_json::Value {
        let s = self.overlay.summary();
        serde_json::json!({"entries": s.entries, "bytes": s.bytes, "appended": s.appended,
            "appended_bytes": s.appended_bytes, "files": s.files})
    }
}
