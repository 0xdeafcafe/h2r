//! The `map-pack` converter of `convert` items
//! (docs/editor/studio-redesign.md, "Conversions"): the install above as a
//! [`workspace::Converter`]. Each map has a unit with its store marker
//! as output. Installing the units also publishes the game's content
//! metadata and the matching title update.
use workspace::{ConvertSpec, Converter, Input, Unit};
use workspace::converted::marker_path;

use super::*;

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
        CONVERTER
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
        let root = boot::store_status::root(&self.data);
        self.data.join(STAMP).is_file()
            && workspace::converted::levels(&root).iter().any(|mark| {
                workspace::converted::verified(&root, mark, CONVERTER).is_some()
            })
    }

    fn units(&self, input: &Input, spec: &ConvertSpec) -> Result<Vec<Unit>, String> {
        if let Some(k) = spec.settings.keys().next() {
            return Err(format!("settings: the map-pack converter takes none, not {k:?}"));
        }
        let mut iso = crate::open_iso(&input.path).map_err(|e| format!("{e:#}"))?;
        let entries = iso.entries().map_err(|e| format!("{e:#}"))?;
        let plan = Plan::from_entries(&entries).map_err(|e| format!("{e:#}"))?;
        let packages = layout(&mut iso, &plan).map_err(|e| format!("{e:#}"))?;
        let root = boot::store_status::root(&self.data);
        Ok(packages.iter().map(|package| {
            let name = map_stem(&package.map);
            Unit { output: Some(marker_path(&root, &name)), name, key: String::new() }
        }).collect())
    }

    fn convert(
        &self,
        input: &Input,
        _: &ConvertSpec,
        todo: &[Unit],
        done: &mut dyn FnMut(&str, Result<(), String>),
    ) {
        if todo.is_empty() {
            return;
        }
        let r = self.base().and_then(|base| {
            let line = install_disc(&input.path, &self.game_disc, &base, &self.data)
                .map_err(|e| format!("{e:#}"))?;
            println!("map pack: {line}");
            Ok(())
        });
        for unit in todo {
            done(&unit.name, r.clone());
        }
    }
}
