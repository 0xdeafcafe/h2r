//! Hot iteration on the agent API (docs/graphics/hot-iteration.md):
//! `edit.*` (live tag edits through the runtime's memory) and `render.*`
//! (the host renderer's WGSL overrides, parameters and toggles, on
//! `hot_render::global()`, which the renderer reads once a frame).
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
//! Live collision commits (`edit.collision.*`) get the game's BSP
//! activation ([`install_collision_activation`]) and make the bots'
//! walls stale ([`install`]).
//! Every applied tag write and revert sends a notice per changed tag
//! (`hot_tags::TagWrite`) to the host copies of tag bytes ([`tag_written`]):
//! the native renderer's queue (`native_edits::Edit::Tag`), the animation
//! clip store, the HUD kit, aim assist and the bots' data. Before them,
//! [`MetaGlue`] copies the written bytes into the raw overlay at their
//! metadata's file offset, so a live open of the map
//! (`native_map::open`) and the guest's own re-reads of the file read the
//! edited tag.
//!
//! Grown tag blocks go to the edit arena (`hot_tags::arena`, in the HLE
//! reservation, pages mapped through the runtime's memory as it grows).
//! [`install`] hands back its [`hot_tags::arena::Link`] so the boot can
//! give it to `world.placement.edit`'s queue, which grows placement blocks
//! into the same arena.

use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
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
    let vfs_for_meta = vfs.clone();
    editor = editor.with_raw(
        Box::new(RawGlue { vfs, overlay: vfs::RawOverlay::global().clone() }),
        Box::new(|r| render_bridge::native_edits::push(native_edit(r))),
    );
    // A live collision commit on the active structure makes the bots'
    // line-of-sight walls stale: rebuilt from the committed BSP
    // (docs/editor/realtime-edits.md, "Collision").
    agent.on_structure_collision(|c| {
        if c.active {
            bots::autonav::collision_edited(Arc::clone(&c.bsp));
        }
    });
    let mut meta = MetaGlue {
        maps: maps.clone(),
        vfs: vfs_for_meta,
        overlay: vfs::RawOverlay::global().clone(),
        spaces: None,
    };
    editor = editor.with_tag_writes(Box::new(move |g: &mut runtime::Guest, n: &hot_tags::TagWrite| {
        meta.put(g, n);
        tag_written(g, n);
    }));
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

