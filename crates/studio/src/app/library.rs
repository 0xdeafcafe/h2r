//! The Maps list beyond the open source: the map packages under the data
//! root (user packages and converted Halo 1 maps), and Halo 1 maps shown
//! read-only, with a converted map's Halo 1 source as an overlay.
use std::path::Path;
use std::sync::Arc;

use editor_core::halo1::{self, H1Kind, H1Source, H1View};
use editor_core::packages::{self, Halo1Origin, PackageEntry};
use scenario_edit::geometry::{Marker, Tri, WorldPart};
use scenario_edit::scene::{Blend, Image, ReferencePart, Scene, reference_scene};
use scenario_edit::{BlockKind, MapSource as EditSource, Reference};

use super::{App, ToolKind};

/// The scenario block a Halo 1 placement kind is shown as (for its glyph,
/// picking and selection).
pub fn block_of(kind: H1Kind) -> BlockKind {
    match kind {
        H1Kind::StartingLocation => BlockKind::StartingLocations,
        H1Kind::NetgameFlag => BlockKind::NetgameFlags,
        H1Kind::NetgameEquipment => BlockKind::NetgameEquipment,
        H1Kind::Vehicle => BlockKind::Vehicles,
    }
}

/// The Halo 1 kind shown as `block`, if any.
pub fn kind_of(block: BlockKind) -> Option<H1Kind> {
    H1Kind::ALL.into_iter().find(|k| block_of(*k) == block)
}

/// A Halo 1 map's art in the viewport's terms: its BSPs' base maps and
/// lightmap pages drawn through the Halo 2 scene and renderer.
pub fn scene(v: &H1View) -> Scene {
    use editor_core::halo1_art::{H1Blend, H1Picture};
    let image = |p: &H1Picture| Image {
        width: p.width,
        height: p.height,
        rgba: p.rgba.clone(),
    };
    let parts: Vec<ReferencePart> = v
        .art
        .parts
        .iter()
        .map(|p| ReferencePart {
            part: WorldPart {
                transparent: p.blend == H1Blend::Translucent,
                positions: p.positions.clone(),
                normals: p.normals.clone(),
                uvs: p.uvs.clone(),
                tris: p.tris.clone(),
                lightmap_uvs: p.lightmap_uvs.clone(),
                ..WorldPart::default()
            },
            texture: p.texture,
            lightmap: p.lightmap,
            blend: match p.blend {
                H1Blend::Opaque => Blend::Opaque,
                H1Blend::Mask => Blend::Mask,
                H1Blend::Translucent => Blend::Translucent,
            },
        })
        .collect();
    reference_scene(
        v.art.textures.iter().map(image).collect(),
        v.art.lightmaps.iter().map(image).collect(),
        &parts,
        v.art.shaders,
        v.art.textured,
    )
}

/// A Halo 1 map's triangles in the viewport's terms.
pub fn tris(v: &H1View) -> Vec<Tri> {
    v.tris.iter().map(|t| Tri { p: t.p, n: t.n }).collect()
}

/// A Halo 1 map's placements as viewport markers (no models: glyphs).
pub fn markers(v: &H1View) -> Vec<Marker> {
    v.placements
        .iter()
        .map(|p| {
            let (s, c) = p.facing.sin_cos();
            Marker {
                block: block_of(p.kind),
                index: p.index,
                position: p.position,
                axes: [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]],
                scale: 1.0,
                object: None,
            }
        })
        .collect()
}

/// A converted map's Halo 1 source, for the overlay.
pub struct Compare {
    pub origin: Halo1Origin,
    /// The source map's view, or why it could not be found.
    pub view: Result<H1View, String>,
}

impl App {
    /// The packages under the data root, as the Maps list groups them.
    pub fn packages(&self) -> &[PackageEntry] {
        &self.packages
    }
    /// The open package entry's key, if a package is open.
    pub fn opened_package(&self) -> Option<&str> {
        self.opened_package.as_deref()
    }
    /// The open Halo 1 source.
    pub fn halo1_source(&self) -> Option<&H1Source> {
        self.h1.as_ref()
    }
    /// Its maps.
    pub fn halo1_maps(&self) -> &[String] {
        &self.h1_maps
    }
    /// The Halo 1 map shown read-only, if any.
    pub fn halo1_view(&self) -> Option<&H1View> {
        self.h1_view.as_ref()
    }
    /// The open converted map's source, when it has a Halo 1 origin.
    pub fn compare(&self) -> Option<&Compare> {
        self.compare.as_ref()
    }
    /// Changes when the overlay's geometry or visibility changes.
    pub fn overlay_rev(&self) -> u64 {
        self.overlay_rev
    }
    /// The overlay to draw: the converted map's Halo 1 source, when shown.
    pub fn overlay(&self) -> Option<&H1View> {
        if !self.show_overlay {
            return None;
        }
        self.compare.as_ref()?.view.as_ref().ok()
    }
    pub fn overlay_shown(&self) -> bool {
        self.show_overlay
    }

    /// Re-read the packages under the data root.
    pub fn refresh_packages(&mut self) {
        self.packages = packages::list(&self.session.maps_dir());
        self.refresh_map_pack();
    }

    pub(super) fn set_overlay(&mut self, on: bool) {
        self.show_overlay = on;
        self.overlay_rev += 1;
    }

