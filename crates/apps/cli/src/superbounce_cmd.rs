//! `h2 superbounce scan`: the bounce slots and forced-crouch ceilings the
//! `superbounce` crate finds in a map's collision (docs/game/superbounce.md,
//! "Spots in the geometry"). `h2 superbounce hud-preview`: the superbounce
//! HUD and its tutorial cards drawn offline, and the HUD's feed for
//! `h2 play` (docs/game/superbounce.md, "The superbounce game type").
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use superbounce::{Kind, Spot};

const USAGE: &str = "usage: h2 superbounce scan <map-name-or-file> [--bsp N] [--json] [--disc disc.iso] [--near x,y,z[,r]] [--explain x,y,z[,r]]\n\nScans one structure BSP (default 0) of the map, with its instanced\ngeometry, for superbounce slots and forced-crouch ceilings, and prints\nthem best first: a heuristic over the collision geometry, not a game\nrun. --json prints them as a JSON array. --near prints, instead, the\ncollision polygons with a vertex or centroid within r (default 0.6)\nof the point: what a scanned spot is checked against. --explain prints,\ninstead, every forced-crouch region with a sample within r (default 1.0)\nof the point and how each filter judged it.";

const HUD_USAGE: &str = "usage: h2 superbounce hud-preview [--state NAME|all] [--out DIR]\n\nDraws the superbounce HUD and its tutorial cards over a plain backdrop,\nwith the code the game window uses, and writes one PNG a state\n(superbounce-hud-STATE-WxH.png, 1280x720; `primed` also at 1920x1080)\nto DIR (default the temporary directory). States: scanning, not-primed,\nprimed, primed-airborne, tutorial-1 to tutorial-4.";

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("scan") => {}
        Some("hud-preview") => return hud_preview(args.collect()),
        Some("-h" | "--help") | None => {
            println!("{USAGE}\n\n{HUD_USAGE}");
            return Ok(());
        }
        Some(other) => bail!("unknown superbounce command {other}\n{USAGE}\n\n{HUD_USAGE}"),
    }
    let (mut positional, mut disc, mut bsp, mut json) = (Vec::new(), None, 0usize, false);
    let mut near: Option<([f32; 3], f32)> = None;
    let mut explain: Option<([f32; 3], f32)> = None;
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
            "--explain" => {
                explain = Some(parse_point(
                    &args.next().context("--explain needs x,y,z")?,
                    1.0,
                )?)
            }
            "--near" => near = Some(parse_near(&args.next().context("--near needs x,y,z")?)?),
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
    if let Some((at, r)) = near {
        let polys = superbounce::map_polys(&mut map, bsp)
            .with_context(|| format!("reading BSP {bsp} of {name}"))?;
        for p in polys.iter().filter(|p| poly_near(p, at, r)) {
            let verts: Vec<_> = p.verts.iter().map(|&v| v3(v)).collect();
            println!("normal {}  {}", n3(p.normal), verts.join(" "));
        }
        return Ok(());
    }
    if let Some((at, r)) = explain {
        let regions = superbounce::explain_map(&mut map, bsp, at, r)
            .with_context(|| format!("scanning BSP {bsp} of {name}"))?;
        println!(
            "{name} BSP {bsp}: {} forced-crouch regions within {r} of {}",
            regions.len(),
            v3(at)
        );
        for (i, region) in regions.iter().enumerate() {
            print!("{}", explain_text(i, region));
        }
        return Ok(());
    }
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

// ---- the HUD ----

use app::superbounce_hud as hud;
use boot::superbounce_mode as mode;

/// The HUD's view of the mode's state.
pub(crate) fn hud_view(s: &mode::State) -> hud::View {
    hud::View {
        primed: s.primed(),
        scan: match &s.scan {
            mode::ScanState::Idle | mode::ScanState::Scanning => hud::Scan::Scanning,
            mode::ScanState::Ready => hud::Scan::Ready,
            mode::ScanState::Failed(why) => hud::Scan::Failed(why.clone()),
        },
        nearest: s.nearest.map(|n| hud::Nearest {
            distance_m: n.distance * mode::METRES_PER_UNIT,
            launch: n.launch,
            fall: n.fall,
        }),
        bearing: s.bearing(),
        airborne: s.airborne,
        vz: s.vz,
    }
}

