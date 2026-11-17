//! "Convert to Halo 2": a read-only Halo 1 map converted by `h1convert`
//! onto its Halo 2 base map, off the UI thread, and written as the Halo 1
//! import's package (`<data>/maps/h1_<map>/`, one `[[variant]]` per
//! sandbox; docs/game/h1-multiplayer.md, "Converting a map").
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use content::Manifest;
use content::manifest::MANIFEST_FILE;
use h1convert::convert::{Converted, Sandbox, known};

use super::{Action, App};

/// The conversion itself: the Halo 1 map's file (as stored), the Halo 2
/// base map's bytes, the sandbox. [`h1convert_convert`] unless a test
/// injects another.
pub type Converter = fn(&[u8], Vec<u8>, Sandbox) -> Result<Converted, h1convert::Error>;

/// `h1convert::convert::convert` over bytes; the Halo 1 sandbox's objects
/// converted by `h1objects` (`convert::halo1::convert_halo1`).
pub fn h1convert_convert(
    h1_file: &[u8],
    base: Vec<u8>,
    sandbox: Sandbox,
) -> Result<Converted, h1convert::Error> {
    let h1 = h1map::H1Map::from_reader(h1_file)?;
    let mut base = map::Map::from_reader(Cursor::new(base))?;
    let sha = content::Sha256::of(h1_file);
    match sandbox {
        Sandbox::Halo2 => h1convert::convert::convert(&h1, sha, &mut base, sandbox),
        Sandbox::Halo1 => {
            let templates = h1objects::base::Base::read(&mut base)
                .map_err(|e| h1convert::Error::Objects(e.to_string()))?;
            let mut objects = h1objects::sandbox::Objects::new(&h1, &templates);
            h1convert::convert::halo1::convert_halo1(&h1, sha, &mut base, &mut objects)
        }
    }
}

/// The Halo 2 map a Halo 1 map is converted onto: h1convert's choice
/// (`convert::MAPS`), else Beaver Creek, as `h2 h1 convert` does.
pub fn base_for(h1_map: &str) -> &'static str {
    known(h1_map).map_or("beavercreek", |(_, base)| base)
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

/// Write a conversion into `maps` (`<data>/maps`) the way `h2 h1 convert`
/// does: the variant's map at `h1_<map>/<sandbox>/h1_<map>.map`, and the
/// package manifest, keeping the other sandbox's variant when there is
/// one.
pub fn write(maps: &Path, done: &Converted) -> std::io::Result<Written> {
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
    rx: mpsc::Receiver<Result<Written, String>>,
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
        let base = base_for(&h1_map);
        let Some(location) = self
            .source
            .as_ref()
            .filter(|s| s.map_names().iter().any(|m| m.eq_ignore_ascii_case(base)))
            .and_then(|s| s.location(base).ok())
        else {
            let msg =
                format!("converting {h1_map} needs {base} from your Halo 2 disc: open it first");
            self.convert_error = Some(msg.clone());
            self.error(msg);
            return;
        };
        let h1_file = match src.read_raw(&h1_map) {
            Ok(b) => b,
            Err(e) => {
                self.error(format!("{h1_map}: {e:#}"));
                return;
            }
        };
        let maps = self.session.maps_dir();
        let converter = self.converter;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let base = scenario_edit::MapSource::region(location.0, location.1)
                .read_base()
                .map_err(|e| format!("reading the base map: {e}"));
            let r = base.and_then(|base| {
                let done = converter(&h1_file, base, sandbox).map_err(|e| explain(&e))?;
                write(&maps, &done).map_err(|e| format!("writing the package: {e}"))
            });
            let _ = tx.send(r);
        });
        self.convert_error = None;
        self.info(format!(
            "converting {h1_map} onto {base} ({} sandbox)...",
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

    fn finish_convert(&mut self, result: Result<Written, String>) {
        let Some(r) = self.convert.take() else {
            return;
        };
        let secs = r.started.elapsed().as_secs_f32();
        match result {
            Ok(w) => {
                self.info(format!(
                    "converted {} in {secs:.1} s: {}",
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
