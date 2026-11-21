//! `h2 forge looks`: the census of the models this disc can draw as a
//! Forge monitor (docs/game/forge-monitor-appearance.md, "Which model").
//!
//! The rules, the cache and the report live in `forge::look::census`; this
//! file reads the user's own disc for them. Nothing is written but the
//! census cache under the data root.
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use content::{Sha256, store::default_data_root};
use forge::look::census::Disc as _;
use forge::look::census::{self, Census, Described, MapInfo};
use map::{CacheType, FourCC, TagId, TagQuery};

use crate::live_art::Disc;
use crate::live_models::{MAX_BIPED_TRIANGLES, read_source};

const USAGE: &str = "usage: h2 forge looks [--disc <iso>] [--json]";
const BIPD: FourCC = FourCC(*b"bipd");
const LIGH: FourCC = FourCC(*b"ligh");

#[derive(Debug, PartialEq, Eq)]
struct Args {
    disc: Option<PathBuf>,
    json: bool,
}

fn parse(args: Vec<String>) -> Result<Args> {
    let mut args = args.into_iter();
    let (mut disc, mut json) = (None, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" if !json => json = true,
            "--disc" if disc.is_none() => {
                disc = Some(PathBuf::from(
                    args.next()
                        .filter(|s| !s.starts_with("--"))
                        .context("--disc needs a path")?,
                ))
            }
            _ => bail!("unexpected argument {arg:?}\n{USAGE}"),
        }
    }
    Ok(Args { disc, json })
}

/// The campaign maps of a scanned disc, read for the census.
struct DiscSource<'a> {
    disc: &'a Disc,
}

impl DiscSource<'_> {
    fn map(&self, info: &MapInfo) -> Result<&crate::live_art::MapFile> {
        self.disc
            .maps_of(CacheType::Campaign)
            .find(|m| m.name() == info.name)
            .ok_or_else(|| anyhow!("no campaign map {}", info.name))
    }

    fn describe_in(&self, info: &MapInfo, tag: &str) -> Result<Described> {
        let (mut map, mut files) = self.disc.open(self.map(info)?)?;
        let id = map.tag(TagQuery::NameInGroup(tag, BIPD))?.id;
        let ctx = tags::object::Context::read(&mut map)?;
        let object = ctx.resolve_object(id)?;
        let light = object
            .header
            .attachments
            .iter()
            .any(|a| a.kind.group == LIGH && a.kind.id != TagId::NULL);
        let model = object.model.as_ref().context("the biped has no model")?;
        let variants = model.tag.layers[0]
            .elements("variants")
            .iter()
            .filter_map(|v| v.sid("name").and_then(|s| s.1.map(str::to_owned)))
            .collect();
        let render = model
            .render_model
            .as_ref()
            .context("the model has no render model")?;
        let source = read_source(&mut map, &mut files, render.id, MAX_BIPED_TRIANGLES)?;
        Ok(Described {
            variants,
            change_colours: source.change_colour.iter().any(Option::is_some),
            light,
            nodes: u32::try_from(source.bones.len())?,
            triangles: u32::try_from(
                source
                    .pieces
                    .iter()
                    .map(|p| p.triangles.len())
                    .sum::<usize>(),
            )?,
        })
    }
}

impl census::Disc for DiscSource<'_> {
    fn campaign_maps(&self) -> Result<Vec<MapInfo>, String> {
        self.disc
            .maps_of(CacheType::Campaign)
            .map(|m| {
                let bytes = self
                    .disc
                    .read(m.path())
                    .map_err(|e| format!("{}: {e:#}", m.name()))?;
                Ok(MapInfo {
                    name: m.name().to_owned(),
                    sha256: Sha256::of(&bytes).to_string(),
                })
            })
            .collect()
    }

    fn bipeds(&self, info: &MapInfo) -> Result<Vec<String>, String> {
        let (map, _) = self
            .map(info)
            .and_then(|m| self.disc.open(m))
            .map_err(|e| format!("{e:#}"))?;
        Ok(map
            .tags()
            .iter()
            .filter(|t| t.group == BIPD && t.location.is_some())
            .filter_map(|t| t.name.clone())
            .collect())
    }

    fn describe(&self, info: &MapInfo, tag: &str) -> Result<Described, String> {
        self.describe_in(info, tag).map_err(|e| format!("{e:#}"))
    }
}

