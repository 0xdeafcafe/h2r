//! The waveform edits of a package sound (docs/editor/studio-redesign.md,
//! "The sound editor"): the `sound` item's source WAV is read and never
//! written; its edit list and permutations are the `<name>.sound.yaml`
//! beside it (`sound-edit`), a `data` item of the package, which the build
//! renders (`workspace::import`'s sound builder). `sound.edit`,
//! `sound.render` and `sound.permutation` are over this.
use std::path::{Path, PathBuf};

use ::workspace::Loaded;
use content::package::{Item, Kind as ItemKind, LOCK_FILE, MANIFEST_FILE, Manifest, Maps};
use sound_edit::{Edit, SoundDoc, Wave};

use super::assets::CopyRefusal;
use super::sound_import::package_dir;

use CopyRefusal::{Failed, Refused};

/// A path inside a package: relative, with no `..`.
fn inside(path: &str) -> bool {
    !(path.is_empty() || path.starts_with(['/', '\\']) || path.contains("..") || path.contains('\0'))
}

/// A package's sound: its source WAV, its settings file and the manifest.
pub struct Opened {
    pub dir: PathBuf,
    pub source: String,
    pub sidecar: String,
    pub wave: Wave,
    pub doc: SoundDoc,
    manifest: Manifest,
}

fn read_wave(path: &Path) -> Result<Wave, CopyRefusal> {
    let bytes = std::fs::read(path).map_err(|e| Failed(format!("{}: {e}", path.display())))?;
    Wave::from_wav(&bytes).map_err(|e| Refused(format!("{}: {e}", path.display())))
}

/// The `sound` item of `package` whose source is `source`: the file is
/// read, its sidecar too if there is one.
pub fn open(data_root: &Path, package: &str, source: &str) -> Result<Opened, CopyRefusal> {
    let dir = package_dir(data_root, package)?;
    if !inside(source) {
        return Err(CopyRefusal::Params(format!("{source:?} is not a path inside the package")));
    }
    let manifest_path = dir.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| Failed(format!("{}: {e}", manifest_path.display())))?;
    let manifest = Manifest::from_toml(&text)
        .map_err(|e| Failed(format!("{}: {e}", manifest_path.display())))?;
    if !manifest
        .items
        .iter()
        .any(|i| i.kind == ItemKind::Sound && i.source.as_deref() == Some(source))
    {
        return Err(Refused(format!("package {package:?} has no sound item with source {source}")));
    }
    let wave = read_wave(&dir.join(source))?;
    let sidecar = ::workspace::import::sidecar_path(source);
    let side_path = dir.join(&sidecar);
    let doc = match std::fs::read_to_string(&side_path) {
        Ok(text) => SoundDoc::from_yaml(&text)
            .map_err(|e| Refused(format!("{}: {e}", side_path.display())))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SoundDoc::default(),
        Err(e) => return Err(Failed(format!("{}: {e}", side_path.display()))),
    };
    Ok(Opened {
        dir,
        source: source.to_owned(),
        sidecar,
        wave,
        doc,
        manifest,
    })
}

impl Opened {
    /// The WAV `permutation` is (the source's own for `None`).
    pub fn wave_of(&self, permutation: Option<usize>) -> Result<Wave, CopyRefusal> {
        match permutation {
            None => Ok(self.wave.clone()),
            Some(i) => {
                let p = self.doc.permutations.get(i).ok_or_else(|| {
                    CopyRefusal::Params(format!(
                        "permutation {i}: the sound lists {}",
                        self.doc.permutations.len()
                    ))
                })?;
                read_wave(&self.dir.join(&p.file))
            }
        }
    }

