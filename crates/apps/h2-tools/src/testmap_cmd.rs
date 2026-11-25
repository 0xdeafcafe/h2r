//! `h2 testmap`: a small test map from an existing `.map` and a JSON edit
//! script (`scenario_edit::script`; docs/runtime/agent-api.md, phase 5).
//! Writes a new map and prints what the script did as JSON. Never touches
//! the source, never runs the game.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

const USAGE: &str = "usage: h2 testmap <source.map> <script.json | -> --out <new.map> [--force]
  runs a JSON edit script over the source's placements, through the
  editor's own checks, and saves a new map:
    {\"ops\": [
      {\"op\": \"clear\", \"blocks\": \"all\"},
      {\"op\": \"keep\", \"block\": \"starting_locations\", \"indices\": [0]},
      {\"op\": \"add\", \"block\": \"weapons\", \"like\": 0, \"set\": {\"position\": [1, 2, 0.5]}},
      {\"op\": \"set\", \"block\": \"starting_locations\", \"index\": 0, \"set\": {\"facing\": 1.57}},
      {\"op\": \"remove\", \"block\": \"scenery\", \"index\": 3}],
     \"allow_growth\": false}
  blocks: scenery vehicles equipment weapons machines sound_scenery crates
  starting_locations netgame_flags netgame_equipment starting_equipment.
  Load the result with H2_MAP_DIR=<its folder> (the user or coordinator
  runs the game; docs/formats/map-builder.md).";

#[derive(Debug, PartialEq)]
struct Args {
    source: PathBuf,
    script: String,
    out: PathBuf,
    force: bool,
}

fn parse(args: &[String]) -> Result<Args> {
    let (mut pos, mut out, mut force) = (Vec::new(), None, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(it.next().context("--out needs a path")?)),
            "--force" => force = true,
            "-h" | "--help" => bail!("{USAGE}"),
            _ => pos.push(a.clone()),
        }
    }
    let [source, script] = <[String; 2]>::try_from(pos).map_err(|_| anyhow::anyhow!("{USAGE}"))?;
    Ok(Args {
        source: source.into(),
        script,
        out: out.context(USAGE)?,
        force,
    })
}

pub fn run(args: Vec<String>) -> Result<()> {
    let a = parse(&args)?;
    let text = if a.script == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        std::fs::read_to_string(&a.script).with_context(|| format!("reading {}", a.script))?
    };
    let script: scenario_edit::script::Script =
        serde_json::from_str(&text).with_context(|| format!("{}: not an edit script", a.script))?;
    if a.out.exists() && !a.force {
        bail!("{} exists (--force replaces it)", a.out.display());
    }
    if a.out.canonicalize().ok() == a.source.canonicalize().ok() && a.out.exists() {
        bail!("--out is the source; write a new map");
    }
    let report = scenario_edit::script::build(&a.source, &script, &a.out)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn arguments() {
        assert_eq!(
            parse(&w("a.map edits.json --out b.map")).unwrap(),
            Args {
                source: "a.map".into(),
                script: "edits.json".into(),
                out: "b.map".into(),
                force: false
            }
        );
        assert!(parse(&w("a.map - --out b.map --force")).unwrap().force);
        for bad in ["a.map edits.json", "a.map --out b.map", "a b c --out d"] {
            assert!(parse(&w(bad)).is_err(), "{bad}");
        }
    }
}
