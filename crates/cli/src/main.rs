use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use xiso::Xiso;

mod anim;
mod bitmap;
mod collision;
mod effects;
mod fonts;
mod maps;
mod movies_cmd;
mod safe_out;
mod scripts;
mod sound;
mod strings;
mod ui;

const USAGE: &str = "usage: h2 <command> [disc]

  info                    title, sections, libraries
  ls                      files on the disc
  maps [disc.iso]         map headers and tag counts on the disc
  tags <map> [group]      tags in a disc map or local .map; optional --disc <iso>
  tag <map> <tag name>    decode one tag; optional group and --disc <iso>
  bitmap <map> <tag>      decode a bitmap tag to PNG (--image N --out DIR --mips)
  bitmap --all <map>      bulk decode check of every bitmap in a map
  collision <map> <tag>   summarise a coll/phmo tag; --obj FILE exports geometry
  collision --all <map|all>  validate every coll/phmo tag (all = every disc map)
  sound <map> <tag>       export a snd! tag to WAV; --all <map> validates every sound
                          (options --out DIR, --disc <iso>)
  strings <map>           localized text; --lang en, --tag name, --all, --disc iso
  font <font> <text> --out <png>
                          render text with a /maps/fonts file; --all validates every font
                          (options --color RRGGBB[AA], --force, --disc <iso>)
  movies <iso> --out DIR [--ffmpeg PATH] [--force]
                          convert the Bink movies to MP4 with your own ffmpeg
                          (optional; exit 3 if ffmpeg is missing, 4 if unusable)
  effect <map> <tag>      print an effects-family tag (effe tree); --all <map|all> validates the family
                          (option --disc <iso>)
  hs <map> [--script name] [--builtins] [--disc iso]
  hs --all [--disc iso]   verify all disc maps (codec symmetry and node counts)
  ui <map> <screen tag>   print a wgit screen's widget tree (no localized text);
                          --all <map|all> validates every UI tag (option --disc <iso>)
  anim <map> <jmad tag>   animation graph summary; --anim NAME for one animation,
                          --json PATH to dump sampled transforms; --all validates the disc
  extract [disc] [out-dir] [--force]
                          copy every file off the disc (default disc/extracted)
  extract-xbe [disc] [out] [--force]
                          just default.xbe (default disc/default.xbe). With two
                          arguments it is <disc> <out>: the output comes LAST
  (Output is never written over an existing file without --force, and never
  over the disc being read, a .iso/.xiso or anything holding a disc image.)
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
    if cmd == "collision" {
        return collision::run(args.collect());
    }
    if cmd == "anim" {
        return anim::run(args.collect());
    }
    if cmd == "bitmap" {
        return bitmap::run(args.collect());
    }
    if cmd == "effect" {
        return effects::run(args.collect());
    }
    if cmd == "hs" {
        return scripts::run(args.collect());
    }
    if cmd == "ui" {
        return ui::run(args.collect());
    }
    let mut positional = Vec::new();
    let mut functions_out = None;
    let mut against = None;
    let mut explain = Vec::new();
    let mut list = false;
    let mut no_xcheck = false;
    let mut db_dir = None;
    let mut force = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--functions" => functions_out = Some(PathBuf::from(args.next().context("--functions needs a path")?)),
            "--explain" => explain.push(args.next().context("--explain needs an address")?),
            "--against" => against = Some(PathBuf::from(args.next().context("--against needs a path")?)),
            "--list" => list = true,
            "--force" => force = true,
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
        ("extract", [out]) => extract(&disc, out, force),
        ("extract", []) => extract(&disc, &repo_root().join("disc/extracted"), force),
        ("extract-xbe", [out]) => extract_xbe(&disc, out, force),
        ("extract-xbe", []) => extract_xbe(&disc, &repo_root().join("disc/default.xbe"), force),
        ("analyze", []) => run_analyze(&disc, functions_out, against, &explain, force),
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
    safe_out::protect(path);
    let f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    Ok(Xiso::open(BufReader::new(f))?)
}

