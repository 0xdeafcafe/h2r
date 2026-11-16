//! The scenario editor's state and commands without any UI: the open map,
//! its edit history, the selection, the camera, the markers, viewport
//! drags and gizmo gestures, packages. `h2-editor`'s scenario tool
//! (`crates/studio`) drives a [`Session`].
use crate::edit::{Command, History};
use crate::geometry::{self, Bounds, Camera, Marker, Ray, Tri, Vec3};
use crate::layout::{BlockKind, Role};
use crate::package::{self, PackageForm, SavedPackage};
use crate::save::{self, MapSource};
use crate::scenario::{Changes, Scenario, Value};
use crate::scene::{Scene, SceneBuilder};
use crate::{Error, Result};
use map::{ResourceFile, TagId};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An open map.
struct Doc {
    source: MapSource,
    /// The map's name (`maps\\<name>.map`), the base a package names.
    name: String,
    /// The package directory this state came from or was last saved to.
    package: Option<String>,
    scenario: Scenario,
    history: History,
    /// Changes at the last save (or the source's, empty), for `is_dirty`.
    saved: Changes,
    world: Vec<Tri>,
    /// How many BSP sections failed to load.
    world_failed: usize,
    /// The textured BSP and the placed objects' models, when the map's art
    /// could be read.
    scene: Option<Arc<Scene>>,
    /// A new map (no disc base): `source.bytes` is its whole unedited file,
    /// and a package saved from it carries that file as its first layer.
    derived: bool,
}

/// A map shown read-only: triangles and markers with no scenario to edit
/// (a Halo 1 map, read through `h1map`).
#[derive(Clone, Debug, Default)]
pub struct Reference {
    pub name: String,
    pub world: Vec<Tri>,
    pub markers: Vec<Marker>,
    /// Its art, when the caller built one ([`crate::scene::reference_scene`]).
    pub scene: Option<Arc<Scene>>,
}

/// Scenes kept for maps opened before (reopening one, or opening its
/// package, reuses its art). A few levels are tens of MB each.
const SCENE_CACHE: usize = 4;

/// A drag of the selected placement on a plane (a viewport's
/// primary-button drag).
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub key: u64,
    /// Plane the pointer moves on, and the grab offset from the placement.
    pub plane_point: Vec3,
    pub plane_normal: Vec3,
    pub offset: Vec3,
    pub vertical: bool,
}

/// Why the editor refuses to grow a block while growth is off.
pub const GROWTH_REFUSAL: &str = "the block is full: growing it needs more tag memory, and the \
     engine's headroom for that is not established (docs/editor/scenario-editor.md §6)";

