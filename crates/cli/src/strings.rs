//! Localized text inspection. No dump-file option: text is written only to stdout.
use anyhow::{Context, Result, bail};
use map::{FourCC, Map, TagQuery};
use std::{
    collections::HashSet,
    io::{Read, Seek},
    path::PathBuf,
};
use tags::{
    Language, Localization,
    localization::{LocaleEntry, LocaleTable},
};

const USAGE: &str = "usage: h2 strings <map-name-or-file> [--lang en] [--tag name] [--disc disc.iso]\n       h2 strings --all [--disc disc.iso]\n\nLanguages: en ja de fr es it ko zh pt. --all validates every map and all nine languages; prints counts only.";

#[derive(Debug)]
struct Options {
    map: Option<String>,
    disc: Option<PathBuf>,
    language: Language,
    tag: Option<String>,
    all: bool,
    help: bool,
}
fn parse(args: Vec<String>) -> Result<Options> {
    let mut args = args.into_iter();
    let mut options = Options {
        map: None,
        disc: None,
        language: Language::English,
        tag: None,
        all: false,
        help: false,
    };
    let mut language_set = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--disc" if options.disc.is_none() => {
                options.disc = Some(option_value(&mut args, "--disc")?.into())
            }
            "--lang" if !language_set => {
                options.language = option_value(&mut args, "--lang")?.parse()?;
                language_set = true;
            }
            "--tag" if options.tag.is_none() => {
                options.tag = Some(option_value(&mut args, "--tag")?)
            }
            "--all" if !options.all => options.all = true,
            "-h" | "--help" => options.help = true,
            a if a.starts_with('-') => bail!("unknown or repeated option {a}\n{USAGE}"),
            _ if options.map.is_none() => options.map = Some(arg),
            _ => bail!("unexpected argument {arg}\n{USAGE}"),
        }
    }
    if options.help {
        return Ok(options);
    }
    if options.all {
        if options.map.is_some() || options.tag.is_some() || language_set {
            bail!("--all cannot be combined with a map, --tag or --lang\n{USAGE}");
        }
    } else if options.map.is_none() {
        bail!("{USAGE}");
    }
    Ok(options)
}
fn option_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    let value = args
        .next()
        .with_context(|| format!("{flag} requires a value"))?;
    if value.starts_with('-') {
        bail!("{flag} requires a value, got flag {value}");
    }
    Ok(value)
}
pub(super) fn run(args: Vec<String>) -> Result<()> {
    let options = parse(args)?;
    if options.help {
        println!("{USAGE}");
        return Ok(());
    }
    if options.all {
        return validate_all(options.disc);
    }
    let mut map = super::maps::load_map(options.map.as_deref().unwrap(), options.disc)?;
    let localization = Localization::read(&mut map)?;
    let table = localization.table(options.language);
    let (start, count) = if let Some(name) = options.tag {
        let id = map.tag(TagQuery::NameInGroup(&name, FourCC(*b"unic")))?.id;
        let unic = localization.unic(&mut map, id)?;
        let range = unic.ranges[options.language as usize];
        (
            if range.count == 0 {
                0
            } else {
                range.start as usize
            },
            range.count as usize,
        )
    } else {
        (0, table.entries().len())
    };
    println!(
        "row   string_id   name => text ({})",
        options.language.code()
    );
    for row in start..start + count {
        let entry = table.entries()[row];
        let name = entry
            .id
            .resolve(map.strings())
            .ok()
            .flatten()
            .unwrap_or("<unresolved name>");
        // Debug quoting escapes line breaks, controls, quotes, and terminal escapes.
        println!(
            "{row:5} {:#010x} {name:?} => {:?}",
            entry.id.0,
            table.text_at(row).unwrap()
        );
    }
    println!(
        "{count} strings; {} non-UTF-8 entries in this language (engine conversion applied)",
        table.non_utf8_entries()
    );
    Ok(())
}