    /// Open `path` as a Halo 1 source if it is one. Returns false (and
    /// changes nothing) when it is not.
    pub(super) fn try_open_halo1(&mut self, path: &Path) -> bool {
        let Ok(src) = H1Source::open(path) else {
            return false;
        };
        self.h1_maps = src.map_names();
        self.info(format!(
            "opened Halo 1 {} ({} maps, read only)",
            path.display(),
            self.h1_maps.len()
        ));
        self.h1 = Some(src);
        true
    }

    /// Leave whatever the scenario tool showed that is not a disc map.
    pub(super) fn forget_library_state(&mut self) {
        self.opened_package = None;
        self.opened_pack = None;
        self.h1_view = None;
        self.h1_loaded = None;
        self.convert_error = None;
        if self.compare.take().is_some() {
            self.overlay_rev += 1;
        }
    }

    /// The Halo 1 map shown read-only.
    pub fn loaded_halo1(&self) -> Option<&str> {
        self.h1_loaded.as_deref()
    }

    pub(super) fn select_halo1_map(&mut self, name: &str) {
        let Some(src) = &self.h1 else {
            self.error("open a Halo 1 disc or folder first");
            return;
        };
        let map = match src.open_map(name) {
            Ok(m) => m,
            Err(e) => {
                self.error(format!("{name}: {e:#}"));
                return;
            }
        };
        let view = halo1::view(name, &map);
        self.session.open_reference(Reference {
            name: name.to_owned(),
            world: tris(&view),
            markers: markers(&view),
            scene: Some(Arc::new(scene(&view))),
        });
        self.forget_library_state();
        self.loaded = None;
        self.saved_path = None;
        self.tool = ToolKind::Scenario;
        let notes = view.notes.clone();
        self.h1_loaded = Some(name.to_owned());
        self.h1_view = Some(view);
        let msg = format!("opened Halo 1 {name}: {}", self.session.status());
        self.info(msg);
        for n in notes {
            self.error(format!("{name}: {n}"));
        }
    }

    pub(super) fn open_package_entry(&mut self, key: &str) {
        let Some(entry) = self.packages.iter().find(|e| e.key() == key).cloned() else {
            self.error(format!("no package {key}: it may have been removed"));
            return;
        };
        if !entry.derived && entry.halo1.is_none() {
            // An edit of a disc map: open that map, then the package over it.
            if !self.maps.iter().any(|m| m.eq_ignore_ascii_case(&entry.map)) {
                self.error(format!(
                    "{} edits {}: open a disc that has that map first",
                    entry.dir, entry.map
                ));
                return;
            }
            self.select_map(&entry.map);
            if self.loaded.as_deref() != Some(entry.map.as_str()) {
                return;
            }
            match self.session.open_package(&entry.dir) {
                Ok(_) => {
                    self.opened_package = Some(key.to_owned());
                    let msg = self.session.status().to_owned();
                    self.info(msg);
                }
                Err(e) => self.error(e.to_string()),
            }
            return;
        }
        let maps = self.session.maps_dir();
        let bytes = match packages::read_map(&maps, &entry, None) {
            Ok(b) => b,
            Err(e) => {
                self.error(format!("{}: {e:#}", entry.label()));
                return;
            }
        };
        // The shared caches come from the open disc, when there is one.
        let anchor = self
            .source
            .as_ref()
            .and_then(|s| s.location("shared").ok())
            .map(|(path, region)| EditSource::region(path, region))
            .unwrap_or_else(|| EditSource::file(maps.join(&entry.dir).join("package.map")));
        let source = EditSource {
            bytes: Some(Arc::from(bytes)),
            ..anchor
        };
        // A converted variant saves as a new package: the import's own
        // package holds both variants and their origins.
        let package = entry.halo1.is_none().then(|| entry.dir.clone());
        if let Err(e) = self.session.open_derived(source, &entry.map, package, key) {
            self.error(format!("{}: {e}", entry.label()));
            return;
        }
        self.forget_library_state();
        self.loaded = Some(entry.map.clone());
        self.saved_path = None;
        self.tool = ToolKind::Scenario;
        self.opened_package = Some(key.to_owned());
        let msg = format!("opened {}: {}", entry.label(), self.session.status());
        self.info(msg);
        if let Some(origin) = entry.halo1 {
            self.load_compare(origin);
        }
    }

    /// Find the converted map's Halo 1 source (the open Halo 1 source, then
    /// `$H1_MAP_DIR`), checked against its hash, for the overlay.
    fn load_compare(&mut self, origin: Halo1Origin) {
        let env = std::env::var_os("H1_MAP_DIR")
            .map(std::path::PathBuf::from)
            .and_then(|p| H1Source::open(&p).ok());
        let sources: Vec<&H1Source> = self.h1.iter().chain(env.iter()).collect();
        let view = halo1::find_source(&sources, &origin.map, origin.source_sha256.as_deref())
            .map(|(m, _)| halo1::view(&origin.map, &m))
            .map_err(|e| format!("{e:#}"));
        match &view {
            Ok(_) => self.info(format!(
                "Halo 1 source {} found: View > Halo 1 overlay shows it over the map",
                origin.map
            )),
            Err(e) => self.info(format!("no Halo 1 overlay: {e}")),
        }
        self.compare = Some(Compare { origin, view });
        self.overlay_rev += 1;
    }
}