/// `h2 play`'s superbounce HUD: the mode's state each present, and the
/// tutorial cards unless `superbounce.tutorial_seen` is set (or
/// `H2_SUPERBOUNCE_TUTORIAL=1` asks for them anyway), which the last card
/// sets.
pub(crate) fn hud_source(store: &Arc<settings::Store>) -> hud::Source {
    use boot::setting_defs::{
        SUPERBOUNCE_TUTORIAL_ENV, SUPERBOUNCE_TUTORIAL_SEEN, superbounce_tutorial_shows,
    };
    let key = SUPERBOUNCE_TUTORIAL_SEEN.key;
    let seen = store.handle::<bool>(key).get();
    let env = std::env::var(SUPERBOUNCE_TUTORIAL_ENV).ok();
    let show = superbounce_tutorial_shows(seen, env.as_deref());
    let store = Arc::clone(store);
    hud::Source::new(|| mode::hud_state().map(|s| hud_view(&s))).with_tutorial(show, move || {
        match store.set(key, settings::Value::Bool(true)) {
            Ok(o) => {
                if let Some(e) = o.not_saved {
                    eprintln!("superbounce: the tutorial's seen flag was not saved: {e}");
                }
            }
            Err(e) => eprintln!("superbounce: the tutorial's seen flag: {e}"),
        }
    })
}

/// One preview: its name, the view, the card showing and the animation
/// time (seconds) it is drawn at.
pub(crate) fn hud_states() -> Vec<(&'static str, hud::View, Option<u8>, f32)> {
    let not_primed = hud::View {
        primed: false,
        scan: hud::Scan::Ready,
        nearest: Some(hud::Nearest {
            distance_m: 6.2,
            launch: 17.9,
            fall: 1.1,
        }),
        bearing: Some(0.6),
        airborne: false,
        vz: Some(0.0),
    };
    let primed = hud::View {
        primed: true,
        ..not_primed.clone()
    };
    let airborne = hud::View {
        airborne: true,
        vz: Some(-4.0),
        ..primed.clone()
    };
    let scanning = hud::View {
        scan: hud::Scan::Scanning,
        nearest: None,
        bearing: None,
        ..not_primed.clone()
    };
    // 0.6 s: the primed pulse at its brightest.
    vec![
        ("scanning", scanning, None, 0.3),
        ("not-primed", not_primed.clone(), None, 0.0),
        ("primed", primed.clone(), None, 0.6),
        ("primed-airborne", airborne.clone(), None, 0.6),
        ("tutorial-1", not_primed, Some(1), 0.0),
        ("tutorial-2", primed.clone(), Some(2), 0.6),
        ("tutorial-3", primed, Some(3), 0.6),
        ("tutorial-4", airborne, Some(4), 0.6),
    ]
}

/// The sizes a state is drawn at.
fn hud_sizes(state: &str) -> &'static [(u32, u32)] {
    if state == "primed" {
        &[(1280, 720), (1920, 1080)]
    } else {
        &[(1280, 720)]
    }
}

