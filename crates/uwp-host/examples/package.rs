//! Writes the Xbox package's `AppxManifest.xml` and images into a staging
//! directory and prints the package's identity (platform/xbox/build.sh):
//!
//!     cargo run -p uwp-host --example package -- <stage-dir> [--version a.b.c.d] [--publisher CN=...]
//!
//! Prints `family_name=...` and `aumid=...` lines.

use anyhow::{Context, Result, bail};
use uwp_host::manifest::{self, Manifest};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut m = Manifest::default();
    let mut dir = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--version" => m.version = args.next().context("--version needs a.b.c.d")?,
            "--publisher" => m.publisher = args.next().context("--publisher needs a subject")?,
            _ if dir.is_none() => dir = Some(std::path::PathBuf::from(a)),
            _ => bail!("unexpected argument {a}"),
        }
    }
    let Some(dir) = dir else { bail!("usage: package <stage-dir> [--version a.b.c.d] [--publisher CN=...]") };
    manifest::write(&dir, &m)?;
    println!("family_name={}", m.family_name());
    println!("aumid={}", m.aumid());
    Ok(())
}
