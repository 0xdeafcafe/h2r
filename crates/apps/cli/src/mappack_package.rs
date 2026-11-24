//! The `map-pack` converter of `convert` items
//! (docs/editor/studio-redesign.md, "Conversions"): the install above as a
//! [`workspace::Converter`]. The disc installs as one unit, `maps` (its
//! maps and, for the game disc's version, its title update), keyed by the
//! disc's size and modification time and the game disc's version, as
//! [`STAMP`] is. A disc [`STAMP`] already says is installed is not
//! installed again.
use workspace::{ConvertSpec, Converter, Input, Unit};

use super::*;

/// The one unit: the whole disc.
pub const UNIT: &str = "maps";

/// The Map Pack disc beside `game_disc`, installed into `data`.
pub struct MapPack {
    pub game_disc: PathBuf,
    pub data: PathBuf,
}

impl MapPack {
    /// The game disc's certificate; an error when it is not Halo 2's.
    fn base(&self) -> Result<xbe::Certificate, String> {
        let bytes = crate::load_xbe(&self.game_disc).map_err(|e| format!("{e:#}"))?;
        let x = xbe::Xbe::parse(&bytes).map_err(|e| format!("{e:#}"))?;
        if x.certificate.title_id != HALO2_TITLE {
            return Err(format!("{} is not Halo 2", self.game_disc.display()));
        }
        Ok(x.certificate)
    }
}

impl Converter for MapPack {
    fn name(&self) -> &str {
        "map-pack"
    }

    fn version(&self) -> String {
        STAMP_VERSION.to_string()
    }

    fn input(&self, _: &ConvertSpec) -> Result<Input, String> {
        let iso = discover(&self.game_disc).ok_or_else(|| {
            format!(
                "no Map Pack disc beside {} (put {AUTO_NAME} there)",
                self.game_disc.display()
            )
        })?;
        let base = self.base()?;
        let identity = stamp_text(&iso, base.version).map_err(|e| format!("{e:#}"))?;
        Ok(Input {
            label: iso.display().to_string(),
            identity: identity.trim().to_owned(),
            path: iso,
        })
    }

    /// What `h2 import-mappack` (or the install before packages) put in.
    fn kept(&self, _: &ConvertSpec) -> bool {
        self.data.join(STAMP).is_file()
    }

    fn units(&self, _: &Input, spec: &ConvertSpec) -> Result<Vec<Unit>, String> {
        if let Some(k) = spec.settings.keys().next() {
            return Err(format!(
                "settings: the map-pack converter takes none, not {k:?}"
            ));
        }
        let content = vfs::Config::new(&self.data, HALO2_TITLE)
            .title_data_dir()
            .join(vfs::CONTENT_DIR);
        Ok(vec![Unit {
            name: UNIT.into(),
            key: String::new(),
            output: Some(content),
        }])
    }

    fn convert(
        &self,
        input: &Input,
        _: &ConvertSpec,
        todo: &[Unit],
        done: &mut dyn FnMut(&str, Result<(), String>),
    ) {
        if !todo.iter().any(|u| u.name == UNIT) {
            return;
        }
        let r = self.base().and_then(|base| {
            if already_installed(&self.data, &input.path, base.version) {
                return Ok(());
            }
            let line =
                install_disc(&input.path, &base, &self.data).map_err(|e| format!("{e:#}"))?;
            println!("map pack: {line}");
            Ok(())
        });
        done(UNIT, r);
    }
}
