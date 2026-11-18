//! "Convert to Halo 2": a read-only Halo 1 map built into a whole new Halo 2
//! map by `h1convert`'s from-scratch path (`fresh::from_scratch`,
//! docs/halo1/multiplayer.md, "Then built from scratch"), off the UI
//! thread, and written as the Halo 1 import's package
//! (`<data>/maps/h1_<map>/`, one `[[variant]]` per sandbox).
//!
//! The scratch build still reads the retail tags a level needs (globals,
//! shaders, the sandbox's weapons and vehicles) by path from a Halo 2 map:
//! that map is read from the open Halo 2 source without asking, and is
//! never shown as something the conversion goes "onto".
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use content::Manifest;
use content::manifest::MANIFEST_FILE;
use h1convert::convert::{Converted, MAPS, Sandbox, known};
use h1convert::ui::Card;

use super::{Action, App};

/// What a conversion reads.
#[derive(Clone, Debug)]
pub struct Input {
    /// The Halo 1 map's name (`bloodgulch`).
    pub h1_map: String,
    /// The Halo 1 map's file, as stored.
    pub h1_file: Vec<u8>,
    /// A retail Halo 2 map's bytes: where the scratch build reads the
    /// retail tags it names by path.
    pub retail: Vec<u8>,
    pub sandbox: Sandbox,
    /// Halo 1's `ui.map`, for the map's own card (picture, name,
    /// description), when the Halo 1 source has one.
    pub ui: Option<Vec<u8>>,
}

/// A finished conversion: the new map and its package manifest, the
/// card, and notes for the log.
#[derive(Clone, Debug)]
pub struct Built {
    pub converted: Converted,
    pub card: Option<Card>,
    pub notes: Vec<String>,
}

/// The conversion itself. [`from_scratch`] unless a test injects another.
pub type Converter = fn(&Input) -> Result<Built, h1convert::Error>;

/// The from-scratch conversion, as `h2 h1 convert` makes it by default:
/// prepared with the sandbox's objects (both sandboxes place Halo 1's
/// scenery), the MOPP built, then every tag laid out anew
/// (`Prepared::build_fresh`, which builds from scratch).
pub fn from_scratch(input: &Input) -> Result<Built, h1convert::Error> {
    use h1convert::convert::halo1::{prepare_halo1, prepare_halo2};
    let h1 = h1map::H1Map::from_reader(&input.h1_file[..])?;
    let mut retail = map::Map::from_reader(Cursor::new(input.retail.clone()))?;
    let sha = content::Sha256::of(&input.h1_file);
    let templates = h1objects::base::Base::read(&mut retail)
        .map_err(|e| h1convert::Error::Objects(e.to_string()))?;
    let mut objects = h1objects::sandbox::Objects::new(&h1, &templates);
    let prepared = match input.sandbox {
        Sandbox::Halo2 => prepare_halo2(&h1, sha, &mut retail, &mut objects)?,
        Sandbox::Halo1 => prepare_halo1(&h1, sha, &mut retail, &mut objects)?,
    };
    let mopp = h1convert::convert::structure_mopp(&prepared.collision)?;
    let fresh = prepared.build_fresh(&mut retail, &mopp)?;
    let mut notes = Vec::new();
    if let Some(why) = &fresh.report.scratch_failed {
        notes.push(format!(
            "not every tag could be laid out from scratch ({why}); the pruned map was written"
        ));
    }
    let converted = Converted {
        manifest: prepared.manifest_fresh(&fresh),
        name: fresh.name,
        bytes: fresh.bytes,
    };
    let card = input.ui.as_deref().and_then(|ui| {
        let ui = h1map::H1Map::from_reader(ui).ok()?;
        let name = input.h1_map.to_ascii_lowercase();
        h1convert::ui::mp_card(&ui, &name).ok().flatten()
    });
    Ok(Built {
        converted,
        card,
        notes,
    })
}

