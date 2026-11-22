//! One light, playerless game process per Live host lease.

use anyhow::{Result, bail};

#[cfg(unix)]
mod lifecycle;
#[cfg(unix)]
mod live;
#[cfg(unix)]
mod pack;
#[cfg(unix)]
mod process;
#[cfg(unix)]
mod sim_run;
#[cfg(unix)]
mod supervisor;

const USAGE: &str = "usage:
  h2 host run --lease ID --live URL --data-sha SHA256
              [--region REGION] [--base-port 31000] [--tunnel-listen 127.0.0.1:31100]
              [--out DIR] [--data DIR] [--film FILE] [--archive dual|film]
  h2 host pack [--control-listen 127.0.0.1:31099] [--max-matches 5]
               [--vcpus N] [--memory-mib MIB] [--linger-s 60] [--data DIR]
  h2 host bundle --disc ISO --out DIR

Supply H2_TOKEN and H2_DATA_URL in the host environment.
See docs/live/dedicated-host.md for other boot inputs.";

pub fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("run") => {
            #[cfg(unix)]
            return supervisor::run(args);
            #[cfg(not(unix))]
            bail!("h2 host run requires Unix process and socket support");
        }
        Some("pack") => {
            #[cfg(unix)]
            return pack::run(args);
            #[cfg(not(unix))]
            bail!("h2 host pack requires Unix process and socket support");
        }
        // dh-provision supplies the host-data dependency and bundle entry point.
        Some("bundle") => host_data::bundle_cmd(args.collect()),
        None | Some("-h" | "--help") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}
