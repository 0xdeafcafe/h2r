//! `h2 firefight kit`: build a Firefight package from the user's disc.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use vfs::Backend;
use firefight_kit::{ClassSpec, Classes, Config, Donor, Mode, Team};
use tags::schema::Schema;
use scenario_edit::{MapSource, recipe};

const USAGE: &str =
    "usage: h2 firefight kit MAP [--home red|blue] [--disc ISO_OR_EXTRACTED_DISC] [--data DIR] [--probe] [--replace]
    [--retail-classes] [--class NAME=DONOR:CHAR_PATH]... [--classes FILE] [--cut]
    [--layout] [--layout-report]
  builds <data>/maps/firefight-MAP from a multiplayer map on the disc, e.g. beavercreek.
  --home picks the map team whose base is the home base (default red): the
    players' base, or team 0's with two teams. Drops keep off both bases.
  --layout takes the drops from the map's authored layout (coagulation,
    zanzibar) instead of the generator's picks. A drop the map has no
    usable sector for fails the build by name.
  --layout-report prints, and builds nothing: every netgame flag, start
    cluster and weapon pad of the map with its kind, team and index; then
    each of the map's layout anchors with the point it resolves to, the
    sector it snaps to, its distance to each base, each refusal by name
    and the build's cost table.
  --probe builds the firefight-probe test map with one squad of two Elites.
  --replace updates an existing package of the same name.
  --class copies a character class from a campaign map on the disc: DONOR is
    the disc map's name (e.g. 01b_spacestation), CHAR_PATH the `char` tag's
    name in it. The list is in keep order (the first is the last cut); the
    palette follows it after the Elite. --classes reads one such class per
    line from FILE (blank lines and # comments are skipped).
  --retail-classes puts the retail class table first: the Grunt, Jackal,
    Drone, Brute and Hunter, each copied from a campaign map on your disc.
  A map short of tag headroom merges its sectors, then keeps six drops,
    before anything else; the build says which it took.
  --cut drops classes from the end of the list until the build fits, instead
    of refusing it.";

struct Args {
    base: String,
    disc: Option<PathBuf>,
    data: Option<PathBuf>,
    probe: bool,
    replace: bool,
    config: Config,
    classes: Vec<Wanted>,
    /// `--retail-classes`: the retail class table before `classes`.
    retail: bool,
    cut: bool,
    /// `--layout-report`: print the census and build nothing.
    report: bool,
}

/// One requested class: a donor disc map and a `char` tag in it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Wanted {
    name: String,
    donor: String,
    path: String,
}

/// `NAME=DONOR:CHAR_PATH`.
fn parse_class(text: &str) -> Result<Wanted> {
    let bad = || format!("a class is NAME=DONOR:CHAR_PATH, got {text:?}");
    let (name, rest) = text.split_once('=').with_context(bad)?;
    let (donor, path) = rest.split_once(':').with_context(bad)?;
    let (name, donor, path) = (name.trim(), donor.trim().to_ascii_lowercase(), path.trim());
    let donor = donor.strip_suffix(".map").unwrap_or(&donor).to_owned();
    if name.is_empty() || path.is_empty() || !content::manifest::is_map_name(&donor) {
        bail!("{}", bad());
    }
    Ok(Wanted {
        name: name.to_owned(),
        donor,
        path: path.to_owned(),
    })
}

/// A class table: one class per line, `#` comments and blank lines skipped.
fn parse_table(text: &str) -> Result<Vec<Wanted>> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|(i, l)| parse_class(l).with_context(|| format!("class table line {}", i + 1)))
        .collect()
}

/// The retail class table ([`firefight_kit::retail`]) with each class's
/// first donor `on_disc` says the disc has.
fn retail_wanted(on_disc: impl FnMut(&str) -> bool) -> Result<Vec<Wanted>> {
    Ok(firefight_kit::retail::choose(on_disc)
        .map_err(anyhow::Error::msg)?
        .into_iter()
        .map(|(name, donor, path)| Wanted {
            name: name.to_owned(),
            donor: donor.to_owned(),
            path: path.to_owned(),
        })
        .collect())
}

