use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use xiso::Xiso;

mod maps;

const USAGE: &str = "usage: h2 <command> [disc]

  info                    title, sections, libraries
  ls                      files on the disc
  maps [disc.iso]         map headers and tag counts on the disc
  tags <map> [group]      tags in a disc map or local .map; optional --disc <iso>
  extract [out-dir]       copy every file off the disc (default disc/extracted)
  extract-xbe [out]       just default.xbe (default disc/default.xbe)
  disasm <addr> [count]   decode instructions at a virtual address
  analyze [--functions <out.txt>] [--against <ghidra-functions.tsv>] [--explain <addr>]...
                          function discovery and coverage

[disc] is an .iso or a default.xbe. Without it, h2 uses $H2_DISC, then the
first .iso in the repo's disc/ folder.";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else { bail!("{USAGE}") };
    if matches!(cmd.as_str(), "maps" | "tags") {
        return maps::run(&cmd, args.collect());
    }
    let mut positional = Vec::new();
    let mut functions_out = None;
    let mut against = None;
    let mut explain = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--functions" => functions_out = Some(PathBuf::from(args.next().context("--functions needs a path")?)),
            "--explain" => explain.push(args.next().context("--explain needs an address")?),
            "--against" => against = Some(PathBuf::from(args.next().context("--against needs a path")?)),
            "-h" | "--help" => bail!("{USAGE}"),
            _ => positional.push(PathBuf::from(a)),
        }
    }
    // Commands take at most this many arguments of their own; one more in
    // front of them is the disc.
    let own = match cmd.as_str() {
        "extract" | "extract-xbe" => 1,
        "disasm" => 2,
        _ => 0,
    };
    let disc = if positional.len() > own {
        let p = positional.remove(0);
        if !is_disc(&p) {
            bail!("{} is not an .iso or .xbe file\n\n{USAGE}", p.display());
        }
        p
    } else {
        default_disc()?
    };
    match (cmd.as_str(), positional.as_slice()) {
        ("info", []) => info(&disc),
        ("ls", []) => ls(&disc),
        ("extract", [out]) => extract(&disc, out),
        ("extract", []) => extract(&disc, &repo_root().join("disc/extracted")),
        ("extract-xbe", [out]) => extract_xbe(&disc, out),
        ("extract-xbe", []) => extract_xbe(&disc, &repo_root().join("disc/default.xbe")),
        ("analyze", []) => run_analyze(&disc, functions_out, against, &explain),
        ("disasm", [addr]) => disasm(&disc, addr, None),
        ("disasm", [addr, n]) => disasm(&disc, addr, Some(n)),
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
        .filter(|p| is_disc(p) && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("iso")))
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

fn disasm(disc: &Path, addr: &Path, count: Option<&PathBuf>) -> Result<()> {
    use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter};
    let parse = |p: &Path| -> Result<u32> {
        let s = p.to_string_lossy();
        Ok(u32::from_str_radix(s.trim_start_matches("0x"), 16).with_context(|| format!("bad hex: {s}"))?)
    };
    let va = parse(addr)?;
    let n: usize = count.map(|c| c.to_string_lossy().parse()).transpose()?.unwrap_or(20);
    let data = load_xbe(disc)?;
    let x = xbe::Xbe::parse(&data)?;
    let s = x.section_containing(va).with_context(|| format!("{va:#x} is not in any section"))?;
    let avail = (s.va + s.raw_size.min(s.vsize)).saturating_sub(va) as usize;
    let bytes = x.bytes_at(va, avail)?;
    let mut dec = Decoder::with_ip(32, bytes, va as u64, DecoderOptions::NONE);
    let mut fmt = IntelFormatter::new();
    let mut text = String::new();
    for insn in dec.iter().take(n) {
        text.clear();
        fmt.format(&insn, &mut text);
        let off = (insn.ip() as u32 - va) as usize;
        let hex: String = bytes[off..off + insn.len()].iter().map(|b| format!("{b:02x}")).collect();
        println!("{:#010x}  {hex:<20} {text}", insn.ip());
    }
    Ok(())
}

fn extract_xbe(disc: &Path, out: &Path) -> Result<()> {
    let data = load_xbe(disc)?;
    fs::write(out, &data).with_context(|| format!("writing {}", out.display()))?;
    println!("wrote {} ({} bytes)", out.display(), data.len());
    Ok(())
}

fn run_analyze(path: &Path, functions_out: Option<PathBuf>, against: Option<PathBuf>, explain: &[String]) -> Result<()> {
    let data = load_xbe(path)?;
    let x = xbe::Xbe::parse(&data)?;
    let a = analyze::analyze(&x)?;
    if !explain.is_empty() {
        // Per byte: ? unknown, I instruction start, i instruction body, d data.
        for e in explain {
            let va = u32::from_str_radix(e.trim_start_matches("0x"), 16).with_context(|| format!("bad hex: {e}"))?;
            for row in 0..4u32 {
                let start = va.wrapping_sub(16) + row * 16;
                let mut line = format!("{start:#010x} ");
                let mut owners = std::collections::BTreeSet::new();
                for v in start..start + 16 {
                    let (c, o) = a.claim(v).unwrap_or((' ', None));
                    line.push(if v == va { '[' } else { ' ' });
                    line.push(c);
                    owners.extend(o);
                }
                let owners: Vec<String> = owners
                    .iter()
                    .map(|o| format!("{o:#x}{}", a.functions.get(o).map_or(String::new(), |f| format!("({:?})", f.source))))
                    .collect();
                println!("{line}  {}", owners.join(" "));
            }
            if let Some(f) = a.functions.get(&va) {
                println!("  function start, found by {:?}, returns: {}", f.source, f.returns);
            }
            if a.labels.contains(&va) {
                println!("  label (address-taken code inside a function)");
            }
            println!();
        }
        return Ok(());
    }
    print!("{a}");
    if let Some(out) = functions_out {
        let mut s = String::new();
        for f in a.functions.values() {
            s.push_str(&format!("{:#010x} {:<10} {:?}\n", f.start, f.section, f.source));
        }
        fs::write(&out, s)?;
        println!("wrote {} functions to {}", a.functions.len(), out.display());
    }
    if let Some(tsv) = against {
        let text = fs::read_to_string(&tsv).with_context(|| format!("reading {}", tsv.display()))?;
        let theirs = analyze::compare::parse_ghidra_tsv(&text);
        println!("\ncompared with {} ({} functions)", tsv.display(), theirs.len());
        print!("{}", analyze::compare::compare(&a, &theirs));
    }
    Ok(())
}
