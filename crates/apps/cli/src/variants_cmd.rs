//! `h2 variants port`: makes self-contained `.h2variant` files (version 2,
//! `variants::portable::port`) from the variants people already have
//! (docs/editor/variants.md, "Portable files"). Pure over its inputs except
//! `--all-builtins`, which reads the retail built-ins from the disc's main
//! menu map. [`port_text`] does the work, so tests run it without a
//! process.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use variants::portable::{self, Bundle, PortScript};
use variants::record::GameVariant;

const USAGE: &str = "usage: h2 variants port <input>... [--script builtin|keep|<file.rules|file.rir>] [-o <dir>]
       h2 variants port --all-builtins [disc] [--script builtin|keep] [-o <dir>]
  port writes one version-2 .h2variant per input, named by the variant's
  name, and prints each file's id (the SHA-256 of its canonical bytes) and
  path. An input is a raw 0x130-byte record, a saved-variant file (the
  record and its signature), a hopper record slot (0x37c bytes) or a
  .h2variant of either version.
  --script builtin (the default) attaches the built-in script of the
  record's engine; keep leaves the variant unscripted, playing retail
  rules; a .rules file is compiled and travels with its source, a .rir
  blob travels alone. The same input and script give the same id.
  --all-builtins writes every retail built-in variant. It needs the disc:
  the built-ins are read from mainmenu.map's goof tag. [disc] is an .iso;
  without it, $H2_DISC or the .iso in disc/. Community variants are added
  by the game at boot and are not ported here.
  -o is the output directory (default: the current one).";

pub fn run(words: Vec<String>) -> Result<()> {
    let mut it = words.into_iter();
    let Some(sub) = it.next() else {
        bail!("{USAGE}")
    };
    let rest: Vec<String> = it.collect();
    match sub.as_str() {
        "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        "port" => port_cmd(&rest),
        _ => bail!("{USAGE}"),
    }
}

/// `port`'s words.
#[derive(Debug, Default, PartialEq)]
struct Parsed {
    inputs: Vec<PathBuf>,
    script: Option<String>,
    out: Option<PathBuf>,
    all_builtins: bool,
}

fn parse(words: &[String]) -> Result<Parsed> {
    let mut p = Parsed::default();
    let mut it = words.iter();
    while let Some(w) = it.next() {
        match w.as_str() {
            "-o" | "--out" => p.out = Some(PathBuf::from(it.next().context("-o requires a directory")?)),
            "--script" => p.script = Some(it.next().context("--script requires a value")?.clone()),
            "--all-builtins" => p.all_builtins = true,
            "-h" | "--help" => bail!("{USAGE}"),
            f if !f.starts_with('-') => p.inputs.push(PathBuf::from(f)),
            _ => bail!("{USAGE}"),
        }
    }
    if p.inputs.is_empty() && !p.all_builtins {
        bail!("{USAGE}");
    }
    Ok(p)
}

/// What `--script` asks for.
fn script(arg: Option<&str>) -> Result<PortScript> {
    Ok(match arg {
        None | Some("builtin") => PortScript::Builtin,
        Some("keep") => PortScript::Keep,
        Some(path) => {
            let p = Path::new(path);
            match p.extension().and_then(|e| e.to_str()) {
                Some("rules") => PortScript::Source(
                    std::fs::read_to_string(p).with_context(|| format!("reading {path}"))?,
                ),
                Some("rir") => {
                    PortScript::Ir(std::fs::read(p).with_context(|| format!("reading {path}"))?)
                }
                _ => bail!("--script takes builtin, keep, a .rules file or a .rir blob"),
            }
        }
    })
}

/// Ports one input's bytes: the bundle, or why not.
pub fn port_bytes(bytes: &[u8], script: PortScript) -> Result<Bundle, portable::PortableError> {
    let (v, description) = portable::port_input(bytes)?;
    portable::port(&v, None, description.as_deref(), script)
}

/// Ports a variant that is already decoded (a retail built-in).
pub fn port_variant(v: &GameVariant, script: PortScript) -> Result<Bundle, portable::PortableError> {
    portable::port(v, None, None, script)
}

/// Writes `b` into `dir` under its name, avoiding names already written
/// this run; returns the line `port` prints.
fn write(dir: &Path, b: &Bundle, taken: &mut Vec<String>) -> Result<String> {
    let base = portable::file_name(&b.name);
    let mut name = base.clone();
    let mut n = 2;
    while taken.contains(&name) {
        let stem = base.trim_end_matches(&format!(".{}", portable::EXTENSION));
        name = format!("{stem} {n}.{}", portable::EXTENSION);
        n += 1;
    }
    taken.push(name.clone());
    let path = dir.join(&name);
    let pretty = serde_json::to_string_pretty(b).expect("plain data serializes");
    std::fs::write(&path, pretty + "\n").with_context(|| format!("writing {}", path.display()))?;
    Ok(format!("{}  {}", b.id(), path.display()))
}

/// Ports `inputs` (path, bytes) to bundles: one result per input, in order.
pub fn port_text(
    inputs: &[(String, Vec<u8>)],
    script: &PortScript,
) -> Vec<(String, Result<Bundle, portable::PortableError>)> {
    inputs
        .iter()
        .map(|(label, bytes)| (label.clone(), port_bytes(bytes, script.clone())))
        .collect()
}