fn hud_preview(args: Vec<String>) -> Result<()> {
    let (mut want, mut out) = (None::<String>, None::<PathBuf>);
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--state" => want = Some(args.next().context("--state needs a name or all")?),
            "--out" => {
                out = Some(PathBuf::from(
                    args.next().context("--out needs a directory")?,
                ))
            }
            "-h" | "--help" => {
                println!("{HUD_USAGE}");
                return Ok(());
            }
            _ => bail!("unknown argument {a}\n{HUD_USAGE}"),
        }
    }
    let mut states = hud_states();
    if let Some(w) = want.as_deref().filter(|w| *w != "all") {
        states.retain(|(n, ..)| *n == w);
        if states.is_empty() {
            let names: Vec<_> = hud_states().into_iter().map(|s| s.0).collect();
            bail!("no HUD state {w:?}; one of {names:?} or all");
        }
    }
    let dir = out.unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    for (name, view, card, t) in &states {
        for &(w, h) in hud_sizes(name) {
            let px = hud::preview(view, *card, *t, (w, h));
            let mut png = Vec::new();
            assets::png::write_rgba(&mut png, w, h, &px)?;
            let path = dir.join(format!("superbounce-hud-{name}-{w}x{h}.png"));
            std::fs::write(&path, &png).with_context(|| format!("writing {}", path.display()))?;
            println!("{name}: {w}x{h} -> {}", path.display());
        }
    }
    Ok(())
}

