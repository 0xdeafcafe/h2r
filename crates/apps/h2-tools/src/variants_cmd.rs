//! `h2 variants port|from-script`: makes self-contained `.h2variant` files
//! (version 2). `port` makes them from the variants people already have
//! (`variants::portable::port`), `from-script` from a rules script's own
//! variant (`variants::portable::from_script`; docs/editor/variants.md,
//! "Portable files"). Pure over their inputs except `--all-builtins`,
//! which reads the retail built-ins from the disc's main menu map.
//! [`port_text`] and [`from_script_bundle`] do the work, so tests run
//! them without a process.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rules::ir::OptionValue;
use variants::portable::{self, Bundle, PortScript};
use variants::record::GameVariant;

const USAGE: &str = "usage: h2 variants port <input>... [--script builtin|<file.rules|file.rir>]
                        [--name <name>] [--description <text>] [--option <key>=<value>]... [-o <dir>]
       h2 variants port --all-builtins [disc] [--script builtin] [--option <key>=<value>]... [-o <dir>]
       h2 variants from-script <file.rules> [--option <key>=<value>]... [-o <dir>]
  port writes one version-2 .h2variant per input, named by the variant's
  name, and prints each file's id (the SHA-256 of its canonical bytes) and
  path. An input is a raw 0x130-byte record, a saved-variant file (the
  record and its signature), a hopper record slot (0x37c bytes) or a
  .h2variant of either version.
  --script builtin (the default) attaches the built-in script of the
  record's engine; a .rules file is compiled and travels with its source,
  a .rir blob travels alone. Every ported variant names a script. The
  script's settings are the only extras the record carries: the input's
  own `hx` extras are not kept. The same input and script give the same id.
  --name and --description replace the variant's name and description;
  they take one input. --option sets a script option by name (an int,
  true or false, or an enum value's name), checked against the attached
  script's options; it repeats, and an option left out takes its default.
  --all-builtins writes every retail built-in variant. It needs the disc:
  the built-ins are read from mainmenu.map's goof tag. [disc] is an .iso;
  without it, $H2_DISC or the .iso in disc/. Community variants are added
  by the game at boot and are not ported here.
  from-script writes the .h2variant of the script's own variant: the stock
  record of its carrier with the script's settings, named by the script,
  with its about line as the description and the script and its source
  attached. It prints the file's id and path; the same script and options
  give the same id. --option is as for port. A script check refuses, or
  whose record the game would not keep, writes nothing.
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
        "from-script" => from_script_cmd(&rest),
        _ => bail!("{USAGE}"),
    }
}

/// `from-script`'s words: one script, its options, the directory.
fn parse_from_script(words: &[String]) -> Result<Parsed> {
    let p = parse(words)?;
    if p.inputs.len() != 1
        || p.all_builtins
        || p.script.is_some()
        || p.name.is_some()
        || p.description.is_some()
    {
        bail!("{USAGE}");
    }
    Ok(p)
}

/// The bundle of the script in `src`: compiled, verified and held to the
/// budget (`rules::ship`), then [`portable::from_script`] with its source
/// attached. Errors name `label`.
pub fn from_script_bundle(
    label: &str,
    src: &str,
    options: &[(String, OptionValue)],
) -> Result<Bundle, String> {
    let (ir, _) = rules::ship(src).map_err(|e| format!("{label}: {e}"))?;
    portable::from_script(&ir, Some(src), options).map_err(|e| format!("{label}: {e}"))
}

fn from_script_cmd(words: &[String]) -> Result<()> {
    let p = parse_from_script(words)?;
    let path = &p.inputs[0];
    let label = path.display().to_string();
    let src = std::fs::read_to_string(path).with_context(|| format!("reading {label}"))?;
    let b = match from_script_bundle(&label, &src, &p.settings().options) {
        Ok(b) => b,
        Err(e) => bail!("{e}"),
    };
    let dir = p.out.clone().unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    println!("{}", write(&dir, &b, &mut Vec::new())?);
    Ok(())
}

