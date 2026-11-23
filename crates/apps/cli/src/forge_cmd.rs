//! `h2 forge build` and `h2 forge check`. Both materialise the document
//! against the user's disc and print its tier (map variant or Forge map)
//! and each entry print they compared with the disc's row. Check stops
//! before writing the local package.

use std::{fs::File, path::PathBuf};

use anyhow::{Context, Result, bail};
use content::store::default_data_root;
use scenario_edit::MapSource;

const USAGE: &str = "usage: h2 forge build <doc.h2forge> [--disc <iso>] [--maps <dir>]
       h2 forge check <doc.h2forge> [--disc <iso>]
       h2 forge looks [--disc <iso>] [--json]";

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Build,
    Check,
}

#[derive(Debug, PartialEq, Eq)]
struct Args {
    action: Action,
    document: PathBuf,
    disc: Option<PathBuf>,
    maps: Option<PathBuf>,
}

fn parse(args: Vec<String>) -> Result<Args> {
    let mut args = args.into_iter();
    let action = match args.next().as_deref() {
        Some("build") => Action::Build,
        Some("check") => Action::Check,
        _ => bail!("{USAGE}"),
    };
    let document = args.next().filter(|s| !s.starts_with('-')).context(USAGE)?;
    let (mut disc, mut maps) = (None, None);
    while let Some(arg) = args.next() {
        let slot = match arg.as_str() {
            "--disc" if disc.is_none() => &mut disc,
            "--maps" if maps.is_none() && action == Action::Build => &mut maps,
            _ => bail!("unexpected argument {arg:?}\n{USAGE}"),
        };
        *slot = Some(PathBuf::from(
            args.next()
                .filter(|s| !s.starts_with("--"))
                .with_context(|| format!("{arg} needs a path"))?,
        ));
    }
    Ok(Args {
        action,
        document: document.into(),
        disc,
        maps,
    })
}

pub fn run(args: Vec<String>) -> Result<()> {
    if matches!(args.as_slice(), [arg] if matches!(arg.as_str(), "-h" | "--help")) {
        println!("{USAGE}");
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("looks") {
        return crate::forge_looks::run(args[1..].to_vec());
    }
    let args = parse(args)?;
    let file = File::open(&args.document)
        .with_context(|| format!("reading {}", args.document.display()))?;
    let document = forge_build::read_document(file)
        .with_context(|| format!("reading {}", args.document.display()))?;
    let disc = match args.disc {
        Some(path) => path,
        None => crate::default_disc()?,
    };
    let maps = MapSource::disc_maps(&disc)
        .with_context(|| format!("reading maps from {}", disc.display()))?;
    let built = forge_build::build(&document, |name| {
        maps.iter()
            .find(|(map, _)| map.eq_ignore_ascii_case(name))
            .map(|(_, source)| source.clone())
    })?;
    println!("Tier {}", document.tier().name());
    println!("MapId {}", built.manifest().map_id());
    for line in print_lines(&built.prints) {
        println!("{line}");
    }
    for block in &built.reserves {
        println!(
            "{}: {} active, {} reserved",
            block.block, block.active, block.spare
        );
    }
    if args.action == Action::Build {
        let maps = args
            .maps
            .unwrap_or_else(|| default_data_root().join("maps"));
        let path = built.write_package(&maps)?;
        println!("{}", path.display());
    }
    Ok(())
}

/// One line for each print compared: a print that differs stops the build
/// as `variant_print` before this is reached.
fn print_lines(prints: &[forge_build::Printed]) -> Vec<String> {
    prints
        .iter()
        .map(|p| format!("print {} {} {}: matches", p.block.name(), p.index, p.print))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &str) -> Vec<String> {
        words.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn forge_build_and_check_arguments() {
        let build = parse(args("build draft.h2forge --disc retail.iso --maps local")).unwrap();
        assert_eq!(build.action, Action::Build);
        assert_eq!(build.document, PathBuf::from("draft.h2forge"));
        assert_eq!(build.disc, Some("retail.iso".into()));
        assert_eq!(build.maps, Some("local".into()));

        let check = parse(args("check draft.h2forge")).unwrap();
        assert_eq!(check.action, Action::Check);
        assert_eq!(check.disc, None);
        for invalid in [
            "",
            "build",
            "check --disc retail.iso",
            "check draft --maps local",
            "build draft --disc",
            "build draft --maps a --maps b",
        ] {
            assert!(parse(args(invalid)).is_err(), "{invalid}");
        }
    }

    #[test]
    fn each_compared_print_is_one_line() {
        let prints = [
            forge_build::Printed {
                block: forge_doc::BlockKind::Weapons,
                index: 1,
                print: forge_doc::Print([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]),
            },
            forge_build::Printed {
                block: forge_doc::BlockKind::NetgameFlags,
                index: 0,
                print: forge_doc::Print([0; 8]),
            },
        ];
        assert_eq!(
            print_lines(&prints),
            vec![
                "print weapons 1 0123456789abcdef: matches",
                "print netgame_flags 0 0000000000000000: matches",
            ]
        );
        assert!(print_lines(&[]).is_empty());
    }
}