/// The Halo 2 maps the scratch build may read retail tags from, best
/// first: h1convert's pick for the map (`convert::MAPS`), then every other
/// map that table picks for any level. Only the first one the open source
/// has is read.
pub fn retail_sources(h1_map: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    if let Some((_, m)) = known(h1_map) {
        out.push(m);
    }
    for (_, _, m) in MAPS {
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

/// What a finished conversion wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    /// The package directory (`h1_<map>`).
    pub dir: String,
    /// The Maps list key of the new variant.
    pub key: String,
    pub map_file: PathBuf,
}

/// Why a conversion did not write anything, in the player's words.
pub fn explain(e: &h1convert::Error) -> String {
    match e {
        h1convert::Error::TooManySurfaces(n) => format!(
            "This level has {} surfaces; Halo 2 holds at most 65,536, so it can't be converted.",
            thousands(*n)
        ),
        h1convert::Error::Halo1Sandbox => {
            "This conversion has no Halo 1 objects: convert with the Halo 2 sandbox.".into()
        }
        other => format!("The conversion failed: {other}"),
    }
}

/// `n` with thousands separators (`70,000`).
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The card's picture in the package (as `h2 h1 convert` names it).
pub const PREVIEW_FILE: &str = "preview.png";

/// Write a conversion into `maps` (`<data>/maps`) the way `h2 h1 convert`
/// does: the variant's map at `h1_<map>/<sandbox>/h1_<map>.map`, the card's
/// picture, and the package manifest, keeping the other sandbox's variant
/// when there is one.
pub fn write(maps: &Path, built: &Built) -> std::io::Result<Written> {
    let done = &built.converted;
    let dir = maps.join(&done.name);
    let manifest_path = dir.join(MANIFEST_FILE);
    let mut manifest = done.manifest.clone();
    let ours: Vec<String> = manifest
        .variants
        .iter()
        .map(|v| v.origin.sandbox.clone())
        .collect();
    if let Some(old) = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|t| Manifest::from_toml(&t).ok())
    {
        manifest.variants.extend(
            old.variants
                .into_iter()
                .filter(|v| !ours.contains(&v.origin.sandbox)),
        );
        manifest
            .variants
            .sort_by(|x, y| x.origin.sandbox.cmp(&y.origin.sandbox));
    }
    let variant = done
        .manifest
        .variants
        .first()
        .ok_or_else(|| std::io::Error::other("the conversion's manifest has no variant"))?;
    let map_file = dir.join(&variant.file);
    if let Some(parent) = map_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    content::store::write_atomic(&map_file, &done.bytes)?;
    if let Some(c) = &built.card {
        let mut png = Vec::new();
        assets::png::write_rgba(&mut png, c.width, c.height, &c.rgba)?;
        content::store::write_atomic(&dir.join(PREVIEW_FILE), &png)?;
        manifest.package.name = c.name.clone();
        manifest.package.description = Some(c.description.clone());
        manifest.package.image = Some(PREVIEW_FILE.to_owned());
    }
    content::store::write_atomic(&manifest_path, manifest.to_toml().as_bytes())?;
    let sandbox = editor_core::packages::Sandbox::parse(&variant.origin.sandbox);
    Ok(Written {
        dir: done.name.clone(),
        key: match sandbox {
            Some(s) => format!("{}#{}", done.name, s.short()),
            None => done.name.clone(),
        },
        map_file,
    })
}

/// A conversion running on its own thread.
pub struct Running {
    /// The Halo 1 map and the sandbox, for the status line.
    pub h1_map: String,
    pub sandbox: Sandbox,
    pub started: std::time::Instant,
    rx: mpsc::Receiver<Result<(Written, Vec<String>), String>>,
}

impl App {
    /// The conversion in progress, if any.
    pub fn converting(&self) -> Option<&Running> {
        self.convert.as_ref()
    }
    /// Why the last conversion wrote nothing, if it did not.
    pub fn convert_error(&self) -> Option<&str> {
        self.convert_error.as_deref()
    }
    /// Replace the converter (tests).
    pub fn set_converter(&mut self, converter: Converter) {
        self.converter = converter;
    }