/// The donor maps the list names, first use first, and each class as a
/// spec indexing them. A name used twice is an error: palette rows and
/// costs are by name.
fn plan(wanted: &[Wanted]) -> Result<(Vec<String>, Vec<ClassSpec>)> {
    let mut donors: Vec<String> = Vec::new();
    let mut specs: Vec<ClassSpec> = Vec::new();
    for w in wanted {
        if specs.iter().any(|s| s.name == w.name) {
            bail!("class {} is listed twice", w.name);
        }
        let donor = match donors.iter().position(|d| *d == w.donor) {
            Some(i) => i,
            None => {
                donors.push(w.donor.clone());
                donors.len() - 1
            }
        };
        specs.push(ClassSpec {
            name: w.name.clone(),
            donor,
            path: w.path.clone(),
        });
    }
    Ok((donors, specs))
}

/// The class list over donors already read, `names` and `bytes` in step.
fn classes_from<'a>(
    schema: &'a Schema,
    names: &'a [String],
    bytes: &'a [Vec<u8>],
    specs: Vec<ClassSpec>,
    cut_to_fit: bool,
) -> Classes<'a> {
    Classes {
        schema,
        donors: names
            .iter()
            .zip(bytes)
            .map(|(name, bytes)| Donor { name, bytes })
            .collect(),
        specs,
        cut_to_fit,
    }
}

fn read_disc_map(disc: &std::path::Path, name: &str) -> Result<Vec<u8>> {
    let file = vfs::open_disc(disc)
        .with_context(|| format!("opening the disc {}", disc.display()))?
        .host_file(&format!("maps\\{name}.map"))
        .with_context(|| format!("{} has no maps/{name}.map", disc.display()))?;
    Ok(MapSource::region(file.path, Some((file.offset, file.len))).read_base()?)
}

fn parse(words: Vec<String>) -> Result<Option<Args>> {
    let mut it = words.into_iter();
    match it.next().as_deref() {
        Some("kit") => {}
        Some("-h" | "--help") => return Ok(None),
        _ => bail!("{USAGE}"),
    }
    let mut base = None;
    let mut disc = None;
    let mut data = None;
    let mut probe = false;
    let mut replace = false;
    let mut config = Config::default();
    let mut classes = Vec::new();
    let mut retail = false;
    let mut cut = false;
    let mut report = false;
    let mut layout = false;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--probe" => probe = true,
            "--replace" => replace = true,
            "--cut" => cut = true,
            "--layout" => layout = true,
            "--layout-report" => report = true,
            "--retail-classes" => retail = true,
            "--class" => {
                classes.push(parse_class(&it.next().context("--class requires NAME=DONOR:CHAR_PATH")?)?)
            }
            "--classes" => {
                let file = it.next().context("--classes requires a file")?;
                let text = std::fs::read_to_string(&file)
                    .with_context(|| format!("reading the class table {file}"))?;
                classes.extend(parse_table(&text)?);
            }
            "--home" => {
                let team = it.next().context("--home requires red or blue")?;
                config.home = Team::parse(&team)
                    .with_context(|| format!("--home takes red or blue, got {team:?}"))?;
            }
            "--disc" => disc = Some(PathBuf::from(it.next().context("--disc requires a path")?)),
            "--data" => {
                data = Some(PathBuf::from(
                    it.next().context("--data requires a directory")?,
                ))
            }
            name if base.is_none() && !name.starts_with('-') => {
                let name = name.to_ascii_lowercase();
                let name = name.strip_suffix(".map").unwrap_or(&name);
                if !content::manifest::is_map_name(name) {
                    bail!("expected a disc map name such as coagulation, got {arg:?}");
                }
                base = Some(name.to_owned());
            }
            _ => bail!("unexpected argument {arg:?}\n{USAGE}"),
        }
    }
    let base = base.context(USAGE)?;
    if layout {
        config.layout = Some(
            firefight_kit::builtin::for_map(&base)
                .with_context(|| format!("{base} has no authored layout"))?,
        );
    }
    Ok(Some(Args {
        base,
        disc,
        data,
        probe,
        replace,
        config,
        classes,
        retail,
        cut,
        report,
    }))
}

