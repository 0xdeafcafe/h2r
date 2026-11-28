//! `h2 pogo`: the pogo stick's kit, looked at outside the game.
//!
//! `h2 pogo kit MAP` adds the kit to a multiplayer map of the user's disc
//! as the game does when it serves the map (`content_kit::inject`), and says
//! what it took. `h2 pogo models` writes the three looks as OBJ files.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use boot::new_maps::{Disc, IsoDisc};
use content_kit::Style;
use content_kit::source::Source;
use scenario_edit::MapSource;

const USAGE: &str = "usage: h2 pogo kit MAP [--out FILE] [--disc ISO_OR_EXTRACTED_DISC]
       h2 pogo models [--out DIR]
  kit adds the pogo stick's three looks to a multiplayer map of the disc, e.g.
    lockout, as the game does when it serves the map, and prints the weapon
    tags and the tag memory taken. --out writes the map built (a file of the
    user's own data: keep it out of any repository).
  models writes each look's mesh as DIR/pogo_<look>.obj (default: the
    current directory), to look at in a model viewer.";

#[derive(Debug, PartialEq)]
enum Args {
    Kit {
        map: String,
        out: Option<PathBuf>,
        disc: Option<PathBuf>,
    },
    Models {
        out: PathBuf,
    },
}

fn parse(words: Vec<String>) -> Result<Option<Args>> {
    let mut it = words.into_iter();
    let verb = match it.next().as_deref() {
        Some("kit") => "kit",
        Some("models") => "models",
        Some("-h" | "--help") => return Ok(None),
        _ => bail!("{USAGE}"),
    };
    let mut map = None;
    let mut out = None;
    let mut disc = None;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--out" => out = Some(PathBuf::from(it.next().context("--out requires a path")?)),
            "--disc" if verb == "kit" => {
                disc = Some(PathBuf::from(it.next().context("--disc requires a path")?))
            }
            name if verb == "kit" && map.is_none() && !name.starts_with('-') => {
                let name = name.to_ascii_lowercase();
                let name = name.strip_suffix(".map").unwrap_or(&name);
                if !content::manifest::is_map_name(name) {
                    bail!("expected a disc map name such as lockout, got {arg:?}");
                }
                map = Some(name.to_owned());
            }
            _ => bail!("unexpected argument {arg:?}\n{USAGE}"),
        }
    }
    Ok(Some(match verb {
        "kit" => Args::Kit {
            map: map.context(USAGE)?,
            out,
            disc,
        },
        _ => Args::Models {
            out: out.unwrap_or_else(|| PathBuf::from(".")),
        },
    }))
}

pub fn run(words: Vec<String>) -> Result<()> {
    match parse(words)? {
        None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(Args::Kit { map, out, disc }) => kit(&map, out, disc),
        Some(Args::Models { out }) => models(&out),
    }
}

fn kit(map: &str, out: Option<PathBuf>, disc: Option<PathBuf>) -> Result<()> {
    let disc = disc.map(Ok).unwrap_or_else(crate::default_disc)?;
    let file = IsoDisc::new(&disc)
        .map_file(map)?
        .with_context(|| format!("{} has no maps/{map}.map", disc.display()))?;
    let bytes = MapSource::region(file.path, Some((file.offset, file.len))).read_base()?;
    let added = content_kit::inject::add_pogo(&bytes, map)?;
    for style in Style::ALL {
        println!(
            "{:<10} {}.weap  {:#010x}",
            style.id(),
            style.tag_path(),
            added.placed.weapons[style.index()]
        );
    }
    println!("{}", added.costs);
    if let Some(out) = out {
        std::fs::write(&out, &added.bytes).with_context(|| format!("writing {}", out.display()))?;
        println!("{}", out.display());
    }
    Ok(())
}

fn models(out: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    for style in Style::ALL {
        let source = Source::builtin(style)?;
        let mesh = content_kit::mesh::build(&source)?;
        let file = out.join(format!("{}.obj", source.name));
        std::fs::write(&file, mesh.obj(&source.name))
            .with_context(|| format!("writing {}", file.display()))?;
        println!(
            "{}  {} triangles, {} vertices, {} parts",
            file.display(),
            mesh.triangles(),
            mesh.vertices(),
            mesh.parts.len()
        );
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
    fn the_kit_takes_a_disc_map_and_an_output_file() {
        assert_eq!(
            args("kit Lockout.map --out built.map --disc disc.iso").unwrap(),
            Some(Args::Kit {
                map: "lockout".into(),
                out: Some("built.map".into()),
                disc: Some("disc.iso".into()),
            })
        );
        assert_eq!(
            args("kit lockout").unwrap(),
            Some(Args::Kit {
                map: "lockout".into(),
                out: None,
                disc: None,
            })
        );
        assert_eq!(args("kit --help").unwrap(), None);
        for bad in [
            "",
            "kit",
            "kit ../some-file",
            "kit one two",
            "kit one --out",
            "build one",
        ] {
            assert!(args(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_models_go_to_a_directory() {
        assert_eq!(
            args("models").unwrap(),
            Some(Args::Models { out: ".".into() })
        );
        assert_eq!(
            args("models --out shapes").unwrap(),
            Some(Args::Models {
                out: "shapes".into()
            })
        );
        for bad in ["models lockout", "models --disc disc.iso", "models --out"] {
            assert!(args(bad).is_err(), "{bad}");
        }
    }
}
