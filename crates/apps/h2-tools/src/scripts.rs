//! Disc-backed HaloScript inspection. Decompiled source goes to stdout only.
use anyhow::{Context, Result, bail};
use std::{
    fs::File,
    io::{self, BufReader, Write},
    path::{Path, PathBuf},
};

const USAGE: &str = "usage: h2 hs <map-name-or-file> [--script name] [--disc disc.iso] [--builtins]\n\
       h2 hs --all [--disc disc.iso]\n\
--builtins prints reached builtin signatures; --all checks every disc map\n\
(codec symmetry plus a source node-count check; no source is printed)";

#[derive(Debug, Default)]
struct Options {
    map: Option<String>,
    script: Option<String>,
    disc: Option<PathBuf>,
    builtins: bool,
    all: bool,
    help: bool,
}
impl Options {
    fn parse(args: Vec<String>) -> Result<Self> {
        let mut result = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--script" if result.script.is_none() => {
                    result.script = Some(args.next().context("--script requires a name")?)
                }
                "--disc" if result.disc.is_none() => {
                    result.disc = Some(PathBuf::from(
                        args.next().context("--disc requires a path")?,
                    ))
                }
                "--builtins" if !result.builtins => result.builtins = true,
                "--all" if !result.all => result.all = true,
                "--help" | "-h" => result.help = true,
                a if a.starts_with('-') => bail!("unknown or repeated option {a}\n{USAGE}"),
                _ if result.map.is_none() => result.map = Some(arg),
                _ => bail!("{USAGE}"),
            }
        }
        if !result.help
            && (result.all && (result.map.is_some() || result.script.is_some() || result.builtins)
                || !result.all && result.map.is_none())
        {
            bail!("{USAGE}");
        }
        Ok(result)
    }
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let options = Options::parse(args)?;
    finish_output(run_to(options, &mut io::stdout().lock()))
}
fn finish_output(result: Result<()>) -> Result<()> {
    match result {
        Err(e)
            if e.downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe) =>
        {
            Ok(())
        }
        other => other,
    }
}
fn run_to(options: Options, out: &mut impl Write) -> Result<()> {
    if options.help {
        writeln!(out, "{USAGE}")?;
        return Ok(());
    }
    let disc = options.disc.map(Ok).unwrap_or_else(super::default_disc)?;
    let bytes = super::load_xbe(&disc)?;
    let xbe = xbe::Xbe::parse(&bytes)?;
    let mut catalog = hs::Catalog::from_xbe(&xbe)?;
    if options.all {
        return check_all(&disc, &mut catalog, out);
    }
    let name = options.map.context(USAGE)?;
    let mut map = super::maps::load_map(&name, Some(disc))?;
    let scenario = hs::Scenario::read(&mut map)?;
    catalog.observe(&scenario)?;
    write_scenario(
        &scenario,
        &catalog,
        options.script.as_deref(),
        options.builtins,
        out,
    )
}
fn write_scenario(
    scenario: &hs::Scenario,
    catalog: &hs::Catalog,
    script: Option<&str>,
    builtins: bool,
    out: &mut impl Write,
) -> Result<()> {
    if !builtins {
        let source = scenario.decompile(catalog, script)?;
        out.write_all(source.source.as_bytes())?;
        return Ok(());
    }
    let roots = if let Some(name) = script {
        vec![scenario.script(name)?.root()]
    } else {
        scenario.roots().collect()
    };
    writeln!(
        out,
        "Static reachability from selected definitions (includes both conditional branches):"
    )?;
    for index in scenario.reached_builtins(roots)? {
        let f = catalog.function(index)?;
        let params = f
            .parameters()
            .iter()
            .map(|&t| catalog.type_name(t))
            .collect::<hs::Result<Vec<_>>>()?
            .join(", ");
        writeln!(
            out,
            "{index:3}  {} ({params}) -> {}  eval={:#010x}{}",
            f.name().unwrap_or("<unnamed>"),
            catalog.type_name(f.return_type())?,
            f.evaluate(),
            if f.flags() & 2 != 0 {
                " [custom arguments]"
            } else {
                ""
            }
        )?;
    }
    Ok(())
}
fn check_all(disc: &Path, catalog: &mut hs::Catalog, out: &mut impl Write) -> Result<()> {
    let mut iso = super::open_iso(disc)?;
    let mut totals = [0usize; 8];
    for e in iso
        .entries()?
        .iter()
        .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
    {
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let mut map =
            map::Map::from_region(BufReader::new(File::open(disc)?), base, e.size.into())?;
        let check =
            hs::check_map(&mut map, catalog).with_context(|| format!("checking {}", e.path))?;
        let v = &check.validation;
        writeln!(
            out,
            "{} slots={} live={} emitted={} unrooted={}",
            e.path, v.slots, v.live_nodes, check.emitted_nodes, v.unrooted_live_nodes
        )?;
        for (total, value) in totals.iter_mut().zip([
            1,
            v.slots,
            v.live_nodes,
            check.scripts,
            check.globals,
            check.references,
            check.emitted_nodes,
            v.unrooted_live_nodes,
        ]) {
            *total += value;
        }
    }
    if totals[0] == 0 {
        bail!("disc contains no maps");
    }
    writeln!(
        out,
        "TOTAL maps={} slots={} live={} scripts={} globals={} refs={} emitted={} unrooted={}",
        totals[0], totals[1], totals[2], totals[3], totals[4], totals[5], totals[6], totals[7]
    )?;
    writeln!(
        out,
        "{} builtin descriptors; {} names; {} types; codec symmetry and source node-count checks pass",
        catalog.functions().len(),
        catalog.named_functions().len(),
        catalog.types().len()
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_arguments_fail_before_disc_io() {
        for args in [
            vec![],
            vec!["--script"],
            vec!["--disc"],
            vec!["a", "b"],
            vec!["--unknown"],
            vec!["--all", "a"],
            vec!["--all", "--builtins"],
            vec!["--all", "--script", "x"],
        ] {
            assert!(Options::parse(args.into_iter().map(str::to_owned).collect()).is_err());
        }
        assert!(Options::parse(vec!["--all".into()]).unwrap().all);
    }
    #[test]
    fn usage_documents_all_options() {
        for flag in ["--all", "--builtins", "--disc", "--script"] {
            assert!(USAGE.contains(flag));
            assert!(super::super::USAGE.contains(flag));
        }
    }
    struct FailedWriter(io::ErrorKind);
    impl Write for FailedWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(self.0))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn broken_pipe_is_success_but_other_write_failures_propagate() {
        for (kind, success) in [
            (io::ErrorKind::BrokenPipe, true),
            (io::ErrorKind::PermissionDenied, false),
        ] {
            let options = Options {
                help: true,
                ..Options::default()
            };
            assert_eq!(
                finish_output(run_to(options, &mut FailedWriter(kind))).is_ok(),
                success
            );
        }
    }
    fn synthetic_xbe() -> Vec<u8> {
        fn put(b: &mut [u8], at: usize, value: u32) {
            b[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut b = vec![0; 0x280400];
        b[..4].copy_from_slice(b"XBEH");
        for (at, value) in [
            (0x104, 0x10000),
            (0x108, 0x400),
            (0x10c, 0x480000),
            (0x118, 0x10200),
            (0x11c, 1),
            (0x120, 0x10300),
            (0x128, 0x002d0aee ^ 0xa8fc57ab),
            (0x208, 0x4d530064),
            (0x2ac, 3),
            (0x300, 4),
            (0x304, 0x200000),
            (0x308, 0x280000),
            (0x30c, 0x400),
            (0x310, 0x280000),
            (0x314, 0x10380),
        ] {
            put(&mut b, at, value);
        }
        let off = |a: u32| (a - 0x200000 + 0x400) as usize;
        b[off(0x20994d)..off(0x20994d) + 7]
            .copy_from_slice(&[0x8b, 0x14, 0x8d, 0xe0, 0x44, 0x47, 0]);
        put(&mut b, off(hs::FUNCTION_COUNT), 911);
        for i in 0..911 {
            put(&mut b, off(hs::FUNCTION_TABLE + i * 4), 0x460000);
        }
        for i in 0..62 {
            put(&mut b, off(hs::TYPE_NAMES + i * 4), 0x460040);
        }
        for i in 0..6 {
            put(&mut b, off(hs::SCRIPT_KIND_NAMES + i * 4), 0x460060);
        }
        for i in 0..0x41d {
            put(&mut b, off(hs::EXTERNAL_GLOBAL_TABLE + i * 4), 0x460020);
        }
        put(&mut b, off(0x460000), 4 | (2 << 16)); // return u16 and flags u16
        put(&mut b, off(0x460004), 0x200010); // synthetic executable pointer
        put(&mut b, off(0x460008), 0x460080); // help string, not name
        put(&mut b, off(0x46000c), 1 | (9 << 16)); // count and first parameter
        put(&mut b, off(0x460020), 5);
        for (address, s) in [
            (0x460040, "invented_type"),
            (0x460060, "invented_kind"),
            (0x460080, "invented argument help"),
        ] {
            b[off(address)..off(address) + s.len()].copy_from_slice(s.as_bytes());
        }
        b
    }
    // Invented acyclic graph with shared argument tails. Rendering exceeds its
    // expansion budget, while reachability is small and must remain usable.
    fn shared_graph() -> (hs::Scenario, hs::Catalog) {
        fn word(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        fn datum(slot: usize) -> u32 {
            ((0x8000 + slot as u32) << 16) | slot as u32
        }
        let mut nodes = Vec::new();
        let mut node = |slot: usize, head: bool, next: u32, child: u32| {
            let mut bytes = [0; 20];
            bytes[..2].copy_from_slice(&((0x8000 + slot) as u16).to_le_bytes());
            bytes[4..6].copy_from_slice(&(if head { 2u16 } else { 4 }).to_le_bytes());
            bytes[6..8].copy_from_slice(&(if head { 9u16 } else { 8 }).to_le_bytes());
            word(&mut bytes, 8, next);
            word(&mut bytes, 16, child);
            nodes.extend(bytes);
        };
        node(0, false, u32::MAX, datum(1));
        node(1, true, datum(2), 0);
        for level in 0..10 {
            let a = 2 + level * 3;
            node(a, false, datum(a + 1), datum(a + 2));
            node(a + 1, false, u32::MAX, datum(a + 2));
            node(
                a + 2,
                true,
                if level == 9 { u32::MAX } else { datum(a + 3) },
                0,
            );
        }
        let mut script = vec![0; 40];
        script[..8].copy_from_slice(b"invented");
        script[34..36].copy_from_slice(&4u16.to_le_bytes());
        word(&mut script, 36, datum(0));
        let base = 0x81000000;
        let mut payload = vec![0; 0x240];
        for (offset, count, data) in [
            (0x238, 32, nodes),
            (0x1b0, 2, b"f\0".to_vec()),
            (0x1b8, 1, script),
        ] {
            while !payload.len().is_multiple_of(4) {
                payload.push(0);
            }
            let address = base + payload.len() as u32;
            word(&mut payload, offset, count);
            word(&mut payload, offset + 4, address);
            payload.extend(data);
        }
        let scenario = hs::Scenario::decode(&payload, base).unwrap();
        let bytes = synthetic_xbe();
        let mut catalog = hs::Catalog::from_xbe(&xbe::Xbe::parse(&bytes).unwrap()).unwrap();
        catalog.observe(&scenario).unwrap();
        (scenario, catalog)
    }
    #[test]
    fn builtins_does_not_render_shared_source_and_handles_broken_pipe() {
        let (scenario, catalog) = shared_graph();
        let error = scenario.decompile(&catalog, None).unwrap_err().to_string();
        assert!(error.contains("expansion"), "{error}");
        let mut output = Vec::new();
        write_scenario(&scenario, &catalog, None, true, &mut output).unwrap();
        assert!(String::from_utf8(output).unwrap().contains("f ("));
        for (kind, success) in [
            (io::ErrorKind::BrokenPipe, true),
            (io::ErrorKind::PermissionDenied, false),
        ] {
            let result = write_scenario(&scenario, &catalog, None, true, &mut FailedWriter(kind));
            assert_eq!(finish_output(result).is_ok(), success);
        }
        assert!(
            write_scenario(&scenario, &catalog, Some("missing"), true, &mut Vec::new()).is_err()
        );
    }
}