/// `port`'s words.
#[derive(Debug, Default, PartialEq)]
struct Parsed {
    inputs: Vec<PathBuf>,
    script: Option<String>,
    out: Option<PathBuf>,
    all_builtins: bool,
    name: Option<String>,
    description: Option<String>,
    options: Vec<(String, String)>,
}

/// What every input is ported with besides its script.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Settings {
    pub name: Option<String>,
    pub description: Option<String>,
    pub options: Vec<(String, OptionValue)>,
}

impl Parsed {
    fn settings(&self) -> Settings {
        Settings {
            name: self.name.clone(),
            description: self.description.clone(),
            options: self
                .options
                .iter()
                .map(|(k, v)| (k.clone(), OptionValue::parse(v)))
                .collect(),
        }
    }
}

fn parse(words: &[String]) -> Result<Parsed> {
    let mut p = Parsed::default();
    let mut it = words.iter();
    while let Some(w) = it.next() {
        match w.as_str() {
            "-o" | "--out" => p.out = Some(PathBuf::from(it.next().context("-o requires a directory")?)),
            "--script" => p.script = Some(it.next().context("--script requires a value")?.clone()),
            "--all-builtins" => p.all_builtins = true,
            "--name" => p.name = Some(it.next().context("--name requires a name")?.clone()),
            "--description" => {
                p.description = Some(it.next().context("--description requires text")?.clone());
            }
            "--option" => {
                let kv = it.next().context("--option requires <key>=<value>")?;
                let (k, v) = kv
                    .split_once('=')
                    .filter(|(k, _)| !k.is_empty())
                    .with_context(|| format!("--option takes <key>=<value>, not {kv:?}"))?;
                p.options.push((k.to_string(), v.to_string()));
            }
            "-h" | "--help" => bail!("{USAGE}"),
            f if !f.starts_with('-') => p.inputs.push(PathBuf::from(f)),
            _ => bail!("{USAGE}"),
        }
    }
    if p.inputs.is_empty() && !p.all_builtins {
        bail!("{USAGE}");
    }
    if (p.name.is_some() || p.description.is_some()) && (p.all_builtins || p.inputs.len() > 1) {
        bail!("--name and --description take one input");
    }
    Ok(p)
}

/// What `--script` asks for.
fn script(arg: Option<&str>) -> Result<PortScript> {
    Ok(match arg {
        None | Some("builtin") => PortScript::Builtin,
        Some(path) => {
            let p = Path::new(path);
            match p.extension().and_then(|e| e.to_str()) {
                Some("rules") => PortScript::Source(
                    std::fs::read_to_string(p).with_context(|| format!("reading {path}"))?,
                ),
                Some("rir") => {
                    PortScript::Ir(std::fs::read(p).with_context(|| format!("reading {path}"))?)
                }
                _ => bail!("--script takes builtin, a .rules file or a .rir blob"),
            }
        }
    })
}

/// Ports one input's bytes: the bundle, or why not. The file's own
/// description stays unless `with` replaces it.
pub fn port_bytes(
    bytes: &[u8],
    script: PortScript,
    with: &Settings,
) -> Result<Bundle, portable::PortableError> {
    let (v, description) = portable::port_input(bytes)?;
    let description = with.description.clone().or(description);
    portable::port_with_options(&v, with.name.as_deref(), description.as_deref(), script, &with.options)
}

/// Ports a variant that is already decoded (a retail built-in).
pub fn port_variant(
    v: &GameVariant,
    script: PortScript,
    with: &Settings,
) -> Result<Bundle, portable::PortableError> {
    portable::port_with_options(v, with.name.as_deref(), with.description.as_deref(), script, &with.options)
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
    with: &Settings,
) -> Vec<(String, Result<Bundle, portable::PortableError>)> {
    inputs
        .iter()
        .map(|(label, bytes)| (label.clone(), port_bytes(bytes, script.clone(), with)))
        .collect()
}

