//! `h2 flood-pod`: the Flood pod's kit, looked at outside the game.
//!
//! `h2 flood-pod kit MAP` adds the pod to a multiplayer map of the user's
//! disc (`content_kit::flood_pod::inject`) and says what it took; `--out` writes the
//! map, which Studio opens to render the pod. `h2 flood-pod model` writes
//! the mesh as an OBJ file.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use boot::new_maps::{Disc, IsoDisc};
use content_kit::flood_pod::{TAG_PATH, source};
use scenario_edit::MapSource;

const USAGE: &str = "usage: h2 flood-pod kit MAP [--out FILE] [--disc ISO_OR_EXTRACTED_DISC]
       h2 flood-pod model [--out DIR]
  kit adds the Flood pod to a multiplayer map of the disc, e.g. coagulation,
    and prints its tags and the tag memory taken. --out writes the map built
    (a file of the user's own data: keep it out of any repository).
  model writes the pod's mesh as DIR/flood_pod.obj (default: the current
    directory), to look at in a model viewer.";

#[derive(Debug, PartialEq)]
enum Args {
    Kit {
        map: String,
        out: Option<PathBuf>,
        disc: Option<PathBuf>,
    },
    Model {
        out: PathBuf,
    },
}

fn parse(words: Vec<String>) -> Result<Option<Args>> {
    let mut it = words.into_iter();
    let verb = match it.next().as_deref() {
        Some("kit") => "kit",
        Some("model") => "model",
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
                    bail!("expected a disc map name such as coagulation, got {arg:?}");
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
        _ => Args::Model {
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
        Some(Args::Model { out }) => model(&out),
    }
}

fn kit(map: &str, out: Option<PathBuf>, disc: Option<PathBuf>) -> Result<()> {
    let disc = disc.map(Ok).unwrap_or_else(crate::default_disc)?;
    let file = IsoDisc::new(&disc)
        .map_file(map)?
        .with_context(|| format!("{} has no maps/{map}.map", disc.display()))?;
    let bytes = MapSource::region(file.path, Some((file.offset, file.len))).read_base()?;
    let added = content_kit::flood_pod::inject::add_flood_pod(&bytes, map)?;
    let t = added.tags;
    for (group, id) in [
        ("vehi", t.vehicle),
        ("hlmt", t.model),
        ("mode", t.render),
        ("phmo", t.physics),
        ("shad", t.shader),
        ("bitm", t.bitmap),
    ] {
        println!("{TAG_PATH}.{group}  {:#010x}", id.0);
    }
    println!("{}", added.costs);
    if let Some(out) = out {
        std::fs::write(&out, &added.bytes).with_context(|| format!("writing {}", out.display()))?;
        println!("{}", out.display());
    }
    Ok(())
}

fn model(out: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let s = source::builtin()?;
    let mesh = source::build(&s)?;
    let file = out.join(format!("{}.obj", s.name));
    std::fs::write(&file, mesh.obj(&s.name))
        .with_context(|| format!("writing {}", file.display()))?;
    println!(
        "{}  {} triangles, {} vertices, {} parts",
        file.display(),
        mesh.triangles(),
        mesh.vertices(),
        mesh.parts.len()
    );
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
            args("kit Coagulation.map --out built.map --disc disc.iso").unwrap(),
            Some(Args::Kit {
                map: "coagulation".into(),
                out: Some("built.map".into()),
                disc: Some("disc.iso".into()),
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
    fn the_model_goes_to_a_directory() {
        assert_eq!(
            args("model").unwrap(),
            Some(Args::Model { out: ".".into() })
        );
        assert_eq!(
            args("model --out meshes").unwrap(),
            Some(Args::Model {
                out: "meshes".into()
            })
        );
        assert!(args("model lockout").is_err());
        assert!(args("model --disc disc.iso").is_err());
    }
}