fn port_cmd(words: &[String]) -> Result<()> {
    let p = parse(words)?;
    let script = script(p.script.as_deref())?;
    let dir = p.out.clone().unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut taken = Vec::new();
    let mut failed = 0;
    let mut results: Vec<(String, Result<Bundle, portable::PortableError>)> = Vec::new();
    if p.all_builtins {
        for (name, v) in retail_builtins(p.inputs.first().map(PathBuf::as_path))? {
            results.push((name, port_variant(&v, script.clone())));
        }
    } else {
        let mut inputs = Vec::new();
        for path in &p.inputs {
            let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            inputs.push((path.display().to_string(), bytes));
        }
        results = port_text(&inputs, &script);
    }
    for (label, result) in results {
        match result {
            Ok(b) => println!("{}", write(&dir, &b, &mut taken)?),
            Err(e) => {
                eprintln!("{label}: {e}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} variant(s) not ported");
    }
    Ok(())
}

/// The retail built-in variants, built as the game builds them at load
/// (`variants::Schema::build`) from the disc's `goof` tag.
#[cfg(feature = "desktop")]
fn retail_builtins(disc: Option<&Path>) -> Result<Vec<(String, GameVariant)>> {
    let disc = match disc {
        Some(d) => d.to_path_buf(),
        None => crate::default_disc()
            .context("--all-builtins needs the disc: the retail built-ins are in mainmenu.map")?,
    };
    let disc = disc.as_path();
    let d = crate::live_art::Disc::scan(disc)
        .with_context(|| format!("reading the disc {}", disc.display()))?;
    let m = d
        .maps_of(map::CacheType::MainMenu)
        .next()
        .context("no main menu map on the disc")?;
    let (mut map, _) = d.open(m)?;
    let schema = variants::Schema::from_map(&mut map, tags::Language::English)
        .context("reading the built-in variants from mainmenu.map")?;
    schema
        .builtins
        .iter()
        .map(|b| {
            let v = schema
                .build(b)
                .with_context(|| format!("building the built-in {:?}", b.name))?;
            Ok((b.name.clone(), v))
        })
        .collect()
}

#[cfg(not(feature = "desktop"))]
fn retail_builtins(_disc: Option<&Path>) -> Result<Vec<(String, GameVariant)>> {
    bail!("--all-builtins reads the disc, which this build of h2 cannot (it needs the desktop feature)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use variants::record::GameType;

    fn king() -> GameVariant {
        let mut v = GameVariant::blank(GameType::King);
        v.set_name("Hill").unwrap();
        v
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn parses_port_words() {
        let p = parse(&words("a.bin b.h2variant --script keep -o out")).unwrap();
        assert_eq!(p.inputs, [PathBuf::from("a.bin"), PathBuf::from("b.h2variant")]);
        assert_eq!(p.script.as_deref(), Some("keep"));
        assert_eq!(p.out, Some(PathBuf::from("out")));
        assert!(parse(&words("--all-builtins")).unwrap().all_builtins);
        assert!(parse(&[]).is_err());
        assert!(parse(&words("a --bogus")).is_err());
        assert!(script(Some("x.txt")).is_err());
    }

    #[test]
    fn ports_each_kind_of_input_the_same() {
        let v = king();
        let raw = v.to_bytes().to_vec();
        let save = variants::save::encode_file(&v, &variants::save::RUNTIME_KEY);
        let mut slot = vec![0u8; portable::HOPPER_SLOT_RECORD_AT];
        slot.extend_from_slice(&raw);
        let v1 = portable::export(&v).into_bytes();
        let inputs: Vec<(String, Vec<u8>)> = [raw, save, slot, v1]
            .into_iter()
            .enumerate()
            .map(|(i, b)| (format!("in{i}"), b))
            .collect();
        let out = port_text(&inputs, &PortScript::Builtin);
        let ids: Vec<String> = out.iter().map(|(_, r)| r.as_ref().unwrap().id()).collect();
        assert!(ids.iter().all(|i| *i == ids[0]), "{ids:?}");
        let b = out[0].1.as_ref().unwrap();
        assert_eq!(b.script.as_ref().unwrap().name, "King of the Hill");
        // Keep gives another file, with no script.
        let kept = port_text(&inputs[..1], &PortScript::Keep);
        let k = kept[0].1.as_ref().unwrap();
        assert!(k.script.is_none());
        assert_ne!(k.id(), ids[0]);
        // A short file is refused by name.
        let bad = port_text(&[("short".into(), vec![0; 4])], &PortScript::Builtin);
        assert_eq!(bad[0].1.as_ref().unwrap_err(), &portable::PortableError::Record);
    }

    #[test]
    fn writes_files_by_name_without_clobbering() {
        let dir = std::env::temp_dir().join(format!("h2-variants-port-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let b = port_variant(&king(), PortScript::Builtin).unwrap();
        let mut taken = Vec::new();
        let first = write(&dir, &b, &mut taken).unwrap();
        let second = write(&dir, &b, &mut taken).unwrap();
        assert!(first.starts_with(&b.id()));
        assert!(first.ends_with("Hill.h2variant"));
        assert!(second.ends_with("Hill 2.h2variant"));
        let text = std::fs::read_to_string(dir.join("Hill.h2variant")).unwrap();
        assert_eq!(portable::read(&text).unwrap().id(), b.id());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
