use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use xiso::Xiso;

mod bitmap;
mod fonts;
mod maps;
mod movies_cmd;
mod scripts;
mod safe_out;
mod sound;
mod strings;

const USAGE: &str = "usage: h2 <command> [disc]

  info                    title, sections, libraries
  ls                      files on the disc
  maps [disc.iso]         map headers and tag counts on the disc
  tags <map> [group]      tags in a disc map or local .map; optional --disc <iso>
  tag <map> <tag name>    decode one tag; optional group and --disc <iso>
  bitmap <map> <tag>      decode a bitmap tag to PNG (--image N --out DIR --mips)
  bitmap --all <map>      bulk decode check of every bitmap in a map
  sound <map> <tag>       export a snd! tag to WAV; --all <map> validates every sound
                          (options --out DIR, --disc <iso>)
  strings <map>           localized text; --lang en, --tag name, --all, --disc iso
  font <font> <text> --out <png>
                          render text with a /maps/fonts file; --all validates every font
                          (options --color RRGGBB[AA], --force, --disc <iso>)
  movies <iso> --out DIR [--ffmpeg PATH] [--force]
                          convert the Bink movies to MP4 with your own ffmpeg
                          (optional; exit 3 if ffmpeg is missing, 4 if unusable)
  hs <map> [--script name] [--builtins] [--disc iso]
  hs --all [--disc iso]   verify all disc maps (codec symmetry and node counts)
  extract [out-dir]       copy every file off the disc (default disc/extracted)
  extract-xbe [out]       just default.xbe (default disc/default.xbe)
  disasm <addr> [count]   decode instructions at a virtual address
  analyze [--functions <out.txt>] [--against <ghidra-functions.tsv>] [--explain <addr>]...
                          function discovery and coverage
  symbols [--list] [--no-xcheck] [--db <dir>]
                          identify statically linked XDK library functions
                          (needs scripts/setup-xbsymboldb.sh)

[disc] is an .iso or a default.xbe. Without it, h2 uses $H2_DISC, then the
first .iso in the repo's disc/ folder.";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else { bail!("{USAGE}") };
    if cmd == "sound" {
        return sound::run(args.collect());
    }
    if cmd == "strings" {
        return strings::run(args.collect());
    }
    if matches!(cmd.as_str(), "maps" | "tags" | "tag") {
        return maps::run(&cmd, args.collect());
    }
    if cmd == "font" {
        return fonts::run(args.collect());
    }
    if cmd == "movies" {
        std::process::exit(movies_cmd::run(args.collect()));
    }
    if cmd == "bitmap" {
        return bitmap::run(args.collect());
    }
    if cmd == "hs" {
        return scripts::run(args.collect());
    }
    let mut positional = Vec::new();
    let mut functions_out = None;
    let mut against = None;
    let mut explain = Vec::new();
    let mut list = false;
    let mut no_xcheck = false;
    let mut db_dir = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--functions" => functions_out = Some(PathBuf::from(args.next().context("--functions needs a path")?)),
            "--explain" => explain.push(args.next().context("--explain needs an address")?),
            "--against" => against = Some(PathBuf::from(args.next().context("--against needs a path")?)),
            "--list" => list = true,
            "--no-xcheck" => no_xcheck = true,
            "--db" => db_dir = Some(PathBuf::from(args.next().context("--db needs a path")?)),
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
        ("symbols", []) => run_symbols(&disc, list, !no_xcheck, db_dir),
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
        u32::from_str_radix(s.trim_start_matches("0x"), 16).with_context(|| format!("bad hex: {s}"))
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

fn run_symbols(path: &Path, list: bool, xcheck: bool, db_dir: Option<PathBuf>) -> Result<()> {
    use std::collections::BTreeSet;
    use symbols::Kind;
    let data = load_xbe(path)?;
    let x = xbe::Xbe::parse(&data)?;
    let dir = db_dir.unwrap_or_else(|| symbols::default_db_dir(&repo_root()));
    let r = symbols::identify(&x, &dir)?;
    let starts: Option<BTreeSet<u32>> =
        if xcheck { Some(analyze::analyze(&x)?.functions.keys().copied().collect()) } else { None };

    println!("linked libraries");
    for l in &x.libraries {
        println!("  {:<10} {}.{}.{}", l.name, l.major, l.minor, l.build);
    }
    if !r.unscanned_libraries.is_empty() {
        println!("  (no signature database for: {})", r.unscanned_libraries.join(", "));
    }
    println!("\nlibrary     found  candidates  ltcg  ambiguous  on-start  off-start");
    let mut total = 0;
    for (lib, (n, amb, ltcg)) in &r.counts() {
        let (mut on, mut off) = (0, 0);
        if let Some(st) = &starts {
            for m in r.matches.iter().filter(|m| m.library == *lib && m.kind == Kind::Function) {
                if st.contains(&m.va) {
                    on += 1
                } else {
                    off += 1
                }
            }
        }
        let cand = r.candidate_symbols.get(lib).copied().unwrap_or(0);
        let (a, b) = if starts.is_some() { (on.to_string(), off.to_string()) } else { ("-".into(), "-".into()) };
        println!("{lib:<10} {n:>6} {cand:>11} {ltcg:>5} {amb:>10} {a:>9} {b:>10}");
        total += n;
    }
    println!("{:<10} {total:>6}", "total");
    if let Some(st) = &starts {
        let uniq: BTreeSet<u32> = r.matches.iter().filter(|m| m.kind == Kind::Function).map(|m| m.va).collect();
        let on = uniq.iter().filter(|v| st.contains(v)).count();
        let shared = r.matches.iter().filter(|m| m.shared).map(|m| m.va).collect::<BTreeSet<_>>().len();
        println!("{shared} addresses carry more than one name (weak signatures from different library flavours)");
        println!(
            "\n{} distinct addresses identified: {} are `h2 analyze` function starts, {} are not",
            uniq.len(),
            on,
            uniq.len() - on
        );
    }
    println!(
        "{} signature tasks never ran (they reference a symbol that was not found); {} table entries named a missing signature, {} loose matches dropped",
        r.unresolved_tasks, r.missing_sigs, r.dropped_ambiguous
    );
    if list {
        println!("\nunresolved references (tasks waiting on each):");
        for (n, c) in &r.missing_refs {
            println!("  {n} {c}");
        }
        println!();
        for m in &r.matches {
            let flag = starts.as_ref().map_or("", |s| if s.contains(&m.va) { "" } else { " [not a function start]" });
            println!(
                "{:#010x} {:<9} {:<5} {}{}{}{}{}",
                m.va,
                m.library,
                m.xdk_version,
                m.name,
                if m.ltcg { " (ltcg)" } else { "" },
                if m.ambiguous { " (ambiguous)" } else { "" },
                if m.shared { " (shared)" } else { "" },
                flag
            );
        }
    }
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