pub fn run(words: Vec<String>) -> Result<()> {
    let Some(args) = parse(words)? else {
        println!("{USAGE}");
        return Ok(());
    };
    if args.report {
        let disc = args.disc.map(Ok).unwrap_or_else(crate::default_disc)?;
        let bytes = read_disc_map(&disc, &args.base)?;
        print!(
            "{}",
            firefight_kit::builtin::layout_report(&bytes, &args.base, &args.config)?
        );
        return Ok(());
    }
    let (mode, dir) = if args.probe {
        let recipe = recipe::built_in_named("firefight-probe")?.on_base(&args.base)?;
        (Mode::Probe, recipe.package_dir())
    } else {
        (Mode::Kit, format!("firefight-{}", args.base))
    };
    let disc = args.disc.map(Ok).unwrap_or_else(crate::default_disc)?;
    let bytes = read_disc_map(&disc, &args.base)?;
    if args.probe && (!args.classes.is_empty() || args.retail || args.cut) {
        bail!("--probe takes no classes");
    }
    let mut wanted = Vec::new();
    if args.retail {
        let mut files = vfs::open_disc(&disc)
            .with_context(|| format!("opening the disc {}", disc.display()))?;
        wanted = retail_wanted(|donor| files.host_file(&format!("maps\\{donor}.map")).is_some())?;
    }
    wanted.extend(args.classes);
    let (donor_names, specs) = plan(&wanted)?;
    let donor_bytes = donor_names
        .iter()
        .map(|name| read_disc_map(&disc, name))
        .collect::<Result<Vec<_>>>()?;
    let classes = (!specs.is_empty())
        .then(|| classes_from(Schema::builtin(), &donor_names, &donor_bytes, specs, args.cut));
    let data = args.data.unwrap_or_else(content::store::default_data_root);
    let maps = content::store::maps_dir(&data);
    let saved = firefight_kit::build_package_with(
        &bytes,
        &args.base,
        mode,
        &args.config,
        classes.as_ref(),
        &maps,
        &dir,
        args.replace,
    )?;
    println!("{}", maps.join(dir).display());
    println!("MapId {}", saved.id);
    println!("{} drop zones, {} squads", saved.zones, saved.squads);
    let [home, other] = saved.bases.points;
    println!(
        "home base ({:?} team) at {home:?}, other base at {other:?}, from the {:?}",
        saved.bases.home, saved.bases.source
    );
    println!("{}", saved.costs);
    for c in &saved.classes {
        println!(
            "class {}: {} tags, {} sounds, {} bytes of tag memory",
            c.name, c.tags, c.sounds, c.bytes
        );
    }
    for step in &saved.fitted {
        println!("fit: {step}: {} (short of tag headroom)", step.reason());
    }
    for name in &saved.cut {
        println!("cut {name} (over the tag headroom)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &str) -> Result<Option<Args>> {
        parse(words.split_whitespace().map(str::to_owned).collect())
    }

    #[test]
    fn the_probe_builds_on_a_named_disc_map() {
        let a = args("kit Lockout.map --probe --disc disc.iso --data own-data --replace")
            .unwrap()
            .unwrap();
        assert_eq!(a.base, "lockout");
        assert!(a.probe && a.replace);
        assert_eq!(a.disc.unwrap(), PathBuf::from("disc.iso"));
        assert_eq!(a.data.unwrap(), PathBuf::from("own-data"));
        assert!(!args("kit lockout").unwrap().unwrap().probe);
        assert_eq!(args("kit lockout").unwrap().unwrap().config.home, Team::Red);
        for bad in [
            "kit",
            "kit ../some-file",
            "kit one two",
            "kit one --disc",
            "kit one --home",
            "kit one --home green",
        ] {
            assert!(args(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_layout_options_name_the_maps_authored_layout() {
        let plain = args("kit coagulation").unwrap().unwrap();
        assert!(plain.config.layout.is_none() && !plain.report);
        let a = args("kit coagulation --layout").unwrap().unwrap();
        assert_eq!(a.config.layout.unwrap().map, "coagulation");
        let r = args("kit Zanzibar.map --layout-report").unwrap().unwrap();
        assert!(r.report && r.config.layout.is_none());
        // A map with no layout is named, not quietly built without one.
        let err = args("kit lockout --layout").err().unwrap();
        assert!(format!("{err:#}").contains("lockout has no authored layout"));
    }

    #[test]
    fn the_home_option_picks_a_team() {
        let a = args("kit beavercreek --home blue").unwrap().unwrap();
        assert_eq!(a.base, "beavercreek");
        assert_eq!(a.config.home, Team::Blue);
        let red = args("kit beavercreek --home Red").unwrap().unwrap();
        assert_eq!(red.config.home, Team::Red);
    }

    #[test]
    fn classes_are_named_by_donor_and_char_path() {
        let a = args(
            "kit beavercreek --class grunt=01b_Spacestation.map:characters\\grunt\\grunt --cut",
        )
        .unwrap()
        .unwrap();
        assert!(a.cut);
        assert_eq!(
            a.classes,
            vec![Wanted {
                name: "grunt".into(),
                donor: "01b_spacestation".into(),
                path: "characters\\grunt\\grunt".into(),
            }]
        );
        for bad in [
            "kit one --class grunt",
            "kit one --class grunt=donor",
            "kit one --class =d:p",
            "kit one --class g=:p",
            "kit one --class g=../d:p",
            "kit one --class g=d:",
            "kit one --class",
            "kit one --classes /no/such/class-table",
        ] {
            assert!(args(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_retail_table_comes_first_from_the_discs_own_maps() {
        let a = args("kit zanzibar --retail-classes --class mine=03a_oldmombasa:x --cut")
            .unwrap()
            .unwrap();
        assert!(a.retail && a.cut);
        assert!(!args("kit zanzibar").unwrap().unwrap().retail);
        let mut wanted = retail_wanted(|_| true).unwrap();
        let names: Vec<_> = wanted.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["grunt", "jackal", "drone", "brute", "hunter"]);
        assert_eq!(wanted[3].donor, "08a_deltacliffs");
        assert_eq!(wanted[0].path, r"objects\characters\grunt\ai\grunt");
        // Outskirts gives four classes: one donor read for them.
        wanted.extend(a.classes);
        let (donors, specs) = plan(&wanted).unwrap();
        assert_eq!(donors, ["03a_oldmombasa", "08a_deltacliffs"]);
        assert_eq!(specs.len(), 6);
        // A class listed by hand under a table name is refused.
        let twice = [retail_wanted(|_| true).unwrap(), parse_table("grunt=a:b").unwrap()].concat();
        assert!(plan(&twice).is_err());
        // A disc without any of a class's donors is refused by class.
        let error = retail_wanted(|d| d.starts_with("03")).unwrap_err().to_string();
        assert!(error.contains("brute"), "{error}");
    }

    #[test]
    fn a_table_skips_comments_and_names_its_bad_line() {
        let table = "# invented\n\ngrunt=a:c\\grunt\n  jackal = b : c\\jackal \n";
        let list = parse_table(table).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!((list[1].name.as_str(), list[1].donor.as_str()), ("jackal", "b"));
        assert_eq!(list[1].path, "c\\jackal");
        let error = format!("{:#}", parse_table("a=b:c\nnope\n").unwrap_err());
        assert!(error.contains("line 2"), "{error}");
    }

    #[test]
    fn the_plan_shares_donors_in_first_use_order() {
        let list = parse_table("g=a:x\nj=b:y\nb=a:z\n").unwrap();
        let (donors, specs) = plan(&list).unwrap();
        assert_eq!(donors, ["a", "b"]);
        let at: Vec<_> = specs.iter().map(|s| (s.name.as_str(), s.donor)).collect();
        assert_eq!(at, [("g", 0), ("j", 1), ("b", 0)]);
        assert_eq!(specs[2].path, "z");
        let twice = parse_table("g=a:x\ng=b:y\n").unwrap();
        assert!(plan(&twice).unwrap_err().to_string().contains("twice"));
        assert!(plan(&[]).unwrap().1.is_empty());
    }

    #[test]
    fn the_class_list_keeps_donor_bytes_and_order() {
        let list = parse_table("g=a:x\nj=b:y\n").unwrap();
        let (names, specs) = plan(&list).unwrap();
        let bytes = vec![vec![1u8], vec![2, 2]];
        let schema = Schema::builtin();
        let with = classes_from(schema, &names, &bytes, specs, true);
        assert!(with.cut_to_fit);
        assert_eq!(with.donors[1].name, "b");
        assert_eq!(with.donors[1].bytes, &[2, 2]);
        assert_eq!(with.specs[1].name, "j");
    }
}
