//! The Firefight kit of every bundled multiplayer map that takes one
//! (docs/game/firefight.md, "The kit"), built here from the disc's own map
//! by the offline builder `h2 firefight kit` uses, so the host's package is
//! the one its players build from their discs. Each is carried as
//! `firefight/firefight-<map>/` (the package's manifest and patch), with
//! `firefight/kits.json` naming the kit version and each map's package ID.
//! A Firefight lease's host places its map's package in its data root
//! ([`crate::install_firefight_kit`]).

use std::{collections::BTreeMap, fs, io::{Read, Write}, path::Path};

use super::{Payload, Source};
use crate::{Error, FIREFIGHT_DIR, KIT_INDEX, KitIndex};

/// One multiplayer map's disc copy, the kit's base.
pub(super) struct KitMap {
    /// The lower-case map name (`beavercreek`): the disc file's stem.
    pub key: String,
    pub source: Source,
}

/// What [`stage`] carried and what it left out.
pub(super) struct Staged {
    pub index: KitIndex,
    /// Maps the kit refuses (no headroom, too few drops, no Elite), with
    /// the builder's reason.
    pub refused: Vec<(String, String)>,
}

/// Build each map's kit into `staging/firefight`, add the packages and the
/// index to `payload`.
pub(super) fn stage(
    maps: &[KitMap],
    staging: &Path,
    payload: &mut Vec<Payload>,
) -> Result<Staged, Error> {
    let out = staging.join(FIREFIGHT_DIR);
    crate::private_dir(&out, false)?;
    let mut index = KitIndex { kit: firefight_kit::KIT_VERSION, maps: BTreeMap::new() };
    let mut refused = Vec::new();
    for map in maps {
        let mut base = Vec::new();
        map.source.open()?.read_to_end(&mut base)?;
        let dir = crate::kit_dir(&map.key);
        let built = firefight_kit::build_package(
            &base,
            &map.key,
            firefight_kit::Mode::Kit,
            &firefight_kit::Config::default(),
            &out,
            &dir,
            false,
        );
        let package = match built {
            Ok(package) => package,
            Err(error) => {
                eprintln!("host-data: no Firefight kit for {}: {error:#}", map.key);
                refused.push((map.key.clone(), format!("{error:#}")));
                continue;
            }
        };
        let mut names = Vec::new();
        for entry in fs::read_dir(out.join(&dir))? {
            names.push(entry?.file_name().into_string().map_err(|name| {
                Error::Bundle(format!("{dir} holds a non-UTF-8 file name {name:?}"))
            })?);
        }
        names.sort();
        for name in names {
            let path = out.join(&dir).join(&name);
            payload.push(Payload {
                path: format!("{FIREFIGHT_DIR}/{dir}/{name}"),
                size: fs::metadata(&path)?.len(),
                source: Source::File(path),
                sha256: String::new(),
            });
        }
        index.maps.insert(map.key.clone(), package.id.to_string());
    }
    if !index.maps.is_empty() {
        let json = serde_json::to_vec_pretty(&index)
            .map_err(|e| Error::Bundle(format!("writing {KIT_INDEX}: {e}")))?;
        let path = out.join(KIT_INDEX);
        let mut file = crate::new_private_file(&path)?;
        file.write_all(&json)?;
        file.sync_all()?;
        payload.push(Payload {
            path: format!("{FIREFIGHT_DIR}/{KIT_INDEX}"),
            size: json.len() as u64,
            source: Source::File(path),
            sha256: String::new(),
        });
    }
    payload.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Staged { index, refused })
}
