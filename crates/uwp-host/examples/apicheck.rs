//! Lists the functions the given binaries import that the app-container
//! umbrella libraries do not export (`uwp_host::imports`;
//! platform/xbox/build.sh):
//!
//!     cargo run -p uwp-host --example apicheck -- <OneCoreUAP.lib>... -- <h2uwp.exe> <h2game.dll>...
//!
//! Runs `llvm-nm` and `llvm-readobj` from `H2_LLVM` (platform/windows/env.sh)
//! or `PATH`. Exits 1 when anything is outside.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use uwp_host::imports;

fn tool(name: &str) -> PathBuf {
    std::env::var_os("H2_LLVM").map(|d| Path::new(&d).join(name)).filter(|p| p.is_file()).unwrap_or_else(|| name.into())
}

fn run(tool_name: &str, args: &[&str], file: &Path) -> Result<String> {
    let o = Command::new(tool(tool_name)).args(args).arg(file).output().with_context(|| format!("running {tool_name}"))?;
    if !o.status.success() {
        bail!("{tool_name} {}: {}", file.display(), String::from_utf8_lossy(&o.stderr));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

fn main() -> Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let split = args.iter().position(|a| a.as_os_str() == "--").context("usage: apicheck <lib>... -- <binary>...")?;
    let (libs, bins) = (&args[..split], &args[split + 1..]);
    let mut allowed = BTreeSet::new();
    for lib in libs {
        allowed.extend(imports::parse_nm(&run("llvm-nm", &["--just-symbol-name"], lib)?));
    }
    let mut bad = 0;
    for bin in bins {
        let all = imports::parse_readobj(&run("llvm-readobj", &["--coff-imports"], bin)?);
        let out = imports::outside(&all, &allowed);
        println!("apicheck: {}: {} imports, {} outside the app-container libraries", bin.display(), all.len(), out.len());
        for (dll, f) in &out {
            println!("apicheck:   {dll} {f}");
        }
        bad += out.len();
    }
    if bad > 0 {
        std::process::exit(1);
    }
    Ok(())
}
