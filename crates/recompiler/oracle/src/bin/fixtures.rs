use oracle::{
    Fault, Stop,
    fixtures::{self, Encoding, Inputs},
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .ok_or("TMPDIR must be set (or set it before invoking the generator)")?
        .join("oracle-fixtures.jsonl");
    let mut random = 16usize;
    let mut seed = fixtures::DEFAULT_SEED;
    let mut list = fixtures::encodings();
    let mut small = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => output = args.next().ok_or("--out requires a path")?.into(),
            "--random" => random = args.next().ok_or("--random requires a count")?.parse()?,
            "--seed" => {
                seed = args
                    .next()
                    .ok_or("--seed requires a decimal u64")?
                    .parse()?
            }
            "--encodings" => {
                list = serde_json::from_reader::<_, Vec<Encoding>>(File::open(
                    args.next().ok_or("--encodings requires a JSON path")?,
                )?)?
            }
            "--small-out" => {
                small = Some(PathBuf::from(
                    args.next().ok_or("--small-out requires a path")?,
                ))
            }
            "--help" => {
                println!(
                    "oracle-fixtures [--out PATH] [--random N] [--seed U64] [--encodings JSON] [--small-out PATH]"
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    let mut writer = BufWriter::new(File::create(&output)?);
    let mut inputs = Inputs::new(seed);
    let mut statuses = BTreeMap::<String, usize>::new();
    let mut selected = Vec::new();
    let mut total = 0usize;
    for encoding in &list {
        for n in 0..fixtures::EDGES.len() + random {
            let edge = (n < fixtures::EDGES.len()).then_some(n);
            let (initial, memory) = inputs.state(edge, encoding);
            let case = match edge {
                Some(n) => format!("edge-{n}"),
                None => format!("random-{}", n - fixtures::EDGES.len()),
            };
            let fixture = fixtures::evaluate(encoding, case, seed, initial, memory);
            if matches!(
                fixture.outcome.stop,
                Stop::BackendError(_) | Stop::BackendUnavailable | Stop::InvalidInput(_)
            ) {
                return Err(format!("{}: {:?}", encoding.name, fixture.outcome.stop).into());
            }
            let status = match &fixture.outcome.stop {
                Stop::Completed => "completed".into(),
                Stop::Fault(Fault::PageFault {
                    access, protection, ..
                }) => format!("page-fault-{access:?}-protection-{protection}"),
                Stop::Fault(f) => format!("{f:?}"),
                other => format!("{other:?}"),
            };
            *statuses.entry(status).or_default() += 1;
            serde_json::to_writer(&mut writer, &fixture)?;
            writeln!(writer)?;
            // Small public corpus: deliberate edge cases from named instruction families.
            if (n == 0
                && [
                    "add-32",
                    "adc-32",
                    "sbb-32",
                    "bsf-32",
                    "bsr-32",
                    "mov-ah",
                    "rep-movsb",
                    "ud2",
                    "unmapped-read",
                    "unmapped-write",
                    "x87-add",
                    "sse-addss",
                ]
                .contains(&encoding.name.as_str()))
                || (n == 2 && ["idiv-32", "div-32"].contains(&encoding.name.as_str()))
                || (n == 3 && encoding.name == "rep-movsb")
                || (n == 24
                    && [
                        "shl-32-imm-0",
                        "shl-32-imm-1",
                        "shl-32-imm-32",
                        "rcl-8-imm-9",
                    ]
                    .contains(&encoding.name.as_str()))
            {
                selected.push(fixture);
            }
            total += 1;
        }
    }
    writer.flush()?;
    if let Some(path) = small {
        let mut writer = BufWriter::new(File::create(path)?);
        // One compact object per line inside a JSON array keeps byte arrays
        // readable and the public smoke corpus small enough to review.
        writeln!(writer, "[")?;
        for (n, fixture) in selected.iter().enumerate() {
            if n != 0 {
                writeln!(writer, ",")?;
            }
            serde_json::to_writer(&mut writer, fixture)?;
        }
        writeln!(writer, "\n]")?;
    }
    println!(
        "{total} fixtures from {} encodings; seed {seed}; {} hand-picked\n{}",
        list.len(),
        selected.len(),
        output.display()
    );
    println!("{}", serde_json::to_string_pretty(&statuses)?);
    Ok(())
}