    /// The WAV of `permutation` through its first `upto` edits (all by
    /// default). The file is not touched.
    pub fn rendered(
        &self,
        permutation: Option<usize>,
        upto: Option<usize>,
    ) -> Result<Wave, CopyRefusal> {
        let wave = self.wave_of(permutation)?;
        let edits: &[Edit] = match permutation {
            None => &self.doc.edits,
            Some(i) => &self.doc.permutations[i].edits,
        };
        let n = upto.unwrap_or(edits.len());
        if n > edits.len() {
            return Err(CopyRefusal::Params(format!("upto {n}: the list has {}", edits.len())));
        }
        sound_edit::render(&wave, &edits[..n]).map_err(|e| Refused(e.to_string()))
    }

    /// The settings file, a `data` item when it was not, and the lock,
    /// written. A document that does not render is not written.
    pub fn save(&mut self) -> Result<(), CopyRefusal> {
        self.doc.check().map_err(Refused)?;
        self.rendered(None, None)?;
        for i in 0..self.doc.permutations.len() {
            self.rendered(Some(i), None)?;
        }
        let side = self.dir.join(&self.sidecar);
        std::fs::write(&side, self.doc.to_yaml())
            .map_err(|e| Failed(format!("{}: {e}", side.display())))?;
        self.ensure_item(ItemKind::Data, &self.sidecar.clone());
        for f in self.doc.permutations.iter().map(|p| p.file.clone()).collect::<Vec<_>>() {
            self.ensure_item(ItemKind::Sound, &f);
        }
        self.manifest.validate().map_err(|e| Refused(e.to_string()))?;
        let path = self.dir.join(MANIFEST_FILE);
        std::fs::write(&path, self.manifest.to_toml())
            .map_err(|e| Failed(format!("{}: {e}", path.display())))?;
        // The lock follows the files, as `package.check` expects.
        let loaded = Loaded::from_dir(&self.dir).map_err(|e| Failed(e.to_string()))?;
        std::fs::write(self.dir.join(LOCK_FILE), loaded.package.lock.to_toml())
            .map_err(|e| Failed(format!("{LOCK_FILE}: {e}")))?;
        Ok(())
    }

    fn ensure_item(&mut self, kind: ItemKind, source: &str) {
        if !self.manifest.items.iter().any(|i| i.source.as_deref() == Some(source)) {
            self.manifest.items.push(Item {
                kind,
                inputs: Vec::new(),
                source: Some(source.to_owned()),
                tag: None,
                maps: Maps::default(),
            });
        }
    }

    /// A permutation file read for `add`: it must be a WAV of the package.
    pub fn check_file(&self, file: &str) -> Result<(), CopyRefusal> {
        if !inside(file) {
            return Err(CopyRefusal::Params(format!("{file:?} is not a path inside the package")));
        }
        read_wave(&self.dir.join(file)).map(|_| ())
    }
}

/// Where a render may be written: anywhere but inside the package, whose
/// files are the sources.
pub fn render_target(dir: &Path, out: &Path) -> Result<PathBuf, CopyRefusal> {
    let name = out
        .file_name()
        .ok_or_else(|| CopyRefusal::Params(format!("{}: not a file path", out.display())))?;
    let parent = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    if parent.components().any(|c| c == std::path::Component::ParentDir) {
        return Err(CopyRefusal::Params(format!("{}: no `..` in a path", out.display())));
    }
    // Where it would land, found before anything is created: the nearest
    // folder that exists, and the names below it.
    let mut base = parent.as_path();
    let mut below = Vec::new();
    while !base.exists() {
        below.push(base.file_name().map(PathBuf::from).unwrap_or_default());
        base = base.parent().unwrap_or(Path::new("."));
    }
    let mut landing = base
        .canonicalize()
        .map_err(|e| Failed(format!("{}: {e}", base.display())))?;
    landing.extend(below.iter().rev());
    let dir = dir.canonicalize().map_err(|e| Failed(format!("{}: {e}", dir.display())))?;
    if landing.starts_with(&dir) {
        return Err(Refused(format!(
            "{} is inside the package: a render is written elsewhere, the package's WAVs are sources",
            out.display()
        )));
    }
    std::fs::create_dir_all(&landing).map_err(|e| Failed(format!("{}: {e}", landing.display())))?;
    Ok(landing.join(name))
}