#[derive(Default)]
struct Counts {
    ids: usize,
    rows: usize,
    unics: usize,
    scoped: usize,
    non_utf8: usize,
    failures: usize,
}
// These checks are stricter than lookup: stripped/malformed names and duplicate
// keys can still be looked up by the engine, but --all should flag them.
fn validate_name_ids(table: &LocaleTable, names: &[Option<String>]) -> Result<()> {
    for (row, entry) in table.entries().iter().enumerate() {
        entry
            .id
            .resolve(names)
            .with_context(|| format!("locale row {row} key {:#010x}", entry.id.0))?
            .with_context(|| format!("locale row {row} has null name ID {:#010x}", entry.id.0))?;
    }
    Ok(())
}
fn validate_unique_ids(entries: &[LocaleEntry]) -> Result<()> {
    let mut seen = HashSet::new();
    for entry in entries {
        if !seen.insert(entry.id.0) {
            bail!(
                "duplicate ID {:#010x} within unic language range",
                entry.id.0
            );
        }
    }
    Ok(())
}
fn validate<R: Read + Seek>(map: &mut Map<R>) -> Result<Counts> {
    let localization = Localization::read(map)?;
    let mut counts = Counts {
        ids: map.strings().len(),
        ..Counts::default()
    };
    for language in Language::ALL {
        let table = localization.table(language);
        counts.rows += table.entries().len();
        counts.non_utf8 += table.non_utf8_entries();
        if let Err(error) = validate_name_ids(table, map.strings()) {
            eprintln!("{} {}: {error:#}", map.header().name, language.code());
            counts.failures += 1;
        }
    }
    let unics: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"unic"))
        .map(|t| t.id)
        .collect();
    for id in unics {
        counts.unics += 1;
        let checked = (|| -> Result<()> {
            let unic = localization.unic(map, id)?;
            for language in Language::ALL {
                let entries = localization.entries_for(&unic, language)?;
                counts.scoped += entries.len();
                validate_unique_ids(entries)
                    .with_context(|| format!("{} range", language.code()))?;
            }
            Ok(())
        })();
        if let Err(error) = checked {
            eprintln!("{} unic {id}: {error:#}", map.header().name);
            counts.failures += 1;
        }
    }
    Ok(counts)
}
fn validate_all(disc: Option<PathBuf>) -> Result<()> {
    let disc = disc.map(Ok).unwrap_or_else(super::default_disc)?;
    let mut iso = super::open_iso(&disc)?;
    let mut total = Counts::default();
    let mut maps = 0;
    println!(
        "map                          ids    unic  locale rows  scoped rows  non-UTF8  failures"
    );
    for entry in iso
        .entries()?
        .into_iter()
        .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
    {
        maps += 1;
        let result = super::maps::disc_map(&disc, iso.partition_offset(), &entry)
            .and_then(|mut m| validate(&mut m));
        match result {
            Ok(c) => {
                println!(
                    "{:<28} {:>6} {:>6} {:>12} {:>12} {:>9} {:>9}",
                    entry.path.rsplit('/').next().unwrap(),
                    c.ids,
                    c.unics,
                    c.rows,
                    c.scoped,
                    c.non_utf8,
                    c.failures
                );
                total.ids += c.ids;
                total.unics += c.unics;
                total.rows += c.rows;
                total.scoped += c.scoped;
                total.non_utf8 += c.non_utf8;
                total.failures += c.failures;
            }
            Err(error) => {
                eprintln!("{}: {error:#}", entry.path);
                total.failures += 1;
            }
        }
    }
    if maps == 0 {
        bail!("no maps found on disc");
    }
    println!(
        "{maps} maps, {} string-name slots, {} unic tags, {} locale rows, {} scoped rows, {} non-UTF-8 entries, {} failures",
        total.ids, total.unics, total.rows, total.scoped, total.non_utf8, total.failures
    );
    if total.failures != 0 {
        bail!("localization validation failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn parses_explicit_scopes_and_rejects_silently_ignored_options() {
        let o = parse(args(&["mainmenu", "--lang", "ja", "--tag", "ui\\demo"])).unwrap();
        assert_eq!(o.language, Language::Japanese);
        assert_eq!(o.tag.as_deref(), Some("ui\\demo"));
        assert!(
            parse(args(&["--all", "--disc", "invented.iso"]))
                .unwrap()
                .all
        );
        for a in [
            vec![],
            vec!["--all", "mainmenu"],
            vec!["--all", "--lang", "en"],
            vec!["mainmenu", "--lang", "xx"],
            vec!["mainmenu", "--tag"],
            vec!["mainmenu", "--all", "--tag", "ui"],
            vec!["--all", "--all"],
            vec!["mainmenu", "--lang", "en", "--lang", "ja"],
        ] {
            assert!(parse(args(&a)).is_err(), "{a:?}");
        }
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }
    fn table(keys: &[u32]) -> LocaleTable {
        let index: Vec<u8> = keys
            .iter()
            .flat_map(|key| [key.to_le_bytes(), 0u32.to_le_bytes()].concat())
            .collect();
        LocaleTable::decode(&index, b"invented\0".to_vec()).unwrap()
    }
    #[test]
    fn flag_values_cannot_swallow_all_or_help() {
        for option in ["--tag", "--lang", "--disc"] {
            for flag in ["--all", "-h", "--help"] {
                let error = parse(args(&["demo", option, flag]))
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("requires a value"), "{error}");
                assert!(run(args(&["demo", option, flag])).is_err());
            }
        }
        assert!(parse(args(&["-h"])).unwrap().help);
        assert!(
            parse(args(&["demo", "--tag", "ui\\demo", "-h"]))
                .unwrap()
                .help
        );
        assert!(run(args(&["--help"])).is_ok());
    }
    #[test]
    fn validation_detects_duplicate_ids_only_within_each_scope() {
        let table = table(&[1, 2, 1]);
        assert!(validate_unique_ids(table.entries()).is_err());
        assert!(validate_unique_ids(&table.entries()[..2]).is_ok());
        assert!(validate_unique_ids(&table.entries()[2..]).is_ok());
    }
    #[test]
    fn validation_reports_name_index_length_and_null_failures_separately() {
        let names = [Some("key".into())];
        assert!(validate_name_ids(&table(&[0x03000000]), &names).is_ok());
        for (key, message) in [
            (0x02000000, "string ID length"),
            (0x03000001, "string ID index"),
            (u32::MAX, "null name ID"),
        ] {
            let error = format!(
                "{:#}",
                validate_name_ids(&table(&[key]), &names).unwrap_err()
            );
            assert!(error.contains(message), "{error}");
            if key != u32::MAX {
                assert!(!error.contains("null name ID"));
            }
        }
    }
    #[test]
    fn top_level_usage_aligns_strings_with_maps() {
        let description_column = super::super::USAGE
            .lines()
            .find(|s| s.starts_with("  maps "))
            .unwrap()
            .find("map headers")
            .unwrap();
        let strings = super::super::USAGE
            .lines()
            .find(|s| s.starts_with("  strings "))
            .unwrap();
        assert_eq!(strings.find("localized"), Some(description_column));
    }
}
