//! `h2 tags project` and `h2 tags edits`: the tag workspace's projects and
//! edit files over the tree (docs/editor/tag-workspace.md, "Projects",
//! "Edits are data"), the same calls as Studio's `project.*` and `edits.*`
//! (crates/apps/studio/src/api/projects.rs).
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use workspace::edits;
use workspace::project::{self, Roots};
use workspace::project_build::{self, Tools};

const USAGE: &str = "usage:
  h2 tags project list [--json]
      every project: each map item of the packages content/*/ in the
      checkout, then <data>/packages/*/
  h2 tags project show PROJECT [--closure] [--json]
      what PROJECT (a map's name, or a package directory with one map
      item) resolves to: its layers top first (edits, authored, imports,
      tree), edit files, kits, and every problem that would stop its build;
      --closure walks a fresh project's closure for lossy tags
  h2 tags project build PROJECT [--out FILE.map] [--json]
      build PROJECT from the tree, its layers, edit files, kits and the
      items it takes, and write the map package <data>/maps/<name>/ with
      receipt.json; --out also writes the map alone
  h2 tags edits list [--json]
      every edit file: assets/tag-edits/*.yaml, then <data>/workspace/edits/
  h2 tags edits check PROJECT [--json]
      apply every edit of PROJECT once over its layers; each problem with
      its file, line and tag; fails when there is one
  h2 tags edits set FILE TAG PATH VALUE [--project PROJECT]
      write one set line into local edit file FILE
      (<data>/workspace/edits/FILE.yaml); TAG is <group>:<path>, VALUE is
      JSON (quote text: '\"text\"'); --project records the tag's hash under
      PROJECT's layers as the entry's was
  common: --repo DIR (the tree; default <data>/tags), --data DIR (default:
  the user's data directory), --checkout DIR (default: this checkout)";

/// Whether `args` (after `h2 tags`) is one of these subcommands.
pub(crate) fn handles(args: &[String]) -> bool {
    matches!(args.first().map(String::as_str), Some("project" | "edits"))
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    let o = Opts::parse(&args[1.min(args.len())..])?;
    match (
        args.first().map(String::as_str),
        o.words.first().map(String::as_str),
    ) {
        (Some("project"), Some("list")) => project_list(&o),
        (Some("project"), Some("show")) => project_show(&o),
        (Some("project"), Some("build")) => project_build_cmd(&o),
        (Some("edits"), Some("list")) => edits_list(&o),
        (Some("edits"), Some("check")) => edits_check(&o),
        (Some("edits"), Some("set")) => edits_set(&o),
        _ => bail!("{USAGE}"),
    }
}

struct Opts {
    roots: Roots,
    /// The words after the subcommand, flags removed.
    words: Vec<String>,
    json: bool,
    closure: bool,
    out: Option<PathBuf>,
    project: Option<String>,
}

impl Opts {
    fn parse(args: &[String]) -> Result<Self> {
        let mut repo = None;
        let mut data = None;
        let mut checkout = None;
        let mut o = Opts {
            roots: Roots {
                tree: PathBuf::new(),
                data: PathBuf::new(),
                checkout: None,
            },
            words: Vec::new(),
            json: false,
            closure: false,
            out: None,
            project: None,
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut next = |what: &str| -> Result<String> {
                it.next()
                    .cloned()
                    .with_context(|| format!("{a} needs {what}\n{USAGE}"))
            };
            match a.as_str() {
                "--repo" => repo = Some(PathBuf::from(next("a tag tree")?)),
                "--data" => data = Some(PathBuf::from(next("a data directory")?)),
                "--checkout" => checkout = Some(PathBuf::from(next("a checkout")?)),
                "--out" => o.out = Some(PathBuf::from(next("a file")?)),
                "--project" => o.project = Some(next("a project")?),
                "--json" => o.json = true,
                "--closure" => o.closure = true,
                "-h" | "--help" => bail!("{USAGE}"),
                // A VALUE may be a negative number.
                a if a.starts_with("--") => bail!("unknown option {a}\n{USAGE}"),
                a => o.words.push(a.to_owned()),
            }
        }
        let data = data.unwrap_or_else(content::store::default_data_root);
        o.roots = Roots {
            tree: repo.unwrap_or_else(|| data.join("tags")),
            data,
            checkout: Some(checkout.unwrap_or_else(|| {
                let here = cli_common::repo_root();
                here.canonicalize().unwrap_or(here)
            })),
        };
        Ok(o)
    }

    /// The `n`th word after the subcommand.
    fn word(&self, n: usize, what: &str) -> Result<&str> {
        self.words
            .get(n)
            .map(String::as_str)
            .with_context(|| format!("name {what}\n{USAGE}"))
    }
}

fn tools() -> Tools<'static> {
    Tools {
        kits: workspace::stage::project_kit::KITS,
        keep: workspace::stage::project_kit::KEEP,
    }
}

/// Print a result as JSON (a macro: this crate names no `serde` trait).
macro_rules! print_json {
    ($v:expr) => {{
        println!("{}", serde_json::to_string_pretty($v)?);
        Ok::<(), anyhow::Error>(())
    }};
}

fn project_list(o: &Opts) -> Result<()> {
    let list = project::list(&o.roots);
    if o.json {
        return print_json!(&list);
    }
    for p in &list {
        match (&p.name, &p.error) {
            (Some(name), _) => println!(
                "{name}\t{:?}\t{}\t{:?}\t{} item {}\t{}",
                p.mode.expect("a read project"),
                p.base.as_deref().unwrap_or_default(),
                p.origin,
                p.package.as_deref().unwrap_or_default(),
                p.item.unwrap_or_default(),
                p.path
            ),
            (None, Some(e)) => println!("(unreadable)\t{}\t{e}", p.path),
            (None, None) => {}
        }
    }
    Ok(())
}

fn project_show(o: &Opts) -> Result<()> {
    let file = project::load(&o.roots, o.word(1, "a project")?)?;
    let d = project_build::describe(&o.roots, &file, &tools(), o.closure);
    if o.json {
        return print_json!(&d);
    }
    println!(
        "{} ({:?}, base {}): package {} item {} at {}",
        d.project.name, d.project.mode, d.project.base, d.package, d.item, d.file
    );
    for l in &d.layers {
        println!("  layer {}\t{}\t{}", l.kind, l.name, l.path);
    }
    for k in &d.kits {
        match &k.fingerprint {
            Some(f) => println!("  kit {}\t{f}", k.name),
            None => println!("  kit {}\t(not in this build)", k.name),
        }
    }
    if !d.edited.is_empty() {
        println!("  edits {}", d.edited.join(", "));
    }
    if !d.authored.is_empty() {
        println!("  authored {}", d.authored.join(", "));
    }
    if let Some(from) = d.scenario_from {
        println!("  scenario from the {from}");
    }
    if let Some(lossy) = &d.lossy {
        println!("  {} lossy tags in the closure", lossy.len());
        for l in lossy.iter().take(20) {
            println!("    {l}");
        }
    }
    for p in &d.problems {
        println!("  problem: {p}");
    }
    if !d.problems.is_empty() {
        bail!(
            "{} problems stop the build of {}",
            d.problems.len(),
            d.project.name
        );
    }
    Ok(())
}

fn project_build_cmd(o: &Opts) -> Result<()> {
    let file = project::load(&o.roots, o.word(1, "a project")?)?;
    let started = std::time::Instant::now();
    let built = project_build::build(&o.roots, &file, &tools())?;
    let wrote = project_build::write(&o.roots, &built)?;
    if let Some(out) = &o.out {
        std::fs::write(out, &built.bytes).with_context(|| format!("writing {}", out.display()))?;
        println!("{}", out.display());
    }
    if o.json {
        return print_json!(&serde_json::json!({
            "dir": wrote.dir,
            "map_id": wrote.map_id,
            "file": wrote.file,
            "unchanged": wrote.unchanged,
            "took_ms": started.elapsed().as_millis() as u64,
            "receipt": built.receipt,
        }));
    }
    let r = &built.receipt;
    println!(
        "{}: {} ({:?} over {}, {} bytes, tag memory {:#x} of {:#x}, {} edited, {} authored, {} added tags, key {}), {:.1?}",
        wrote.dir,
        wrote.map_id,
        r.mode,
        r.base,
        r.file_size,
        r.memory_size,
        r.memory_max,
        r.edited.len(),
        r.authored.len(),
        r.added.len(),
        &r.key[..16.min(r.key.len())],
        started.elapsed()
    );
    for ov in &r.overrides {
        println!(
            "    {} {}: {} over {}",
            ov.tag, ov.path, ov.later, ov.earlier
        );
    }
    Ok(())
}

fn edits_list(o: &Opts) -> Result<()> {
    let list = edits::list(o.roots.checkout.as_deref(), &o.roots.workspace());
    if o.json {
        return print_json!(&list);
    }
    for f in &list {
        println!(
            "{}\t{:?}\t{} entries\t{}",
            f.name, f.origin, f.entries, f.path
        );
        for p in &f.problems {
            println!("    problem: {p}");
        }
    }
    Ok(())
}

fn edits_check(o: &Opts) -> Result<()> {
    let file = project::load(&o.roots, o.word(1, "a project")?)?;
    let c = project::check_edits(&o.roots, &file)?;
    if o.json {
        print_json!(&c)?;
    } else {
        println!(
            "{}: {} edit files, {} edited tags",
            c.project,
            c.files.len(),
            c.edited.len()
        );
        for p in &c.problems {
            println!("    {p}");
        }
        for ov in &c.overrides {
            println!(
                "    {} {}: {} over {}",
                ov.tag, ov.path, ov.later, ov.earlier
            );
        }
    }
    if !c.problems.is_empty() {
        bail!("{}", edits::problems_text(&c.problems));
    }
    Ok(())
}

fn edits_set(o: &Opts) -> Result<()> {
    let name = o.word(1, "a local edit file")?;
    let tag = o.word(2, "a tag (<group>:<path>)")?;
    let path = o.word(3, "a field path")?;
    let text = o.word(4, "a value")?;
    let value: serde_json::Value = serde_json::from_str(text)
        .with_context(|| format!("VALUE {text:?} is JSON: quote text as '\"text\"'"))?;
    let was = match &o.project {
        None => None,
        Some(which) => {
            let file = project::load(&o.roots, which)?;
            let key = edits::parse_key(tag).map_err(anyhow::Error::msg)?;
            Some(project::was_of(&o.roots, &file, &key)?)
        }
    };
    let w = edits::set_line(&o.roots.workspace(), name, tag, path, &value, was)
        .map_err(anyhow::Error::msg)?;
    println!(
        "{}:{}: {} {path} = {value}{}",
        w.path,
        w.line,
        tag,
        if w.replaced { " (replaced)" } else { "" }
    );
    Ok(())
}