/// The BSP activation `edit.collision.commit` runs with `activate` or
/// `mode: rebuild`: the deactivate and activate pair `BSP_SWITCH` calls,
/// queued on the agent's guest calls for the next tick
/// (`agent::hot::collision::QueuedActivation`;
/// docs/editor/realtime-edits.md, "Collision").
pub fn install_collision_activation(agent: &agent::Agent, calls: agent::sim::calls::Shared) {
    agent.set_collision_activation(Box::new(agent::hot::collision::QueuedActivation::new(calls)));
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

/// Each tag's metadata region in a map's file, by tag index slot: the
/// ordinary metadata, or a structure BSP's or lightmap's own region
/// (docs/formats/map-format.md, "BSP and lightmap metadata").
#[derive(Clone, Debug, Default)]
pub struct Spaces {
    slots: std::collections::HashMap<u32, map::AddressSpace>,
}

impl Spaces {
    pub fn of<R: Read + Seek>(map: &map::Map<R>) -> Spaces {
        Spaces {
            slots: map
                .tags()
                .iter()
                .filter_map(|t| Some((t.id.0 & 0xffff, t.location?.space)))
                .collect(),
        }
    }

    /// The file offset of `len` bytes at guest address `at` in tag `tag`
    /// (matched by index, as `native_map::tag_of` does): `None` outside
    /// the tag's region, as a block moved into the edit arena is.
    pub fn offset(&self, tag: u32, at: u32, len: u32) -> Option<u64> {
        self.slots.get(&(tag & 0xffff))?.translate(at, len).ok()
    }
}

/// Puts each span of tag `tag` (its guest address and the bytes guest
/// memory holds there now) into `overlay` at its file offset in `file`,
/// as metadata. A span outside the tag's region is left out. How many
/// went in.
pub fn overlay_metadata(
    overlay: &vfs::RawOverlay,
    file: &str,
    spaces: &Spaces,
    tag: u32,
    spans: &[(u32, Vec<u8>)],
) -> usize {
    let mut put = 0;
    for (at, bytes) in spans {
        let Ok(len) = u32::try_from(bytes.len()) else { continue };
        if let Some(offset) = spaces.offset(tag, *at, len) {
            overlay.put_metadata(file, offset, bytes);
            put += 1;
        }
    }
    put
}

/// The metadata overlay (docs/editor/realtime-edits.md, "The edit
/// overlay"): every applied tag write and revert, as guest memory holds
/// it after, at its file offset in the loaded map. The entries mirror
/// guest bytes, so they never refuse a snapshot
/// (`vfs::RawOverlay::holds_raw`); a new map clears them with the rest
/// of the overlay.
struct MetaGlue {
    maps: Option<Arc<MapLocator>>,
    vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    overlay: vfs::RawOverlay,
    /// The loaded map's regions, keyed by its file and scenario.
    spaces: Option<((std::path::PathBuf, u64, u64, u32), Spaces)>,
}

impl MetaGlue {
    fn spaces(&mut self, g: &runtime::Guest) -> Result<&Spaces, String> {
        let locate = self.maps.as_ref().ok_or("the loaded map's file is not known")?;
        let file = locate(g).ok_or("the loaded map's file is not found")?;
        let scenario = hot_tags::index::index(g)?.scenario;
        let key = (file.path.clone(), file.offset, file.len, scenario);
        if self.spaces.as_ref().is_none_or(|(k, _)| *k != key) {
            let reader = std::fs::File::open(&file.path).map_err(|e| e.to_string())?;
            let map = map::Map::from_region(std::io::BufReader::new(reader), file.offset, file.len)
                .map_err(|e| e.to_string())?;
            self.spaces = Some((key, Spaces::of(&map)));
        }
        Ok(&self.spaces.as_ref().expect("set above").1)
    }

    fn put(&mut self, g: &runtime::Guest, n: &hot_tags::TagWrite) {
        if n.spans.is_empty() {
            return;
        }
        let name = match loaded_name(g) {
            Ok(name) => overlay_name(ResourceFile::Local, &name),
            Err(_) => return,
        };
        // The guest re-reads a BSP's region from its slot at a switch:
        // the slot serves the overlay as the map.
        if let Some(vfs) = &self.vfs {
            let vfs = vfs.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(why) = alias_slots(g, &vfs, &self.overlay) {
                eprintln!("hot edits: tag {:#010x}: the guest's slot does not read the metadata overlay ({why})", n.tag);
            }
        }
        let spans: Vec<(u32, Vec<u8>)> = n
            .spans
            .iter()
            .filter_map(|r| {
                let mut b = vec![0; r.end.checked_sub(r.start)? as usize];
                g.read_bytes(r.start, &mut b, 1).ok()?;
                Some((r.start, b))
            })
            .collect();
        let overlay = self.overlay.clone();
        match self.spaces(g) {
            Ok(spaces) => {
                overlay_metadata(&overlay, &name, spaces, n.tag, &spans);
            }
            Err(why) => eprintln!(
                "hot edits: tag {:#010x}: not in the metadata overlay ({why}); native copies read the file's bytes",
                n.tag
            ),
        }
    }
}

const SBSP: u32 = u32::from_be_bytes(*b"sbsp");
const JMAD: u32 = u32::from_be_bytes(*b"jmad");
/// `jmad` header's animation block pair (docs/formats/animation.md, "Tag layout").
const ANIMATION_BLOCK: u32 = 0x2c;
const NHDT: u32 = u32::from_be_bytes(*b"nhdt");
const LTMP: u32 = u32::from_be_bytes(*b"ltmp");
const BITM: u32 = u32::from_be_bytes(*b"bitm");

/// The host subscribers of a tag-write notice
/// (docs/editor/realtime-edits.md, "Realtime coverage", R8), called with
/// the guest held right after the edit applied:
///
/// * every tag: `native_edits::Edit::Tag`, for the native copies (a
///   `shad`, `stem`, `spas` or `vrtx` is queued as `Edit::Shader`, R10);
/// * `jmad`: the clip store drops the graph's animation records, including
///   a previous array displaced by a move or revert;
/// * `nhdt`: the HUD kit copies the edited widget records again;
/// * `weap`, `matg`: aim assist adopts the edited fields as retail;
/// * `sbsp`: the bots' data is marked stale;
/// * `sbsp`, `ltmp`, `bitm`: the lightmap store (`engine::sim_lightmap`)
///   loads again at its next sample.
fn tag_written(g: &mut runtime::Guest, n: &hot_tags::TagWrite) {
    let group = n.group.as_u32();
    render_bridge::native_edits::push(render_bridge::native_edits::Edit::Tag { group, tag: n.tag });
    match group {
        JMAD => {
            for range in animation_clip_ranges(g, n) {
                engine::sim_anim::STORE.flush_range(range.start, range.end);
            }
        }
        NHDT => {
            if let Err(e) = crate::hud_kit::tag_written(g, n.range.start, n.range.end) {
                eprintln!("hud kit: an edited nhdt {:#010x} was not copied again ({e:?}); it shows at the next game", n.tag);
            }
        }
        crate::aim_assist::WEAP | crate::aim_assist::MATG => {
            crate::aim_assist::tag_written(&*g, group, n.tag, n.address, n.range.start, n.range.end);
        }
        SBSP => {
            bots::cache::mark_stale(format!("a live edit wrote sbsp {:#010x}", n.tag));
            engine::sim_lightmap::invalidate();
        }
        LTMP | BITM => engine::sim_lightmap::invalidate(),
        _ => {}
    }
}

/// The cache key is in an animation record, not the graph header. The
/// records can be shared metadata or in the edit arena after a move.
fn animation_clip_ranges(g: &runtime::Guest, notice: &hot_tags::TagWrite) -> Vec<Range<u32>> {
    if notice.size < ANIMATION_BLOCK + 8 {
        return Vec::new();
    }
    let Some(pair_end) = notice.address.checked_add(ANIMATION_BLOCK + 8) else {
        return Vec::new();
    };
    let root = pair_end - 8;
    let current = g.u32(root).ok().zip(g.u32(root + 4).ok());
    current
        .into_iter()
        .chain(
            notice
                .moved_blocks
                .iter()
                .filter(|m| m.root == root)
                .map(|m| m.previous),
        )
        .filter_map(|(count, at)| {
            if count == 0 || at == 0 {
                return None;
            }
            let len = count.checked_mul(tags::anim::ANIMATION_SIZE as u32)?;
            let end = at.checked_add(len)?;
            g.check(at, len as usize, runtime::guest::Access::Read, 1).ok()?;
            Some(at..end)
        })
        .collect()
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

/// The lightmap store's reader of the loaded map
/// (docs/engine/sim-render-split.md, "6. Render scratch and stamps"): raw
/// geometry blocks and bitmap levels through the VFS and the raw overlay,
/// the bytes the guest's caches read.
pub fn install_lightmap_files(vfs: Arc<Mutex<vfs::Vfs>>) {
    engine::sim_lightmap::set_raw_files(Box::new(LightmapFiles {
        glue: RawGlue { vfs: Some(vfs), overlay: vfs::RawOverlay::global().clone() },
        bound: Vec::new(),
    }));
}

/// The reader, with each file selector's path and overlay name taken at
/// the store's load ([`engine::sim_lightmap::RawFiles::bind`]), so a
/// block read at its first ask needs no guest.
struct LightmapFiles {
    glue: RawGlue,
    bound: Vec<(ResourceFile, Result<String, String>, String)>,
}

impl engine::sim_lightmap::RawFiles for LightmapFiles {
    fn bind(&mut self, g: &runtime::Guest) {
        self.bound = [
            ResourceFile::Local,
            ResourceFile::MainMenu,
            ResourceFile::Shared,
            ResourceFile::SinglePlayerShared,
        ]
        .into_iter()
        .map(|file| (file, source_of(g, file), self.glue.name(g, file)))
        .collect();
    }

    fn read_raw(&mut self, pointer: u32, size: u32) -> Result<Vec<u8>, String> {
        let p = map::RawPointer::decode(pointer).ok_or("the raw pointer is 0xffffffff")?;
        let (_, path, name) = self
            .bound
            .iter()
            .find(|(f, _, _)| *f == p.file)
            .ok_or("the reader is not bound to a map")?;
        let (source, own) = self.glue.source_at(path.as_ref().map_err(|e| e.clone())?)?;
        self.glue.read_from(source, own, name, u64::from(p.offset), size as usize)
    }
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
        self.source_at(&source_of(g, file)?)
    }

    /// `len` bytes at `offset` of `source` (`own` bytes long), with the raw
    /// overlay's edits of the file named `name` applied.
    fn read_from(&self, source: vfs::FileSource, own: u64, name: &str, offset: u64, len: usize) -> Result<Vec<u8>, String> {
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
            vfs::FileSource::Spliced(f) => {
                let mut source = f.open().map_err(|e| e.to_string())?;
                content::patch::Source::read_exact_at(&mut source, offset, &mut buf[..inside])
                    .map_err(|e| e.to_string())?;
            }
        }
        let served = self.overlay.apply(name, offset, &mut buf, inside);
        if served < len {
            return Err(format!("{name}: {len} bytes at {offset:#x} run past the file"));
        }
        Ok(buf)
    }

    /// The file at `path`: its bytes source and length.
    fn source_at(&self, path: &str) -> Result<(vfs::FileSource, u64), String> {
        let path = path.to_owned();
        let source = {
            let mut vfs = self.vfs()?;
            vfs.file_source(&path)
                .or_else(|| vfs.host_file(&path).map(vfs::FileSource::Host))
        }
        .ok_or_else(|| format!("{path}: no readable source"))?;
        let len = match &source {
            vfs::FileSource::Host(h) => h.len,
            vfs::FileSource::Memory(b) => b.len() as u64,
            vfs::FileSource::Spliced(f) => f.len(),
        };
        Ok((source, len))
    }
}