/// The editor state over one map.
pub struct Session {
    doc: Option<Doc>,
    /// The read-only map shown when no map is open for editing.
    reference: Option<Reference>,
    disc_maps: Vec<(String, MapSource)>,
    status: String,
    selected: Option<(BlockKind, usize)>,
    camera: Camera,
    markers: Vec<Marker>,
    /// Bumped whenever the markers or the selection change, so a viewport
    /// knows to rebuild its marker buffer.
    markers_rev: u64,
    /// Bumped whenever a map is opened, so a viewport re-uploads the world.
    world_rev: u64,
    drag: Option<Drag>,
    /// The merge key of the gizmo gesture in progress, if any.
    gesture: Option<u64>,
    next_key: u64,
    /// Whether adding may grow a block past its source size (appending it
    /// to the map's metadata). Off: the engine's headroom for more tag
    /// memory is not established (docs/editor/scenario-editor.md §6).
    allow_growth: bool,
    /// The runtime data root; packages live in `<data>/maps`.
    data_root: PathBuf,
    /// The last package saved or opened.
    last_package: Option<(String, content::MapId)>,
    /// Built scenes by map (path, region, name), newest last.
    scenes: Vec<(String, Arc<Scene>)>,
    /// Whether opening a map builds its [`Scene`] (textures and models).
    pub build_scenes: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            doc: None,
            reference: None,
            disc_maps: Vec::new(),
            status: "Open a .map file, or a disc image to pick a map from it.".into(),
            selected: None,
            camera: Camera::default(),
            markers: Vec::new(),
            markers_rev: 0,
            world_rev: 0,
            drag: None,
            gesture: None,
            next_key: 1,
            allow_growth: false,
            data_root: content::store::default_data_root(),
            last_package: None,
            scenes: Vec::new(),
            build_scenes: true,
        }
    }

    /// The status line: what the last action did or why it failed.
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    /// Where packages are saved and listed: `<root>/maps`. Defaults to
    /// the runtime's default data root (`h2 play` without `--data`).
    pub fn set_data_root(&mut self, root: impl Into<PathBuf>) {
        self.data_root = root.into();
    }
    pub fn maps_dir(&self) -> PathBuf {
        content::store::maps_dir(&self.data_root)
    }
    /// Whether a map is open.
    pub fn is_open(&self) -> bool {
        self.doc.is_some()
    }
    /// The open map's name, as a package's base names it.
    pub fn map_name(&self) -> Option<&str> {
        self.doc.as_ref().map(|d| d.name.as_str())
    }
    /// The package the open state came from or was last saved to.
    pub fn package_dir(&self) -> Option<&str> {
        self.doc.as_ref()?.package.as_deref()
    }
    /// The last package saved or opened, and its `MapId`.
    pub fn last_package(&self) -> Option<&(String, content::MapId)> {
        self.last_package.as_ref()
    }
    /// The maps of the disc listed by [`Self::open_disc`].
    pub fn disc_maps(&self) -> &[(String, MapSource)] {
        &self.disc_maps
    }

    /// Save the current state as a map package under [`Self::maps_dir`]:
    /// one patch from the base (the disc's map) to now, the manifest and
    /// the image. Saving to the package that was opened (or last saved)
    /// replaces its layer; another existing directory is refused.
    pub fn save_package(&mut self, form: &PackageForm) -> Result<SavedPackage> {
        let maps = self.maps_dir();
        let doc = self.doc.as_mut().ok_or(Error::Save("no map is open"))?;
        let saved = package::save_package(
            &maps,
            &doc.name,
            &doc.source,
            &doc.scenario,
            form,
            doc.package.as_deref(),
            doc.derived,
        )?;
        doc.saved = doc.scenario.changes();
        doc.package = Some(form.dir.clone());
        doc.history.seal();
        self.last_package = Some((form.dir.clone(), saved.id));
        self.status = format!(
            "saved package {} ({} {} bytes{}), MapId {}",
            saved.dir.display(),
            saved.format.extension().to_ascii_uppercase(),
            saved.patch_len,
            if form.activate { ", active" } else { "" },
            saved.id
        );
        Ok(saved)
    }

    /// The packages for the open map under [`Self::maps_dir`].
    pub fn packages(&self) -> Vec<content::Package> {
        match self.map_name() {
            Some(name) => package::packages_for(&self.maps_dir(), name),
            None => Vec::new(),
        }
    }

    /// Open package `dir` for the open map: resolve it over the map's base
    /// and make the result the current, unedited state. Refused over
    /// unsaved edits, as switching maps is.
    pub fn open_package(&mut self, dir: &str) -> Result<content::MapId> {
        let doc = self.doc.as_ref().ok_or(Error::Save("no map is open"))?;
        if self.is_dirty() {
            return Err(Error::Refused(
                "the open map has unsaved edits: save or undo them first",
            ));
        }
        let (name, base) = (doc.name.clone(), doc.source.base());
        let (source, p) = package::open_package(&self.maps_dir(), dir, &base)?;
        if !p.manifest.base.map.eq_ignore_ascii_case(&name) {
            return Err(Error::Package(format!(
                "{dir} is a package for {}, not {name}",
                p.manifest.base.map
            )));
        }
        self.open_named(source, &name)?;
        if let Some(doc) = self.doc.as_mut() {
            doc.package = Some(dir.to_owned());
        }
        self.last_package = Some((dir.to_owned(), p.id));
        self.status = format!(
            "opened package {dir} ({}), MapId {}",
            p.manifest.package.name, p.id
        );
        Ok(p.id)
    }

    /// The package form's starting values: the opened package's, else a
    /// new one.
    pub fn form_defaults(&self) -> PackageForm {
        let name = self.map_name().unwrap_or("map").to_owned();
        let opened = self.package_dir().and_then(|dir| {
            let m = self
                .packages()
                .into_iter()
                .find(|p| p.dir_name() == dir)?
                .manifest;
            Some(PackageForm {
                name: m.package.name,
                author: m.package.author.unwrap_or_default(),
                description: m.package.description.unwrap_or_default(),
                image: None,
                dir: dir.to_owned(),
                activate: false,
            })
        });
        opened.unwrap_or_else(|| PackageForm {
            name: format!("{name} edit"),
            dir: PackageForm::default_dir(&name, "edit"),
            ..PackageForm::default()
        })
    }

    /// Open a map, named by its file name (or its name on the disc). The
    /// source is only read; edits stay in memory until saved.
    pub fn open(&mut self, source: MapSource) -> Result<()> {
        let name = self
            .disc_maps
            .iter()
            .find(|(_, s)| *s == source)
            .map(|(n, _)| n.clone())
            .or_else(|| {
                (source.region.is_none())
                    .then(|| source.path.file_stem()?.to_str().map(str::to_owned))
                    .flatten()
            })
            .unwrap_or_else(|| "map".into());
        self.open_named(source, &name)
    }

    /// Open a map whose name (`maps\\<name>.map`) is `name`: read its
    /// scenario and its structure BSP (with the shared caches next to it).
    pub fn open_named(&mut self, source: MapSource, name: &str) -> Result<()> {
        self.open_keyed(source, name, "")
    }

    /// Open a new map (a package with no disc base): `source.bytes` holds
    /// its whole file, and `source`'s location is only where the shared
    /// caches are found. `package` is the package directory it came from,
    /// which a save then replaces; `None` saves as a new package.
    pub fn open_derived(
        &mut self,
        source: MapSource,
        name: &str,
        package: Option<String>,
        cache_key: &str,
    ) -> Result<()> {
        if source.bytes.is_none() {
            return Err(Error::Save("a new map opens from its bytes"));
        }
        self.open_keyed(source, name, cache_key)?;
        if let Some(doc) = self.doc.as_mut() {
            doc.derived = true;
            doc.package = package;
        }
        Ok(())
    }

    /// Whether the open map is a new map (no disc base).
    pub fn is_derived(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.derived)
    }

    /// Show a map read-only (closing any open map).
    pub fn open_reference(&mut self, reference: Reference) {
        self.doc = None;
        self.selected = None;
        self.drag = None;
        self.gesture = None;
        self.camera = Camera::frame(&frame_bounds(&reference.world, &reference.markers));
        let art = reference.scene.as_ref().map_or(String::new(), |s| {
            format!(
                ", {} of {} shaders textured, {} lightmap pages",
                s.stats.textured, s.stats.shaders, s.stats.lightmap_pages
            )
        });
        self.status = format!(
            "{} triangles, {} placements{art} (read only)",
            reference.world.len(),
            reference.markers.len()
        );
        self.reference = Some(reference);
        self.world_rev += 1;
        self.refresh_markers();
    }

    /// The map shown read-only, if any.
    pub fn reference(&self) -> Option<&Reference> {
        self.reference.as_ref()
    }

    fn open_keyed(&mut self, source: MapSource, name: &str, cache_key: &str) -> Result<()> {
        let mut map = source.open()?;
        let scenario = Scenario::read(&mut map)?;
        let mut files = vec![(ResourceFile::Local, map)];
        for (file, s) in geometry::shared_sources(&source) {
            if let Ok(m) = s.open() {
                files.push((file, m));
            }
        }
        let mut raw = geometry::RawFiles::new(files);
        let mut map = source.open()?;
        let started = std::time::Instant::now();
        let (parts, lightmaps, world_failed) =
            geometry::world_parts_lit(&mut map, &mut raw).unwrap_or_default();
        let world = geometry::opaque_triangles(&parts);
        let markers = geometry::markers(&scenario);
        let scene = self.build_scenes.then(|| {
            self.scene_for(
                &source, name, cache_key, &scenario, &markers, &parts, &lightmaps, &mut map,
                &mut raw,
            )
        });
        self.camera = Camera::frame(&frame_bounds(&world, &markers));
        let art = match &scene {
            Some(s) => format!(
                ", {} of {} shaders textured, {} lightmap pages, {} of {} objects modelled ({} ms)",
                s.stats.textured,
                s.stats.shaders,
                s.stats.lightmap_pages,
                s.stats.modelled,
                s.stats.objects,
                started.elapsed().as_millis()
            ),
            None => String::new(),
        };
        self.status = format!(
            "{} triangles{}, {} placements{art}",
            world.len(),
            if world_failed > 0 {
                format!(" ({world_failed} sections not loaded)")
            } else {
                String::new()
            },
            markers.len()
        );
        self.doc = Some(Doc {
            source,
            name: name.to_owned(),
            package: None,
            scenario,
            history: History::default(),
            saved: Changes::default(),
            world,
            world_failed,
            scene,
            derived: false,
        });
        self.reference = None;
        self.selected = None;
        self.drag = None;
        self.gesture = None;
        self.world_rev += 1;
        self.refresh_markers();
        Ok(())
    }

    /// Close the open map, dropping unsaved edits.
    pub fn close(&mut self) {
        self.doc = None;
        self.reference = None;
        self.selected = None;
        self.drag = None;
        self.gesture = None;
        self.world_rev += 1;
        self.refresh_markers();
    }

    /// List the maps on a disc image, for a map picker.
    pub fn open_disc(&mut self, iso: &Path) -> Result<()> {
        self.disc_maps = MapSource::disc_maps(iso)?;
        self.status = format!("{} maps on the disc", self.disc_maps.len());
        Ok(())
    }

    pub fn scenario(&self) -> Option<&Scenario> {
        self.doc.as_ref().map(|d| &d.scenario)
    }
    /// The open map's source.
    pub fn source(&self) -> Option<&MapSource> {
        self.doc.as_ref().map(|d| &d.source)
    }
    /// The structure BSP's triangles, and how many sections failed to load.
    pub fn world(&self) -> (&[Tri], usize) {
        match (&self.doc, &self.reference) {
            (Some(d), _) => (&d.world, d.world_failed),
            (None, Some(r)) => (&r.world, 0),
            (None, None) => (&[], 0),
        }
    }
    /// The open map's textured world and object models, if built.
    pub fn scene(&self) -> Option<&Arc<Scene>> {
        match (&self.doc, &self.reference) {
            (Some(d), _) => d.scene.as_ref(),
            (None, Some(r)) => r.scene.as_ref(),
            (None, None) => None,
        }
    }

    /// The cached scene of a map, else a new one (cached).
    #[allow(clippy::too_many_arguments)]
    fn scene_for<R: std::io::Read + std::io::Seek>(
        &mut self,
        source: &MapSource,
        name: &str,
        cache_key: &str,
        scenario: &Scenario,
        markers: &[Marker],
        parts: &[geometry::WorldPart],
        lightmaps: &[geometry::LightmapPages],
        map: &mut map::Map<R>,
        raw: &mut dyn tags::geometry::RawSource,
    ) -> Arc<Scene> {
        let key = format!("{:?}|{:?}|{name}|{cache_key}", source.path, source.region);
        if let Some(i) = self.scenes.iter().position(|(k, _)| *k == key) {
            let hit = self.scenes.remove(i);
            let scene = hit.1.clone();
            self.scenes.push(hit);
            return scene;
        }
        let mut b = SceneBuilder::new(map, raw, crate::scene::resource_files(source));
        b.world(parts, lightmaps);
        // Every palette entry, so a placement switched to another entry
        // has its model too, and each placement's own reference.
        let objects: Vec<TagId> = scenario
            .blocks
            .iter()
            .flat_map(|b| b.palette.iter().map(|r| r.id))
            .chain(markers.iter().filter_map(|m| m.object))
            .filter(|id| *id != TagId::NULL)
            .collect();
        b.objects(objects);
        let scene = Arc::new(b.finish());
        self.scenes.push((key, scene.clone()));
        if self.scenes.len() > SCENE_CACHE {
            self.scenes.remove(0);
        }
        scene
    }

    /// Changes when another map (or none) is opened.
    pub fn world_rev(&self) -> u64 {
        self.world_rev
    }
    /// A marker per placement with a position, in block order.
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }
    /// Changes whenever [`Self::markers`] or the selection change.
    pub fn markers_rev(&self) -> u64 {
        self.markers_rev
    }
    fn refresh_markers(&mut self) {
        self.markers = match (&self.doc, &self.reference) {
            (Some(d), _) => geometry::markers(&d.scenario),
            (None, Some(r)) => r.markers.clone(),
            (None, None) => Vec::new(),
        };
        self.markers_rev += 1;
    }

    pub fn camera(&self) -> &Camera {
        &self.camera
    }
    pub fn camera_mut(&mut self) -> &mut Camera {
        &mut self.camera
    }
    /// Frame the whole map (world and placements).
    pub fn frame_all(&mut self) {
        let world = self.world().0;
        self.camera = Camera::frame(&frame_bounds(world, &self.markers));
    }
    /// Point the camera at the selected placement.
    pub fn focus_selected(&mut self) {
        if let Some(sel) = self.selected
            && let Some(m) = self.markers.iter().find(|m| (m.block, m.index) == sel)
        {
            self.camera.target = m.position;
        }
    }

    pub fn selected(&self) -> Option<(BlockKind, usize)> {
        self.selected
    }
    pub fn select(&mut self, selection: Option<(BlockKind, usize)>) {
        if selection != self.selected
            && let Some(doc) = self.doc.as_mut()
        {
            doc.history.seal();
        }
        self.selected = selection;
        self.markers_rev += 1;
    }

    /// The placement under a camera ray, if any.
    pub fn pick(&self, ray: &Ray) -> Option<(BlockKind, usize)> {
        match self.scene() {
            Some(s) => s.pick(&self.markers, ray),
            None => geometry::pick(&self.markers, ray),
        }
    }

    /// Edits since the last save (undoing back to the saved state clears it).
    pub fn is_dirty(&self) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| d.scenario.changes() != d.saved)
    }

    /// Write the edited map to `path`: the source with the changed fields
    /// patched and the checksum repaired. Never overwrites the source.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<u64> {
        let doc = self.doc.as_mut().ok_or(Error::Save("no map is open"))?;
        let n = save::save_as(&doc.source, &doc.scenario, path.as_ref())?;
        doc.saved = doc.scenario.changes();
        doc.history.seal();
        self.status = format!("saved {} ({n} bytes)", path.as_ref().display());
        Ok(n)
    }

    /// Apply an edit as one undo step (or merged into the last, by its
    /// merge key).
    pub fn apply(&mut self, command: Command) -> Result<()> {
        let doc = self.doc.as_mut().ok_or(Error::Save("no map is open"))?;
        doc.history.apply(&mut doc.scenario, command)?;
        self.refresh_markers();
        Ok(())
    }
    pub fn undo(&mut self) -> bool {
        self.step(true)
    }
    pub fn redo(&mut self) -> bool {
        self.step(false)
    }
    fn step(&mut self, undo: bool) -> bool {
        let Some(doc) = self.doc.as_mut() else {
            return false;
        };
        self.drag = None;
        self.gesture = None;
        let r = if undo {
            doc.history.undo(&mut doc.scenario)
        } else {
            doc.history.redo(&mut doc.scenario)
        };
        // An undone add or remove can leave the selection past the end.
        if let Some((kind, i)) = self.selected
            && i >= doc.scenario.block(kind).len()
        {
            self.selected = None;
        }
        self.refresh_markers();
        match r {
            Ok(Some(label)) => {
                self.status = format!("{} {label}", if undo { "undid" } else { "redid" });
                true
            }
            Ok(None) => false,
            Err(e) => {
                self.status = e.to_string();
                false
            }
        }
    }
    pub fn can_undo(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.history.can_undo())
    }
    pub fn can_redo(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.history.can_redo())
    }
    /// End the current merge run: the next edit is a new undo step.
    pub fn seal(&mut self) {
        if let Some(doc) = self.doc.as_mut() {
            doc.history.seal();
        }
    }

    /// Let adding grow a block past its source size. Off by default; see
    /// [`GROWTH_REFUSAL`].
    pub fn set_allow_growth(&mut self, allow: bool) {
        self.allow_growth = allow;
    }
    pub fn allow_growth(&self) -> bool {
        self.allow_growth
    }

    /// Why entry removal is refused for a block, if it is.
    pub fn remove_refusal(&self, kind: BlockKind) -> Option<&'static str> {
        self.scenario()?.remove_refusal(kind)
    }

    /// Why adding to a block is refused, if it is.
    pub fn add_refusal(&self, kind: BlockKind) -> Option<&'static str> {
        let s = self.scenario()?;
        if let Some(why) = s.add_refusal(kind) {
            return Some(why);
        }
        let b = s.block(kind);
        (!self.allow_growth && b.len() >= b.capacity()).then_some(GROWTH_REFUSAL)
    }

    /// Remove an entry (the block's last entry takes its slot), as one undo
    /// step. The selection moves to the entry now in that slot, if any.
    pub fn remove_entry(&mut self, kind: BlockKind, index: usize) -> Result<()> {
        if let Some(why) = self.remove_refusal(kind) {
            return Err(Error::Refused(why));
        }
        self.apply(Command::remove(kind, index))?;
        let len = self.scenario().map_or(0, |s| s.block(kind).len());
        self.select((index < len).then_some((kind, index)));
        Ok(())
    }

    /// Add an entry as one undo step: a copy of entry `like`, or a zeroed
    /// entry with the palette's first item, placed at the camera focus.
    /// Selects and returns the new entry's index.
    pub fn add_entry(&mut self, kind: BlockKind, like: Option<usize>) -> Result<usize> {
        if let Some(why) = self.add_refusal(kind) {
            return Err(Error::Refused(why));
        }
        let s = self.scenario().ok_or(Error::Save("no map is open"))?;
        let entry = s.new_entry(kind, like, Some(self.camera.target))?;
        let index = s.block(kind).len();
        let mut command = Command::add(kind, entry);
        if let Some(i) = like {
            command.label = format!("duplicate {kind:?} {i}");
        }
        self.apply(command)?;
        self.select(Some((kind, index)));
        Ok(index)
    }

    /// A fresh merge key: edits made with it merge into one undo step.
    pub fn new_key(&mut self) -> u64 {
        let key = self.next_key;
        self.next_key += 1;
        key
    }

    fn position_field(kind: BlockKind) -> Option<&'static str> {
        crate::layout::block(kind)
            .role(Role::Position)
            .map(|f| f.name)
    }

    /// Start dragging the selected placement on a plane through it:
    /// horizontal, or (`vertical`) facing the camera. False when nothing
    /// draggable is selected.
    pub fn begin_drag(&mut self, ray: &Ray, vertical: bool) -> bool {
        self.drag = self.start_drag(ray, vertical);
        self.drag.is_some()
    }

    fn start_drag(&mut self, ray: &Ray, vertical: bool) -> Option<Drag> {
        let (kind, index) = self.selected?;
        let field = Self::position_field(kind)?;
        let pos = self.scenario()?.get(kind, index, field).ok()?.as_vec3()?;
        let normal = if vertical {
            let f = self.camera.forward();
            geometry::normalize([f[0], f[1], 0.0])
        } else {
            [0.0, 0.0, 1.0]
        };
        let hit = ray.plane(pos, normal)?;
        Some(Drag {
            key: self.new_key(),
            plane_point: pos,
            plane_normal: normal,
            offset: geometry::sub(pos, hit),
            vertical,
        })
    }

    /// Whether a plane drag is in progress.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Move the dragged placement to where `ray` meets the drag plane.
    pub fn drag_to(&mut self, ray: &Ray) {
        let (Some(drag), Some((kind, index))) = (self.drag, self.selected) else {
            return;
        };
        let Some(field) = Self::position_field(kind) else {
            return;
        };
        let Some(hit) = ray.plane(drag.plane_point, drag.plane_normal) else {
            return;
        };
        let Some(Value::Vec3(old)) = self.scenario().and_then(|s| s.get(kind, index, field).ok())
        else {
            return;
        };
        let target = geometry::add(hit, drag.offset);
        let new = if drag.vertical {
            [old[0], old[1], target[2]]
        } else {
            [target[0], target[1], old[2]]
        };
        if new != old {
            let cmd = Command::set(kind, index, field, Value::Vec3(new)).merging(drag.key);
            if let Err(e) = self.apply(cmd) {
                self.status = e.to_string();
            }
        }
    }

    /// End a plane drag: the whole drag is one undo step.
    pub fn end_drag(&mut self) {
        if self.drag.take().is_some() {
            self.seal();
        }
    }

    /// The selected placement's transform: position and, when its block
    /// has one, its rotation as yaw/pitch/roll (a facing-only block gives
    /// `[yaw, 0, 0]`).
    pub fn selected_transform(&self) -> Option<Transform> {
        let (kind, index) = self.selected?;
        let s = self.scenario()?;
        let def = crate::layout::block(kind);
        let position = s
            .get(kind, index, def.role(Role::Position)?.name)
            .ok()?
            .as_vec3()?;
        let rotation = match (def.role(Role::Rotation), def.role(Role::Facing)) {
            (Some(r), _) => Some(RotationField::Ypr(
                s.get(kind, index, r.name).ok()?.as_vec3()?,
            )),
            (None, Some(f)) => Some(RotationField::Facing(
                s.get(kind, index, f.name).ok()?.as_f32()?,
            )),
            _ => None,
        };
        Some(Transform { position, rotation })
    }

    /// Start a gizmo gesture: every [`Self::gesture_set`] until
    /// [`Self::end_gesture`] is one undo step.
    pub fn begin_gesture(&mut self) {
        if self.gesture.is_none() {
            self.gesture = Some(self.new_key());
        }
    }
    pub fn in_gesture(&self) -> bool {
        self.gesture.is_some()
    }

    /// Set the selected placement's position and (if it has one) rotation
    /// within the current gesture (one is begun if needed). Only fields
    /// that changed are written.
    pub fn gesture_set(&mut self, position: Vec3, ypr: Vec3) -> Result<()> {
        self.begin_gesture();
        let key = self.gesture.expect("just begun");
        let (kind, index) = self.selected.ok_or(Error::Save("nothing is selected"))?;
        let before = self
            .selected_transform()
            .ok_or(Error::Save("the selection has no position"))?;
        let def = crate::layout::block(kind);
        let mut cmd = Command {
            label: "move".into(),
            ops: Vec::new(),
            merge: Some(key),
        };
        let finite = |v: &[f32]| v.iter().all(|x| x.is_finite());
        if position != before.position && finite(&position) {
            let f = def.role(Role::Position).expect("has a position");
            cmd.ops
                .push(set_op(kind, index, f.name, Value::Vec3(position)));
        }
        match (
            before.rotation,
            def.role(Role::Rotation),
            def.role(Role::Facing),
        ) {
            (Some(RotationField::Ypr(old)), Some(f), _) if !close3(old, ypr) && finite(&ypr) => {
                cmd.label = "rotate".into();
                cmd.ops.push(set_op(kind, index, f.name, Value::Vec3(ypr)));
            }
            (Some(RotationField::Facing(old)), None, Some(f))
                if (old - ypr[0]).abs() > 1e-5 && ypr[0].is_finite() =>
            {
                cmd.label = "rotate".into();
                cmd.ops
                    .push(set_op(kind, index, f.name, Value::Float(ypr[0])));
            }
            _ => {}
        }
        if cmd.ops.is_empty() {
            return Ok(());
        }
        self.apply(cmd)
    }

    /// End a gizmo gesture (the mouse was released).
    pub fn end_gesture(&mut self) {
        if self.gesture.take().is_some() {
            self.seal();
        }
    }
}