/// `x,y,z` or `x,y,z,r`, with `r` defaulting to `default_r`.
fn parse_point(s: &str, default_r: f32) -> Result<([f32; 3], f32)> {
    let n: Vec<f32> = s
        .split(',')
        .map(|t| t.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .context("needs numbers: x,y,z[,r]")?;
    match n.as_slice() {
        [x, y, z] => Ok(([*x, *y, *z], default_r)),
        [x, y, z, r] => Ok(([*x, *y, *z], *r)),
        _ => bail!("needs x,y,z or x,y,z,r"),
    }
}

/// One region of `--explain`: what it is, then each filter's finding.
fn explain_text(i: usize, r: &superbounce::CrouchRegion) -> String {
    let mut out = format!(
        "region {i}: {} samples, {} to {}, length {:.2}\n",
        r.samples,
        v3(r.min),
        v3(r.max),
        r.length
    );
    if r.widths.is_empty() {
        out += "  widths: not probed\n";
    }
    for (at, d, w) in &r.widths {
        out += &format!("  width {:.3} at {} out ({}, {})\n", w, v3(*at), d[0], d[1]);
    }
    let reach = if r.reach.is_empty() {
        "not walked"
    } else {
        r.reach.as_str()
    };
    out += &format!(
        "  {}: {reach}\n",
        if r.reachable {
            "reachable"
        } else {
            "not reachable"
        }
    );
    if let Some(s) = &r.spot {
        out += &format!("  spot: {}\n", line(s));
    }
    if let Some(by) = r.suppressed_by {
        out += &format!("  suppressed by the spot standing at {}\n", v3(by));
    }
    out += &match (&r.rejected, r.kept) {
        (_, true) => "  kept\n".to_string(),
        (Some(why), _) => format!("  dropped: {why}\n"),
        (None, false) => "  dropped\n".to_string(),
    };
    out
}

fn parse_near(s: &str) -> Result<([f32; 3], f32)> {
    let n: Vec<f32> = s
        .split(',')
        .map(|t| t.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .context("--near needs numbers: x,y,z[,r]")?;
    match n.as_slice() {
        [x, y, z] => Ok(([*x, *y, *z], 0.6)),
        [x, y, z, r] => Ok(([*x, *y, *z], *r)),
        _ => bail!("--near needs x,y,z or x,y,z,r"),
    }
}

fn poly_near(p: &superbounce::Poly, at: [f32; 3], r: f32) -> bool {
    let d = |v: [f32; 3]| {
        ((v[0] - at[0]).powi(2) + (v[1] - at[1]).powi(2) + (v[2] - at[2]).powi(2)).sqrt()
    };
    if p.verts.is_empty() {
        return false;
    }
    let k = p.verts.len() as f32;
    let c = p.verts.iter().fold([0.0; 3], |a, v| {
        [a[0] + v[0] / k, a[1] + v[1] / k, a[2] + v[2] / k]
    });
    d(c) <= r || p.verts.iter().any(|&v| d(v) <= r)
}

fn n3(n: [f32; 3]) -> String {
    format!("({:.4}, {:.4}, {:.4})", n[0], n[1], n[2])
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
    fn the_hud_sees_the_mode_in_metres_with_its_arrow() {
        let s = mode::State {
            active: true,
            word: Some(0xffff_fffd),
            position: Some([0.0; 3]),
            nearest: Some(mode::Near {
                distance: 2.0,
                launch: 17.85,
                fall: 1.1,
                edge: [[5.0, -1.0, 0.0], [5.0, 1.0, 0.0]],
            }),
            scan: mode::ScanState::Ready,
            airborne: true,
            vz: Some(-3.0),
            yaw: Some(0.0),
            ..Default::default()
        };
        let v = hud_view(&s);
        assert!(v.primed && v.airborne);
        assert_eq!(v.scan, hud::Scan::Ready);
        let n = v.nearest.unwrap();
        assert!((n.distance_m - 6.096).abs() < 1e-4);
        assert_eq!((n.launch, n.fall), (17.85, 1.1));
        assert_eq!(v.bearing, Some(0.0));
        assert_eq!(v.vz, Some(-3.0));
        let idle = hud_view(&mode::State::default());
        assert!(!idle.primed && idle.nearest.is_none() && idle.bearing.is_none());
        assert_eq!(idle.scan, hud::Scan::Scanning);
    }

    #[test]
    fn the_hud_preview_has_every_state_once() {
        let names: Vec<_> = hud_states().into_iter().map(|s| s.0).collect();
        assert_eq!(
            names,
            [
                "scanning",
                "not-primed",
                "primed",
                "primed-airborne",
                "tutorial-1",
                "tutorial-2",
                "tutorial-3",
                "tutorial-4"
            ]
        );
        for (name, view, card, _) in hud_states() {
            assert_eq!(
                view.airborne && view.primed,
                name.contains("airborne") || card == Some(4),
                "{name}"
            );
            assert_eq!(card.is_some(), name.starts_with("tutorial"), "{name}");
        }
        assert_eq!(hud_sizes("primed"), [(1280, 720), (1920, 1080)]);
        assert_eq!(hud_sizes("scanning"), [(1280, 720)]);
    }

    #[test]
    fn json_names_the_kind_and_keeps_the_numbers() {
        let v = to_json(&spot(Kind::Bounce));
        assert_eq!(v["kind"], "bounce");
        assert_eq!(v["edge"][1][2], 6.0);
        assert_eq!(v["drift"][1], 1.0);
    }

    #[test]
    fn explain_names_each_filter_finding() {
        let kept = superbounce::CrouchRegion {
            samples: 3,
            min: [0.0; 3],
            max: [0.1, 0.1, 0.0],
            widths: vec![([0.0, 0.0, 0.01], [0, 1], 0.05)],
            length: 0.08,
            reachable: true,
            reach: "reaches open floor at (0.00, 0.24) after 3 clear samples".into(),
            rejected: None,
            spot: Some(spot(Kind::Crouch)),
            suppressed_by: None,
            kept: true,
        };
        let t = explain_text(0, &kept);
        assert!(t.starts_with("region 0: 3 samples"), "{t}");
        assert!(
            t.contains("width 0.050 at (0.00, 0.00, 0.01) out (0, 1)"),
            "{t}"
        );
        assert!(t.contains("reachable: reaches open floor"), "{t}");
        assert!(
            t.contains("spot: crouch") && t.trim_end().ends_with("kept"),
            "{t}"
        );
        let dropped = superbounce::CrouchRegion {
            widths: Vec::new(),
            reachable: false,
            reach: String::new(),
            rejected: Some("no exits".into()),
            spot: None,
            kept: false,
            ..kept
        };
        let t = explain_text(1, &dropped);
        assert!(
            t.contains("widths: not probed") && t.contains("not reachable: not walked"),
            "{t}"
        );
        assert!(t.trim_end().ends_with("dropped: no exits"), "{t}");
    }
}