fn port_cmd(words: &[String]) -> Result<()> {
    let p = parse(words)?;
    let script = script(p.script.as_deref())?;
    let with = p.settings();
    let dir = p.out.clone().unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut taken = Vec::new();
    let mut failed = 0;
    let mut results: Vec<(String, Result<Bundle, portable::PortableError>)> = Vec::new();
    if p.all_builtins {
        for (name, v) in retail_builtins(p.inputs.first().map(PathBuf::as_path))? {
            results.push((name, port_variant(&v, script.clone(), &with)));
        }
    } else {
        let mut inputs = Vec::new();
        for path in &p.inputs {
            let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            inputs.push((path.display().to_string(), bytes));
        }
        results = port_text(&inputs, &script, &with);
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
fn retail_builtins(disc: Option<&Path>) -> Result<Vec<(String, GameVariant)>> {
    let disc = match disc {
        Some(d) => d.to_path_buf(),
        None => crate::default_disc()
            .context("--all-builtins needs the disc: the retail built-ins are in mainmenu.map")?,
    };
    let mut map = crate::maps::load_map("mainmenu", Some(disc.clone()))
        .with_context(|| format!("reading mainmenu.map from the disc {}", disc.display()))?;
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
        let p = parse(&words("a.bin b.h2variant --script builtin -o out")).unwrap();
        assert_eq!(p.inputs, [PathBuf::from("a.bin"), PathBuf::from("b.h2variant")]);
        assert_eq!(p.script.as_deref(), Some("builtin"));
        assert_eq!(p.out, Some(PathBuf::from("out")));
        assert!(parse(&words("--all-builtins")).unwrap().all_builtins);
        assert!(parse(&[]).is_err());
        assert!(parse(&words("a --bogus")).is_err());
        assert!(script(Some("x.txt")).is_err());
    }

    #[test]
    fn parses_name_description_and_options() {
        let p = parse(&[
            "a.bin".into(),
            "--name".into(),
            "Big Hill".into(),
            "--description".into(),
            "One hill, more points".into(),
            "--option".into(),
            "hill_points=3".into(),
            "--option".into(),
            "moving=false".into(),
        ])
        .unwrap();
        assert_eq!(p.name.as_deref(), Some("Big Hill"));
        assert_eq!(p.description.as_deref(), Some("One hill, more points"));
        assert_eq!(
            p.settings().options,
            [
                ("hill_points".to_string(), OptionValue::Int(3)),
                ("moving".to_string(), OptionValue::Bool(false)),
            ]
        );
        assert!(parse(&words("a --option nokey")).is_err());
        assert!(parse(&words("a --option =1")).is_err());
        assert!(parse(&words("a b --name Two")).is_err());
        assert!(parse(&words("--all-builtins --description x")).is_err());
        assert!(parse(&words("--all-builtins --option a=1")).is_ok());
    }

    #[test]
    fn options_are_checked_against_the_attached_script() {
        let mut v = GameVariant::blank(GameType::Slayer);
        v.set_name("Fists").unwrap();
        let bytes = v.to_bytes().to_vec();
        let with = Settings {
            name: Some("Triple".into()),
            description: Some("Three a kill".into()),
            options: vec![("kill_points".into(), OptionValue::Int(3))],
        };
        let b = port_bytes(&bytes, PortScript::Builtin, &with).unwrap();
        assert_eq!(b.name, "Triple");
        assert_eq!(b.description, "Three a kill");
        let plain = port_bytes(&bytes, PortScript::Builtin, &Settings::default()).unwrap();
        assert_ne!(b.variant().unwrap().ext().script.options, plain.variant().unwrap().ext().script.options);
        let bad = Settings {
            options: vec![("hill_points".into(), OptionValue::Int(3))],
            ..Settings::default()
        };
        let e = port_bytes(&bytes, PortScript::Builtin, &bad).unwrap_err();
        assert_eq!(e.to_string(), "the option values: the script has no option \"hill_points\"");
        assert!(script(Some("keep")).is_err(), "there is no keep path");
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
        let out = port_text(&inputs, &PortScript::Builtin, &Settings::default());
        let ids: Vec<String> = out.iter().map(|(_, r)| r.as_ref().unwrap().id()).collect();
        assert!(ids.iter().all(|i| *i == ids[0]), "{ids:?}");
        let b = out[0].1.as_ref().unwrap();
        assert_eq!(b.script.as_ref().unwrap().name, "King of the Hill");
        // A short file is refused by name.
        let bad = port_text(&[("short".into(), vec![0; 4])], &PortScript::Builtin, &Settings::default());
        assert_eq!(bad[0].1.as_ref().unwrap_err(), &portable::PortableError::Record);
    }

    #[test]
    fn writes_files_by_name_without_clobbering() {
        let dir = std::env::temp_dir().join(format!("h2-variants-port-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let b = port_variant(&king(), PortScript::Builtin, &Settings::default()).unwrap();
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

    const SWAT: &str = "rules \"SWAT\" 1
carrier slayer
scoring carrier
about \"No shields. 50 points to win.\"
settings {
    score_limit = 50
    shields = none
}
";

    #[test]
    fn parses_from_script_words() {
        let p = parse_from_script(&words("swat.rules -o out --option a=1")).unwrap();
        assert_eq!(p.inputs, [PathBuf::from("swat.rules")]);
        assert_eq!(p.out, Some(PathBuf::from("out")));
        assert_eq!(p.options, [("a".to_string(), "1".to_string())]);
        assert!(parse_from_script(&[]).is_err());
        assert!(parse_from_script(&words("a.rules b.rules")).is_err());
        assert!(parse_from_script(&words("a.rules --script keep")).is_err());
        assert!(parse_from_script(&words("a.rules --name X")).is_err());
        assert!(parse_from_script(&words("--all-builtins")).is_err());
    }

    #[test]
    fn from_script_writes_the_scripts_own_variant() {
        let b = from_script_bundle("swat.rules", SWAT, &[]).unwrap();
        assert_eq!(b.name, "SWAT");
        assert_eq!(b.description, "No shields. 50 points to win.");
        assert_eq!(b.game_type, "Slayer");
        let v = b.variant().unwrap();
        assert_eq!(v.get(variants::SettingId(2)), Some(50));
        assert_eq!(v.get(variants::SettingId(19)), Some(1));
        assert_eq!(b.script.as_ref().unwrap().source.as_deref(), Some(SWAT));
        // The same script, the same file.
        assert_eq!(from_script_bundle("swat.rules", SWAT, &[]).unwrap().id(), b.id());

        let dir = std::env::temp_dir().join(format!("h2-variants-from-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let line = write(&dir, &b, &mut Vec::new()).unwrap();
        assert_eq!(line, format!("{}  {}", b.id(), dir.join("SWAT.h2variant").display()));
        let text = std::fs::read_to_string(dir.join("SWAT.h2variant")).unwrap();
        assert_eq!(portable::read(&text).unwrap(), b);
        std::fs::remove_dir_all(&dir).unwrap();

        // Refusals name the file: a compile error, a record the game would
        // not keep, an option the script lacks.
        let e = from_script_bundle("x.rules", "rules \"X\" 1\ncarrier slayer\n", &[]).unwrap_err();
        assert!(e.starts_with("x.rules: "), "{e}");
        let jugg = "rules \"J\" 1\ncarrier juggernaut\nscoring carrier\nsettings { team_play = true }\n";
        assert_eq!(
            from_script_bundle("j.rules", jugg, &[]).unwrap_err(),
            "j.rules: the game would not keep the record: Juggernaut is never team play"
        );
        let e = from_script_bundle("swat.rules", SWAT, &[("nope".into(), OptionValue::Int(1))]).unwrap_err();
        assert_eq!(e, "swat.rules: the option values: the script has no option \"nope\"");
    }
}
