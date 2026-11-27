//! `h2 siege layout <map>`: scaffold a Siege layout for a map from its own
//! flags and starts (`forge_doc::siege_layout`; docs/game/rules-engine.md,
//! "Siege layouts"). It writes the labels-only document the built-in
//! layouts are (`crates/formats/rules/src/builtin/layouts/siege/`), and
//! with `--scenario` the fake-game walk through its phases that a layout's
//! test runs.
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use forge_doc::siege_layout::{self, Barrier, Recipe};

const USAGE: &str = "usage: h2 siege layout <map-name-or-file> [--attack red|blue] [--per-side N] [--barrier <block>:<index>=<phase>]... [-o layout.json] [--scenario scenario.json] [--disc disc.iso] [--force]\n\nPlans a Siege layout from the map's own rows: the attack line runs from the\nattackers' CTF flag stand (red's by default) to the defenders', the\nterritories between them are the sites (two a phase, in order along the\nline), the defenders' stand is the siege_goal, and each phase's starts are\nlabelled siege_attack and siege_defend, N a side (default 6). A barrier is\nnamed by its placement and the phase it opens in (crates:22=2). It prints\nthe plan, and writes the document with -o and the scenario with --scenario.";

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("layout") => {}
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            return Ok(());
        }
        Some(other) => bail!("unknown siege command {other}\n{USAGE}"),
    }
    let mut recipe = Recipe::default();
    let (mut positional, mut disc, mut out, mut scenario, mut force) = (Vec::new(), None, None, None, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--attack" => {
                recipe.attack_team = match args.next().as_deref() {
                    Some("red") => 0,
                    Some("blue") => 1,
                    _ => bail!("--attack is red or blue"),
                }
            }
            "--per-side" => {
                recipe.per_side = args
                    .next()
                    .context("--per-side needs a number")?
                    .parse()
                    .context("--per-side needs a number")?
            }
            "--barrier" => {
                let text = args.next().context("--barrier needs <block>:<index>=<phase>")?;
                recipe.barriers.push(Barrier::parse(&text).map_err(anyhow::Error::msg)?);
            }
            "-o" | "--out" => out = Some(PathBuf::from(args.next().context("-o needs a path")?)),
            "--scenario" => scenario = Some(PathBuf::from(args.next().context("--scenario needs a path")?)),
            "--disc" => disc = Some(PathBuf::from(args.next().context("--disc needs an ISO path")?)),
            "--force" => force = true,
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
    let rows = siege_layout::read(&mut map).map_err(|e| anyhow::anyhow!("{e}"))?;
    let plan = siege_layout::plan(&rows, &recipe).map_err(anyhow::Error::msg)?;
    let short = std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_ascii_lowercase();
    print!("{}", summary(&plan, &rows));
    let side = ["red", "blue"][usize::from(recipe.attack_team)];
    let description = format!(
        "Siege on {short}, labels only, scaffolded by h2 siege layout: the attackers start at {side}'s flag stand; {} sites in {} phases, then the defenders' flag stand (netgame flag {}).",
        plan.sites.len(),
        plan.phases,
        plan.goal
    );
    let doc = siege_layout::document(&short, &plan, &description);
    let text = siege_layout::to_text(&doc);
    forge_doc::ForgeDoc::from_bytes(text.as_bytes(), &forge_doc::Limits::default())
        .map_err(|e| anyhow::anyhow!("the layout does not read as a Forge document: {e}"))?;
    if let Some(path) = out {
        write(&path, &text, force)?;
        println!("wrote {}", path.display());
    }
    if let Some(path) = scenario {
        let mut s = serde_json::to_string_pretty(&siege_layout::scenario(&plan))?;
        s.push('\n');
        write(&path, &s, force)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn write(path: &std::path::Path, text: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!("{} exists: pass --force to write over it", path.display());
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// The plan, a line a label: what each placement is and where.
fn summary(plan: &siege_layout::Plan, rows: &siege_layout::SiegeMap) -> String {
    let at = |p: [f32; 3]| format!("({:.1}, {:.1}, {:.1})", p[0], p[1], p[2]);
    let mut s = String::new();
    for &(i, n) in &plan.sites {
        s += &format!("site      phase {n}  netgame_flags[{i}] {}\n", at(rows.flags[i as usize].position));
    }
    s += &format!("goal               netgame_flags[{}] {}\n", plan.goal, at(rows.flags[plan.goal as usize].position));
    for k in 0..=plan.phases {
        let of = |list: &[(u32, i32)]| {
            list.iter()
                .filter(|e| e.1 == k)
                .map(|e| e.0.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        };
        s += &format!("attack    phase {k}  starting_locations {}\n", of(&plan.attack));
        s += &format!("defend    phase {k}  starting_locations {}\n", of(&plan.defend));
    }
    for b in &plan.barriers {
        s += &format!("barrier   phase {}  {}[{}]\n", b.phase, b.block.name(), b.index);
    }
    s
}
