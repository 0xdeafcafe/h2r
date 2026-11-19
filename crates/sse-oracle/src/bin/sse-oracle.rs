use anyhow::{Context, Result, bail};
use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let scratch = std::env::var_os("TMPDIR").context("TMPDIR is required")?;
    let directory =
        PathBuf::from(scratch).join(format!("sse-oracle-native-{}", std::process::id()));
    run(&args, &directory)
}

fn run(args: &[String], directory: &Path) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("fidelity") | Some("build") | Some("query") | Some("generate")
            if !sse_oracle::runner_available() =>
        {
            eprintln!("skipped: requires clang and either x86-64 Linux or macOS with Rosetta 2");
        }
        Some("inventory") if args.len() == 2 => {
            let forms = sse_oracle::inventory(&std::fs::read(&args[1])?)?;
            serde_json::to_writer_pretty(std::io::stdout(), &forms)?;
        }
        Some("regen-fixtures") if args.len() == 3 && args[1] != "--live" => {
            // CORPUS.jsonl OUTPUT.bin.zst: no Rosetta needed.
            let forms =
                sse_oracle::fixtures::select_from_corpus(BufReader::new(File::open(&args[1])?))?;
            write_fixtures(&forms, &args[2])?;
        }
        Some("regen-fixtures") if args.len() == 4 && args[1] == "--live" => {
            // --live INVENTORY.json OUTPUT.bin.zst: explicit Rosetta opt-in.
            if !sse_oracle::rosetta_available() {
                eprintln!("skipped: Rosetta 2 (arch -x86_64) or clang is not available");
                return Ok(());
            }
            let inventory: Vec<sse_oracle::Form> =
                serde_json::from_reader(BufReader::new(File::open(&args[2])?))?;
            let binary = sse_oracle::build_runner(directory)?;
            let mut oracle = sse_oracle::Oracle::spawn(&binary)?;
            let inputs = sse_oracle::fixtures::Inputs::default();
            let mut builder = sse_oracle::fixtures::Builder::default();
            sse_oracle::corpus::generate_filtered(
                &mut oracle,
                &inventory,
                sse_oracle::fixtures::RANDOM_CASES,
                sse_oracle::corpus::SEED,
                |form, case| sse_oracle::fixtures::keep(&form.mnemonic, case),
                |f| builder.add(&inputs, f),
            )?;
            write_fixtures(&builder.finish()?, &args[3])?;
        }
        Some("build") => {
            println!("{}", sse_oracle::build_runner(directory)?.display());
        }
        Some("fidelity") => {
            let binary = sse_oracle::build_runner(directory)?;
            let mut oracle = sse_oracle::Oracle::spawn(&binary)?;
            serde_json::to_writer_pretty(
                std::io::stdout(),
                &sse_oracle::fidelity::check(&mut oracle)?,
            )?;
        }
        Some("audit") if args.len() == 2 => {
            let audit = sse_oracle::corpus::audit(BufReader::new(File::open(&args[1])?))?;
            serde_json::to_writer_pretty(std::io::stdout(), &audit)?;
        }
        Some("query") if args.len() == 2 => {
            #[derive(serde::Deserialize)]
            struct Query {
                instruction: Vec<u8>,
                state: sse_oracle::State,
            }
            let query: Query = serde_json::from_reader(BufReader::new(File::open(&args[1])?))?;
            let binary = sse_oracle::build_runner(directory)?;
            let mut oracle = sse_oracle::Oracle::spawn(&binary)?;
            serde_json::to_writer_pretty(
                std::io::stdout(),
                &oracle.query(&query.instruction, &query.state)?,
            )?;
        }
        Some("generate") if args.len() >= 3 => {
            let forms: Vec<sse_oracle::Form> =
                serde_json::from_reader(BufReader::new(File::open(&args[1])?))?;
            let path = PathBuf::from(&args[2]);
            // Never create derived outputs on the disc, even via a symlink.
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new("."));
            let parent = parent.canonicalize()?;
            if parent.components().any(|c| c.as_os_str() == "disc") || path.is_symlink() {
                bail!("refusing disc output or symlink");
            }
            let random = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(64);
            let binary = sse_oracle::build_runner(directory)?;
            let mut oracle = sse_oracle::Oracle::spawn(&binary)?;
            let mut output =
                BufWriter::new(File::options().write(true).create_new(true).open(&path)?);
            let counts = sse_oracle::corpus::generate(
                &mut oracle,
                &forms,
                &mut output,
                random,
                0x243f6a8885a308d3,
            )?;
            output.flush()?;
            serde_json::to_writer_pretty(std::io::stdout(), &counts)?;
        }
        _ => bail!(
            "usage: sse-oracle inventory XBE | audit CORPUS.jsonl | regen-fixtures CORPUS.jsonl OUT.bin.zst\n  x86-64 Linux or Rosetta: build | fidelity | query REQUEST.json | generate INVENTORY OUTPUT.jsonl [RANDOM_CASES]\n  Rosetta fixture source: regen-fixtures --live INVENTORY.json OUT.bin.zst"
        ),
    }
    Ok(())
}
fn write_fixtures(forms: &[sse_oracle::fixtures::FormFixtures], out: &str) -> Result<()> {
    let path = PathBuf::from(out);
    if path
        .canonicalize()
        .unwrap_or(path.clone())
        .components()
        .any(|c| c.as_os_str() == "disc")
        || path.is_symlink()
    {
        bail!("refusing disc output or symlink");
    }
    let raw = sse_oracle::fixtures::encode(forms);
    let packed = sse_oracle::fixtures::compress(&raw)?;
    let cases: usize = forms.iter().map(|f| f.cases.len()).sum();
    std::fs::write(&path, &packed)?;
    eprintln!(
        "{} forms, {} cases, {} fixtures; {} raw bytes, {} compressed",
        forms.len(),
        cases,
        cases * 8,
        raw.len(),
        packed.len()
    );
    Ok(())
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
#[test]
fn build_subcommand_produces_a_linux_x86_64_executable() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("sse-cli-build-{}", std::process::id()));
    std::fs::create_dir(&dir)?;
    run(&["build".to_owned()], &dir)?;
    let elf = std::fs::read(dir.join("sse-runner"))?;
    assert_eq!(&elf[..6], b"\x7fELF\x02\x01"); // ELF64, little endian
    assert_eq!(u16::from_le_bytes([elf[18], elf[19]]), 62); // EM_X86_64
    std::fs::remove_dir_all(dir)?;
    Ok(())
}