/// Aliases each cache slot file the guest reads to the map it holds, so
/// the guest's reads serve `overlay` as the map. The loaded map's slot
/// must resolve; a shared map's may not.
fn alias_slots(g: &runtime::Guest, vfs: &vfs::Vfs, overlay: &vfs::RawOverlay) -> Result<(), String> {
    let map = loaded_name(g)?;
    for file in [
        ResourceFile::Local,
        ResourceFile::MainMenu,
        ResourceFile::Shared,
        ResourceFile::SinglePlayerShared,
    ] {
        let object = slot_of(g, file).and_then(|slot| slot_object(vfs, slot));
        match object {
            Ok(object) => overlay.alias(&object, &overlay_name(file, &map)),
            // The guest pages the loaded map in from its slot; without
            // it a live raw write would be lost at the next page-in.
            Err(why) if file == ResourceFile::Local => return Err(why),
            Err(_) => {}
        }
    }
    Ok(())
}

impl hot_tags::RawFiles for RawGlue {
    fn ready(&mut self, g: &runtime::Guest) -> Result<(), String> {
        let vfs = self.vfs()?;
        alias_slots(g, &vfs, &self.overlay)
    }

    fn read(&mut self, g: &runtime::Guest, file: ResourceFile, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let (source, own) = self.source(g, file)?;
        self.read_from(source, own, &self.name(g, file), offset, len)
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
            "appended_bytes": s.appended_bytes, "metadata": s.metadata,
            "metadata_bytes": s.metadata_bytes, "files": s.files})
    }
}

#[cfg(test)]
mod tests;