    /// Why "Convert to Halo 2" can't start for the Halo 1 map on show, if
    /// it can't: the scratch build needs a Halo 2 source open.
    pub fn convert_refusal(&self) -> Option<&'static str> {
        let h1_map = self.h1_loaded.as_deref()?;
        self.retail_location(h1_map).is_none().then_some(
            "Open your Halo 2 disc too: a new map takes its shaders and objects from it.",
        )
    }

    /// Where the retail tags come from: the first of [`retail_sources`]
    /// the open Halo 2 source has.
    fn retail_location(&self, h1_map: &str) -> Option<(PathBuf, Option<(u64, u64)>)> {
        let source = self.source.as_ref()?;
        let names = source.map_names();
        retail_sources(h1_map)
            .into_iter()
            .find(|m| names.iter().any(|n| n.eq_ignore_ascii_case(m)))
            .and_then(|m| source.location(m).ok())
    }

    /// Start converting the Halo 1 map on show.
    pub(super) fn start_convert(&mut self, sandbox: Sandbox) {
        if self.convert.is_some() {
            self.error("a conversion is already running");
            return;
        }
        let (Some(src), Some(h1_map)) = (&self.h1, self.h1_loaded.clone()) else {
            self.error("show a Halo 1 map first");
            return;
        };
        let Some(location) = self.retail_location(&h1_map) else {
            let msg = self
                .convert_refusal()
                .unwrap_or("open your Halo 2 disc first");
            self.convert_error = Some(msg.to_owned());
            self.error(format!("{h1_map}: {msg}"));
            return;
        };
        let h1_file = match src.read_raw(&h1_map) {
            Ok(b) => b,
            Err(e) => {
                self.error(format!("{h1_map}: {e:#}"));
                return;
            }
        };
        let map_name = h1_map.clone();
        let ui = src
            .map_names()
            .iter()
            .any(|m| m.eq_ignore_ascii_case("ui"))
            .then(|| src.read_raw("ui").ok())
            .flatten();
        let maps = self.session.maps_dir();
        let converter = self.converter;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let retail = scenario_edit::MapSource::region(location.0, location.1)
                .read_base()
                .map_err(|e| format!("reading your Halo 2 disc: {e}"));
            let r = retail.and_then(|retail| {
                let input = Input {
                    h1_map: map_name,
                    h1_file,
                    retail,
                    sandbox,
                    ui,
                };
                let built = converter(&input).map_err(|e| explain(&e))?;
                let w = write(&maps, &built).map_err(|e| format!("writing the package: {e}"))?;
                Ok((w, built.notes))
            });
            let _ = tx.send(r);
        });
        self.convert_error = None;
        self.info(format!(
            "building a new map from {h1_map} ({} sandbox)...",
            sandbox.name()
        ));
        self.convert = Some(Running {
            h1_map,
            sandbox,
            started: std::time::Instant::now(),
            rx,
        });
    }

    /// Pick up a finished conversion (each frame): refresh the Maps list
    /// and open the new variant, or say why nothing was written.
    pub fn poll_convert(&mut self) {
        let Some(r) = self.convert.as_ref() else {
            return;
        };
        let result = match r.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("the conversion stopped".into()),
        };
        self.finish_convert(result);
    }

    /// Wait for the running conversion (tests and headless runs).
    pub fn wait_convert(&mut self) {
        if let Some(r) = self.convert.as_ref() {
            let result =
                r.rx.recv()
                    .unwrap_or_else(|_| Err("the conversion stopped".into()));
            self.finish_convert(result);
        }
    }

    fn finish_convert(&mut self, result: Result<(Written, Vec<String>), String>) {
        let Some(r) = self.convert.take() else {
            return;
        };
        let secs = r.started.elapsed().as_secs_f32();
        match result {
            Ok((w, notes)) => {
                for n in notes {
                    self.info(format!("{}: {n}", r.h1_map));
                }
                self.info(format!(
                    "built {} from {} in {secs:.1} s: {}",
                    w.dir,
                    r.h1_map,
                    w.map_file.display()
                ));
                self.refresh_packages();
                self.dispatch(Action::OpenPackageEntry(w.key));
            }
            Err(msg) => {
                self.error(format!("{}: {msg}", r.h1_map));
                self.convert_error = Some(msg);
            }
        }
    }
}
