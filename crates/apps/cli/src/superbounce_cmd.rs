//! `h2 superbounce scan`: the bounce slots and forced-crouch ceilings the
//! `superbounce` crate finds in a map's collision (docs/game/superbounce.md,
//! "Spots in the geometry").
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use superbounce::{Kind, Spot};

const USAGE: &str = "usage: h2 superbounce scan <map-name-or-file> [--bsp N] [--json] [--disc disc.iso]\n\nScans one structure BSP (default 0) of the map, with its instanced\ngeometry, for superbounce slots and forced-crouch ceilings, and prints\nthem best first: a heuristic over the collision geometry, not a game\nrun. --json prints them as a JSON array.";

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("scan") => {}
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            return Ok(());
        }
        Some(other) => bail!("unknown superbounce command {other}\n{USAGE}"),
    }
    let (mut positional, mut disc, mut bsp, mut json) = (Vec::new(), None, 0usize, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bsp" => {
                bsp = args
                    .next()
                    .context("--bsp needs a BSP index")?
                    .parse()
                    .context("--bsp needs a number")?
            }
            "--json" => json = true,
            "--disc" => {
                disc = Some(PathBuf::from(
                    args.next().context("--disc needs an ISO path")?,
                ))
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            s if s.starts_with('-') => bail!("unknown option {s}\n{USAGE}"),
            _ => positional.push(a),
        }
    }
    let [name] = positional.as_slice() else {
        bail!("{USAGE}")
    };
    let mut map = crate::maps::load_map(name, disc)?;
    let spots = superbounce::scan_map(&mut map, bsp)
        .with_context(|| format!("scanning BSP {bsp} of {name}"))?;
    if json {
        let rows: Vec<_> = spots.iter().map(to_json).collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    let count = |k| spots.iter().filter(|s| s.kind == k).count();
    println!(
        "{name} BSP {bsp}: {} bounce, {} crouch",
        count(Kind::Bounce),
        count(Kind::Crouch)
    );
    for s in &spots {
        println!("{}", line(s));
    }
    Ok(())
}

fn kind(k: Kind) -> &'static str {
    match k {
        Kind::Bounce => "bounce",
        Kind::Crouch => "crouch",
    }
}

fn v3(p: [f32; 3]) -> String {
    format!("({:.2}, {:.2}, {:.2})", p[0], p[1], p[2])
}

fn line(s: &Spot) -> String {
    let mut out = format!(
        "{:<6} score {:.2}  stand {}  edge {} to {}",
        kind(s.kind),
        s.score,
        v3(s.stand),
        v3(s.edge[0]),
        v3(s.edge[1]),
    );
    if s.kind == Kind::Bounce {
        out += &format!(
            "  launch {:.1}  fall {:.2}  drift ({:.2}, {:.2})",
            s.launch, s.fall, s.drift[0], s.drift[1]
        );
    }
    out
}

fn to_json(s: &Spot) -> serde_json::Value {
    serde_json::json!({
        "kind": kind(s.kind),
        "edge": s.edge,
        "stand": s.stand,
        "fall": s.fall,
        "launch": s.launch,
        "drift": s.drift,
        "score": s.score,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(kind: Kind) -> Spot {
        Spot {
            kind,
            edge: [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            stand: [-10.62, 3.88, -3.19],
            fall: 0.5,
            launch: 17.85,
            drift: [0.0, 1.0],
            score: 0.75,
        }
    }

    #[test]
    fn a_bounce_line_has_its_launch_and_a_crouch_line_does_not() {
        let b = line(&spot(Kind::Bounce));
        assert!(b.starts_with("bounce"), "{b}");
        assert!(b.contains("stand (-10.62, 3.88, -3.19)"), "{b}");
        assert!(
            b.contains("launch 17.9") && b.contains("drift (0.00, 1.00)"),
            "{b}"
        );
        let c = line(&spot(Kind::Crouch));
        assert!(c.starts_with("crouch") && !c.contains("launch"), "{c}");
    }

    #[test]
    fn json_names_the_kind_and_keeps_the_numbers() {
        let v = to_json(&spot(Kind::Bounce));
        assert_eq!(v["kind"], "bounce");
        assert_eq!(v["edge"][1][2], 6.0);
        assert_eq!(v["drift"][1], 1.0);
    }
}
