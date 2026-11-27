//! A source file dropped on the window (docs/editor/studio-redesign.md,
//! "The browser"): a WAV, a PNG or a model YAML goes into a package as
//! the item that builds it, the way the Browser's import buttons and
//! `import.*` put it there. The package is the one the Browser shows or
//! has selected, else the edit target. Any other file opens as a source,
//! as a drop always has.
//!
//! | File | Item | Tag |
//! |---|---|---|
//! | `.wav` | `sound` | `sounds\<name>` |
//! | `.png` | `bitmap` | `bitmaps\<name>` |
//! | `.yaml`, `.yml` | `model`, with its sibling atlas PNG as a `data` item | `models\<name>\model` |
use super::*;

/// What a dropped file builds, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropKind {
    Sound,
    Bitmap,
    Model,
}

impl DropKind {
    /// The kind of a source file; `None` for a file that is not one (a
    /// map, a disc image, a folder), which opens as a source.
    pub fn of(file: &Path) -> Option<Self> {
        let ext = file.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "wav" => Some(Self::Sound),
            "png" => Some(Self::Bitmap),
            "yaml" | "yml" => Some(Self::Model),
            _ => None,
        }
    }

    /// The item kind, as a manifest names it.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Sound => "sound",
            Self::Bitmap => "bitmap",
            Self::Model => "model",
        }
    }

    /// The tag the file's item writes: the kind's folder and the file's
    /// name made safe for a path.
    pub fn tag(self, file: &Path) -> String {
        let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or(self.id());
        let name = crate::app::edit_target::slug_of(stem);
        match self {
            Self::Sound => format!("sounds\\{name}"),
            Self::Bitmap => format!("bitmaps\\{name}"),
            Self::Model => format!("models\\{name}\\model"),
        }
    }
}

fn why(e: CopyRefusal) -> String {
    match e {
        CopyRefusal::Params(m) | CopyRefusal::Refused(m) | CopyRefusal::Failed(m) => m,
    }
}

impl App {
    /// [`super::super::Action::DropFile`]: a source file into `package`
    /// (with none, the edit target; a sound or a model with no target
    /// either asks for one, as **Import sound...** does), any other file
    /// as a source. A sound and a model open their source tab.
    pub(crate) fn drop_file(&mut self, file: PathBuf, package: Option<String>) {
        let Some(kind) = DropKind::of(&file) else {
            return self.open_source(&file);
        };
        let tag = kind.tag(&file);
        let request = match kind {
            DropKind::Sound => CreateAsset::Sound {
                file,
                tag,
                class: None,
            },
            DropKind::Model => CreateAsset::Model { file, tag },
            DropKind::Bitmap => {
                let Some(package) = package.or_else(|| self.edit_target()) else {
                    return self.error(
                        "no package to add the bitmap to: show a package in the Browser, or set the edit target",
                    );
                };
                let done = crate::app::bitmap_import::import_bitmap(
                    &self.data_root(),
                    &file,
                    &package,
                    &tag,
                );
                return match done {
                    Ok(v) => {
                        self.refresh_package_document();
                        self.info(format!(
                            "added {} to {package} as {}",
                            file.display(),
                            v["tag"].as_str().unwrap_or("a bitmap")
                        ));
                    }
                    Err(e) => self.error(why(e)),
                };
            }
        };
        match package {
            Some(package) => {
                if let Err(e) = self.create_asset(request, &package) {
                    self.error(why(e));
                }
            }
            None => self.create_in_target(request),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_file_is_a_source_by_its_extension_and_names_its_tag() {
        let of = |f: &str| DropKind::of(Path::new(f));
        assert_eq!(of("/tmp/Boing Loud.WAV"), Some(DropKind::Sound));
        assert_eq!(of("icon.png"), Some(DropKind::Bitmap));
        assert_eq!(of("pogo.yaml"), Some(DropKind::Model));
        assert_eq!(of("pogo.yml"), Some(DropKind::Model));
        // A map, a disc image and a folder open as a source.
        assert_eq!(of("lockout.map"), None);
        assert_eq!(of("halo2.iso"), None);
        assert_eq!(of("/games/maps"), None);
        assert_eq!(DropKind::Sound.tag(Path::new("clip.wav")), "sounds\\clip");
        assert_eq!(DropKind::Bitmap.tag(Path::new("a/pic.png")), "bitmaps\\pic");
        assert_eq!(DropKind::Model.tag(Path::new("pogo.yaml")), "models\\pogo\\model");
    }
}
