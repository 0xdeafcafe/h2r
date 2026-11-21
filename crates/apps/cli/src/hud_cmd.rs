//! `h2 hud preview` and `h2 hud sheet`: the HUD kit's HUDs drawn offline
//! over the retail player HUD, with the kit's checks (docs/game/hud-kit.md,
//! "The preview and the checks").
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use hud_kit::check::{self, Report};
use hud_kit::preview::{self, mode_name, parse_mode};
use hud_kit::{Describe, Source, huds};

/// The map the disc source draws from by default (a multiplayer map with
/// the Master Chief's HUD).
pub(crate) const DEFAULT_MAP: &str = "lockout";

const USAGE: &str = "usage: h2 hud preview list
       h2 hud preview <hud> [--state NAME|all] [--mode full|half|quarter|all] [--map NAME] [--disc ISO] [--synthetic] --out DIR
       h2 hud sheet [--map NAME] [--disc ISO] [--synthetic] --out DIR

Draws a HUD of the kit over the retail player HUD at 640x480 and runs the
kit's checks. preview writes <hud>-<state>-<mode>.png and .json for each
state and mode asked (default: every state, fullscreen); sheet writes every
HUD x state x mode, sheet.png, sheet.json and checks.json, and fails on a
finding. The map (default lockout), its fonts and the XBE's colours come
from the disc (--disc, else $H2_DISC); --synthetic draws an invented map
with flat boxes and a fixed-advance font instead. DIR must be outside the
Git checkout.";

struct Opts {
    positional: Vec<String>,
    state: Option<String>,
    mode: Option<String>,
    map: String,
    disc: Option<PathBuf>,
    synthetic: bool,
    out: Option<PathBuf>,
}

fn parse(args: Vec<String>) -> Result<Option<Opts>> {
    let mut o = Opts {
        positional: Vec::new(),
        state: None,
        mode: None,
        map: DEFAULT_MAP.to_owned(),
        disc: None,
        synthetic: false,
        out: None,
    };
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--state" => o.state = Some(args.next().context("--state needs a name or all")?),
            "--mode" => {
                o.mode = Some(
                    args.next()
                        .context("--mode needs full, half, quarter or all")?,
                )
            }
            "--map" => o.map = args.next().context("--map needs a map name")?,
            "--disc" => {
                o.disc = Some(PathBuf::from(
                    args.next().context("--disc needs an ISO path")?,
                ))
            }
            "--out" => {
                o.out = Some(PathBuf::from(
                    args.next().context("--out needs a directory")?,
                ))
            }
            "--synthetic" => o.synthetic = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            s if s.starts_with('-') => bail!("unknown option {s}\n{USAGE}"),
            _ => o.positional.push(a),
        }
    }
    Ok(Some(o))
}

fn source(o: &Opts) -> Result<Source> {
    if o.synthetic {
        return Source::synthetic();
    }
    let disc = o.disc.clone().map(Ok).unwrap_or_else(super::default_disc)?;
    crate::safe_out::protect(&disc);
    Source::disc(&disc, &o.map)
        .with_context(|| format!("reading {} from {}", o.map, disc.display()))
}

fn modes(o: &Opts) -> Result<Vec<hud_kit::Mode>> {
    match o.mode.as_deref() {
        None => Ok(vec![hud_kit::Mode::Full]),
        Some("all") => Ok(hud_kit::Mode::ALL.to_vec()),
        Some(m) => Ok(vec![parse_mode(m).with_context(|| {
            format!("no screen mode {m:?}: full, half, quarter or all")
        })?]),
    }
}

fn print_report(r: &Report, states: Option<&str>) {
    for n in &r.notes {
        println!("note: {n}");
    }
    for (c, why) in &r.skipped {
        println!("skipped {}: {why}", c.name());
    }
    let shown: Vec<_> = r
        .findings
        .iter()
        .filter(|f| states.is_none() || f.state.is_none() || f.state.as_deref() == states)
        .collect();
    for f in &shown {
        println!("{f}");
    }
    println!("{} finding(s)", shown.len());
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let sub = args.next();
    let Some(o) = parse(args.collect())? else {
        return Ok(());
    };
    match sub.as_deref() {
        Some("preview") => preview_cmd(o),
        Some("sheet") => sheet_cmd(o),
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown hud command {other}\n{USAGE}"),
    }
}

fn list() {
    for d in huds::all() {
        let names: Vec<_> = d.states().into_iter().map(|s| s.0).collect();
        println!("{}: {}", d.hud().name, names.join(", "));
    }
}

fn preview_cmd(o: Opts) -> Result<()> {
    let [name] = o.positional.as_slice() else {
        bail!("{USAGE}")
    };
    if name == "list" {
        list();
        return Ok(());
    }
    let d: Box<dyn Describe> = huds::find(name).with_context(|| {
        let names: Vec<_> = huds::all().iter().map(|d| d.hud().name).collect();
        format!("no HUD {name:?}; one of {names:?}")
    })?;
    let out = o
        .out
        .clone()
        .context("--out DIR is required (outside the Git checkout)")?;
    preview::guard(&out)?;
    let want = o.state.as_deref().filter(|s| *s != "all");
    if let Some(w) = want
        && !d.states().iter().any(|(n, _)| *n == w)
    {
        let names: Vec<_> = d.states().into_iter().map(|s| s.0).collect();
        bail!("no {name} state {w:?}; one of {names:?} or all");
    }
    let modes = modes(&o)?;
    let mut src = source(&o)?;
    let r = check::hud(&mut src, d.as_ref(), &modes)?;
    for s in r.shots.iter().filter(|s| want.is_none_or(|w| s.state == w)) {
        let json = preview::shot_json(s, &src, &r.findings);
        let [png, js] = preview::write(&out, s, &json)?;
        println!(
            "{} {}: {} and {}",
            s.state,
            mode_name(s.mode),
            png.display(),
            js.display()
        );
    }
    print_report(&r, want);
    Ok(())
}

fn sheet_cmd(o: Opts) -> Result<()> {
    if !o.positional.is_empty() {
        bail!("{USAGE}");
    }
    let out = o
        .out
        .clone()
        .context("--out DIR is required (outside the Git checkout)")?;
    preview::guard(&out)?;
    let mut src = source(&o)?;
    let all = huds::all();
    let ds: Vec<&dyn Describe> = all.iter().map(|d| d.as_ref()).collect();
    let modes = hud_kit::Mode::ALL;
    let r = check::all(&mut src, &ds, &modes)?;
    for s in &r.shots {
        let json = preview::shot_json(s, &src, &r.findings);
        preview::write(&out, s, &json)?;
    }
    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    let (sheet, tiles) = preview::sheet(&r.shots, modes.len());
    sheet.write_png(&out.join("sheet.png"))?;
    std::fs::write(out.join("sheet.json"), serde_json::to_vec_pretty(&tiles)?)?;
    std::fs::write(
        out.join("checks.json"),
        serde_json::to_vec_pretty(&r.json())?,
    )?;
    println!(
        "{} pictures and sheet.png in {}",
        r.shots.len(),
        out.display()
    );
    print_report(&r, None);
    if !r.findings.is_empty() {
        bail!("{} finding(s)", r.findings.len());
    }
    Ok(())
}
