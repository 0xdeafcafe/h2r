//! The client: `h2 studio <call> [args]` and `h2-editor call <call> [args]`
//! are this. One call per run, or a script of calls on one connection
//! (`-`), as `h2 agent` does.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_proto::{CallError, Client};
use anyhow::{Context, Result, bail};
use serde_json::Value;

/// The usage text, `prog` being how the user ran it (`h2 studio`).
pub fn usage(prog: &str) -> String {
    let mut s = format!(
        "usage:
  {prog} <call> [args] [--socket PATH] [--timeout SECONDS]
  {prog} - [--socket PATH]     calls from stdin, one per line, on one connection

Talks to a running h2-editor: the window (which listens while it is open)
or `h2-editor --serve` (headless, no window). A call is a method and JSON
params (`scenario.set '{{\"block\":\"vehicles\",\"index\":0,\"field\":\"position\",\"value\":[1,2,3]}}'`)
or a short form:
"
    );
    for (form, what) in crate::short::FORMS {
        s.push_str(&format!("  {form:<32} {what}\n"));
    }
    s.push_str(
        "
The socket is --socket, else $H2_STUDIO, else studio.sock in the cache
directory (beside the game's agent.sock). Results print as JSON, one per line
in scripts; an error prints to stderr and exits 1 (a script stops at its
first error). docs/editor/agent-api.md has every method.",
    );
    s
}

/// Runs the client over `args` (everything after `h2 studio`).
pub fn run(prog: &str, args: Vec<String>) -> Result<()> {
    let mut socket = None;
    let mut timeout = Some(Duration::from_secs(120));
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--socket" => socket = Some(PathBuf::from(it.next().context("--socket needs a path")?)),
            "--timeout" => {
                let s: f64 = it
                    .next()
                    .context("--timeout needs seconds")?
                    .parse()
                    .context("--timeout needs seconds")?;
                timeout = Duration::try_from_secs_f64(s).ok().filter(|d| !d.is_zero());
            }
            "-h" | "--help" => {
                println!("{}", usage(prog));
                return Ok(());
            }
            _ => rest.push(a),
        }
    }
    if rest.is_empty() {
        bail!("{}", usage(prog));
    }
    let path = socket.unwrap_or_else(crate::default_socket);
    let mut c = connect(&path)?;
    c.set_timeout(timeout)?;
    if rest[0] == "-" {
        return script(&mut c);
    }
    let (m, p) = crate::parse_call(&rest).map_err(anyhow::Error::msg)?;
    let v = call(&mut c, &m, p)?;
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

/// Connects to the studio at `path` (a Unix socket only: the studio never
/// listens on TCP).
pub fn connect(path: &Path) -> Result<Client> {
    if path.to_str().is_some_and(|s| s.starts_with("tcp:")) {
        bail!(
            "the studio listens on a local Unix socket only, not {}",
            path.display()
        );
    }
    Client::connect_to(&agent_proto::Endpoint::Unix(path.to_path_buf())).with_context(|| {
        format!(
            "connecting to {}: is h2-editor open (or `h2-editor --serve`) running?",
            path.display()
        )
    })
}

fn call(c: &mut Client, m: &str, p: Value) -> Result<Value> {
    match c.call(m, p) {
        Ok(v) => Ok(v),
        Err(CallError::Rpc(e)) => {
            eprintln!("{}", serde_json::to_string_pretty(&e)?);
            std::process::exit(1);
        }
        Err(e) => Err(anyhow::anyhow!("{m}: {e}")),
    }
}

fn script(c: &mut Client) -> Result<()> {
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words = crate::split(line).map_err(anyhow::Error::msg)?;
        let (m, p) = crate::parse_call(&words).map_err(anyhow::Error::msg)?;
        let v = call(c, &m, p)?;
        println!("{}", serde_json::to_string(&v)?);
    }
    Ok(())
}
