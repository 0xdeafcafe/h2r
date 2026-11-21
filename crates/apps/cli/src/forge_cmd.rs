//! `h2 forge build` and `h2 forge check`. Both materialise the document
//! against the user's disc. Check stops before writing the local package.

use std::{fs::File, path::PathBuf};

use anyhow::{Context, Result, bail};
use content::store::default_data_root;
use scenario_edit::MapSource;

const USAGE: &str = "usage: h2 forge build <doc.h2forge> [--disc <iso>] [--maps <dir>]
       h2 forge check <doc.h2forge> [--disc <iso>]";

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
    println!("MapId {}", built.manifest().map_id());
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
}
