use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use xiso::Xiso;

const USAGE: &str = "usage: h2 <command> [disc]

  info                    title, sections, libraries
  ls                      files on the disc
  extract [out-dir]       copy every file off the disc (default disc/extracted)
  analyze [--functions <out.txt>]
                          function discovery and coverage

[disc] is an .iso or a default.xbe. Without it, h2 uses $H2_DISC, then the
first .iso in the repo's disc/ folder.";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else { bail!("{USAGE}") };
    let mut positional = Vec::new();
    let mut functions_out = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--functions" => functions_out = Some(PathBuf::from(args.next().context("--functions needs a path")?)),
            "-h" | "--help" => bail!("{USAGE}"),
            _ => positional.push(PathBuf::from(a)),
        }
    }
    // The disc is the only positional that names an existing .iso/.xbe file.
    let disc_pos = positional.iter().position(|p| is_disc(p));
    let disc = match disc_pos {
        Some(i) => positional.remove(i),
        None => default_disc()?,
    };
    match (cmd.as_str(), positional.as_slice()) {
        ("info", []) => info(&disc),
        ("ls", []) => ls(&disc),
        ("extract", [out]) => extract(&disc, out),
        ("extract", []) => extract(&disc, &repo_root().join("disc/extracted")),
        ("analyze", []) => run_analyze(&disc, functions_out),
        _ => bail!("{USAGE}"),
    }
}

fn is_disc(p: &Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    p.is_file() && matches!(ext.as_deref(), Some("iso" | "xbe"))
}

fn repo_root() -> PathBuf {
    // crates/cli -> repo root
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn default_disc() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("H2_DISC") {
        return Ok(p.into());
    }
    let dir = repo_root().join("disc");
    let mut isos: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_disc(p))
        .collect();
    isos.sort();
    isos.into_iter().next().with_context(|| {
        format!("no disc given and none found in {} (put your .iso there or set H2_DISC)", dir.display())
    })
}

fn open_iso(path: &Path) -> Result<Xiso<BufReader<File>>> {
    let f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    Ok(Xiso::open(BufReader::new(f))?)
}

/// Accepts a raw XBE or a disc image, returning the XBE bytes.
fn load_xbe(path: &Path) -> Result<Vec<u8>> {
    let mut magic = [0u8; 4];
    File::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .read_exact(&mut magic)?;
    if &magic == b"XBEH" {
        return Ok(fs::read(path)?);
    }
    let mut iso = open_iso(path)?;
    let entry = iso.find("default.xbe")?;
    Ok(iso.read(&entry)?)
}

fn info(path: &Path) -> Result<()> {
    let data = load_xbe(path)?;
    let x = xbe::Xbe::parse(&data)?;
    let c = &x.certificate;
    println!("title      {} ({:08X})", c.title_name, c.title_id);
    println!("build      {:?}, cert version {}, regions {:#x}", x.kind, c.version, c.game_regions);
    println!("base       {:#010x}, image size {:#x}", x.base, x.size_of_image);
    println!("entry      {:#010x}", x.entry);
    println!("thunks     {:#010x} ({} kernel imports)", x.kernel_thunk, x.kernel_imports()?.len());
    println!("\nsections");
    for s in &x.sections {
        let kind = if analyze::is_code_section(s) { "code" } else { "data" };
        println!("  {:<10} {:#010x} {:>9} {} {kind}", s.name, s.va, s.vsize, s.flags);
    }
    println!("\nlibraries");
    for l in &x.libraries {
        println!("  {:<8} {}.{}.{}", l.name, l.major, l.minor, l.build);
    }
    Ok(())
}

fn ls(path: &Path) -> Result<()> {
    let mut iso = open_iso(path)?;
    for e in iso.entries()? {
        if e.is_dir {
            println!("{:>12}  {}/", "", e.path);
        } else {
            println!("{:>12}  {}", e.size, e.path);
        }
    }
    Ok(())
}

fn extract(path: &Path, out: &Path) -> Result<()> {
    let mut iso = open_iso(path)?;
    let entries = iso.entries()?;
    for e in &entries {
        // Paths come from the disc: refuse anything that could escape `out`.
        let rel = e.path.trim_start_matches('/');
        if rel.split('/').any(|p| p.is_empty() || p == "." || p == ".." || p.contains('\\')) {
            bail!("refusing unsafe path on disc: {:?}", e.path);
        }
        let dest = out.join(rel);
        if e.is_dir {
            fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut w = BufWriter::new(File::create(&dest)?);
        iso.copy_to(e, &mut w)?;
    }
    println!("extracted {} entries to {}", entries.len(), out.display());
    Ok(())
}

fn run_analyze(path: &Path, functions_out: Option<PathBuf>) -> Result<()> {
    let data = load_xbe(path)?;
    let x = xbe::Xbe::parse(&data)?;
    let a = analyze::analyze(&x)?;
    print!("{a}");
    if let Some(out) = functions_out {
        let mut s = String::new();
        for f in a.functions.values() {
            s.push_str(&format!("{:#010x} {:<10} {:?}\n", f.start, f.section, f.source));
        }
        fs::write(&out, s)?;
        println!("wrote {} functions to {}", a.functions.len(), out.display());
    }
    Ok(())
}