/// A placement's rotation field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RotationField {
    /// Yaw, pitch, roll in radians.
    Ypr(Vec3),
    /// Yaw only, in radians.
    Facing(f32),
}

/// A placement's position and rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Option<RotationField>,
}

impl Transform {
    /// Yaw, pitch, roll (zero when the block has no rotation).
    pub fn ypr(&self) -> Vec3 {
        match self.rotation {
            Some(RotationField::Ypr(v)) => v,
            Some(RotationField::Facing(y)) => [y, 0.0, 0.0],
            None => [0.0; 3],
        }
    }
    /// A column-major model matrix (axes from [`geometry::rotation`], then
    /// the position), as ImGuizmo takes it.
    pub fn matrix(&self) -> [f32; 16] {
        let [f, l, u] = geometry::rotation(self.ypr());
        let p = self.position;
        [
            f[0], f[1], f[2], 0.0, l[0], l[1], l[2], 0.0, u[0], u[1], u[2], 0.0, p[0], p[1], p[2],
            1.0,
        ]
    }
}

fn set_op(kind: BlockKind, index: usize, field: &'static str, value: Value) -> crate::edit::Op {
    crate::edit::Op::Set(crate::edit::SetField {
        block: kind,
        index,
        field,
        value,
    })
}

fn close3(a: Vec3, b: Vec3) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= 1e-5)
}