/// Accepts a raw XBE or a disc image, returning the XBE bytes.
fn load_xbe(path: &Path) -> Result<Vec<u8>> {
    safe_out::protect(path);
    let mut magic = [0u8; 4];
    File::open(path).with_context(|| format!("opening {}", path.display()))?.read_exact(&mut magic)?;
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

fn extract(path: &Path, out: &Path, force: bool) -> Result<()> {
    let mut iso = open_iso(path)?;
    extract_to(&mut iso, path, out, force)
}

fn extract_to<R: Read + std::io::Seek>(iso: &mut Xiso<R>, path: &Path, out: &Path, force: bool) -> Result<()> {
    let entries = iso.entries()?;
    if out.is_file() {
        bail!("{} is a file, not an output directory (is the argument order <disc> <out-dir>?)", out.display());
    }
    let dir = safe_out::OutDir::new(out, force)?.protecting(path);
    for e in &entries {
        // Paths come from the disc: refuse anything that could escape `out`.
        let rel = e.path.trim_start_matches('/');
        if rel.split('/').any(|p| p.is_empty() || p == "." || p == ".." || p.contains('\\')) {
            bail!("refusing unsafe path on disc: {:?}", e.path);
        }
        let comps: Vec<String> = rel.split('/').map(str::to_string).collect();
        if e.is_dir {
            dir.create_dir(&comps).with_context(|| format!("creating {}", e.path))?;
            continue;
        }
        // Streams straight into the guarded temp file; published only when whole.
        dir.write_with(&comps, |f| {
            iso.copy_to(e, f).map(drop).map_err(|x| std::io::Error::other(x.to_string()))
        })
        .with_context(|| format!("writing {}", e.path))?;
    }
    println!("extracted {} entries to {}", entries.len(), out.display());
    Ok(())
}

/// Write one file through the guarded [`safe_out::OutDir`]: no overwrite
/// without `force`, never the disc being read, never a disc image.
fn write_guarded(dest: &Path, source: &Path, force: bool, bytes: &[u8]) -> Result<()> {
    safe_out::guard_target(dest, Some(source))?;
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{} is not a file path", dest.display()))?
        .to_string();
    let parent = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    safe_out::OutDir::new(parent, force)?
        .protecting(source)
        .write(&[name], bytes)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("writing {}", dest.display()))?;
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

fn extract_xbe(disc: &Path, out: &Path, force: bool) -> Result<()> {
    // Check before doing any work, so a swapped argument costs nothing.
    safe_out::guard_target(out, Some(disc))?;
    let data = load_xbe(disc)?;
    write_guarded(out, disc, force, &data)?;
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
                if st.contains(&m.va) { on += 1 } else { off += 1 }
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

fn run_analyze(
    path: &Path,
    functions_out: Option<PathBuf>,
    against: Option<PathBuf>,
    explain: &[String],
    force: bool,
) -> Result<()> {
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
                    .map(|o| {
                        format!("{o:#x}{}", a.functions.get(o).map_or(String::new(), |f| format!("({:?})", f.source)))
                    })
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
        write_guarded(&out, path, force, s.as_bytes())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening a disc registers it, so any exporter's `OutDir` refuses it even
    /// under `--force` and even when the file has an innocent name.
    #[test]
    fn opened_discs_are_protected_from_every_exporter() {
        use std::io::{Seek, SeekFrom, Write};
        let d = std::env::temp_dir().join(format!("h2-main-test-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let disc = d.join("pretty.png");
        let mut f = File::create(&disc).unwrap();
        f.seek(SeekFrom::Start(0x10000)).unwrap();
        f.write_all(b"MICROSOFT*XBOX*MEDIA").unwrap();
        f.write_all(&[0u8; 8]).unwrap();
        drop(f);
        let before = fs::read(&disc).unwrap();
        open_iso(&disc).unwrap();
        let out = safe_out::OutDir::new(&d, true).unwrap();
        assert!(out.write(&["pretty.png".to_string()], b"png").is_err());
        assert_eq!(fs::read(&disc).unwrap(), before);
        fs::remove_dir_all(d).unwrap();
    }

    /// Read + Seek over an image that records its largest single read.
    struct Spy {
        inner: std::io::Cursor<Vec<u8>>,
        max_read: std::rc::Rc<std::cell::Cell<usize>>,
    }
    impl Read for Spy {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.max_read.set(self.max_read.get().max(buf.len()));
            self.inner.read(buf)
        }
    }
    impl std::io::Seek for Spy {
        fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(to)
        }
    }

    fn dirent(left: u16, right: u16, sector: u32, size: u32, attr: u8, name: &str) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend(left.to_le_bytes());
        v.extend(right.to_le_bytes());
        v.extend(sector.to_le_bytes());
        v.extend(size.to_le_bytes());
        v.push(attr);
        v.push(name.len() as u8);
        v.extend(name.as_bytes());
        while v.len() % 4 != 0 {
            v.push(0xFF);
        }
        v
    }

    const BIG: usize = 1 << 20;

    /// A tiny XISO: `/big` (1 MiB patterned file) and an empty directory `/empty`.
    fn tiny_xiso() -> Vec<u8> {
        let mut img = vec![0u8; 40 * 2048 + BIG];
        img[0x10000..0x10014].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
        img[0x10014..0x10018].copy_from_slice(&33u32.to_le_bytes());
        img[0x10018..0x1001C].copy_from_slice(&2048u32.to_le_bytes());
        let big = dirent(0, 5, 40, BIG as u32, 0, "big");
        assert_eq!(big.len(), 20);
        let empty = dirent(0, 0, 34, 0, 0x10, "empty");
        let mut root = [big, empty].concat();
        root.resize(2048, 0xFF);
        img[33 * 2048..34 * 2048].copy_from_slice(&root);
        for (i, b) in img[40 * 2048..].iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        img
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-main-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn extract_streams_files_and_creates_empty_directories() {
        let d = scratch("extract");
        let max_read = std::rc::Rc::new(std::cell::Cell::new(0));
        let spy = Spy { inner: std::io::Cursor::new(tiny_xiso()), max_read: max_read.clone() };
        let mut iso = Xiso::open(spy).unwrap();
        let out = d.join("out");
        extract_to(&mut iso, &d.join("disc.xiso"), &out, false).unwrap();
        // Empty directories are created.
        assert!(out.join("empty").is_dir());
        // File contents are right...
        let got = fs::read(out.join("big")).unwrap();
        assert_eq!(got.len(), BIG);
        assert!(got.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8));
        // ...and arrived in small reads, never as one whole-file buffer.
        assert!(max_read.get() <= 64 << 10, "largest read was {} bytes", max_read.get());
        // Re-extracting over existing files needs --force.
        assert!(extract_to(&mut iso, &d.join("disc.xiso"), &out, false).is_err());
        extract_to(&mut iso, &d.join("disc.xiso"), &out, true).unwrap();
        fs::remove_dir_all(d).unwrap();
    }

    /// Registration must not depend on the disc-magic fallback: a file that is
    /// not a disc at all is still protected once a loader has seen it.
    #[test]
    fn loaders_register_their_input_even_without_disc_magic() {
        let d = scratch("register");
        let junk = d.join("junk.dat");
        fs::write(&junk, b"not a disc at all").unwrap();
        assert!(open_iso(&junk).is_err());
        let xbe = d.join("x.dat");
        fs::write(&xbe, b"XBEH synthetic").unwrap();
        load_xbe(&xbe).unwrap();
        let out = safe_out::OutDir::new(&d, true).unwrap();
        for n in ["junk.dat", "x.dat"] {
            assert!(out.write(&[n.to_string()], b"x").is_err(), "{n} was overwritten");
        }
        assert_eq!(fs::read(&junk).unwrap(), b"not a disc at all");
        assert_eq!(fs::read(&xbe).unwrap(), b"XBEH synthetic");
        fs::remove_dir_all(d).unwrap();
    }
}