/// The census of the disc at `iso`, from the cache when the campaign maps'
/// hashes are those it was made from.
fn census_of(iso: &std::path::Path) -> Result<Census> {
    let disc = Disc::scan(iso).with_context(|| format!("reading {}", iso.display()))?;
    let source = DiscSource { disc: &disc };
    let cache = default_data_root().join("cache");
    let maps = census::Disc::campaign_maps(&source).map_err(|e| anyhow!(e))?;
    let key = census::cache_key(&maps);
    if let Some(cached) = census::load_cached(&cache, &key) {
        return Ok(cached);
    }
    let found = census::run(&Hashed { source, maps }).map_err(|e| anyhow!(e))?;
    if let Err(e) = census::store_cached(&cache, &key, &found) {
        eprintln!("forge looks: census not cached: {e}");
    }
    Ok(found)
}

/// A source whose map hashes are already read, so the census does not read
/// every campaign map twice.
struct Hashed<'a> {
    source: DiscSource<'a>,
    maps: Vec<MapInfo>,
}

impl census::Disc for Hashed<'_> {
    fn campaign_maps(&self) -> Result<Vec<MapInfo>, String> {
        Ok(self.maps.clone())
    }
    fn bipeds(&self, map: &MapInfo) -> Result<Vec<String>, String> {
        self.source.bipeds(map)
    }
    fn describe(&self, map: &MapInfo, tag: &str) -> Result<Described, String> {
        self.source.describe(map, tag)
    }
}

pub fn run(args: Vec<String>) -> Result<()> {
    if matches!(args.as_slice(), [arg] if matches!(arg.as_str(), "-h" | "--help")) {
        println!("{USAGE}");
        return Ok(());
    }
    let args = parse(args)?;
    let iso = match args.disc {
        Some(path) => path,
        None => crate::default_disc()?,
    };
    let found = census_of(&iso)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&found.to_value())?);
    } else {
        print!("{}", report(&found));
    }
    Ok(())
}

/// The census as a table, then what was left out and why.
fn report(found: &Census) -> String {
    let mut out = format!(
        "{:<24} {:<24} {:<12} {:>8} {:>7} {:>6} {:>6} {:>9}\n",
        "KEY", "NAME", "SOURCE", "VARIANTS", "COLOURS", "LIGHT", "NODES", "TRIANGLES"
    );
    for e in &found.entries {
        out += &format!(
            "{:<24} {:<24} {:<12} {:>8} {:>7} {:>6} {:>6} {:>9}\n",
            e.key,
            e.name,
            e.source,
            e.variants.len(),
            if e.change_colours { "yes" } else { "no" },
            if e.light { "yes" } else { "no" },
            e.nodes,
            e.triangles
        );
    }
    out += &format!("{} models\n", found.entries.len());
    for s in &found.skipped {
        out += &format!("skipped {}: {}\n", s.what, s.reason);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge::look::census::{Entry, Skipped};

    fn args(words: &str) -> Vec<String> {
        words.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn looks_arguments() {
        assert_eq!(
            parse(args("--disc retail.iso --json")).unwrap(),
            Args {
                disc: Some("retail.iso".into()),
                json: true
            }
        );
        assert_eq!(
            parse(args("")).unwrap(),
            Args {
                disc: None,
                json: false
            }
        );
        for bad in [
            "--disc",
            "--disc --json",
            "--json --json",
            "--maps x",
            "extra",
        ] {
            assert!(parse(args(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_report_lists_each_model_then_what_was_skipped() {
        let found = Census {
            entries: vec![Entry {
                key: "widget".into(),
                name: "widget".into(),
                source: "map_a".into(),
                sha256: "a".repeat(64),
                variants: vec!["blue".into(), "red".into()],
                change_colours: true,
                light: false,
                nodes: 12,
                triangles: 3400,
            }],
            skipped: vec![Skipped {
                what: "loose".into(),
                reason: "its directory is not a model key".into(),
            }],
        };
        let text = report(&found);
        let row = text.lines().nth(1).unwrap();
        for part in ["widget", "map_a", "yes", "no", "12", "3400"] {
            assert!(row.contains(part), "{row}");
        }
        assert!(text.contains("1 models"));
        assert!(text.contains("skipped loose: its directory is not a model key"));
    }
}