/// What "frame the map" frames: the placements when there are a few (the
/// playable space; distant scenery such as Ascension's canyon rocks, 300
/// units out, would shrink the level to a speck), trimmed of outliers
/// ([`trimmed_bounds`]: Lockout keeps a few placements far outside the
/// level) and padded a little; else the opaque world.
fn frame_bounds(world: &[Tri], markers: &[Marker]) -> Bounds {
    if markers.len() >= 4 {
        let points: Vec<Vec3> = markers.iter().map(|m| m.position).collect();
        let mut bounds = trimmed_bounds(&points, FRAME_TRIM);
        bounds.add(geometry::sub(bounds.min, [FRAME_PAD; 3]));
        bounds.add(geometry::add(bounds.max, [FRAME_PAD; 3]));
        return bounds;
    }
    let mut bounds = Bounds::EMPTY;
    for t in world {
        t.p.iter().for_each(|&p| bounds.add(p));
    }
    if bounds.is_empty() {
        markers.iter().for_each(|m| bounds.add(m.position));
    }
    bounds
}

/// Bounds of `points` without the `trim` fraction at each end of each
/// axis (at least one point is kept).
pub fn trimmed_bounds(points: &[Vec3], trim: f32) -> Bounds {
    let mut b = Bounds::EMPTY;
    if points.is_empty() {
        return b;
    }
    let n = points.len();
    let cut = ((n as f32 * trim).floor() as usize).min((n - 1) / 2);
    for k in 0..3 {
        let mut v: Vec<f32> = points.iter().map(|p| p[k]).collect();
        v.sort_by(f32::total_cmp);
        b.min[k] = v[cut];
        b.max[k] = v[n - 1 - cut];
    }
    b
}

/// The fraction of placements at each end of each axis that framing drops.
const FRAME_TRIM: f32 = 0.04;

/// Units around the placements that framing keeps in view.
const FRAME_PAD: f32 = 4.0;
