//! The `kit` builder: a named code builder of ours (docs/editor/studio-redesign.md,
//! "Contents"; "Each content path as a package").
//!
//! A `kit` item's `source` is its settings, `kits/<name>.kit.yaml`; the file's
//! name says which kit it is. The registry of kits is closed:
//!
//! | Kit | Adds | Where |
//! |---|---|---|
//! | `campaign-cards` | our campaigns' level pictures, as new `bitm` tags in mainmenu's level select (`game::campaign_cards`) | the file `maps\mainmenu.map` |
//! | `glyphs` | the bot mark's gear, the Mods list's marks, the HUD's arrows and rings and the superbounce medals in every font file (`game::bot_mark`), after the button icon sets when the boot has them | the files `maps\fonts\*` |
//!
//! Neither changes a map the way a tag item does: a kit's input is the
//! user's own data (the cards found under the data root, the icon choice),
//! which a package does not hold. So [`apply`] only reads the item again,
//! and [`PackageRewrite`](crate::PackageRewrite) runs the kits of the packages
//! in the set on the files they are for, with the inputs its boot gave it
//! ([`KitInputs`]). `changes_map` is false for a kit item.
use std::collections::BTreeSet;

use content::package::{Item, Kind, Package};
use serde::Deserialize;

use crate::build::{Cx, Failure};
use crate::builders::Job;

/// A kit the registry has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kit {
    CampaignCards,
    Glyphs,
}

/// The suffix of a kit's settings file.
pub const SETTINGS_SUFFIX: &str = ".kit.yaml";

impl Kit {
    pub const ALL: [Kit; 2] = [Kit::CampaignCards, Kit::Glyphs];

    pub fn name(self) -> &'static str {
        match self {
            Self::CampaignCards => "campaign-cards",
            Self::Glyphs => "glyphs",
        }
    }

    /// The kit a settings file's name says: `kits/glyphs.kit.yaml`.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// The kit `item` is: a `kit` item whose source is `<folder>/<name>.kit.yaml`.
pub fn kit_of(item: &Item) -> Result<Kit, String> {
    if item.kind != Kind::Kit {
        return Err("not a kit item".into());
    }
    let source = item.source.as_deref().ok_or_else(|| {
        format!("a kit item names its settings file: source = \"kits/<kit>{SETTINGS_SUFFIX}\"")
    })?;
    let file = source.rsplit('/').next().unwrap_or(source);
    let name = file
        .strip_suffix(SETTINGS_SUFFIX)
        .ok_or_else(|| format!("{source}: a kit's settings file ends in {SETTINGS_SUFFIX}"))?;
    Kit::named(name).ok_or_else(|| {
        let all: Vec<_> = Kit::ALL.iter().map(|k| k.name()).collect();
        format!("{name:?} is not a kit (the kits are {})", all.join(", "))
    })
}

/// Which kits the packages of a set carry.
pub fn kits_of(packages: &[&Package]) -> BTreeSet<Kit> {
    packages
        .iter()
        .flat_map(|p| &p.manifest.items)
        .filter_map(|i| kit_of(i).ok())
        .collect()
}

/// Neither kit has a setting yet; a file holds comments, or an empty map.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {}

fn read(job: &Job<'_>) -> Result<Kit, Failure> {
    let kit = kit_of(job.item)?;
    if job.item.tag.is_some() {
        return Err("a kit item adds no tag: drop `tag`".into());
    }
    let text = std::str::from_utf8(job.source).map_err(|e| format!("not text: {e}"))?;
    serde_yaml_ng::from_str::<Option<Settings>>(text).map_err(|e| Failure {
        line: e.location().map(|l| l.line()),
        why: format!("{} settings: {e}", kit.name()),
    })?;
    Ok(kit)
}

pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    read(job).map(drop)
}

/// Changes nothing in the map: the game runs the kit on its files
/// ([`crate::PackageRewrite`]). The item is read again so a build stops on
/// a kit that is not ours or a setting it does not have.
pub fn apply(_cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    read(job).map(drop)
}
